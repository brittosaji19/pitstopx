//! Application core: `AppState` (the single source of truth), the 120s refresh
//! pipeline with per-account backoff, the action handlers, and threshold
//! notifications. Logic mirrors PitStop's `AppDelegate`, now OS-agnostic.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::credentials::CredentialBlob;
use crate::notify::Notifier;
use crate::prefs::{IndicatorMetric, IndicatorPrefs, IndicatorStyle};
use crate::profile_store::{Profile, ProfileStore};
use crate::ui_events::{self, UiSnapshot};
use crate::usage_api::{self, UsageError, UsageReport};

/// Regular refresh cadence.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(120);
/// Don't auto-refresh on popover open if data is fresher than this.
pub const MENU_REFRESH_DEBOUNCE: Duration = Duration::from_secs(30);
/// Backoff cap for rate-limited accounts (15 min).
const BACKOFF_CAP: Duration = Duration::from_secs(900);
/// A rejected token won't self-heal: back off an hour.
const UNAUTHORIZED_BACKOFF: Duration = Duration::from_secs(3600);

/// Mutable application state. Single source of truth; guarded by an async lock.
#[derive(Default)]
pub struct AppState {
    pub profiles: Vec<Profile>,
    pub active_email: Option<String>,
    /// Last **successful** report per account (kept on failure → staleness).
    pub usage: HashMap<String, UsageReport>,
    pub fetch_error: HashMap<String, String>,
    /// Per-account backoff gate (always future-or-absent).
    pub next_fetch_allowed: HashMap<String, Instant>,
    pub failure_count: HashMap<String, u32>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub last_top_level_error: Option<String>,
    pub refreshing: bool,
    pub refresh_queued: bool,
    /// Threshold bucket already notified per account (0/1/2).
    pub notified_bucket: HashMap<String, u8>,
    pub prefs: IndicatorPrefs,
}

impl AppState {
    /// The utilization figure the tray should display, per the metric pref.
    pub fn indicator_utilization(&self) -> Option<f64> {
        let email = self.active_email.as_ref()?;
        let report = self.usage.get(email)?;
        match self.prefs.metric {
            IndicatorMetric::Binding => report.max_utilization(),
            IndicatorMetric::FiveHour => report.five_hour.utilization,
            IndicatorMetric::Weekly => report.seven_day.utilization,
        }
    }

    /// The active account currently shows stale data (has a report *and* a live
    /// fetch error).
    pub fn active_is_stale(&self) -> bool {
        match &self.active_email {
            Some(e) => self.usage.contains_key(e) && self.fetch_error.contains_key(e),
            None => false,
        }
    }

    fn is_in_backoff(&self, email: &str, now: Instant) -> bool {
        self.next_fetch_allowed
            .get(email)
            .map(|gate| *gate > now)
            .unwrap_or(false)
    }
}

/// The Tauri-managed controller: owns state + the two helpers. Cloned (via
/// `Arc`) into background tasks.
pub struct Controller {
    pub state: RwLock<AppState>,
    pub store: ProfileStore,
    pub notifier: Notifier,
    /// Serializes the refresh pipeline so single-flight is airtight.
    refresh_lock: Mutex<()>,
}

impl Controller {
    pub fn new(store: ProfileStore) -> Self {
        Self {
            state: RwLock::new(AppState::default()),
            store,
            notifier: Notifier::new(),
            refresh_lock: Mutex::new(()),
        }
    }
}

pub type SharedController = Arc<Controller>;

/// Load persisted indicator prefs from the store into state.
pub async fn load_prefs(ctrl: &Controller, app: &AppHandle) {
    use tauri_plugin_store::StoreExt;
    let Ok(store) = app.store(crate::prefs::STORE_FILE) else {
        return;
    };
    let style = store
        .get(crate::prefs::KEY_STYLE)
        .and_then(|v| v.as_str().map(str::to_string))
        .and_then(|s| IndicatorStyle::from_key(&s))
        .unwrap_or_default();
    let metric = store
        .get(crate::prefs::KEY_METRIC)
        .and_then(|v| v.as_str().map(str::to_string))
        .and_then(|s| IndicatorMetric::from_key(&s))
        .unwrap_or_default();
    ctrl.state.write().await.prefs = IndicatorPrefs { style, metric };
}

// ---------------------------------------------------------------------------
// Refresh pipeline
// ---------------------------------------------------------------------------

/// Run the full refresh, single-flighted and coalescing.
pub async fn refresh_all(ctrl: SharedController, app: AppHandle) {
    // Single-flight: if a refresh is running, queue one and return.
    {
        let mut s = ctrl.state.write().await;
        if s.refreshing {
            s.refresh_queued = true;
            return;
        }
        s.refreshing = true;
    }

    let _guard = ctrl.refresh_lock.lock().await;
    loop {
        run_refresh_once(&ctrl, &app).await;

        let mut s = ctrl.state.write().await;
        if s.refresh_queued {
            s.refresh_queued = false; // coalesced: run again
        } else {
            s.refreshing = false;
            break;
        }
    }

    schedule_backoff_retry(&ctrl, &app).await;
}

async fn run_refresh_once(ctrl: &Controller, app: &AppHandle) {
    let now = Instant::now();
    let now_ms = Utc::now().timestamp_millis();

    // 1) Capture the live account into a saved profile.
    if let Err(e) = ctrl.store.capture_current().await {
        ctrl.state.write().await.last_top_level_error = Some(format!("capture: {e}"));
        tracing::warn!(error = %e, "capture_current failed");
    } else {
        ctrl.state.write().await.last_top_level_error = None;
    }

    // 2) Reload profiles + active email.
    let (profiles, active_email) = {
        let profiles = ctrl.store.load().unwrap_or_default();
        let active = ctrl.store.active_email().unwrap_or(None);
        let mut s = ctrl.state.write().await;
        s.profiles = profiles.clone();
        s.active_email = active.clone();
        (profiles, active)
    };

    // 3) Per-profile fetch (skipping accounts inside their backoff window).
    for profile in &profiles {
        let email = profile.email.clone();
        let is_active = active_email.as_deref() == Some(email.as_str());

        if ctrl.state.read().await.is_in_backoff(&email, now) {
            continue;
        }

        match refresh_one(ctrl, &email, is_active, now_ms).await {
            Ok(report) => {
                let mut s = ctrl.state.write().await;
                s.usage.insert(email.clone(), report);
                s.fetch_error.remove(&email);
                s.failure_count.remove(&email);
                s.next_fetch_allowed.remove(&email);
            }
            Err(err) => apply_error(ctrl, &email, err).await,
        }
    }

    // 4) Finish: stamp, re-render tray, push snapshot, run thresholds.
    ctrl.state.write().await.last_refresh = Some(Utc::now());
    update_tray(app, ctrl).await;
    emit_snapshot(app, ctrl).await;
    check_thresholds(ctrl, app).await;
}

/// Fetch usage for one account, refreshing its token first if needed.
async fn refresh_one(
    ctrl: &Controller,
    email: &str,
    is_active: bool,
    now_ms: i64,
) -> Result<UsageReport, UsageError> {
    let access = fresh_credentials(ctrl, email, is_active, now_ms).await?;
    usage_api::fetch_usage(&access).await
}

/// Return a non-expired access token for the account. For the active account we
/// trust Claude Code to keep it fresh; for saved accounts we run the OAuth
/// refresh grant when expired and persist the patched blob.
async fn fresh_credentials(
    ctrl: &Controller,
    email: &str,
    is_active: bool,
    now_ms: i64,
) -> Result<String, UsageError> {
    let raw = ctrl
        .store
        .blob_for(email, is_active)
        .await
        .map_err(|e| UsageError::Decode(e.to_string()))?
        .ok_or_else(|| UsageError::Decode("no credential blob".into()))?;

    let blob = CredentialBlob::parse(&raw).map_err(|e| UsageError::Decode(e.to_string()))?;
    let creds = blob
        .credentials()
        .map_err(|e| UsageError::Decode(e.to_string()))?;

    // Fresh, or active (Claude Code owns the active token) → use as-is.
    if is_active || !creds.is_expired(now_ms) {
        return Ok(creds.access_token);
    }

    // Expired saved account: refresh via OAuth if we have a refresh token.
    let Some(refresh) = &creds.refresh_token else {
        return Err(UsageError::Unauthorized);
    };
    let fresh = usage_api::refresh_token(refresh, now_ms).await?;
    // Preserve subscription/tier metadata the refresh endpoint doesn't return.
    let merged = usage_api::OAuthCredentials {
        subscription_type: creds.subscription_type.clone(),
        rate_limit_tier: creds.rate_limit_tier.clone(),
        ..fresh.clone()
    };
    let patched = blob
        .patching(&merged)
        .map_err(|e| UsageError::Decode(e.to_string()))?;
    ctrl.store
        .store_refreshed_blob(email, is_active, &patched.to_bytes())
        .await
        .map_err(|e| UsageError::Decode(e.to_string()))?;
    Ok(merged.access_token)
}

/// Record an error and set the appropriate backoff gate.
async fn apply_error(ctrl: &Controller, email: &str, err: UsageError) {
    let mut s = ctrl.state.write().await;
    s.fetch_error.insert(email.to_string(), err.to_string());

    match err {
        UsageError::RateLimited(retry_after) => {
            let n = s.failure_count.entry(email.to_string()).or_insert(0);
            *n += 1;
            let backoff = match retry_after {
                Some(secs) => Duration::from_secs(secs),
                None => {
                    let shift = (*n - 1).min(4);
                    let exp = 120u64.saturating_mul(1u64 << shift);
                    Duration::from_secs(exp).min(BACKOFF_CAP)
                }
            };
            s.next_fetch_allowed
                .insert(email.to_string(), Instant::now() + backoff);
        }
        UsageError::Unauthorized => {
            s.next_fetch_allowed
                .insert(email.to_string(), Instant::now() + UNAUTHORIZED_BACKOFF);
        }
        // Other errors: record the message, no backoff.
        _ => {}
    }
}

/// One-shot retry: sleep until the earliest future backoff gate, then refresh —
/// so a rate-limited account doesn't idle a full cycle. Skipped when the gate is
/// further out than the regular interval.
async fn schedule_backoff_retry(ctrl: &SharedController, app: &AppHandle) {
    let now = Instant::now();
    let earliest = {
        let s = ctrl.state.read().await;
        s.next_fetch_allowed
            .values()
            .filter(|gate| **gate > now)
            .min()
            .copied()
    };
    let Some(gate) = earliest else { return };

    let wait = gate.saturating_duration_since(now);
    if wait >= REFRESH_INTERVAL {
        return; // the regular cadence will cover it
    }
    let wait = wait.max(Duration::from_secs(10)); // floor

    let ctrl = ctrl.clone();
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        refresh_all_task(ctrl, app).await;
    });
}

/// `refresh_all` as an explicitly-`Send` boxed future. This anchors the
/// `refresh_all` → `schedule_backoff_retry` → spawn-`refresh_all` recursion so
/// the compiler can prove `Send` (it would otherwise fail to resolve the cyclic
/// auto-trait inference, even though nothing in the cycle is genuinely `!Send`).
fn refresh_all_task(
    ctrl: SharedController,
    app: AppHandle,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(refresh_all(ctrl, app))
}

/// Push a fresh snapshot to the WebView panel.
pub async fn emit_snapshot(app: &AppHandle, ctrl: &Controller) {
    let snapshot = {
        let s = ctrl.state.read().await;
        UiSnapshot::build(&s)
    };
    if let Err(e) = app.emit(ui_events::SNAPSHOT_EVENT, snapshot) {
        tracing::warn!(error = %e, "failed to emit snapshot");
    }
}

/// Re-render the tray icon, tooltip, and menu from current state.
///
/// All the tauri tray/menu types are `!Send`, so the state is snapshotted into
/// `Send` models under the async lock and the actual UI mutation is performed on
/// the main thread (where tray UI work belongs anyway).
pub async fn update_tray(app: &AppHandle, ctrl: &Controller) {
    let (visual, tooltip, model) = {
        let s = ctrl.state.read().await;
        (
            crate::tray::TrayVisual::from_state(&s),
            crate::tray::tooltip(&s),
            crate::tray::MenuModel::from_state(&s),
        )
    };
    let launch = crate::actions::is_launch_at_login(app);
    let app2 = app.clone();

    let _ = app.run_on_main_thread(move || {
        let Some(tray) = app2.tray_by_id("main") else {
            return;
        };
        match crate::tray::render_icon(&visual) {
            Ok(icon) => {
                let _ = tray.set_icon(Some(icon));
                #[cfg(target_os = "macos")]
                let _ = tray
                    .set_icon_as_template(matches!(visual.prefs.style, IndicatorStyle::IconOnly));
            }
            Err(e) => tracing::warn!(error = %e, "tray icon render failed"),
        }
        let _ = tray.set_tooltip(Some(tooltip.as_str()));
        match crate::tray::build_menu(&app2, &model, launch) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(e) => tracing::warn!(error = %e, "menu build failed"),
        }
    });
}

/// Rebuild and attach the native context menu (main thread).
pub async fn rebuild_menu(app: &AppHandle, ctrl: &Controller) {
    let model = {
        let s = ctrl.state.read().await;
        crate::tray::MenuModel::from_state(&s)
    };
    let launch = crate::actions::is_launch_at_login(app);
    let app2 = app.clone();

    let _ = app.run_on_main_thread(move || {
        let Some(tray) = app2.tray_by_id("main") else {
            return;
        };
        match crate::tray::build_menu(&app2, &model, launch) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(e) => tracing::warn!(error = %e, "menu build failed"),
        }
    });
}

// ---------------------------------------------------------------------------
// Threshold notifications
// ---------------------------------------------------------------------------

/// Notify on an upward threshold crossing for the active account.
async fn check_thresholds(ctrl: &Controller, app: &AppHandle) {
    // Decide everything under a single write lock so the bucket update and the
    // crossing test can't race.
    let message = {
        let mut s = ctrl.state.write().await;
        let Some(active) = s.active_email.clone() else {
            return;
        };
        // Need a current, error-free report.
        if s.fetch_error.contains_key(&active) {
            return;
        }
        let Some(pct) = s
            .usage
            .get(&active)
            .and_then(|r| r.max_utilization())
            .map(|u| u * 100.0)
        else {
            return;
        };

        let bucket = threshold_bucket(pct);
        let prev = s.notified_bucket.get(&active).copied().unwrap_or(0);
        // Always record the current bucket (so a later drop+rise re-notifies).
        s.notified_bucket.insert(active.clone(), bucket);

        if bucket <= prev {
            return; // not an upward crossing
        }

        let report = s.usage.get(&active).expect("checked above");
        let reset = ui_events::binding_reset_text(report);
        let pit = best_pit(&s, &active);
        Some((
            format!("Claude usage at {}%", pct.round() as i64),
            format!("Active account {reset}. {pit}"),
        ))
    };

    if let Some((title, body)) = message {
        ctrl.notifier.notify(app, &title, &body);
    }
}

fn threshold_bucket(pct: f64) -> u8 {
    if pct >= 95.0 {
        2
    } else if pct >= 80.0 {
        1
    } else {
        0
    }
}

/// Find the "best pit": the saved non-active account with the lowest
/// `max_utilization`, and phrase the advice line.
fn best_pit(s: &AppState, active: &str) -> String {
    let mut best: Option<(&str, f64)> = None;
    let mut any_other = false;
    for p in &s.profiles {
        if p.email == active {
            continue;
        }
        any_other = true;
        if let Some(report) = s.usage.get(&p.email) {
            if let Some(u) = report.max_utilization() {
                if best.map(|(_, bu)| u < bu).unwrap_or(true) {
                    best = Some((p.email.as_str(), u));
                }
            }
        }
    }

    match best {
        Some((email, u)) if u < 0.80 => {
            format!(
                "Best pit: {email} ({}% used) — switch from the menu.",
                (u * 100.0).round() as i64
            )
        }
        Some(_) => "All saved accounts are running hot — check the menu.".to_string(),
        None if any_other => "All saved accounts are running hot — check the menu.".to_string(),
        None => "Add a second account in PitStopX to keep working.".to_string(),
    }
}

/// Spawn the regular 120s refresh loop.
pub fn spawn_refresh_loop(ctrl: SharedController, app: AppHandle) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REFRESH_INTERVAL);
        loop {
            ticker.tick().await;
            refresh_all(ctrl.clone(), app.clone()).await;
        }
    });
}

/// Convenience accessor for the managed controller.
pub fn controller(app: &AppHandle) -> SharedController {
    app.state::<SharedController>().inner().clone()
}
