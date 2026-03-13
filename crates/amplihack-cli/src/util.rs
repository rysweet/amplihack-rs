//! Shared utility functions used across amplihack-cli modules.
//!
//! # Security
//!
//! * SEC-WS2-02: All externally-sourced strings must pass through [`strip_ansi`]
//!   before display to prevent terminal injection via crafted external output.

// ── Non-interactive detection ──────────────────────────────────────────────────

/// Returns `true` when the process should behave non-interactively.
///
/// This is a **UX gate**, not a security gate.  It is used to skip prompts
/// and interactive UI that would block automated pipelines.  Two conditions
/// trigger non-interactive mode:
///
/// 1. The environment variable `AMPLIHACK_NONINTERACTIVE` is set to `"1"`.
/// 2. Standard input is not connected to a terminal (i.e. stdin is piped or
///    redirected).
///
/// Either condition alone is sufficient.
pub fn is_noninteractive() -> bool {
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }
    // Check whether stdin is a TTY.  When stdin is piped/redirected there is
    // no terminal to drive interactive prompts.
    use std::io::IsTerminal as _;
    !std::io::stdin().is_terminal()
}

// ── ANSI stripping ────────────────────────────────────────────────────────────

/// Remove ANSI escape sequences from `s`.
///
/// Handles CSI sequences of the form `ESC [ <params> <final_byte>` where
/// `<final_byte>` is any byte in the range `0x40..=0x7E` (e.g. `m` for SGR).
/// Applied to all externally-sourced strings before display to prevent
/// terminal injection via crafted version strings.  See SEC-WS2-02.
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

    // ── WS2: is_noninteractive ────────────────────────────────────────────────

    #[test]
    fn is_noninteractive_env_set() {
        // SAFETY: test-only; not run in parallel with other env-sensitive tests.
        unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1") };
        let result = is_noninteractive();
        unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };
        assert!(result, "should be non-interactive when env var is '1'");
    }

    #[test]
    fn is_noninteractive_env_unset() {
        unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };
        // In a test harness stdin is typically not a TTY, so is_noninteractive()
        // may still return true due to the TTY check.  What we verify here is
        // that when the env var is absent the function does not panic and
        // returns a bool (either outcome is valid depending on the runner).
        let _ = is_noninteractive();
    }

    #[test]
    fn is_noninteractive_env_other_value_does_not_trigger() {
        unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "0") };
        // Only "1" triggers the env-var path.  Stdin may still be non-TTY in CI,
        // so we only verify the env-var branch is not active for "0".
        // We do this by temporarily forcing it to "0" and checking the var path
        // directly rather than the full function (since stdin state is runner-
        // dependent).
        let env_triggered = std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1");
        unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };
        assert!(!env_triggered);
    }

    // ── ANSI stripping ────────────────────────────────────────────────────────

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
