//! Scratch and stdin of a supervised step (issue #1618): the home scratch
//! directory `$HOME/.amplihack/rerun/`, and what each attempt reads on
//! stdin.
//!
//! The supervisor's own thread never reads stdin, so a pipe the step
//! inherited and nobody closes cannot hold it up. One bounded exception
//! exists since issue #2815: when the step's prompt arrives on a pipe
//! rather than in argv, [`capture_prompt`] prepares a copier that pumps
//! that pipe into the first attempt's own stdin pipe and keeps every byte
//! of it in memory, so a later attempt can read the same bytes.
//!
//! The copy is kept in memory and nowhere else. It
//! used to go to a private file in the rerun directory, with memory taking
//! over only once the disk refused it (issue #3036). A copy split that way
//! was lost when the part on disk could not be read back, and the copier's
//! other failures (a final flush, a panic) were each a way for a usage
//! limit to end a step whose attempt had been given its whole task. Kept
//! in memory, every byte the copier reads is a byte a retry is given.
//! Memory is not capped, because a cap would be a budget that ends a step
//! on a limit.
//!
//! Everything the copy needs is made before the first attempt starts: a
//! second description of the source, the first attempt's stdin pipe, and
//! the copier thread itself. When any of them cannot be made, which happens
//! only to a process out of file descriptors or threads, [`capture_prompt`]
//! returns the error and the supervisor runs no attempt at all
//! (`auto_switch::rerun::run`). No step is run on a piped prompt of which no
//! copy is kept, so no limit can meet an attempt that a retry could not
//! repeat.
//!
//! Two rules keep that copier from ever holding the step up, because the
//! pipe it reads belongs to the caller and nothing promises the caller will
//! close it (issue #2816).
//!
//! The first is that the copier takes nothing off fd 0 until there is a
//! child to give it to. Its thread exists from the start but waits, and
//! [`RerunStdin::attempt_started`] lets it go only once the first attempt
//! has been spawned. Every path that stands down and lets a plain
//! in-process launch run the step instead is reached before that, so the
//! plain launch inherits fd 0 with the whole prompt still on it and there
//! is nothing to give back ([`RerunStdin::stand_down`]); the waiting thread
//! ends without reading a byte when the capture is dropped. It also means
//! an `EPIPE` from the attempt's end of the pipe is always a real attempt
//! that stopped reading, which is why the copier may report it.
//!
//! The second is that nothing waits for the copier before a rerun. A rerun
//! starts at once, from the copy already held: a feeder thread gives it
//! every byte kept so far from the first, then each
//! further byte as the copier keeps it, and closes its pipe when the copier
//! reaches the end of the source. A rerun therefore reads exactly what the
//! first attempt read, in the same order, and reaches the end of its stdin
//! when the caller closes the pipe and not before, as the first attempt
//! did. A caller that holds the pipe open holds up only an attempt that
//! reads it, never the supervisor, and an attempt that does not read its
//! stdin runs and ends whatever the caller does. Before this review a
//! rerun waited, in the supervisor, for a caller that had not closed its
//! pipe (issue #3036), and a caller that never did held the step for ever.
//! The copier is joined only when a step that ran a rerun has finished and
//! the caller has closed the pipe ([`RerunStdin::settle`]), so that what it
//! reports is on stderr before the step's result; nothing here is timed,
//! and the question asked is the state of the descriptor, not the clock.
//!
//! One failure is left, and it costs the prompt itself rather than its
//! copy: a source that cannot be read to its end. The copier's pipe has no
//! way to say so: closing it is the only signal a pipe gives its reader,
//! and a reader takes it for the end of the prompt. So a failed read of the
//! source is published as a prompt cut short (issue #2816), named on stderr
//! the moment it happens, and read by the supervisor before it passes an
//! attempt's success up as the step's. A copier that panicked is published
//! the same way, because its pipe closed wherever it stopped. That is the
//! one input no rerun can be given, and a step whose prompt was cut short
//! fails whether or not it meets a limit: it is a lost prompt, not a limit
//! ending.
//!
//! The first attempt therefore inherits stdin unchanged, except for a
//! captured prompt, which it reads from the copier's pipe. A rerun gets a
//! regular file again from where the step started reading it (on Linux
//! through its own open of `/proc/self/fd/0`, so no read offset is shared
//! with other processes), a captured prompt from its first byte through a
//! pipe fed from the kept copy and then from the copier, a terminal
//! unchanged, and anything else (a pipe whose prompt is in argv, a socket,
//! a closed descriptor) `/dev/null`: what the first attempt read from it is
//! gone.

use super::super::fsx;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;

/// What a rerun can be given of this step's input, asked at the moment one
/// is due (issues #2816, #3036).
///
/// Two answers, and neither is "wait". A prompt still arriving on a pipe
/// its caller has not closed is [`Replay::Ready`]: the rerun is fed the copy
/// already held and then the rest as it arrives, as the first attempt was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Replay {
    /// A rerun can read the input: it is stored, it is being stored as it
    /// arrives, or it needs no storing.
    Ready,
    /// No rerun can ever be given this input, for the reason given. Only a
    /// prompt that could not be read to its end reaches this; the reason is
    /// static text or an I/O error, never prompt bytes.
    Never(String),
}

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
    /// contents went to attempt 1. An attempt runs on such a stdin only
    /// when the prompt is in argv: a piped prompt is always captured, or the
    /// step is not run.
    Pipe,
    /// A closed descriptor: `/dev/null`.
    Null,
    /// The step's prompt, kept in memory as it is delivered (issue #2815).
    /// Every attempt reads the same bytes, a rerun from the copy already
    /// held and then from the copier.
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

    /// What a rerun can be given of this step's prompt, asked at the moment
    /// one is about to run (issues #2816, #3036).
    ///
    /// Never waits, and never joins the copier. A capture answers
    /// [`Replay::Never`] only for a prompt cut short (see
    /// [`Capture::replay`]); a prompt still arriving is `Ready`, because the
    /// rerun is fed the rest as it arrives. Every other kind of stdin always
    /// answers `Ready`: `for_attempt` can give each of them a rerun's stdin
    /// without waiting on anything.
    pub(crate) fn replay(&mut self) -> Replay {
        match self {
            RerunStdin::Captured(c) => c.replay(),
            _ => Replay::Ready,
        }
    }

    /// Lets the copier finish, once the step has finished, when it is
    /// certain to end: the caller has closed the pipe.
    ///
    /// Nothing before this joins the copier, so a report it makes (an
    /// attempt that stopped reading, a source that failed) could otherwise
    /// still be on its way to stderr when the step's result is passed up,
    /// and a source that failed after the last attempt ended could go unseen
    /// by [`RerunStdin::prompt_cut_short`]. A caller still holding the pipe
    /// is not waited for: the step ends, and the copier with the process.
    /// Only a step that ran a rerun is held by this ([`Capture::settle`]).
    /// Does nothing for any other kind of stdin.
    pub(crate) fn settle(&mut self) {
        if let RerunStdin::Captured(c) = self {
            c.settle();
        }
    }

    /// Whether this step's prompt could not be read to its end, so the
    /// attempt in hand may have been given less than the prompt that was
    /// sent and no rerun can be given all of it (issue #2816).
    ///
    /// Asked without joining the copier, and for the case that matters it
    /// cannot be missed: the flag is stored before an attempt's pipe is
    /// dropped (the first attempt's by the copier, a rerun's by its feeder),
    /// the attempt reaches end of file only once that pipe is dropped, and
    /// the supervisor asks this only after the attempt has exited. It is the
    /// one way a captured prompt is lost, because every byte the copier
    /// reads is kept, so it is also the question a
    /// decision about a rerun asks before a wait. `false` for every other
    /// kind of stdin, and for a capture whose source is still being read or
    /// has ended properly.
    pub(crate) fn prompt_cut_short(&self) -> bool {
        matches!(self, RerunStdin::Captured(c) if c.cut_short())
    }

    /// Checks that fd 0 is whole, for the plain launch that follows the
    /// supervisor standing down (issue #2816).
    ///
    /// Nothing has been taken off it. Every kind but a capture left the
    /// step's own descriptor in place and never touched it; a capture lets
    /// its copier read only once the first attempt is running
    /// ([`RerunStdin::attempt_started`]), and a supervisor stands down only
    /// before that. So the plain launch inherits stdin exactly as it always
    /// did, and this is the check that says so rather than a hand-back.
    ///
    /// The error arm is the guard that keeps it that way: a copier that has
    /// been let go has read an unknown amount of the prompt and nothing can put
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

/// Writes one line to stderr, and ignores a failure to write it.
///
/// `eprintln!` panics when stderr cannot be written, and the copier is the
/// one thread whose panic would cost a prompt. Its lines are reports, and a
/// report that cannot be made must not end the copy it reports on.
fn say(line: std::fmt::Arguments<'_>) {
    let _ = writeln!(io::stderr(), "{line}");
}

/// What the copier publishes about its own progress while it is still
/// running, so that a decision can read it without joining the copier
/// (issue #2816). A join waits for the writer to close the pipe, which is a
/// wait of unknown length and the wrong one to take before a decision.
#[derive(Default)]
pub(super) struct CopyState {
    /// Set only after end of file, with every byte kept.
    complete: AtomicBool,
    /// Set the moment the prompt is known to be cut short: a read of the
    /// source failed, or the copier panicked. The attempt in hand may then
    /// have been given less than the prompt that was sent, and no rerun can
    /// be given all of it. Always set before the attempt's pipe is dropped.
    cut_short: AtomicBool,
    /// Set when a write of the prompt to the attempt's own stdin fails
    /// while the source still has prompt to come: whatever was reading it
    /// closed its end early. Neither the copy nor any later attempt loses
    /// anything, so this is a report and not a failure.
    reader_gone: AtomicBool,
    /// Every byte of the prompt read so far, in order: the copy a rerun is
    /// given.
    bytes: Mutex<Vec<u8>>,
    /// Signalled whenever `bytes` grows or the copy ends, either way, so a
    /// rerun's feeder can give its attempt the rest of the prompt as it
    /// arrives. Every change it announces is made
    /// with `bytes` locked, so a feeder that has just looked cannot miss
    /// one.
    grew: Condvar,
}

impl std::fmt::Debug for CopyState {
    /// Flags and a length only: no part of the prompt is ever printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopyState")
            .field("complete", &self.is_complete())
            .field("cut_short", &self.was_cut_short())
            .field("reader_gone", &self.reader_is_gone())
            .field("kept", &self.kept_len())
            .finish()
    }
}

impl CopyState {
    /// The whole prompt reached the attempt, or was offered to it, and is
    /// kept.
    fn done(&self) {
        self.announce(&self.complete);
    }

    /// Sets `flag` with `bytes` locked, then wakes every feeder, so a feeder
    /// that found nothing new to give cannot go to sleep past the end of the
    /// copy.
    fn announce(&self, flag: &AtomicBool) {
        let held = self.bytes.lock().unwrap_or_else(|p| p.into_inner());
        flag.store(true, Ordering::SeqCst);
        drop(held);
        self.grew.notify_all();
    }

    pub(super) fn is_complete(&self) -> bool {
        self.complete.load(Ordering::SeqCst)
    }

    /// Appends `chunk` to the kept copy, and wakes every feeder.
    fn keep(&self, chunk: &[u8]) {
        self.bytes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend_from_slice(chunk);
        self.grew.notify_all();
    }

    /// The kept bytes from offset `at`, at most [`COPY_BUF`] of them, once
    /// there are any; `None` once the copy has ended with nothing left past
    /// `at`.
    ///
    /// Waits while the copy has nothing past `at` and has not ended, which
    /// is a wait for the caller to write more or to close the pipe: the
    /// same wait the first attempt makes on a read of its stdin. Only a
    /// rerun's feeder thread calls this, never the supervisor's own.
    fn after(&self, at: usize) -> Option<Vec<u8>> {
        let mut bytes = self.bytes.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if bytes.len() > at {
                let to = bytes.len().min(at.saturating_add(COPY_BUF));
                return Some(bytes[at..to].to_vec());
            }
            if self.is_complete() || self.was_cut_short() {
                return None;
            }
            bytes = self.grew.wait(bytes).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// How many bytes are kept so far.
    fn kept_len(&self) -> usize {
        self.bytes.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// The prompt stops here: the attempt in hand will reach the end of its
    /// stdin without having been given the rest of its task, and no rerun
    /// can be given it either.
    ///
    /// The flag is stored before this returns, and the attempt's pipe is
    /// dropped only afterwards, so nothing that can observe that end of
    /// file can observe the flag unset. The line on stderr is the report
    /// that survives whatever the supervisor does next, an attempt that
    /// succeeds and is never asked about again included. It names the error
    /// and no part of the prompt.
    fn cut_short(&self, e: &io::Error) {
        self.announce(&self.cut_short);
        say(format_args!(
            "amplihack: this step's prompt could not be read to its end ({e}), so the \
             attempt reading it is given only what arrived before that: a prompt cut \
             short, which it cannot tell from a whole one"
        ));
    }

    pub(super) fn was_cut_short(&self) -> bool {
        self.cut_short.load(Ordering::SeqCst)
    }

    /// The attempt's end of the prompt pipe closed with prompt still to
    /// come: it exited, or stopped reading, before the whole task reached
    /// it.
    ///
    /// Nothing is lost here (the kept copy still gets every byte and so
    /// does any later attempt), but the step did its work on part of its
    /// task, and that deserves a line. The recipe runner says as much when
    /// it writes a task straight to an agent that exits early; a supervised
    /// step takes that writer's place, always drains it, and so always lets
    /// the runner's write succeed, which would leave the case silent (issue
    /// #2816). This is the supervisor's own version of it, and like the
    /// runner's it is diagnostic: it names no part of the prompt and changes
    /// no exit code.
    ///
    /// It can only ever describe a real attempt. The copier is let go only
    /// after the first attempt's child has been spawned, so the far end of
    /// this pipe belonged to a running step; a supervisor that stood down
    /// before any attempt never lets the copier go and never reaches this.
    fn reader_gone(&self) {
        self.reader_gone.store(true, Ordering::SeqCst);
        say(format_args!(
            "amplihack: this step stopped reading its prompt before all of it had been \
             delivered, so it ran on part of its task; the rest went to the kept copy"
        ));
    }

    pub(super) fn reader_is_gone(&self) -> bool {
        self.reader_gone.load(Ordering::SeqCst)
    }
}

/// One copy of the step's piped prompt.
///
/// A recipe agent step is given its prompt on stdin, so before issue #2815
/// a usage limit in such a step could neither be waited out nor rerun under
/// another profile: a second attempt had nothing to read. The copier thread
/// pumps that pipe into the pipe the first attempt reads *and* into memory,
/// so the first attempt is not delayed and every later attempt gets the
/// same bytes from the first one: those already kept at once, and the rest
/// as the copier keeps them.
///
/// Nothing is written to disk, so nothing is left behind for `sweep_rerun`.
pub(crate) struct Capture {
    /// Lets the copier start reading, sent once by [`Capture::arm`] when
    /// the first attempt is running (issue #2816). `None` once sent.
    /// Dropped unsent with the capture, which ends the copier without its
    /// having read a byte.
    go: Option<mpsc::Sender<()>>,
    /// The read end of the first attempt's stdin pipe, until it is handed
    /// to that attempt.
    first: Option<fs::File>,
    /// A second description of the source, polled and never read: it
    /// answers whether any writer of the prompt is left, which is what
    /// makes joining the copier at the end of the step a bounded wait
    /// (issue #2816).
    watch: fs::File,
    /// The copier, until [`Capture::settle`] joins it at the end of the
    /// step. Nothing else joins it.
    copier: Option<JoinHandle<io::Result<()>>>,
    /// Whether a rerun has been fed from the copy, which is what lets
    /// [`Capture::settle`] join the copier: a step that never ran one is
    /// never held by it.
    replayed: bool,
    /// What the copier publishes as it runs, read without joining it
    /// (issue #2816), and the kept copy every rerun is fed from.
    state: Arc<CopyState>,
    /// Why the prompt cannot be replayed, once that is known.
    failure: Option<String>,
}

impl std::fmt::Debug for Capture {
    /// Never prints the prompt.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("started", &self.started())
            .field("state", &self.state)
            .field("joined", &self.copier.is_none())
            .field("failed", &self.failure.is_some())
            .finish()
    }
}

impl Capture {
    /// Lets the copier start, now that the first attempt holds the other
    /// end of its stdin pipe (issue #2816). Does nothing when it has
    /// already been let go.
    ///
    /// The copier thread was started with the capture and has been waiting
    /// for this, so nothing here can fail for want of a thread. A copier
    /// that has somehow ended already has dropped the attempt's pipe, and
    /// that attempt then reads an immediate end of file: exactly a prompt
    /// cut short, and published as one, so the supervisor refuses to report
    /// the attempt's success as the step's.
    fn arm(&mut self) {
        let Some(go) = self.go.take() else {
            return;
        };
        if go.send(()).is_err() {
            let e = io::Error::other("the thread copying this step's prompt had already ended");
            self.state.cut_short(&e);
            self.failure = Some(e.to_string());
        }
    }

    /// Whether the copier has been let go, and so whether any byte of this
    /// process's stdin may have been taken (issue #2816).
    fn started(&self) -> bool {
        self.go.is_none()
    }

    /// The stdin for attempt `n` (from 1): the copier's pipe for the first
    /// attempt, and for every later one the kept copy from its first byte,
    /// then the rest as the copier keeps it.
    ///
    /// Refuses a prompt cut short, because a partial prompt is a shortened
    /// task and no caller may be given one. Never refuses a prompt that is
    /// still arriving, and never waits for one.
    fn for_attempt(&mut self, n: u32) -> io::Result<Stdio> {
        if n == 1
            && let Some(first) = self.first.take()
        {
            return Ok(Stdio::from(first));
        }
        match self.replay() {
            Replay::Ready => {
                let stdin = fed_from_copy(Arc::clone(&self.state))?;
                self.replayed = true;
                Ok(stdin)
            }
            Replay::Never(why) => Err(io::Error::other(format!(
                "{why}, so it cannot be given to attempt {n}"
            ))),
        }
    }

    /// Whether the prompt is already known to be cut short, without joining
    /// the copier (issue #2816).
    fn cut_short(&self) -> bool {
        self.state.was_cut_short()
    }

    /// Whether a rerun can be given this step's prompt (issues #2816,
    /// #3036). Never waits and never joins the copier.
    ///
    /// [`Replay::Never`] is left for a prompt that no rerun can ever be
    /// given, which is a prompt cut short: the source could not be read to
    /// its end, or the copier panicked part way through it. Every byte the
    /// copier did read is kept, so no failure of the copy itself reaches
    /// this. A prompt still arriving is [`Replay::Ready`]: its rerun is fed
    /// the copy already held and then the rest as it arrives, exactly as
    /// the first attempt was. Before this review it answered `Arriving`,
    /// and the supervisor waited, without end, for a caller that never
    /// closed its pipe.
    ///
    /// The first attempt's pipe is dropped first, when no attempt took it:
    /// the copier would otherwise block on a full pipe nobody drains.
    fn replay(&mut self) -> Replay {
        self.first = None;
        if let Some(why) = &self.failure {
            return Replay::Never(why.clone());
        }
        if self.go.is_some() {
            // The copier is let go by the first attempt, and none has run,
            // so not one byte has been copied yet. Unreachable: a rerun is
            // only ever asked for after an attempt ran.
            return Replay::Never(
                "no attempt has run, so none of this step's prompt has been copied".into(),
            );
        }
        if self.state.was_cut_short() {
            // The copier named the cause on stderr as it happened.
            return Replay::Never("this step's prompt could not be read to its end".into());
        }
        Replay::Ready
    }

    /// Joins the copier at the end of the step, when it is certain to end
    /// ([`Capture::can_end`]), so that every line it writes is written
    /// before the step's result and a late failure of the source is seen.
    /// A caller still holding the pipe is not waited
    /// for, and neither is a copier never let go, which reads nothing and
    /// ends when the capture is dropped.
    ///
    /// Only a step that ran a rerun joins it. That is the step whose copier
    /// was joined before this review, before its rerun; a step that met no
    /// limit was never held by its copier and is not held by it now.
    fn settle(&mut self) {
        if !self.replayed || !self.can_end() {
            return;
        }
        let Some(copier) = self.copier.take() else {
            return;
        };
        // An `Err` is a failed read, already published as a prompt cut
        // short; a panic is caught by the copier itself. Neither has
        // anything left to say here.
        let _ = copier.join();
    }

    /// Whether the copier is certain to end, so joining it is bounded.
    ///
    /// Either it has already ended, either way, and published that, or no
    /// writer of the source is left, after which what it still has to read
    /// is only what the kernel is already holding, and its writes go to
    /// memory and to a pipe whose reader has gone.
    fn can_end(&self) -> bool {
        self.state.is_complete() || self.state.was_cut_short() || no_writer_left(&self.watch)
    }
}

/// A rerun's stdin for a captured prompt: the read end
/// of a pipe, and a feeder thread that writes the kept copy into the other
/// end from its first byte, then each further byte as the copier keeps it,
/// and closes it when the copy ends.
///
/// The rerun is thereby in the first attempt's position: it reads the same
/// bytes in the same order, and it reaches the end of its stdin when the
/// caller closes the pipe and not before. When the prompt is cut short
/// while it reads, its pipe closes at the cut, after the flag that says so
/// is set, exactly as the first attempt's does, so the supervisor refuses
/// its success in the same way.
///
/// The feeder ends when the copy has ended and every kept byte is written,
/// or when the attempt closes its end (the write then fails with `EPIPE`,
/// which is the attempt's own choice and costs nothing kept). A feeder
/// whose attempt exits while the caller still holds the pipe idles until
/// the caller writes again or the process ends; it holds nothing but its
/// own end of a pipe nobody reads. The pipe is `O_CLOEXEC` on both ends, so
/// only the descriptor `Command` puts on fd 0 reaches the child, as for the
/// first attempt's pipe.
fn fed_from_copy(state: Arc<CopyState>) -> io::Result<Stdio> {
    let (read, mut write) = std::io::pipe()?;
    std::thread::Builder::new()
        .name("amplihack-prompt-replay".to_string())
        .spawn(move || {
            let mut at = 0usize;
            while let Some(chunk) = state.after(at) {
                if write.write_all(&chunk).is_err() {
                    return;
                }
                at = at.saturating_add(chunk.len());
            }
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
/// `POLLERR` and `POLLNVAL` say the same in their own way. Anything else
/// (readable now, or quiet with a writer still holding it) means the prompt
/// may still be on its way, and a call that cannot be made at all is read
/// the same way: the cautious answer is the one that does not join a
/// copier that may never end.
///
/// It is an answer for the instant it is asked, and the two ways it can be
/// wrong both cost only the join at the end of the step, never a byte and
/// never a rerun: a writer that is about to close reads as one that may
/// never, and so does a copy of the descriptor that some other process is
/// holding (every `fork` holds one of each of its parent's descriptors
/// until it `exec`s). Nothing about a rerun depends on it:
/// [`Capture::settle`] asks it once the step has finished.
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
/// An error when stdin cannot be duplicated or the copy cannot be set up
/// ([`capture_prompt`]). Nothing has been read from stdin either way.
pub(crate) fn capture_own_prompt() -> io::Result<RerunStdin> {
    let owned = io::stdin().as_fd().try_clone_to_owned()?;
    capture_prompt(fs::File::from(owned))
}

/// Prepares a [`Capture`] from `source`, a duplicate of the step's stdin.
/// No byte of `source` is read until [`RerunStdin::attempt_started`].
///
/// Everything the copy needs is made here, before any attempt runs: a
/// second description of the source, the first attempt's stdin pipe, and
/// the copier thread, which waits until it is let go. An error means one of
/// them could not be made, which happens only to a process out of file
/// descriptors or threads. Nothing has then been read from `source`, and
/// the caller must not run the step: an attempt run on a prompt of which no
/// copy is kept could not be repeated after a limit.
/// Nothing here refuses a prompt for its size or its contents.
pub(crate) fn capture_prompt(source: fs::File) -> io::Result<RerunStdin> {
    refused_setup()?;
    let watch = source.try_clone()?;
    // `std::io::pipe` is `O_CLOEXEC` on both ends, so only the descriptor
    // `Command` puts on fd 0 reaches the child.
    let (child_read, child_write) = std::io::pipe()?;
    let state = Arc::new(CopyState::default());
    let published = Arc::clone(&state);
    let (go, armed) = mpsc::channel::<()>();
    let copier = std::thread::Builder::new()
        .name("amplihack-prompt-capture".to_string())
        .spawn(move || run_copier(&armed, source, child_write, &published))?;
    Ok(RerunStdin::Captured(Capture {
        go: Some(go),
        first: Some(fs::File::from(std::os::fd::OwnedFd::from(child_read))),
        watch,
        copier: Some(copier),
        replayed: false,
        state,
        failure: None,
    }))
}

/// The copier thread: waits to be let go, then copies, and publishes a
/// panic in the copy as a prompt cut short.
///
/// A capture dropped before the first attempt ran drops the sender, and the
/// wait ends with nothing read, so the plain launch that follows a stand-
/// down inherits fd 0 whole (issue #2816).
///
/// The attempt's pipe is held here and not by [`copy_prompt`], so that a
/// panic inside the copy cannot drop it before the loss is published: the
/// attempt reaches end of file only after `cut_short` is stored, as it does
/// after a failed read.
///
/// `source` is taken by bound for the reason [`copy_prompt`] gives: a test
/// can hand it a source whose read panics, which no real descriptor does.
pub(super) fn run_copier<S: Read + AsRawFd>(
    armed: &mpsc::Receiver<()>,
    mut source: S,
    child: std::io::PipeWriter,
    state: &CopyState,
) -> io::Result<()> {
    if armed.recv().is_err() {
        return Ok(());
    }
    let mut child = Some(child);
    let copied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        copy_prompt(&mut source, &mut child, state)
    }));
    match copied {
        Ok(result) => result,
        Err(_) => {
            let e = io::Error::other("the thread copying this step's prompt panicked");
            state.cut_short(&e);
            Err(e)
        }
    }
}

/// Debug-only seam: `AMPLIHACK_TEST_FAIL_PROMPT_COPY=1` makes the copy's
/// setup fail as it does in a process out of file descriptors (`EMFILE`),
/// so a test can check that such a step is not run at all. The real cause
/// cannot be arranged from outside a test without
/// starving the test's own process of descriptors. Compiled out of release
/// builds, as the usage-limit wait's test clock and
/// `AMPLIHACK_TEST_FAIL_ATTEMPT` are, so no installed binary reads this
/// variable.
#[cfg(debug_assertions)]
fn refused_setup() -> io::Result<()> {
    match std::env::var_os("AMPLIHACK_TEST_FAIL_PROMPT_COPY") {
        Some(v) if !v.is_empty() => Err(io::Error::from_raw_os_error(libc::EMFILE)),
        _ => Ok(()),
    }
}

#[cfg(not(debug_assertions))]
fn refused_setup() -> io::Result<()> {
    Ok(())
}

/// Pumps `source` into `child`, the first attempt's stdin, and into the
/// kept copy, until end of file. `state` is marked complete only then.
///
/// The prompt is opaque bytes throughout: never decoded and never printed.
/// Each block is kept before it is forwarded, so whatever the attempt was
/// given, a rerun is given too. A first attempt that
/// exits without reading closes its end, and the `EPIPE` that follows only
/// stops the copy to the child; the kept copy still receives all of it, and
/// [`CopyState::reader_gone`] reports the early close.
///
/// A failed read of the source is the one loss, and the one the attempt
/// cannot be told about: there are no further bytes to forward, so the copy
/// ends, and the attempt's pipe closes. Closing it is the only signal a
/// pipe gives, and a reader takes it for the end of the prompt.
/// [`CopyState::cut_short`] therefore publishes the loss, says so on stderr
/// the moment it happens, and the supervisor refuses to report the
/// attempt's success as the step's (issue #2816). `EAGAIN` is not such a
/// failure: an inherited descriptor may carry `O_NONBLOCK` on a description
/// shared with the writer, and this waits for the next byte as a blocking
/// read would have.
///
/// `child` is dropped (set to `None`) once the copy has ended, and on a
/// failed read only after the loss is published, so the attempt's end of
/// file never comes before the flag that says what it means.
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
/// `AsRawFd` yields, which is the source's own.
pub(super) fn copy_prompt<S: Read + AsRawFd>(
    source: &mut S,
    child: &mut Option<std::io::PipeWriter>,
    state: &CopyState,
) -> io::Result<()> {
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
                Err(e) => return Err(lost(state, child, e)),
            },
            // There are no further bytes to forward, so the source failing
            // ends the copy. The attempt's pipe closes once the loss is
            // published, which that attempt reads as the end of its prompt;
            // `cut_short` is what keeps that from passing for a whole one.
            Err(e) => return Err(lost(state, child, e)),
        };
        let chunk = &buf[..k];
        state.keep(chunk);
        if let Some(w) = child.as_mut()
            && w.write_all(chunk).is_err()
        {
            // The attempt is gone, or stopped reading, with prompt still
            // to come. Its prompt is still kept; closing our end lets it
            // see end of file if it is somehow still there. The early
            // close is reported, since the runner's own warning for it
            // cannot fire for a supervised step (issue #2816).
            *child = None;
            state.reader_gone();
        }
    }
    state.done();
    *child = None;
    Ok(())
}

/// Publishes a prompt cut short, then closes the attempt's pipe, in that
/// order (issue #2816).
fn lost(state: &CopyState, child: &mut Option<std::io::PipeWriter>, e: io::Error) -> io::Error {
    state.cut_short(&e);
    *child = None;
    e
}

/// The prompt `state` keeps, for the tests that pin where each byte goes.
#[cfg(test)]
pub(super) fn kept(state: &CopyState) -> Vec<u8> {
    state
        .bytes
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
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
