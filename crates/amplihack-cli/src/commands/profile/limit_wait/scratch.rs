//! Scratch and stdin of a supervised step (issue #1618): the home scratch
//! directory `$HOME/.amplihack/rerun/`, and what each attempt reads on
//! stdin.
//!
//! The supervisor's own thread never reads stdin, so a pipe the step
//! inherited and nobody closes cannot hold it up. One bounded exception
//! exists since issue #2815: when the step's prompt arrives on a pipe
//! rather than in argv, [`capture_prompt`] prepares a copier that pumps
//! that pipe into a private `<pid>-0.in` file in the rerun directory and
//! into the first attempt's own stdin pipe, so a later attempt can read the
//! same bytes. Of the two sinks the attempt's comes first in standing: a
//! file that cannot be written costs the rerun, never a byte of the prompt
//! the attempt in hand is reading.
//!
//! Two rules keep that copier from ever holding the step up, because the
//! pipe it reads belongs to the caller and nothing promises the caller will
//! close it (issue #2816).
//!
//! The first is that the copier takes nothing off fd 0 until there is a
//! child to give it to: [`RerunStdin::attempt_started`] starts the thread,
//! and the supervisor calls that only once the first attempt has been
//! spawned. Every path that stands down and lets a plain in-process launch
//! run the step instead is reached before that, so the plain launch
//! inherits fd 0 with the whole prompt still on it and there is nothing to
//! give back ([`RerunStdin::stand_down`]). It also means an `EPIPE` from
//! the attempt's end of the pipe is always a real attempt that stopped
//! reading, which is why the copier may report it.
//!
//! The second is that the copier is joined only when it is certain to end:
//! it has reached the end of the source, it has already failed, or the last
//! writer of the source has gone and what is left to read is bounded by
//! what the kernel is holding. When the prompt is still arriving, a rerun
//! is told so ([`Replay::Arriving`]) and asks again later, rather than
//! joining a copier that may never end. Nothing here is timed: the question
//! asked is the state of the descriptor, not the clock.
//!
//! The two sinks fail differently because only one of those failures can be
//! told to the attempt. A capture file that cannot be created or written
//! costs nothing a rerun needs (issue #3036): the copy goes on in memory
//! from the first byte the disk refused, and a rerun is given the part the
//! disk holds followed by the part memory holds. Memory is used only after
//! the disk has refused the copy, and is not capped, because a cap would be
//! a budget that ends a step on a limit. A source that cannot be read costs
//! the prompt itself, and the copier's pipe has no way to say so: closing it
//! is the only signal a pipe gives its reader, and a reader takes it for the
//! end of the prompt. So a failed read of the source is published as a
//! prompt cut short (issue #2816), named on stderr the moment it happens,
//! and read by the supervisor before it passes an attempt's success up as
//! the step's. That is the one input no rerun can be given, and a step whose
//! prompt was cut short fails whether or not it meets a limit.
//!
//! The first attempt therefore inherits stdin unchanged, except for a
//! captured prompt, which it reads from the copier's pipe. A rerun gets a
//! regular file again from where the step started reading it (on Linux
//! through its own open of `/proc/self/fd/0`, so no read offset is shared
//! with other processes), the capture from offset 0 when there is one (the
//! capture file, or a pipe fed from memory when the disk refused the copy),
//! a terminal unchanged, and anything else (a pipe with no capture, a
//! socket, a closed descriptor) `/dev/null`: what the first attempt read
//! from it is gone.

use super::super::fsx;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What a rerun can be given of this step's input, asked at the moment one
/// is due (issues #2816, #3036).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Replay {
    /// The whole input is stored, or the input needs no storing: a rerun
    /// can read it.
    Ready,
    /// The input is still arriving on a pipe its caller has not closed, so
    /// there is no whole copy yet. Not recorded: asked again later, once
    /// the caller has finished writing, the answer is [`Replay::Ready`].
    Arriving,
    /// No rerun can ever be given this input, for the reason given. Only a
    /// prompt that could not be read to its end reaches this today; the
    /// reason is static text or an I/O error, never prompt bytes.
    Never(String),
}

/// Why [`Replay::Arriving`] is not a whole copy, for an attempt that is
/// handed the capture regardless.
const ARRIVING: &str = "this step's prompt is still arriving on a pipe its caller has not \
                        closed, so there is no whole copy of it to give a rerun";

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

    /// The first attempt's child exists and holds the other end of the
    /// prompt pipe, so the copier may start taking the prompt off this
    /// process's stdin (issue #2816).
    ///
    /// Nothing before this call reads a byte of fd 0. That is what lets a
    /// supervisor that cannot go on hand the whole of fd 0 to a plain
    /// launch, and what makes a write that fails on the attempt's end of
    /// the pipe mean an attempt that stopped reading rather than an attempt
    /// that never was. Called once per step, after the first attempt is
    /// spawned; it does nothing for any other kind of stdin, and nothing on
    /// a later attempt.
    pub(crate) fn attempt_started(&mut self) {
        if let RerunStdin::Captured(c) = self {
            c.arm();
        }
    }

    /// Whether this step's prompt is being copied aside, so that a rerun
    /// can be given it again (issue #2815).
    pub(crate) fn is_captured(&self) -> bool {
        matches!(self, RerunStdin::Captured(_))
    }

    /// Whether the stored prompt is already known to be unusable, asked
    /// without waiting on the copier (issue #2816).
    ///
    /// The copier learns at once that the source failed, but
    /// [`Capture::for_attempt`] only asks when a rerun is already due, which
    /// is after the wait. This is the question a decision can afford:
    /// `false` for every other kind of stdin, and for a capture still going
    /// well, already complete, or carried on in memory after the disk
    /// refused it (issue #3036), none of which costs a rerun anything.
    pub(crate) fn capture_lost(&self) -> bool {
        matches!(self, RerunStdin::Captured(c) if c.lost())
    }

    /// What a rerun can be given of this step's prompt, asked at the moment
    /// one is about to run (issues #2816, #3036).
    ///
    /// For a capture this settles the copier: see [`Capture::settle`]. A
    /// prompt that is still arriving answers [`Replay::Arriving`] and that
    /// answer is not recorded, so a later call, after a wait say, can find
    /// the copier settled and answer [`Replay::Ready`]. Every other kind of
    /// stdin always answers `Ready`: `for_attempt` can give each of them a
    /// rerun's stdin without waiting on anything.
    pub(crate) fn replay(&mut self) -> Replay {
        match self {
            RerunStdin::Captured(c) => c.settle(),
            _ => Replay::Ready,
        }
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
    /// whose source is still being read or has ended properly, including
    /// one the disk refused, which the copier carries on in memory.
    pub(crate) fn prompt_cut_short(&self) -> bool {
        matches!(self, RerunStdin::Captured(c) if c.cut_short())
    }

    /// Checks that fd 0 is whole, for the plain launch that follows the
    /// supervisor standing down (issue #2816).
    ///
    /// Nothing has been taken off it. Every kind but a capture left the
    /// step's own descriptor in place and never touched it; a capture
    /// starts its copier only once the first attempt is running
    /// ([`RerunStdin::attempt_started`]), and a supervisor stands down only
    /// before that. So the plain launch inherits stdin exactly as it always
    /// did, and this is the check that says so rather than a hand-back.
    ///
    /// The error arm is the guard that keeps it that way: a copier that has
    /// started has read an unknown amount of the prompt and nothing can put
    /// it back, so the caller must not run the step on what is left. No
    /// path reaches it today, and a new one that did would be refused here
    /// instead of silently shortening a task.
    pub(crate) fn stand_down(&self) -> io::Result<()> {
        match self {
            RerunStdin::Captured(c) if c.started() => Err(fsx::invalid(
                "the copy of this step's prompt had already begun, so fd 0 is short by whatever \
                 it read and the rest cannot be put back",
            )),
            _ => Ok(()),
        }
    }
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
    /// Set only after end of file, with every byte stored on disk or in
    /// memory.
    complete: AtomicBool,
    /// Set the moment the capture is known to be lost, which is before the
    /// copier itself ends: the source failed, or (unreachably for a regular
    /// file) the capture file could not be flushed. A rerun is refused on
    /// this while the limit result can still be passed up. A refused write
    /// to the capture file does not set it (issue #3036): the copy goes on
    /// in [`CopyState::spill`].
    lost: AtomicBool,
    /// The part of the prompt kept in memory, once the disk refused the
    /// copy or could not hold it at all (issue #3036). `None` while the
    /// disk holds every byte.
    spill: Mutex<Option<Spill>>,
    /// Set the moment a read of the source fails, which means the attempt
    /// in hand is being given less than the prompt that was sent. Always
    /// set together with `lost`, and always before the attempt's pipe is
    /// dropped.
    cut_short: AtomicBool,
    /// Set when a write of the prompt to the attempt's own stdin fails
    /// while the source still has prompt to come: whatever was reading it
    /// closed its end early. Neither the capture nor any later attempt
    /// loses anything, so this is a report and not a failure.
    reader_gone: AtomicBool,
}

impl CopyState {
    /// The whole prompt reached both sinks.
    fn done(&self) {
        self.complete.store(true, Ordering::SeqCst);
    }

    pub(super) fn is_complete(&self) -> bool {
        self.complete.load(Ordering::SeqCst)
    }

    /// The capture file could not be flushed after every write to it had
    /// succeeded, so what it holds is not known to be whole. `flush` on a
    /// regular file does nothing and cannot fail, so this is unreachable in
    /// practice; it is kept so that a failure there could never be read as
    /// a whole copy. The attempt in hand still gets every byte.
    ///
    /// The error is named on stderr here, as a prompt cut short names its
    /// own (issue #2816): the supervisor reads the loss as a flag and never
    /// the thread's result. It names the error and no part of the prompt.
    fn lose(&self, e: &io::Error) {
        self.lost.store(true, Ordering::SeqCst);
        eprintln!(
            "amplihack: this step's prompt could not be stored for a rerun ({e}), so a \
             usage limit will end this step; the attempt running now still receives all of it"
        );
    }

    pub(super) fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// The disk refused the copy after `stored` bytes were safely in the
    /// capture file, or (`stored` 0, `why` `None`) never held it at all:
    /// every byte from here on is kept in memory (issue #3036).
    ///
    /// A refused write names its error on stderr here, the one place it is
    /// said, and says that nothing is lost by it. The chunk whose write
    /// failed is kept whole in memory, so whatever part of it the disk took
    /// is ignored and the copy has no gap. It names the error and no part
    /// of the prompt.
    fn begin_spill(&self, stored: u64, why: Option<&io::Error>) {
        if let Some(e) = why {
            eprintln!(
                "amplihack: this step's prompt could not be stored on disk for a rerun ({e}), \
                 so the rest of it is kept in memory instead; a rerun still reads all of it"
            );
        }
        let mut spill = self.spill.lock().unwrap_or_else(|p| p.into_inner());
        spill.get_or_insert(Spill {
            stored,
            bytes: Vec::new(),
        });
    }

    /// Appends `chunk` to the part of the prompt kept in memory.
    fn keep(&self, chunk: &[u8]) {
        let mut spill = self.spill.lock().unwrap_or_else(|p| p.into_inner());
        spill
            .get_or_insert(Spill {
                stored: 0,
                bytes: Vec::new(),
            })
            .bytes
            .extend_from_slice(chunk);
    }

    /// Whether any of the prompt is kept in memory (issue #3036).
    pub(super) fn is_spilled(&self) -> bool {
        self.spill
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Takes the part of the prompt kept in memory, once the copier has
    /// finished with it.
    fn take_spill(&self) -> Option<Spill> {
        self.spill.lock().unwrap_or_else(|p| p.into_inner()).take()
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

    /// The attempt's end of the prompt pipe closed with prompt still to
    /// come: it exited, or stopped reading, before the whole task reached
    /// it.
    ///
    /// Nothing is lost here — the capture still gets every byte and so does
    /// any later attempt — but the step did its work on part of its task,
    /// and that is worth a line. The recipe runner says as much when it
    /// writes a task straight to an agent that exits early; a supervised
    /// step takes that writer's place, always drains it, and so always
    /// lets the runner's write succeed, which would leave the case silent
    /// (issue #2816). This is the supervisor's own version of it, and like
    /// the runner's it is diagnostic: it names no part of the prompt and
    /// changes no exit code.
    ///
    /// It can only ever describe a real attempt. The copier starts after
    /// the first attempt's child has been spawned, so the far end of this
    /// pipe belonged to a running step; a supervisor that stood down before
    /// any attempt never started a copier and never reaches this.
    fn reader_gone(&self) {
        self.reader_gone.store(true, Ordering::SeqCst);
        eprintln!(
            "amplihack: this step stopped reading its prompt before all of it had been \
             delivered, so it ran on part of its task; the rest went to the stored copy"
        );
    }

    pub(super) fn reader_is_gone(&self) -> bool {
        self.reader_gone.load(Ordering::SeqCst)
    }
}

/// The part of the prompt kept in memory (issue #3036): the capture file
/// holds the first `stored` bytes, and `bytes` holds every byte after them.
struct Spill {
    stored: u64,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for Spill {
    /// Lengths only: no part of the prompt is ever printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spill")
            .field("stored", &self.stored)
            .field("in_memory", &self.bytes.len())
            .finish()
    }
}

/// The capture file, when the disk would take one.
struct Disk {
    /// `<rerun dir>/<pid>-0.in`, this supervisor's own name.
    path: PathBuf,
    /// The capture file, opened for writing.
    writer: fs::File,
    /// A read-only description of `path`: see [`Capture::reader`].
    reader: fs::File,
}

/// What the copier needs, held until the first attempt is running.
struct Pending {
    /// A duplicate of the step's stdin, where the prompt arrives.
    source: fs::File,
    /// The capture file, opened for writing; `None` when the disk would
    /// not take one, and the copy is then held in memory (issue #3036).
    writer: Option<fs::File>,
    /// The write end of the first attempt's stdin pipe.
    child: std::io::PipeWriter,
}

/// One copy of the step's piped prompt.
///
/// A recipe agent step is given its prompt on stdin, so before issue #2815
/// a usage limit in such a step could neither be waited out nor rerun under
/// another profile: a second attempt had nothing to read. The copier thread
/// started by [`Capture::arm`] pumps that pipe into a private file in the
/// rerun scratch directory *and* into the pipe the first attempt reads, so
/// the first attempt is not delayed and every later attempt gets the same
/// bytes from offset 0.
///
/// When the disk will not take the copy, at the start or part way through,
/// the copier keeps the rest of it in memory (issue #3036), and a rerun is
/// given the disk's part and then memory's through a pipe. A storage
/// failure therefore never costs a rerun, and so never ends a step on a
/// limit.
///
/// The file is removed when the step returns, on success and on error.
/// `sweep_rerun` removes one left behind by a supervisor that was killed.
pub(crate) struct Capture {
    /// `<rerun dir>/<pid>-0.in`, this supervisor's own name; `None` for a
    /// capture held in memory from the start.
    path: Option<PathBuf>,
    /// A read-only description of `path`, opened before any byte was
    /// written and checked against the writer's inode, so a rerun never
    /// re-opens the path and never races a same-user replacement. `None`
    /// with `path`.
    reader: Option<fs::File>,
    /// The whole prompt, once the copier has finished and part of it had to
    /// be kept in memory (issue #3036): the disk's part read back through
    /// `reader`, then memory's. `None` while the disk alone holds it.
    memory: Option<Arc<Vec<u8>>>,
    /// What the copier will be given, until the first attempt is running
    /// (issue #2816). `None` once it has been.
    pending: Option<Pending>,
    /// The read end of the first attempt's stdin pipe, until it is handed
    /// to that attempt.
    first: Option<fs::File>,
    /// A second description of the source, polled and never read: it
    /// answers whether any writer of the prompt is left, which is what
    /// makes joining the copier a bounded wait (issue #2816).
    watch: fs::File,
    /// The copier, until a rerun joins it.
    copier: Option<JoinHandle<io::Result<()>>>,
    /// What the copier publishes as it runs, read without joining it
    /// (issue #2816).
    state: Arc<CopyState>,
    /// Why the capture cannot be replayed, once that is settled.
    failure: Option<String>,
}

impl std::fmt::Debug for Capture {
    /// Never prints the prompt, nor the capture path, which no log line
    /// carries either.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("started", &self.started())
            .field("complete", &self.state.is_complete())
            .field("failed", &self.lost())
            .field("cut_short", &self.cut_short())
            .field("reader_gone", &self.state.reader_is_gone())
            .field(
                "in_memory",
                &(self.memory.is_some() || self.state.is_spilled()),
            )
            .finish()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

impl Capture {
    /// Starts the copier, now that the first attempt holds the other end of
    /// its stdin pipe (issue #2816). Does nothing when it has already run.
    ///
    /// A thread that cannot be started leaves the attempt's pipe with no
    /// writer, so that attempt reads an immediate end of file and acts on
    /// an empty prompt. That is exactly a prompt cut short, and it is
    /// published as one: the supervisor then refuses to report the
    /// attempt's success as the step's.
    fn arm(&mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let Pending {
            mut source,
            writer,
            child,
        } = p;
        let published = Arc::clone(&self.state);
        match std::thread::Builder::new()
            .name("amplihack-prompt-capture".to_string())
            .spawn(move || copy_prompt(&mut source, writer, child, &published))
        {
            Ok(copier) => self.copier = Some(copier),
            Err(e) => {
                self.state.cut_short(&e);
                self.failure = Some(format!(
                    "the thread to copy this step's prompt could not be started: {e}"
                ));
            }
        }
    }

    /// Whether the copier has been started, and so whether any byte of this
    /// process's stdin has been taken (issue #2816).
    fn started(&self) -> bool {
        self.pending.is_none()
    }

    /// The stdin for attempt `n` (from 1): the copier's pipe for the first
    /// attempt, the completed capture from offset 0 for every later one.
    fn for_attempt(&mut self, n: u32) -> io::Result<Stdio> {
        if n == 1
            && let Some(first) = self.first.take()
        {
            return Ok(Stdio::from(first));
        }
        self.whole()
            .map_err(|e| io::Error::other(format!("{e}, so it cannot be given to attempt {n}")))
    }

    /// The whole stored prompt from offset 0, once the copier has finished
    /// with it: the capture file read-only, or a pipe fed from memory when
    /// any of the prompt had to be kept there (issue #3036).
    ///
    /// Settles the copier (see [`Capture::settle`]) and refuses a capture
    /// that is short, failed or still arriving: a partial prompt is a
    /// shortened task, and no caller may be given one.
    fn whole(&mut self) -> io::Result<Stdio> {
        match self.settle() {
            Replay::Ready => {}
            Replay::Arriving => return Err(io::Error::other(ARRIVING)),
            Replay::Never(why) => return Err(io::Error::other(why)),
        }
        if let Some(bytes) = &self.memory {
            return fed_from_memory(Arc::clone(bytes));
        }
        let Some(reader) = &self.reader else {
            // Unreachable: a capture with no file spills from its first
            // byte, so a whole one always has `memory`.
            return Err(io::Error::other(
                "this step's prompt was kept neither on disk nor in memory",
            ));
        };
        let mut f = reader.try_clone()?;
        f.seek(SeekFrom::Start(0))?;
        Ok(Stdio::from(f))
    }

    /// Whether the capture is already known to be unusable, without
    /// joining the copier (issue #2816). A copy the disk refused and memory
    /// carried on is not (issue #3036).
    fn lost(&self) -> bool {
        self.failure.is_some() || self.state.is_lost()
    }

    /// Whether the attempt in hand was given a prompt that stops short of
    /// the one that was sent (issue #2816).
    fn cut_short(&self) -> bool {
        self.state.was_cut_short()
    }

    /// Settles the copier when a rerun needs the whole prompt, and says
    /// whether there is one to give (issues #2816, #3036).
    ///
    /// The copier ends at the end of its source, and that source is a pipe
    /// belonging to whoever launched this step: nothing promises it will
    /// ever be closed. Joining it unconditionally would therefore trade
    /// issue #1618's rule (a supervised step never waits on a pipe nobody
    /// closes) for a wait with no end, deaf to the stop signals the
    /// supervisor catches. So the join is made only when the copier is
    /// certain to reach it ([`Capture::can_end`]), and otherwise the answer
    /// is [`Replay::Arriving`]: the caller decides how to wait for the rest,
    /// and asks again.
    ///
    /// Nothing here is timed, and an `Arriving` answer is not recorded: the
    /// next call, after a wait say, by which point the caller has normally
    /// long since finished writing, settles the copier and answers
    /// [`Replay::Ready`].
    ///
    /// [`Replay::Never`] is left for a prompt that no rerun can ever be
    /// given: the source could not be read to its end, the copier could not
    /// start or failed, or the part of the prompt the disk held before it
    /// refused the rest cannot be read back. A capture file that could not
    /// be created or written is none of these (issue #3036).
    ///
    /// The first attempt's pipe is dropped first: a child that exited
    /// without reading its prompt would otherwise leave the copier blocked
    /// on a full pipe nobody drains. With the read end gone those writes
    /// fail with `EPIPE`, which the copier absorbs, and it runs on to end
    /// of file.
    fn settle(&mut self) -> Replay {
        self.first = None;
        if let Some(why) = &self.failure {
            return Replay::Never(why.clone());
        }
        if self.pending.is_some() {
            // The copier is started by the first attempt, and none has
            // run, so not one byte has been copied yet. Unreachable: a
            // rerun is only ever asked for after an attempt ran.
            return Replay::Never(
                "no attempt has run, so none of this step's prompt has been copied".into(),
            );
        }
        if self.state.was_cut_short() {
            // The source failed, which the copier named on stderr with its
            // error as it happened, and it has already returned.
            return Replay::Never("this step's prompt could not be read to its end".into());
        }
        if self.state.is_lost() {
            // The capture file could not be flushed, which the copier named
            // on stderr as it happened. Unreachable for a regular file.
            return Replay::Never("the copy of this step's prompt could not be stored".into());
        }
        let Some(copier) = self.copier.take() else {
            return Replay::Ready; // already joined, and whole
        };
        if !self.can_end() {
            self.copier = Some(copier); // ask again when a rerun is next due
            return Replay::Arriving;
        }
        self.failure = match copier.join() {
            Ok(Ok(())) if self.state.is_complete() => self.gather().err(),
            Ok(Ok(())) => {
                Some("the copy of this step's prompt ended before the end of the input".into())
            }
            Ok(Err(e)) => Some(format!("copying this step's prompt failed: {e}")),
            Err(_) => Some("the thread copying this step's prompt panicked".into()),
        };
        match &self.failure {
            Some(why) => Replay::Never(why.clone()),
            None => Replay::Ready,
        }
    }

    /// Joins the two parts of a prompt the disk refused part of into one
    /// copy in memory, once the copier has finished (issue #3036): the
    /// first `stored` bytes of the capture file, read back through
    /// `reader`, then the bytes the copier kept in memory. Done once, so
    /// every rerun reads the same bytes. Nothing to do when the disk holds
    /// the whole prompt.
    ///
    /// A prefix that cannot be read back whole is the one way a refused
    /// disk can still cost the rerun, and it is answered as
    /// [`Replay::Never`] rather than handing a rerun part of a task. The
    /// reason names the error and no part of the prompt.
    fn gather(&mut self) -> Result<(), String> {
        let Some(Spill { stored, bytes }) = self.state.take_spill() else {
            return Ok(());
        };
        let whole = join_parts(self.reader.as_ref(), stored, bytes).map_err(|e| {
            format!(
                "the part of this step's prompt stored on disk before the disk refused the \
                 rest could not be read back: {e}"
            )
        })?;
        self.memory = Some(Arc::new(whole));
        Ok(())
    }

    /// Whether the copier is certain to end, so joining it is bounded.
    ///
    /// Either it has already reached the end of the source and published
    /// that, or no writer of the source is left — after which what it still
    /// has to read is only what the kernel is already holding, and its
    /// writes go to a regular file or to memory, and to a pipe whose reader
    /// has gone. A copier that failed has already returned and
    /// [`Capture::settle`] answers before it asks this.
    fn can_end(&self) -> bool {
        self.state.is_complete() || no_writer_left(&self.watch)
    }
}

/// The whole prompt from its two parts (issue #3036): the first `stored`
/// bytes of the capture file `reader` describes, then `rest`.
///
/// Refuses a file that holds fewer than `stored` bytes, and a non-zero
/// `stored` with no file, rather than return a prompt with a gap in it.
pub(super) fn join_parts(
    reader: Option<&fs::File>,
    stored: u64,
    rest: Vec<u8>,
) -> io::Result<Vec<u8>> {
    if stored == 0 {
        return Ok(rest);
    }
    let reader = reader.ok_or_else(|| io::Error::other("there is no capture file to read"))?;
    let mut f = reader.try_clone()?;
    f.seek(SeekFrom::Start(0))?;
    let mut whole = Vec::new();
    f.take(stored).read_to_end(&mut whole)?;
    if u64::try_from(whole.len()).ok() != Some(stored) {
        return Err(io::Error::other(
            "the capture file holds less than was written to it",
        ));
    }
    whole.extend_from_slice(&rest);
    Ok(whole)
}

/// A rerun's stdin for a prompt held in memory (issue #3036): the read end
/// of a pipe, and a thread that writes `bytes` into the other end and
/// closes it.
///
/// The thread ends when every byte is written or the attempt closes its end
/// (the write then fails with `EPIPE`, which is the attempt's own choice and
/// costs nothing stored), so it never outlives the attempt that reads it.
/// The pipe is `O_CLOEXEC` on both ends, so only the descriptor `Command`
/// puts on fd 0 reaches the child, as for the first attempt's pipe.
fn fed_from_memory(bytes: Arc<Vec<u8>>) -> io::Result<Stdio> {
    let (read, mut write) = std::io::pipe()?;
    std::thread::Builder::new()
        .name("amplihack-prompt-replay".to_string())
        .spawn(move || {
            let _ = write.write_all(&bytes);
        })?;
    Ok(Stdio::from(fs::File::from(std::os::fd::OwnedFd::from(
        read,
    ))))
}

/// Whether `watch`, a second description of the step's stdin, has no writer
/// left.
///
/// `poll(2)` with a zero timeout reads the state of the descriptor and does
/// nothing else: it consumes no byte, and it waits for nothing. `POLLHUP`
/// on a pipe means every writer has closed, so no further byte can arrive;
/// `POLLERR` and `POLLNVAL` say the same in their own way. Anything else —
/// readable now, or quiet with a writer still holding it — means the prompt
/// may still be on its way, and a call that cannot be made at all is read
/// the same way: the cautious answer is the one that refuses a rerun rather
/// than the one that waits for ever.
///
/// It is an answer for the instant it is asked, and the two ways it can be
/// wrong both cost a rerun and never a byte: a writer that is about to
/// close reads as one that may never, and so does a copy of the descriptor
/// that some other process is holding — every `fork` holds one of each of
/// its parent's descriptors until it `exec`s. Neither can make this say a
/// prompt is whole when it is not, which is the answer that would matter.
/// The supervisor asks only when a rerun is due, which is after an attempt
/// has run and usually after a wait, so a caller that finished writing long
/// ago is seen to have finished.
fn no_writer_left(watch: &fs::File) -> bool {
    let mut fds = libc::pollfd {
        fd: watch.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialised `pollfd` naming a descriptor this process
    // owns for the whole of the call; `poll` writes only `revents`, and a
    // zero timeout cannot block.
    let rc = unsafe { libc::poll(&mut fds, 1, 0) };
    rc >= 0 && fds.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// Copies this process's stdin aside so every attempt can read it.
///
/// `None` when stdin cannot be duplicated; the caller keeps the policy it
/// already had.
pub(crate) fn capture_own_prompt(dir: &Path) -> Option<RerunStdin> {
    let owned = io::stdin().as_fd().try_clone_to_owned().ok()?;
    Some(capture_prompt(dir, fs::File::from(owned)))
}

/// Prepares a [`Capture`] in `dir` from `source`, a duplicate of the step's
/// stdin. No byte of `source` is read until
/// [`RerunStdin::attempt_started`].
///
/// A capture file that cannot be made in `dir` does not cost the rerun
/// (issue #3036): the capture is then held in memory from its first byte,
/// and the reason is named on stderr. Only when the descriptors the copy
/// itself needs cannot be made (the first attempt's pipe, a second
/// description of the source) does this return [`RerunStdin::Pipe`], the
/// conduct of a piped prompt before #2815: no copy and no rerun. Nothing
/// here refuses a prompt for its size or its contents.
pub(crate) fn capture_prompt(dir: &Path, source: fs::File) -> RerunStdin {
    let disk = match open_disk(dir) {
        Ok(disk) => Some(disk),
        Err(e) => {
            eprintln!(
                "amplihack: this step's prompt could not be stored on disk for a rerun ({e}), \
                 so it is kept in memory instead; a rerun still reads all of it"
            );
            None
        }
    };
    let path = disk.as_ref().map(|d| d.path.clone());
    match prepare_copier(disk, source) {
        Ok(c) => RerunStdin::Captured(c),
        Err(e) => {
            // Anything that fails from here on takes the file away again,
            // so a half-made capture is never left for the sweep to find.
            if let Some(path) = path {
                let _ = fs::remove_file(path);
            }
            eprintln!(
                "amplihack: this step's prompt could not be copied for a rerun, so a usage \
                 limit will end it: {e}"
            );
            RerunStdin::Pipe
        }
    }
}

/// The capture file in `dir`, created private and matched to a read-only
/// description of itself; removed again if that cannot be done.
fn open_disk(dir: &Path) -> io::Result<Disk> {
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
    // Opened before a byte is written and matched to the writer's inode,
    // so later attempts never re-open the path: a same-user replacement of
    // it cannot change what they read.
    match read_back(&path, &writer) {
        Ok(reader) => {
            let writer = refuse_writes(&path, writer);
            Ok(Disk {
                path,
                writer,
                reader,
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&path);
            Err(e)
        }
    }
}

/// The rest of [`capture_prompt`], with or without a capture file:
/// duplicate the source for the writer check, and make the first attempt's
/// pipe. The copier itself waits for [`Capture::arm`].
fn prepare_copier(disk: Option<Disk>, source: fs::File) -> io::Result<Capture> {
    let watch = source.try_clone()?;
    // `std::io::pipe` is `O_CLOEXEC` on both ends, so only the descriptor
    // `Command` puts on fd 0 reaches the child.
    let (child_read, child_write) = std::io::pipe()?;
    let (path, writer, reader) = match disk {
        Some(Disk {
            path,
            writer,
            reader,
        }) => (Some(path), Some(writer), Some(reader)),
        None => (None, None, None),
    };
    Ok(Capture {
        path,
        reader,
        memory: None,
        pending: Some(Pending {
            source,
            writer,
            child: child_write,
        }),
        first: Some(fs::File::from(std::os::fd::OwnedFd::from(child_read))),
        watch,
        copier: None,
        state: Arc::new(CopyState::default()),
        failure: None,
    })
}

/// Debug-only seam for issue #2816: `AMPLIHACK_TEST_REFUSE_CAPTURE_WRITE=1`
/// hands the copier a read-only description of the capture file, so every
/// write to it fails as a full disk, a quota or a failing device would.
///
/// Since issue #3036 that failure costs neither the prompt nor the rerun:
/// the copy goes on in memory. The line the copier prints and the rerun
/// that still reads every byte are what an operator sees of it, so there
/// has to be a way to make it happen on demand: the real causes cannot be
/// arranged from outside a test. Compiled out of release builds, as the usage-limit
/// wait's test clock and `AMPLIHACK_TEST_FAIL_ATTEMPT` are, so no installed
/// binary reads this variable.
#[cfg(debug_assertions)]
fn refuse_writes(path: &Path, writer: fs::File) -> fs::File {
    match std::env::var_os("AMPLIHACK_TEST_REFUSE_CAPTURE_WRITE") {
        Some(v) if !v.is_empty() => fs::File::open(path).unwrap_or(writer),
        _ => writer,
    }
}

#[cfg(not(debug_assertions))]
fn refuse_writes(_path: &Path, writer: fs::File) -> fs::File {
    writer
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
/// The prompt is opaque bytes throughout: never decoded and never printed,
/// and held in memory only once the disk has refused it (issue #3036). A
/// first attempt that exits without reading closes its end, and the
/// `EPIPE` that follows only stops the copy to the child; the capture still
/// receives all of it, and [`CopyState::reader_gone`] reports the early
/// close.
///
/// The capture is secondary to the attempt in hand, and the disk is
/// secondary to the capture. `file` is `None` when the disk would not take
/// a capture file at all, and every byte is then kept in memory. A write to
/// the file that fails stops the copy to the file and nothing else
/// (issue #3036): the bytes fully stored before that chunk stay where they
/// are, the failing chunk and every later one are kept in memory
/// ([`CopyState::begin_spill`] names the error on stderr), and the source
/// is still drained to end of file for the child and the capture alike. So
/// a full disk, a quota or a failing device costs neither the attempt in
/// hand nor a rerun, and is never a reason for a limit to end the step.
///
/// A failed read of the source is the other case, and the one the attempt
/// cannot be told about: there are no further bytes to forward, so the copy
/// ends for both sinks, and the attempt's pipe closes. Closing it is the
/// only signal a pipe gives, and a reader takes it for the end of the
/// prompt. [`CopyState::cut_short`] therefore publishes the loss, says so
/// on stderr the moment it happens, and the supervisor refuses to report
/// the attempt's success as the step's (issue #2816). That is the one loss
/// no rerun can make good. `EAGAIN` is not such a failure: an inherited
/// descriptor may carry `O_NONBLOCK` on a description shared with the
/// writer, and this waits for the next byte as a blocking read would have.
///
/// An attempt that stopped reading is not a loss and never publishes one.
/// The supervisor reads these flags when it decides whether a rerun is
/// possible at all, so a prompt already known to be cut short refuses the
/// rerun then and there, rather than after a wait of hours.
///
/// `source` is the step's own stdin, an `fs::File`. It is taken by bound
/// rather than by type only so that a test can see the reads as they
/// happen: whether the `EAGAIN` arm below is reached at all otherwise
/// depends on when the copier and the writer are scheduled, and a test of
/// it would be a test of that. `wait_readable` still polls the descriptor
/// `AsRawFd` yields, which is the source's own. `file` is taken by bound
/// for the same reason: a test can give it a sink that refuses a write part
/// way through a chunk, which no real file can be made to do on demand.
pub(super) fn copy_prompt<S: Read + AsRawFd, W: Write>(
    source: &mut S,
    mut file: Option<W>,
    child: std::io::PipeWriter,
    state: &CopyState,
) -> io::Result<()> {
    let mut child = Some(child);
    // Bytes of the prompt fully written to `file`; meaningful only until
    // the first refused write, after which memory holds the rest.
    let mut stored: u64 = 0;
    if file.is_none() {
        // The disk never took a capture file, and the reason was named
        // where that was found out (`capture_prompt`).
        state.begin_spill(0, None);
    }
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
        let chunk = &buf[..k];
        match &mut file {
            Some(f) => match f.write_all(chunk) {
                Ok(()) => stored = stored.saturating_add(k as u64),
                Err(e) => {
                    // The disk refused this chunk, perhaps part way through
                    // it. What it held before the chunk stays; the chunk
                    // itself is kept whole in memory, so the copy has no
                    // gap, and so is everything after it (issue #3036).
                    state.begin_spill(stored, Some(&e));
                    state.keep(chunk);
                    file = None;
                }
            },
            None => state.keep(chunk),
        }
        if let Some(w) = &mut child
            && w.write_all(chunk).is_err()
        {
            // The attempt is gone, or stopped reading, with prompt still
            // to come. Its prompt is still stored; closing our end lets it
            // see end of file if it is somehow still there. The early
            // close is reported, since the runner's own warning for it
            // cannot fire for a supervised step (issue #2816).
            child = None;
            state.reader_gone();
        }
    }
    if let Some(f) = &mut file
        && let Err(e) = f.flush()
    {
        state.lose(&e);
        return Err(e);
    }
    state.done();
    Ok(())
}

/// The part of the prompt `state` keeps in memory, for the tests that pin
/// where a refused write leaves each byte (issue #3036).
#[cfg(test)]
pub(super) fn spilled(state: &CopyState) -> Option<(u64, Vec<u8>)> {
    state
        .spill
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| (s.stored, s.bytes.clone()))
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
fn wait_readable(source: &impl AsRawFd) -> io::Result<()> {
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
