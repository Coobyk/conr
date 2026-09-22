use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::{rngs::OsRng, RngCore};

use crate::client::config::data_dir;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const MAGIC: &[u8] = b"CONRBAK1";

pub fn backup_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("backup").join("token"))
}

/// Encrypt token with Argon2id(passphrase) -> ChaCha20-Poly1305.
pub fn write_backup(token: &str, passphrase: &str) -> Result<()> {
    if passphrase.is_empty() {
        bail!("passphrase must not be empty");
    }
    let path = backup_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(passphrase, &salt)?;
    let key = Key::try_from(&key[..]).map_err(|e| anyhow::anyhow!("key: {e}"))?;
    let cipher = ChaCha20Poly1305::new(&key);
    let nonce = Nonce::try_from(&nonce_bytes[..]).map_err(|e| anyhow::anyhow!("nonce: {e}"))?;
    let ciphertext = cipher
        .encrypt(&nonce, token.as_bytes())
        .map_err(|e| anyhow::anyhow!("encrypt: {e}"))?;

    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    fs::write(&path, &out)?;
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&path, perms)?;
    Ok(())
}

#[allow(dead_code)]
pub fn read_backup(passphrase: &str) -> Result<String> {
    let path = backup_path()?;
    let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    if data.len() < MAGIC.len() + SALT_LEN + NONCE_LEN + 16 {
        bail!("backup file corrupted or too short");
    }
    if &data[..MAGIC.len()] != MAGIC {
        bail!("not a conr backup file");
    }
    let salt = &data[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &data[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let ciphertext = &data[MAGIC.len() + SALT_LEN + NONCE_LEN..];

    let key = derive_key(passphrase, salt)?;
    let key = Key::try_from(&key[..]).map_err(|e| anyhow::anyhow!("key: {e}"))?;
    let cipher = ChaCha20Poly1305::new(&key);
    let nonce =
        Nonce::try_from(&nonce_bytes[..]).map_err(|e| anyhow::anyhow!("nonce: {e}"))?;
    let plain = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("wrong passphrase or corrupted backup"))?;
    String::from_utf8(plain).context("token utf-8")
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    Ok(key)
}
