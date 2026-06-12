//! Parsing and patching of the Claude Code credential blob and of
//! `~/.claude.json` (identity). The blob is treated as opaque JSON: PitStopX
//! only ever rewrites the token fields inside `claudeAiOauth`, leaving
//! `mcpOAuth` and other sections byte-for-byte intact so per-account MCP OAuth
//! tokens travel with the account on a switch.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Safety margin: treat a token as expired this many ms before its real expiry.
const EXPIRY_MARGIN_MS: i64 = 120_000;

/// The OAuth section PitStopX understands inside the credential blob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Absolute expiry in epoch milliseconds.
    pub expires_at_ms: i64,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

impl OAuthCredentials {
    /// Expired (or within the 2-minute safety margin)?
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.expires_at_ms - EXPIRY_MARGIN_MS
    }
}

/// The full credential blob, kept as parsed JSON so unknown sections survive a
/// round-trip untouched.
#[derive(Debug, Clone)]
pub struct CredentialBlob {
    root: Value,
}

impl CredentialBlob {
    /// Parse raw bytes; requires `claudeAiOauth.accessToken` to be present.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let root: Value =
            serde_json::from_slice(bytes).context("credential blob is not valid JSON")?;
        let blob = Self { root };
        blob.oauth_section()
            .and_then(|o| o.get("accessToken"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("credential blob missing claudeAiOauth.accessToken"))?;
        Ok(blob)
    }

    fn oauth_section(&self) -> Option<&Value> {
        self.root.get("claudeAiOauth")
    }

    /// Extract the typed OAuth credentials.
    pub fn credentials(&self) -> Result<OAuthCredentials> {
        let o = self
            .oauth_section()
            .ok_or_else(|| anyhow!("blob missing claudeAiOauth"))?;
        Ok(OAuthCredentials {
            access_token: o
                .get("accessToken")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("missing accessToken"))?
                .to_string(),
            refresh_token: o
                .get("refreshToken")
                .and_then(Value::as_str)
                .map(str::to_string),
            expires_at_ms: o
                .get("expiresAt")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            subscription_type: o
                .get("subscriptionType")
                .and_then(Value::as_str)
                .map(str::to_string),
            rate_limit_tier: o
                .get("rateLimitTier")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// Return a copy of the blob with only the token fields inside
    /// `claudeAiOauth` rewritten. All other keys/sections are preserved.
    pub fn patching(&self, fresh: &OAuthCredentials) -> Result<CredentialBlob> {
        let mut root = self.root.clone();
        let o = root
            .get_mut("claudeAiOauth")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("blob missing claudeAiOauth object"))?;
        o.insert(
            "accessToken".into(),
            Value::String(fresh.access_token.clone()),
        );
        if let Some(rt) = &fresh.refresh_token {
            o.insert("refreshToken".into(), Value::String(rt.clone()));
        }
        o.insert(
            "expiresAt".into(),
            Value::Number(fresh.expires_at_ms.into()),
        );
        if let Some(st) = &fresh.subscription_type {
            o.insert("subscriptionType".into(), Value::String(st.clone()));
        }
        if let Some(tier) = &fresh.rate_limit_tier {
            o.insert("rateLimitTier".into(), Value::String(tier.clone()));
        }
        Ok(CredentialBlob { root })
    }

    /// Serialize back to bytes (compact, matching Claude Code's own writes).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.root).expect("serializing a parsed blob cannot fail")
    }

    /// Byte-equality check used by `capture_current`'s short-circuit.
    pub fn equals_bytes(&self, other: &[u8]) -> bool {
        match serde_json::from_slice::<Value>(other) {
            Ok(v) => v == self.root,
            Err(_) => false,
        }
    }
}

/// Thin reader/writer over `~/.claude.json`, the cross-OS identity file.
pub struct ClaudeConfig {
    path: PathBuf,
}

impl ClaudeConfig {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<Value> {
        if !self.path.exists() {
            return Ok(Value::Object(Default::default()));
        }
        let bytes = std::fs::read(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", self.path.display()))
    }

    /// Verbatim `oauthAccount` object, if present.
    pub fn oauth_account(&self) -> Result<Option<Value>> {
        Ok(self.load()?.get("oauthAccount").cloned())
    }

    /// `oauthAccount.emailAddress`, the currently active account.
    pub fn active_email(&self) -> Result<Option<String>> {
        Ok(self
            .load()?
            .get("oauthAccount")
            .and_then(|a| a.get("emailAddress"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Replace only `oauthAccount`, writing the whole file atomically
    /// (temp file + rename) and preserving every other key.
    pub fn set_oauth_account(&self, account: &Value) -> Result<()> {
        let mut root = self.load()?;
        let obj = root
            .as_object_mut()
            .ok_or_else(|| anyhow!("~/.claude.json is not a JSON object"))?;
        obj.insert("oauthAccount".into(), account.clone());
        let bytes = serde_json::to_vec_pretty(&root)?;
        crate::claude_source::atomic_write(&self.path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob_bytes() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "old-access",
                "refreshToken": "old-refresh",
                "expiresAt": 1_000_000i64,
                "subscriptionType": "max",
                "rateLimitTier": "max_20x"
            },
            "mcpOAuth": { "atlassian": { "token": "keep-me" } }
        }))
        .unwrap()
    }

    #[test]
    fn parse_requires_access_token() {
        assert!(CredentialBlob::parse(b"{}").is_err());
        assert!(CredentialBlob::parse(&blob_bytes()).is_ok());
    }

    #[test]
    fn patching_preserves_other_sections() {
        let blob = CredentialBlob::parse(&blob_bytes()).unwrap();
        let fresh = OAuthCredentials {
            access_token: "new-access".into(),
            refresh_token: Some("new-refresh".into()),
            expires_at_ms: 2_000_000,
            subscription_type: Some("max".into()),
            rate_limit_tier: Some("max_20x".into()),
        };
        let patched = blob.patching(&fresh).unwrap();
        let v: Value = serde_json::from_slice(&patched.to_bytes()).unwrap();
        assert_eq!(v["claudeAiOauth"]["accessToken"], "new-access");
        assert_eq!(v["claudeAiOauth"]["expiresAt"], 2_000_000);
        // mcpOAuth survives untouched.
        assert_eq!(v["mcpOAuth"]["atlassian"]["token"], "keep-me");
    }

    #[test]
    fn expiry_margin_is_120s() {
        let c = OAuthCredentials {
            access_token: "a".into(),
            refresh_token: None,
            expires_at_ms: 1_000_000,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert!(c.is_expired(1_000_000 - 119_000)); // within margin
        assert!(!c.is_expired(1_000_000 - 121_000)); // safely fresh
    }
}
