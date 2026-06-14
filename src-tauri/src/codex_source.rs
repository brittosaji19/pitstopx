//! OpenAI Codex account source.
//!
//! Codex (ChatGPT-backed login) stores credentials in a plaintext JSON file at
//! `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`):
//!
//! ```json
//! {
//!   "OPENAI_API_KEY": null,
//!   "tokens": { "id_token": "<JWT>", "access_token": "<JWT>",
//!               "refresh_token": "...", "account_id": "..." },
//!   "last_refresh": "2026-06-13T..."
//! }
//! ```
//!
//! Identity (email + ChatGPT plan) is not stored separately — it's encoded in
//! the `id_token` JWT claims, which we decode (no signature verification; we
//! only read claims we already trust from the user's own machine).

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;

use crate::provider::Provider;
use crate::source::{AccountSource, Identity, LiveAccount};

/// Build the Codex source, honoring `$CODEX_HOME`.
pub fn build() -> Result<Box<dyn AccountSource>> {
    Ok(Box::new(CodexSource { path: auth_path()? }))
}

/// `$CODEX_HOME/auth.json`, else `~/.codex/auth.json`.
pub fn auth_path() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        if !home.trim().is_empty() {
            return Ok(PathBuf::from(home).join("auth.json"));
        }
    }
    Ok(crate::paths::home_dir()?.join(".codex").join("auth.json"))
}

struct CodexSource {
    path: PathBuf,
}

#[async_trait]
impl AccountSource for CodexSource {
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn read_live(&self) -> Result<Option<LiveAccount>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let blob = std::fs::read(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        match identity_from_blob(&blob) {
            Some(identity) => Ok(Some(LiveAccount { blob, identity })),
            // File present but unparseable / no id_token → not a usable account.
            None => Ok(None),
        }
    }

    async fn write_live(&self, blob: &[u8], _identity: &Value) -> Result<()> {
        // Identity is embedded in the blob; just write auth.json atomically,
        // preserving its user-only permissions.
        crate::claude_source::atomic_write(&self.path, blob)
    }

    async fn clear_live(&self) -> Result<()> {
        // Remove auth.json entirely so `codex` starts a clean login. A missing
        // file is already the desired state.
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", self.path.display())),
        }
    }

    fn describe(&self) -> String {
        format!("file {}", self.path.display())
    }
}

/// Decode a JWT's payload claims (middle segment, base64url, no signature check).
pub fn decode_jwt_claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Parsed Codex auth needed to refresh + query usage.
pub struct CodexAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
    /// Access-token expiry (epoch ms) from its JWT `exp` claim; 0 if unknown.
    pub expires_at_ms: i64,
}

/// Parse the auth.json blob into the fields the usage engine needs.
pub fn parse_auth(blob: &[u8]) -> Result<CodexAuth> {
    let root: Value = serde_json::from_slice(blob).context("auth.json is not valid JSON")?;
    let tokens = root
        .get("tokens")
        .ok_or_else(|| anyhow!("auth.json missing tokens"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("auth.json missing tokens.access_token"))?
        .to_string();

    let auth_claim = decode_jwt_claims(&access_token);
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| chatgpt_account_id(auth_claim.as_ref()));

    let expires_at_ms = auth_claim
        .as_ref()
        .and_then(|c| c.get("exp"))
        .and_then(Value::as_i64)
        .map(|exp| exp * 1000)
        .unwrap_or(0);

    Ok(CodexAuth {
        access_token,
        refresh_token: tokens
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        account_id,
        expires_at_ms,
    })
}

/// Rewrite the token fields in an auth.json blob after a refresh, leaving every
/// other key intact.
pub fn patch_blob(
    blob: &[u8],
    access_token: &str,
    refresh_token: Option<&str>,
    id_token: Option<&str>,
) -> Result<Vec<u8>> {
    let mut root: Value = serde_json::from_slice(blob).context("auth.json is not valid JSON")?;
    let tokens = root
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("auth.json missing tokens object"))?;
    tokens.insert(
        "access_token".into(),
        Value::String(access_token.to_string()),
    );
    if let Some(rt) = refresh_token {
        tokens.insert("refresh_token".into(), Value::String(rt.to_string()));
    }
    if let Some(it) = id_token {
        tokens.insert("id_token".into(), Value::String(it.to_string()));
    }
    if let Some(obj) = root.as_object_mut() {
        obj.insert(
            "last_refresh".into(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
    }
    Ok(serde_json::to_vec(&root)?)
}

/// Derive identity from the auth.json blob via the `id_token` JWT claims.
fn identity_from_blob(blob: &[u8]) -> Option<Identity> {
    let root: Value = serde_json::from_slice(blob).ok()?;
    let tokens = root.get("tokens")?;
    let id_token = tokens.get("id_token").and_then(Value::as_str)?;
    let claims = decode_jwt_claims(id_token)?;

    // Email: standard OIDC `email` claim, else the profile claim.
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| {
            claims
                .get("https://api.openai.com/profile")
                .and_then(|p| p.get("email"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)?;

    let auth = claims.get("https://api.openai.com/auth");
    let plan = auth
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| chatgpt_account_id(Some(&claims)));

    let oauth_account = serde_json::json!({
        "emailAddress": email,
        "accountId": account_id,
        "planType": plan,
    });

    Some(Identity {
        email,
        // Surface the ChatGPT plan as the subscription label (e.g. "Plus").
        subscription_type: plan,
        rate_limit_tier: None,
        oauth_account,
    })
}

fn chatgpt_account_id(claims: Option<&Value>) -> Option<String> {
    claims
        .and_then(|c| c.get("https://api.openai.com/auth"))
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    fn jwt(claims: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn parses_identity_from_id_token() {
        let id = jwt(serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_account_id": "acc-123"
            }
        }));
        let blob = serde_json::to_vec(&serde_json::json!({
            "tokens": { "id_token": id, "access_token": "x", "account_id": "acc-123" }
        }))
        .unwrap();

        let identity = identity_from_blob(&blob).expect("identity");
        assert_eq!(identity.email, "dev@example.com");
        assert_eq!(identity.subscription_type.as_deref(), Some("pro"));
        assert_eq!(identity.oauth_account["accountId"], "acc-123");
    }

    #[test]
    fn patch_blob_preserves_other_keys() {
        let blob = serde_json::to_vec(&serde_json::json!({
            "OPENAI_API_KEY": Value::Null,
            "tokens": { "id_token": "old", "access_token": "old", "refresh_token": "r0" },
            "last_refresh": "old"
        }))
        .unwrap();
        let out = patch_blob(&blob, "new-access", Some("r1"), Some("new-id")).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["tokens"]["access_token"], "new-access");
        assert_eq!(v["tokens"]["refresh_token"], "r1");
        assert_eq!(v["tokens"]["id_token"], "new-id");
        assert!(v.get("OPENAI_API_KEY").is_some());
    }
}
