use rand::Rng;
use thiserror::Error;

/// Identity token alphabet: A–Z a–z 0–9 (no symbols).
pub const TOKEN_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

pub const TOKEN_LEN: usize = 128;
pub const TOKEN_LINE_LEN: usize = 16;
pub const TOKEN_LINES: usize = TOKEN_LEN / TOKEN_LINE_LEN; // 8

pub const HANDLE_MIN: usize = 3;
pub const HANDLE_MAX: usize = 32;
pub const POST_MAX_LEN: usize = 512;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("handle must be {HANDLE_MIN}–{HANDLE_MAX} characters: lowercase letters, digits, underscore")]
    InvalidHandle,
    #[error("token must be exactly {TOKEN_LEN} alphanumeric characters")]
    InvalidToken,
    #[error("post content must be 1–{POST_MAX_LEN} characters after trim")]
    InvalidContent,
}

/// Normalize handle: trim, lowercase. Returns validated handle.
pub fn normalize_handle(raw: &str) -> Result<String, ValidationError> {
    let h = raw.trim().to_ascii_lowercase();
    if h.len() < HANDLE_MIN || h.len() > HANDLE_MAX {
        return Err(ValidationError::InvalidHandle);
    }
    if !h
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ValidationError::InvalidHandle);
    }
    Ok(h)
}

/// Keep only A–Z a–z 0–9 (login paste stripping).
pub fn strip_token(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

pub fn validate_token(token: &str) -> Result<(), ValidationError> {
    if token.len() != TOKEN_LEN {
        return Err(ValidationError::InvalidToken);
    }
    if !token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ValidationError::InvalidToken);
    }
    Ok(())
}

/// Generate a 128-char CSPRNG token from the alphabet.
pub fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    (0..TOKEN_LEN)
        .map(|_| {
            let idx = rng.gen_range(0..TOKEN_ALPHABET.len());
            TOKEN_ALPHABET[idx] as char
        })
        .collect()
}

pub fn hash_token(token: &str) -> [u8; 32] {
    *blake3::hash(token.as_bytes()).as_bytes()
}

/// Format token as 8 lines of 16 chars for the reveal UI.
pub fn format_token_block(token: &str) -> String {
    token
        .as_bytes()
        .chunks(TOKEN_LINE_LEN)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn new_id() -> String {
    xid::new().to_string()
}

/// Trim and validate post content.
pub fn validate_content(raw: &str) -> Result<String, ValidationError> {
    let content = raw.trim().to_string();
    if content.is_empty() || content.chars().count() > POST_MAX_LEN {
        return Err(ValidationError::InvalidContent);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_normalizes() {
        assert_eq!(normalize_handle("  Alice_1 ").unwrap(), "alice_1");
        assert!(normalize_handle("ab").is_err());
        assert!(normalize_handle("Bad-Handle").is_err());
    }

    #[test]
    fn token_roundtrip() {
        let t = generate_token();
        assert_eq!(t.len(), TOKEN_LEN);
        validate_token(&t).unwrap();
        assert_eq!(strip_token(&format_token_block(&t).replace('\n', " ")), t);
        let h = hash_token(&t);
        assert_eq!(h, hash_token(&t));
        assert_ne!(h, hash_token("other"));
    }

    #[test]
    fn content_rules() {
        assert!(validate_content("   ").is_err());
        assert!(validate_content("hi").is_ok());
        let long: String = "a".repeat(513);
        assert!(validate_content(&long).is_err());
    }
}
