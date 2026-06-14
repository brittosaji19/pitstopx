//! age-encrypted file secret store, shared by the Linux fallback (no Secret
//! Service) and the Windows fallback (blobs larger than the Credential Manager
//! limit). One file per account under `<data>/<subdir>/`, encrypted with a
//! passphrase derived from a machine-bound id. A deliberately weaker, clearly
//! logged degraded mode relative to the OS keystore.

use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::SecretStore;

pub struct EncryptedFileStore {
    dir: PathBuf,
    passphrase: String,
}

impl EncryptedFileStore {
    pub fn new(subdir: &str) -> Result<Self> {
        let dir = crate::paths::data_dir()?.join(subdir);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            passphrase: machine_passphrase(),
            dir,
        })
    }

    fn file(&self, account: &str) -> PathBuf {
        // Hash the account into a filename-safe, non-reversible-on-disk handle.
        self.dir.join(format!("{}.age", simple_hash(account)))
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
        let decryptor =
            match age::Decryptor::new(&ciphertext[..]).context("opening fallback secret")? {
                age::Decryptor::Passphrase(d) => d,
                _ => anyhow::bail!("unexpected age recipient in fallback secret"),
            };
        let mut reader = decryptor
            .decrypt(&age::secrecy::Secret::new(self.passphrase.clone()), None)
            .context("decrypting fallback secret")?;
        let mut plain = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut plain)?;
        Ok(Some(plain))
    }

    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()> {
        use std::io::Write;
        let encryptor = age::Encryptor::with_user_passphrase(age::secrecy::Secret::new(
            self.passphrase.clone(),
        ));
        let mut ciphertext = Vec::new();
        let mut writer = encryptor
            .wrap_output(&mut ciphertext)
            .context("encrypting fallback secret")?;
        writer.write_all(blob)?;
        writer.finish()?;
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
        let key = format!("pitstopx:{}", hex(&bytes));
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
