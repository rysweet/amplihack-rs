//! The number, size and error words every `sessions` message is built from.
//! No message formats an `io::Error`, a parser error or file contents: error
//! detail goes through [`kind_word`], and every path or name through [`esc`].

use std::io::ErrorKind;
use std::path::Path;

/// `28013` → `28,013`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Bytes as GiB (2^30) with two decimals, without the unit.
pub fn gib(bytes: u64) -> String {
    format!("{:.2}", bytes as f64 / f64::from(1u32 << 30))
}

/// The one word or phrase an I/O error is reduced to.
pub fn kind_word(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::PermissionDenied => "permission denied",
        ErrorKind::NotFound => "not found",
        ErrorKind::AlreadyExists => "already exists",
        ErrorKind::StorageFull => "disk full",
        _ => "I/O error",
    }
}

/// A path or name as text, with every control character escaped (`\n`,
/// `\u{1b}`). Non-UTF-8 bytes become U+FFFD. Nothing is quoted or cut.
pub fn esc(p: impl AsRef<Path>) -> String {
    let raw = p.as_ref().to_string_lossy();
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}
