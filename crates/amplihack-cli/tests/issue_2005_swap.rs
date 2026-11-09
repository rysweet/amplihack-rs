//! Issue #2005: a Codex swap relinks the default home (`~/.codex`) to the
//! account with the most room, by an atomic `rename()` of a temporary
//! symlink. No account directory is renamed, no login is opened, moved or
//! copied, and a job with an explicit `CODEX_HOME` keeps its account. See
//! docs/reference/profile-command.md, "Codex swap" and "Codex decision".

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::profile_rotate::{assert_log_grammar, text, unix_now};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn a_swap_relinks_the_home_and_moves_no_directory_or_login() {
    let c = CodexFixture::standard();
    let inodes = c.account_inodes();
    let traps = c.auth_traps();
    let before = unix_now();
    let out = c.rotate();
    assert!(out.status.success());

    // The link now points at the new account, with an absolute target.
    let target = fs::read_link(&c.link).unwrap();
    assert_eq!(target, c.account("ggrant"));
    assert!(target.is_absolute());
    assert!(
        fs::symlink_metadata(&c.link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Nothing renamed, nothing copied, every trap untouched.
    assert_eq!(c.account_inodes(), inodes);
    assert_eq!(c.auth_traps(), traps);
    assert!(c.auth_traps().iter().all(|(_, _, fifo)| *fifo));
    // No temp path left.
    let tmp = c.link.with_file_name(".codex.amplihack-swap");
    assert!(fs::symlink_metadata(&tmp).is_err(), "temp link left behind");

    let log = c.codex_log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].ends_with(" -> swap ffarmer->ggrant"), "{log:?}");
    assert_eq!(
        log[1],
        format!(
            "codex swap ffarmer->ggrant link={} reason=room",
            c.link.display()
        )
    );
    for l in c.f.log().lines() {
        assert_log_grammar(l, true);
    }
    assert!(
        c.f.log()
            .lines()
            .any(|l| l.contains("change: codex_home ffarmer -> ggrant by pid=")),
        "{}",
        c.f.log()
    );

    // State: last_swap_at set, no journal, private regular file.
    let state = c.state().expect("codex-state.json");
    assert_eq!(state["version"], 1, "{state}");
    assert!(state.get("journal").is_none_or(|j| j.is_null()), "{state}");
    let at = state["last_swap_at"].as_i64().expect("last_swap_at");
    assert!(at >= before && at <= unix_now(), "{state}");
    let m = fs::symlink_metadata(c.state_path()).unwrap();
    assert!(m.file_type().is_file());
    assert_eq!(m.mode() & 0o777, 0o600);
}

#[test]
fn the_claude_ledger_and_journal_never_hold_codex_data() {
    let c = CodexFixture::standard();
    c.rotate();
    for name in ["ledger.json", "journal.json"] {
        let raw = fs::read_to_string(c.f.rotate_dir().join(name)).unwrap_or_default();
        for word in ["ffarmer", "ggrant", "codex"] {
            assert!(!raw.contains(word), "{name} holds {word}: {raw}");
        }
    }
}

#[test]
fn a_second_cycle_after_a_swap_keeps_the_new_account() {
    let c = CodexFixture::standard();
    c.rotate();
    let out = c.rotate();
    assert!(out.status.success());
    let log = c.codex_log();
    let last = log
        .iter()
        .rev()
        .find(|l| l.starts_with("codex cycle "))
        .unwrap();
    assert!(last.starts_with("codex cycle live=ggrant "), "{last}");
    assert!(last.ends_with(" -> keep"), "{last}");
    assert_eq!(fs::read_link(&c.link).unwrap(), c.account("ggrant"));
}

#[test]
fn a_recent_codex_swap_holds_the_next_one() {
    let c = CodexFixture::standard();
    c.write_state(&serde_json::json!({"version": 1, "last_swap_at": unix_now(), "resumes": []}));
    let out = c.rotate();
    assert!(out.status.success());
    let log = c.codex_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].ends_with(" -> none(held)"), "{log:?}");
    assert_eq!(c.live().as_deref(), Some("ffarmer"));

    // `--hold 0` turns the hold off.
    let out = c.ok(&["profile", "rotate", "--hold", "0"]);
    assert!(out.status.success());
    assert_eq!(c.live().as_deref(), Some("ggrant"));
}

#[test]
fn a_claude_swap_does_not_count_towards_the_codex_hold() {
    let c = CodexFixture::standard();
    // The Claude part swaps work away in this same run; the Codex swap must
    // still happen (only `last_swap_at` in codex-state.json holds it).
    c.rotate();
    assert!(c.f.log().lines().any(|l| l.contains(" swap work->")));
    assert_eq!(c.live().as_deref(), Some("ggrant"));
}

#[test]
fn a_small_gain_is_damped_by_the_margin() {
    let c = CodexFixture::standard();
    // live ffarmer room 59, ggrant 66: a gain of 7.
    let out = c.ok(&["profile", "rotate", "--margin", "10"]);
    assert!(out.status.success());
    let log = c.codex_log();
    assert!(log[0].ends_with(" -> none(damped)"), "{log:?}");
    assert_eq!(log.len(), 1);
    assert_eq!(c.live().as_deref(), Some("ffarmer"));

    let out = c.ok(&["profile", "rotate", "--margin", "7"]);
    assert!(out.status.success());
    assert_eq!(c.live().as_deref(), Some("ggrant"), "{:?}", c.codex_log());
}

#[test]
fn a_full_live_account_is_never_held_or_damped() {
    let c = CodexFixture::standard();
    c.reading("ffarmer", 100, 50);
    c.write_state(&serde_json::json!({"version": 1, "last_swap_at": unix_now(), "resumes": []}));
    let out = c.ok(&["profile", "rotate", "--margin", "100"]);
    assert!(out.status.success());
    assert_eq!(c.live().as_deref(), Some("ggrant"), "{:?}", c.codex_log());
}

#[test]
fn a_job_with_an_explicit_codex_home_keeps_its_account() {
    let c = CodexFixture::standard();
    // A job started with CODEX_HOME=<root>/ffarmer: its path and the inode
    // of its directory stay, and its trap file is untouched.
    let job_home = c.account("ffarmer");
    let ino = fs::metadata(&job_home).unwrap().ino();
    let auth_ino = fs::symlink_metadata(job_home.join("auth.json"))
        .unwrap()
        .ino();
    c.rotate();
    assert_eq!(c.live().as_deref(), Some("ggrant"));
    assert_eq!(fs::metadata(&job_home).unwrap().ino(), ino);
    assert_eq!(
        fs::symlink_metadata(job_home.join("auth.json"))
            .unwrap()
            .ino(),
        auth_ino
    );
    assert!(job_home.join("config.toml").exists());
}

#[test]
fn a_process_with_another_explicit_codex_home_does_not_block_the_swap() {
    let c = CodexFixture::standard();
    let cwd = c.base.join("cwd-job");
    fs::create_dir_all(&cwd).unwrap();
    let home = format!("CODEX_HOME={}", c.account("ffarmer").display());
    fake_proc(
        &c.proc_root,
        4242,
        "codex",
        "/usr/bin/codex",
        &["codex", "exec"],
        &["PATH=/usr/bin", &home],
        &cwd,
    );
    c.rotate();
    assert_eq!(c.live().as_deref(), Some("ggrant"), "{:?}", c.codex_log());
}

#[test]
fn a_relative_link_target_under_the_root_is_swapped_to_an_absolute_one() {
    let c = CodexFixture::standard();
    fs::remove_file(&c.link).unwrap();
    let rel = pathdiff(&c.account("ffarmer"), c.link.parent().unwrap());
    std::os::unix::fs::symlink(&rel, &c.link).unwrap();
    c.rotate();
    assert_eq!(fs::read_link(&c.link).unwrap(), c.account("ggrant"));
}

#[test]
fn the_state_file_is_private_and_written_atomically() {
    let c = CodexFixture::standard();
    c.rotate();
    let dir = c.f.rotate_dir();
    let m = fs::metadata(&dir).unwrap();
    assert_eq!(m.permissions().mode() & 0o077, 0, "rotate dir is private");
    // No leftover temp files beside the state file.
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("codex-state") && n != "codex-state.json")
        .collect();
    assert!(names.is_empty(), "{names:?}");
}

/// `to` relative to `from` (both absolute, `from` a directory).
fn pathdiff(to: &std::path::Path, from: &std::path::Path) -> std::path::PathBuf {
    let to: Vec<_> = to.components().collect();
    let from: Vec<_> = from.components().collect();
    let common = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut p = std::path::PathBuf::new();
    for _ in common..from.len() {
        p.push("..");
    }
    for c in &to[common..] {
        p.push(c);
    }
    p
}

#[test]
fn rotate_status_is_unchanged_by_the_codex_part() {
    let c = CodexFixture::standard();
    c.rotate();
    let out = c.ok(&["profile", "rotate", "status"]);
    let s = text(&out.stdout);
    assert!(!s.contains("ggrant") && !s.contains("ffarmer"), "{s}");
}
