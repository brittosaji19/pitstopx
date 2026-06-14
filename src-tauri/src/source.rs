//! Provider-agnostic **account source**: locating, reading, and writing a
//! provider's *live* login. One implementation per provider — `claude_source`
//! (Anthropic) and `codex_source` (OpenAI). This is the seam that lets the
//! refresh loop, capture, and switching stay provider-neutral.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::provider::Provider;

/// Identity of an account, derived from its live credentials.
#[derive(Debug, Clone)]
pub struct Identity {
    pub email: String,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    /// Provider-shaped identity object persisted with the profile and restored
    /// on switch. Claude: the verbatim `~/.claude.json` `oauthAccount`. Codex:
    /// a synthesized object (the identity lives inside the blob there).
    pub oauth_account: Value,
}

/// A provider's current live account: raw credential blob + parsed identity.
pub struct LiveAccount {
    pub blob: Vec<u8>,
    pub identity: Identity,
}

#[async_trait]
pub trait AccountSource: Send + Sync {
    fn provider(&self) -> Provider;

    /// Read the live account (blob + identity), or `None` if not logged in.
    async fn read_live(&self) -> Result<Option<LiveAccount>>;

    /// Write a saved account's blob into the live location, restoring identity
    /// where the provider keeps it separately (Claude's `~/.claude.json`).
    async fn write_live(&self, blob: &[u8], identity: &Value) -> Result<()>;

    /// Remove the live credential from the machine, logging the provider's CLI
    /// out. A missing credential is not an error.
    async fn clear_live(&self) -> Result<()>;

    /// Human-readable location of the live credentials (for `--print-paths`).
    fn describe(&self) -> String;
}

/// Build every account source available on this host. Sources whose provider
/// isn't installed still build; they simply return `None` from `read_live`.
pub fn build_all() -> Result<Vec<Box<dyn AccountSource>>> {
    Ok(vec![
        crate::claude_source::build()?,
        crate::codex_source::build()?,
    ])
}

/// Whether two providers should also factor the provider into the secret-store
/// key. Anthropic keeps the legacy email-only key (so existing saved blobs are
/// found); other providers are namespaced to avoid collisions when the same
/// email is logged into multiple providers.
pub fn secret_key(provider: Provider, email: &str) -> String {
    match provider {
        Provider::Anthropic => email.to_string(),
        other => format!("{}:{}", other.id(), email),
    }
}
