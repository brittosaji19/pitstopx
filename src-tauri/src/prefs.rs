//! Persisted indicator preferences (backed by `tauri-plugin-store`). Mirrors
//! PitStop's `UserDefaults`-backed settings, adapted to icon rendering.

use serde::{Deserialize, Serialize};

use crate::provider::Provider;

/// Store file name (under the app config dir).
pub const STORE_FILE: &str = "prefs.json";
pub const KEY_STYLE: &str = "indicatorStyle";
pub const KEY_METRIC: &str = "indicatorMetric";
pub const KEY_CLAUDE_BIN: &str = "claudeBinPath";
pub const KEY_CODEX_BIN: &str = "codexBinPath";
pub const KEY_SHORTCUT: &str = "openShortcut";
/// Account pinned to the tray icon (its `Profile::key()`); empty = auto.
pub const KEY_TRAY_ACCOUNT: &str = "trayAccount";

/// Global hotkey to open the popover, used until the user changes it. `CmdOrCtrl`
/// maps to ⌘ on macOS and Ctrl elsewhere.
pub const DEFAULT_SHORTCUT: &str = "CmdOrCtrl+Shift+U";

/// User-set overrides for the provider CLI executables. `None`/empty means
/// "auto-detect" (`PATH` + known install locations). See [`crate::login`].
#[derive(Debug, Clone, Default)]
pub struct CliPaths {
    pub claude: Option<String>,
    pub codex: Option<String>,
}

impl CliPaths {
    /// The configured override path for `provider`, if any.
    pub fn get(&self, provider: Provider) -> Option<&str> {
        match provider {
            Provider::Anthropic => self.claude.as_deref(),
            Provider::OpenAI => self.codex.as_deref(),
        }
    }

    /// The store key holding a provider's override path.
    pub fn key_for(provider: Provider) -> &'static str {
        match provider {
            Provider::Anthropic => KEY_CLAUDE_BIN,
            Provider::OpenAI => KEY_CODEX_BIN,
        }
    }
}

/// How the tray icon presents the figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndicatorStyle {
    /// Gauge glyph + `NN%` (default).
    #[default]
    IconAndPercent,
    /// Glyph only, no number.
    IconOnly,
    /// Number-only glyph, no gauge motif.
    PercentOnly,
}

impl IndicatorStyle {
    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "iconAndPercent" => Some(Self::IconAndPercent),
            "iconOnly" => Some(Self::IconOnly),
            "percentOnly" => Some(Self::PercentOnly),
            _ => None,
        }
    }
    pub fn as_key(self) -> &'static str {
        match self {
            Self::IconAndPercent => "iconAndPercent",
            Self::IconOnly => "iconOnly",
            Self::PercentOnly => "percentOnly",
        }
    }
    pub fn shows_percent(self) -> bool {
        matches!(self, Self::IconAndPercent | Self::PercentOnly)
    }
    pub fn shows_gauge(self) -> bool {
        matches!(self, Self::IconAndPercent | Self::IconOnly)
    }
}

/// Which usage window the tray figure reflects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndicatorMetric {
    /// `max(5-hour, weekly)` (default, "Highest").
    #[default]
    Binding,
    FiveHour,
    Weekly,
}

impl IndicatorMetric {
    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "binding" => Some(Self::Binding),
            "fiveHour" => Some(Self::FiveHour),
            "weekly" => Some(Self::Weekly),
            _ => None,
        }
    }
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::FiveHour => "fiveHour",
            Self::Weekly => "weekly",
        }
    }
}

/// The resolved indicator settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndicatorPrefs {
    pub style: IndicatorStyle,
    pub metric: IndicatorMetric,
}
