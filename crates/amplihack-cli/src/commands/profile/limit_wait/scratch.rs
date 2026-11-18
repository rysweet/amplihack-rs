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
//! on it.
//!
//! The first attempt therefore inherits stdin unchanged, except for a
//! captured prompt, which it reads from the copier's pipe. A rerun gets a
//! regular file again from where the step started reading it (on Linux
//! through its own open of `/proc/self/fd/0`, so no read offset is shared
//! with other processes), the capture file from offset 0 when there is one,
//! a terminal unchanged, and anything else (a pipe with no capture, a
//! socket, a closed descriptor) `/dev/null`: what the first attempt read
//! from it is gone.

use super::super::fsx;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
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
        // A captured prompt is the only kind every attempt reads, so it is
        // decided before the `n == 1` shortcut.
        if let RerunStdin::Captured(c) = self {
            return c.for_attempt(n);
        }
        if n == 1 {
            return Ok(Stdio::inherit());
        }
        match self {
            RerunStdin::Inherit => Ok(Stdio::inherit()),
            RerunStdin::File { file, start } => {
                let mut f = reopen(file).map_or_else(|| file.try_clone(), Ok)?;
                f.seek(SeekFrom::Start(*start))?;
                Ok(Stdio::from(f))
            }
            RerunStdin::Pipe | RerunStdin::Null => Ok(Stdio::null()),
            RerunStdin::Captured(_) => unreachable!("handled above"),
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
    /// Set by the copier only after end of file and a complete write.
    complete: Arc<AtomicBool>,
    /// Why the capture cannot be replayed, once that is known.
    failure: Option<String>,
}

impl std::fmt::Debug for Capture {
    /// Never prints the prompt, nor the capture path, which no log line
    /// carries either.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("complete", &self.complete.load(Ordering::SeqCst))
            .field("failed", &self.failure.is_some())
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
        self.settle();
        if let Some(why) = &self.failure {
            return Err(io::Error::other(format!(
                "this step's prompt was not stored whole, so it cannot be given to attempt {n}: \
                 {why}"
            )));
        }
        let mut f = self.reader.try_clone()?;
        f.seek(SeekFrom::Start(0))?;
        Ok(Stdio::from(f))
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
            Ok(Ok(())) if self.complete.load(Ordering::SeqCst) => None,
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

fn start_capture(dir: &Path, mut source: fs::File) -> io::Result<Capture> {
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
    let reader = match read_back(&path, &writer) {
        Ok(r) => r,
        Err(e) => {
            drop(writer);
            let _ = fs::remove_file(&path);
            return Err(e);
        }
    };
    // `std::io::pipe` is `O_CLOEXEC` on both ends, so only the descriptor
    // `Command` puts on fd 0 reaches the child.
    let (child_read, child_write) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => {
            drop(writer);
            drop(reader);
            let _ = fs::remove_file(&path);
            return Err(e);
        }
    };
    let complete = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&complete);
    let copier = std::thread::Builder::new()
        .name("amplihack-prompt-capture".to_string())
        .spawn(move || copy_prompt(&mut source, writer, child_write, &flag));
    let copier = match copier {
        Ok(h) => h,
        Err(e) => {
            drop(reader);
            let _ = fs::remove_file(&path);
            return Err(e);
        }
    };
    Ok(Capture {
        path,
        reader,
        first: Some(fs::File::from(std::os::fd::OwnedFd::from(child_read))),
        copier: Some(copier),
        complete,
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
/// until end of file. Sets `complete` only then.
///
/// The prompt is opaque bytes throughout: never decoded, never held whole
/// in memory and never printed. A first attempt that exits without reading
/// closes its end, and the `EPIPE` that follows only stops the copy to the
/// child; the file still receives all of it. Any error on the file is the
/// copy failing, and a rerun is refused rather than given part of a
/// prompt.
fn copy_prompt(
    source: &mut fs::File,
    mut file: fs::File,
    child: std::io::PipeWriter,
    complete: &AtomicBool,
) -> io::Result<()> {
    let mut child = Some(child);
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let k = match source.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        file.write_all(&buf[..k])?;
        if let Some(w) = &mut child
            && w.write_all(&buf[..k]).is_err()
        {
            // The attempt is gone, or stopped reading. Its prompt is still
            // stored; closing our end lets it see end of file if it is
            // somehow still there.
            child = None;
        }
    }
    file.flush()?;
    complete.store(true, Ordering::SeqCst);
    Ok(())
}
