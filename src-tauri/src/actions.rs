//! Action handlers: the Tauri commands invoked from the panel and the shared
//! implementations the native-menu handlers also call. All actions ultimately
//! mutate state through the `Controller` and re-emit a snapshot.

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::app::{self, SharedController};
use crate::prefs::{IndicatorMetric, IndicatorStyle};
use crate::provider::Provider;
use crate::source::secret_key;

/// Result type surfaced to the panel: a short error string on failure.
type CmdResult = Result<(), String>;

fn ctrl(app: &AppHandle) -> SharedController {
    app.state::<SharedController>().inner().clone()
}

// ---------------------------------------------------------------------------
// Shared implementations (used by both commands and the native menu)
// ---------------------------------------------------------------------------

pub async fn do_switch_to(app: &AppHandle, provider: Provider, email: &str) -> CmdResult {
    let ctrl = ctrl(app);
    ctrl.store
        .switch_to(provider, email)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut s = ctrl.state.write().await;
        // Reset the switched-to account's notify bucket.
        s.notified_bucket.insert(secret_key(provider, email), 0);
    }
    ctrl.notifier.notify(
        app,
        "Switched account",
        &format!("Now using {email} ({})", provider.display_name()),
    );
    app::refresh_all(ctrl, app.clone()).await;
    Ok(())
}

/// Start a browser login for a *new* account of `provider`: save the current
/// one first (so it isn't lost when the login overwrites the live creds), open
/// the provider's login terminal, then refresh a few times to pick up the new
/// account once the user finishes.
pub async fn do_login(app: &AppHandle, provider: Provider) -> CmdResult {
    let ctrl = ctrl(app);

    // User-set CLI path override (settings page), if any.
    let override_path = ctrl
        .state
        .read()
        .await
        .cli_paths
        .get(provider)
        .map(str::to_string);

    // Verify the provider's CLI is installed *before* touching any credentials —
    // we clear the live login below, so a missing CLI must fail fast and clean
    // rather than leave the user logged out with no way to sign back in. Surface
    // it as a notification too, since the tray-menu entry point shows nothing.
    if let Err(e) = crate::login::ensure_installed(provider, override_path.as_deref()) {
        let msg = e.to_string();
        ctrl.notifier.notify(
            app,
            &format!("{} CLI not found", provider.display_name()),
            &msg,
        );
        return Err(msg);
    }

    // Snapshot the current account(s) so a switch-back is always possible. This
    // only reads the machine credential, so the outgoing account is preserved
    // before we clear it below.
    let _ = ctrl.store.capture_current().await;

    // Remove the live credential before launching the login: some providers
    // invalidate or reuse whatever is already on disk when a new login starts,
    // which would corrupt the outgoing account. Abort if we can't clear it.
    ctrl.store
        .clear_live(provider)
        .await
        .map_err(|e| e.to_string())?;

    crate::login::launch(provider, override_path.as_deref()).map_err(|e| e.to_string())?;

    ctrl.notifier.notify(
        app,
        &format!("Log in to {}", provider.display_name()),
        "Finish signing in via the opened terminal/browser. PitStopX will pick up the new account automatically.",
    );

    // Poll a few times so the new account appears shortly after login.
    let ctrl2 = ctrl.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        for delay in [10u64, 20, 40, 80, 120] {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            app::refresh_all(ctrl2.clone(), app2.clone()).await;
        }
    });
    Ok(())
}

pub async fn do_save_current(app: &AppHandle) -> CmdResult {
    let ctrl = ctrl(app);
    match ctrl.store.capture_current().await {
        Ok(captured) if !captured.is_empty() => {
            let list = captured
                .iter()
                .map(|(p, e)| format!("{e} ({})", p.display_name()))
                .collect::<Vec<_>>()
                .join(", ");
            ctrl.notifier
                .notify(app, "Saved account", &format!("Saved {list}"));
        }
        Ok(_) => {
            ctrl.notifier.notify(
                app,
                "Nothing to save",
                "No active Claude/Codex account found.",
            );
        }
        Err(e) => return Err(e.to_string()),
    }
    app::refresh_all(ctrl, app.clone()).await;
    Ok(())
}

pub async fn do_remove_account(app: &AppHandle, provider: Provider, email: &str) -> CmdResult {
    let ctrl = ctrl(app);
    ctrl.store
        .remove(provider, email)
        .await
        .map_err(|e| e.to_string())?;
    let key = secret_key(provider, email);
    {
        let mut s = ctrl.state.write().await;
        s.profiles
            .retain(|p| !(p.provider == provider && p.email == email));
        s.usage.remove(&key);
        s.fetch_error.remove(&key);
        s.next_fetch_allowed.remove(&key);
        s.failure_count.remove(&key);
        s.notified_bucket.remove(&key);
        s.active_keys.remove(&key);
    }
    app::update_tray(app, &ctrl).await;
    app::emit_snapshot(app, &ctrl).await;
    Ok(())
}

pub async fn do_refresh_now(app: &AppHandle) -> CmdResult {
    let ctrl = ctrl(app);
    // Clear ALL backoff so Refresh Now truly forces a fetch.
    {
        let mut s = ctrl.state.write().await;
        s.next_fetch_allowed.clear();
        s.failure_count.clear();
    }
    app::refresh_all(ctrl, app.clone()).await;
    Ok(())
}

pub async fn do_set_indicator_style(app: &AppHandle, style: IndicatorStyle) -> CmdResult {
    persist_pref(app, crate::prefs::KEY_STYLE, style.as_key());
    let ctrl = ctrl(app);
    ctrl.state.write().await.prefs.style = style;
    app::update_tray(app, &ctrl).await;
    Ok(())
}

pub async fn do_set_indicator_metric(app: &AppHandle, metric: IndicatorMetric) -> CmdResult {
    persist_pref(app, crate::prefs::KEY_METRIC, metric.as_key());
    let ctrl = ctrl(app);
    ctrl.state.write().await.prefs.metric = metric;
    app::update_tray(app, &ctrl).await;
    Ok(())
}

pub fn do_set_launch_at_login(app: &AppHandle, enabled: bool) -> CmdResult {
    let mgr = app.autolaunch();
    let res = if enabled { mgr.enable() } else { mgr.disable() };
    res.map_err(|e| e.to_string())
}

pub fn is_launch_at_login(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

pub fn do_quit(app: &AppHandle) {
    app.exit(0);
}

/// Persist (and apply) a manual CLI path override for a provider. An empty path
/// clears the override, reverting to auto-detection.
pub async fn do_set_cli_path(app: &AppHandle, provider: Provider, path: String) -> CmdResult {
    let trimmed = path.trim().to_string();
    persist_pref(app, crate::prefs::CliPaths::key_for(provider), &trimmed);
    let ctrl = ctrl(app);
    let value = (!trimmed.is_empty()).then_some(trimmed);
    let mut s = ctrl.state.write().await;
    match provider {
        Provider::Anthropic => s.cli_paths.claude = value,
        Provider::OpenAI => s.cli_paths.codex = value,
    }
    Ok(())
}

/// Settings surfaced to the panel: the saved overrides plus the *effective*
/// resolved CLI path (override or auto-detected), or `null` when not found.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub claude_bin: Option<String>,
    pub codex_bin: Option<String>,
    pub claude_resolved: Option<String>,
    pub codex_resolved: Option<String>,
    /// Global open-popover hotkey accelerator (empty = none).
    pub shortcut: String,
}

/// Set (and persist) the global open-popover hotkey. An empty string clears it.
/// Re-registration is validated first; on failure the previous hotkey is kept.
pub async fn do_set_shortcut(app: &AppHandle, accel: String) -> CmdResult {
    let accel = accel.trim().to_string();
    let ctrl = ctrl(app);
    let old = ctrl.state.read().await.shortcut.clone();
    if let Err(e) = crate::set_open_shortcut(app, Some(&old), &accel) {
        // Registration failed — restore the previous hotkey so we keep one.
        let _ = crate::set_open_shortcut(app, None, &old);
        return Err(e);
    }
    persist_pref(app, crate::prefs::KEY_SHORTCUT, &accel);
    ctrl.state.write().await.shortcut = accel;
    app::rebuild_menu(app, &ctrl).await;
    Ok(())
}

fn persist_pref(app: &AppHandle, key: &str, value: &str) {
    use tauri_plugin_store::StoreExt;
    if let Ok(store) = app.store(crate::prefs::STORE_FILE) {
        store.set(key, serde_json::Value::String(value.to_string()));
        let _ = store.save();
    }
}

// ---------------------------------------------------------------------------
// Tauri commands (invoked from the Svelte panel)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn switch_to(app: AppHandle, email: String, provider: String) -> CmdResult {
    do_switch_to(&app, Provider::from_id(&provider), &email).await
}

#[tauri::command]
pub async fn save_current(app: AppHandle) -> CmdResult {
    do_save_current(&app).await
}

#[tauri::command]
pub async fn login_new(app: AppHandle, provider: String) -> CmdResult {
    do_login(&app, Provider::from_id(&provider)).await
}

#[tauri::command]
pub async fn remove_account(app: AppHandle, email: String, provider: String) -> CmdResult {
    do_remove_account(&app, Provider::from_id(&provider), &email).await
}

#[tauri::command]
pub async fn refresh_now(app: AppHandle) -> CmdResult {
    do_refresh_now(&app).await
}

#[tauri::command]
pub async fn set_indicator_style(app: AppHandle, value: String) -> CmdResult {
    let style = IndicatorStyle::from_key(&value).ok_or("unknown indicator style")?;
    do_set_indicator_style(&app, style).await
}

#[tauri::command]
pub async fn set_indicator_metric(app: AppHandle, value: String) -> CmdResult {
    let metric = IndicatorMetric::from_key(&value).ok_or("unknown indicator metric")?;
    do_set_indicator_metric(&app, metric).await
}

#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, enabled: bool) -> CmdResult {
    do_set_launch_at_login(&app, enabled)
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    do_quit(&app);
}

/// Let the panel pull the current snapshot on mount (before the first push).
#[tauri::command]
pub async fn request_snapshot(app: AppHandle) -> Result<(), String> {
    let ctrl = ctrl(&app);
    app::emit_snapshot(&app, &ctrl).await;
    Ok(())
}

#[tauri::command]
pub async fn set_cli_path(app: AppHandle, provider: String, path: String) -> CmdResult {
    do_set_cli_path(&app, Provider::from_id(&provider), path).await
}

#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Result<SettingsDto, String> {
    let ctrl = ctrl(&app);
    let (paths, shortcut) = {
        let s = ctrl.state.read().await;
        (s.cli_paths.clone(), s.shortcut.clone())
    };
    Ok(SettingsDto {
        claude_resolved: crate::login::resolve_program(
            Provider::Anthropic,
            paths.claude.as_deref(),
        )
        .map(|p| p.display().to_string()),
        codex_resolved: crate::login::resolve_program(Provider::OpenAI, paths.codex.as_deref())
            .map(|p| p.display().to_string()),
        claude_bin: paths.claude,
        codex_bin: paths.codex,
        shortcut,
    })
}

#[tauri::command]
pub async fn set_shortcut(app: AppHandle, shortcut: String) -> CmdResult {
    do_set_shortcut(&app, shortcut).await
}
