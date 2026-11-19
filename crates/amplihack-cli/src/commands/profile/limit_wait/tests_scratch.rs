//! Scratch directory and rerun stdin (issue #1618): `$HOME/.amplihack/rerun`
//! ownership and mode rules, what a rerun reads on stdin (a regular file
//! from where the step started, a terminal unchanged, anything else
//! `/dev/null`, never a held-open pipe), and the `rerun/` sweep.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_scratch;`.

use super::super::auto_switch::{rerun_owner, sweep_rerun};
use super::scratch::{CopyState, RerunStdin, capture_prompt, copy_prompt, home_rerun_dir};
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
// Contract: `capture_prompt(dir, source)` takes the rerun scratch directory
// and a duplicate of the step's stdin (a pipe). It returns
// `RerunStdin::Captured` once the copier runs, and `RerunStdin::Pipe` (with
// no byte of `source` consumed) when the capture file cannot be created.

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

fn capture_file(dir: &Path) -> PathBuf {
    dir.join(format!("{}-0.in", std::process::id()))
}

/// A capture sink that refuses every write, standing in for the `ENOSPC`
/// or `EIO` of a real file.
///
/// A read-only description of a real file, so the refusal is `EBADF` and it
/// is the same on every run. A pipe whose reader has been dropped refuses
/// too, but not reliably in this binary: these tests run beside others that
/// fork, and between a fork and its exec the child holds a copy of every
/// descriptor open at the time, the dropped read end included. A write in
/// that window succeeds, and the test then sees no loss to publish.
fn refusing_sink(dir: &Path) -> fs::File {
    let path = dir.join("read-only-sink");
    fs::write(&path, b"").unwrap();
    fs::File::open(&path).unwrap()
}

#[test]
fn every_attempt_of_a_captured_prompt_reads_the_same_bytes_from_offset_zero() {
    let (_t, dir) = rerun();
    let prompt = awkward_prompt();
    let (source, feeder) = fed_pipe(prompt.clone());
    let mut policy = capture_prompt(&dir, source);
    assert!(matches!(policy, RerunStdin::Captured(_)), "{policy:?}");
    // Attempt 1 reads the pipe while the copier is still pumping.
    assert_eq!(cat(policy.for_attempt(1).unwrap()), prompt, "attempt 1");
    feeder.join().unwrap();
    for n in [2, 3, 4] {
        assert_eq!(cat(policy.for_attempt(n).unwrap()), prompt, "attempt {n}");
    }
}

#[test]
fn an_attempt_that_never_reads_its_prompt_does_not_stop_the_capture() {
    let (_t, dir) = rerun();
    let prompt = awkward_prompt();
    let (source, feeder) = fed_pipe(prompt.clone());
    let mut policy = capture_prompt(&dir, source);
    // The child exits without reading: its stdin is closed at once, so the
    // copier's writes to it fail with EPIPE and are absorbed.
    drop(policy.for_attempt(1).unwrap());
    feeder.join().unwrap();
    assert_eq!(cat(policy.for_attempt(2).unwrap()), prompt);
}

#[test]
fn the_capture_file_is_private_named_by_pid_and_removed_on_drop() {
    let (_t, dir) = rerun();
    let (source, feeder) = fed_pipe(b"prompt".to_vec());
    let mut policy = capture_prompt(&dir, source);
    let path = capture_file(&dir);
    assert_eq!(mode(&path), 0o600);
    assert!(fs::symlink_metadata(&path).unwrap().is_file());
    cat(policy.for_attempt(1).unwrap());
    feeder.join().unwrap();
    assert!(path.exists(), "kept for reruns until the step is done");
    drop(policy);
    assert!(!path.exists(), "removed when the step returns");
}

#[test]
fn the_capture_file_is_removed_when_the_step_ends_in_an_error() {
    fn step(dir: &Path) -> Result<(), String> {
        let (source, _feeder) = fed_pipe(b"prompt".to_vec());
        let _policy = capture_prompt(dir, source);
        Err("the step failed".to_string())
    }
    let (_t, dir) = rerun();
    assert!(step(&dir).is_err());
    assert!(!capture_file(&dir).exists());
}

#[test]
fn a_capture_that_cannot_be_created_leaves_the_pipe_unread() {
    let (_t, dir) = rerun();
    // An existing file, and a planted symlink, both refuse `create_new`.
    for plant in ["file", "symlink"] {
        let path = capture_file(&dir);
        let victim = dir.join("victim");
        fs::write(&victim, b"keep").unwrap();
        match plant {
            "file" => fs::write(&path, b"other").unwrap(),
            _ => symlink(&victim, &path).unwrap(),
        }
        let (source, feeder) = fed_pipe(b"prompt".to_vec());
        let reader = source.try_clone().unwrap();
        let policy = capture_prompt(&dir, source);
        assert!(matches!(policy, RerunStdin::Pipe), "{plant}: {policy:?}");
        feeder.join().unwrap();
        let mut left = Vec::new();
        let mut reader = reader;
        reader.read_to_end(&mut left).unwrap();
        assert_eq!(left, b"prompt", "{plant}: no byte consumed");
        assert_eq!(
            fs::read(&victim).unwrap(),
            b"keep",
            "{plant}: no write-through"
        );
        assert!(fs::symlink_metadata(&path).is_ok(), "{plant}: left alone");
        drop(policy);
        assert!(
            fs::symlink_metadata(&path).is_ok(),
            "{plant}: not ours to remove"
        );
        fs::remove_file(&path).unwrap();
        fs::remove_file(&victim).unwrap();
    }
}

#[test]
fn a_failed_copy_is_an_error_for_a_rerun_never_a_partial_prompt() {
    let (_t, dir) = rerun();
    // A write-only descriptor: reading it fails (EBADF), so the copier ends
    // without having seen end of file.
    let source = fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let mut policy = capture_prompt(&dir, source);
    assert!(matches!(policy, RerunStdin::Captured(_)), "{policy:?}");
    // Issue #2816: the loss is readable without joining the copier, which
    // is what lets the supervisor refuse the rerun before the wait rather
    // than after it. Nothing here is timed: the copier publishes the loss
    // before it drops the attempt's pipe, so draining that pipe to its end
    // puts this test after the publication rather than beside it.
    assert_eq!(cat(policy.for_attempt(1).unwrap()), b"");
    assert!(policy.capture_lost(), "published without a join");
    // The refusal says why, so a caller is never left to guess.
    let why = policy.for_attempt(2).unwrap_err().to_string();
    assert!(why.contains("was not stored whole"), "{why}");
    assert!(why.contains("attempt 2"), "{why}");
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
    let (_t, dir) = rerun();
    // A write-only descriptor: the first read of it fails (EBADF).
    let source = fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let mut policy = capture_prompt(&dir, source);
    assert_eq!(
        cat(policy.for_attempt(1).unwrap()),
        b"",
        "the attempt is given the end of file it cannot tell from a prompt"
    );
    assert!(
        policy.prompt_cut_short(),
        "published before the attempt could reach that end of file"
    );
    assert!(
        policy.capture_lost(),
        "a prompt cut short is a lost capture as well"
    );
    assert!(policy.for_attempt(2).is_err(), "no partial replay");
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
/// whatever had arrived — here, nothing at all.
///
/// Nothing is timed and nothing is assumed about scheduling. The source
/// reports each `EAGAIN`, and not one byte is written until a report has
/// arrived, so the retry is always the path taken: against the previous
/// behaviour the copier has already given up by then, and the join below
/// returns that error.
#[test]
fn an_empty_non_blocking_source_is_waited_on_not_taken_for_the_end() {
    let (_t, dir) = rerun();
    let capture = dir.join("capture");
    let sink = fs::File::create(&capture).unwrap();
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
        std::thread::spawn(move || copy_prompt(&mut source, sink, child_write, &published));

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
    assert_eq!(fs::read(&capture).unwrap(), prompt, "and the capture's");
    assert!(
        !state.was_cut_short(),
        "an empty non-blocking pipe is not a prompt that failed"
    );
    assert!(!state.is_lost(), "nor a capture that was lost");
    assert!(state.is_complete(), "the whole prompt reached both sinks");
    assert!(
        !state.reader_is_gone(),
        "the attempt read its prompt to the end"
    );
}

/// Issue #2816: the copier publishes a lost capture while it is still
/// draining the source, not only when it ends. The supervisor reads that
/// flag when it decides whether a rerun is possible, which is before the
/// wait; a flag set only at the end would be read only after one.
#[test]
fn a_lost_capture_is_published_while_the_copy_is_still_running() {
    let (_t, dir) = rerun();
    // A source the test holds open, so the copier cannot reach end of file.
    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut source = fs::File::from(OwnedFd::from(reader));
    // A capture sink that fails on the first write (see `refusing_sink`).
    let sink = refusing_sink(&dir);
    // The attempt's stdin, read here rather than on a thread: the copier
    // tries the capture file and publishes what became of it before it
    // forwards a block to the attempt, so reading that block back is proof
    // the flag is already set. No clock, and no join.
    let (mut child_read, child_write) = std::io::pipe().unwrap();

    let state = Arc::new(CopyState::default());
    let published = Arc::clone(&state);
    let copier =
        std::thread::spawn(move || copy_prompt(&mut source, sink, child_write, &published));

    let block = b"the first block of the prompt";
    writer.write_all(block).unwrap();
    let mut got = vec![0u8; block.len()];
    child_read.read_exact(&mut got).unwrap();
    assert_eq!(got, block, "the attempt still received every byte");
    assert!(
        state.is_lost(),
        "the loss is published before that block is"
    );
    assert!(!state.is_complete(), "a lost capture is never complete");
    assert!(
        !state.was_cut_short(),
        "the source is being read fine: only the capture was lost"
    );

    // Only now is the source closed, so the assertions above held while the
    // copier was still running.
    drop(writer);
    assert!(copier.join().unwrap().is_err());
    let mut rest = Vec::new();
    child_read.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "the block was all that was written");
}

/// Issue #2816: `hand_back` is only for the one kind of stdin the
/// supervisor takes bytes off. For every other kind fd 0 is the step's own
/// descriptor, untouched, and handing it back is nothing at all.
#[test]
fn handing_stdin_back_does_nothing_when_nothing_was_taken() {
    let before = fs::read_link("/proc/self/fd/0").ok();
    let file = fs::File::open("/dev/null").unwrap();
    for mut policy in [
        RerunStdin::Inherit,
        RerunStdin::Pipe,
        RerunStdin::Null,
        RerunStdin::File { file, start: 0 },
    ] {
        assert!(!policy.was_drained(), "{policy:?}");
        assert!(!policy.capture_lost(), "{policy:?}");
        policy.hand_back().unwrap();
    }
    assert_eq!(
        fs::read_link("/proc/self/fd/0").ok(),
        before,
        "fd 0 was replaced for a stdin that was never drained"
    );
}

/// Issue #2816: a capture says so, which is how the supervisor knows that
/// standing down would hand a plain launch a stdin it has already eaten.
#[test]
fn a_capture_reports_that_stdin_was_drained() {
    let (_t, dir) = rerun();
    let (source, feeder) = fed_pipe(b"the piped prompt".to_vec());
    let mut policy = capture_prompt(&dir, source);
    assert!(policy.was_drained(), "{policy:?}");
    assert!(!policy.capture_lost(), "{policy:?}");
    feeder.join().unwrap();
    assert_eq!(cat(policy.for_attempt(2).unwrap()), b"the piped prompt");
    assert!(!policy.capture_lost(), "a whole capture is not lost");
}

/// The capture is secondary to the attempt in hand: a file that cannot be
/// written costs the rerun, never a byte of the first attempt's prompt.
///
/// The capture sink here refuses every write (see [`refusing_sink`]), so
/// the first chunk is already lost. The prompt is larger than `COPY_BUF`,
/// so the copy runs on for several chunks after that.
#[test]
fn a_capture_that_cannot_be_written_still_hands_the_attempt_its_whole_prompt() {
    let (_t, dir) = rerun();
    let prompt = awkward_prompt();
    let (mut source, feeder) = fed_pipe(prompt.clone());
    let sink = refusing_sink(&dir);

    // The attempt's stdin, drained as the copier writes it.
    let (mut child_read, child_write) = std::io::pipe().unwrap();
    let drain = std::thread::spawn(move || {
        let mut got = Vec::new();
        child_read.read_to_end(&mut got).unwrap();
        got
    });

    let state = CopyState::default();
    let err = copy_prompt(&mut source, sink, child_write, &state);

    assert!(err.is_err(), "the lost capture is reported: {err:?}");
    assert_eq!(drain.join().unwrap(), prompt, "the attempt's whole prompt");
    assert!(
        !state.is_complete(),
        "an unwritten capture is never complete, so a rerun is refused"
    );
    assert!(
        state.is_lost(),
        "issue #2816: the loss is published for a decision taken without joining"
    );
    assert!(
        !state.was_cut_short(),
        "the source was read to its end: the attempt's prompt is whole"
    );
    // Last: a copier that stopped early would leave the feeder blocked on a
    // pipe nobody is draining, and the assertions above say so first.
    feeder.join().unwrap();
}

/// Issue #2816: an attempt that closes its stdin before the whole prompt
/// has been delivered is reported, not passed over in silence.
///
/// The recipe runner says as much when it writes a task straight to an
/// agent that exits early. A supervised step takes that writer's place and
/// always drains it, so the runner's write always succeeds and its own
/// warning can no longer fire; the copier makes the report instead.
/// Nothing is lost by the close: the capture, and so every later attempt,
/// still receives all of it.
#[test]
fn an_attempt_that_closes_its_stdin_early_is_reported() {
    let (_t, dir) = rerun();
    let capture = dir.join("capture");
    let sink = fs::File::create(&capture).unwrap();
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
    copy_prompt(&mut source, sink, child_write, &state).unwrap();
    feeder.join().unwrap();

    assert!(state.reader_is_gone(), "the early close is reported");
    assert!(
        state.is_complete(),
        "the capture still has the whole prompt"
    );
    assert!(
        !state.is_lost(),
        "an attempt that stops reading costs no rerun"
    );
    assert!(!state.was_cut_short(), "nor is this a prompt cut short");
    assert_eq!(fs::read(&capture).unwrap(), prompt);
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
/// made through `std`, which sets `O_CLOEXEC` on all of them whatever
/// `custom_flags` says, so only a move to raw `libc` could break it. It is
/// asserted against a baseline child taken in the same process, because
/// what the test binary already had open is not this module's doing and a
/// CI runner hands its test binaries descriptors a developer's shell does
/// not.
#[cfg(target_os = "linux")]
#[test]
fn an_attempt_gets_only_its_own_stdin_and_cannot_write_to_it() {
    let (_t, dir) = rerun();
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
    let mut policy = capture_prompt(&dir, source);
    for n in 1..=2 {
        let fds = list(policy.for_attempt(n).unwrap());
        let extra: Vec<&String> = fds.difference(&baseline).collect();
        assert!(extra.is_empty(), "attempt {n} inherited {extra:?}");
    }
    feeder.join().unwrap();
    // A rerun's stdin is the capture opened read-only: writing to it fails
    // and the stored prompt is untouched, so attempt 2 cannot change what
    // attempt 3 reads.
    let wrote = Command::new("/bin/sh")
        .args(["-c", "echo tampered >&0"])
        .stdin(policy.for_attempt(2).unwrap())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!wrote.success(), "a rerun's stdin must not be writable");
    assert_eq!(fs::read(capture_file(&dir)).unwrap(), prompt);
    assert_eq!(cat(policy.for_attempt(3).unwrap()), prompt);
}

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
