//! `SecretStore` — where PitStopX keeps its *own* copies of each saved
//! account's full credential blob. One trait, three per-OS backends, selected
//! at runtime. The stored value is the credential blob verbatim; `account` is
//! the profile email.

use anyhow::Result;
use async_trait::async_trait;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod encrypted_file;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Service / collection name PitStopX uses across all backends.
pub const SERVICE: &str = "PitStopX-profile";

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>>;
    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()>;
    async fn delete(&self, account: &str) -> Result<()>;
}

/// Build the secret store appropriate to the host OS.
pub fn build() -> Result<Box<dyn SecretStore>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacSecurityStore::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WinCredStore::new()?))
    }
    #[cfg(target_os = "linux")]
    {
        linux::build()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("unsupported platform for SecretStore")
    }
}
