//! Notification wrapper over `tauri-plugin-notification`. Requests OS
//! permission lazily on first send, queues notifications issued before the
//! grant resolves, and drops them if denied — the same state-machine behavior
//! as PitStop's `Notifier`.

use std::sync::Mutex;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Permission {
    Unknown,
    Granted,
    Denied,
}

/// Holds permission state and any notifications queued before the grant
/// resolved.
pub struct Notifier {
    state: Mutex<Inner>,
}

struct Inner {
    permission: Permission,
    queued: Vec<(String, String)>,
}

impl Notifier {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(Inner {
                permission: Permission::Unknown,
                queued: Vec::new(),
            }),
        }
    }

    /// Send a notification, requesting permission on first use. Safe to call
    /// from any thread; never panics on a permission failure.
    pub fn notify(&self, app: &AppHandle, title: &str, body: &str) {
        let permission = { self.state.lock().unwrap().permission };

        match permission {
            Permission::Granted => self.show(app, title, body),
            Permission::Denied => { /* dropped */ }
            Permission::Unknown => {
                // Queue, then resolve the permission.
                {
                    let mut inner = self.state.lock().unwrap();
                    inner.queued.push((title.to_string(), body.to_string()));
                }
                self.resolve_permission(app);
            }
        }
    }

    fn resolve_permission(&self, app: &AppHandle) {
        let granted = match app.notification().permission_state() {
            Ok(tauri_plugin_notification::PermissionState::Granted) => true,
            Ok(tauri_plugin_notification::PermissionState::Denied) => false,
            _ => matches!(
                app.notification().request_permission(),
                Ok(tauri_plugin_notification::PermissionState::Granted)
            ),
        };

        let drained: Vec<(String, String)> = {
            let mut inner = self.state.lock().unwrap();
            inner.permission = if granted {
                Permission::Granted
            } else {
                Permission::Denied
            };
            std::mem::take(&mut inner.queued)
        };

        if granted {
            for (title, body) in drained {
                self.show(app, &title, &body);
            }
        }
    }

    fn show(&self, app: &AppHandle, title: &str, body: &str) {
        if let Err(e) = app.notification().builder().title(title).body(body).show() {
            tracing::warn!(error = %e, "failed to post notification");
        }
    }
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new()
    }
}
