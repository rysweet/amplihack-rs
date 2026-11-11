//! Unit tests for `launch.rs`: the pane argv (step 5b section 7, S16), the
//! `env -u` list (design 4.13, D6, A3), the constant resume message (R5.7,
//! S23), and the amplihack executable path (D22, S16).
//!
//! API under test:
//! - `launch::pane_unset() -> Vec<&'static str>`
//! - `launch::message(lost_at: &str, now: &str) -> String`
//! - `launch::pane_command(name: &str, e: &Entry, exe: &Path, msg: &str) -> Vec<String>`
//! - `launch::check_exe(p: &Path) -> Result<(), String>`
//! - `launch::resolve_exe(raw: &Path) -> Result<PathBuf, String>`: drops a
//!   trailing ` (deleted)` when the plain path exists, then `check_exe`
//! - `registry::Entry` (deserialised from its JSON form), `env_record::ALLOWLIST`

use super::env_record::ALLOWLIST;
use super::launch::*;
use super::registry::Entry;
use amplihack_utils::agent_binary::SESSION_MARKERS;
use serde_json::json;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const ID: &str = "0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c";
const EXE: &str = "/home/alex/.local/bin/amplihack";
const EXE_REFUSED: &str = "the amplihack binary path must be absolute and UTF-8, without '=' or control characters, for env to run it; nothing was started";

fn entry(extra: serde_json::Value) -> Entry {
    let mut e = json!({
        "state": "active",
        "tmux_session": "coord",
        "tmux_window": "main",
        "launch_dir": "/home/alex/src/project",
        "conversation_id": ID,
        "registered_at": "2026-10-30T09:12:04Z",
        "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d"
    });
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    serde_json::from_value(e).expect("a valid entry")
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// `env`, then `-u NAME` for each `pane_unset()` name.
fn env_prefix() -> Vec<String> {
    let mut v = s(&["env"]);
    for n in pane_unset() {
        v.extend(s(&["-u", n]));
    }
    v
}

#[test]
fn pane_unset_is_the_markers_then_the_launcher_names() {
    let mut want: Vec<&str> = Vec::new();
    let extra = [
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_PROJECT_DIR",
        "AMPLIHACK_SESSION_ID",
        "AMPLIHACK_DEPTH",
        "AMPLIHACK_SESSION_DEPTH",
        "AMPLIHACK_TREE_ID",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE",
    ];
    for n in SESSION_MARKERS.iter().map(|(n, _)| *n).chain(extra) {
        if !want.contains(&n) {
            want.push(n);
        }
    }
    assert_eq!(pane_unset(), want);
    for kept in ["TMUX", "TMUX_PANE", "HOME", "PATH"] {
        assert!(!pane_unset().contains(&kept), "{kept} must reach the pane");
    }
}

#[test]
fn the_resume_message_is_constant() {
    let m = message("2026-10-30T10:01:09Z", "2026-10-30T10:02:11Z");
    assert_eq!(
        m,
        "amplihack agent supervisor: the terminal session for this agent was lost at \
         2026-10-30T10:01:09Z. This conversation was resumed at 2026-10-30T10:02:11Z. Before \
         continuing, check what your interrupted step had already done (git status, open pull \
         requests, running processes) and do not repeat finished steps."
    );
    assert!(!m.starts_with('-'));
}

#[test]
fn the_full_pane_argv_in_order() {
    let e = entry(json!({
        "model": "opus", "effort": "high",
        "add_dirs": ["/home/alex/src/docs", "/srv/a b"],
        "skip_permissions": true,
        "amplihack_flags": { "subprocess_safe": true, "no_reflection": true, "profile": "work" },
        "env": { "MAX_THINKING_TOKENS": "8000", "CLAUDE_CODE_EFFORT_LEVEL": "high" }
    }));
    let mut want = env_prefix();
    want.extend(s(&[
        "CLAUDE_CODE_EFFORT_LEVEL=high",
        "MAX_THINKING_TOKENS=8000",
        EXE,
        "claude",
        "--subprocess-safe",
        "--no-reflection",
        "--profile",
        "work",
        "--supervise",
        "coordinator",
        "--",
        "--resume",
        ID,
        "MSG with spaces",
        "--model",
        "opus",
        "--effort",
        "high",
        "--add-dir",
        "/home/alex/src/docs",
        "--add-dir",
        "/srv/a b",
        "--dangerously-skip-permissions",
    ]));
    assert_eq!(
        pane_command("coordinator", &e, Path::new(EXE), "MSG with spaces"),
        want
    );
}

#[test]
fn the_pane_argv_with_nothing_optional() {
    let e = entry(json!({}));
    let mut want = env_prefix();
    want.extend(s(&[
        EXE,
        "claude",
        "--supervise",
        "a",
        "--",
        "--resume",
        ID,
        "M",
    ]));
    assert_eq!(pane_command("a", &e, Path::new(EXE), "M"), want);
}

#[test]
fn a_permission_mode_wins_over_skip_permissions() {
    let e = entry(json!({ "permission_mode": "acceptEdits", "skip_permissions": true }));
    let argv = pane_command("a", &e, Path::new(EXE), "M");
    assert_eq!(
        argv[argv.len() - 2..],
        s(&["--permission-mode", "acceptEdits"])
    );
    assert!(!argv.iter().any(|a| a == "--dangerously-skip-permissions"));
}

#[test]
fn the_message_comes_before_add_dir() {
    let e = entry(json!({ "add_dirs": ["/x"] }));
    let argv = pane_command("a", &e, Path::new(EXE), "M");
    let msg = argv.iter().position(|a| a == "M").unwrap();
    let add = argv.iter().position(|a| a == "--add-dir").unwrap();
    assert!(msg < add, "--add-dir would swallow the message: {argv:?}");
}

#[test]
fn the_argv_cannot_be_run_by_a_shell_or_read_as_flags() {
    let e = entry(json!({
        "model": "m", "effort": "e", "add_dirs": ["/x"], "permission_mode": "plan",
        "amplihack_flags": { "profile": "p" }, "env": { "ANTHROPIC_MODEL": "opus" }
    }));
    let argv = pane_command("a", &e, Path::new(EXE), "M");
    assert!(argv.len() > 1, "a single argument would go through sh -c");
    assert_eq!(argv[0], "env");
    let takes_value = [
        "--resume",
        "--model",
        "--effort",
        "--add-dir",
        "--permission-mode",
        "--profile",
        "--supervise",
    ];
    for (i, a) in argv.iter().enumerate() {
        if takes_value.contains(&a.as_str()) {
            let v = &argv[i + 1];
            assert!(!v.starts_with('-'), "{a} {v}");
        }
    }
    let exe_at = argv.iter().position(|a| a == EXE).unwrap();
    let mut i = 1;
    while i < exe_at {
        if argv[i] == "-u" {
            assert!(
                pane_unset().contains(&argv[i + 1].as_str()),
                "{}",
                argv[i + 1]
            );
            i += 2;
        } else {
            let (k, _) = argv[i]
                .split_once('=')
                .expect("env gets only -u and KEY=VALUE");
            assert!(ALLOWLIST.contains(&k), "{k}");
            i += 1;
        }
    }
    assert!(!argv.iter().any(|a| a == "sh" || a == "-c" || a == "bash"));
}

#[test]
fn the_binary_path_rules() {
    assert_eq!(check_exe(Path::new(EXE)), Ok(()));
    for bad in [
        Path::new("amplihack"),
        Path::new("bin/amplihack"),
        Path::new("/opt/a=b/amplihack"),
        Path::new("/opt/a\nb/amplihack"),
        Path::new("/opt/a\u{7f}b/amplihack"),
        Path::new(OsStr::from_bytes(b"/opt/\xff/amplihack")),
    ] {
        assert_eq!(check_exe(bad), Err(EXE_REFUSED.to_string()), "{bad:?}");
    }
}

#[test]
fn a_replaced_executable_loses_its_deleted_suffix() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().canonicalize().unwrap().join("amplihack");
    std::fs::write(&plain, "").unwrap();
    let deleted = PathBuf::from(format!("{} (deleted)", plain.display()));
    assert_eq!(resolve_exe(&deleted), Ok(plain.clone()));
    assert_eq!(resolve_exe(&plain), Ok(plain.clone()));
    let gone = tmp.path().canonicalize().unwrap().join("gone (deleted)");
    assert_eq!(
        resolve_exe(&gone),
        Ok(gone.clone()),
        "kept when the plain path is missing"
    );
    let eq = tmp.path().canonicalize().unwrap().join("a=b");
    std::fs::create_dir(&eq).unwrap();
    std::fs::write(eq.join("amplihack"), "").unwrap();
    let raw = PathBuf::from(format!("{} (deleted)", eq.join("amplihack").display()));
    assert_eq!(resolve_exe(&raw), Err(EXE_REFUSED.to_string()));
}
