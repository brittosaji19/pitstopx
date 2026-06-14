//! Windows backend: Credential Manager via the `keyring` crate (`wincred`).
//! Entries are generic credentials `PitStopX-profile:<account>`, DPAPI-encrypted
//! at rest under the user account; no interactive prompt, no argv exposure.
//!
//! Credential Manager caps a generic credential's blob at
//! `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes). Some provider credentials are
//! larger than that (e.g. Codex's `auth.json` is several KB of JWTs), so blobs
//! over the limit transparently fall back to the shared age-encrypted file
//! store. Reads check both locations.

use anyhow::{Context, Result};
use async_trait::async_trait;
use keyring::Entry;

use super::encrypted_file::EncryptedFileStore;
use super::{SecretStore, SERVICE};

/// Stay safely under `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes).
const CRED_BLOB_LIMIT: usize = 2400;

pub struct WinCredStore {
    /// Fallback for oversized blobs.
    file: EncryptedFileStore,
}

impl WinCredStore {
    pub fn new() -> Result<Self> {
        Ok(Self {
            file: EncryptedFileStore::new("secrets")?,
        })
    }

    fn entry(account: &str) -> Result<Entry> {
        Entry::new(SERVICE, account).context("opening Credential Manager entry")
    }

    async fn cred_read(account: &str) -> Result<Option<Vec<u8>>> {
        let account = account.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            match Self::entry(&account)?.get_secret() {
                Ok(bytes) => Ok(Some(bytes)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await?
    }

    async fn cred_delete(account: &str) -> Result<()> {
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

#[async_trait]
impl SecretStore for WinCredStore {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = Self::cred_read(account).await? {
            return Ok(Some(bytes));
        }
        // Fall back to the encrypted file (oversized blobs live there).
        self.file.read(account).await
    }

    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()> {
        if blob.len() <= CRED_BLOB_LIMIT {
            let account_s = account.to_string();
            let blob_v = blob.to_vec();
            tokio::task::spawn_blocking(move || -> Result<()> {
                Self::entry(&account_s)?
                    .set_secret(&blob_v)
                    .context("writing credential")
            })
            .await??;
            // Remove any stale oversized-file copy from a previous larger blob.
            let _ = self.file.delete(account).await;
            Ok(())
        } else {
            // Too big for Credential Manager → encrypted file; drop any small copy.
            self.file.upsert(account, blob).await?;
            let _ = Self::cred_delete(account).await;
            Ok(())
        }
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let _ = Self::cred_delete(account).await;
        let _ = self.file.delete(account).await;
        Ok(())
    }
}
