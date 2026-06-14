//! Usage fetch + OAuth refresh against the same unofficial OAuth surface Claude
//! Code uses. TLS is rustls (no OpenSSL), identical across OSes.

use std::time::Duration;

use chrono::{DateTime, Local};
use serde::Deserialize;
use serde_json::Value;

pub use crate::credentials::OAuthCredentials;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
/// Public PKCE client id shared with Claude Code.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Typed errors surfaced to the refresh loop so it can pick the right backoff.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("unauthorized")]
    Unauthorized,
    /// We deliberately declined to refresh an inactive account's token (rotating
    /// it would revoke the copy the provider's own CLI relies on). Surfaced as a
    /// benign "stale" state, not a hard failure.
    #[error("paused while inactive")]
    RefreshSkipped,
    /// 429; carries the parsed `Retry-After` (seconds) when present.
    #[error("rate limited")]
    RateLimited(Option<u64>),
    #[error("http status {0}")]
    Http(u16),
    #[error("network error: {0}")]
    Network(String),
    #[error("malformed response: {0}")]
    Decode(String),
}

/// One usage window (5-hour or 7-day).
#[derive(Debug, Clone, Default)]
pub struct Window {
    pub utilization: Option<f64>,
    pub resets_at: Option<DateTime<Local>>,
}

/// Extra-usage (pay-as-you-go overage) status.
#[derive(Debug, Clone, Default)]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub utilization: Option<f64>,
}

/// Identifies which window currently binds the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingWindow {
    FiveHour,
    SevenDay,
}

/// A parsed `/usage` response.
#[derive(Debug, Clone, Default)]
pub struct UsageReport {
    pub five_hour: Window,
    pub seven_day: Window,
    pub seven_day_opus: Option<f64>,
    pub seven_day_sonnet: Option<f64>,
    pub extra_usage: ExtraUsage,
}

impl UsageReport {
    /// `max(fiveHour, sevenDay)` — the figure the tray/sorting use.
    pub fn max_utilization(&self) -> Option<f64> {
        match (self.five_hour.utilization, self.seven_day.utilization) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// The binding window for reset display; ties resolve to the 5-hour window.
    pub fn binding_window(&self) -> BindingWindow {
        let five = self.five_hour.utilization.unwrap_or(0.0);
        let seven = self.seven_day.utilization.unwrap_or(0.0);
        if seven > five {
            BindingWindow::SevenDay
        } else {
            BindingWindow::FiveHour
        }
    }

    pub fn binding_reset(&self) -> Option<DateTime<Local>> {
        match self.binding_window() {
            BindingWindow::FiveHour => self.five_hour.resets_at,
            BindingWindow::SevenDay => self.seven_day.resets_at,
        }
    }
}

/// Parse `resets_at`: ISO-8601 with fractional seconds first, then plain.
fn parse_reset(v: Option<&Value>) -> Option<DateTime<Local>> {
    let s = v.and_then(Value::as_str)?;
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Local))
        .ok()
}

/// Read a `utilization` field and normalize it to a 0..1 fraction. The API
/// reports utilization as a 0–100 percentage; the rest of PitStopX works in
/// fractions, so divide here at the single parse boundary.
fn utilization_of(v: Option<&Value>) -> Option<f64> {
    v.and_then(|v| v.get("utilization"))
        .and_then(Value::as_f64)
        .map(|u| u / 100.0)
}

fn window_from(v: Option<&Value>) -> Window {
    let Some(v) = v else { return Window::default() };
    Window {
        utilization: utilization_of(Some(v)),
        resets_at: parse_reset(v.get("resets_at")),
    }
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("PitStopX/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client builds with rustls")
}

/// `GET /api/oauth/usage` with the OAuth bearer token.
pub async fn fetch_usage(access_token: &str) -> Result<UsageReport, UsageError> {
    let client = build_client();
    let resp = client
        .get(USAGE_URL)
        .bearer_auth(access_token)
        .header("anthropic-beta", ANTHROPIC_BETA)
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| UsageError::Network(e.to_string()))?;

    let status = resp.status();
    match status.as_u16() {
        200 => {}
        401 | 403 => return Err(UsageError::Unauthorized),
        429 => {
            let retry = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            return Err(UsageError::RateLimited(retry));
        }
        other => return Err(UsageError::Http(other)),
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| UsageError::Decode(e.to_string()))?;

    let extra = body.get("extra_usage");
    Ok(UsageReport {
        five_hour: window_from(body.get("five_hour")),
        seven_day: window_from(body.get("seven_day")),
        seven_day_opus: utilization_of(body.get("seven_day_opus")),
        seven_day_sonnet: utilization_of(body.get("seven_day_sonnet")),
        extra_usage: ExtraUsage {
            is_enabled: extra
                .and_then(|e| e.get("is_enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            utilization: utilization_of(extra),
        },
    })
}

/// Exchange a refresh token for a fresh access token. Used **only** for
/// inactive/saved profiles; the active account is kept fresh by Claude Code.
pub async fn refresh_token(
    refresh_token: &str,
    now_ms: i64,
) -> Result<OAuthCredentials, UsageError> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        expires_in: i64,
    }

    let client = build_client();
    let resp = client
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|e| UsageError::Network(e.to_string()))?;

    let status = resp.status();
    match status.as_u16() {
        200 => {}
        401 | 403 => return Err(UsageError::Unauthorized),
        429 => return Err(UsageError::RateLimited(None)),
        other => return Err(UsageError::Http(other)),
    }

    let token: TokenResponse = resp
        .json()
        .await
        .map_err(|e| UsageError::Decode(e.to_string()))?;

    Ok(OAuthCredentials {
        access_token: token.access_token,
        // Keep the existing refresh token if the server didn't rotate it.
        refresh_token: token
            .refresh_token
            .or_else(|| Some(refresh_token.to_string())),
        expires_at_ms: now_ms + token.expires_in * 1000,
        subscription_type: None,
        rate_limit_tier: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(u: f64) -> Window {
        Window {
            utilization: Some(u),
            resets_at: None,
        }
    }

    #[test]
    fn utilization_normalized_to_fraction() {
        // API reports 0–100 percentages; we store 0..1 fractions.
        let v = serde_json::json!({ "utilization": 83.0 });
        assert_eq!(utilization_of(Some(&v)), Some(0.83));
        let w = window_from(Some(&serde_json::json!({ "utilization": 9 })));
        assert_eq!(w.utilization, Some(0.09));
        assert_eq!(utilization_of(Some(&serde_json::json!({}))), None);
    }

    #[test]
    fn max_utilization_and_binding() {
        let r = UsageReport {
            five_hour: win(0.4),
            seven_day: win(0.6),
            ..Default::default()
        };
        assert_eq!(r.max_utilization(), Some(0.6));
        assert_eq!(r.binding_window(), BindingWindow::SevenDay);

        let tie = UsageReport {
            five_hour: win(0.5),
            seven_day: win(0.5),
            ..Default::default()
        };
        // Ties resolve to the 5-hour window.
        assert_eq!(tie.binding_window(), BindingWindow::FiveHour);
    }

    #[test]
    fn reset_parses_fractional_and_plain() {
        let frac = parse_reset(Some(&Value::String("2026-06-13T21:49:00.123Z".into())));
        let plain = parse_reset(Some(&Value::String("2026-06-13T21:49:00Z".into())));
        assert!(frac.is_some());
        assert!(plain.is_some());
        assert!(parse_reset(Some(&Value::String("not-a-date".into()))).is_none());
    }
}
