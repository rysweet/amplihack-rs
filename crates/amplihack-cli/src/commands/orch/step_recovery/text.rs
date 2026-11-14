//! Checks on free-text values: agent reply fields, record fields and the
//! caller's `--owner`. Text never carries control or bidi characters, so it
//! cannot inject terminal escapes or reorder what `report` prints.

use anyhow::{Result, ensure};

/// Longest text field (reason, repair, needs, owner).
pub const TEXT_LIMIT: usize = 1024;

/// A control character, or a bidi embedding, override or isolate
/// (U+202A-U+202E, U+2066-U+2069) that could reorder terminal text.
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// Checks a text field: 1..=1024 bytes, no control or bidi characters.
pub fn check_text(name: &str, value: &str) -> Result<()> {
    ensure!(
        (1..=TEXT_LIMIT).contains(&value.len()),
        "{name} must be 1-{TEXT_LIMIT} bytes"
    );
    ensure!(
        !value.chars().any(is_unsafe_char),
        "{name} must not contain control or bidi characters"
    );
    Ok(())
}

/// A caller's `--owner`: `^[A-Za-z0-9._@/-]{1,128}$`.
pub fn validate_owner(owner: &str) -> Result<()> {
    ensure!(
        (1..=128).contains(&owner.len())
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'/' | b'-')),
        "--owner must match ^[A-Za-z0-9._@/-]{{1,128}}$: {owner:?}"
    );
    Ok(())
}
