//! `sessions::fmt` (issue #1746): the number, size and error-kind words every
//! `sessions` message is built from.
//!
//! Contract:
//! - `thousands(u64) -> String`: ASCII digits with `,` every three.
//! - `gib(u64) -> String`: bytes / 2^30 with two decimals, no unit.
//! - `kind_word(io::ErrorKind) -> &'static str`: one of `permission denied`,
//!   `not found`, `already exists`, `disk full`, `I/O error`.
//! - `esc(impl AsRef<Path>) -> String`: the path as text with control
//!   characters escaped; a plain path is unchanged (no quotes, no cut).

use super::fmt::{esc, gib, kind_word, thousands};
use std::io::ErrorKind;
use std::path::Path;

#[test]
fn thousands_separates_groups_of_three() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(1), "1");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1_000), "1,000");
    assert_eq!(thousands(28_013), "28,013");
    assert_eq!(thousands(1_234_567), "1,234,567");
    assert_eq!(thousands(u64::MAX), "18,446,744,073,709,551,615");
}

#[test]
fn gib_has_two_decimals() {
    assert_eq!(gib(0), "0.00");
    assert_eq!(gib(1), "0.00");
    assert_eq!(gib(1 << 30), "1.00");
    assert_eq!(gib(8_611_000_000), "8.02");
    assert_eq!(gib(3 * (1 << 29)), "1.50");
    // Never panics or overflows.
    assert!(!gib(u64::MAX).is_empty());
}

#[test]
fn kind_word_maps_to_the_five_documented_words() {
    assert_eq!(kind_word(ErrorKind::PermissionDenied), "permission denied");
    assert_eq!(kind_word(ErrorKind::NotFound), "not found");
    assert_eq!(kind_word(ErrorKind::AlreadyExists), "already exists");
    assert_eq!(kind_word(ErrorKind::StorageFull), "disk full");
    for other in [
        ErrorKind::Other,
        ErrorKind::InvalidData,
        ErrorKind::UnexpectedEof,
        ErrorKind::Interrupted,
        ErrorKind::WouldBlock,
    ] {
        assert_eq!(kind_word(other), "I/O error", "{other:?}");
    }
}

#[test]
fn esc_leaves_plain_paths_alone() {
    let long = format!("/tmp/{}/x.jsonl", "a".repeat(200));
    assert_eq!(esc(Path::new(&long)), long, "a long path must not be cut");
    assert_eq!(
        esc(Path::new("/home/u/.claude/projects/-p")),
        "/home/u/.claude/projects/-p"
    );
    assert_eq!(esc("-home-u-src-demo"), "-home-u-src-demo");
    assert_eq!(esc("ünïcode-ok"), "ünïcode-ok");
}

#[test]
fn esc_escapes_control_characters() {
    for raw in [
        "a\nb",
        "a\rb",
        "a\u{1b}[31mb",
        "a\u{7}b",
        "a\u{0}b",
        "a\u{7f}b",
    ] {
        let out = esc(raw);
        assert!(!out.chars().any(char::is_control), "{raw:?} gave {out:?}");
        assert!(out.starts_with('a') && out.ends_with('b'), "{out:?}");
    }
}

#[cfg(unix)]
#[test]
fn esc_handles_non_utf8_without_panicking() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let p = Path::new(OsStr::from_bytes(b"/tmp/\xff\n"));
    let out = esc(p);
    assert!(!out.chars().any(char::is_control), "{out:?}");
}
