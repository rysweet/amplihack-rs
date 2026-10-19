//! Scratch directory and stdin copy (issue #1618): `$HOME/.amplihack/rerun`
//! ownership and mode rules, the `<pid>-0.in` copy (0600, no size cap,
//! device and inode checked on every reopen, removed on drop), the two copy
//! failure kinds, and the `rerun/` sweep that now also removes `.in` files.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_scratch;`.
//! The sweep is reached through `auto_switch`'s
//! `pub(crate) use swap::{rerun_owner, sweep_rerun};`.

use super::super::auto_switch::{rerun_owner, sweep_rerun};
use super::scratch::{CopyError, StdinCopy, home_rerun_dir};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    (tmp, home)
}

/// A private `rerun/` ready for copies.
fn rerun() -> (tempfile::TempDir, PathBuf) {
    let (tmp, home) = home();
    let dir = home_rerun_dir(&home).unwrap();
    (tmp, dir)
}

/// Runs `cat` with the copy as its stdin and returns what it read.
fn cat(copy: &StdinCopy) -> io::Result<Vec<u8>> {
    let out = Command::new("cat").stdin(copy.stdio()?).output()?;
    assert!(out.status.success());
    Ok(out.stdout)
}

/// A reader that must never be read: proves a failure came before input.
struct Untouched;
impl Read for Untouched {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("stdin was read although the copy could not be made");
    }
}

/// Yields `n` bytes, then fails.
struct FailsAfter(usize);
impl Read for FailsAfter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.0 == 0 {
            return Err(io::Error::other("disk full"));
        }
        let k = buf.len().min(self.0);
        buf[..k].fill(b'x');
        self.0 -= k;
        Ok(k)
    }
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

// --- StdinCopy -----------------------------------------------------------

#[test]
fn the_copy_is_a_private_file_named_for_this_process() {
    let (_t, dir) = rerun();
    let copy = StdinCopy::take_from(&dir, &b"prompt"[..], false).unwrap();
    let path = copy.path().expect("piped stdin is copied").to_path_buf();
    assert_eq!(path, dir.join(format!("{}-0.in", std::process::id())));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(fs::read(&path).unwrap(), b"prompt");
}

#[test]
fn the_copy_has_no_size_cap_and_every_attempt_reads_it_from_the_start() {
    let (_t, dir) = rerun();
    let input: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let copy = StdinCopy::take_from(&dir, input.as_slice(), false).unwrap();
    assert_eq!(cat(&copy).unwrap(), input, "first attempt");
    assert_eq!(cat(&copy).unwrap(), input, "second attempt");
}

#[test]
fn an_empty_stdin_is_copied_as_an_empty_file() {
    let (_t, dir) = rerun();
    let copy = StdinCopy::take_from(&dir, io::empty(), false).unwrap();
    assert_eq!(cat(&copy).unwrap(), b"");
}

#[test]
fn a_replaced_copy_is_refused() {
    let (_t, dir) = rerun();
    let copy = StdinCopy::take_from(&dir, &b"original"[..], false).unwrap();
    let path = copy.path().unwrap().to_path_buf();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"swapped").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        copy.stdio().is_err(),
        "a new inode must not be fed to an attempt"
    );
}

#[test]
fn a_copy_replaced_by_a_symlink_is_refused() {
    let (_t, dir) = rerun();
    let copy = StdinCopy::take_from(&dir, &b"original"[..], false).unwrap();
    let path = copy.path().unwrap().to_path_buf();
    let other = dir.join("other");
    fs::write(&other, b"evil").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&other, &path).unwrap();
    assert!(copy.stdio().is_err());
}

#[test]
fn dropping_the_copy_removes_the_file() {
    let (_t, dir) = rerun();
    let copy = StdinCopy::take_from(&dir, &b"x"[..], false).unwrap();
    let path = copy.path().unwrap().to_path_buf();
    drop(copy);
    assert!(!path.exists());
}

#[test]
fn a_terminal_is_not_copied_and_is_inherited() {
    let (_t, dir) = rerun();
    let copy = StdinCopy::take_from(&dir, Untouched, true).unwrap();
    assert!(copy.path().is_none());
    assert!(copy.stdio().is_ok());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "no file for a TTY");
}

#[test]
fn a_file_that_cannot_be_created_fails_before_any_input_is_read() {
    let (_t, dir) = rerun();
    let missing = dir.join("missing");
    assert!(matches!(
        StdinCopy::take_from(&missing, Untouched, false),
        Err(CopyError::BeforeRead(_))
    ));
    // A leftover file of the same name is never reused or truncated.
    let taken = dir.join(format!("{}-0.in", std::process::id()));
    fs::write(&taken, b"stale").unwrap();
    assert!(matches!(
        StdinCopy::take_from(&dir, Untouched, false),
        Err(CopyError::BeforeRead(_))
    ));
    assert_eq!(fs::read(&taken).unwrap(), b"stale");
}

#[test]
fn a_read_error_part_way_fails_after_read_and_leaves_no_file() {
    let (_t, dir) = rerun();
    assert!(matches!(
        StdinCopy::take_from(&dir, FailsAfter(100_000), false),
        Err(CopyError::AfterRead(_))
    ));
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "partial copy removed"
    );
}

// --- rerun/ sweep --------------------------------------------------------

#[test]
fn rerun_owner_parses_in_and_out_names() {
    assert_eq!(rerun_owner(OsStr::new("123-0.in")), Some(123));
    assert_eq!(rerun_owner(OsStr::new("123-4.out")), Some(123));
    for bad in [
        "123-0.txt",
        "abc-0.in",
        "0-0.in",
        "123.in",
        "123-x.in",
        "-1-0.in",
        ".in",
    ] {
        assert_eq!(rerun_owner(OsStr::new(bad)), None, "{bad}");
    }
}

/// A pid that has exited (and been reaped), so it names no live process.
fn dead_pid() -> u32 {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

#[test]
fn the_sweep_removes_orphaned_in_files_and_nothing_it_cannot_parse() {
    let (_t, dir) = rerun();
    let dead = dead_pid();
    let me = std::process::id();
    let orphan_in = dir.join(format!("{dead}-0.in"));
    let orphan_out = dir.join(format!("{dead}-1.out"));
    let live_in = dir.join(format!("{me}-0.in"));
    let unknown = dir.join("notes.txt");
    for p in [&orphan_in, &orphan_out, &live_in, &unknown] {
        fs::write(p, b"x").unwrap();
    }
    let target = dir.join("target");
    fs::write(&target, b"keep").unwrap();
    let link = dir.join(format!("{dead}-2.in"));
    symlink(&target, &link).unwrap();
    let subdir = dir.join(format!("{dead}-3.in"));
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join(format!("{dead}-4.in")), b"x").unwrap();

    sweep_rerun(&dir);

    assert!(!orphan_in.exists(), "orphaned .in removed");
    assert!(!orphan_out.exists(), "orphaned .out removed");
    assert!(live_in.exists(), "a live process's copy is kept");
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
    assert!(subdir.join(format!("{dead}-4.in")).exists(), "no recursion");
}
