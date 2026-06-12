//! `ClaudeSource` — locate and read/write Claude Code's *own* live login. The
//! credential blob's location differs per OS; identity (`~/.claude.json`) does
//! not. The backend is detected at runtime so the rest of the app is OS-agnostic.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;

/// Keychain service name Claude Code uses on macOS.
#[cfg(target_os = "macos")]
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[async_trait]
pub trait ClaudeSource: Send + Sync {
    /// Read the live credential blob (`claudeAiOauth` + other sections).
    async fn read_live_blob(&self) -> Result<Option<Vec<u8>>>;
    /// Write the live credential blob (used when switching accounts).
    async fn write_live_blob(&self, blob: &[u8]) -> Result<()>;
    /// `~/.claude.json` — identity (`oauthAccount`), same on every OS.
    fn identity_path(&self) -> PathBuf;
    /// Human-readable description of the live blob location (for `--print-paths`).
    fn describe(&self) -> String;
}

/// Build the source appropriate to this OS, auto-detecting the backend.
pub fn build() -> Result<Box<dyn ClaudeSource>> {
    let home = crate::paths::home_dir()?;
    let identity = home.join(".claude.json");

    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(KeychainSource { identity }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let file = home.join(".claude").join(".credentials.json");
        Ok(Box::new(FileSource { file, identity }))
    }
}

/// macOS: live blob in the login keychain, service `Claude Code-credentials`.
/// Updated in-place (`-U`) to preserve the item and its ACL.
#[cfg(target_os = "macos")]
struct KeychainSource {
    identity: PathBuf,
}

#[cfg(target_os = "macos")]
#[async_trait]
impl ClaudeSource for KeychainSource {
    async fn read_live_blob(&self) -> Result<Option<Vec<u8>>> {
        use tokio::process::Command;
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", CLAUDE_KEYCHAIN_SERVICE, "-w"])
            .output()
            .await
            .context("reading Claude Code keychain item")?;
        if !out.status.success() {
            return Ok(None);
        }
        let mut s = String::from_utf8(out.stdout)?;
        if s.ends_with('\n') {
            s.pop();
        }
        Ok(Some(s.into_bytes()))
    }

    async fn write_live_blob(&self, blob: &[u8]) -> Result<()> {
        use tokio::process::Command;
        let value = std::str::from_utf8(blob).context("blob not utf-8")?;
        // In-place update preserves the existing item + its ACL grants.
        let status = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-s",
                CLAUDE_KEYCHAIN_SERVICE,
                "-a",
                CLAUDE_KEYCHAIN_SERVICE,
                "-w",
                value,
                "-U",
            ])
            .status()
            .await
            .context("writing Claude Code keychain item")?;
        anyhow::ensure!(status.success(), "security add (-U) failed");
        Ok(())
    }

    fn identity_path(&self) -> PathBuf {
        self.identity.clone()
    }

    fn describe(&self) -> String {
        format!("macOS Keychain service '{CLAUDE_KEYCHAIN_SERVICE}'")
    }
}

/// Windows/Linux: live blob in `~/.claude/.credentials.json`. Atomic writes,
/// preserving the file's user-only permissions.
#[cfg(not(target_os = "macos"))]
struct FileSource {
    file: PathBuf,
    identity: PathBuf,
}

#[cfg(not(target_os = "macos"))]
#[async_trait]
impl ClaudeSource for FileSource {
    async fn read_live_blob(&self) -> Result<Option<Vec<u8>>> {
        if !self.file.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.file)
            .with_context(|| format!("reading {}", self.file.display()))?;
        Ok(Some(bytes))
    }

    async fn write_live_blob(&self, blob: &[u8]) -> Result<()> {
        atomic_write(&self.file, blob)
    }

    fn identity_path(&self) -> PathBuf {
        self.identity.clone()
    }

    fn describe(&self) -> String {
        format!("file {}", self.file.display())
    }
}

/// Atomic file write: temp file in the same directory + rename, preserving the
/// existing file's permissions when present (user-only ACL on Unix).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(dir).ok();

    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("pitstopx")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing temp {}", tmp.display()))?;

    // Preserve permissions from the original file when it exists; otherwise lock
    // the new file down to the user only on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o600);
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode & 0o777));
    }

    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
