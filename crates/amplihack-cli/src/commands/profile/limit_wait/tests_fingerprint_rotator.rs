//! Issue #1761: whether a rotator is seen for the watched directory
//! ([`Watch::rotator_running`]) and the login a retry resumes on
//! ([`Watch::login`]), using the helpers of `tests_fingerprint.rs`.
//!
//! A rotator counts only for `$HOME/.claude` with a known profile root, a
//! pid file naming a running `profile rotate watch` of this user, and a
//! rotate ledger that is readable or absent. The fake rotator is a shell
//! loop whose arguments hold `profile rotate watch` (never `--spawned`); it
//! exits when this test process does and is killed on drop.
//!
//! Wired from `limit_wait/mod.rs` with
//! `#[cfg(test)] mod tests_fingerprint_rotator;`.

use super::fingerprint::Watch;
use super::tests_fingerprint::{env, private, rotate_ledger};
use amplihack_utils::rotator_status::pid_path;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// A child process killed and reaped on drop.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(args: &[&str]) -> Proc {
    Proc(
        Command::new("/bin/sh")
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn /bin/sh"),
    )
}

/// A process whose arguments hold `profile rotate watch`, without
/// `--spawned`, that exits once its parent (this test process) is gone.
fn fake_rotator() -> Proc {
    let p = spawn(&[
        "-c",
        "while kill -0 \"$PPID\" 2>/dev/null; do sleep 1; done",
        "amplihack",
        "profile",
        "rotate",
        "watch",
    ]);
    await_args(p.0.id());
    p
}

/// Waits until `/proc/<pid>/cmdline` holds the child's arguments. A spawn
/// returns once the child's `execve` has replaced its memory, a moment
/// before the kernel records where the arguments are, so a check made at
/// once can still read an empty command line.
fn await_args(pid: u32) {
    if !Path::new("/proc").is_dir() {
        return;
    }
    let cmdline = format!("/proc/{pid}/cmdline");
    for _ in 0..500 {
        if fs::read(&cmdline).is_ok_and(|c| c.split(|b| *b == 0).any(|a| a == b"watch")) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the fake rotator's arguments never appeared in {cmdline}");
}

/// `pid` written to `root`'s rotate pid file, with private directories.
fn pid_file(root: &Path, pid: u32) {
    let path = pid_path(root);
    let rotate = path.parent().unwrap();
    fs::create_dir_all(rotate).unwrap();
    for d in [rotate.parent().unwrap(), rotate] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(&path, &format!("{pid}\n"));
}

#[test]
fn a_running_rotator_is_seen_for_home_claude() {
    let e = env();
    let r = fake_rotator();
    pid_file(e.store.root(), r.0.id());
    assert!(e.home_watch().rotator_running(), "no ledger yet is fine");
    e.rotator("a");
    assert!(e.home_watch().rotator_running(), "with a readable ledger");
    let cfg = e.home.join(".claude");
    let explicit = Watch::new(Some(&e.store), None, Some(&cfg), Some(&e.home));
    assert!(
        explicit.rotator_running(),
        "CLAUDE_CONFIG_DIR set to $HOME/.claude is the same directory"
    );
}

#[test]
fn no_pid_file_means_no_rotator() {
    let e = env();
    e.rotator("a");
    assert!(!e.home_watch().rotator_running());
}

#[test]
fn a_pid_file_naming_no_rotator_means_none() {
    let e = env();
    let other = spawn(&["-c", "sleep 30"]);
    pid_file(e.store.root(), other.0.id());
    assert!(!e.home_watch().rotator_running(), "not a rotator");

    let mut gone = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    pid_file(e.store.root(), pid);
    assert!(!e.home_watch().rotator_running(), "an exited process");

    private(&pid_path(e.store.root()), "not a pid\n");
    assert!(!e.home_watch().rotator_running(), "a malformed pid file");
}

#[test]
fn an_unreadable_ledger_means_no_rotator() {
    let e = env();
    let r = fake_rotator();
    pid_file(e.store.root(), r.0.id());
    for body in ["not json", r#"{"version":99,"live":"a"}"#] {
        private(&rotate_ledger(e.store.root()), body);
        assert!(!e.home_watch().rotator_running(), "{body}");
    }
    e.rotator("a");
    fs::set_permissions(
        rotate_ledger(e.store.root()),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert!(
        !e.home_watch().rotator_running(),
        "a writable-by-others ledger"
    );
}

#[test]
fn a_rotator_does_not_count_for_a_profile_or_another_config_dir() {
    let e = env();
    let r = fake_rotator();
    pid_file(e.store.root(), r.0.id());
    e.rotator("a");
    assert!(e.home_watch().rotator_running(), "sanity: seen for home");
    assert!(!e.profile("a").rotator_running(), "an inherited profile");
    let other = e.tmp.join("elsewhere");
    fs::create_dir_all(&other).unwrap();
    let w = Watch::new(Some(&e.store), None, Some(&other), Some(&e.home));
    assert!(!w.rotator_running(), "another CLAUDE_CONFIG_DIR");
}

#[test]
fn an_unknown_profile_root_means_no_rotator() {
    let e = env();
    let r = fake_rotator();
    pid_file(e.store.root(), r.0.id());
    let w = Watch::new(None, None, None, Some(&e.home));
    assert!(!w.rotator_running());
}

#[test]
fn the_check_only_reads_and_never_signals_the_rotator() {
    let e = env();
    let mut r = fake_rotator();
    pid_file(e.store.root(), r.0.id());
    let before = fs::read(pid_path(e.store.root())).unwrap();
    for _ in 0..3 {
        assert!(e.home_watch().rotator_running());
    }
    assert!(r.0.try_wait().unwrap().is_none(), "the rotator was stopped");
    assert_eq!(fs::read(pid_path(e.store.root())).unwrap(), before);
}

// --- login ----------------------------------------------------------------

#[test]
fn a_profile_resumes_on_its_own_name() {
    let e = env();
    e.rotator("b");
    assert_eq!(e.profile("a").login().as_deref(), Some("a"));
}

#[test]
fn home_claude_resumes_on_the_rotators_live_profile() {
    let e = env();
    assert_eq!(e.home_watch().login(), None, "no ledger");
    e.rotator("b");
    assert_eq!(e.home_watch().login().as_deref(), Some("b"));
    e.rotator("a");
    assert_eq!(e.home_watch().login().as_deref(), Some("a"), "read anew");
}

#[test]
fn home_claude_with_no_readable_live_name_is_unknown() {
    let e = env();
    for body in [
        "not json",
        r#"{"version":99,"live":"a"}"#,
        r#"{"version":1,"live":"a@b.com","last_cycle":{"at":1,"outcome":"swapped"}}"#,
        r#"{"version":1,"live":null}"#,
    ] {
        private(&rotate_ledger(e.store.root()), body);
        assert_eq!(e.home_watch().login(), None, "{body}");
    }
    let no_root = Watch::new(None, None, None, Some(&e.home));
    assert_eq!(no_root.login(), None, "no profile root");
}

#[test]
fn another_config_dir_is_unknown() {
    let e = env();
    e.rotator("b");
    let other = e.tmp.join("elsewhere");
    fs::create_dir_all(&other).unwrap();
    let w = Watch::new(Some(&e.store), None, Some(&other), Some(&e.home));
    assert_eq!(w.login(), None);
}
