//! The two value rules that both `amplihack agent` (Unix only) and the
//! launch path's effort and model inheritance (issue #1883, every platform)
//! apply. They live here, without a `cfg`, so the launch path uses the same
//! rule rather than a copy; `validate` re-exports them for its own callers.

const MAX_OPTION_BYTES: usize = 256;

/// A control character, or a Unicode format character that can make shown
/// text differ from what it holds: the bidirectional embeddings, overrides
/// and isolates (U+202A to U+202E, U+2066 to U+2069), the zero-width and
/// direction marks (U+200B to U+200F) and the byte-order mark (U+FEFF).
pub fn unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Model, effort and permission mode: 1 to 256 bytes, no control
/// character, and no leading `-`, so the value can never become a flag.
pub fn option_value(v: &str) -> Result<(), String> {
    let ok = !v.is_empty()
        && v.len() <= MAX_OPTION_BYTES
        && !v.starts_with('-')
        && !v.chars().any(unsafe_char);
    if ok {
        Ok(())
    } else {
        Err(
            "must be 1 to 256 bytes, without control characters, and must not start with '-'"
                .into(),
        )
    }
}
