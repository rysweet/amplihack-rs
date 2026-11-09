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
