//! Issue #1954 (R8, D11): install, update, self-heal, the install lock, the
//! release layout and uninstall never signal a process they did not start.
//!
//! A source scan, so a later change cannot add a `kill` of a foreign process
//! without this test naming it. The only signals allowed are on the caller's
//! own child:
//!
//! - anything in `util.rs` (`run_with_timeout`, `terminate_timed_out_child`);
//! - `Child::kill` (a method call, `.kill()`), which reaches only a child this
//!   process spawned;
//! - `clone.rs::terminate_git`, which kills the process group of the `git`
//!   child it started with `setsid`.
//!
//! Each failure is printed as `file:line: text`.

use std::fs;
use std::path::{Path, PathBuf};

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Files and directory trees in scope, relative to `src/`.
const SCOPE_TREES: &[&str] = &["commands/install", "update"];
const SCOPE_FILES: &[&str] = &[
    "self_heal.rs",
    "install_lock.rs",
    "uninstall.rs",
    "commands/install/release_layout.rs",
    "commands/install/uninstall.rs",
];

/// `(file suffix, enclosing fn)` pairs allowed to signal their own child.
const ALLOWED: &[(&str, &str)] = &[("commands/install/clone.rs", "terminate_git")];

/// Text that sends a signal or starts a signalling tool.
const PATTERNS: &[&str] = &[
    "libc::kill",
    "killpg",
    "Signal::",
    "nix::sys::signal",
    "Command::new(\"kill\")",
    "Command::new(\"pkill\")",
    "Command::new(\"killall\")",
    "Command::new(\"taskkill\")",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // Test code may stop the processes it starts.
            if name == "tests" {
                continue;
            }
            rust_files(&path, out);
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

fn scanned_files() -> Vec<PathBuf> {
    let root = src();
    let mut files = Vec::new();
    for tree in SCOPE_TREES {
        rust_files(&root.join(tree), &mut files);
    }
    for file in SCOPE_FILES {
        let path = root.join(file);
        if path.is_file() && !files.contains(&path) {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// A bare `kill(` call: not a method (`.kill(`) and not part of a longer
/// identifier (`do_kill(`).
fn has_bare_kill_call(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(at) = line[from..].find("kill(") {
        let at = from + at;
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let is_method = before == Some(b'.');
        let is_ident = before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
        // `libc::kill(` is reported by its own pattern.
        let is_path = before == Some(b':');
        if !is_method && !is_ident && !is_path {
            return true;
        }
        from = at + 1;
    }
    false
}

fn enclosing_fn(lines: &[&str], index: usize) -> Option<String> {
    lines[..=index].iter().rev().find_map(|line| {
        let trimmed = line.trim_start();
        let at = trimmed.find("fn ")?;
        let head = &trimmed[..at];
        let is_decl = head.split_whitespace().all(|w| {
            matches!(
                w,
                "pub" | "pub(crate)" | "pub(super)" | "unsafe" | "async" | "const" | "extern"
            ) || w.starts_with("pub(")
        });
        if !is_decl {
            return None;
        }
        let rest = &trimmed[at + 3..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

fn violations(path: &Path) -> Vec<String> {
    let rel = path
        .strip_prefix(src())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    if rel == "util.rs" {
        return Vec::new();
    }
    let text = fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // The in-file unit tests start here; test code may stop its own children.
        if trimmed == "#[cfg(test)]"
            && lines[index + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| l.trim_start().starts_with("mod "))
        {
            break;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        let hit = PATTERNS.iter().any(|p| line.contains(p)) || has_bare_kill_call(line);
        if !hit {
            continue;
        }
        let function = enclosing_fn(&lines, index);
        let allowed = ALLOWED
            .iter()
            .any(|(file, func)| rel.ends_with(file) && function.as_deref() == Some(*func));
        if !allowed {
            found.push(format!("src/{rel}:{}: {}", index + 1, line.trim()));
        }
    }
    found
}

#[test]
fn the_scan_covers_the_lock_and_release_layout_modules() {
    let files = scanned_files();
    let root = src();
    let missing: Vec<&str> = ["install_lock.rs", "commands/install/release_layout.rs"]
        .into_iter()
        .filter(|rel| !files.contains(&root.join(rel)))
        .collect();
    assert!(
        missing.is_empty(),
        "issue #1954: the no-signal scan must cover {missing:?}; these modules hold the install \
         lock and the release cleanup (D6, D7)"
    );
    assert!(
        files.iter().any(|f| f.ends_with("self_heal.rs")),
        "the scan must cover self_heal.rs"
    );
}

#[test]
fn install_update_self_heal_and_uninstall_never_signal_foreign_processes() {
    let found: Vec<String> = scanned_files().iter().flat_map(|f| violations(f)).collect();
    assert!(
        found.is_empty(),
        "issue #1954 (R8): install code must never signal a process it did not start. Only \
         util::run_with_timeout / terminate_timed_out_child, Child::kill and \
         clone.rs::terminate_git may stop the caller's own child:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_scanner_reports_what_it_should() {
    assert!(has_bare_kill_call("    kill(pid, 15);"));
    assert!(has_bare_kill_call("let _ = unsafe { kill(pid, SIGTERM) };"));
    assert!(!has_bare_kill_call("child.kill()"));
    assert!(!has_bare_kill_call("let _ = do_kill(x);"));
    assert!(
        !has_bare_kill_call("libc::kill(pid, 9)"),
        "reported by its own pattern"
    );
    assert!(PATTERNS.iter().any(|p| "libc::kill(pid, 9)".contains(p)));
    assert!(
        PATTERNS
            .iter()
            .any(|p| "nix::sys::signal::kill(p, Signal::SIGTERM)".contains(p))
    );
    let lines = [
        "fn terminate_git(child: &mut Child) {",
        "    unsafe {",
        "        libc::kill(-pid, 9);",
    ];
    assert_eq!(enclosing_fn(&lines, 2).as_deref(), Some("terminate_git"));
}
