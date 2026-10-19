//! Issue #1618, AC11: Claude's usage-limit patterns live in one place,
//! `crates/amplihack-cli/src/commands/recipe/run/limit_patterns.rs`.
//!
//! Fails if a string literal in any other source file under
//! `crates/amplihack-cli/src` is exactly a usage-limit marker or the
//! `resets ` anchor, or contains Claude's epoch-form message. Equality, not
//! containment, is used for the markers so that ordinary messages that
//! mention a usage limit are not flagged. Every string literal counts,
//! including log and error text; only `//` comments (and `/* */` blocks) and
//! `tests_*.rs` files are skipped. The markers are spelled out here because
//! an integration test cannot read the crate-private table.

use std::fs;
use std::path::{Path, PathBuf};

const MARKERS: [&str; 8] = [
    "session limit",
    "usage limit",
    "usage_limit_reached",
    "quota exceeded",
    "monthly limit",
    "weekly limit",
    "spend limit",
    "credit balance is too low",
];
const ANCHOR: &str = "resets ";
/// Held in two pieces so this file is not itself an example of the pattern.
fn claude_epoch_message() -> String {
    format!("Claude AI usage limit {}", "reached")
}

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn home_file() -> PathBuf {
    src_dir().join("commands/recipe/run/limit_patterns.rs")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The string literals of `src` (contents only, escapes left as written),
/// with comments, char literals and lifetimes skipped. Raw strings
/// (`r"…"`, `r#"…"#`) and byte strings are included.
fn string_literals(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'r' if (i == 0
                || !ident(b[i - 1])
                || (b[i - 1] == b'b' && (i < 2 || !ident(b[i - 2]))))
                && matches!(b.get(i + 1), Some(b'"') | Some(b'#')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let start = j + 1;
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let end = (start..b.len())
                    .find(|&k| b[k..].starts_with(&close))
                    .unwrap_or(b.len());
                out.push(src[start..end].to_string());
                i = end + close.len();
            }
            b'"' => {
                let start = i + 1;
                let mut k = start;
                while k < b.len() && b[k] != b'"' {
                    k += if b[k] == b'\\' { 2 } else { 1 };
                }
                let end = k.min(b.len());
                out.push(src[start..end].to_string());
                i = end + 1;
            }
            b'\'' => {
                // A char literal ('x', '\n', '\'', '"') or a lifetime ('a).
                if b.get(i + 1) == Some(&b'\\') {
                    // Skip the escaped character itself, which may be '\''.
                    let mut k = i + 3;
                    while k < b.len() && b[k] != b'\'' {
                        k += 1;
                    }
                    i = k + 1;
                } else if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                } else {
                    // Multi-byte char literal or a lifetime.
                    let next = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                    if b.get(i + 1 + next) == Some(&b'\'') {
                        i += 2 + next;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn violation(lit: &str) -> bool {
    MARKERS.contains(&lit) || lit == ANCHOR || lit.contains(&claude_epoch_message())
}

#[test]
fn the_literal_scanner_finds_strings_and_skips_comments() {
    let src = r##"
        // "usage limit" in a line comment is skipped
        /* "session limit" in a block comment is skipped */
        let a = "usage limit"; let c = '"'; let q = '\''; fn f<'a>(x: &'a str) {}
        let r = r#"resets "#; let e = "esc \" usage limit"; let u = 'é';
        let b = b"weekly limit"; let br = br"spend limit";
    "##;
    let lits = string_literals(src);
    assert_eq!(
        lits,
        vec![
            "usage limit",
            "resets ",
            "esc \\\" usage limit",
            "weekly limit",
            "spend limit"
        ]
    );
    let flagged: Vec<&String> = lits.iter().filter(|l| violation(l)).collect();
    assert_eq!(
        flagged,
        ["usage limit", "resets ", "weekly limit", "spend limit"]
    );
    assert!(violation(&format!("{}|", claude_epoch_message())));
    assert!(
        !violation("hit the usage limit, waiting"),
        "containment is fine"
    );
}

#[test]
fn the_pattern_home_holds_the_claude_patterns() {
    let home = fs::read_to_string(home_file())
        .unwrap_or_else(|e| panic!("{} is missing: {e}", home_file().display()));
    let lits = string_literals(&home);
    for m in MARKERS {
        assert!(
            lits.iter().any(|l| l == m),
            "marker {m:?} is not in the table"
        );
    }
    assert!(lits.iter().any(|l| l.contains(&claude_epoch_message())));
    assert!(lits.iter().any(|l| l == ANCHOR));
}

#[test]
fn no_other_source_file_holds_a_usage_limit_pattern() {
    let mut files = Vec::new();
    rs_files(&src_dir(), &mut files);
    assert!(files.len() > 50, "found only {} source files", files.len());
    let home = home_file();
    let mut found = Vec::new();
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy();
        if f == home || (name.starts_with("tests_") && name.ends_with(".rs")) {
            continue;
        }
        let src = fs::read_to_string(&f).unwrap();
        for lit in string_literals(&src) {
            if violation(&lit) {
                let rel = f.strip_prefix(src_dir()).unwrap().display().to_string();
                found.push(format!("{rel}: {lit:?}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "usage-limit patterns outside limit_patterns.rs:\n{}",
        found.join("\n")
    );
}
