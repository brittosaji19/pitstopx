//! The snapshot DTOs pushed to the WebView panel (serde → camelCase JSON) and
//! the helper that builds a `UiSnapshot` from `AppState`. The panel is a pure
//! view: Rust computes order and all display strings here.

use chrono::Local;
use serde::Serialize;

use crate::app::AppState;
use crate::format;
use crate::usage_api::{BindingWindow, UsageError, UsageReport};

/// Event name the panel subscribes to.
pub const SNAPSHOT_EVENT: &str = "pitstopx://snapshot";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSnapshot {
    pub active_email: Option<String>,
    /// ISO timestamp; formatted client-side.
    pub last_refresh: Option<String>,
    pub last_top_level_error: Option<String>,
    /// Pre-sorted: active first, then emptiest.
    pub rows: Vec<AccountRowDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRowDto {
    pub email: String,
    /// Provider display name, e.g. "Anthropic".
    pub provider_label: String,
    /// Provider machine id, e.g. "anthropic" (CSS hook for per-provider color).
    pub provider_id: String,
    /// Provider accent color (hex).
    pub provider_accent: String,
    pub plan_label: String,
    pub is_active: bool,
    pub bars: Vec<UsageBarDto>,
    pub models_line: Option<String>,
    pub status_line: Option<String>,
    pub switchable: bool,
    /// Inactive (saved) accounts can be removed from the app store.
    pub removable: bool,
    /// The last fetch failed with an auth error — offer re-authentication.
    pub needs_reauth: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBarDto {
    /// "5h" or "7d".
    pub label: String,
    pub utilization: Option<f64>,
    pub reset_text: String,
}

impl UiSnapshot {
    /// Build the snapshot from current state. Row order: active first, then
    /// ascending `max_utilization` (emptiest next).
    pub fn build(state: &AppState) -> Self {
        let now = Local::now();
        // Compared per row to flag accounts that need re-authentication.
        let unauthorized = UsageError::Unauthorized.to_string();

        let mut rows: Vec<AccountRowDto> = state
            .profiles
            .iter()
            .map(|p| {
                let key = p.key();
                let is_active = state.active_keys.contains(&key);
                let report = state.usage.get(&key);
                let error = state.fetch_error.get(&key);

                AccountRowDto {
                    email: p.email.clone(),
                    provider_label: p.provider.display_name().to_string(),
                    provider_id: p.provider.id().to_string(),
                    provider_accent: p.provider.accent().to_string(),
                    plan_label: p.plan_label(),
                    is_active,
                    bars: build_bars(report, now),
                    models_line: report.and_then(models_line),
                    status_line: status_line(state, &key, report, error, now),
                    switchable: !is_active,
                    removable: !is_active,
                    needs_reauth: error.is_some_and(|e| *e == unauthorized),
                }
            })
            .collect();

        rows.sort_by(|a, b| {
            // Active first.
            match (b.is_active, a.is_active) {
                (false, true) => return std::cmp::Ordering::Less,
                (true, false) => return std::cmp::Ordering::Greater,
                _ => {}
            }
            // Then emptiest (ascending max utilization; unknown sorts last).
            let au = max_util(&a.bars).unwrap_or(f64::INFINITY);
            let bu = max_util(&b.bars).unwrap_or(f64::INFINITY);
            au.partial_cmp(&bu).unwrap_or(std::cmp::Ordering::Equal)
        });

        UiSnapshot {
            active_email: state.primary_email().map(str::to_string),
            last_refresh: state.last_refresh.map(|t| t.to_rfc3339()),
            last_top_level_error: state.last_top_level_error.clone(),
            rows,
        }
    }
}

fn max_util(bars: &[UsageBarDto]) -> Option<f64> {
    bars.iter()
        .filter_map(|b| b.utilization)
        .fold(None, |acc, u| Some(acc.map_or(u, |a: f64| a.max(u))))
}

fn build_bars(report: Option<&UsageReport>, now: chrono::DateTime<Local>) -> Vec<UsageBarDto> {
    let Some(r) = report else {
        return vec![
            UsageBarDto {
                label: "5h".into(),
                utilization: None,
                reset_text: String::new(),
            },
            UsageBarDto {
                label: "7d".into(),
                utilization: None,
                reset_text: String::new(),
            },
        ];
    };
    vec![
        UsageBarDto {
            label: "5h".into(),
            utilization: r.five_hour.utilization,
            reset_text: r
                .five_hour
                .resets_at
                .map(|t| format::compact_reset(t, now))
                .unwrap_or_default(),
        },
        UsageBarDto {
            label: "7d".into(),
            utilization: r.seven_day.utilization,
            reset_text: r
                .seven_day
                .resets_at
                .map(|t| format::compact_reset(t, now))
                .unwrap_or_default(),
        },
    ]
}

/// `"Opus wk 12% · Sonnet wk 10% · Extra 4%"` — built only when at least one
/// per-model or extra-usage figure is present.
fn models_line(r: &UsageReport) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(o) = r.seven_day_opus {
        parts.push(format!("Opus wk {}", format::percent(Some(o))));
    }
    if let Some(s) = r.seven_day_sonnet {
        parts.push(format!("Sonnet wk {}", format::percent(Some(s))));
    }
    if r.extra_usage.is_enabled {
        if let Some(e) = r.extra_usage.utilization {
            parts.push(format!("Extra {}", format::percent(Some(e))));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// Error / stale / loading line for a row.
fn status_line(
    state: &AppState,
    email: &str,
    report: Option<&UsageReport>,
    error: Option<&String>,
    now: chrono::DateTime<Local>,
) -> Option<String> {
    match (report, error) {
        // Stale: we have a prior report *and* a current error.
        (Some(_), Some(err)) => {
            let retry = state
                .next_fetch_allowed
                .get(email)
                .map(|gate| {
                    let secs = gate
                        .saturating_duration_since(std::time::Instant::now())
                        .as_secs();
                    format!(" — retrying in {}m", (secs / 60).max(1))
                })
                .unwrap_or_default();
            let stamp = state
                .last_refresh
                .map(|t| format::updated(t.with_timezone(&Local)))
                .unwrap_or_default();
            Some(format!("⚠ {err} · showing {stamp} data{retry}"))
        }
        // Hard error with no prior data.
        (None, Some(err)) => Some(format!("⚠ {err}")),
        // No report yet, no error: loading.
        (None, None) => {
            let _ = now;
            Some("Loading…".to_string())
        }
        // Fresh data, no error.
        (Some(_), None) => None,
    }
}

/// The binding-window reset text used by threshold notifications.
pub fn binding_reset_text(report: &UsageReport) -> String {
    let now = Local::now();
    match report.binding_window() {
        BindingWindow::FiveHour => report
            .five_hour
            .resets_at
            .map(|t| format::reset(t, now))
            .unwrap_or_else(|| "soon".into()),
        BindingWindow::SevenDay => report
            .seven_day
            .resets_at
            .map(|t| format::reset(t, now))
            .unwrap_or_else(|| "soon".into()),
    }
}
