//! `Profile` + `ProfileStore`: non-secret account metadata persisted to
//! `<config>/pitstopx/profiles.json`, plus capture/switch logic across every
//! provider. Secrets (the credential blob) live in the `SecretStore`, never
//! here. Provider-specific credential locations live behind `AccountSource`.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::Provider;
use crate::secrets::SecretStore;
use crate::source::{secret_key, AccountSource};

/// Non-secret metadata for one saved account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub email: String,
    pub saved_at: DateTime<Utc>,
    /// The model provider this account belongs to. Defaults to Anthropic so
    /// profiles saved before providers existed load correctly.
    #[serde(default)]
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
    /// Provider-shaped identity object restored on switch.
    #[serde(default)]
    pub oauth_account: Value,
}

impl Profile {
    /// Canonical key used across the secret store and the in-memory caches.
    pub fn key(&self) -> String {
        secret_key(self.provider, &self.email)
    }

    /// Derived display label, e.g. `"Acme AI · Team · 5x"` (Anthropic) or
    /// `"Pro"` (OpenAI/Codex).
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

/// Owns persistence of profiles and the capture/switch operations across all
/// providers.
pub struct ProfileStore {
    secrets: Box<dyn SecretStore>,
    sources: Vec<Box<dyn AccountSource>>,
}

impl ProfileStore {
    pub fn new(secrets: Box<dyn SecretStore>, sources: Vec<Box<dyn AccountSource>>) -> Self {
        Self { secrets, sources }
    }

    fn source_for(&self, provider: Provider) -> Result<&dyn AccountSource> {
        self.sources
            .iter()
            .find(|s| s.provider() == provider)
            .map(|b| b.as_ref())
            .ok_or_else(|| anyhow!("no source for provider {}", provider.id()))
    }

    /// All account sources (for diagnostics like `--print-paths`).
    pub fn sources(&self) -> &[Box<dyn AccountSource>] {
        &self.sources
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

    /// The currently-active account for each provider, as `(provider, email)`.
    pub async fn active_accounts(&self) -> Vec<(Provider, String)> {
        let mut out = Vec::new();
        for source in &self.sources {
            if let Ok(Some(live)) = source.read_live().await {
                out.push((source.provider(), live.identity.email));
            }
        }
        out
    }

    /// Read a profile's credential blob — the live item for the active account,
    /// the saved item otherwise.
    pub async fn blob_for(
        &self,
        provider: Provider,
        email: &str,
        is_active: bool,
    ) -> Result<Option<Vec<u8>>> {
        if is_active {
            Ok(self
                .source_for(provider)?
                .read_live()
                .await?
                .map(|l| l.blob))
        } else {
            self.secrets.read(&secret_key(provider, email)).await
        }
    }

    /// Persist a freshly-refreshed blob for an inactive saved account.
    pub async fn store_refreshed_blob(
        &self,
        provider: Provider,
        email: &str,
        blob: &[u8],
    ) -> Result<()> {
        self.secrets
            .upsert(&secret_key(provider, email), blob)
            .await
    }

    /// Snapshot every provider's current live account into a saved profile.
    /// Short-circuits per provider when nothing changed (blob + identity
    /// byte-equal). Returns the captured `(provider, email)` pairs.
    pub async fn capture_current(&self) -> Result<Vec<(Provider, String)>> {
        let mut profiles = self.load()?;
        let mut captured = Vec::new();
        let mut dirty = false;

        for source in &self.sources {
            let provider = source.provider();
            let Some(live) = source.read_live().await? else {
                continue;
            };
            let email = live.identity.email.clone();
            let key = secret_key(provider, &email);

            // Short-circuit: skip writes when the saved copy already matches.
            let saved = self.secrets.read(&key).await?;
            let existing = profiles
                .iter()
                .find(|p| p.provider == provider && p.email == email);
            let unchanged = saved.as_deref() == Some(live.blob.as_slice())
                && existing
                    .map(|p| p.oauth_account == live.identity.oauth_account)
                    .unwrap_or(false);
            if unchanged {
                captured.push((provider, email));
                continue;
            }

            self.secrets.upsert(&key, &live.blob).await?;
            let profile = Profile {
                email: email.clone(),
                saved_at: Utc::now(),
                provider,
                subscription_type: live.identity.subscription_type.clone(),
                rate_limit_tier: live.identity.rate_limit_tier.clone(),
                oauth_account: live.identity.oauth_account.clone(),
            };
            match profiles
                .iter_mut()
                .find(|p| p.provider == provider && p.email == email)
            {
                Some(slot) => *slot = profile,
                None => profiles.push(profile),
            }
            dirty = true;
            captured.push((provider, email));
        }

        if dirty {
            self.save(&profiles)?;
        }
        Ok(captured)
    }

    /// Switch the live login for `provider` to `email`'s saved account.
    ///
    /// Captures the current accounts first; a failed snapshot **aborts** the
    /// switch so the outgoing refresh token can't be lost.
    pub async fn switch_to(&self, provider: Provider, email: &str) -> Result<()> {
        self.capture_current()
            .await
            .context("aborting switch: failed to snapshot the current account(s)")?;

        let saved_blob = self
            .secrets
            .read(&secret_key(provider, email))
            .await?
            .ok_or_else(|| anyhow!("no saved credentials for {email}"))?;

        let profiles = self.load()?;
        let profile = profiles
            .iter()
            .find(|p| p.provider == provider && p.email == email)
            .ok_or_else(|| anyhow!("no saved profile for {email}"))?;

        self.source_for(provider)?
            .write_live(&saved_blob, &profile.oauth_account)
            .await?;
        Ok(())
    }

    /// Delete a saved account: secret item + profile entry.
    pub async fn remove(&self, provider: Provider, email: &str) -> Result<()> {
        self.secrets.delete(&secret_key(provider, email)).await?;
        let mut profiles = self.load()?;
        profiles.retain(|p| !(p.provider == provider && p.email == email));
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
            provider: Provider::Anthropic,
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

    #[test]
    fn key_namespaces_non_anthropic() {
        let mut p = profile_with("", Some("pro"), None);
        assert_eq!(p.key(), "user@example.com"); // anthropic = legacy email key
        p.provider = Provider::OpenAI;
        assert_eq!(p.key(), "openai:user@example.com");
    }
}
