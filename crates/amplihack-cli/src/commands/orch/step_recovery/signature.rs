//! Failure signature: `exit=<n|none>;[class=<c>;]tail=<16 hex>`.
//!
//! The tail hash is taken over the redacted output after normalisation, so
//! the same failure with different commit ids, PIDs or timestamps gives the
//! same signature and a different error message gives a different one.

use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static HEX_ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b[0-9a-f]{7,}\b").expect("constant regex"));
static DIGITS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[0-9]+").expect("constant regex"));

/// Lines trimmed, blank lines dropped, ids of 7+ hex digits (with at least
/// one letter) replaced by `H`, then digit runs replaced by `0`.
///
/// A run of digits alone is a number, not an id, even when it is 7 digits
/// long: PIDs and counters vary in length, and a 7-digit PID must normalise
/// the same way as a 3-digit one.
pub fn normalise(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut out = Vec::new();
    for line in lowered.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ids = HEX_ID.replace_all(line, |caps: &regex::Captures<'_>| {
            let run = &caps[0];
            if run.bytes().any(|b| b.is_ascii_alphabetic()) {
                "H".to_string()
            } else {
                run.to_string()
            }
        });
        out.push(DIGITS.replace_all(&ids, "0").into_owned());
    }
    out.join("\n")
}

/// Builds the signature from the exit code, an optional failure class and
/// the redacted output tail.
pub fn signature(exit_code: Option<i32>, class: Option<&str>, redacted_tail: &str) -> String {
    let digest = Sha256::digest(normalise(redacted_tail).as_bytes());
    let tail: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let exit = exit_code.map_or_else(|| "none".to_string(), |c| c.to_string());
    match class {
        Some(class) => format!("exit={exit};class={class};tail={tail}"),
        None => format!("exit={exit};tail={tail}"),
    }
}

/// The recipe failure classifier's class for this text, when one applies.
pub fn class_of(redacted_tail: &str) -> Option<&'static str> {
    use crate::commands::recipe::run::{FailureClass, classify_failure_text};
    match classify_failure_text(redacted_tail).0 {
        FailureClass::Indeterminate => None,
        class => Some(class.as_str()),
    }
}
