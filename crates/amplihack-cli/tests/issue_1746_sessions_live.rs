//! Issue #1746: a running session is never archived. The main safety
//! property of `sessions archive`.
//!
//! The live set is read from `~/.claude/sessions`, from every profile's own
//! `sessions/` (profiles are not symlinked there) and, on Linux, from
//! `<CLAUDE_CONFIG_DIR>/sessions` of this user's processes. A session is
//! live when the pid in its pid file, or the file's numeric stem, is alive.
//! The check fails closed. Contract: docs/reference/sessions-command.md
//! (Live sessions, Messages).

#![cfg(unix)]

use crate::support::sessions_archive::*;
use std::fs;
use std::process::{Child, Command, Stdio};

fn applied(n: u32, live: u32) -> String {
    format!(
        "archived {n} sessions, 0.00 GiB (older than 3 days; {live} live sessions kept; 0 failed)"
    )
}

fn alive_pid() -> u32 {
    std::process::id()
}

/// A `sleep` child that is killed on drop.
struct Sleeper(Child);
impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn body(id: &str, pid: i64) -> String {
    format!(r#"{{"pid":{pid},"sessionId":"{id}","cwd":"/x","startedAt":1}}"#)
}

#[test]
fn session_live_in_a_profile_sessions_dir_is_kept() {
    let fx = Fx::new();
    let jsonl = fx.session("live-1", 10, &["sub/a.txt"]);
    fx.session("dead-1", 10, &[]);
    let work = fx.profile("work");
    let pid = alive_pid();
    fx.pid_file(&work, &pid.to_string(), &body("live-1", i64::from(pid)));
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(1, 1));
    assert!(jsonl.is_file(), "live session archived");
    assert!(fx.project_dir().join("live-1/sub/a.txt").is_file());
    assert!(!fx.project_dir().join("dead-1.jsonl").exists());
}

#[test]
fn session_live_in_home_claude_sessions_is_kept() {
    let fx = Fx::new();
    let jsonl = fx.session("live-1", 10, &[]);
    let pid = alive_pid();
    fx.pid_file(
        &fx.home.join(".claude"),
        &pid.to_string(),
        &body("live-1", i64::from(pid)),
    );
    let out = fx.archive(&[]);
    ok(&out);
    assert!(
        text(&out.stdout).contains("would archive 0 sessions")
            && text(&out.stdout).contains("1 live sessions kept"),
        "{}",
        text(&out.stdout)
    );
    ok(&fx.apply());
    assert!(jsonl.is_file());
}

#[test]
fn live_counts_only_sessions_that_were_old_enough() {
    let fx = Fx::new();
    fx.session("new-live", 1, &[]);
    let pid = alive_pid();
    fx.pid_file(
        &fx.home.join(".claude"),
        &pid.to_string(),
        &body("new-live", i64::from(pid)),
    );
    let out = fx.apply();
    assert_stdout(&out, &applied(0, 0));
}

#[test]
fn live_session_is_matched_across_every_project() {
    let fx = Fx::new();
    let a = fx.session_in("-proj-a", "same", 10, &[]);
    let other = fx.session_in("-proj-b", "other", 10, &[]);
    let pid = alive_pid();
    fx.pid_file(
        &fx.profile("spare"),
        &pid.to_string(),
        &body("same", i64::from(pid)),
    );
    ok(&fx.apply());
    assert!(a.is_file());
    assert!(!other.exists());
}

#[test]
fn dead_pid_protects_nothing() {
    let fx = Fx::new();
    let jsonl = fx.session("gone", 10, &[]);
    let pid = dead_pid();
    fx.pid_file(&fx.profile("work"), &pid.to_string(), &body("gone", pid));
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(1, 0));
    assert!(!jsonl.exists());
}

#[test]
fn either_pid_alive_keeps_the_session() {
    let fx = Fx::new();
    let a = fx.session("stem-alive", 10, &[]);
    let b = fx.session("body-alive", 10, &[]);
    let work = fx.profile("work");
    let dead = dead_pid();
    let live = i64::from(alive_pid());
    // Stem alive, body dead.
    fx.pid_file(&work, &live.to_string(), &body("stem-alive", dead));
    // Stem dead, body alive.
    fx.pid_file(
        &fx.profile("spare"),
        &dead.to_string(),
        &body("body-alive", live),
    );
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(0, 2));
    assert!(a.is_file() && b.is_file());
}

#[test]
fn pid_zero_or_negative_protects_nothing_and_does_not_crash() {
    let fx = Fx::new();
    let work = fx.profile("work");
    let mut paths = Vec::new();
    for (i, pid) in [0_i64, -1, -2, 2_147_483_648, i64::MIN]
        .into_iter()
        .enumerate()
    {
        let id = format!("odd-{i}");
        paths.push(fx.session(&id, 10, &[]));
        // A non-numeric stem, so only the body pid counts.
        fx.pid_file(&work, &format!("x{i}"), &body(&id, pid));
    }
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(5, 0));
    for p in paths {
        assert!(!p.exists());
    }
    // This test process (and its group) is still here: kill(0|-1, 0) was
    // never turned into a real signal.
}

#[test]
fn unparseable_or_oversized_pid_file_protects_nothing() {
    let fx = Fx::new();
    let a = fx.session("garbled", 10, &[]);
    let b = fx.session("huge", 10, &[]);
    let work = fx.profile("work");
    let notes = fx.pid_file(&work, "notes", &format!("not json {BODY_MARKER}"));
    let live = alive_pid();
    // Over 64 KiB: not read, even though its pid is alive.
    let big = format!(
        r#"{{"pid":{live},"sessionId":"huge","pad":"{}"}}"#,
        "x".repeat(70 * 1024)
    );
    fx.pid_file(&work, "big", &big);
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(2, 0));
    // B.5: the unparseable file is named once; the oversized one is
    // refused unread and passed over without a line.
    let err = text(&out.stderr);
    let want = format!("ignored unreadable pid file {}", notes.display());
    assert_eq!(err.lines().collect::<Vec<_>>(), [want.as_str()], "{err}");
    assert!(!a.exists() && !b.exists());
}

#[test]
fn symlinked_pid_file_is_not_followed() {
    let fx = Fx::new();
    let jsonl = fx.session("via-link", 10, &[]);
    let live = alive_pid();
    let target = fx.base.join("target.json");
    fs::write(&target, body("via-link", i64::from(live))).unwrap();
    let sdir = fx.profile("work").join("sessions");
    fs::create_dir_all(&sdir).unwrap();
    std::os::unix::fs::symlink(&target, sdir.join("link.json")).unwrap();
    ok(&fx.apply());
    assert!(!jsonl.exists(), "a symlinked pid file counted");
}

#[test]
fn unreadable_profile_sessions_dir_fails_closed() {
    if running_as_root() {
        return;
    }
    let fx = Fx::new();
    let jsonl = fx.session("aaaa", 10, &[]);
    let sdir = fx.profile("work").join("sessions");
    fs::create_dir_all(&sdir).unwrap();
    chmod(&sdir, 0o000);
    let out = fx.apply();
    chmod(&sdir, 0o700);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "cannot read live sessions in {} (permission denied); nothing archived",
            sdir.display()
        ),
    );
    assert!(
        text(&out.stdout).is_empty(),
        "stdout: {}",
        text(&out.stdout)
    );
    assert!(jsonl.is_file());
    assert!(!fx.manifest().exists());
}

#[test]
fn unreadable_profile_root_fails_closed() {
    if running_as_root() {
        return;
    }
    let fx = Fx::new();
    let jsonl = fx.session("aaaa", 10, &[]);
    fx.profile("work");
    chmod(&fx.profile_root, 0o000);
    let out = fx.apply();
    chmod(&fx.profile_root, 0o700);
    assert_eq!(exit_code(&out), 1);
    let err = text(&out.stderr);
    assert!(
        err.lines()
            .any(|l| l.starts_with("cannot read live sessions in ")
                && l.ends_with("(permission denied); nothing archived")),
        "{err}"
    );
    assert!(jsonl.is_file());
}

#[test]
fn unreadable_home_sessions_dir_fails_closed_even_for_a_dry_run() {
    if running_as_root() {
        return;
    }
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let sdir = fx.home.join(".claude/sessions");
    fs::create_dir_all(&sdir).unwrap();
    chmod(&sdir, 0o000);
    let out = fx.archive(&[]);
    chmod(&sdir, 0o700);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "cannot read live sessions in {} (permission denied); nothing archived",
            sdir.display()
        ),
    );
}

/// Linux: a session run with a `CLAUDE_CONFIG_DIR` that is neither
/// `~/.claude` nor a profile is found through `/proc/<pid>/environ`.
#[cfg(target_os = "linux")]
#[test]
fn session_live_under_a_running_process_config_dir_is_kept() {
    let fx = Fx::new();
    let jsonl = fx.session("proc-live", 10, &[]);
    let cfg = fx.base.join("other-config");
    fs::create_dir_all(&cfg).unwrap();
    // Long enough to outlive a first-run build of the binary, which
    // `fx.apply()` triggers; the child is killed on drop.
    let child = Sleeper(
        Command::new("sleep")
            .arg("3600")
            .env_clear()
            .env("CLAUDE_CONFIG_DIR", &cfg)
            .env("SOME_TOKEN", ENVIRON_MARKER)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = i64::from(child.0.id());
    fx.pid_file(&cfg, &pid.to_string(), &body("proc-live", pid));
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(0, 1));
    assert!(
        jsonl.is_file(),
        "session of a running CLAUDE_CONFIG_DIR archived"
    );
    drop(child);
}

#[test]
fn live_session_is_kept_by_the_dry_run_count_too() {
    let fx = Fx::new();
    fx.session("live-1", 10, &[]);
    fx.session("old-1", 10, &[]);
    let pid = alive_pid();
    fx.pid_file(
        &fx.profile("work"),
        &pid.to_string(),
        &body("live-1", i64::from(pid)),
    );
    let out = fx.archive(&[]);
    ok(&out);
    assert_stdout(
        &out,
        "would archive 1 sessions, 0.00 GiB (older than 3 days; 1 live sessions kept); dry run, nothing changed (use --apply)",
    );
}

// ------------------------------------------- registered supervised agents
//
// `amplihack agent` (#1760) resumes a conversation by id after its process
// has gone, so a pid file is not the only sign that a transcript is still
// needed. The conversation of every registered agent that is not `ended`
// counts as live; an `ended` one is archived like any other session.

/// One registry entry in `state` for conversation `id`.
fn agent_entry(state: &str, id: &str) -> serde_json::Value {
    serde_json::json!({
        "state": state, "tmux_session": "coord", "tmux_window": "main",
        "launch_dir": "/srv/project", "conversation_id": id,
        "registered_at": "2026-10-30T09:12:04Z", "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d", "restarts": []
    })
}

/// Writes `~/.amplihack/agents/registry.json` (0600, in a 0700 directory)
/// holding `agents`, and returns its path.
fn registry(fx: &Fx, agents: &[(&str, serde_json::Value)]) -> std::path::PathBuf {
    let top = fx.home.join(".amplihack");
    let dir = top.join("agents");
    fs::create_dir_all(&dir).unwrap();
    chmod(&top, 0o700);
    chmod(&dir, 0o700);
    let map: serde_json::Map<String, serde_json::Value> = agents
        .iter()
        .map(|(n, e)| ((*n).to_string(), e.clone()))
        .collect();
    let path = dir.join("registry.json");
    fs::write(
        &path,
        serde_json::json!({ "version": 1, "agents": map }).to_string(),
    )
    .unwrap();
    chmod(&path, 0o600);
    path
}

#[test]
fn stopped_and_active_agents_are_kept_and_an_ended_one_is_archived() {
    let fx = Fx::new();
    let stopped = fx.session("conv-stopped", 10, &["sub/a.txt"]);
    let active = fx.session("conv-active", 10, &[]);
    let ended = fx.session("conv-ended", 10, &[]);
    registry(
        &fx,
        &[
            ("halted", agent_entry("stopped", "conv-stopped")),
            ("waiting", agent_entry("active", "conv-active")),
            ("done", agent_entry("ended", "conv-ended")),
        ],
    );
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(1, 2));
    assert!(
        stopped.is_file(),
        "a stopped agent's transcript was archived"
    );
    assert!(fx.project_dir().join("conv-stopped/sub/a.txt").is_file());
    assert!(
        active.is_file(),
        "an active agent's transcript was archived"
    );
    assert!(!ended.exists(), "an ended agent's transcript was kept");
}

#[test]
fn a_registered_agent_is_kept_by_the_dry_run_count_too() {
    let fx = Fx::new();
    fx.session("conv-stopped", 10, &[]);
    registry(&fx, &[("halted", agent_entry("stopped", "conv-stopped"))]);
    let out = fx.archive(&[]);
    ok(&out);
    assert!(
        text(&out.stdout).contains("would archive 0 sessions")
            && text(&out.stdout).contains("1 live sessions kept"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn an_unreadable_agent_registry_fails_closed() {
    let fx = Fx::new();
    let jsonl = fx.session("conv-stopped", 10, &[]);
    let path = registry(&fx, &[("halted", agent_entry("stopped", "conv-stopped"))]);
    fs::write(&path, b"not json").unwrap();
    let out = fx.apply();
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "cannot read live sessions in {} (not valid JSON (line 1, column 2); it was not changed); nothing archived",
            path.display()
        ),
    );
    assert!(jsonl.is_file());
    assert!(!fx.manifest().exists());
}

#[test]
fn an_agent_directory_that_is_not_private_fails_closed() {
    let fx = Fx::new();
    let jsonl = fx.session("conv-stopped", 10, &[]);
    registry(&fx, &[("halted", agent_entry("stopped", "conv-stopped"))]);
    let dir = fx.home.join(".amplihack/agents");
    chmod(&dir, 0o777);
    let out = fx.apply();
    chmod(&dir, 0o700);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "cannot read live sessions in {} (is not private (owner-only mode 0700)); nothing archived",
            dir.display()
        ),
    );
    assert!(jsonl.is_file());
}

/// Moves `~/.amplihack` to another directory and leaves a symlink to it in
/// its place, as a user does to put it on another disk.
fn symlink_amplihack_home(fx: &Fx) {
    let real = fx.base.join("elsewhere");
    let top = fx.home.join(".amplihack");
    if top.exists() {
        fs::rename(&top, &real).unwrap();
    } else {
        fs::create_dir_all(&real).unwrap();
    }
    chmod(&real, 0o700);
    std::os::unix::fs::symlink(&real, &top).unwrap();
}

#[test]
fn a_symlinked_amplihack_home_without_agents_does_not_stop_the_archive() {
    let fx = Fx::new();
    let jsonl = fx.session("old-1", 10, &[]);
    symlink_amplihack_home(&fx);
    let out = fx.archive(&[]);
    ok(&out);
    assert!(
        text(&out.stdout).contains("would archive 1 sessions"),
        "{}",
        text(&out.stdout)
    );
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(1, 0));
    assert!(!jsonl.exists());
}

#[test]
fn a_symlinked_amplihack_home_with_an_empty_agents_directory_is_archived() {
    let fx = Fx::new();
    let jsonl = fx.session("old-1", 10, &[]);
    let dir = fx.home.join(".amplihack/agents");
    fs::create_dir_all(&dir).unwrap();
    chmod(&dir, 0o700);
    symlink_amplihack_home(&fx);
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied(1, 0));
    assert!(!jsonl.exists());
}

#[test]
fn a_symlinked_amplihack_home_holding_a_registry_fails_closed_with_the_reason() {
    let fx = Fx::new();
    let jsonl = fx.session("conv-stopped", 10, &[]);
    registry(&fx, &[("halted", agent_entry("stopped", "conv-stopped"))]);
    symlink_amplihack_home(&fx);
    let out = fx.apply();
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "cannot read live sessions in {} (is a symlink); nothing archived",
            fx.home.join(".amplihack").display()
        ),
    );
    assert!(jsonl.is_file());
    assert!(!fx.manifest().exists());
}
