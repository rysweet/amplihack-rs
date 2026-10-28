//! Scratch directory and rerun stdin (issue #1618): `$HOME/.amplihack/rerun`
//! ownership and mode rules, what a rerun reads on stdin (a regular file
//! from where the step started, a terminal unchanged, anything else
//! `/dev/null`, never a held-open pipe), and the `rerun/` sweep.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_scratch;`.

use super::super::auto_switch::{rerun_owner, sweep_rerun};
use super::scratch::{RerunStdin, home_rerun_dir};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    (tmp, home)
}

/// A private `rerun/`.
fn rerun() -> (tempfile::TempDir, PathBuf) {
    let (tmp, home) = home();
    let dir = home_rerun_dir(&home).unwrap();
    (tmp, dir)
}

/// What `cat` reads from `input`.
fn cat(input: Stdio) -> Vec<u8> {
    let out = Command::new("/bin/cat")
        .stdin(input)
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(out.status.success());
    out.stdout
}

// --- home_rerun_dir ------------------------------------------------------

#[test]
fn missing_directories_are_created_private() {
    let (_t, home) = home();
    let dir = home_rerun_dir(&home).unwrap();
    assert_eq!(dir, home.join(".amplihack").join("rerun"));
    assert_eq!(mode(&home.join(".amplihack")), 0o700);
    assert_eq!(mode(&dir), 0o700);
}

#[test]
fn an_existing_0755_dot_amplihack_is_accepted_and_never_chmodded() {
    let (_t, home) = home();
    let dot = home.join(".amplihack");
    fs::create_dir(&dot).unwrap();
    fs::set_permissions(&dot, fs::Permissions::from_mode(0o755)).unwrap();
    home_rerun_dir(&home).unwrap();
    assert_eq!(mode(&dot), 0o755);
}

#[test]
fn a_dot_amplihack_others_can_write_is_refused_and_left_alone() {
    let (_t, home) = home();
    let dot = home.join(".amplihack");
    fs::create_dir(&dot).unwrap();
    fs::set_permissions(&dot, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(home_rerun_dir(&home).is_err());
    assert_eq!(mode(&dot), 0o777, "never chmod'ed");
    assert!(!dot.join("rerun").exists(), "nothing written into it");
}

#[test]
fn a_symlinked_dot_amplihack_is_refused() {
    let (tmp, home) = home();
    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real, home.join(".amplihack")).unwrap();
    assert!(home_rerun_dir(&home).is_err());
    assert!(!real.join("rerun").exists());
}

#[test]
fn rerun_must_be_a_real_directory_with_mode_0700() {
    let (_t, home) = home();
    let rerun = home.join(".amplihack/rerun");
    fs::create_dir_all(&rerun).unwrap();
    fs::set_permissions(home.join(".amplihack"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&rerun, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(home_rerun_dir(&home).is_err(), "0755 rerun/");
    fs::remove_dir(&rerun).unwrap();
    let elsewhere = home.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&elsewhere, &rerun).unwrap();
    assert!(home_rerun_dir(&home).is_err(), "symlinked rerun/");
}

// --- RerunStdin ----------------------------------------------------------

#[test]
fn a_regular_file_is_read_again_from_where_the_step_started() {
    let (tmp, _) = home();
    let path = tmp.path().join("input");
    fs::write(&path, b"head|prompt").unwrap();
    let mut step_stdin = fs::File::open(&path).unwrap();
    let mut skipped = [0u8; 5];
    step_stdin.read_exact(&mut skipped).unwrap();
    let policy = RerunStdin::from_file(step_stdin.try_clone().unwrap());
    assert!(
        matches!(policy, RerunStdin::File { start: 5, .. }),
        "{policy:?}"
    );
    // The first attempt reads it to the end through the shared offset.
    let mut rest = Vec::new();
    step_stdin.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"prompt");
    for n in [2, 3] {
        assert_eq!(
            cat(policy.for_attempt(n).unwrap()),
            b"prompt",
            "attempt {n}"
        );
    }
}

#[test]
fn a_held_open_pipe_gives_reruns_dev_null_without_reading_it() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"never read").unwrap();
    let policy = RerunStdin::from_file(fs::File::from(std::os::fd::OwnedFd::from(reader)));
    assert!(matches!(policy, RerunStdin::Pipe), "{policy:?}");
    // The writer is still open: a rerun reading the pipe would hang here.
    assert_eq!(cat(policy.for_attempt(2).unwrap()), b"");
    drop(writer);
}

#[test]
fn a_character_device_is_passed_through() {
    let policy = RerunStdin::from_file(fs::File::open("/dev/null").unwrap());
    assert!(matches!(policy, RerunStdin::Inherit), "{policy:?}");
}

// --- rerun/ sweep --------------------------------------------------------

#[test]
fn rerun_owner_parses_out_names_only() {
    assert_eq!(rerun_owner(OsStr::new("123-4.out")), Some(123));
    for bad in [
        "123-0.txt",
        "123-0.in",
        "abc-0.out",
        "0-0.out",
        "123.out",
        "123-x.out",
        "-1-0.out",
        ".out",
    ] {
        assert_eq!(rerun_owner(OsStr::new(bad)), None, "{bad}");
    }
}

/// A pid that has exited (and been reaped), so it names no live process.
fn dead_pid() -> u32 {
    let mut c = Command::new("/bin/true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

#[test]
fn the_sweep_removes_orphaned_buffers_and_nothing_it_cannot_parse() {
    let (_t, dir) = rerun();
    let dead = dead_pid();
    let me = std::process::id();
    let orphan_old = dir.join(format!("{dead}-0.out"));
    let orphan_out = dir.join(format!("{dead}-1.out"));
    let live = dir.join(format!("{me}-0.out"));
    let unknown = dir.join("notes.txt");
    for p in [&orphan_old, &orphan_out, &live, &unknown] {
        fs::write(p, b"x").unwrap();
    }
    let target = dir.join("target");
    fs::write(&target, b"keep").unwrap();
    let link = dir.join(format!("{dead}-2.out"));
    symlink(&target, &link).unwrap();
    let subdir = dir.join(format!("{dead}-3.out"));
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join(format!("{dead}-4.out")), b"x").unwrap();

    sweep_rerun(&dir);

    assert!(!orphan_old.exists(), "orphaned buffer removed");
    assert!(!orphan_out.exists(), "orphaned .out removed");
    assert!(live.exists(), "a live process's buffer is kept");
    assert!(unknown.exists(), "unparseable names are kept");
    assert!(
        fs::symlink_metadata(&link).is_ok(),
        "symlinks are not removed"
    );
    assert_eq!(
        fs::read(&target).unwrap(),
        b"keep",
        "symlinks are not followed"
    );
    assert!(
        subdir.join(format!("{dead}-4.out")).exists(),
        "no recursion"
    );
}

#[test]
fn the_age_rule_applies_only_to_names_that_parse() {
    let (_t, dir) = rerun();
    let me = std::process::id();
    let old_live = dir.join(format!("{me}-0.out"));
    let old_unknown = dir.join("notes.txt");
    let two_days_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
    for p in [&old_live, &old_unknown] {
        let f = fs::File::create(p).unwrap();
        f.set_modified(two_days_ago).unwrap();
    }

    sweep_rerun(&dir);

    assert!(
        !old_live.exists(),
        "a parseable name over 24 h old is removed"
    );
    assert!(
        old_unknown.exists(),
        "an unparseable name is kept at any age"
    );
}
