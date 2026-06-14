//! Persisted indicator preferences (backed by `tauri-plugin-store`). Mirrors
//! PitStop's `UserDefaults`-backed settings, adapted to icon rendering.

use serde::{Deserialize, Serialize};

/// Store file name (under the app config dir).
pub const STORE_FILE: &str = "prefs.json";
pub const KEY_STYLE: &str = "indicatorStyle";
pub const KEY_METRIC: &str = "indicatorMetric";

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
