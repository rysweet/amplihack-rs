//! Issue #1710 (D11): `usage_cmd::probe_rows`, the in-process path the
//! `workstreams status` logins section reads. It probes like `profile usage`
//! but never records or persists anything, and stops when its cancel flag is
//! set.
//!
//! Contract: `probe_rows(store, binary, names, timeout, cancel) ->
//! Result<Vec<Row>>`, one row per selected profile (every profile when
//! `names` is empty). `profile usage` keeps passing `&PROBE_CANCEL`; its own
//! tests in `tests_usage_cmd.rs` are unchanged.

#![cfg(unix)]

use super::name::ProfileName;
use super::store::ProfileStore;
use super::usage_cmd::{
    REFUSED_NO_STORE, REFUSED_UNREADABLE, listed_profiles, probe_profiles_and_home, probe_rows,
};
use super::usage_parse::UsageState;
use crate::binary_finder::BinaryInfo;
use crate::test_support::{EnvGuard, HomeGuard, env_lock};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The shared environment lock, Docker mode off and HOME at `home`, so the
/// probe neither sees another test's environment nor reads the real home.
fn isolated(home: &Path) -> (std::sync::MutexGuard<'static, ()>, EnvGuard, HomeGuard) {
    let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    (
        lock,
        EnvGuard::unset(["AMPLIHACK_USE_DOCKER"]),
        HomeGuard::set(home),
    )
}

fn fake_claude(dir: &Path, body: &str) -> BinaryInfo {
    let p = dir.join("claude");
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    BinaryInfo {
        name: "claude".into(),
        path: p,
        version: None,
    }
}

fn store_with(root: &Path, names: &[&str]) -> ProfileStore {
    let s = ProfileStore::new(root.to_path_buf());
    for n in names {
        s.add(&ProfileName::parse(n).unwrap()).unwrap();
    }
    s
}

/// Every path under `root` with its contents (empty for directories).
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            let meta = fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                stack.push(p.clone());
                out.insert(p, Vec::new());
            } else {
                out.insert(p.clone(), fs::read(&p).unwrap_or_default());
            }
        }
    }
    out
}

#[test]
fn an_empty_store_gives_no_rows() {
    let d = tempfile::tempdir().unwrap();
    let _env = isolated(d.path());
    let store = store_with(&d.path().join("root"), &[]);
    let bin = fake_claude(d.path(), "exit 0");
    let cancel = AtomicBool::new(false);
    let rows = probe_rows(&store, &bin, &[], Duration::from_secs(5), &cancel).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn probe_rows_gives_one_row_per_profile_and_leaves_the_store_untouched() {
    let d = tempfile::tempdir().unwrap();
    let _env = isolated(d.path());
    let root = d.path().join("root");
    let store = store_with(&root, &["work", "spare"]);
    let bin = fake_claude(d.path(), "echo 'nothing parseable'");
    let before = tree(&root);
    let cancel = AtomicBool::new(false);

    let rows = probe_rows(&store, &bin, &[], Duration::from_secs(10), &cancel).unwrap();

    let mut names: Vec<&str> = rows.iter().map(|r| r.profile.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["spare", "work"]);
    assert!(
        rows.iter().all(|r| r.report.state == UsageState::Unparsed),
        "{rows:?}"
    );
    assert_eq!(tree(&root), before, "probe_rows must not record or persist");
}

#[test]
fn named_profiles_limit_the_rows() {
    let d = tempfile::tempdir().unwrap();
    let _env = isolated(d.path());
    let store = store_with(&d.path().join("root"), &["work", "spare"]);
    let bin = fake_claude(d.path(), "exit 0");
    let cancel = AtomicBool::new(false);
    let rows = probe_rows(
        &store,
        &bin,
        &["work".to_string()],
        Duration::from_secs(10),
        &cancel,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].profile, "work");
}

#[test]
fn setting_the_cancel_flag_stops_a_stalled_probe_promptly() {
    let d = tempfile::tempdir().unwrap();
    let _env = isolated(d.path());
    let store = store_with(&d.path().join("root"), &["slow"]);
    let pid_file = d.path().join("pid");
    let bin = fake_claude(
        d.path(),
        &format!("echo $$ > '{}'\nexec sleep 60", pid_file.display()),
    );
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&cancel);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::SeqCst);
    });

    let start = Instant::now();
    let rows = probe_rows(&store, &bin, &[], Duration::from_secs(60), &cancel).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
    assert_eq!(rows.len(), 1);
    assert_ne!(rows[0].report.state, UsageState::Ok);

    // The probe's process group was killed, not left behind.
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let pid: i32 = pid.trim().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only checks for existence.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            state.is_empty() || state.contains(") Z"),
            "the probe outlived its cancel: {state}"
        );
    }
}

/// Review of #1710: the refusal `workstreams status` shows comes from a
/// fixed set, even when the store's own error names a path.
#[test]
fn a_directory_with_an_invalid_name_does_not_hide_the_other_profiles() {
    let d = tempfile::tempdir().unwrap();
    let _env = isolated(d.path());
    let root = d.path().join("profiles");
    store_with(&root, &["alpha", "beta"]);
    fs::create_dir(root.join("not a profile!")).unwrap();
    let _root = EnvGuard::set([("AMPLIHACK_CLAUDE_PROFILE_ROOT", root.to_str().unwrap())]);
    let names: Vec<String> = listed_profiles()
        .unwrap()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["alpha", "beta"]);
}

#[test]
fn a_store_error_naming_a_path_is_refused_with_fixed_text() {
    let d = tempfile::tempdir().unwrap();
    let _env = isolated(d.path());
    let real = d.path().join("real-root");
    fs::create_dir(&real).unwrap();
    let link = d.path().join("linked-root");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let _root = EnvGuard::set([("AMPLIHACK_CLAUDE_PROFILE_ROOT", link.to_str().unwrap())]);
    let refused = listed_profiles().unwrap_err();
    assert_eq!(refused, REFUSED_UNREADABLE);
    assert!(!refused.contains('/'), "{refused}");
    drop(_root);

    let _root = EnvGuard::set([("AMPLIHACK_CLAUDE_PROFILE_ROOT", "relative/root")]);
    let refused = listed_profiles().unwrap_err();
    assert_eq!(refused, REFUSED_NO_STORE);
    assert!(!refused.contains('/'), "{refused}");
}

/// A file only this user can read, its directory created.
fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn the_home_row_is_kept_when_names_narrow_the_profiles() {
    // `workstreams status` names the plan profiles when another profile uses
    // an API key; the login in `~/.claude` that no profile holds must still
    // get its row.
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let _env = isolated(&home);
    let root = d.path().join("root");
    let store = store_with(&root, &["a", "b"]);
    let account = |uuid: &str| {
        serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": "x@example.com"}})
            .to_string()
    };
    private(&home.join(".claude/.credentials.json"), "{}");
    private(&home.join(".claude.json"), &account("u1"));
    private(&root.join("a/.claude.json"), &account("u-a"));
    let ledger_dir = root.join(".auto-switch.d/rotate");
    fs::create_dir_all(&ledger_dir).unwrap();
    for dir in [ledger_dir.parent().unwrap(), ledger_dir.as_path()] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(
        &ledger_dir.join("ledger.json"),
        r#"{"version":1,"live":"a","account":"u-a","since":10}"#,
    );
    let bin = fake_claude(d.path(), "exit 0");
    let cancel = AtomicBool::new(false);

    let rows = probe_rows(
        &store,
        &bin,
        &["b".to_string()],
        Duration::from_secs(10),
        &cancel,
    )
    .unwrap();

    assert_eq!(
        rows.iter().filter(|r| r.live_in_home).count(),
        1,
        "{rows:?}"
    );
    assert_eq!(
        rows.iter().filter(|r| !r.live_in_home).count(),
        1,
        "{rows:?}"
    );
}

#[test]
fn the_home_row_alone_is_probed_when_no_profile_is() {
    // `workstreams status` with every profile on an API key probes no plan
    // profile, and the `~/.claude` login must still get its row.
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let _env = isolated(&home);
    let root = d.path().join("root");
    let store = store_with(&root, &["k"]);
    private(&home.join(".claude/.credentials.json"), "{}");
    private(
        &home.join(".claude.json"),
        r#"{"oauthAccount": {"accountUuid": "u1", "emailAddress": "x@example.com"}}"#,
    );
    private(
        &root.join("k/.claude.json"),
        r#"{"oauthAccount": {"accountUuid": "u-k", "emailAddress": "x@example.com"}}"#,
    );
    // The rotator last put `k` in `~/.claude`; a manual login replaced it.
    let ledger_dir = root.join(".auto-switch.d/rotate");
    fs::create_dir_all(&ledger_dir).unwrap();
    for dir in [ledger_dir.parent().unwrap(), ledger_dir.as_path()] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(
        &ledger_dir.join("ledger.json"),
        r#"{"version":1,"live":"k","account":"u-k","since":10}"#,
    );
    let bin = fake_claude(d.path(), "exit 0");
    let cancel = AtomicBool::new(false);

    let rows =
        probe_profiles_and_home(&store, &bin, &[], Duration::from_secs(10), &cancel).unwrap();

    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].live_in_home, "{rows:?}");
}
