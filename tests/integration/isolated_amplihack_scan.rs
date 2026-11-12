//! Issue #1606 (review of #1647): every test that starts the `amplihack`
//! binary goes through `tests/support/isolated_amplihack.rs::isolated`, so no
//! test can read or migrate a developer's real `~/.amplihack`.
//!
//! This scans every `*.rs` file under `crates/*/tests`, `bins/*/tests` and
//! `tests/` for a `Command::new(..)` whose argument is the amplihack binary:
//! it names `CARGO_BIN_EXE_amplihack`, calls `amplihack_bin(..)`, or uses a
//! `fn`/`const` of the same file whose definition does, or a `let` binding
//! whose initializer does (`let bin = amplihack_bin(); Command::new(&bin)`,
//! the most common form). Bindings are matched by name across the file, so a
//! name reused for another program is flagged too: rename it.
//!
//! The binary is recognised by [`MARKERS`]: the cargo variable, the cli
//! crate's `amplihack_bin()`, `AMPLIHACK_TEST_BINARY`, and the
//! `current_exe()...join("amplihack")` path the harnesses build by hand.
//!
//! Known limit: a binary path that reaches `Command::new` as a function
//! parameter is not followed (`issue_1569_auto_switch.rs`, where `cmd_bin`
//! takes `bin: &Path`, and `issue_538_install_completeness.rs`, where a
//! helper takes the resolved path). Those sites were converted by hand;
//! review a new function that takes the binary as a parameter the same way.
use std::{fs, path::Path};

const MARKERS: &[&str] = &[
    "CARGO_BIN_EXE_amplihack\"",
    "amplihack_bin(",
    "\"AMPLIHACK_TEST_BINARY\"",
    ".join(\"amplihack\")",
];
const EXEMPT: &[&str] = &[
    "tests/support/isolated_amplihack.rs",
    "tests/integration/isolated_amplihack_scan.rs",
];

/// `source` with full-line `//` comments blanked, so prose that quotes a
/// spawn is not mistaken for one. Byte offsets are unchanged.
fn without_line_comments(source: &str) -> String {
    source
        .split_inclusive('\n')
        .map(|line| {
            if line.trim_start().starts_with("//") {
                line.chars()
                    .map(|c| if c == '\n' { c } else { ' ' })
                    .collect()
            } else {
                line.to_string()
            }
        })
        .collect()
}

/// `source` with the contents of every string literal (`"..."`, raw
/// `r#"..."#`) blanked to spaces, so text inside a string (a pattern such as
/// `"Command::new(\"sh\")"`, a program name such as `"tmux"`) is not read as
/// code. Byte offsets are unchanged, so a position found here indexes the
/// original source too, where the markers (themselves strings) are checked.
fn blank_strings(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                // A char literal ('x', '\n', '\''), not a lifetime.
                let len = if bytes.get(i + 1) == Some(&b'\\') {
                    4
                } else {
                    3
                };
                if bytes.get(i + len - 1) == Some(&b'\'') {
                    for b in &mut out[i + 1..i + len - 1] {
                        *b = b' ';
                    }
                    i += len;
                } else {
                    i += 1;
                }
            }
            b'r' if matches!(bytes.get(i + 1), Some(b'"' | b'#'))
                && !bytes[..i]
                    .last()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') =>
            {
                let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
                let open = i + 1 + hashes;
                if bytes.get(open) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let end = bytes[open + 1..]
                    .windows(close.len())
                    .position(|w| w == close.as_slice())
                    .map_or(bytes.len(), |p| open + 1 + p);
                for b in &mut out[open + 1..end] {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
                i = end + close.len();
            }
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                for b in &mut out[i + 1..j.min(bytes.len())] {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

/// The identifier that starts `text`.
fn ident(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

/// Whether `name` occurs in `text` as a whole identifier that is not a field
/// (`self.bin` is not the binding `bin`).
fn uses(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        let boundary = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        boundary(before) && before != Some('.') && boundary(after)
    })
}

/// Local `fn`/`const` items whose definition (up to the next `fn`/`const`)
/// reaches the binary, directly or through another such item.
fn binary_items(source: &str, code: &str) -> Vec<String> {
    let mut starts: Vec<(usize, String)> = Vec::new();
    for keyword in ["fn ", "const "] {
        for (at, _) in code.match_indices(keyword) {
            let preceded_ok = code[..at]
                .chars()
                .next_back()
                .is_none_or(|c| c.is_whitespace());
            let name = ident(&code[at + keyword.len()..]);
            if preceded_ok && !name.is_empty() && name != "fn" {
                starts.push((at, name.to_string()));
            }
        }
    }
    starts.sort();
    let items: Vec<(String, &str, &str)> = starts
        .iter()
        .enumerate()
        .map(|(i, (at, name))| {
            let end = starts.get(i + 1).map_or(code.len(), |next| next.0);
            (name.clone(), &source[*at..end], &code[*at..end])
        })
        .collect();
    let mut found: Vec<String> = Vec::new();
    loop {
        let before = found.len();
        for (name, raw, body) in &items {
            let reaches = MARKERS.iter().any(|marker| raw.contains(marker))
                || found.iter().any(|known| known != name && uses(body, known));
            if reaches && !found.contains(name) {
                found.push(name.clone());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Names bound by `let` (or `let mut`) whose initializer, up to the `;`,
/// reaches the binary: a marker, a binary item, or another such binding.
fn binary_bindings(source: &str, code: &str, items: &[String]) -> Vec<String> {
    let mut lets: Vec<(String, &str, &str)> = Vec::new();
    for (at, _) in code.match_indices("let ") {
        let preceded_ok = code[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || c == '{' || c == ';');
        if !preceded_ok {
            continue;
        }
        let mut start = at + "let ".len();
        if code[start..].starts_with("mut ") {
            start += "mut ".len();
        }
        let rest = &code[start..];
        let name = ident(rest);
        let Some(eq) = rest.find('=') else { continue };
        let end = rest[eq..].find(';').map_or(rest.len(), |semi| eq + semi);
        if !name.is_empty() {
            lets.push((
                name.to_string(),
                &source[start + eq + 1..start + end],
                &rest[eq + 1..end],
            ));
        }
    }
    let mut found: Vec<String> = Vec::new();
    loop {
        let before = found.len();
        for (name, raw, init) in &lets {
            let reaches = MARKERS.iter().any(|marker| raw.contains(marker))
                || items.iter().any(|item| uses(init, item))
                || found.iter().any(|known| uses(init, known));
            if reaches && !found.contains(name) {
                found.push(name.clone());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// The argument text of the call whose `(` is at `open`, up to its match.
fn call_argument(source: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (offset, c) in source[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    &source[open + 1..]
}

/// 1-based lines of `source` that build a `Command` for the amplihack binary.
fn offending_lines(source: &str) -> Vec<usize> {
    let source = without_line_comments(source);
    let code = blank_strings(&source);
    let items = binary_items(&source, &code);
    let bindings = binary_bindings(&source, &code, &items);
    let mut lines = Vec::new();
    for (at, call) in code.match_indices("Command::new(") {
        let open = at + call.len() - 1;
        let argument = call_argument(&code, open);
        let raw = &source[open + 1..open + 1 + argument.len()];
        let is_binary = MARKERS.iter().any(|marker| raw.contains(marker))
            || items.iter().any(|item| uses(argument, item))
            || bindings.iter().any(|binding| uses(argument, binding));
        if is_binary {
            lines.push(source[..at].matches('\n').count() + 1);
        }
    }
    lines
}

fn scan(root: &Path, path: &Path, violations: &mut Vec<String>) {
    if path.is_dir() {
        let name = path.file_name().unwrap_or_default();
        if name == "target" || name.to_string_lossy().starts_with('.') {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            scan(root, &entry.unwrap().path(), violations);
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        let relative = path.strip_prefix(root).unwrap().to_string_lossy();
        if EXEMPT.contains(&relative.as_ref()) {
            return;
        }
        let source = fs::read_to_string(path).unwrap();
        for line in offending_lines(&source) {
            violations.push(format!("{relative}:{line}"));
        }
    }
}

#[test]
fn the_detector_catches_a_bare_spawn_and_passes_an_isolated_one() {
    let sample = r##"fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}
fn bad() { let _ = Command::new(bin()).output(); }
fn good() { let _ = isolated(bin()).output(); }
"##;
    assert_eq!(offending_lines(sample), vec![4]);
}

/// Review of #1647: each idiom the scan follows (a marker, a local item, a
/// `let` binding, a chain of bindings), reverted to a bare `Command::new`, is
/// caught; the isolated form of each passes. A path passed in as a function
/// parameter is not followed (see the module docs).
#[test]
fn the_detector_catches_each_idiom_through_let_bindings() {
    let sample = r##"use crate::support::amplihack_bin::amplihack_bin;
fn direct() { Command::new(env!("CARGO_BIN_EXE_amplihack")); }
fn helper() { Command::new(amplihack_bin()); }
fn bound() {
    let bin = amplihack_bin();
    let mut c = Command::new(&bin);
}
fn bound_from_env() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    Command::new(exe.clone());
}
fn chained() {
    let first = amplihack_bin();
    let second = first.clone();
    Command::new(second);
}
fn test_binary_override() {
    let cli = std::env::var_os("AMPLIHACK_TEST_BINARY").map(PathBuf::from);
    Command::new(cli.unwrap());
}
fn current_exe_join() {
    let cli = std::env::current_exe().unwrap().parent().unwrap().join("amplihack");
    Command::new(&cli);
}
fn isolated_forms() {
    let bin = amplihack_bin();
    isolated(&bin);
    isolated(amplihack_bin());
    Command::new("sh");
}
fn not_the_binary(fx: &Fx) {
    let runner = amplihack_bin();
    Command::new(&fx.runner);
    let patterns = [r#"Command::new(bin)"#, "Command::new(runner)"];
    let quote = '"';
    Command::new("runner");
}
"##;
    assert_eq!(offending_lines(sample), vec![2, 3, 6, 10, 15, 19, 23]);
}

#[test]
fn every_test_starts_amplihack_through_the_isolation_helper() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().unwrap();
    let mut violations = Vec::new();
    for parent in ["crates", "bins"] {
        for entry in fs::read_dir(root.join(parent)).unwrap() {
            let tests = entry.unwrap().path().join("tests");
            if tests.is_dir() {
                scan(&root, &tests, &mut violations);
            }
        }
    }
    scan(&root, &root.join("tests"), &mut violations);
    assert!(
        violations.is_empty(),
        "start the amplihack binary with `isolated(..)` from \
         tests/support/isolated_amplihack.rs, not `Command::new(..)`:\n{}",
        violations.join("\n")
    );
}
