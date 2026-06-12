//! Windows backend: Credential Manager via the `keyring` crate (`wincred`).
//! Entries are generic credentials `PitStopX-profile:<email>`, DPAPI-encrypted
//! at rest under the user account; no interactive prompt, no argv exposure.

use anyhow::{Context, Result};
use async_trait::async_trait;
use keyring::Entry;

use super::{SecretStore, SERVICE};

pub struct WinCredStore;

impl WinCredStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(account: &str) -> Result<Entry> {
        Entry::new(SERVICE, account).context("opening Credential Manager entry")
    }
}

#[async_trait]
impl SecretStore for WinCredStore {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>> {
        let account = account.to_string();
        // keyring is blocking; run it off the async reactor.
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            let entry = Self::entry(&account)?;
            match entry.get_secret() {
                Ok(bytes) => Ok(Some(bytes)),
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
            let entry = Self::entry(&account)?;
            entry.set_secret(&blob).context("writing credential")
        })
        .await?
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let account = account.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let entry = Self::entry(&account)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
        .await?
    }
}
