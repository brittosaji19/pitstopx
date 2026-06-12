//! Per-OS config / data / log directory resolution. Uses `dirs` so the same
//! code yields `~/.config/pitstopx` on macOS/Linux and `%APPDATA%\pitstopx` on
//! Windows.

use std::path::PathBuf;

use anyhow::{anyhow, Result};

const APP_DIR: &str = "pitstopx";

/// `~/.config/pitstopx` (macOS/Linux) or `%APPDATA%\pitstopx` (Windows).
pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("no config dir for this OS"))?;
    let dir = base.join(APP_DIR);
    std::fs::create_dir_all(&dir).ok();
    Ok(dir)
}

/// Per-OS data dir (used for the Linux encrypted-file fallback, etc.).
pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir().ok_or_else(|| anyhow!("no data dir for this OS"))?;
    let dir = base.join(APP_DIR);
    std::fs::create_dir_all(&dir).ok();
    Ok(dir)
}

/// Per-OS log dir.
pub fn log_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| anyhow!("no cache dir for this OS"))?;
    let dir = base.join(APP_DIR).join("logs");
    std::fs::create_dir_all(&dir).ok();
    Ok(dir)
}

/// The user's home directory (where `~/.claude.json` and `~/.claude/` live).
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("no home dir for this OS"))
}

/// `<config>/pitstopx/profiles.json`.
pub fn profiles_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("profiles.json"))
}
