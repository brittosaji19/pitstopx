//! macOS backend: route every keychain access through the Apple-signed
//! `/usr/bin/security` binary. Because the *requester* is a stable Apple
//! binary, the one-time "Always Allow" grant survives PitStopX rebuilds and is
//! shared with Claude Code. Writes are staged delete+add so a failed write
//! never leaves zero copies.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::process::Command;

use super::{SecretStore, SERVICE};

pub struct MacSecurityStore;

impl MacSecurityStore {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SecretStore for MacSecurityStore {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>> {
        // `-w` prints the raw password to stdout.
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
            .output()
            .await
            .context("spawning /usr/bin/security find")?;

        if !out.status.success() {
            // Non-zero exit = item not found (or denied); treat as absent.
            return Ok(None);
        }
        let mut s = String::from_utf8(out.stdout).context("non-utf8 keychain value")?;
        // `security -w` appends a trailing newline.
        if s.ends_with('\n') {
            s.pop();
        }
        Ok(Some(s.into_bytes()))
    }

    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()> {
        let value = std::str::from_utf8(blob).context("credential blob is not utf-8")?;
        // Staged write: delete the old item (ignore "not found"), then add.
        let _ = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE, "-a", account])
            .output()
            .await;

        let status = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-s",
                SERVICE,
                "-a",
                account,
                "-w",
                value,
                "-U",
            ])
            .status()
            .await
            .context("spawning /usr/bin/security add")?;

        if status.success() {
            Ok(())
        } else {
            Err(anyhow!(
                "security add-generic-password failed for {account}"
            ))
        }
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE, "-a", account])
            .output()
            .await;
        Ok(())
    }
}
