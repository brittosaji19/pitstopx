//! Linux backend: prefer the Secret Service (libsecret) via `keyring`,
//! collection `PitStopX`. When no secret service is reachable (no GNOME
//! Keyring / KWallet running) fall back to the shared age-encrypted file store
//! — a deliberately weaker, clearly-logged degraded mode.

use anyhow::{Context, Result};
use async_trait::async_trait;
use keyring::Entry;

use super::encrypted_file::EncryptedFileStore;
use super::{SecretStore, SERVICE};

/// Decide which backend to use by probing the Secret Service once.
pub fn build() -> Result<Box<dyn SecretStore>> {
    let probe = Entry::new(SERVICE, "__pitstopx_probe__").and_then(|e| match e.get_secret() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    });

    match probe {
        Ok(()) => {
            tracing::info!("Linux SecretStore: using Secret Service (libsecret)");
            Ok(Box::new(SecretServiceStore))
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Linux SecretStore: no Secret Service available — \
                 falling back to encrypted file (DEGRADED mode)"
            );
            Ok(Box::new(EncryptedFileStore::new("secrets")?))
        }
    }
}

/// Secret Service via keyring.
struct SecretServiceStore;

impl SecretServiceStore {
    fn entry(account: &str) -> Result<Entry> {
        Entry::new(SERVICE, account).context("opening Secret Service entry")
    }
}

#[async_trait]
impl SecretStore for SecretServiceStore {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>> {
        let account = account.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            match Self::entry(&account)?.get_secret() {
                Ok(b) => Ok(Some(b)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await?
    }

    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()> {
        let account = account.to_string();
        let blob = blob.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            Self::entry(&account)?
                .set_secret(&blob)
                .context("writing Secret Service entry")
        })
        .await?
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let account = account.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            match Self::entry(&account)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
        .await?
    }
}
