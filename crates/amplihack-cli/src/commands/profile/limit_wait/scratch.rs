//! Scratch and stdin of a supervised step (issue #1618): the home scratch
//! directory `$HOME/.amplihack/rerun/`, and what each attempt reads on
//! stdin.
//!
//! The supervisor's own thread never reads stdin, so a pipe the step
//! inherited and nobody closes cannot hold it up. One bounded exception
//! exists since issue #2815: when the step's prompt arrives on a pipe
//! rather than in argv, [`capture_prompt`] starts a copier thread that
//! pumps that pipe into a private `<pid>-0.in` file in the rerun directory
//! and into the first attempt's own stdin pipe. The copier stops at end of
//! file, absorbs the first attempt's `EPIPE`, and is joined only when a
//! rerun needs the stored prompt; the supervisor's main loop never waits
//! on it. Of the two sinks the attempt's comes first in standing: a file
//! that cannot be written costs the rerun, never a byte of the prompt the
//! attempt in hand is reading.
//!
//! The two sinks fail differently because only one of those failures can be
//! told to the attempt. A capture file that cannot be written costs the
//! rerun alone. A source that cannot be read costs the prompt itself, and
//! the copier's pipe has no way to say so: closing it is the only signal a
//! pipe gives its reader, and a reader takes it for the end of the prompt.
//! So a failed read of the source is published as a prompt cut short (issue
//! #2816) — named on stderr the moment it happens, and read by the
//! supervisor before it passes an attempt's success up as the step's.
//!
//! The first attempt therefore inherits stdin unchanged, except for a
//! captured prompt, which it reads from the copier's pipe. A rerun gets a
//! regular file again from where the step started reading it (on Linux
//! through its own open of `/proc/self/fd/0`, so no read offset is shared
//! with other processes), the capture file from offset 0 when there is one,
//! a terminal unchanged, and anything else (a pipe with no capture, a
//! socket, a closed descriptor) `/dev/null`: what the first attempt read
//! from it is gone.
//!
//! One path leaves the supervisor altogether. When it cannot go on and no
//! attempt has run, the caller launches the step once in this process on
//! the stdin it inherited; a copier that has already started would have
//! eaten part of that stdin. [`RerunStdin::hand_back`] therefore puts the
//! stored prompt back on this process's own fd 0 before the supervisor
//! stands down, and reports an error rather than let the plain launch read
//! a prompt the copier has taken (issue #2816).

use super::super::fsx;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// `$HOME/.amplihack/rerun`, checked or created: `$HOME/.amplihack` must be
/// a real directory owned by the current user that group and others cannot
/// write (created at `0700` when missing, never `chmod`ed), and `rerun/`
/// must be a private (`0700`) directory owned by the current user.
pub(crate) fn home_rerun_dir(home: &Path) -> io::Result<PathBuf> {
    let dot = home.join(".amplihack");
    match fs::symlink_metadata(&dot) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fsx::ensure_private_dir(&dot)?,
        Err(e) => return Err(e),
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(fsx::invalid("~/.amplihack is not a real directory"));
        }
        Ok(m) if m.uid() != fsx::euid() => {
            return Err(fsx::invalid("~/.amplihack is owned by another user"));
        }
        Ok(m) if fsx::writable_by_others(&m) => {
            return Err(fsx::invalid("~/.amplihack is writable by other users"));
        }
        Ok(_) => {}
    }
    let rerun = dot.join("rerun");
    fsx::ensure_private_dir(&rerun)?;
    Ok(rerun)
}

/// What each attempt reads on stdin.
#[derive(Debug)]
pub(crate) enum RerunStdin {
    /// A terminal, or the platform cannot tell: passed through unchanged.
    Inherit,
    /// A regular file: a duplicate of the step's descriptor and the offset
    /// the step started at.
    File { file: fs::File, start: u64 },
    /// A pipe or socket with no capture: `/dev/null` for a rerun, as its
    /// contents went to attempt 1.
    Pipe,
    /// A closed descriptor: `/dev/null`.
    Null,
    /// The step's prompt, copied aside as it is delivered (issue #2815).
    /// Every attempt reads the same bytes.
    Captured(Capture),
}

impl RerunStdin {
    /// Looks at this process's stdin without reading it.
    pub(crate) fn capture() -> Self {
        let stdin = io::stdin();
        let Ok(owned) = stdin.as_fd().try_clone_to_owned() else {
            return RerunStdin::Null; // closed
        };
        Self::from_file(fs::File::from(owned))
    }

    /// The policy for `file`, a duplicate of the step's stdin.
    pub(crate) fn from_file(mut file: fs::File) -> Self {
        let Ok(meta) = file.metadata() else {
            return RerunStdin::Null;
        };
        if meta.is_file() {
            return match file.stream_position() {
                Ok(start) => RerunStdin::File { file, start },
                Err(_) => RerunStdin::Null,
            };
        }
        use std::os::unix::fs::FileTypeExt;
        if meta.file_type().is_char_device() {
            // A terminal, or /dev/null itself: never waits for input that
            // a held-open pipe would withhold.
            return RerunStdin::Inherit;
        }
        RerunStdin::Pipe
    }

    /// The stdin for attempt `n` (from 1).
    pub(crate) fn for_attempt(&mut self, n: u32) -> io::Result<Stdio> {
        match self {
            // A captured prompt is the only kind every attempt reads, so
            // it comes before the `n == 1` arm below.
            RerunStdin::Captured(c) => c.for_attempt(n),
            _ if n == 1 => Ok(Stdio::inherit()),
            RerunStdin::Inherit => Ok(Stdio::inherit()),
            RerunStdin::File { file, start } => {
                let mut f = reopen(file).map_or_else(|| file.try_clone(), Ok)?;
                f.seek(SeekFrom::Start(*start))?;
                Ok(Stdio::from(f))
            }
            RerunStdin::Pipe | RerunStdin::Null => Ok(Stdio::null()),
        }
    }

    /// Whether a copier has already taken bytes off this process's own
    /// stdin, so that handing fd 0 to a plain launch would shorten its
    /// prompt (issue #2816). True for a capture whatever became of it,
    /// including one that was set up and then failed.
    pub(crate) fn was_drained(&self) -> bool {
        matches!(self, RerunStdin::Captured(_))
    }

    /// Whether the stored prompt is already known to be unusable, asked
    /// without waiting on the copier (issue #2816).
    ///
    /// The copier learns within moments that the capture file cannot be
    /// written, but [`Capture::for_attempt`] only asks when a rerun is
    /// already due, which is after the wait. This is the question a
    /// decision can afford: `false` for every other kind of stdin, and for
    /// a capture still going well or already complete.
    pub(crate) fn capture_lost(&self) -> bool {
        matches!(self, RerunStdin::Captured(c) if c.lost())
    }

    /// Whether the prompt the attempt in hand was given stops short of the
    /// prompt that was sent, because a read of the source failed while the
    /// copier was forwarding it (issue #2816).
    ///
    /// Asked without joining the copier, and for the case that matters it
    /// cannot be missed: the flag is stored before the attempt's pipe is
    /// dropped, the attempt reaches end of file only once that pipe is
    /// dropped, and the supervisor asks this only after the attempt has
    /// exited. `false` for every other kind of stdin, and for a capture
    /// whose source is still being read or has ended properly — including
    /// one whose capture file was lost, which costs the rerun and not the
    /// prompt.
    pub(crate) fn prompt_cut_short(&self) -> bool {
        matches!(self, RerunStdin::Captured(c) if c.cut_short())
    }

    /// Puts the whole stored prompt back on this process's fd 0, for the
    /// plain launch that follows the supervisor standing down (issue
    /// #2816).
    ///
    /// Only a capture has taken anything off fd 0: every other kind left
    /// the step's own descriptor in place, so this does nothing for them
    /// and the launch inherits stdin exactly as it always did. For a
    /// capture the copier is settled first, so the prompt is whole on
    /// disk, and the read-only description opened at creation — already
    /// matched to the writer's inode — is duplicated onto fd 0 at offset
    /// 0. The capture file is unlinked when the [`Capture`] is dropped,
    /// moments later; the open description on fd 0 keeps the bytes
    /// readable for the launch, so nothing is left in the rerun directory
    /// while the step runs.
    ///
    /// An error here means the prompt cannot be given back whole. The
    /// caller must not run the step: its stdin is short by whatever the
    /// copier read.
    pub(crate) fn hand_back(&mut self) -> io::Result<()> {
        let RerunStdin::Captured(c) = self else {
            return Ok(());
        };
        let whole = c.whole()?;
        dup_onto_stdin(&whole)
    }
}

/// Duplicates `file` onto fd 0, leaving `file`'s own descriptor alone.
///
/// `dup2` clears close-on-exec on the descriptor it makes, so the launch
/// that follows inherits it across an `exec` as well as across a spawn.
fn dup_onto_stdin(file: &fs::File) -> io::Result<()> {
    // SAFETY: `file` is an open descriptor this process owns, and fd 0 is
    // replaced in one step; no other thread of this process is reading or
    // closing fd 0 by the time this runs, because the only one that ever
    // read it was the copier, which `whole` has just joined.
    let rc = unsafe { libc::dup2(file.as_raw_fd(), libc::STDIN_FILENO) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// On Linux, a fresh open of the step's stdin through `/proc/self/fd/0`,
/// with its own read offset, when it is still the same file as `file`.
/// `None` elsewhere or on any failure: the caller seeks a duplicate instead.
fn reopen(file: &fs::File) -> Option<fs::File> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let fresh = fs::File::open("/proc/self/fd/0").ok()?;
    let (a, b) = (fresh.metadata().ok()?, file.metadata().ok()?);
    (a.is_file() && a.dev() == b.dev() && a.ino() == b.ino()).then_some(fresh)
}

// ---------------------------------------------------------------------------
// The captured prompt (issue #2815)
// ---------------------------------------------------------------------------

/// How much of the prompt the copier moves at a time.
const COPY_BUF: usize = 64 * 1024;

/// What the copier publishes about its own progress while it is still
/// running, so that a decision can read it without joining the copier
/// (issue #2816). A join waits for the writer to close the pipe, which is a
/// wait of unknown length and the wrong one to take before a decision.
#[derive(Debug, Default)]
pub(super) struct CopyState {
    /// Set only after end of file and a complete write of both sinks.
    complete: AtomicBool,
    /// Set the moment the capture is known to be lost, which is well
    /// before the copier itself ends: a write to the capture file failed,
    /// or the source did. A rerun is refused on this while the limit
    /// result can still be passed up.
    lost: AtomicBool,
    /// Set the moment a read of the source fails, which means the attempt
    /// in hand is being given less than the prompt that was sent. Always
    /// set together with `lost`, and always before the attempt's pipe is
    /// dropped.
    cut_short: AtomicBool,
}

impl CopyState {
    /// The whole prompt reached both sinks.
    fn done(&self) {
        self.complete.store(true, Ordering::SeqCst);
    }

    pub(super) fn is_complete(&self) -> bool {
        self.complete.load(Ordering::SeqCst)
    }

    /// The capture file is gone, so a rerun would have nothing to read.
    /// The attempt in hand still gets every byte.
    fn lose(&self) {
        self.lost.store(true, Ordering::SeqCst);
    }

    pub(super) fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// The source failed, so the prompt stops here: the capture is lost,
    /// and the attempt in hand will reach the end of its stdin without
    /// having been given the rest of its task.
    ///
    /// Both flags are stored before this returns, and the copier drops the
    /// attempt's pipe only afterwards, so nothing that can observe that end
    /// of file can observe these unset. The line on stderr is the report
    /// that survives whatever the supervisor does next — an attempt that
    /// succeeds and is never asked about again included. It names the error
    /// and no part of the prompt.
    fn cut_short(&self, e: &io::Error) {
        self.lost.store(true, Ordering::SeqCst);
        self.cut_short.store(true, Ordering::SeqCst);
        eprintln!(
            "amplihack: this step's prompt could not be read to its end ({e}), so the \
             attempt reading it is given only what arrived before that: a prompt cut \
             short, which it cannot tell from a whole one"
        );
    }

    pub(super) fn was_cut_short(&self) -> bool {
        self.cut_short.load(Ordering::SeqCst)
    }
}

/// One copy of the step's piped prompt.
///
/// A recipe agent step is given its prompt on stdin, so before issue #2815
/// a usage limit in such a step could neither be waited out nor rerun under
/// another profile: a second attempt had nothing to read. The copier thread
/// started by [`capture_prompt`] pumps that pipe into a private file in the
/// rerun scratch directory *and* into the pipe the first attempt reads, so
/// the first attempt is not delayed and every later attempt gets the same
/// bytes from offset 0.
///
/// The file is removed when the step returns, on success and on error.
/// `sweep_rerun` removes one left behind by a supervisor that was killed.
pub(crate) struct Capture {
    /// `<rerun dir>/<pid>-0.in`, this supervisor's own name.
    path: PathBuf,
    /// A read-only description of `path`, opened before any byte was
    /// written and checked against the writer's inode, so a rerun never
    /// re-opens the path and never races a same-user replacement.
    reader: fs::File,
    /// The read end of the first attempt's stdin pipe, until it is handed
    /// to that attempt.
    first: Option<fs::File>,
    /// The copier, until a rerun joins it.
    copier: Option<JoinHandle<io::Result<()>>>,
    /// What the copier publishes as it runs, read without joining it
    /// (issue #2816).
    state: Arc<CopyState>,
    /// Why the capture cannot be replayed, once that is known.
    failure: Option<String>,
}

impl std::fmt::Debug for Capture {
    /// Never prints the prompt, nor the capture path, which no log line
    /// carries either.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("complete", &self.state.is_complete())
            .field("failed", &self.lost())
            .field("cut_short", &self.cut_short())
            .finish()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Capture {
    /// The stdin for attempt `n` (from 1): the copier's pipe for the first
    /// attempt, the completed capture from offset 0 for every later one.
    fn for_attempt(&mut self, n: u32) -> io::Result<Stdio> {
        if n == 1
            && let Some(first) = self.first.take()
        {
            return Ok(Stdio::from(first));
        }
        match self.whole() {
            Ok(f) => Ok(Stdio::from(f)),
            Err(e) => Err(io::Error::other(format!(
                "{e}, so it cannot be given to attempt {n}"
            ))),
        }
    }

    /// The whole stored prompt, read-only and at offset 0, once the copier
    /// has finished with it.
    ///
    /// Joins the copier (see [`Capture::settle`]) and refuses a capture
    /// that is short or failed: a partial prompt is a shortened task, and
    /// no caller may be given one.
    fn whole(&mut self) -> io::Result<fs::File> {
        self.settle();
        if let Some(why) = &self.failure {
            return Err(io::Error::other(format!(
                "this step's prompt was not stored whole: {why}"
            )));
        }
        let mut f = self.reader.try_clone()?;
        f.seek(SeekFrom::Start(0))?;
        Ok(f)
    }

    /// Whether the capture is already known to be unusable, without
    /// joining the copier (issue #2816).
    fn lost(&self) -> bool {
        self.failure.is_some() || self.state.is_lost()
    }

    /// Whether the attempt in hand was given a prompt that stops short of
    /// the one that was sent (issue #2816).
    fn cut_short(&self) -> bool {
        self.state.was_cut_short()
    }

    /// Waits for the copier the first time a rerun needs the whole prompt,
    /// and records why it cannot be replayed when that is the answer.
    ///
    /// The first attempt's pipe is dropped first: a child that exited
    /// without reading its prompt would otherwise leave the copier blocked
    /// on a full pipe nobody drains. With the read end gone those writes
    /// fail with `EPIPE`, which the copier absorbs, and it runs on to end
    /// of file.
    fn settle(&mut self) {
        self.first = None;
        let Some(copier) = self.copier.take() else {
            return;
        };
        self.failure = match copier.join() {
            Ok(Ok(())) if self.state.is_complete() => None,
            Ok(Ok(())) => Some("it ended before the end of the input".to_string()),
            Ok(Err(e)) => Some(format!("copying it failed: {e}")),
            Err(_) => Some("the thread copying it panicked".to_string()),
        };
    }
}

/// Copies this process's stdin aside so every attempt can read it.
///
/// `None` when stdin cannot be duplicated; the caller keeps the policy it
/// already had.
pub(crate) fn capture_own_prompt(dir: &Path) -> Option<RerunStdin> {
    let owned = io::stdin().as_fd().try_clone_to_owned().ok()?;
    Some(capture_prompt(dir, fs::File::from(owned)))
}

/// Starts a [`Capture`] in `dir` from `source`, a duplicate of the step's
/// stdin.
///
/// Returns [`RerunStdin::Pipe`] — today's conduct, no rerun and no wait —
/// when the capture cannot be set up, having read no byte of `source`: the
/// file is created, and the first attempt's pipe made, before anything is
/// consumed, so a failure costs no prompt bytes. Nothing here refuses a
/// prompt for its size or its contents.
pub(crate) fn capture_prompt(dir: &Path, source: fs::File) -> RerunStdin {
    match start_capture(dir, source) {
        Ok(c) => RerunStdin::Captured(c),
        Err(e) => {
            eprintln!(
                "amplihack: this step's prompt could not be stored for a rerun, so a usage \
                 limit will end it: {e}"
            );
            RerunStdin::Pipe
        }
    }
}

fn start_capture(dir: &Path, source: fs::File) -> io::Result<Capture> {
    let path = dir.join(format!("{}-0.in", std::process::id()));
    // As `profile::attempt` creates a step's output buffer: private at
    // creation under the umask, never a symlink, never inherited by a
    // child. The 0700 rerun directory is neither created nor relaxed here.
    let writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    // Anything that fails from here on takes the file away again, so a
    // half-made capture is never left for the sweep to find.
    start_copier(&path, writer, source).inspect_err(|_| {
        let _ = fs::remove_file(&path);
    })
}

/// The rest of [`start_capture`], once the file at `path` exists: open the
/// reader, make the first attempt's pipe, and start the copier.
fn start_copier(path: &Path, writer: fs::File, mut source: fs::File) -> io::Result<Capture> {
    // Opened before a byte is written and matched to the writer's inode,
    // so later attempts never re-open the path: a same-user replacement of
    // it cannot change what they read.
    let reader = read_back(path, &writer)?;
    // `std::io::pipe` is `O_CLOEXEC` on both ends, so only the descriptor
    // `Command` puts on fd 0 reaches the child.
    let (child_read, child_write) = std::io::pipe()?;
    let state = Arc::new(CopyState::default());
    let published = Arc::clone(&state);
    let copier = std::thread::Builder::new()
        .name("amplihack-prompt-capture".to_string())
        .spawn(move || copy_prompt(&mut source, writer, child_write, &published))?;
    Ok(Capture {
        path: path.to_path_buf(),
        reader,
        first: Some(fs::File::from(std::os::fd::OwnedFd::from(child_read))),
        copier: Some(copier),
        state,
        failure: None,
    })
}

/// A read-only description of the file just created at `path`, refused
/// unless it is still the very file `writer` holds: a regular file of this
/// user's, with one link.
fn read_back(path: &Path, writer: &fs::File) -> io::Result<fs::File> {
    let reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let (a, b) = (reader.metadata()?, writer.metadata()?);
    let same = a.is_file() && a.dev() == b.dev() && a.ino() == b.ino();
    if !same || a.uid() != fsx::euid() || a.nlink() != 1 {
        return Err(fsx::invalid(
            "the prompt capture file was replaced as it was created",
        ));
    }
    Ok(reader)
}

/// Pumps `source` into `file` and into `child`, the first attempt's stdin,
/// until end of file. `state` is marked complete only then.
///
/// The prompt is opaque bytes throughout: never decoded, never held whole
/// in memory and never printed. A first attempt that exits without reading
/// closes its end, and the `EPIPE` that follows only stops the copy to the
/// child; the file still receives all of it. Any error on the file is the
/// copy failing, and a rerun is refused rather than given part of a
/// prompt.
///
/// The capture is secondary to the attempt in hand. An error on the file
/// stops the copy to the file and nothing else: the source is still drained
/// to end of file and the child still gets every byte, because a prompt cut
/// short is a shortened task the attempt would act on, whereas a lost
/// capture only costs a rerun. The error is returned once the source is
/// spent, and the state is left incomplete, so a rerun is refused.
///
/// A failed read of the source is the other case, and the one the attempt
/// cannot be told about: there are no further bytes to forward, so the copy
/// ends for both sinks, and the attempt's pipe closes. Closing it is the
/// only signal a pipe gives, and a reader takes it for the end of the
/// prompt. [`CopyState::cut_short`] therefore publishes the loss, says so
/// on stderr the moment it happens, and the supervisor refuses to report
/// the attempt's success as the step's (issue #2816). `EAGAIN` is not such
/// a failure: an inherited descriptor may carry `O_NONBLOCK` on a
/// description shared with the writer, and this waits for the next byte as
/// a blocking read would have.
///
/// Either loss is published the moment it is known — a write to the file
/// failed, or the source did — which for a file error is well before the
/// drain that still has to finish (issue #2816). An attempt that stopped
/// reading is not a loss and never publishes one. The supervisor reads
/// these flags when it decides whether a rerun is possible at all, so a
/// capture already known to be lost refuses the rerun then and there,
/// rather than after a wait of hours.
pub(super) fn copy_prompt(
    source: &mut fs::File,
    mut file: fs::File,
    child: std::io::PipeWriter,
    state: &CopyState,
) -> io::Result<()> {
    let mut child = Some(child);
    let mut file_err: Option<io::Error> = None;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let k = match source.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // Not the end of the prompt, and not a failure of it: an empty
            // non-blocking pipe refuses the read rather than waiting. Wait
            // for the next byte and ask again. A writer that closes
            // instead makes the next read return 0, and a real error makes
            // it return that error, so neither is swallowed here.
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => match wait_readable(source) {
                Ok(()) => continue,
                Err(e) => {
                    state.cut_short(&e);
                    return Err(e);
                }
            },
            // There are no further bytes to forward, so the source failing
            // ends the copy for both sinks. The attempt's pipe closes as
            // this returns, which that attempt reads as the end of its
            // prompt; `cut_short` is what keeps that from passing for a
            // whole one.
            Err(e) => {
                state.cut_short(&e);
                return Err(e);
            }
        };
        if file_err.is_none()
            && let Err(e) = file.write_all(&buf[..k])
        {
            // The capture is lost, so a rerun is refused. The attempt in
            // hand still gets its whole prompt.
            file_err = Some(e);
            state.lose();
        }
        if let Some(w) = &mut child
            && w.write_all(&buf[..k]).is_err()
        {
            // The attempt is gone, or stopped reading. Its prompt is still
            // stored; closing our end lets it see end of file if it is
            // somehow still there.
            child = None;
        }
    }
    if let Some(e) = file_err {
        return Err(e);
    }
    if let Err(e) = file.flush() {
        state.lose();
        return Err(e);
    }
    state.done();
    Ok(())
}

/// Waits until `source` has something to read, after a read of it returned
/// `EAGAIN`.
///
/// Stdin is a descriptor this process inherited, and `O_NONBLOCK` lives on
/// the open file description, not on the descriptor: another process
/// sharing that description can set it, and then a read of an empty pipe
/// fails instead of waiting. Treating that as the end of the prompt would
/// shorten the task; `poll(2)` with no timeout waits exactly as the
/// blocking read would have. `POLLHUP` and `POLLERR` both satisfy the poll,
/// and the read that follows then returns the end of file or the error
/// itself.
fn wait_readable(source: &fs::File) -> io::Result<()> {
    let mut fds = libc::pollfd {
        fd: source.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one initialised `pollfd` naming a descriptor this
        // process owns for the whole of the call; `poll` writes only
        // `revents`.
        let rc = unsafe { libc::poll(&mut fds, 1, -1) };
        if rc >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}
