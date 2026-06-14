//! Model-provider seam.
//!
//! PitStopX currently tracks Claude Code (**Anthropic**) accounts, but accounts
//! are tagged with a `Provider` so other model providers can be added later
//! without touching the storage/UI plumbing: add a variant here, give it an
//! `id`, `display_name`, and accent color, and the rest (persistence, the
//! account-row DTO, the popover badge) flows through automatically.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Claude / Claude Code.
    #[default]
    Anthropic,
    /// OpenAI Codex (ChatGPT-backed login).
    #[serde(rename = "openai")]
    OpenAI,
}

impl Provider {
    /// Every known provider (for iterating sources, menus, etc.).
    pub const ALL: [Provider; 2] = [Provider::Anthropic, Provider::OpenAI];

    /// Stable machine id (persisted in `profiles.json`, used as a CSS hook).
    pub fn id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAI => "openai",
        }
    }

    /// Human-facing product name shown on each account row.
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Anthropic => "Claude",
            Provider::OpenAI => "Codex",
        }
    }

    /// Brand accent color (hex), so each provider's badge is distinguishable.
    pub fn accent(self) -> &'static str {
        match self {
            Provider::Anthropic => "#D97757",
            Provider::OpenAI => "#10A37F",
        }
    }

    /// The CLI command that starts this provider's interactive login flow,
    /// as `(program, args)`. Run in a terminal so the user can complete the
    /// browser OAuth (and paste a code if prompted).
    pub fn login_command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Provider::Anthropic => ("claude", &["/login"]),
            Provider::OpenAI => ("codex", &["login"]),
        }
    }

    /// Resolve from a persisted id; unknown ids fall back to the default.
    pub fn from_id(s: &str) -> Self {
        match s {
            "anthropic" => Provider::Anthropic,
            "openai" => Provider::OpenAI,
            _ => Provider::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        assert_eq!(Provider::Anthropic.id(), "anthropic");
        assert_eq!(Provider::Anthropic.display_name(), "Claude");
        assert_eq!(Provider::from_id("anthropic"), Provider::Anthropic);
        assert_eq!(Provider::from_id("nope"), Provider::default());
    }

    #[test]
    fn serde_uses_snake_case_id() {
        let json = serde_json::to_string(&Provider::Anthropic).unwrap();
        assert_eq!(json, "\"anthropic\"");
    }
}
