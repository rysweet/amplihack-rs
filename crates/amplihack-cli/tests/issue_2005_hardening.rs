//! Issue #2005, review fixes: `codex-resume` uses only accounts that are
//! directories under the root and never follows a symlinked folder or an
//! `auth.json` reached through a symlink; recovery runs before every other
//! rule; an unreadable state file is reported in every cycle. See
//! docs/reference/profile-command.md, "Codex decision", "Codex recovery"
//! and "Codex resume command".

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::profile_rotate::text;
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// `ffarmer` limited (the session's account), `ggrant` with room, `hhale`
/// limited; the session's rollout is under `ffarmer`.
fn setup() -> CodexFixture {
    let c = CodexFixture::with(&["ffarmer", "ggrant", "hhale"]);
    c.limited("ffarmer", PAST_RESET);
    c.reading("ggrant", 20, 30);
    c.limited("hhale", PAST_RESET);
    c.link_to("ffarmer");
    c.rollout("ffarmer", SESSION);
    c
}

fn from(c: &CodexFixture, name: &str) -> String {
    c.account(name).display().to_string()
}

fn journal_of(c: &CodexFixture) -> Option<serde_json::Value> {
    c.state()
        .and_then(|s| s.get("journal").cloned())
        .filter(|j| !j.is_null())
}

fn faulted(c: &CodexFixture, point: &str) {
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env(FAULT_ENV, point);
    let out = c.run_cmd(&format!("rotate with fault {point}"), cmd);
    assert!(!out.status.success(), "the fault hook must stop the swap");
}

#[test]
fn resume_never_uses_an_account_that_links_outside_the_root() {
    let c = setup();
    c.limited("ggrant", PAST_RESET);
    let outside = c.base.join("elsewhere").join("evil");
    trapped(&outside);
    std::os::unix::fs::symlink(&outside, c.root.join("evil")).unwrap();
    c.reading("evil", 1, 1);

    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    assert!(
        !c.reads().iter().any(|r| r == "evil"),
        "the outside account was read: {:?}",
        c.reads()
    );
    assert!(fs::symlink_metadata(outside.join("sessions")).is_err());
    assert!(c.resumes().is_empty(), "codex must not start");
}

#[test]
fn resume_does_not_follow_a_symlinked_sessions_folder() {
    let c = setup();
    let outside = c.base.join("outside-sessions");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, c.account("ggrant").join("sessions")).unwrap();

    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("symlink"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        0,
        "created outside"
    );
    assert!(c.resumes().is_empty());
}

#[test]
fn an_events_symlink_to_an_auth_json_is_refused() {
    let c = setup();
    let backup = c.base.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let auth = backup.join("auth.json");
    // Events that would otherwise resume: the refusal is the name check.
    fs::write(
        &auth,
        format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{SESSION}\"}}\n\
             {{\"type\":\"turn.failed\",\"error\":{{\"message\":\"You've hit your usage limit.\"}}}}\n"
        ),
    )
    .unwrap();
    let events = c.base.join("events.jsonl");
    std::os::unix::fs::symlink(&auth, &events).unwrap();

    let ev = events.display().to_string();
    let out = c.resume(&["--events", &ev, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("a file named auth.json is never read"),
        "{}",
        text(&out.stderr)
    );
    assert!(c.resumes().is_empty());
}

#[test]
fn an_interrupted_swap_is_recovered_while_no_account_has_room() {
    let c = CodexFixture::standard();
    faulted(&c, "after-rename");
    assert_eq!(c.live().as_deref(), Some("ggrant"));
    for a in ["ffarmer", "ggrant", "hhale"] {
        c.limited(a, RESET_AT);
    }

    c.rotate();
    let log = c.codex_log();
    let n = log.len();
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
}

#[test]
fn an_open_journal_with_the_link_missing_runs_recovery_not_no_live() {
    let c = CodexFixture::standard();
    faulted(&c, "after-journal");
    fs::remove_file(&c.link).unwrap();

    c.rotate();
    let log = c.codex_log();
    let n = log.len();
    assert!(log[n - 2].ends_with(" -> none(recovering)"), "{log:?}");
    assert_eq!(log[n - 1], "codex refuse reason=journal-mismatch");
    assert!(journal_of(&c).is_some(), "the journal is kept");
    assert!(fs::symlink_metadata(&c.link).is_err(), "nothing relinked");
}

#[test]
fn a_too_open_state_file_is_reported_in_a_keep_cycle() {
    let c = CodexFixture::standard();
    c.link_to("ggrant");
    c.write_state(&serde_json::json!({"version": 1, "resumes": []}));
    fs::set_permissions(c.state_path(), fs::Permissions::from_mode(0o644)).unwrap();

    c.rotate();
    let log = c.codex_log();
    let n = log.len();
    assert!(log[n - 2].ends_with(" -> keep"), "{log:?}");
    assert_eq!(
        log[n - 1],
        "codex refuse reason=journal-mismatch \
         detail=state-unreadable:not_private__mode_0600_or_narrower_"
    );
    let m = fs::metadata(c.state_path()).unwrap();
    assert_eq!(m.permissions().mode() & 0o777, 0o644, "the file is kept");
    assert_eq!(c.live().as_deref(), Some("ggrant"));
}

#[test]
fn resume_does_not_follow_a_symlinked_folder_below_sessions() {
    // FS-6: every folder codex-resume walks or creates under
    // `<dst>/sessions/` is checked, not only `sessions` itself.
    let c = setup();
    let outside = c.base.join("outside-year");
    fs::create_dir_all(&outside).unwrap();
    let sessions = c.account("ggrant").join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&outside, sessions.join("2026")).unwrap();

    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        0,
        "a folder or link was made outside the account"
    );
    assert!(c.resumes().is_empty(), "codex must not start");
}
