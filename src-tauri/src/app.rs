//! Application core: `AppState` (the single source of truth), the 120s refresh
//! pipeline with per-account backoff, the action handlers, and threshold
//! notifications. Logic mirrors PitStop's `AppDelegate`, now OS-agnostic.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::notify::Notifier;
use crate::prefs::{IndicatorMetric, IndicatorPrefs, IndicatorStyle};
use crate::profile_store::{Profile, ProfileStore};
use crate::provider::Provider;
use crate::source::secret_key;
use crate::ui_events::{self, UiSnapshot};
use crate::usage_api::{UsageError, UsageReport};

/// Regular refresh cadence.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(120);
/// Don't auto-refresh on popover open if data is fresher than this.
pub const MENU_REFRESH_DEBOUNCE: Duration = Duration::from_secs(30);
/// Backoff cap for rate-limited accounts (15 min).
const BACKOFF_CAP: Duration = Duration::from_secs(900);
/// A rejected token won't self-heal: back off an hour.
const UNAUTHORIZED_BACKOFF: Duration = Duration::from_secs(3600);

/// Mutable application state. Single source of truth; guarded by an async lock.
///
/// All per-account maps are keyed by `Profile::key()` (the secret-store key),
/// which is the account email for Anthropic and `"<provider>:<email>"` for
/// other providers, so accounts never collide across providers.
#[derive(Default)]
pub struct AppState {
    pub profiles: Vec<Profile>,
    /// The "primary" active account driving the tray indicator/tooltip — the
    /// active account (any provider) with the highest utilization.
    pub active_primary: Option<(Provider, String)>,
    /// Keys of every currently-active account (one per provider).
    pub active_keys: HashSet<String>,
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
    /// Secret-store/cache key of the primary active account.
    pub fn primary_key(&self) -> Option<String> {
        self.active_primary.as_ref().map(|(p, e)| secret_key(*p, e))
    }

    /// Email of the primary active account (for the snapshot + tooltip).
    pub fn primary_email(&self) -> Option<&str> {
        self.active_primary.as_ref().map(|(_, e)| e.as_str())
    }

    /// The utilization figure the tray should display, per the metric pref.
    pub fn indicator_utilization(&self) -> Option<f64> {
        let key = self.primary_key()?;
        let report = self.usage.get(&key)?;
        match self.prefs.metric {
            IndicatorMetric::Binding => report.max_utilization(),
            IndicatorMetric::FiveHour => report.five_hour.utilization,
            IndicatorMetric::Weekly => report.seven_day.utilization,
        }
    }

    /// The primary account currently shows stale data (report *and* live error).
    pub fn active_is_stale(&self) -> bool {
        match self.primary_key() {
            Some(k) => self.usage.contains_key(&k) && self.fetch_error.contains_key(&k),
            None => false,
        }
    }

    fn is_in_backoff(&self, key: &str, now: Instant) -> bool {
        self.next_fetch_allowed
            .get(key)
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

    // 2) Reload profiles + the active account per provider.
    let (profiles, active) = {
        let profiles = ctrl.store.load().unwrap_or_default();
        let active = ctrl.store.active_accounts().await;
        let active_keys: HashSet<String> = active.iter().map(|(p, e)| secret_key(*p, e)).collect();
        let mut s = ctrl.state.write().await;
        s.profiles = profiles.clone();
        s.active_keys = active_keys;
        (profiles, active)
    };
    let active_keys: HashSet<String> = active.iter().map(|(p, e)| secret_key(*p, e)).collect();

    // 3) Per-profile fetch (skipping accounts inside their backoff window).
    for profile in &profiles {
        let provider = profile.provider;
        let email = profile.email.clone();
        let key = profile.key();
        let is_active = active_keys.contains(&key);

        if ctrl.state.read().await.is_in_backoff(&key, now) {
            continue;
        }

        match refresh_one(ctrl, provider, &email, is_active, now_ms).await {
            Ok(fetched) => {
                // A refreshed blob means the provider rotated the token; the old
                // one is now revoked server-side. Persisting the new one is
                // mandatory — if the write fails we've burned the saved login, so
                // surface it loudly and back off rather than silently dropping it.
                if let Some(blob) = fetched.refreshed_blob {
                    if let Err(e) = ctrl
                        .store
                        .store_refreshed_blob(provider, &email, &blob)
                        .await
                    {
                        tracing::error!(
                            error = %e, %email,
                            "failed to persist refreshed token; saved login is now stale"
                        );
                        let mut s = ctrl.state.write().await;
                        s.usage.insert(key.clone(), fetched.report);
                        s.fetch_error.insert(
                            key.clone(),
                            "could not save refreshed login — re-add this account".to_string(),
                        );
                        s.next_fetch_allowed
                            .insert(key.clone(), Instant::now() + UNAUTHORIZED_BACKOFF);
                        continue;
                    }
                }
                let mut s = ctrl.state.write().await;
                s.usage.insert(key.clone(), fetched.report);
                s.fetch_error.remove(&key);
                s.failure_count.remove(&key);
                s.next_fetch_allowed.remove(&key);
            }
            Err(err) => apply_error(ctrl, &key, err).await,
        }
    }

    // 4) Pick the primary active account (highest utilization), stamp, render.
    {
        let mut s = ctrl.state.write().await;
        let primary = pick_primary(&s, &active);
        s.active_primary = primary;
        s.last_refresh = Some(Utc::now());
    }
    update_tray(app, ctrl).await;
    emit_snapshot(app, ctrl).await;
    check_thresholds(ctrl, app).await;
}

/// Choose the active account that should drive the tray indicator: the one with
/// the highest known utilization, ties broken by provider order (Anthropic
/// first). Falls back to the first active account when no usage is known yet.
fn pick_primary(s: &AppState, active: &[(Provider, String)]) -> Option<(Provider, String)> {
    active
        .iter()
        .max_by(|a, b| {
            let ua = s
                .usage
                .get(&secret_key(a.0, &a.1))
                .and_then(|r| r.max_utilization())
                .unwrap_or(-1.0);
            let ub = s
                .usage
                .get(&secret_key(b.0, &b.1))
                .and_then(|r| r.max_utilization())
                .unwrap_or(-1.0);
            ua.partial_cmp(&ub).unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

/// Fetch usage for one account via the provider-specific engine.
async fn refresh_one(
    ctrl: &Controller,
    provider: Provider,
    email: &str,
    is_active: bool,
    now_ms: i64,
) -> Result<crate::engine::Fetched, UsageError> {
    let raw = ctrl
        .store
        .blob_for(provider, email, is_active)
        .await
        .map_err(|e| UsageError::Decode(e.to_string()))?
        .ok_or_else(|| UsageError::Decode("no credential blob".into()))?;
    crate::engine::fetch(provider, &raw, is_active, now_ms).await
}

/// Record an error and set the appropriate backoff gate (keyed by account key).
async fn apply_error(ctrl: &Controller, key: &str, err: UsageError) {
    let mut s = ctrl.state.write().await;
    s.fetch_error.insert(key.to_string(), err.to_string());

    match err {
        UsageError::RateLimited(retry_after) => {
            let n = s.failure_count.entry(key.to_string()).or_insert(0);
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
                .insert(key.to_string(), Instant::now() + backoff);
        }
        UsageError::Unauthorized => {
            s.next_fetch_allowed
                .insert(key.to_string(), Instant::now() + UNAUTHORIZED_BACKOFF);
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
        let (Some(key), Some((provider, _))) = (s.primary_key(), s.active_primary.clone()) else {
            return;
        };
        // Need a current, error-free report.
        if s.fetch_error.contains_key(&key) {
            return;
        }
        let Some(pct) = s
            .usage
            .get(&key)
            .and_then(|r| r.max_utilization())
            .map(|u| u * 100.0)
        else {
            return;
        };

        let bucket = threshold_bucket(pct);
        let prev = s.notified_bucket.get(&key).copied().unwrap_or(0);
        // Always record the current bucket (so a later drop+rise re-notifies).
        s.notified_bucket.insert(key.clone(), bucket);

        if bucket <= prev {
            return; // not an upward crossing
        }

        let report = s.usage.get(&key).expect("checked above");
        let reset = ui_events::binding_reset_text(report);
        let pit = best_pit(&s, &key);
        Some((
            format!(
                "{} usage at {}%",
                provider.display_name(),
                pct.round() as i64
            ),
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
/// `max_utilization`, and phrase the advice line. `active_key` is the primary
/// active account's cache key.
fn best_pit(s: &AppState, active_key: &str) -> String {
    let mut best: Option<(&str, f64)> = None;
    let mut any_other = false;
    for p in &s.profiles {
        if p.key() == active_key {
            continue;
        }
        any_other = true;
        if let Some(report) = s.usage.get(&p.key()) {
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
