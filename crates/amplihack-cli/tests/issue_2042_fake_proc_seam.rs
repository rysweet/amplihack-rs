//! Issue #2042: `amplihack profile` reads the process table the test names
//! in `AMPLIHACK_TEST_PROC_ROOT`, not the host's `/proc`.
//!
//! Each check runs a real command through the rotate fixture twice: once
//! with a fake tree holding one marked process, once with the shared empty
//! tree, and asserts on output and exit status only. Fake pids lie above
//! `pid_max`, so no signal can reach a host process. Linux-only, like the
//! readers under test.

#![cfg(target_os = "linux")]

use crate::isolated_amplihack::{PROC_ROOT_ENV, shared_empty_proc_root};
use crate::support::fake_proc::{FAKE_PID_BASE, FakeProc, SELF_ENVIRON};
use crate::support::profile_rotate::*;
use crate::support::wait::output_within;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Output;

const SESSION_PID: u32 = FAKE_PID_BASE;
const ROTATOR_PID: u32 = FAKE_PID_BASE + 1;

/// `amplihack ARGS` through the fixture, reading `tree` as `/proc`.
fn run_on(f: &Fixture, tree: &Path, args: &[&str]) -> Output {
    let mut c = f.cmd(args);
    c.env(PROC_ROOT_ENV, tree);
    let out = output_within(&format!("`amplihack {}`", args.join(" ")), &mut c);
    assert_no_secrets(&out);
    out
}

fn report(out: &Output) -> String {
    format!(
        "status {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

/// A fake Claude session using profile `name`, in state `state`.
fn session(fake: &FakeProc, f: &Fixture, name: &str, state: char) {
    let dir = f.root.join(name);
    assert!(dir.is_absolute(), "liveness would read the entry's cwd");
    fake.process(SESSION_PID)
        .cmdline(&["claude"])
        .status(state)
        .environ(&[("CLAUDE_CONFIG_DIR", dir.to_str().unwrap())]);
}

/// A fake `profile rotate watch` named in the fixture's pid file. With
/// `ours`, its `exe` is the binary under test.
fn rotator(fake: &FakeProc, f: &Fixture, ours: bool) {
    let bin = amplihack_bin();
    let entry = fake.process(ROTATOR_PID);
    entry
        .cmdline(&[
            bin.to_str().unwrap(),
            "profile",
            "rotate",
            "watch",
            "--spawned",
        ])
        .status('S');
    if ours {
        entry.exe_link(&bin);
    }
    let dir = f.rotate_dir();
    fs::create_dir_all(&dir).unwrap();
    for d in [dir.parent().unwrap(), dir.as_path()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(dir.join("rotate.pid"), format!("{ROTATOR_PID}\n")).unwrap();
}

// --- the builder ------------------------------------------------------------

#[test]
fn a_new_tree_is_private_and_holds_only_self_environ() {
    let fake = FakeProc::new();
    let root = fake.path();
    assert!(root.starts_with(std::env::temp_dir()));
    assert_eq!(fs::metadata(root).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::read(root.join("self/environ")).unwrap(), SELF_ENVIRON);
    let m = fs::metadata(root.join("self/environ")).unwrap();
    assert_eq!(m.mode() & 0o777, 0o600);
    let entry = fake.process(FAKE_PID_BASE);
    entry
        .status('Z')
        .cmdline(&["a b", "c"])
        .environ(&[("K", "v=w")]);
    let status = fs::read_to_string(entry.path().join("status")).unwrap();
    assert!(status.contains("State:\tZ"), "{status}");
    assert!(status.contains(&format!("Uid:\t{}\t", euid())), "{status}");
    assert_eq!(fs::read(entry.path().join("cmdline")).unwrap(), b"a b\0c\0");
    assert_eq!(fs::read(entry.path().join("environ")).unwrap(), b"K=v=w\0");
    fake.remove(FAKE_PID_BASE);
    fake.remove(FAKE_PID_BASE); // already gone: still fine
    assert!(!entry.path().exists());
}

#[test]
#[should_panic(expected = "fake pid")]
fn a_pid_a_host_process_could_hold_is_refused() {
    FakeProc::new().process(FAKE_PID_BASE - 1);
}

#[test]
#[should_panic(expected = "fake pid")]
fn a_pid_that_wraps_to_a_process_group_is_refused() {
    FakeProc::new().process(i32::MAX as u32 + 1);
}

#[test]
#[should_panic(expected = "NUL")]
fn a_nul_cannot_forge_an_environ_entry() {
    FakeProc::new()
        .process(FAKE_PID_BASE)
        .environ(&[("A", "x\0CLAUDE_CONFIG_DIR=/y")]);
}

#[test]
#[should_panic(expected = "bad environ key")]
fn an_environ_key_cannot_hold_an_equals_sign() {
    FakeProc::new()
        .process(FAKE_PID_BASE)
        .environ(&[("A=B", "x")]);
}

// --- the fixture -------------------------------------------------------------

#[test]
fn the_rotate_fixture_keeps_the_empty_tree_after_env_clear() {
    let f = Fixture::new();
    let c = f.cmd(&["profile", "rotate", "status"]);
    let set = c
        .get_envs()
        .find(|(k, _)| *k == OsStr::new(PROC_ROOT_ENV))
        .and_then(|(_, v)| v);
    assert_eq!(set, Some(shared_empty_proc_root().as_os_str()));
}

// --- liveness -----------------------------------------------------------------

#[test]
fn auto_switch_status_reports_a_tree_without_self_environ() {
    let f = Fixture::new();
    let fake = FakeProc::new();
    let status = |tree: &Path| {
        let out = run_on(&f, tree, &["profile", "auto-switch", "status"]);
        assert!(out.status.success(), "{}", report(&out));
        text(&out.stdout)
    };
    let readable = status(fake.path());
    assert!(
        readable.contains("in-session swap: available (Linux)"),
        "{readable}"
    );
    fs::remove_file(fake.path().join("self/environ")).unwrap();
    let unreadable = status(fake.path());
    assert!(
        unreadable.contains("in-session swap: unavailable (/proc cannot be read here"),
        "the child read the host /proc, not the fake tree: {unreadable}"
    );
}

#[test]
fn a_fake_session_keeps_its_login_from_moving() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "work"]);
    let fake = FakeProc::new();
    session(&fake, &f, "spare", 'S');
    let out = run_on(&f, fake.path(), &["profile", "switch", "spare"]);
    assert_eq!(out.status.code(), Some(1), "{}", report(&out));
    assert!(
        text(&out.stderr).contains("(session running)"),
        "{}",
        report(&out)
    );
    assert!(
        text(&out.stdout).contains("Live login not moved"),
        "{}",
        report(&out)
    );
    assert_eq!(ino(&f.home_login()), Some(f.live_ino), "work stays live");
}

#[test]
fn a_zombie_or_an_empty_tree_holds_no_login() {
    let f = Fixture::new();
    let zombie = FakeProc::new();
    session(&zombie, &f, "spare", 'Z');
    let out = run_on(&f, zombie.path(), &["profile", "switch", "spare"]);
    assert!(
        out.status.success(),
        "a zombie is not a session: {}",
        report(&out)
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "spare moved in");
    let out = run_on(
        &f,
        shared_empty_proc_root(),
        &["profile", "switch", "extra"],
    );
    assert!(out.status.success(), "{}", report(&out));
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino), "extra moved in");
}

// --- rotator --------------------------------------------------------------------

#[test]
fn rotate_status_sees_a_fake_rotator_only_in_its_tree() {
    let f = Fixture::new();
    let fake = FakeProc::new();
    rotator(&fake, &f, true);
    let out = run_on(&f, fake.path(), &["profile", "rotate", "status"]);
    let shown = text(&out.stdout);
    assert!(out.status.success(), "{}", report(&out));
    assert!(shown.contains("\nrotate: running\n"), "{shown}");
    assert!(
        shown.contains(&format!("\npid: {ROTATOR_PID}\n")),
        "{shown}"
    );
    let out = run_on(
        &f,
        shared_empty_proc_root(),
        &["profile", "rotate", "status"],
    );
    let shown = text(&out.stdout);
    assert!(shown.contains("\nrotate: stale\n"), "{shown}");
    assert!(shown.contains("\npid: -\n"), "{shown}");
}

#[test]
fn a_fake_rotator_without_its_exe_is_foreign() {
    let f = Fixture::new();
    let fake = FakeProc::new();
    rotator(&fake, &f, false);
    let out = run_on(&f, fake.path(), &["profile", "switch", "spare"]);
    assert!(out.status.success(), "{}", report(&out));
    let note =
        format!("the Claude login rotator (pid {ROTATOR_PID}) runs another amplihack binary");
    assert!(text(&out.stderr).contains(&note), "{}", report(&out));

    let g = Fixture::new();
    let ours = FakeProc::new();
    rotator(&ours, &g, true);
    let out = run_on(&g, ours.path(), &["profile", "switch", "spare"]);
    assert!(out.status.success(), "{}", report(&out));
    assert!(
        !text(&out.stderr).contains("runs another amplihack binary"),
        "{}",
        report(&out)
    );
}

#[test]
fn rotate_stop_cannot_signal_a_fake_rotator() {
    let f = Fixture::new();
    let fake = FakeProc::new();
    rotator(&fake, &f, true);
    let out = run_on(&f, fake.path(), &["profile", "rotate", "stop"]);
    assert!(!out.status.success(), "{}", report(&out));
    // `stop` checks the pid against the real `/proc` before it signals, and
    // no process there can hold a pid above `pid_max`.
    assert!(
        text(&out.stderr).contains("changed before it was signalled"),
        "{}",
        report(&out)
    );
    assert!(
        f.rotate_dir().join("rotate.pid").exists(),
        "a failed stop keeps the pid file: {}",
        report(&out)
    );
    assert!(
        !text(&out.stdout).contains("rotate: stopped"),
        "{}",
        report(&out)
    );
}
