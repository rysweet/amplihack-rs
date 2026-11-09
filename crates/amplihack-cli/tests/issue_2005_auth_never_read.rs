//! Issue #2005: the Codex rotation and `codex-resume` never open a file
//! inside an account directory: not `auth.json`, not `config.toml`, not a
//! session rollout. Every fixture account holds a FIFO `auth.json` (an
//! open blocks forever) and a mode-000 `config.toml`; here the rollout is a
//! FIFO too. Each command must still finish within the shared ceiling
//! (`support::wait`), so an open of any of them fails the test.
//!
//! A source scan backs this up: in the Codex rotation module, file-opening
//! calls appear only in the three readers that may open a file
//! (`state.rs` for `codex-state.json`, `session.rs` for the `--events`
//! file and `inuse.rs` for `/proc`).

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::profile_rotate::text;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

fn fifo_rollout(c: &CodexFixture, account: &str) -> PathBuf {
    let rel = PathBuf::from(format!(
        "sessions/2026/11/08/rollout-2026-11-08T10-00-00-{SESSION}.jsonl"
    ));
    let p = c.account(account).join(&rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    mkfifo(&p);
    rel
}

fn all_traps_are_fifos(c: &CodexFixture) {
    let traps = c.auth_traps();
    assert!(!traps.is_empty());
    assert!(traps.iter().all(|(_, _, fifo)| *fifo), "{traps:?}");
}

#[test]
fn a_cycle_and_a_swap_finish_with_fifo_logins() {
    let c = CodexFixture::standard();
    let traps = c.auth_traps();
    c.dry_run();
    c.rotate();
    assert_eq!(c.live().as_deref(), Some("ggrant"));
    assert_eq!(c.auth_traps(), traps);
    all_traps_are_fifos(&c);
}

#[test]
fn a_resume_links_a_fifo_rollout_without_opening_it() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.limited("ffarmer", PAST_RESET);
    c.reading("ggrant", 10, 10);
    c.link_to("ffarmer");
    let rel = fifo_rollout(&c, "ffarmer");
    let ff = c.account("ffarmer").display().to_string();
    let out = c.resume(&["--session", SESSION, "--from-home", &ff]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let linked = c.account("ggrant").join(&rel);
    assert!(
        fs::symlink_metadata(&linked)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::metadata(&linked).unwrap().file_type().is_fifo());
    all_traps_are_fifos(&c);
}

#[test]
fn a_real_home_with_a_fifo_login_is_refused_without_reading_it() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.reading("ffarmer", 50, 50);
    c.reading("ggrant", 10, 10);
    trapped(&c.link);
    c.rotate();
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=home-not-link")
    );
    assert!(
        fs::symlink_metadata(c.link.join("auth.json"))
            .unwrap()
            .file_type()
            .is_fifo()
    );
}

fn codex_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate/codex")
}

#[test]
fn only_the_three_readers_open_files() {
    let dir = codex_src();
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!(
            "the Codex rotation module {} must exist: {e}",
            dir.display()
        )
    });
    let allowed = ["state.rs", "session.rs", "inuse.rs"];
    let forbidden = [
        "File::open",
        "fs::read(",
        "fs::read_to_string",
        "read_to_string(",
        "OpenOptions",
    ];
    let mut seen = 0;
    for e in entries {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name.starts_with("tests") {
            continue;
        }
        seen += 1;
        if allowed.contains(&name.as_str()) {
            continue;
        }
        let src = fs::read_to_string(&p).unwrap();
        for f in forbidden {
            assert!(
                !src.contains(f),
                "{name} uses {f}: only {allowed:?} may open files"
            );
        }
    }
    assert!(
        seen >= 10,
        "expected the Codex rotation files in {}",
        dir.display()
    );
}

#[test]
fn no_codex_rotation_source_names_a_credential_file_to_open() {
    let dir = codex_src();
    for e in fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name.starts_with("tests") || name == "session.rs" {
            continue;
        }
        let src = fs::read_to_string(&p).unwrap();
        for word in ["\"auth.json\"", "\"config.toml\"", ".credentials.json"] {
            assert!(!src.contains(word), "{name} names {word}");
        }
    }
}

// ---- SEC-AUTH-3: the live reader is covered by the same scan ----

fn reader_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../amplihack-utils/src/codex_capacity")
}

/// Source lines that are code: `//` comments and doc comments are dropped,
/// so a comment may say a file is never read.
fn code_lines(src: &str) -> impl Iterator<Item = &str> {
    src.lines().filter(|l| !l.trim_start().starts_with("//"))
}

#[test]
fn the_live_reader_never_names_a_credential_file_or_opens_one() {
    let dir = reader_src();
    let entries = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("the reader {} must exist: {e}", dir.display()));
    let mut seen = 0;
    for e in entries {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name.starts_with("tests") {
            continue;
        }
        seen += 1;
        let src = fs::read_to_string(&p).unwrap();
        for line in code_lines(&src) {
            for word in ["auth.json", "config.toml", ".credentials.json"] {
                assert!(!line.contains(word), "{name} names {word}: {line}");
            }
            for f in [
                "File::open",
                "fs::read(",
                "fs::read_to_string",
                "OpenOptions",
            ] {
                assert!(
                    !line.contains(f),
                    "{name} opens a file ({f}); the reader only talks to codex app-server: {line}"
                );
            }
        }
    }
    assert!(
        seen >= 3,
        "expected mod.rs, parse.rs and rpc.rs in {}",
        dir.display()
    );
}

#[test]
fn the_resume_cap_module_opens_no_file() {
    let p =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../amplihack-utils/src/limit_resume_cap.rs");
    let src = fs::read_to_string(&p).unwrap();
    for line in code_lines(&src) {
        for f in ["File::open", "fs::read", "OpenOptions", "auth.json"] {
            assert!(!line.contains(f), "limit_resume_cap.rs: {line}");
        }
    }
}

// ---- TEST-1: the test-only variables are read only in debug builds ----

/// Every non-test `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "testdata" {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") && !name.starts_with("tests") {
            out.push(p);
        }
    }
}

/// The nearest `#[cfg(...)]` line above `idx`, if any.
fn nearest_cfg<'a>(lines: &[&'a str], idx: usize) -> Option<&'a str> {
    lines[..idx]
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|l| l.starts_with("#[cfg("))
}

#[test]
fn test_only_codex_variables_are_read_only_under_debug_assertions() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src_root, &mut files);
    let mut names: Vec<String> = Vec::new();
    let mut hits = 0;
    // Pass 1: the constants that hold the names, each gated.
    for p in &files {
        let src = fs::read_to_string(p).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            if t.starts_with("//") || !t.contains("\"AMPLIHACK_TEST_CODEX_") {
                continue;
            }
            hits += 1;
            assert_eq!(
                lines[..i]
                    .iter()
                    .rev()
                    .map(|l| l.trim())
                    .find(|l| !l.starts_with("///") && !l.starts_with("//")),
                Some("#[cfg(debug_assertions)]"),
                "{}:{}: a test-only variable name outside #[cfg(debug_assertions)]: {l}",
                p.display(),
                i + 1
            );
            if let Some(name) = t
                .strip_prefix("const ")
                .or_else(|| t.strip_prefix("pub(super) const "))
                .or_else(|| t.strip_prefix("pub(crate) const "))
                .or_else(|| t.strip_prefix("pub const "))
                .and_then(|r| r.split(':').next())
            {
                names.push(name.trim().to_string());
            }
        }
    }
    assert!(hits >= 2, "expected the PROC_ROOT and FAULT variable names");
    // Pass 2: every read of an env var through those constants is gated.
    for p in &files {
        let src = fs::read_to_string(p).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            if t.starts_with("//") || !t.contains("var") {
                continue;
            }
            for n in &names {
                if t.contains(&format!("var_os({n}")) || t.contains(&format!("var({n}")) {
                    assert_eq!(
                        nearest_cfg(&lines, i),
                        Some("#[cfg(debug_assertions)]"),
                        "{}:{}: {n} is read outside #[cfg(debug_assertions)]",
                        p.display(),
                        i + 1
                    );
                }
            }
        }
    }
}
