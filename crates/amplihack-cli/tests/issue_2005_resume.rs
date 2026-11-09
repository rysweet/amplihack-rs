//! Issue #2005: `amplihack profile codex-resume` resumes a Codex session
//! that stopped on a usage limit with `codex resume <id>`, on an account
//! with room, or after one wait for the earliest reset. See
//! docs/reference/profile-command.md, "Codex resume command".
//!
//! The fake `codex resume` records its argv, `CODEX_HOME`, whether an API
//! key reached it and the rollout it found. Resets are in the past, so the
//! single `sleep_until` returns at once and no test depends on the clock.

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::profile_rotate::{assert_log_grammar, text, unix_now};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// `ffarmer` limited (the session's account), `ggrant` with room, `hhale`
/// limited; the session's rollout is under `ffarmer`.
fn setup() -> (CodexFixture, std::path::PathBuf) {
    let c = CodexFixture::with(&["ffarmer", "ggrant", "hhale"]);
    c.limited("ffarmer", PAST_RESET);
    c.reading("ggrant", 20, 30);
    c.limited("hhale", PAST_RESET);
    c.link_to("ffarmer");
    let rel = c.rollout("ffarmer", SESSION);
    (c, rel)
}

fn from(c: &CodexFixture, name: &str) -> String {
    c.account(name).display().to_string()
}

fn resume_lines(c: &CodexFixture) -> Vec<String> {
    c.codex_log()
        .into_iter()
        .filter(|l| l.starts_with("codex resume "))
        .collect()
}

#[test]
fn a_limited_session_is_resumed_on_another_account_with_room() {
    let (c, rel) = setup();
    let ff = from(&c, "ffarmer");
    let out = c.resume(&[
        "--session",
        SESSION,
        "--from-home",
        &ff,
        "--",
        "continue please",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    let calls = c.resumes();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call = &calls[0];
    assert_eq!(call[0], "ARGC=2");
    assert_eq!(call[1], format!("ARG={SESSION}"), "the id comes first");
    assert_eq!(call[2], "ARG=continue please");
    assert_eq!(call[3], format!("CODEX_HOME={}", from(&c, "ggrant")));
    assert_eq!(call[4], "OPENAI_API_KEY=unset");
    assert_eq!(call[5], "CODEX_API_KEY=unset");
    assert!(
        call[6].starts_with(&format!("ROLLOUT=link {}", rel.display())),
        "{call:?}"
    );

    // The rollout is linked, not copied, and the source is untouched.
    let linked = c.account("ggrant").join(&rel);
    let m = fs::symlink_metadata(&linked).unwrap();
    assert!(m.file_type().is_symlink());
    assert_eq!(
        fs::canonicalize(&linked).unwrap(),
        fs::canonicalize(c.account("ffarmer").join(&rel)).unwrap()
    );
    let src = fs::symlink_metadata(c.account("ffarmer").join(&rel)).unwrap();
    assert_eq!(src.mode() & 0o777, 0o000, "source rollout mode changed");
    for dir in [
        "sessions",
        "sessions/2026",
        "sessions/2026/11",
        "sessions/2026/11/08",
    ] {
        let m = fs::metadata(c.account("ggrant").join(dir)).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o700, "{dir}");
    }

    let line = format!("codex resume session={SESSION} ffarmer->ggrant reason=room args=1");
    assert_eq!(resume_lines(&c), std::slice::from_ref(&line));
    assert!(text(&out.stderr).contains(&line), "{}", text(&out.stderr));
    assert!(!c.f.log().contains("continue please"), "extra args logged");
    for l in c.f.log().lines() {
        assert_log_grammar(l, true);
    }

    let state = c.state().unwrap();
    let r = &state["resumes"][0];
    assert_eq!(r["session"], SESSION);
    assert_eq!(
        (r["from"].as_str(), r["to"].as_str()),
        (Some("ffarmer"), Some("ggrant"))
    );
    assert!(r["at"].as_i64().unwrap() <= unix_now());
}

#[test]
fn from_home_defaults_to_codex_home_then_the_link() {
    let (c, _) = setup();
    let mut cmd = c.cmd(&["profile", "codex-resume", "--session", SESSION]);
    cmd.env("CODEX_HOME", c.account("ffarmer"));
    let out = c.run_cmd("codex-resume with CODEX_HOME", cmd);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(resume_lines(&c)[0].contains(" ffarmer->ggrant "));

    let (c, _) = setup();
    let out = c.resume(&["--session", SESSION]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        resume_lines(&c)[0].contains(" ffarmer->ggrant "),
        "via the link"
    );
}

#[test]
fn the_session_id_can_come_from_an_events_file() {
    let (c, _) = setup();
    let events = c.base.join("events.jsonl");
    fs::write(
        &events,
        format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{SESSION}\"}}\n\
             {{\"type\":\"turn.started\"}}\n\
             {{\"type\":\"turn.failed\",\"error\":{{\"message\":\"You\u{2019}ve hit your usage limit. Upgrade to Pro or try again later.\"}}}}\n"
        ),
    )
    .unwrap();
    let ev = events.display().to_string();
    let out = c.resume(&["--events", &ev, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(c.resumes()[0][1], format!("ARG={SESSION}"));
}

#[test]
fn a_session_configured_event_and_a_rate_limit_field_also_count() {
    let (c, _) = setup();
    let events = c.base.join("events.jsonl");
    fs::write(
        &events,
        format!(
            "{{\"id\":\"0\",\"msg\":{{\"type\":\"session_configured\",\"session_id\":\"{SESSION}\"}}}}\n\
             {{\"id\":\"1\",\"msg\":{{\"type\":\"token_count\",\"rate_limits\":{{\"primary\":{{\"used_percent\":100}},\"rate_limit_reached_type\":\"primary\"}}}}}}\n"
        ),
    )
    .unwrap();
    let ev = events.display().to_string();
    let out = c.resume(&["--events", &ev, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn session_beats_events() {
    let (c, _) = setup();
    let events = c.base.join("events.jsonl");
    fs::write(
        &events,
        "{\"type\":\"thread.started\",\"thread_id\":\"other-id\"}\n",
    )
    .unwrap();
    let ev = events.display().to_string();
    let out = c.resume(&[
        "--session",
        SESSION,
        "--events",
        &ev,
        "--from-home",
        &from(&c, "ffarmer"),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(c.resumes()[0][1], format!("ARG={SESSION}"));
}

#[test]
fn bad_events_files_are_usage_errors_and_start_nothing() {
    let (c, _) = setup();
    let ff = from(&c, "ffarmer");
    // No usage limit shown.
    let plain = c.base.join("plain.jsonl");
    fs::write(
        &plain,
        format!("{{\"type\":\"thread.started\",\"thread_id\":\"{SESSION}\"}}\n"),
    )
    .unwrap();
    // A limit but no id.
    let noid = c.base.join("noid.jsonl");
    fs::write(
        &noid,
        "{\"type\":\"error\",\"message\":\"usage_limit_reached\"}\n",
    )
    .unwrap();
    // A FIFO: must fail at once, never block on open.
    let fifo = c.base.join("events.fifo");
    mkfifo(&fifo);
    // A file named auth.json, and a file under the profiles root.
    let auth = c.base.join("auth.json");
    fs::write(
        &auth,
        format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{SESSION}\"}}\nusage_limit_reached\n"
        ),
    )
    .unwrap();
    let under_root = c.account("ggrant").join("events.jsonl");
    fs::write(
        &under_root,
        format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{SESSION}\"}}\nusage_limit_reached\n"
        ),
    )
    .unwrap();
    let missing = c.base.join("missing.jsonl");
    for p in [&plain, &noid, &fifo, &auth, &under_root, &missing, &c.base] {
        let ev = p.display().to_string();
        let out = c.resume(&["--events", &ev, "--from-home", &ff]);
        assert_eq!(out.status.code(), Some(2), "{ev}: {}", text(&out.stderr));
    }
    assert!(c.resumes().is_empty(), "codex resume ran");
    assert!(c.reads().is_empty(), "accounts read for a usage error");
}

#[test]
fn an_events_file_never_has_its_contents_logged() {
    let (c, _) = setup();
    let events = c.base.join("events.jsonl");
    fs::write(
        &events,
        format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{SESSION}\"}}\n\
             {{\"type\":\"item.completed\",\"text\":\"PRIVATE-EVENT-TEXT-2005\"}}\n\
             {{\"type\":\"error\",\"message\":\"You've hit your usage limit\"}}\n"
        ),
    )
    .unwrap();
    let ev = events.display().to_string();
    let out = c.resume(&["--events", &ev, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0));
    for s in [text(&out.stdout), text(&out.stderr), c.f.log()] {
        assert!(!s.contains("PRIVATE-EVENT-TEXT-2005"), "{s}");
    }
}

#[test]
fn invalid_session_ids_are_usage_errors() {
    let (c, _) = setup();
    let long = "a".repeat(65);
    for id in [
        "-x",
        "--yolo",
        "a/b",
        "..",
        "a b",
        "",
        long.as_str(),
        "abc;rm",
    ] {
        let arg = format!("--session={id}");
        let out = c.resume(&[&arg, "--from-home", &from(&c, "ffarmer")]);
        assert_eq!(out.status.code(), Some(2), "{id:?}: {}", text(&out.stderr));
    }
    let max = "a".repeat(64);
    let out = c.resume(&["--session", &max, "--from-home", &from(&c, "ffarmer")]);
    // Valid, but it has no rollout: status 1 (not a usage error).
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(c.resumes().is_empty());
}

#[test]
fn neither_session_nor_events_is_a_usage_error() {
    let (c, _) = setup();
    let out = c.resume(&["--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(c.resumes().is_empty());
}

#[test]
fn a_from_home_outside_the_root_is_a_usage_error() {
    let (c, _) = setup();
    let elsewhere = c.base.join("elsewhere");
    trapped(&elsewhere);
    for home in [elsewhere.clone(), c.root.clone(), c.base.join("nope")] {
        let h = home.display().to_string();
        let out = c.resume(&["--session", SESSION, "--from-home", &h]);
        assert_eq!(out.status.code(), Some(2), "{h}: {}", text(&out.stderr));
    }
    assert!(c.resumes().is_empty());
}

#[test]
fn three_resumes_in_a_day_cap_the_session() {
    let (c, _) = setup();
    let now = unix_now();
    let rec = |at: i64| serde_json::json!({"session": SESSION, "at": at, "from": "ffarmer", "to": "ggrant"});
    c.write_state(&serde_json::json!({
        "version": 1,
        "resumes": [rec(now - 100), rec(now - 200), rec(now - 300)]
    }));
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    let line = format!("codex resume none reason=capped session={SESSION}");
    assert_eq!(resume_lines(&c), std::slice::from_ref(&line));
    assert!(text(&out.stderr).contains(&line));
    assert!(c.resumes().is_empty(), "capped must not start codex");
    assert!(c.reads().is_empty(), "capped reads no account");
}

#[test]
fn records_older_than_a_day_do_not_count_and_are_pruned() {
    let (c, _) = setup();
    let old = unix_now() - 90_000;
    let rec = |at: i64| serde_json::json!({"session": SESSION, "at": at, "from": "ffarmer", "to": "ggrant"});
    c.write_state(
        &serde_json::json!({"version": 1, "resumes": [rec(old), rec(old - 1), rec(old - 2)]}),
    );
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let resumes = c.state().unwrap()["resumes"].as_array().unwrap().clone();
    assert_eq!(resumes.len(), 1, "old records pruned: {resumes:?}");
}

#[test]
fn the_fourth_resume_of_a_session_is_capped() {
    let (c, _) = setup();
    let ff = from(&c, "ffarmer");
    for _ in 0..3 {
        let out = c.resume(&["--session", SESSION, "--from-home", &ff]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let out = c.resume(&["--session", SESSION, "--from-home", &ff]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(c.resumes().len(), 3);
    // Another session is not capped.
    let other = "0199a213-0000-7800-8aa1-000000000000";
    c.rollout("ffarmer", other);
    let out = c.resume(&["--session", other, "--from-home", &ff]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn no_room_after_the_reset_exits_4_after_one_reread() {
    let (c, _) = setup();
    c.limited("ggrant", PAST_RESET + 60);
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    assert_eq!(
        resume_lines(&c),
        [format!(
            "codex resume none reason=no-room-after-reset session={SESSION}"
        )]
    );
    assert!(c.resumes().is_empty());
    // Exactly two reads of each account: before and after the one wait.
    for a in ["ffarmer", "ggrant", "hhale"] {
        assert_eq!(
            c.reads().iter().filter(|r| *r == a).count(),
            2,
            "{a}: {:?}",
            c.reads()
        );
    }
}

#[test]
fn after_the_reset_the_original_account_may_be_chosen() {
    let (c, rel) = setup();
    c.limited("ggrant", PAST_RESET + 60);
    // ffarmer has room again on its second read.
    c.reading_nth(
        "ffarmer",
        2,
        serde_json::json!({"primary": window(0, 300, RESET_AT), "rateLimitReachedType": null}),
    );
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        resume_lines(&c),
        [format!(
            "codex resume session={SESSION} ffarmer->ffarmer reason=after-reset args=0"
        )]
    );
    // No extra arguments: ARGC, the id, then CODEX_HOME.
    let call = &c.resumes()[0];
    assert_eq!(call[0], "ARGC=1");
    assert_eq!(call[2], format!("CODEX_HOME={}", from(&c, "ffarmer")));
    assert_eq!(call[5], format!("ROLLOUT=file {}", rel.display()));
}

#[test]
fn no_known_reset_exits_4_without_waiting() {
    let (c, _) = setup();
    let no_reset = serde_json::json!({
        "primary": {"usedPercent": 100, "windowDurationMins": 300},
        "rateLimitReachedType": "primary"
    });
    for a in ["ffarmer", "ggrant", "hhale"] {
        c.reading_json(a, no_reset.clone());
    }
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(
        resume_lines(&c),
        [format!(
            "codex resume none reason=no-reset-known session={SESSION}"
        )]
    );
    assert_eq!(c.reads().len(), 3, "one read each, no second read");
}

#[test]
fn the_childs_exit_status_is_passed_through() {
    let (c, _) = setup();
    let mut cmd = c.cmd(&[
        "profile",
        "codex-resume",
        "--session",
        SESSION,
        "--from-home",
    ]);
    cmd.arg(c.account("ffarmer")).env("FAKE_RESUME_EXIT", "7");
    let out = c.run_cmd("codex-resume with a failing child", cmd);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(c.resumes().len(), 1);
}

#[test]
fn no_rollout_found_exits_1_and_starts_nothing() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.limited("ffarmer", PAST_RESET);
    c.reading("ggrant", 10, 10);
    c.link_to("ffarmer");
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(c.resumes().is_empty());
}

#[test]
fn an_existing_rollout_under_the_target_is_not_relinked() {
    let (c, rel) = setup();
    let existing = c
        .account("ggrant")
        .join("sessions/2026/11/07/rollout-x-".to_string() + SESSION + ".jsonl");
    fs::create_dir_all(existing.parent().unwrap()).unwrap();
    fs::write(&existing, "").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o000)).unwrap();
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(fs::symlink_metadata(c.account("ggrant").join(&rel)).is_err());
}

#[test]
fn a_held_lock_exits_1_with_lock_busy() {
    let (c, _) = setup();
    let dir = c.f.rotate_dir();
    fs::create_dir_all(&dir).unwrap();
    for d in [c.f.root.join(".auto-switch.d"), dir.clone()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let lock = dir.join("lock");
    fs::write(&lock, "").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    let holder = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    // SAFETY: a valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    drop(holder);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        !c.reads().is_empty(),
        "accounts are read with no lock held, before the lock is tried"
    );
    assert!(c.state().is_none(), "no record without the lock");
    assert!(
        text(&out.stderr).contains(&format!(
            "codex resume none reason=lock-busy session={SESSION}"
        )),
        "{}",
        text(&out.stderr)
    );
    assert!(c.resumes().is_empty());
}

#[test]
fn an_unreadable_state_file_exits_1_and_is_kept() {
    let (c, _) = setup();
    c.write_state(&serde_json::json!({}));
    fs::write(c.state_path(), "{broken").unwrap();
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::read_to_string(c.state_path()).unwrap(), "{broken");
    assert!(c.resumes().is_empty());
}

#[test]
fn codex_resume_takes_no_shell_and_keeps_args_in_order() {
    let (c, _) = setup();
    let out = c.resume(&[
        "--session",
        SESSION,
        "--from-home",
        &from(&c, "ffarmer"),
        "--",
        "$(touch /nonexistent-2005)",
        "--yolo",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let call = &c.resumes()[0];
    assert_eq!(call[0], "ARGC=3");
    assert_eq!(call[1], format!("ARG={SESSION}"));
    assert_eq!(call[2], "ARG=$(touch /nonexistent-2005)");
    assert_eq!(call[3], "ARG=--yolo");
    assert!(resume_lines(&c)[0].ends_with(" args=2"));
}

#[test]
fn the_link_and_the_rotate_state_are_unchanged_by_a_resume() {
    let (c, _) = setup();
    let inodes = c.account_inodes();
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        c.live().as_deref(),
        Some("ffarmer"),
        "resume never moves the link"
    );
    assert_eq!(c.account_inodes(), inodes);
    assert!(
        c.state()
            .unwrap()
            .get("journal")
            .is_none_or(|j| j.is_null())
    );
    assert!(!Path::new("/nonexistent-2005").exists());
}

#[test]
fn a_codex_program_under_the_profiles_root_is_refused_with_status_2() {
    // EXEC-1 / S3: the program would be chosen by an account.
    let (c, _) = setup();
    let inside = c.root.join("tools").join("codex");
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    crate::support::exec_stub::copy_executable(&c.bin, &inside);
    let mut cmd = c.cmd(&[
        "profile",
        "codex-resume",
        "--session",
        SESSION,
        "--from-home",
    ]);
    cmd.arg(c.account("ffarmer")).env("CODEX_BIN", &inside);
    let out = c.run_cmd("codex-resume with codex under the root", cmd);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("codex-resume: refused reason=codex-bin-in-home"),
        "{}",
        text(&out.stderr)
    );
    assert!(c.resumes().is_empty(), "the child never ran");
    assert!(c.reads().is_empty(), "no account was read");
    let lines = resume_lines(&c);
    assert_eq!(
        lines,
        [format!(
            "codex resume none reason=codex-bin-in-home session={SESSION}"
        )]
    );
    for l in &lines {
        assert_log_grammar(l, false);
    }
}

#[test]
fn a_rollout_that_is_a_planted_symlink_is_not_resumed() {
    // FS-6 / S2: `x-<id>.jsonl -> ../../auth.json` is not a rollout.
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.limited("ffarmer", PAST_RESET);
    c.reading("ggrant", 10, 10);
    c.link_to("ffarmer");
    let sessions = c.account("ffarmer").join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    std::os::unix::fs::symlink("../auth.json", sessions.join(format!("x-{SESSION}.jsonl")))
        .unwrap();
    let out = c.resume(&["--session", SESSION, "--from-home", &from(&c, "ffarmer")]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(c.resumes().is_empty());
    let planted = c
        .account("ggrant")
        .join(format!("sessions/x-{SESSION}.jsonl"));
    assert!(fs::symlink_metadata(planted).is_err(), "nothing linked");
}
