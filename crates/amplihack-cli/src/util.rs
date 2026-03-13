//! Shared utility functions used across amplihack-cli modules.
//!
//! # Security
//!
//! * SEC-WS2-02: All externally-sourced strings must pass through [`strip_ansi`]
//!   before display to prevent terminal injection via crafted external output.

// ── Shared constants ──────────────────────────────────────────────────────────

/// Maximum length of a version string extracted from external tool output.
///
/// Applied after [`strip_ansi`] so the bound measures visible characters,
/// not raw bytes.  Prevents adversarial version strings from allocating
/// unbounded memory or polluting log output.  See SEC-WS2-02.
pub const MAX_VERSION_LEN: usize = 80;

// ── ANSI stripping ────────────────────────────────────────────────────────────

/// Remove ANSI CSI escape sequences from `s`.
///
/// Handles **CSI sequences only** — sequences of the form
/// `ESC [ <params> <final_byte>` where `<final_byte>` is any byte in the
/// range `0x40..=0x7E` (e.g. `m` for SGR colour/attribute codes).
///
/// Limitations: OSC sequences (`ESC ]`), Fe sequences (`ESC` followed by
/// bytes `0x40..=0x5F` other than `[`), and C1 control codes
/// (`0x80..=0x9F`) are **not** stripped by this function.  For the
/// use-cases in this codebase (sanitising tool version strings before
/// display) CSI coverage is sufficient.
///
/// Applied to all externally-sourced strings before display to prevent
/// terminal injection via crafted version strings.  See SEC-WS2-02.
/// CODE-4.
pub fn strip_ansi(s: &str) -> String {
    // Pre-allocate the full input length — stripped output is always ≤ input.
    // Avoids repeated Vec reallocations for inputs containing many ANSI sequences.
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Detect CSI sequence: ESC (0x1B) followed by '[' (0x5B)
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2; // skip ESC [
            // Consume bytes until the final byte (0x40–0x7E inclusive)
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break; // final byte consumed — CSI sequence done
                }
            }
        } else {
            // Regular character — copy it, advancing by its full UTF-8 width.
            // SAFETY: `s` is valid UTF-8 (guaranteed by `&str`); indexing at a
            // known byte boundary via `chars().next()` is always safe.
            let ch = s[i..].chars().next().expect("non-empty slice");
            result.push(ch);
            i += ch.len_utf8();
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_passthrough_on_plain_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        let input = "\x1b[1mbold\x1b[0m normal";
        assert_eq!(strip_ansi(input), "bold normal");
    }

    #[test]
    fn strip_ansi_removes_multiple_sequences() {
        let input = "\x1b[32m\x1b[1mgreen bold\x1b[0m";
        assert_eq!(strip_ansi(input), "green bold");
    }
}
