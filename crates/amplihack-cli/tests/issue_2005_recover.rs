//! Issue #2005: an interrupted Codex swap is finished or rolled back on the
//! next cycle, from the journal in `codex-state.json`. Interruptions are
//! made with the debug-only fault hook (`AMPLIHACK_TEST_CODEX_FAULT`), never
//! with timing. Journal paths are never trusted: a tampered journal removes
//! nothing. See docs/reference/profile-command.md, "Codex recovery".

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::profile_rotate::{assert_log_grammar, text};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

fn ino(p: &Path) -> u64 {
    fs::metadata(p).unwrap().ino()
}

fn journal(c: &CodexFixture, from: &str, to: &str, phase: &str) -> serde_json::Value {
    serde_json::json!({
        "from": from,
        "to": to,
        "link": c.link,
        "old_target": c.account(from),
        "new_target": c.account(to),
        "from_ino": ino(&c.account(from)),
        "to_ino": ino(&c.account(to)),
        "phase": phase
    })
}

fn with_journal(c: &CodexFixture, j: serde_json::Value) {
    c.write_state(&serde_json::json!({"version": 1, "journal": j, "resumes": []}));
}

fn faulted(c: &CodexFixture, point: &str) -> std::process::Output {
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env(FAULT_ENV, point);
    let out = c.run_cmd(&format!("rotate with fault {point}"), cmd);
    assert!(!out.status.success(), "the fault hook must stop the swap");
    out
}

fn journal_of(c: &CodexFixture) -> Option<serde_json::Value> {
    c.state()
        .and_then(|s| s.get("journal").cloned())
        .filter(|j| !j.is_null())
}

#[test]
fn a_swap_stopped_after_the_journal_is_rolled_back() {
    let c = CodexFixture::standard();
    faulted(&c, "after-journal");
    let j = journal_of(&c).expect("journal written before the fault");
    assert_eq!(j["phase"], "prepared");
    assert_eq!(j["from"], "ffarmer");
    assert_eq!(j["to"], "ggrant");
    assert_eq!(j["link"], c.link.to_str().unwrap());
    assert_eq!(c.live().as_deref(), Some("ffarmer"), "link not yet moved");

    // A temp symlink as a crash between symlink() and rename() leaves it.
    let tmp = c.link.with_file_name(".codex.amplihack-swap");
    std::os::unix::fs::symlink(c.account("ggrant"), &tmp).unwrap();

    c.rotate();
    let log = c.codex_log();
    let n = log.len();
    assert!(log[n - 2].ends_with(" -> none(recovering)"), "{log:?}");
    assert_eq!(
        log[n - 1],
        "codex recover ffarmer->ggrant reason=rolled-back"
    );
    assert!(journal_of(&c).is_none(), "journal closed");
    assert!(fs::symlink_metadata(&tmp).is_err(), "temp symlink removed");
    assert_eq!(
        c.live().as_deref(),
        Some("ffarmer"),
        "a recovering cycle makes no swap"
    );
    assert!(
        !c.f.log().contains("change: codex_home"),
        "a rollback writes no change line"
    );

    // The next cycle decides again and swaps.
    c.rotate();
    assert_eq!(c.live().as_deref(), Some("ggrant"));
}

#[test]
fn a_swap_stopped_after_the_rename_is_finished() {
    let c = CodexFixture::standard();
    faulted(&c, "after-rename");
    assert_eq!(c.live().as_deref(), Some("ggrant"), "rename happened");
    let j = journal_of(&c).expect("journal still open");
    assert_eq!(j["phase"], "prepared");
    assert!(!c.f.log().contains("change: codex_home"));

    c.rotate();
    let log = c.codex_log();
    let n = log.len();
    assert!(
        log[n - 2].starts_with("codex cycle live=ggrant "),
        "{log:?}"
    );
    assert!(log[n - 2].ends_with(" -> none(recovering)"), "{log:?}");
    assert_eq!(log[n - 1], "codex recover ffarmer->ggrant reason=recovered");
    assert!(journal_of(&c).is_none());
    assert!(c.state().unwrap()["last_swap_at"].as_i64().is_some());
    assert_eq!(
        c.f.log()
            .lines()
            .filter(|l| l.contains("change: codex_home ffarmer -> ggrant by pid="))
            .count(),
        1,
        "{}",
        c.f.log()
    );
    for l in c.f.log().lines() {
        assert_log_grammar(l, true);
    }
}

#[test]
fn a_linked_journal_is_finished_without_a_second_change_line() {
    let c = CodexFixture::standard();
    c.link_to("ggrant");
    with_journal(&c, journal(&c, "ffarmer", "ggrant", "linked"));
    c.rotate();
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex recover ffarmer->ggrant reason=recovered")
    );
    assert!(
        !c.f.log().contains("change: codex_home"),
        "already recorded"
    );
    assert!(journal_of(&c).is_none());
}

#[test]
fn a_link_that_matches_neither_target_keeps_the_journal() {
    let c = CodexFixture::standard();
    c.link_to("hhale");
    let j = journal(&c, "ffarmer", "ggrant", "prepared");
    with_journal(&c, j.clone());
    let before = fs::read(c.state_path()).unwrap();
    c.rotate();
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=journal-mismatch")
    );
    assert_eq!(fs::read(c.state_path()).unwrap(), before, "journal kept");
    assert_eq!(c.live().as_deref(), Some("hhale"));
}

#[test]
fn a_new_target_with_another_inode_keeps_the_journal() {
    let c = CodexFixture::standard();
    c.link_to("ggrant");
    let mut j = journal(&c, "ffarmer", "ggrant", "prepared");
    j["to_ino"] = serde_json::json!(1);
    with_journal(&c, j);
    c.rotate();
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=journal-mismatch")
    );
    assert!(journal_of(&c).is_some());
}

#[test]
fn a_journal_naming_another_link_removes_nothing() {
    let c = CodexFixture::standard();
    let victim = c.base.join("victim-link");
    std::os::unix::fs::symlink(c.account("hhale"), &victim).unwrap();
    let victim_tmp = c.base.join("victim-link.amplihack-swap");
    std::os::unix::fs::symlink(c.account("hhale"), &victim_tmp).unwrap();
    let mut j = journal(&c, "ffarmer", "ggrant", "prepared");
    j["link"] = serde_json::json!(victim);
    with_journal(&c, j);
    c.rotate();
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=journal-mismatch")
    );
    assert!(fs::symlink_metadata(&victim).is_ok());
    assert!(
        fs::symlink_metadata(&victim_tmp).is_ok(),
        "victim temp removed"
    );
    assert!(journal_of(&c).is_some(), "journal kept");
    assert_eq!(c.live().as_deref(), Some("ffarmer"));
}

#[test]
fn a_journal_with_targets_outside_the_root_removes_nothing() {
    let c = CodexFixture::standard();
    let outside = c.base.join("outside");
    trapped(&outside);
    let mut j = journal(&c, "ffarmer", "ggrant", "prepared");
    j["old_target"] = serde_json::json!(outside);
    j["new_target"] = serde_json::json!("/");
    with_journal(&c, j);
    let tmp = c.link.with_file_name(".codex.amplihack-swap");
    std::os::unix::fs::symlink(&outside, &tmp).unwrap();
    c.rotate();
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=journal-mismatch")
    );
    assert!(outside.join("auth.json").exists());
    assert!(
        fs::symlink_metadata(&tmp).is_ok(),
        "nothing removed on a mismatch"
    );
    assert!(journal_of(&c).is_some());
}

#[test]
fn a_dry_run_reports_an_open_journal_and_changes_nothing() {
    let c = CodexFixture::standard();
    with_journal(&c, journal(&c, "ffarmer", "ggrant", "prepared"));
    let before = fs::read(c.state_path()).unwrap();
    let out = c.dry_run();
    let codex = codex_lines(&text(&out.stdout));
    assert!(
        codex.contains(&"dry-run would-recover ffarmer->ggrant phase=prepared".to_string()),
        "{codex:?}"
    );
    assert!(!codex.iter().any(|l| l.starts_with("codex recover ")));
    assert_eq!(fs::read(c.state_path()).unwrap(), before);
    assert_eq!(c.live().as_deref(), Some("ffarmer"));
}

#[test]
fn the_fault_hook_names_are_the_only_ones_honoured() {
    let c = CodexFixture::standard();
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env(FAULT_ENV, "after-everything");
    let out = c.run_cmd("rotate with an unknown fault name", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(c.live().as_deref(), Some("ggrant"));
    assert!(journal_of(&c).is_none());
}
