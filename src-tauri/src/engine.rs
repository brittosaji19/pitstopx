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

    // We never rotate an *inactive* Codex account's token. OpenAI uses single-use
    // rotating refresh tokens, and the moment the user switches to this account
    // its own CLI (`codex`) takes over that same token chain. Refreshing here
    // would revoke the copy sitting in `~/.codex/auth.json`, so the next login
    // fails with "refresh token was revoked". The active account is kept fresh by
    // `codex` itself.
    if !is_active {
        // Known-expired (decodable JWT `exp` in the past): skip without a request.
        if expired {
            return Err(UsageError::RefreshSkipped);
        }
        // Otherwise try once. A 401/403 means the saved access token is stale
        // (its `exp` may be in the future, or wasn't a decodable JWT so we
        // couldn't tell). We still won't refresh it, so report the same benign
        // stale state rather than a hard `Unauthorized`.
        return match codex_usage::fetch_usage(&auth.access_token, auth.account_id.as_deref()).await
        {
            Ok(report) => Ok(Fetched {
                report,
                refreshed_blob: None,
            }),
            Err(UsageError::Unauthorized) => Err(UsageError::RefreshSkipped),
            Err(e) => Err(e),
        };
    }

    let report = codex_usage::fetch_usage(&auth.access_token, auth.account_id.as_deref()).await?;
    Ok(Fetched {
        report,
        refreshed_blob: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    /// Build a Codex `auth.json` blob whose access token expires at `exp` (epoch
    /// seconds), with a refresh token present.
    fn codex_blob(exp: i64) -> Vec<u8> {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({ "exp": exp })).unwrap());
        let access = format!("{header}.{payload}.sig");
        serde_json::to_vec(&serde_json::json!({
            "tokens": { "access_token": access, "refresh_token": "rt", "account_id": "acc" }
        }))
        .unwrap()
    }

    /// An inactive Codex account with an expired access token must NOT be
    /// refreshed (that would revoke the token its CLI relies on) — it returns the
    /// benign `RefreshSkipped` without making any network call.
    #[tokio::test]
    async fn inactive_expired_codex_is_not_refreshed() {
        let now_ms = 1_000_000_000_000;
        let blob = codex_blob(now_ms / 1000 - 3600); // expired an hour ago
        let result = openai(&blob, false, now_ms).await;
        assert!(
            matches!(result, Err(UsageError::RefreshSkipped)),
            "inactive expired Codex account must be skipped, not refreshed"
        );
    }
}
