use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use keyring::Entry;

use crate::client::config::data_dir;

const SERVICE: &str = "conr";

pub fn store_token(handle: &str, token: &str) -> Result<()> {
    // Always write the local file so auto-login works even if keyring fails.
    let path = fallback_path(handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, token)?;
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&path, perms)?;

    match Entry::new(SERVICE, handle) {
        Ok(entry) => {
            if let Err(e) = entry.set_password(token) {
                tracing::warn!(?e, "keyring set failed; using file store");
            }
        }
        Err(e) => tracing::warn!(?e, "keyring unavailable; using file store"),
    }
    Ok(())
}

pub fn load_token(handle: &str) -> Result<Option<String>> {
    if let Ok(entry) = Entry::new(SERVICE, handle) {
        if let Ok(pw) = entry.get_password() {
            return Ok(Some(pw));
        }
    }
    let path = fallback_path(handle)?;
    if path.exists() {
        let s = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        return Ok(Some(s.trim().to_string()));
    }
    Ok(None)
}

/// Permanently forget one account's identity token (file + keyring).
/// Does not affect other handles.
#[allow(dead_code)]
pub fn clear_token(handle: &str) -> Result<()> {
    if let Ok(entry) = Entry::new(SERVICE, handle) {
        let _ = entry.delete_credential();
    }
    let path = fallback_path(handle)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Handles that have a saved identity token on disk (for multi-account testing).
pub fn list_saved_handles() -> Result<Vec<String>> {
    let dir = data_dir()?.join("tokens");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut handles = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                if !name.is_empty() && !name.starts_with('.') {
                    handles.push(name.to_string());
                }
            }
        }
    }
    handles.sort();
    Ok(handles)
}

fn fallback_path(handle: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join("tokens").join(handle))
}

#[allow(dead_code)]
pub fn using_fallback(handle: &str) -> bool {
    if let Ok(entry) = Entry::new(SERVICE, handle) {
        if entry.get_password().is_ok() {
            return false;
        }
    }
    fallback_path(handle).map(|p| p.exists()).unwrap_or(false)
}
