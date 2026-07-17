use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

use crate::config::data_dir;

/// Plain-text file with the raw token (mode 0600). Always written so the user
/// can open it outside the TUI if clipboard/selection fails.
pub fn token_file_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("token-ONCE.txt"))
}

/// Persist token to disk (0600) and try every clipboard mechanism we know.
/// Returns a short status string for the UI.
pub fn export_token(token: &str) -> Result<String> {
    let path = token_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, token).with_context(|| format!("write {}", path.display()))?;
    let mut perms = std::fs::metadata(&path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(&path, perms)?;

    let mut methods = vec![format!("file:{}", path.display())];

    if osc52_copy(token).is_ok() {
        methods.push("osc52".into());
    }
    if wl_copy(token) {
        methods.push("wl-copy".into());
    }
    if xclip_copy(token) {
        methods.push("xclip".into());
    }
    if xsel_copy(token) {
        methods.push("xsel".into());
    }

    Ok(methods.join(" · "))
}

/// OSC 52 — works in Ghostty, kitty, iTerm2, many SSH setups.
fn osc52_copy(text: &str) -> io::Result<()> {
    let b64 = B64.encode(text.as_bytes());
    // Prefer BEL terminator; some terminals want ST.
    let seq = format!("\x1b]52;c;{b64}\x07");
    let mut out = io::stdout();
    out.write_all(seq.as_bytes())?;
    out.flush()?;
    Ok(())
}

fn wl_copy(text: &str) -> bool {
    let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn xclip_copy(text: &str) -> bool {
    let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn xsel_copy(text: &str) -> bool {
    let Ok(mut child) = Command::new("xsel")
        .args(["--clipboard", "--input"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}
