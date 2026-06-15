//! Linux global open-popover hotkey via the XDG **GlobalShortcuts** portal
//! (`org.freedesktop.portal.GlobalShortcuts`), which is the only mechanism that
//! works under Wayland. The `tauri-plugin-global-shortcut` backend
//! (`global-hotkey`) is X11-only, so we use `ashpd` here and fall back to that
//! plugin's X11 grab when the portal isn't available (e.g. GNOME < 45).
//!
//! Flow: create a portal session, `BindShortcuts` a single "open" shortcut with
//! our accelerator as the *preferred* trigger (the compositor shows a one-time
//! approval dialog and ultimately owns the key), then translate `Activated`
//! signals into showing the popover. The session is held alive by the listening
//! task; changing the hotkey aborts it (dropping the session → unbinding) and
//! starts a fresh one.

use std::sync::Mutex;

use tauri::AppHandle;

/// Stable id for our single shortcut, matched in the `Activated` signal.
const SHORTCUT_ID: &str = "open-pitstopx";

/// The live portal listener task; aborted + replaced whenever the hotkey changes.
static TASK: Mutex<Option<tauri::async_runtime::JoinHandle<()>>> = Mutex::new(None);

/// (Re)bind the global open-popover hotkey to `accel` (an accelerator like
/// `"CmdOrCtrl+Shift+U"`; empty clears it). Non-blocking: the portal work runs
/// in a background task so the caller (settings/startup) isn't held up by the
/// approval dialog.
pub fn rebind(app: &AppHandle, accel: &str) {
    if let Some(handle) = TASK.lock().unwrap().take() {
        handle.abort();
    }
    let trigger = to_portal_trigger(accel);
    if trigger.is_empty() {
        return; // cleared → leave unbound
    }
    let app = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = portal_loop(&app, &trigger).await {
            tracing::warn!(error = %e, "GlobalShortcuts portal unavailable; trying X11 fallback");
            if let Err(e) = fallback_x11(&app, &trigger) {
                tracing::warn!(error = %e, "X11 global shortcut fallback failed");
            }
        }
    });
    *TASK.lock().unwrap() = Some(handle);
}

/// Bind via the portal and stream activations until the session ends. Holding
/// `session` for the loop's lifetime keeps the binding registered.
async fn portal_loop(app: &AppHandle, trigger: &str) -> Result<(), ashpd::Error> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;

    let shortcuts = GlobalShortcuts::new().await?;
    let session = shortcuts.create_session(Default::default()).await?;
    let new = NewShortcut::new(SHORTCUT_ID, "Open PitStopX").preferred_trigger(Some(trigger));
    // `bind_shortcuts` returns a portal Request; calling `.response()` confirms
    // the binding was created (and surfaces a denied/unsupported error).
    shortcuts
        .bind_shortcuts(&session, &[new], None, Default::default())
        .await?
        .response()?;

    let mut activated = shortcuts.receive_activated().await?;
    while let Some(act) = activated.next().await {
        if act.shortcut_id() == SHORTCUT_ID {
            crate::show_popover_on_main_thread(app);
        }
    }
    drop(session);
    Ok(())
}

/// X11 fallback for compositors without GlobalShortcuts portal support: register
/// through the plugin (whose handler, set in `lib::run`, shows the popover).
fn fallback_x11(app: &AppHandle, trigger: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    app.global_shortcut()
        .register(trigger)
        .map_err(|e| e.to_string())
}

/// Convert our accelerator (`CmdOrCtrl+Shift+U`) to the portal/X11 trigger form
/// (`Ctrl+Shift+U`). Trimmed/empty input yields an empty string.
fn to_portal_trigger(accel: &str) -> String {
    let accel = accel.trim();
    if accel.is_empty() {
        return String::new();
    }
    accel
        .split('+')
        .map(|p| match p {
            "CmdOrCtrl" | "CommandOrControl" => "Ctrl",
            "Cmd" | "Command" => "Super",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}
