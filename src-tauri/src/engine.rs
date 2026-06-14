//! Per-provider credential-refresh + usage dispatch. Given a saved/live
//! credential blob, returns a usage report and (for inactive accounts whose
//! token was refreshed) the new blob to persist. Keeps the refresh loop in
//! `app.rs` provider-neutral.

use crate::codex_source;
use crate::codex_usage;
use crate::credentials::CredentialBlob;
use crate::provider::Provider;
use crate::usage_api::{self, UsageError, UsageReport};

/// Result of fetching one account's usage.
pub struct Fetched {
    pub report: UsageReport,
    /// Set when an inactive account's token was refreshed and should be saved.
    pub refreshed_blob: Option<Vec<u8>>,
}

/// Fetch usage for one account, refreshing its token first if needed. The
/// active account is never refreshed by PitStopX — the provider's own CLI keeps
/// it fresh.
pub async fn fetch(
    provider: Provider,
    blob: &[u8],
    is_active: bool,
    now_ms: i64,
) -> Result<Fetched, UsageError> {
    match provider {
        Provider::Anthropic => anthropic(blob, is_active, now_ms).await,
        Provider::OpenAI => openai(blob, is_active, now_ms).await,
    }
}

async fn anthropic(blob: &[u8], is_active: bool, now_ms: i64) -> Result<Fetched, UsageError> {
    let parsed = CredentialBlob::parse(blob).map_err(|e| UsageError::Decode(e.to_string()))?;
    let creds = parsed
        .credentials()
        .map_err(|e| UsageError::Decode(e.to_string()))?;

    let (token, refreshed_blob) = if is_active || !creds.is_expired(now_ms) {
        (creds.access_token.clone(), None)
    } else {
        let Some(rt) = &creds.refresh_token else {
            return Err(UsageError::Unauthorized);
        };
        let fresh = usage_api::refresh_token(rt, now_ms).await?;
        // Preserve subscription/tier the refresh endpoint doesn't return.
        let merged = usage_api::OAuthCredentials {
            subscription_type: creds.subscription_type.clone(),
            rate_limit_tier: creds.rate_limit_tier.clone(),
            ..fresh
        };
        let patched = parsed
            .patching(&merged)
            .map_err(|e| UsageError::Decode(e.to_string()))?;
        (merged.access_token, Some(patched.to_bytes()))
    };

    let report = usage_api::fetch_usage(&token).await?;
    Ok(Fetched {
        report,
        refreshed_blob,
    })
}

async fn openai(blob: &[u8], is_active: bool, now_ms: i64) -> Result<Fetched, UsageError> {
    let auth = codex_source::parse_auth(blob).map_err(|e| UsageError::Decode(e.to_string()))?;
    // 2-minute safety margin, matching the Anthropic path.
    let expired = auth.expires_at_ms != 0 && now_ms >= auth.expires_at_ms - 120_000;

    let (access, account_id, refreshed_blob) = if is_active || !expired {
        (auth.access_token.clone(), auth.account_id.clone(), None)
    } else {
        let Some(rt) = &auth.refresh_token else {
            return Err(UsageError::Unauthorized);
        };
        let (access, new_rt, id_token, _expires_in) = codex_usage::refresh(rt).await?;
        let patched =
            codex_source::patch_blob(blob, &access, new_rt.as_deref(), id_token.as_deref())
                .map_err(|e| UsageError::Decode(e.to_string()))?;
        (access, auth.account_id.clone(), Some(patched))
    };

    let report = codex_usage::fetch_usage(&access, account_id.as_deref()).await?;
    Ok(Fetched {
        report,
        refreshed_blob,
    })
}
