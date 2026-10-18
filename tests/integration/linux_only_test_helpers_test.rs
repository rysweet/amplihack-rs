//! Contracts for the macOS lint allows in `issue_1569_auto_switch.rs` (PR #1530).
//!
//! Six items in that test file are used only from code gated on
//! `#[cfg(target_os = "linux")]`. On macOS the gated code is compiled out, the
//! items become dead, and `cargo clippy --all-targets -- -D warnings` fails
//! the pre-commit hook. Each item therefore carries
//! `#[cfg_attr(not(target_os = "linux"), allow(dead_code))]`.
//!
//! CI's macOS jobs never run clippy on test targets, and on Linux the attribute
//! expands to nothing, so these source checks are the only guard. They also
//! check the reverse: an allow must not outlive the reason for it, because the
//! compiler does not warn when an allow no longer hides anything.

use std::fs;
use std::ops::Range;
use std::path::PathBuf;

const TARGET: &str = "crates/amplihack-cli/tests/issue_1569_auto_switch.rs";
const ALLOW: &str = r#"#[cfg_attr(not(target_os = "linux"), allow(dead_code))]"#;
const LINUX_GATE: &str = r#"#[cfg(target_os = "linux")]"#;

/// (definition prefix, pattern that marks a use of the item)
const HELPERS: &[(&str, &str)] = &[
    ("const ABORT_ENV:", "ABORT_ENV"),
    ("struct Holder(", "Holder"),
    ("fn hold(", ".hold("),
    ("fn hosts(", ".hosts("),
    ("fn oauth_uuid(", ".oauth_uuid("),
    ("fn parked(", ".parked("),
];

fn lines() -> Vec<String> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    let path = path.join(TARGET);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn definition(lines: &[String], prefix: &str) -> usize {
    let found: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].trim_start().starts_with(prefix))
        .collect();
    assert_eq!(found.len(), 1, "expected one `{prefix}` in {TARGET}");
    found[0]
}

/// Lines of the item starting at `start`: up to its first `;` if that comes
/// before any `{`, otherwise to the matching `}`.
fn item_span(lines: &[String], start: usize) -> Range<usize> {
    let mut depth = 0i64;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                ';' if !opened => return start..i + 1,
                _ => {}
            }
        }
        if opened && depth == 0 {
            return start..i + 1;
        }
    }
    panic!("item at line {} in {TARGET} never closes", start + 1);
}

fn linux_gated_spans(lines: &[String]) -> Vec<Range<usize>> {
    let spans: Vec<Range<usize>> = (0..lines.len())
        .filter(|&i| lines[i].trim() == LINUX_GATE)
        .map(|i| item_span(lines, i + 1))
        .collect();
    assert!(
        !spans.is_empty(),
        "{TARGET} no longer has Linux-gated code; drop the allows and this test"
    );
    spans
}

#[test]
fn linux_only_helpers_allow_dead_code_off_linux() {
    let lines = lines();
    for (prefix, _) in HELPERS {
        let def = definition(&lines, prefix);
        let above = lines[def - 1].trim();
        assert!(
            above.starts_with(ALLOW),
            "line {} `{prefix}` needs {ALLOW} directly above it (after any doc \
             comment), or macOS clippy -D warnings fails; found `{above}`",
            def + 1
        );
        // `def` is the 0-based index of the item, so it is the 1-based number
        // of the attribute line above it.
        assert!(
            above[ALLOW.len()..].trim_start().starts_with("//"),
            "line {}: give the allow on `{prefix}` a trailing reason comment",
            def
        );
    }
}

#[test]
fn allowed_helpers_are_used_only_from_linux_gated_code() {
    let lines = lines();
    let mut known: Vec<Range<usize>> = linux_gated_spans(&lines);
    for (prefix, _) in HELPERS {
        known.push(item_span(&lines, definition(&lines, prefix)));
    }
    known.push(item_span(
        &lines,
        definition(&lines, "impl Drop for Holder"),
    ));
    for (prefix, needle) in HELPERS {
        let stray: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].contains(needle))
            .filter(|i| !known.iter().any(|span| span.contains(i)))
            .map(|i| i + 1)
            .collect();
        assert!(
            stray.is_empty(),
            "`{prefix}` is used outside Linux-gated code at lines {stray:?}; \
             remove its dead_code allow"
        );
    }
}

#[test]
fn allows_stay_narrow_and_holder_drop_stays_unsuppressed() {
    let lines = lines();
    let text = lines.join("\n");
    assert!(
        !text.contains("#![allow"),
        "no file-level allows in {TARGET}"
    );
    for banned in ["allow(unused)", "allow(warnings)"] {
        assert!(!text.contains(banned), "{TARGET} must not use {banned}");
    }
    let allows = lines.iter().filter(|l| l.trim().starts_with(ALLOW)).count();
    assert_eq!(
        allows,
        HELPERS.len(),
        "expected exactly one dead_code allow per Linux-only helper"
    );
    // Holder's Drop kills and reaps the stand-in session process; it must be
    // compiled on every platform and never hidden behind an allow.
    let drop = definition(&lines, "impl Drop for Holder");
    assert!(
        !lines[drop - 1].contains("cfg"),
        "leave `impl Drop for Holder` unannotated"
    );
}
