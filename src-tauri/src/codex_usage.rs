//! OpenAI Codex usage (`wham/usage`) + OAuth token refresh.
//!
//! Usage is read from the same unofficial endpoint the Codex CLI polls
//! (`https://chatgpt.com/backend-api/wham/usage`, confirmed by openai/codex
//! issue #10869). The response shape is undocumented and has drifted between
//! Codex versions, so the parser is deliberately tolerant: it tries several
//! container/window/field names and normalizes everything to PitStopX's 0..1
//! fraction model. Refresh uses the public Codex OAuth client.

use std::time::Duration;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::usage_api::{UsageError, UsageReport, Window};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Public Codex CLI OAuth client id.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("PitStopX/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client builds with rustls")
}

/// `GET /backend-api/wham/usage` with the Codex bearer token + account id.
pub async fn fetch_usage(
    access_token: &str,
    account_id: Option<&str>,
) -> Result<UsageReport, UsageError> {
    let mut req = client()
        .get(USAGE_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/");
    if let Some(id) = account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| UsageError::Network(e.to_string()))?;
    match resp.status().as_u16() {
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
    Ok(parse_usage(&body))
}

/// Tolerant parse of the usage response into a `UsageReport`.
fn parse_usage(body: &Value) -> UsageReport {
    // The windows may sit at the root or under a `rate_limits`/`rate_limit` key.
    let container = body
        .get("rate_limits")
        .or_else(|| body.get("rate_limit"))
        .or_else(|| body.get("usage"))
        .unwrap_or(body);

    // Codex exposes a single monthly limit. Different payload versions label it
    // `primary`, `secondary`, or `monthly`; use whichever is present (preferring
    // the longer secondary window when both exist) and show it as monthly unless
    // the response states an explicit window length. It lives in the `seven_day`
    // slot; `five_hour` stays absent so no 5-hour bar is rendered for Codex.
    let monthly = codex_window(
        container,
        &[
            "secondary",
            "secondary_window",
            "monthly",
            "weekly",
            "seven_day",
            "7d",
            "primary",
            "primary_window",
            "five_hour",
            "5h",
        ],
        43200,
    );

    UsageReport {
        seven_day: monthly,
        ..Default::default()
    }
}

/// Locate a window under any alias and tag it with its length. The length comes
/// from the response when present, else `default_minutes`. Absent → a default
/// (period-less) window the UI skips.
fn codex_window(container: &Value, keys: &[&str], default_minutes: i64) -> Window {
    for k in keys {
        if let Some(w) = container.get(*k) {
            if !w.is_null() {
                let mut win = window_from(w);
                win.period_minutes = Some(window_minutes(w).unwrap_or(default_minutes));
                return win;
            }
        }
    }
    Window::default()
}

/// Explicit window length (minutes) from whichever field the response carries.
fn window_minutes(w: &Value) -> Option<i64> {
    if let Some(m) = num(w, &["window_minutes", "window_size_minutes", "limit_window_minutes"]) {
        return Some(m as i64);
    }
    num(w, &["window_seconds", "window_size_seconds"]).map(|s| (s / 60.0) as i64)
}

fn window_from(w: &Value) -> Window {
    // Used percentage, under any of several names; or derived from remaining.
    let used_pct = num(
        w,
        &[
            "used_percent",
            "usage_percent",
            "percent_used",
            "utilization",
        ],
    )
    .or_else(|| num(w, &["percent_left", "remaining_percent"]).map(|r| 100.0 - r));

    Window {
        utilization: used_pct.map(|u| (u / 100.0).clamp(0.0, 1.0)),
        resets_at: parse_reset(w),
        // Set by the caller (`codex_window`) from the window length.
        period_minutes: None,
    }
}

fn num(w: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| w.get(*k).and_then(Value::as_f64))
}

/// Resolve a reset time from whichever field the response carries.
fn parse_reset(w: &Value) -> Option<DateTime<Local>> {
    let now = Utc::now();
    // Relative seconds from now.
    if let Some(secs) = num(
        w,
        &[
            "resets_in_seconds",
            "reset_after_seconds",
            "seconds_to_reset",
        ],
    ) {
        return Some((now + chrono::Duration::seconds(secs as i64)).with_timezone(&Local));
    }
    // Absolute epoch milliseconds.
    if let Some(ms) = num(w, &["reset_time_ms", "resets_at_ms"]) {
        return Utc
            .timestamp_millis_opt(ms as i64)
            .single()
            .map(|d| d.with_timezone(&Local));
    }
    // Absolute reset: ISO-8601 string or epoch seconds.
    if let Some(v) = w.get("reset_at").or_else(|| w.get("resets_at")) {
        if let Some(s) = v.as_str() {
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                return Some(dt.with_timezone(&Local));
            }
        }
        if let Some(secs) = v.as_f64() {
            return Utc
                .timestamp_opt(secs as i64, 0)
                .single()
                .map(|d| d.with_timezone(&Local));
        }
    }
    None
}

/// Exchange a refresh token for fresh tokens. Returns
/// `(access_token, new_refresh_token?, id_token?, expires_in_secs)`.
pub async fn refresh(
    refresh_token: &str,
) -> Result<(String, Option<String>, Option<String>, i64), UsageError> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        id_token: Option<String>,
        #[serde(default)]
        expires_in: i64,
    }

    let resp = client()
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "scope": "openid profile email",
        }))
        .send()
        .await
        .map_err(|e| UsageError::Network(e.to_string()))?;

    match resp.status().as_u16() {
        200 => {}
        401 | 403 => return Err(UsageError::Unauthorized),
        429 => return Err(UsageError::RateLimited(None)),
        other => return Err(UsageError::Http(other)),
    }

    let t: TokenResponse = resp
        .json()
        .await
        .map_err(|e| UsageError::Decode(e.to_string()))?;
    Ok((t.access_token, t.refresh_token, t.id_token, t.expires_in))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_single_monthly_window() {
        // Both windows present → show only the monthly (secondary) one; no 5h bar.
        let body = serde_json::json!({
            "rate_limits": {
                "primary": { "used_percent": 42.0, "resets_in_seconds": 3600 },
                "secondary": { "used_percent": 8.5, "resets_in_seconds": 600000 }
            }
        });
        let r = parse_usage(&body);
        assert_eq!(r.five_hour.label(), None); // no 5-hour bar for Codex
        assert_eq!(r.seven_day.utilization, Some(0.085));
        assert!(r.seven_day.resets_at.is_some());
        assert_eq!(r.seven_day.label().as_deref(), Some("Monthly"));
    }

    #[test]
    fn monthly_only_when_primary_absent() {
        // A plan reporting just the monthly window → no 5h bar.
        let body = serde_json::json!({
            "rate_limits": { "secondary": { "used_percent": 68.0 } }
        });
        let r = parse_usage(&body);
        assert_eq!(r.five_hour.label(), None); // absent → not rendered
        assert_eq!(r.seven_day.label().as_deref(), Some("Monthly"));
        assert_eq!(r.seven_day.utilization, Some(0.68));
    }

    #[test]
    fn explicit_window_length_overrides_default() {
        let body = serde_json::json!({
            "rate_limits": {
                "secondary": { "used_percent": 10.0, "window_minutes": 10080 }
            }
        });
        let r = parse_usage(&body);
        assert_eq!(r.seven_day.label().as_deref(), Some("7d"));
    }

    #[test]
    fn derives_used_from_remaining_and_alt_names() {
        // `percent_left` is converted to a used fraction on the monthly window.
        let body = serde_json::json!({
            "rate_limits": { "primary": { "percent_left": 70.0 } }
        });
        let r = parse_usage(&body);
        assert_eq!(r.seven_day.utilization, Some(0.30));
        assert_eq!(r.seven_day.label().as_deref(), Some("Monthly"));
    }
}
