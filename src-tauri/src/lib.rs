//! Tauri app assembly: plugins, managed state, the programmatic tray (so we own
//! its click + menu events), the popover window toggle, and the background
//! refresh loop. CLI/diagnostic modes are handled in `main.rs` *before* this
//! runs.

pub mod actions;
pub mod app;
pub mod claude_source;
pub mod cli;
pub mod codex_source;
pub mod codex_usage;
pub mod credentials;
pub mod engine;
pub mod format;
pub mod login;
pub mod notify;
pub mod paths;
pub mod prefs;
pub mod profile_store;
pub mod provider;
pub mod secrets;
pub mod source;
pub mod tray;
pub mod ui_events;
pub mod usage_api;

use std::sync::Arc;

use tauri::menu::MenuEvent;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_autostart::MacosLauncher;

use app::{Controller, SharedController};
use profile_store::ProfileStore;

/// Build the `Controller` from the platform abstractions (secret store + every
/// provider's account source).
pub fn build_controller() -> anyhow::Result<SharedController> {
    let secrets = secrets::build()?;
    let sources = source::build_all()?;
    let store = ProfileStore::new(secrets, sources);
    Ok(Arc::new(Controller::new(store)))
}

/// Run the tray application (the normal, non-CLI entrypoint).
pub fn run() {
    let controller = match build_controller() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to initialize platform layer");
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Second launch → focus the existing popover instead of starting anew.
            show_popover(app);
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::AppleScript,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Open the popover on key-down (ignore the release event).
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        show_popover(app);
                    }
                })
                .build(),
        )
        .manage(controller)
        .invoke_handler(tauri::generate_handler![
            actions::switch_to,
            actions::save_current,
            actions::login_new,
            actions::remove_account,
            actions::refresh_now,
            actions::set_indicator_style,
            actions::set_indicator_metric,
            actions::set_launch_at_login,
            actions::quit,
            actions::request_snapshot,
            actions::set_cli_path,
            actions::get_settings,
            actions::set_shortcut,
        ])
        .on_menu_event(handle_menu_event)
        .setup(|app| {
            let handle = app.handle().clone();

            // Hide the popover at startup; it's shown on tray click.
            if let Some(win) = app.get_webview_window("popover") {
                let _ = win.hide();
                attach_blur_autohide(&win);
            }

            // Build the tray programmatically so we own its events.
            let base_icon = tray::render_icon(&tray::TrayVisual {
                utilization: None,
                stale: false,
                prefs: Default::default(),
            })
            .expect("base tray icon renders");

            TrayIconBuilder::with_id("main")
                .icon(base_icon)
                .tooltip("PitStopX")
                // Left-click opens the popover; the context menu is right-click
                // only (default is to show the menu on left-click too, notably on
                // macOS). On Linux the appindicator tray can't deliver a
                // left-click event, so the menu's "Open PitStopX" item is the way
                // in there.
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle());
                    }
                })
                .build(app)?;

            // Initial state load + first refresh + start the cadence.
            let ctrl = app::controller(&handle);
            tauri::async_runtime::spawn(async move {
                app::load_prefs(&ctrl, &handle).await;
                // Register the configured global open-popover hotkey.
                let shortcut = ctrl.state.read().await.shortcut.clone();
                if let Err(e) = set_open_shortcut(&handle, None, &shortcut) {
                    tracing::warn!(error = %e, "failed to register open shortcut");
                }
                app::update_tray(&handle, &ctrl).await;
                app::refresh_all(ctrl.clone(), handle.clone()).await;
                app::spawn_refresh_loop(ctrl, handle);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building PitStopX")
        .run(|_app, event| {
            // Keep running as a background agent even with no windows visible.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // allow normal exit; nothing to hold open
            }
        });
}

/// Dispatch native-menu clicks to the matching action.
fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().0.clone();

    // Showing the popover touches the window (GTK on Linux) and must run on the
    // main thread, where this menu callback already is — handle it here instead
    // of on the async runtime thread.
    if id == tray::ids::SHOW {
        show_popover(app);
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        use tray::ids;
        let result: Result<(), String> = if id == ids::SAVE_CURRENT {
            actions::do_save_current(&app).await
        } else if id == ids::REFRESH_NOW {
            actions::do_refresh_now(&app).await
        } else if id == ids::LAUNCH_AT_LOGIN {
            let now = actions::is_launch_at_login(&app);
            let res = actions::do_set_launch_at_login(&app, !now);
            // Reflect the new checkbox state in the menu.
            let ctrl = app::controller(&app);
            app::rebuild_menu(&app, &ctrl).await;
            res
        } else if id == ids::QUIT {
            actions::do_quit(&app);
            Ok(())
        } else if let Some(pid) = id.strip_prefix(ids::LOGIN_PREFIX) {
            actions::do_login(&app, provider::Provider::from_id(pid)).await
        } else if let Some(rest) = id.strip_prefix(ids::REMOVE_PREFIX) {
            // rest = "<provider_id>:<email>"
            match rest.split_once(':') {
                Some((pid, email)) => {
                    actions::do_remove_account(&app, provider::Provider::from_id(pid), email).await
                }
                None => Ok(()),
            }
        } else if let Some(key) = id.strip_prefix(ids::STYLE_PREFIX) {
            match prefs::IndicatorStyle::from_key(key) {
                Some(s) => actions::do_set_indicator_style(&app, s).await,
                None => Ok(()),
            }
        } else if let Some(key) = id.strip_prefix(ids::METRIC_PREFIX) {
            match prefs::IndicatorMetric::from_key(key) {
                Some(m) => actions::do_set_indicator_metric(&app, m).await,
                None => Ok(()),
            }
        } else {
            Ok(())
        };

        if let Err(e) = result {
            tracing::warn!(menu_id = %id, error = %e, "menu action failed");
        }
    });
}

// ---------------------------------------------------------------------------
// Popover window
// ---------------------------------------------------------------------------

/// (Re)register the global open-popover hotkey: unregisters `old` first, then
/// registers `new`. An empty `new` clears the hotkey. The plugin's handler (set
/// in `run`) shows the popover when it fires. Returns a user-facing error.
pub fn set_open_shortcut(app: &AppHandle, old: Option<&str>, new: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    if let Some(old) = old.map(str::trim).filter(|s| !s.is_empty()) {
        let _ = gs.unregister(old);
    }
    let new = new.trim();
    if new.is_empty() {
        return Ok(());
    }
    gs.register(new)
        .map_err(|e| format!("couldn't set shortcut \"{new}\": {e}"))
}

fn toggle_popover(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("popover") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            show_popover(app);
        }
    }
}

fn show_popover(app: &AppHandle) {
    let Some(win) = app.get_webview_window("popover") else {
        return;
    };
    position_near_tray(app, &win);
    let _ = win.show();
    let _ = win.set_focus();

    // Opening triggers an immediate refresh only when data is stale enough.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctrl = app::controller(&app);
        let should = {
            let s = ctrl.state.read().await;
            match s.last_refresh {
                None => true,
                Some(t) => {
                    let age = chrono::Utc::now().signed_duration_since(t);
                    age.num_seconds() as u64 >= app::MENU_REFRESH_DEBOUNCE.as_secs()
                }
            }
        };
        // Always push the current snapshot so the panel paints immediately.
        app::emit_snapshot(&app, &ctrl).await;
        if should {
            app::refresh_all(ctrl, app).await;
        }
    });
}

/// Anchor the popover near the tray icon, kept fully on-screen.
///
/// The tray is bottom-right on Windows and typically top-right on macOS, so the
/// window is opened on the side of the cursor that has room (above a bottom
/// taskbar, below a top menu bar) and clamped to the monitor it's on. On Wayland
/// (restricted cursor/tray geometry) it falls back to centering on the primary
/// monitor.
fn position_near_tray(app: &AppHandle, win: &WebviewWindow) {
    use tauri::{PhysicalPosition, PhysicalSize, Position};

    let cursor = app.cursor_position().ok();

    // The monitor under the cursor (multi-monitor aware), else the primary one.
    let monitor = cursor
        .as_ref()
        .and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };

    let mpos = monitor.position(); // top-left of the monitor (physical)
    let msize = monitor.size(); // physical pixels
    let scale = monitor.scale_factor();
    let (mon_x, mon_y) = (mpos.x, mpos.y);
    let (mon_w, mon_h) = (msize.width as i32, msize.height as i32);

    // Window size in physical pixels (estimate from the configured logical size
    // if the window hasn't been laid out yet).
    let wsize = win.outer_size().unwrap_or(PhysicalSize {
        width: 0,
        height: 0,
    });
    let (ww, wh) = if wsize.width > 0 && wsize.height > 0 {
        (wsize.width as i32, wsize.height as i32)
    } else {
        ((360.0 * scale) as i32, (520.0 * scale) as i32)
    };

    let margin = (8.0 * scale) as i32;

    // No cursor geometry (typically Wayland, where the global cursor/tray
    // position isn't queryable). We can't know the tray icon's location, so
    // pick a sensible anchor instead of dead-center.
    let Some(cursor) = cursor else {
        #[cfg(target_os = "linux")]
        {
            // The appindicator lives in the top bar — top-right on GNOME/Ubuntu.
            // Anchor there, clearing a typical top panel. (Wayland compositors
            // may still override a client's requested position.)
            let top_bar = (40.0 * scale) as i32;
            let x = (mon_x + mon_w - ww - margin).max(mon_x + margin);
            let y = mon_y + top_bar;
            let _ = win.set_position(Position::Physical(PhysicalPosition { x, y }));
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let x = mon_x + (mon_w - ww) / 2;
            let y = mon_y + (mon_h - wh) / 2;
            let _ = win.set_position(Position::Physical(PhysicalPosition { x, y }));
            return;
        }
    };

    let (cx, cy) = (cursor.x as i32, cursor.y as i32);

    // Horizontal: center on the cursor, clamped within the monitor.
    let x = (cx - ww / 2).clamp(mon_x + margin, mon_x + mon_w - ww - margin);

    // Vertical: prefer opening above the cursor (bottom tray); if it would clip
    // the top of the monitor, open below instead. Always clamp.
    let above = cy - wh - margin;
    let below = cy + margin;
    let y_anchor = if above >= mon_y + margin {
        above
    } else {
        below
    };
    let y = y_anchor.clamp(mon_y + margin, mon_y + mon_h - wh - margin);

    let _ = win.set_position(Position::Physical(PhysicalPosition { x, y }));
}

/// Auto-hide the popover when it loses focus (blur).
fn attach_blur_autohide(win: &WebviewWindow) {
    let handle = win.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(false) = event {
            let _ = handle.hide();
        }
    });
}
