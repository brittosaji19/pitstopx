//! Anthropic (Claude Code) account source. The live credential blob's location
//! differs per OS (macOS Keychain vs `~/.claude/.credentials.json`); identity
//! (`~/.claude.json` `oauthAccount`) is the same everywhere. Implements the
//! provider-agnostic [`AccountSource`] trait.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::credentials::{ClaudeConfig, CredentialBlob};
use crate::provider::Provider;
use crate::source::{AccountSource, Identity, LiveAccount};

/// Keychain service name Claude Code uses on macOS.
#[cfg(target_os = "macos")]
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Build the Claude source for this OS.
pub fn build() -> Result<Box<dyn AccountSource>> {
    let home = crate::paths::home_dir()?;
    let identity = home.join(".claude.json");

    #[cfg(target_os = "macos")]
    let blob = BlobLocation::Keychain;
    #[cfg(not(target_os = "macos"))]
    let blob = BlobLocation::File(home.join(".claude").join(".credentials.json"));

    Ok(Box::new(ClaudeSource { identity, blob }))
}

/// Where Claude Code keeps the live credential blob on this OS.
enum BlobLocation {
    #[cfg(target_os = "macos")]
    Keychain,
    #[cfg(not(target_os = "macos"))]
    File(PathBuf),
}

struct ClaudeSource {
    identity: PathBuf,
    blob: BlobLocation,
}

impl ClaudeSource {
    async fn read_blob(&self) -> Result<Option<Vec<u8>>> {
        match &self.blob {
            #[cfg(target_os = "macos")]
            BlobLocation::Keychain => {
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
            #[cfg(not(target_os = "macos"))]
            BlobLocation::File(path) => {
                if !path.exists() {
                    return Ok(None);
                }
                let bytes =
                    std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
                Ok(Some(bytes))
            }
        }
    }

    async fn write_blob(&self, blob: &[u8]) -> Result<()> {
        match &self.blob {
            #[cfg(target_os = "macos")]
            BlobLocation::Keychain => {
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
            #[cfg(not(target_os = "macos"))]
            BlobLocation::File(path) => atomic_write(path, blob),
        }
    }
}

#[async_trait]
impl AccountSource for ClaudeSource {
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn read_live(&self) -> Result<Option<LiveAccount>> {
        let Some(blob) = self.read_blob().await? else {
            return Ok(None);
        };
        let config = ClaudeConfig::at(&self.identity);
        let Some(oauth_account) = config.oauth_account()? else {
            return Ok(None);
        };
        let Some(email) = oauth_account
            .get("emailAddress")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(None);
        };

        // subscription / tier live inside the blob's claudeAiOauth section.
        let creds = CredentialBlob::parse(&blob)
            .ok()
            .and_then(|b| b.credentials().ok());

        let identity = Identity {
            email,
            subscription_type: creds.as_ref().and_then(|c| c.subscription_type.clone()),
            rate_limit_tier: creds.as_ref().and_then(|c| c.rate_limit_tier.clone()),
            oauth_account,
        };
        Ok(Some(LiveAccount { blob, identity }))
    }

    async fn write_live(&self, blob: &[u8], identity: &Value) -> Result<()> {
        // 1) Write the live credential blob.
        self.write_blob(blob).await?;
        // 2) Restore identity (only oauthAccount changes; atomic whole-file write).
        ClaudeConfig::at(&self.identity).set_oauth_account(identity)?;
        Ok(())
    }

    async fn clear_live(&self) -> Result<()> {
        // 1) Delete the credential blob (keychain item / file).
        match &self.blob {
            #[cfg(target_os = "macos")]
            BlobLocation::Keychain => {
                use tokio::process::Command;
                // Non-zero exit just means "no such item" → already cleared.
                let _ = Command::new("/usr/bin/security")
                    .args(["delete-generic-password", "-s", CLAUDE_KEYCHAIN_SERVICE])
                    .status()
                    .await
                    .context("deleting Claude Code keychain item")?;
            }
            #[cfg(not(target_os = "macos"))]
            BlobLocation::File(path) => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
            },
        }
        // 2) Clear the stored identity so `read_live` reports logged-out.
        ClaudeConfig::at(&self.identity).clear_oauth_account()?;
        Ok(())
    }

    fn describe(&self) -> String {
        match &self.blob {
            #[cfg(target_os = "macos")]
            BlobLocation::Keychain => {
                format!("macOS Keychain service '{CLAUDE_KEYCHAIN_SERVICE}'")
            }
            #[cfg(not(target_os = "macos"))]
            BlobLocation::File(path) => format!("file {}", path.display()),
        }
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
