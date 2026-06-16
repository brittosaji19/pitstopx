//! Linux global open-popover hotkey via the XDG **GlobalShortcuts** portal
//! (`org.freedesktop.portal.GlobalShortcuts`), which is the only mechanism that
//! works under Wayland. The `tauri-plugin-global-shortcut` backend
//! (`global-hotkey`) is X11-only, so we use `ashpd` here and fall back to that
//! plugin's X11 grab when the portal isn't available (e.g. GNOME < 45).
//!
//! Flow: create a portal session, `BindShortcuts` a single "open" shortcut with
//! our accelerator as the *preferred* trigger, then translate `Activated`
//! signals into showing the popover. The session is held alive by the listening
//! task; changing the hotkey aborts it (dropping the session → unbinding) and
//! starts a fresh one.
//!
//! Note the compositor — not us — owns the actual key on the portal path: the
//! preferred trigger is only a hint, and GNOME ignores a re-bind's new trigger
//! for an existing shortcut id. So when the portal is active ([`is_managed`]),
//! the app can't set the key; the panel instead shows the assigned trigger
//! ([`current_trigger_description`]) read-only and offers [`configure`] to open
//! GNOME's own reconfiguration UI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

/// Stable id for our single shortcut, matched in the `Activated` signal.
const SHORTCUT_ID: &str = "open-pitstopx";

/// Emitted to the panel whenever the portal-managed binding changes (initial
/// bind, reconfiguration, or clear) so settings can refresh its display.
pub const SHORTCUT_EVENT: &str = "pitstopx://shortcut";

struct State {
    /// The live portal listener task; aborted + replaced whenever the hotkey changes.
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Asks the live portal task to open GNOME's reconfigure UI. `None` when the
    /// portal isn't active (cleared, or running the X11 fallback).
    configure_tx: Option<mpsc::UnboundedSender<()>>,
}

static STATE: Mutex<State> = Mutex::new(State {
    task: None,
    configure_tx: None,
});

/// Whether the portal owns the binding (vs the X11 fallback / unbound). When
/// true the app can't set the key itself — GNOME does.
static MANAGED: AtomicBool = AtomicBool::new(false);

/// Human-readable trigger GNOME assigned (e.g. `"Ctrl+P"`), for display.
static TRIGGER_DESC: Mutex<Option<String>> = Mutex::new(None);

/// True when the active binding is portal-managed (key owned by the compositor).
pub fn is_managed() -> bool {
    MANAGED.load(Ordering::Relaxed)
}

/// The compositor-assigned trigger for our shortcut, if portal-managed.
pub fn current_trigger_description() -> Option<String> {
    TRIGGER_DESC.lock().unwrap().clone()
}

/// Ask GNOME to open its reconfiguration UI for our shortcut. Requires an active
/// portal session and GlobalShortcuts v2; returns an error otherwise.
pub fn configure() -> Result<(), String> {
    let st = STATE.lock().unwrap();
    let tx = st
        .configure_tx
        .as_ref()
        .ok_or("global shortcut is not system-managed")?;
    tx.send(())
        .map_err(|_| "shortcut service unavailable".to_string())
}

/// (Re)bind the global open-popover hotkey to `accel` (an accelerator like
/// `"CmdOrCtrl+Shift+U"`; empty clears it). Non-blocking: the portal work runs
/// in a background task so the caller (settings/startup) isn't held up by the
/// approval dialog.
pub fn rebind(app: &AppHandle, accel: &str) {
    {
        let mut st = STATE.lock().unwrap();
        if let Some(handle) = st.task.take() {
            handle.abort();
        }
        st.configure_tx = None; // drop the old sender → old task's loop exits
    }
    MANAGED.store(false, Ordering::Relaxed);
    *TRIGGER_DESC.lock().unwrap() = None;

    let trigger = to_portal_trigger(accel);
    if trigger.is_empty() {
        let _ = app.emit(SHORTCUT_EVENT, ()); // cleared
        return; // leave unbound
    }

    let (tx, rx) = mpsc::unbounded_channel();
    let app = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = portal_loop(&app, &trigger, rx).await {
            tracing::warn!(error = %e, "GlobalShortcuts portal unavailable; trying X11 fallback");
            // The plugin's `register` blocks on its backend, which panics
            // ("runtime within a runtime") if called from this tokio worker.
            // Hop to the GTK main thread, which carries no tokio context.
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(e) = fallback_x11(&app2, &trigger) {
                    tracing::warn!(error = %e, "X11 global shortcut fallback failed");
                }
            });
        }
    });

    let mut st = STATE.lock().unwrap();
    st.task = Some(handle);
    st.configure_tx = Some(tx);
}

/// Bind via the portal and service activations, reconfigure requests, and
/// trigger changes until the session ends. Holding `session` for the loop's
/// lifetime keeps the binding registered.
async fn portal_loop(
    app: &AppHandle,
    trigger: &str,
    mut configure_rx: mpsc::UnboundedReceiver<()>,
) -> Result<(), ashpd::Error> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use ashpd::{register_host_app_with_connection, zbus, AppID};
    use futures_util::StreamExt;

    // One explicit session-bus connection shared by both the host-app
    // registration and the GlobalShortcuts session. GNOME ties the registered
    // app id to the *connection*, so the bind must run on the very same one —
    // ashpd's implicit shared connection isn't guaranteed to be that one, which
    // left the bind rejected with "An app id is required".
    let connection = zbus::Connection::session().await?;

    // GNOME's GlobalShortcuts backend rejects a connection with no app id
    // ("NotAllowed: An app id is required"). Non-sandboxed (host) apps must
    // declare theirs via the host Registry portal first; this no-ops under
    // Flatpak. "Already associated" is benign — the connection already carries
    // the id. We reuse the bundle identifier so the two can't drift. The portal
    // resolves it against an installed `<id>.desktop`, which our packaging ships.
    if let Ok(app_id) = AppID::try_from(app.config().identifier.clone()) {
        if let Err(e) = register_host_app_with_connection(connection.clone(), app_id).await {
            tracing::debug!(error = %e, "host app_id registration returned");
        }
    }

    let shortcuts = GlobalShortcuts::with_connection(connection).await?;
    let session = shortcuts.create_session(Default::default()).await?;
    let new = NewShortcut::new(SHORTCUT_ID, "Open PitStopX").preferred_trigger(Some(trigger));
    // `bind_shortcuts` returns a portal Request; `.response()` confirms the
    // binding was created (and surfaces a denied/unsupported error). The reply
    // carries the trigger GNOME actually assigned — which may differ from our
    // preferred one — so we surface that to the panel rather than our guess.
    let bound = shortcuts
        .bind_shortcuts(&session, &[new], None, Default::default())
        .await?
        .response()?;

    update_trigger(app, bound.shortcuts());
    MANAGED.store(true, Ordering::Relaxed);
    tracing::debug!("GlobalShortcuts portal bind succeeded; listening");

    let mut activated = shortcuts.receive_activated().await?;
    let mut changed = shortcuts.receive_shortcuts_changed().await?;
    loop {
        tokio::select! {
            Some(act) = activated.next() => {
                if act.shortcut_id() == SHORTCUT_ID {
                    tracing::debug!("global shortcut activated; toggling popover");
                    crate::toggle_popover_on_main_thread(app);
                }
            }
            Some(ch) = changed.next() => {
                update_trigger(app, ch.shortcuts());
            }
            recv = configure_rx.recv() => match recv {
                // ConfigureShortcuts is v2; older portals reject it — log and
                // let the panel's "use GNOME Settings" hint cover that case.
                Some(()) => if let Err(e) = shortcuts
                    .configure_shortcuts(&session, None, Default::default())
                    .await
                {
                    tracing::warn!(error = %e, "ConfigureShortcuts failed (portal may be < v2)");
                },
                None => break, // sender dropped by a rebind → tear this down
            },
            else => break,
        }
    }

    MANAGED.store(false, Ordering::Relaxed);
    drop(session);
    Ok(())
}

/// Record the compositor-assigned trigger for our shortcut and notify the panel.
fn update_trigger(app: &AppHandle, shortcuts: &[ashpd::desktop::global_shortcuts::Shortcut]) {
    let desc = shortcuts
        .iter()
        .find(|s| s.id() == SHORTCUT_ID)
        .map(|s| s.trigger_description().to_string());
    if let Some(d) = &desc {
        tracing::debug!(trigger = %d, "portal shortcut trigger assigned");
    }
    *TRIGGER_DESC.lock().unwrap() = desc;
    let _ = app.emit(SHORTCUT_EVENT, ());
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
