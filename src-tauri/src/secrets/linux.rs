//! Linux backend: prefer the Secret Service (libsecret) via `keyring`,
//! collection `PitStopX`. When no secret service is reachable (no GNOME
//! Keyring / KWallet running) fall back to an age-encrypted file under the data
//! dir, with the key derived from a machine-bound secret. The fallback is a
//! deliberately weaker, clearly-logged degraded mode.

use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use keyring::Entry;

use super::{SecretStore, SERVICE};

/// Decide which backend to use by probing the Secret Service once.
pub fn build() -> Result<Box<dyn SecretStore>> {
    // A trivial round-trip probe: if we can construct + query an entry without
    // a D-Bus error, the Secret Service is usable.
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
            Ok(Box::new(EncryptedFileStore::new()?))
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

/// age-encrypted file fallback. One file per account under
/// `<data>/pitstopx/secrets/`, encrypted with a passphrase derived from a
/// machine-bound id.
struct EncryptedFileStore {
    dir: PathBuf,
    passphrase: String,
}

impl EncryptedFileStore {
    fn new() -> Result<Self> {
        let dir = crate::paths::data_dir()?.join("secrets");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            passphrase: machine_passphrase(),
            dir,
        })
    }

    fn file(&self, account: &str) -> PathBuf {
        // Hash the email into a filename-safe, non-reversible-on-disk handle.
        let digest = simple_hash(account);
        self.dir.join(format!("{digest}.age"))
    }
}

#[async_trait]
impl SecretStore for EncryptedFileStore {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>> {
        let path = self.file(account);
        if !path.exists() {
            return Ok(None);
        }
        let ciphertext = std::fs::read(&path)?;
        let plain = age::decrypt(
            &age::scrypt::Identity::new(self.passphrase.clone().into()),
            &ciphertext,
        )
        .context("decrypting fallback secret")?;
        Ok(Some(plain))
    }

    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()> {
        let ciphertext = age::encrypt(
            &age::scrypt::Recipient::new(self.passphrase.clone().into()),
            blob,
        )
        .context("encrypting fallback secret")?;
        crate::claude_source::atomic_write(&self.file(account), &ciphertext)
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let path = self.file(account);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// Derive a stable, machine-bound passphrase. Uses the Linux machine-id when
/// available; otherwise a per-install random key persisted to the data dir.
fn machine_passphrase() -> String {
    if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
        let id = id.trim();
        if !id.is_empty() {
            return format!("pitstopx:{id}");
        }
    }
    // Fallback: a random key file we create once.
    if let Ok(dir) = crate::paths::data_dir() {
        let key_path = dir.join(".fallback-key");
        if let Ok(existing) = std::fs::read_to_string(&key_path) {
            if !existing.trim().is_empty() {
                return existing.trim().to_string();
            }
        }
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let key = format!("pitstopx:{}", simple_hash(&hex(&bytes)));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&key_path, &key);
        return key;
    }
    "pitstopx:insecure-default".to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Tiny FNV-1a hash, used only for filename derivation (not for security).
fn simple_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
