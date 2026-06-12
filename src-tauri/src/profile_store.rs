//! `Profile` + `ProfileStore`: non-secret account metadata persisted to
//! `<config>/pitstopx/profiles.json`, plus the capture/switch logic. Secrets
//! (the blob) live in the `SecretStore`, never here.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude_source::ClaudeSource;
use crate::credentials::{ClaudeConfig, CredentialBlob};
use crate::secrets::SecretStore;

/// Non-secret metadata for one saved account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub email: String,
    pub saved_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
    /// Verbatim `oauthAccount` object from `~/.claude.json`.
    #[serde(default)]
    pub oauth_account: Value,
}

impl Profile {
    /// Derived display label, e.g. `"Acme AI · Team · 5x"`.
    ///
    /// Joins, with `" · "`: the org name (dropping the auto-generated
    /// `"<email>'s Organization"`), the capitalized `subscription_type`, and the
    /// tier suffix after `max_` (`"5x"`/`"20x"`).
    pub fn plan_label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if let Some(org) = self
            .oauth_account
            .get("organizationName")
            .and_then(Value::as_str)
        {
            let auto = format!("{}'s Organization", self.email);
            if !org.is_empty() && org != auto {
                parts.push(org.to_string());
            }
        }

        if let Some(sub) = &self.subscription_type {
            parts.push(capitalize(sub));
        }

        if let Some(tier) = &self.rate_limit_tier {
            if let Some(suffix) = tier.strip_prefix("max_") {
                parts.push(suffix.to_string());
            }
        }

        if parts.is_empty() {
            self.email.clone()
        } else {
            parts.join(" · ")
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Owns persistence of profiles and the capture/switch operations. Holds the
/// two platform abstractions.
pub struct ProfileStore {
    secrets: Box<dyn SecretStore>,
    source: Box<dyn ClaudeSource>,
}

impl ProfileStore {
    pub fn new(secrets: Box<dyn SecretStore>, source: Box<dyn ClaudeSource>) -> Self {
        Self { secrets, source }
    }

    fn identity_config(&self) -> ClaudeConfig {
        ClaudeConfig::at(self.source.identity_path())
    }

    /// Load the saved profile list from disk (empty when the file is absent).
    pub fn load(&self) -> Result<Vec<Profile>> {
        let path = crate::paths::profiles_file()?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&path)?;
        let profiles = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(profiles)
    }

    fn save(&self, profiles: &[Profile]) -> Result<()> {
        let path = crate::paths::profiles_file()?;
        let bytes = serde_json::to_vec_pretty(profiles)?;
        crate::claude_source::atomic_write(&path, &bytes)
    }

    /// The active account email, per `~/.claude.json`.
    pub fn active_email(&self) -> Result<Option<String>> {
        self.identity_config().active_email()
    }

    /// Read a profile's credential blob — the *live* item for the active
    /// account, the *saved* item otherwise.
    pub async fn blob_for(&self, email: &str, is_active: bool) -> Result<Option<Vec<u8>>> {
        if is_active {
            self.source.read_live_blob().await
        } else {
            self.secrets.read(email).await
        }
    }

    /// Persist a freshly-refreshed blob back to the correct store.
    pub async fn store_refreshed_blob(
        &self,
        email: &str,
        is_active: bool,
        blob: &[u8],
    ) -> Result<()> {
        if is_active {
            self.source.write_live_blob(blob).await
        } else {
            self.secrets.upsert(email, blob).await
        }
    }

    /// Snapshot the live account (blob + identity) into a saved profile.
    /// Short-circuits when nothing changed (blob + `oauthAccount` byte-equal)
    /// to avoid needless secret-store / file writes. Returns the captured email
    /// (or `None` when there is nothing to save).
    pub async fn capture_current(&self) -> Result<Option<String>> {
        let Some(live_bytes) = self.source.read_live_blob().await? else {
            return Ok(None);
        };
        let blob =
            CredentialBlob::parse(&live_bytes).context("live credential blob is malformed")?;

        let config = self.identity_config();
        let Some(oauth_account) = config.oauth_account()? else {
            return Ok(None);
        };
        let email = oauth_account
            .get("emailAddress")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("active identity has no emailAddress"))?
            .to_string();

        let mut profiles = self.load()?;

        // Short-circuit: skip writes when the saved copy already matches.
        if let Some(existing) = profiles.iter().find(|p| p.email == email) {
            let saved_blob = self.secrets.read(&email).await?;
            let blob_same = saved_blob
                .as_deref()
                .map(|b| blob.equals_bytes(b))
                .unwrap_or(false);
            if blob_same && existing.oauth_account == oauth_account {
                return Ok(Some(email));
            }
        }

        // Persist the (possibly refreshed) blob and the metadata.
        self.secrets.upsert(&email, &live_bytes).await?;

        let creds = blob.credentials().ok();
        let profile = Profile {
            email: email.clone(),
            saved_at: Utc::now(),
            subscription_type: creds.as_ref().and_then(|c| c.subscription_type.clone()),
            rate_limit_tier: creds.as_ref().and_then(|c| c.rate_limit_tier.clone()),
            oauth_account,
        };

        match profiles.iter_mut().find(|p| p.email == email) {
            Some(slot) => *slot = profile,
            None => profiles.push(profile),
        }
        self.save(&profiles)?;
        Ok(Some(email))
    }

    /// Switch the live login to `email`'s saved account.
    ///
    /// Captures the current account first; a failed snapshot **aborts** the
    /// switch so the outgoing refresh token can't be lost. Then writes the
    /// saved blob into the live location and restores `oauthAccount`.
    pub async fn switch_to(&self, email: &str) -> Result<()> {
        // Capture-first guard.
        self.capture_current()
            .await
            .context("aborting switch: failed to snapshot the current account")?;

        let saved_blob = self
            .secrets
            .read(email)
            .await?
            .ok_or_else(|| anyhow!("no saved credentials for {email}"))?;
        // Validate before writing.
        CredentialBlob::parse(&saved_blob)
            .with_context(|| format!("saved blob for {email} is malformed"))?;

        let profiles = self.load()?;
        let profile = profiles
            .iter()
            .find(|p| p.email == email)
            .ok_or_else(|| anyhow!("no saved profile for {email}"))?;

        // 1) Write the live credential blob.
        self.source.write_live_blob(&saved_blob).await?;
        // 2) Restore identity (only oauthAccount changes; atomic whole-file write).
        self.identity_config()
            .set_oauth_account(&profile.oauth_account)?;
        Ok(())
    }

    /// Delete a saved account: secret item + profile entry.
    pub async fn remove(&self, email: &str) -> Result<()> {
        self.secrets.delete(email).await?;
        let mut profiles = self.load()?;
        profiles.retain(|p| p.email != email);
        self.save(&profiles)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile_with(org: &str, sub: Option<&str>, tier: Option<&str>) -> Profile {
        Profile {
            email: "user@example.com".into(),
            saved_at: Utc::now(),
            subscription_type: sub.map(str::to_string),
            rate_limit_tier: tier.map(str::to_string),
            oauth_account: json!({ "organizationName": org }),
        }
    }

    #[test]
    fn plan_label_joins_parts() {
        let p = profile_with("Acme AI", Some("team"), Some("max_5x"));
        assert_eq!(p.plan_label(), "Acme AI · Team · 5x");
    }

    #[test]
    fn plan_label_drops_auto_org() {
        let p = profile_with(
            "user@example.com's Organization",
            Some("max"),
            Some("max_20x"),
        );
        assert_eq!(p.plan_label(), "Max · 20x");
    }

    #[test]
    fn plan_label_falls_back_to_email() {
        let mut p = profile_with("", None, None);
        p.oauth_account = json!({});
        assert_eq!(p.plan_label(), "user@example.com");
    }
}
