//! Launch a provider's interactive login in a fresh terminal window.
//!
//! Both Claude Code and Codex log in via a browser OAuth flow that may print a
//! URL or ask the user to paste a code, so we open a visible terminal running
//! the provider's login command rather than capturing it headlessly. PitStopX
//! saves the current account first (see `actions::do_login`) and picks up the
//! newly-logged-in account on the next refresh.
//!
//! The executables are often **not on `PATH`** (the Codex desktop app ships
//! `codex.exe` under `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\`; Claude Code is a
//! WinGet shim), so we resolve the real path before launching.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Result};

use crate::provider::Provider;

/// Open a terminal running the provider's login command. `override_path` is the
/// user-set CLI path (settings page); empty/missing/invalid falls back to
/// auto-detection.
pub fn launch(provider: Provider, override_path: Option<&str>) -> Result<()> {
    let program =
        resolve_program(provider, override_path).ok_or_else(|| not_installed_error(provider))?;
    let (_, args) = provider.login_command();
    spawn_terminal(&program, args)
}

/// Whether the provider's login CLI can be located (override path or auto-detect).
pub fn is_installed(provider: Provider, override_path: Option<&str>) -> bool {
    resolve_program(provider, override_path).is_some()
}

/// `Err` (with a user-facing message) if the provider's login CLI isn't found.
pub fn ensure_installed(provider: Provider, override_path: Option<&str>) -> Result<()> {
    if is_installed(provider, override_path) {
        Ok(())
    } else {
        Err(not_installed_error(provider))
    }
}

fn not_installed_error(provider: Provider) -> anyhow::Error {
    let (name, _) = provider.login_command();
    anyhow!(
        "{} CLI (`{name}`) isn't installed — install it and try again.",
        provider.display_name()
    )
}

/// Resolve the provider's CLI executable: an explicit user override (if it
/// points at an existing file) wins; otherwise `PATH`, then known install
/// locations.
pub fn resolve_program(provider: Provider, override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(raw) = override_path {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let pb = PathBuf::from(trimmed);
            if pb.is_file() {
                return Some(pb);
            }
            // Invalid override → fall through to auto-detection rather than fail.
        }
    }
    let (name, _) = provider.login_command();
    if let Some(p) = which_on_path(name) {
        return Some(p);
    }
    match provider {
        Provider::OpenAI => find_codex(),
        Provider::Anthropic => find_claude(),
    }
}

/// Minimal `which`: scan `PATH` (with Windows executable extensions).
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe` (newest hash dir).
#[cfg(windows)]
fn find_codex() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?
        .join("OpenAI")
        .join("Codex")
        .join("bin");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&base).ok()?.flatten() {
        let exe = entry.path().join("codex.exe");
        if exe.is_file() {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                newest = Some((mtime, exe));
            }
        }
    }
    newest.map(|(_, p)| p)
}

/// macOS: a Finder/Dock-launched `.app` inherits a minimal `PATH`, so probe the
/// common install locations (Homebrew, npm-global, native installers) as well.
/// Exotic prefixes (nvm, custom) are covered by the manual path override.
#[cfg(target_os = "macos")]
fn find_in_known_dirs(name: &str) -> Option<PathBuf> {
    let mut search = vec![
        PathBuf::from("/opt/homebrew/bin"), // Apple Silicon Homebrew
        PathBuf::from("/usr/local/bin"),    // Intel Homebrew / npm default
    ];
    if let Ok(home) = crate::paths::home_dir() {
        search.push(home.join(".local/bin")); // native installers
        search.push(home.join(".npm-global/bin"));
        search.push(home.join(".bun/bin"));
        search.push(home.join(".claude/local")); // Claude Code native install
    }
    search
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[cfg(target_os = "macos")]
fn find_codex() -> Option<PathBuf> {
    find_in_known_dirs("codex")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn find_codex() -> Option<PathBuf> {
    None // Linux: PATH-only
}

/// Claude Code WinGet shim: `%LOCALAPPDATA%\Microsoft\WinGet\Packages\Anthropic.ClaudeCode_*\claude.*`.
#[cfg(windows)]
fn find_claude() -> Option<PathBuf> {
    let pkgs = dirs::data_local_dir()?
        .join("Microsoft")
        .join("WinGet")
        .join("Packages");
    for entry in std::fs::read_dir(&pkgs).ok()?.flatten() {
        let dir_name = entry.file_name().to_string_lossy().to_lowercase();
        if dir_name.contains("anthropic.claudecode") {
            for exe in ["claude.exe", "claude.cmd", "claude.bat", "claude"] {
                let cand = entry.path().join(exe);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn find_claude() -> Option<PathBuf> {
    find_in_known_dirs("claude")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn find_claude() -> Option<PathBuf> {
    None // Linux: PATH-only
}

/// Open a new terminal window running `program args…`, kept open afterward so
/// the user can read prompts / paste codes.
fn spawn_terminal(program: &std::path::Path, args: &[&str]) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        // cmd /K <program> <args…>  in its own console window.
        let mut cmd = Command::new("cmd");
        cmd.creation_flags(CREATE_NEW_CONSOLE)
            .arg("/K")
            .arg(program);
        cmd.args(args);
        cmd.spawn()
            .map_err(|e| anyhow!("failed to open terminal: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let cmd_line = shell_line(program, args);
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            cmd_line.replace('\\', "\\\\").replace('"', "\\\"")
        );
        Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .map_err(|e| anyhow!("failed to open Terminal: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let cmd_line = shell_line(program, args);
        let inner = format!("{cmd_line}; echo; echo '[press Enter to close]'; read _");
        for term in [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ] {
            if Command::new(term)
                .args(["-e", "sh", "-c", &inner])
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(anyhow!("no terminal emulator found"))
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_line(program: &std::path::Path, args: &[&str]) -> String {
    let mut parts = vec![format!("'{}'", program.display())];
    parts.extend(args.iter().map(|s| s.to_string()));
    parts.join(" ")
}
