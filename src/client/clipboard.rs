use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::shared::{sniff_image_type, MEDIA_MAX_BYTES};
use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

use crate::client::config::data_dir;

/// Image bytes pulled from the system clipboard, if any.
#[derive(Debug)]
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    /// Suggested filename (e.g. `clipboard.png`).
    pub filename: String,
}

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

/// Read an image from the system clipboard (Wayland `wl-paste` or X11 `xclip`).
/// Returns `Ok(None)` when no image is on the clipboard.
pub fn read_image() -> Result<Option<ClipboardImage>> {
    if let Some(img) = wl_paste_image()? {
        return Ok(Some(img));
    }
    if let Some(img) = xclip_paste_image()? {
        return Ok(Some(img));
    }
    Ok(None)
}

const IMAGE_MIMES: &[(&str, &str)] = &[
    ("image/png", "clipboard.png"),
    ("image/jpeg", "clipboard.jpg"),
    ("image/webp", "clipboard.webp"),
    ("image/gif", "clipboard.gif"),
];

fn finalize_image(bytes: Vec<u8>, fallback_name: &str) -> Result<Option<ClipboardImage>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > MEDIA_MAX_BYTES {
        anyhow::bail!("image too large (max {MEDIA_MAX_BYTES} bytes)");
    }
    let Some(ct) = sniff_image_type(&bytes) else {
        return Ok(None);
    };
    let filename = match ct {
        "image/png" => "clipboard.png",
        "image/jpeg" => "clipboard.jpg",
        "image/webp" => "clipboard.webp",
        "image/gif" => "clipboard.gif",
        _ => fallback_name,
    };
    Ok(Some(ClipboardImage {
        bytes,
        filename: filename.into(),
    }))
}

fn wl_paste_image() -> Result<Option<ClipboardImage>> {
    // Prefer listing types so we request a concrete image MIME.
    let types = Command::new("wl-paste")
        .args(["--list-types"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let type_list = match types {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Ok(None), // wl-paste missing or no clipboard
    };

    for (mime, name) in IMAGE_MIMES {
        if !type_list
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case(mime))
        {
            continue;
        }
        let output = Command::new("wl-paste")
            .args(["--type", mime, "--no-newline"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let Ok(o) = output else { continue };
        if !o.status.success() {
            continue;
        }
        if let Some(img) = finalize_image(o.stdout, name)? {
            return Ok(Some(img));
        }
    }

    // Some compositors only advertise a generic type; try png/jpeg blindly.
    for (mime, name) in IMAGE_MIMES {
        let output = Command::new("wl-paste")
            .args(["--type", mime, "--no-newline"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let Ok(o) = output else { continue };
        if !o.status.success() || o.stdout.is_empty() {
            continue;
        }
        if let Some(img) = finalize_image(o.stdout, name)? {
            return Ok(Some(img));
        }
    }
    Ok(None)
}

fn xclip_paste_image() -> Result<Option<ClipboardImage>> {
    let targets = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let target_list = match targets {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Ok(None),
    };

    for (mime, name) in IMAGE_MIMES {
        if !target_list
            .split_whitespace()
            .any(|t| t.eq_ignore_ascii_case(mime))
        {
            continue;
        }
        let output = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", mime, "-o"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let Ok(o) = output else { continue };
        if !o.status.success() {
            continue;
        }
        if let Some(img) = finalize_image(o.stdout, name)? {
            return Ok(Some(img));
        }
    }
    Ok(None)
}

/// True when `text` looks like a single local path that exists and is an image file.
pub fn path_from_paste(text: &str) -> Option<PathBuf> {
    let t = text.trim().trim_matches('"').trim_matches('\'');
    if t.is_empty() || t.contains('\n') || t.contains('\r') {
        return None;
    }
    // Allow `file://` URIs from some paste sources.
    let path = if let Some(rest) = t.strip_prefix("file://") {
        PathBuf::from(rest)
    } else {
        PathBuf::from(t)
    };
    if !path.is_file() {
        return None;
    }
    // Quick extension gate; full validation is magic-byte sniff on read.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "webp" | "gif") => Some(path),
        _ => {
            // Still try if we can sniff without extension.
            if let Ok(mut f) = std::fs::File::open(&path) {
                let mut head = [0u8; 16];
                if f.read(&mut head).ok().filter(|&n| n > 0).is_some()
                    && sniff_image_type(&head).is_some()
                {
                    return Some(path);
                }
            }
            None
        }
    }
}
