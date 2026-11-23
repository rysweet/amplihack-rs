//! Scratch directory and rerun stdin (issue #1618): `$HOME/.amplihack/rerun`
//! ownership and mode rules, what a rerun reads on stdin (a regular file
//! from where the step started, a captured prompt from memory, a terminal
//! unchanged, anything else `/dev/null`, never a held-open pipe), and the
//! `rerun/` sweep.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_scratch;`.

use super::super::auto_switch::{rerun_owner, sweep_rerun};
use super::scratch::{
    CopyState, Replay, RerunStdin, capture_prompt, copy_prompt, home_rerun_dir, kept, run_copier,
};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
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
    let mut policy = RerunStdin::from_file(step_stdin.try_clone().unwrap());
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
    let mut policy = RerunStdin::from_file(fs::File::from(std::os::fd::OwnedFd::from(reader)));
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
    assert_eq!(rerun_owner(OsStr::new("123-0.in")), Some(123), "#2815");
    for bad in [
        "123-0.txt",
        "123-1.in",
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
/// Spawned as `/bin/sh -c "exit 0"`: an absolute path keeps it independent
/// of tests that change `PATH`, and `/bin/true` is absent on macOS.
fn dead_pid() -> u32 {
    let mut c = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
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

// --- Captured prompt (issue #2815) ----------------------------------------
//
// Contract: `capture_prompt(source)` takes a duplicate of the step's stdin
// (a pipe) and returns `RerunStdin::Captured`, which keeps the copy in
// memory and nowhere else. It returns an error only
// when what the copy needs (a second description of the source, the first
// attempt's pipe, the copier thread) cannot be made, and has then read
// nothing; the supervisor does not run such a step. That path is pinned
// outside in, in `issue_2911_prompt_loss_ending`, because the only way to
// reach it on demand is an environment variable, and setting one here
// would reach every capture test running beside it in this process.
//
// Nothing is read off `source` until `attempt_started`, which the
// supervisor calls once the first attempt's child holds the other end of
// the prompt pipe (issue #2816). These tests therefore call it where a
// supervised step would, right after `for_attempt(1)`.

/// A prompt with NUL bytes, invalid UTF-8 and more bytes than a pipe holds.
fn awkward_prompt() -> Vec<u8> {
    let mut v = b"head\0\xff\xfe\n".to_vec();
    v.extend((0..300_000u32).map(|i| (i % 251) as u8));
    v
}

/// A pipe whose read end is a `File`, and the thread writing `bytes` then
/// closing it.
fn fed_pipe(bytes: Vec<u8>) -> (fs::File, std::thread::JoinHandle<()>) {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let t = std::thread::spawn(move || {
        writer.write_all(&bytes).unwrap();
    });
    (fs::File::from(std::os::fd::OwnedFd::from(reader)), t)
}

/// `capture_prompt`, which must succeed in these tests.
fn captured(source: fs::File) -> RerunStdin {
    let policy = capture_prompt(source).expect("the copy could be set up");
    assert!(matches!(policy, RerunStdin::Captured(_)), "{policy:?}");
    policy
}

/// The whole prompt a rerun is given.
///
/// `replay` answers at once and never waits: a prompt still arriving is
/// `Ready`, because the rerun's feeder gives it the rest as it arrives.
/// So nothing here asks twice. `cat` reads the
/// rerun's stdin to its end, which comes when the copier reaches the end of
/// the source, and the caller of this helper has closed it or will.
fn settled(policy: &mut RerunStdin) -> Vec<u8> {
    match policy.replay() {
        Replay::Ready => cat(policy.for_attempt(2).unwrap()),
        Replay::Never(why) => panic!("a rerun can never be given this prompt: {why}"),
    }
}

#[test]
fn every_attempt_of_a_captured_prompt_reads_the_same_bytes_from_offset_zero() {
    let prompt = awkward_prompt();
    let (source, feeder) = fed_pipe(prompt.clone());
    let mut policy = captured(source);
    // Attempt 1 reads the pipe while the copier is still pumping.
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    assert_eq!(cat(first), prompt, "attempt 1");
    feeder.join().unwrap();
    for n in [2, 3, 4] {
        assert_eq!(cat(policy.for_attempt(n).unwrap()), prompt, "attempt {n}");
    }
}

#[test]
fn an_attempt_that_never_reads_its_prompt_does_not_stop_the_capture() {
    let prompt = awkward_prompt();
    let (source, feeder) = fed_pipe(prompt.clone());
    let mut policy = captured(source);
    // The child exits without reading: its stdin is closed at once, so the
    // copier's writes to it fail with EPIPE and are absorbed.
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    drop(first);
    feeder.join().unwrap();
    assert_eq!(settled(&mut policy), prompt);
}

/// Every byte the copier forwards to the attempt is
/// kept, in order, before it is forwarded. So whatever the attempt was
/// given, a rerun is given too, and there is no second store whose failure
/// could cost a rerun: the copy cannot be lost while the prompt is read.
#[test]
fn the_kept_copy_is_every_byte_the_attempt_was_given() {
    let prompt = awkward_prompt();
    let (mut source, feeder) = fed_pipe(prompt.clone());
    let (mut child_read, child_write) = std::io::pipe().unwrap();
    let drain = std::thread::spawn(move || {
        let mut got = Vec::new();
        child_read.read_to_end(&mut got).unwrap();
        got
    });
    let state = CopyState::default();
    let mut child = Some(child_write);
    copy_prompt(&mut source, &mut child, &state).expect("a whole prompt copies");
    feeder.join().unwrap();
    assert!(
        child.is_none(),
        "the attempt's pipe is closed at end of file"
    );
    assert_eq!(drain.join().unwrap(), prompt, "the attempt's whole prompt");
    assert_eq!(kept(&state), prompt, "and the kept copy, byte for byte");
    assert!(state.is_complete() && !state.was_cut_short(), "{state:?}");
    assert!(
        !format!("{state:?}").contains("head"),
        "no part of the prompt is printed: {state:?}"
    );
}

#[test]
fn a_failed_copy_is_an_error_for_a_rerun_never_a_partial_prompt() {
    // A write-only descriptor: reading it fails (EBADF), so the copier ends
    // without having seen end of file.
    let source = fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let mut policy = captured(source);
    // Issue #2816: the loss is readable without joining the copier, which
    // is what lets the supervisor refuse the rerun before the wait rather
    // than after it. Nothing here is timed: the copier publishes the loss
    // before it drops the attempt's pipe, so draining that pipe to its end
    // puts this test after the publication rather than beside it.
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    assert_eq!(cat(first), b"");
    assert!(policy.prompt_cut_short(), "published without a join");
    // The refusal says why, so a caller is never left to guess.
    let why = policy.for_attempt(2).unwrap_err().to_string();
    assert!(why.contains("could not be read to its end"), "{why}");
    assert!(why.contains("attempt 2"), "{why}");
    // The supervisor asks the same question without building an `Stdio`,
    // and is given the same cause (issue #2816). No wait can make this
    // prompt whole (issue #3036).
    match policy.replay() {
        Replay::Never(why) => assert!(why.contains("could not be read to its end"), "{why}"),
        other => panic!("a prompt cut short can never be replayed: {other:?}"),
    }
}

/// Issue #2816: a read of the source that fails is published as a prompt
/// cut short, which is the one thing the attempt's own pipe cannot say.
///
/// Closing that pipe is the only signal it carries, and the attempt reads
/// it as the end of the prompt. So the flag has to be set before the pipe
/// is dropped, and it is: by the time anything can observe that end of
/// file, `prompt_cut_short` already holds. Reading the attempt's stdin to
/// its end before asking is how this test pins that order.
#[test]
fn a_failed_source_read_is_published_as_a_prompt_cut_short() {
    // A write-only descriptor: the first read of it fails (EBADF).
    let source = fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let mut policy = captured(source);
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    assert_eq!(
        cat(first),
        b"",
        "the attempt is given the end of file it cannot tell from a prompt"
    );
    assert!(
        policy.prompt_cut_short(),
        "published before the attempt could reach that end of file"
    );
    assert!(policy.for_attempt(2).is_err(), "no partial replay");
    // Issue #3036: the one input no rerun can ever be given; no wait could
    // make it whole.
    assert!(
        matches!(policy.replay(), Replay::Never(_)),
        "{:?}",
        policy.replay()
    );
}

/// A source whose every read panics, standing in for a fault in the copy
/// itself, which no real descriptor can be made to produce.
struct PanicsOnRead(fs::File);

impl Read for PanicsOnRead {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        panic!("a read that panics, on purpose, for this test");
    }
}

impl std::os::fd::AsRawFd for PanicsOnRead {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

/// A copier that panics is a prompt cut short, not a
/// failed copy of a prompt the attempt was given whole.
///
/// The attempt's pipe closes wherever the copier stopped, and the attempt
/// reads that as the end of its prompt, so it may have been given part of
/// its task. The panic is caught and published as `cut_short` before that
/// pipe is dropped, exactly as a failed read is: draining the pipe to its
/// end before asking pins that order. Before this the panic set nothing,
/// and an attempt that succeeded on part of its prompt passed for the step.
#[test]
fn a_copier_that_panics_publishes_a_prompt_cut_short_before_the_end_of_file() {
    let source = PanicsOnRead(fs::File::open("/dev/null").unwrap());
    let (mut child_read, child_write) = std::io::pipe().unwrap();
    let state = Arc::new(CopyState::default());
    let published = Arc::clone(&state);
    let (go, armed) = std::sync::mpsc::channel();
    let copier = std::thread::spawn(move || run_copier(&armed, source, child_write, &published));
    go.send(()).unwrap();
    let mut got = Vec::new();
    child_read.read_to_end(&mut got).unwrap();
    assert!(got.is_empty(), "nothing was forwarded");
    assert!(
        state.was_cut_short(),
        "published before the attempt could reach that end of file"
    );
    let e = copier
        .join()
        .expect("the copier catches its own panic")
        .expect_err("and reports it as a failed copy");
    assert!(e.to_string().contains("panicked"), "{e}");
    assert!(!state.is_complete(), "{state:?}");
}

/// Issue #2816: a copier that is never let go reads nothing, and ends when
/// its capture is dropped. That is what a stand-down relies on: the plain
/// launch that follows inherits fd 0 with the whole prompt still on it.
#[test]
fn a_copier_never_let_go_reads_nothing_and_ends() {
    let prompt = b"the piped prompt".to_vec();
    let (reader, mut writer) = std::io::pipe().unwrap();
    let source = fs::File::from(OwnedFd::from(reader));
    let mut spare = source.try_clone().unwrap();
    writer.write_all(&prompt).unwrap();
    drop(writer);
    let (child_read, child_write) = std::io::pipe().unwrap();
    let state = Arc::new(CopyState::default());
    let published = Arc::clone(&state);
    let (go, armed) = std::sync::mpsc::channel::<()>();
    let copier = std::thread::spawn(move || run_copier(&armed, source, child_write, &published));
    drop(go);
    copier
        .join()
        .unwrap()
        .expect("a copier that was never let go has nothing to report");
    drop(child_read);
    assert_eq!(kept(&state), b"", "nothing was copied");
    let mut left = Vec::new();
    spare.read_to_end(&mut left).unwrap();
    assert_eq!(left, prompt, "and nothing was taken off the source");
}

/// A source that reports every `EAGAIN` it hands back, so a test can wait
/// for the copier to meet one rather than guess when it will. The
/// descriptor is the pipe's own, so `copy_prompt` polls what it polls in
/// production.
struct ReportsEagain {
    inner: fs::File,
    met: std::sync::mpsc::Sender<()>,
}

impl Read for ReportsEagain {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let r = self.inner.read(buf);
        if matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock) {
            let _ = self.met.send(());
        }
        r
    }
}

impl std::os::fd::AsRawFd for ReportsEagain {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.inner.as_raw_fd()
    }
}

/// Issue #2816: an empty non-blocking source is waited on, not taken for
/// the end of the prompt.
///
/// `O_NONBLOCK` lives on the open file description, not on the descriptor,
/// so a process sharing the description the step inherited on fd 0 can set
/// it. A read of the empty pipe then fails with `EAGAIN`. Retrying only
/// `EINTR` would end the copy there and hand the attempt a prompt of
/// whatever had arrived (here, nothing at all).
///
/// Nothing is timed and nothing is assumed about scheduling. The source
/// reports each `EAGAIN`, and not one byte is written until a report has
/// arrived, so the retry is always the path taken: against the previous
/// behaviour the copier has already given up by then, and the join below
/// returns that error.
#[test]
fn an_empty_non_blocking_source_is_waited_on_not_taken_for_the_end() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut inner = fs::File::from(OwnedFd::from(reader));
    // SAFETY: an open descriptor this test owns; `F_SETFL` takes an
    // integer argument and writes nothing back.
    let rc = unsafe { libc::fcntl(inner.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    assert_eq!(rc, 0, "the source could not be made non-blocking");
    // The setup is the point of the test, so it is checked rather than
    // assumed: the pipe is empty and reading it now refuses.
    assert_eq!(
        inner.read(&mut [0u8; 1]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "the source must refuse an empty read for this test to mean anything"
    );
    let (met, eagain) = std::sync::mpsc::channel();
    let mut source = ReportsEagain { inner, met };

    // The attempt's stdin, drained as the copier writes it.
    let (mut child_read, child_write) = std::io::pipe().unwrap();
    let drain = std::thread::spawn(move || {
        let mut got = Vec::new();
        child_read.read_to_end(&mut got).unwrap();
        got
    });
    let state = Arc::new(CopyState::default());
    let published = Arc::clone(&state);
    let copier =
        std::thread::spawn(move || copy_prompt(&mut source, &mut Some(child_write), &published));

    // The pipe is empty, so the copier's first read refuses. Only once it
    // has said so does the prompt exist at all.
    eagain
        .recv()
        .expect("the copier must meet EAGAIN on the empty source");
    let prompt = awkward_prompt();
    writer.write_all(&prompt).unwrap();
    drop(writer);

    copier.join().unwrap().expect("EAGAIN is not a failed copy");
    assert_eq!(drain.join().unwrap(), prompt, "the attempt's whole prompt");
    assert_eq!(kept(&state), prompt, "and the kept copy");
    assert!(
        !state.was_cut_short(),
        "an empty non-blocking pipe is not a prompt that failed"
    );
    assert!(state.is_complete(), "the whole prompt was copied");
    assert!(
        !state.reader_is_gone(),
        "the attempt read its prompt to the end"
    );
}

/// Issue #2816: standing down leaves fd 0 whole, for every kind of stdin.
///
/// The plain launch that follows a stand-down inherits this process's own
/// fd 0. Every kind but a capture never touched it; a capture has not
/// either, because its copier is let go by the first attempt and a
/// supervisor stands down before one has run. `stand_down` is the check
/// that says so, and it neither reads fd 0 nor replaces it.
#[test]
fn standing_down_leaves_fd_zero_alone() {
    let before = fs::read_link("/proc/self/fd/0").ok();
    let file = fs::File::open("/dev/null").unwrap();
    let prompt = b"the piped prompt".to_vec();
    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut spare = fs::File::from(OwnedFd::from(reader));
    let source = spare.try_clone().unwrap();
    writer.write_all(&prompt).unwrap();
    drop(writer);
    let capture = captured(source);

    for policy in [
        RerunStdin::Inherit,
        RerunStdin::Pipe,
        RerunStdin::Null,
        RerunStdin::File { file, start: 0 },
        capture,
    ] {
        assert!(!policy.prompt_cut_short(), "{policy:?}");
        policy
            .stand_down()
            .unwrap_or_else(|e| panic!("{policy:?}: {e}"));
    }
    assert_eq!(
        fs::read_link("/proc/self/fd/0").ok(),
        before,
        "fd 0 was replaced by a stand-down"
    );
    // And the prompt is still on the pipe for the plain launch to read: the
    // copier was never let go, so not one byte of it was taken.
    let mut left = Vec::new();
    spare.read_to_end(&mut left).unwrap();
    assert_eq!(left, prompt, "the whole prompt is still there");
}

/// Issue #2816: once the copier has been let go, standing down is refused
/// rather than allowed to shorten the step's prompt.
///
/// No supervisor path reaches this (the copier is let go only after the
/// first attempt has spawned, and a stand-down only happens before that),
/// and this is the guard that keeps a new one from silently handing a plain
/// launch a prompt that is short by whatever the copier read.
#[test]
fn standing_down_after_the_copier_started_is_refused() {
    let (source, feeder) = fed_pipe(b"the piped prompt".to_vec());
    let mut policy = captured(source);
    assert!(policy.stand_down().is_ok(), "nothing taken yet");
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    let e = policy.stand_down().expect_err("fd 0 is short by now");
    assert!(e.to_string().contains("cannot be put back"), "{e}");
    drop(first);
    feeder.join().unwrap();
}

/// Issue #2816: a whole capture is not cut short, and replays the whole
/// prompt.
#[test]
fn a_capture_replays_the_whole_prompt() {
    let (source, feeder) = fed_pipe(b"the piped prompt".to_vec());
    let mut policy = captured(source);
    assert!(!policy.prompt_cut_short(), "{policy:?}");
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    drop(first);
    feeder.join().unwrap();
    assert_eq!(settled(&mut policy), b"the piped prompt");
    assert!(!policy.prompt_cut_short(), "a whole capture is not lost");
}

/// A prompt still arriving is fed to a rerun
/// as it arrives, and nothing waits for the caller to finish.
///
/// The copier ends at the end of the source, and the source is a pipe
/// belonging to whoever launched the step: a caller that holds it open for
/// ever (a prompt this version's option table misread as stdin's, or a
/// stream channel) would make any wait for the end of it a wait with no
/// end. Before this review `replay` answered `Arriving` here and the
/// supervisor waited, so such a caller held the step for ever. Now `replay`
/// answers `Ready` at once, and the rerun's stdin is the copy already held,
/// then each further byte as the copier keeps it, ending when the caller
/// closes the pipe: the position the first attempt was in.
///
/// Nothing here is timed, and nothing depends on when the copier runs: the
/// writer is this test's own, so "still arriving" and "finished" are
/// states it sets. The rerun is a real `cat`, which writes what it reads as
/// it reads it, and its first half is read back before the second half is
/// written, so the test pins that the rerun was running, and reading, while
/// the prompt was still arriving.
#[test]
fn a_prompt_still_arriving_is_fed_to_a_rerun_as_it_arrives() {
    let prompt = b"the piped prompt, in two halves".to_vec();
    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut policy = captured(fs::File::from(OwnedFd::from(reader)));
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    writer.write_all(&prompt[..10]).unwrap();
    // The first attempt has exited: its end of the prompt pipe is closed,
    // as it would be once the supervisor is back from running it.
    drop(first);

    // The caller still holds the pipe. The rerun is not refused and not
    // made to wait: it can start now.
    assert_eq!(policy.replay(), Replay::Ready);
    assert!(
        !policy.prompt_cut_short(),
        "nothing is lost: it is not finished"
    );
    let mut rerun = Command::new("/bin/cat")
        .stdin(policy.for_attempt(2).unwrap())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = rerun.stdout.take().unwrap();
    let mut head = vec![0u8; 10];
    out.read_exact(&mut head).unwrap();
    assert_eq!(head, prompt[..10], "the copy already held, at once");

    // The caller finishes. The rerun is given the rest, then the end of
    // its stdin, and reads the whole prompt in order.
    writer.write_all(&prompt[10..]).unwrap();
    drop(writer);
    let mut rest = Vec::new();
    out.read_to_end(&mut rest).unwrap();
    assert!(rerun.wait().unwrap().success());
    assert_eq!(rest, prompt[10..], "then the rest");
    assert_eq!(policy.replay(), Replay::Ready, "and stays whole");
    assert_eq!(settled(&mut policy), prompt, "a later rerun reads it all");
}

/// A rerun's stdin never ends early while the
/// caller holds the pipe, and a rerun that does not read it costs nothing.
///
/// The rerun here never reads: its stdin is dropped at once, as an agent
/// that exits without reading its prompt drops it. The step must not be
/// held by that, and nothing here may block: `for_attempt` returns at once
/// with the caller still holding the pipe, and so does `settle`, which
/// joins the copier only once the caller has closed it.
#[test]
fn a_rerun_starts_at_once_while_the_caller_holds_the_pipe() {
    let (reader, writer) = std::io::pipe().unwrap();
    let mut policy = captured(fs::File::from(OwnedFd::from(reader)));
    let first = policy.for_attempt(1).unwrap();
    policy.attempt_started();
    drop(first);
    // Nothing written, nothing closed.
    let rerun = policy.for_attempt(2).expect("a rerun starts at once");
    drop(rerun);
    policy.settle();
    assert!(!policy.prompt_cut_short(), "{policy:?}");
    drop(writer);
}

/// Issue #2816: an attempt that closes its stdin before the whole prompt
/// has been delivered is reported, not passed over in silence.
///
/// The recipe runner says as much when it writes a task straight to an
/// agent that exits early. A supervised step takes that writer's place and
/// always drains it, so the runner's write always succeeds and its own
/// warning can no longer fire; the copier makes the report instead.
/// Nothing is lost by the close: the kept copy, and so every later attempt,
/// still receives all of it.
#[test]
fn an_attempt_that_closes_its_stdin_early_is_reported() {
    let prompt = awkward_prompt();
    let (mut source, feeder) = fed_pipe(prompt.clone());
    // The attempt's stdin, closed before a byte of it is read. The prompt
    // is larger than a pipe holds several times over, so the copier must
    // meet that closed end: not even the fork of a neighbouring test,
    // which briefly copies the dropped read end, can take more than one
    // pipeful before its exec closes the copy again.
    let (child_read, child_write) = std::io::pipe().unwrap();
    drop(child_read);

    let state = CopyState::default();
    copy_prompt(&mut source, &mut Some(child_write), &state).unwrap();
    feeder.join().unwrap();

    assert!(state.reader_is_gone(), "the early close is reported");
    assert!(state.is_complete(), "the copy still has the whole prompt");
    assert!(!state.was_cut_short(), "nor is this a prompt cut short");
    assert_eq!(kept(&state), prompt);
}

#[test]
fn the_other_kinds_of_stdin_behave_as_before_with_a_mutable_policy() {
    let mut inherit = RerunStdin::Inherit;
    let mut null = RerunStdin::Null;
    assert!(inherit.for_attempt(1).is_ok() && inherit.for_attempt(2).is_ok());
    assert_eq!(cat(null.for_attempt(2).unwrap()), b"");
}

/// The capture hands the child no descriptor of its own beyond the stdin it
/// was given, and that stdin cannot be written back to, so an attempt
/// cannot rewrite the prompt a later attempt will read.
///
/// The descriptor half is a floor rather than a guard: every handle here is
/// made through `std`, which sets `O_CLOEXEC` on all of them, so only a
/// move to raw `libc` could break it. It is asserted against a baseline
/// child taken in the same process, because what the test binary already
/// had open is not this module's doing and a CI runner hands its test
/// binaries descriptors a developer's shell does not.
#[cfg(target_os = "linux")]
#[test]
fn an_attempt_gets_only_its_own_stdin_and_cannot_write_to_it() {
    let list = |stdin: Stdio| -> std::collections::BTreeSet<String> {
        let out = Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null; ls /proc/$$/fd"])
            .stdin(stdin)
            .stdout(Stdio::piped())
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    let baseline = list(Stdio::null());
    let prompt = b"the stored prompt".to_vec();
    let (source, feeder) = fed_pipe(prompt.clone());
    let mut policy = captured(source);
    for n in 1..=2 {
        let stdin = policy.for_attempt(n).unwrap();
        policy.attempt_started();
        let fds = list(stdin);
        let extra: Vec<&String> = fds.difference(&baseline).collect();
        assert!(extra.is_empty(), "attempt {n} inherited {extra:?}");
    }
    feeder.join().unwrap();
    // A rerun's stdin is the read end of a pipe fed from memory: writing to
    // it fails and the kept prompt is untouched, so attempt 2 cannot change
    // what attempt 3 reads.
    let wrote = Command::new("/bin/sh")
        .args(["-c", "echo tampered >&0"])
        .stdin(policy.for_attempt(2).unwrap())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!wrote.success(), "a rerun's stdin must not be writable");
    assert_eq!(cat(policy.for_attempt(3).unwrap()), prompt);
}

/// The sweep still removes a `<pid>-0.in` an earlier version left behind:
/// this version keeps the captured prompt in memory and writes none.
#[test]
fn the_sweep_removes_orphaned_captures_only_for_dead_pids() {
    let (_t, dir) = rerun();
    let dead = dead_pid();
    let me = std::process::id();
    let orphan = dir.join(format!("{dead}-0.in"));
    let live = dir.join(format!("{me}-0.in"));
    let target = dir.join("target");
    let planted = dir.join(format!("{}-0.in", dead_pid()));
    fs::write(&orphan, b"x").unwrap();
    fs::write(&live, b"x").unwrap();
    fs::write(&target, b"keep").unwrap();
    symlink(&target, &planted).unwrap();

    sweep_rerun(&dir);

    assert!(!orphan.exists(), "an orphaned capture is removed");
    assert!(live.exists(), "a live process's capture is kept");
    assert!(fs::symlink_metadata(&planted).is_ok(), "symlinks are kept");
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}
