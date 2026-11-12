//! One supervised attempt of a nested `-p` launch, shared by the auto-switch
//! rerun supervisor (#1569) and the usage-limit wait (#1618).
//!
//! An attempt runs the same amplihack command line as a child with
//! `AMPLIHACK_AUTO_SWITCH_SUPERVISED=1` and the stdin `RerunStdin` gives it,
//! streams its stderr while keeping the last 64 KiB, and buffers its stdout
//! in a private file that is printed only for the final attempt.

use super::auto_switch::SUPERVISED_ENV;
use super::fsx;
use super::limit_wait::plan::{self, WaitFor};
use super::name::ProfileName;
use super::{LaunchTarget, PROFILE_ENV};
use crate::commands::recipe::run::limit_patterns::{self, Harness};
use crate::commands::recipe::run::{FailureClass, classify_failure_text};
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const TAIL: usize = 64 * 1024;

pub(crate) struct Attempt {
    code: i32,
    signalled: bool,
    stderr_tail: Vec<u8>,
    stdout: fs::File,
    stdout_path: PathBuf,
}

impl Attempt {
    /// Prints the buffered stdout and returns the exit code, or `code` when
    /// given.
    pub(crate) fn finish(self, code: Option<i32>) -> Result<Option<i32>> {
        let mut out = self.stdout;
        out.seek(SeekFrom::Start(0))?;
        let mut stdout = std::io::stdout().lock();
        std::io::copy(&mut out, &mut stdout)?;
        stdout.flush()?;
        drop(out);
        let _ = fs::remove_file(&self.stdout_path);
        Ok(Some(code.unwrap_or(self.code)))
    }

    /// Drops the buffered stdout of an attempt that is run again.
    pub(crate) fn discard(self) {
        drop(self.stdout);
        let _ = fs::remove_file(&self.stdout_path);
    }
}

/// Stop signals, caught once for the whole supervised run. signal-hook does
/// not restore the default action on unregister, so handlers registered per
/// attempt would leave the supervisor deaf between attempts.
pub(crate) struct Signals {
    term: Arc<AtomicBool>,
    hup: Arc<AtomicBool>,
    /// SIGINT reaches the attempt through the terminal; catching it here
    /// keeps the supervisor alive to pass the attempt's output back.
    int: Arc<AtomicBool>,
    /// The last stop signal seen, or 0.
    seen: AtomicI32,
    ids: Vec<signal_hook::SigId>,
}

impl Signals {
    pub(crate) fn register() -> std::io::Result<Self> {
        let mut s = Self {
            term: Arc::new(AtomicBool::new(false)),
            hup: Arc::new(AtomicBool::new(false)),
            int: Arc::new(AtomicBool::new(false)),
            seen: AtomicI32::new(0),
            ids: Vec::new(),
        };
        for (sig, flag) in [
            (libc::SIGTERM, Arc::clone(&s.term)),
            (libc::SIGHUP, Arc::clone(&s.hup)),
            (libc::SIGINT, Arc::clone(&s.int)),
        ] {
            s.ids.push(signal_hook::flag::register(sig, flag)?);
        }
        Ok(s)
    }

    /// Passes SIGTERM and SIGHUP on to `pid` and remembers every stop signal.
    fn forward(&self, pid: libc::pid_t) {
        for (flag, sig) in [(&self.term, libc::SIGTERM), (&self.hup, libc::SIGHUP)] {
            if flag.swap(false, Ordering::Relaxed) {
                self.seen.store(sig, Ordering::Relaxed);
                // SAFETY: plain kill(2) on our own child's pid.
                unsafe { libc::kill(pid, sig) };
            }
        }
        if self.int.swap(false, Ordering::Relaxed) {
            self.seen.store(libc::SIGINT, Ordering::Relaxed);
        }
    }

    /// The last stop signal received so far, including any not yet forwarded.
    /// No attempt is running here, so nothing is forwarded.
    pub(crate) fn received(&self) -> Option<i32> {
        for (flag, sig) in [
            (&self.term, libc::SIGTERM),
            (&self.hup, libc::SIGHUP),
            (&self.int, libc::SIGINT),
        ] {
            if flag.swap(false, Ordering::Relaxed) {
                self.seen.store(sig, Ordering::Relaxed);
            }
        }
        match self.seen.load(Ordering::Relaxed) {
            0 => None,
            sig => Some(sig),
        }
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

/// Runs attempt `n`. `env` is a profile to switch to and its directory;
/// `None` keeps the inherited environment.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_attempt_with_recovery(
    exe: &Path,
    args: &[std::ffi::OsString],
    rerun_dir: &Path,
    target: Option<&LaunchTarget>,
    stdin: std::io::Result<Stdio>,
    n: u32,
    signals: &Signals,
    recovery: Option<&recipe_runner_rs::recovery::ipc::Client>,
) -> Result<Attempt> {
    let context = target
        .map(|t| t.write_context(rerun_dir, n))
        .transpose()?
        .unwrap_or_else(super::launch_target::LaunchContext::empty);
    let mut forwarding =
        signal_hook::iterator::Signals::new([libc::SIGTERM, libc::SIGHUP, libc::SIGCHLD])?;
    let stdout_path = rerun_dir.join(format!("{}-{n}.out", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let stdout = opts.open(&stdout_path)?;
    let mut cmd = Command::new(exe);
    if let Some(client) = recovery {
        cmd.env(
            recipe_runner_rs::recovery::ipc::ATTACHMENT_ENV,
            client.environment()?,
        );
    }
    let spawned = stdin.and_then(|input| {
        cmd.args(args)
            .env(SUPERVISED_ENV, "1")
            .stdin(input)
            .stdout(stdout.try_clone()?)
            .stderr(Stdio::piped());
        cmd.env_remove(super::launch_target::CONTEXT_ENV);
        if let Some(target) = target {
            target.apply(&mut cmd);
        }
        context.apply(&mut cmd);
        cmd.spawn()
    });
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            drop(stdout);
            let _ = fs::remove_file(&stdout_path);
            return Err(e).context("failed to start the supervised launch");
        }
    };
    let mut pipe = child.stderr.take().context("no stderr pipe")?;
    let reader = std::thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        let mut err = std::io::stderr();
        while let Ok(k) = pipe.read(&mut buf) {
            if k == 0 {
                break;
            }
            let _ = err.write_all(&buf[..k]);
            tail.extend_from_slice(&buf[..k]);
            // Trim only once the buffer doubles, so each byte is moved at
            // most once instead of 64 KiB being shifted on every read.
            if tail.len() > 2 * TAIL {
                tail.drain(..tail.len() - TAIL);
            }
        }
        if tail.len() > TAIL {
            tail.drain(..tail.len() - TAIL);
        }
        tail
    });

    // SIGCHLD provides completion readiness. One worker owns both forwarding
    // and reaping, so it can never signal a PID after it has reaped that child.
    // Registering the iterator before spawn also covers an immediate exit.
    let pid = child.id() as libc::pid_t;
    signals.forward(pid);
    let forwarder = std::thread::spawn(move || -> std::io::Result<std::process::ExitStatus> {
        for signal in forwarding.forever() {
            if matches!(signal, libc::SIGTERM | libc::SIGHUP) {
                // SAFETY: this worker still owns the unreaped child's PID.
                if unsafe { libc::kill(pid, signal) } != 0 {
                    let error = std::io::Error::last_os_error();
                    // An exited child needs only reaping; other failures must
                    // reach the supervisor instead of silently losing a stop.
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }
            // Readiness may belong to another child; inspect only ours. This
            // runs in response to a signal, never on an interval.
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
        }
        Err(std::io::Error::other(
            "supervised signal stream closed before child completion",
        ))
    });
    let status = forwarder
        .join()
        .map_err(|_| anyhow::anyhow!("supervised signal forwarding thread panicked"))??;
    let stderr_tail = reader.join().unwrap_or_else(|_| {
        eprintln!("amplihack: this step's stderr reader thread panicked");
        Vec::new()
    });
    use std::os::unix::process::ExitStatusExt;
    let (code, signalled) = match (status.code(), status.signal()) {
        (Some(c), _) => (c, false),
        (None, Some(sig)) => (128 + sig, true),
        (None, None) => (1, true),
    };
    Ok(Attempt {
        code,
        signalled,
        stderr_tail,
        stdout,
        stdout_path,
    })
}

/// What an attempt that hit its usage limit said about it.
pub(crate) struct Limited {
    /// For the auto-switch ledger: the stated reset or five hours from now.
    pub(crate) ledger_reset: i64,
    /// Whether, and until when, the usage-limit wait may wait.
    pub(crate) wait: WaitFor,
    /// The limit as read, for a capacity wait (issue #1801). `None` for a
    /// spend limit, which keeps its skip whatever reset it states.
    pub(crate) seen: Option<(Harness, Option<i64>)>,
}

/// `Some` when `attempt` hit its usage limit. `now` is the wait's clock.
pub(crate) fn usage_limited(attempt: &Attempt, now: i64) -> Option<Limited> {
    if attempt.signalled || attempt.code == 0 {
        return None; // no need to read the output
    }
    let out_tail = tail(&attempt.stdout).unwrap_or_else(|e| {
        eprintln!("amplihack: could not read this step's buffered output: {e}");
        Vec::new()
    });
    let (err, out) = (&attempt.stderr_tail, &out_tail);
    let text = joined(err, out);
    let ledger_reset = classified_reset(&text, err, out, fsx::now())?;
    let message = limit_patterns::limit_message(err, out, now);
    Some(Limited {
        ledger_reset,
        wait: plan::wait_for_message(message, now),
        seen: message.filter(|m| !m.spend).map(|m| (m.harness, m.reset)),
    })
}

/// Issue #1700: when a failed attempt ran on an API-key profile (`profile`,
/// else the inherited `AMPLIHACK_CLAUDE_PROFILE`, else `~/.claude` holding
/// a login without OAuth) and its output says the key is out of funds,
/// revoked or rate-limited, records the API-key block under `root` (else
/// the resolved profile root), with one ALERT for a new block. The output
/// is matched and dropped: never stored, logged or printed. A subscription
/// attempt is never read here, since `authentication_error` there means
/// "not logged in".
pub(crate) fn note_key_error(
    attempt: &Attempt,
    profile: Option<&ProfileName>,
    root: Option<&Path>,
) {
    use super::kind::{Ctx, Kind};
    use super::rotate::identify::{LoginKind, login_kind};
    if attempt.signalled || attempt.code == 0 {
        return;
    }
    let Some(root) = root
        .map(Path::to_path_buf)
        .or_else(amplihack_utils::rotator_status::resolve_root)
    else {
        return;
    };
    let store = super::store::ProfileStore::new(root);
    let home = super::usage_cmd::home_dir();
    let named = profile.cloned().or_else(|| {
        std::env::var(PROFILE_ENV)
            .ok()
            .and_then(|v| ProfileName::parse(&v).ok())
    });
    let platform = match named {
        Some(p) => Ctx::load(&store, home.as_deref()).kind(&store, &p) == Kind::Platform,
        None => home.as_deref().is_some_and(|h| {
            login_kind(&h.join(".claude").join(super::auto_switch::CREDS)) == LoginKind::NoOAuth
        }),
    };
    if !platform {
        return;
    }
    let found = super::usage_parse::classify_key_error(&key_error_text(attempt));
    let Some((reason, retry_after)) = found else {
        return;
    };
    let now = fsx::now();
    let until = retry_after.and_then(|s| now.checked_add(s));
    super::rotate::platform_block::record_and_alert(store.root(), reason, until, now);
}

/// An attempt hit its usage limit only when it was not killed by a signal,
/// exited non-zero, and the recipe runner's classifier calls its stderr tail
/// followed by its stdout tail `UsageLimit`. Work and environmental markers
/// outrank a usage limit there, so a real failure that also mentions one is
/// not rerun. Returns the reset time to record. `usage_limited` is the
/// production path; this is its test entry point.
#[cfg(test)]
pub fn limit_reset(
    signalled: bool,
    code: i32,
    stderr: &[u8],
    stdout: &[u8],
    now: i64,
) -> Option<i64> {
    if signalled || code == 0 {
        return None;
    }
    classified_reset(&joined(stderr, stdout), stderr, stdout, now)
}

/// The text checked for an API-key error: the stderr tail, or the stdout
/// tail when stderr is empty. Claude reports API errors on stderr, while
/// stdout carries model output that may quote such an error, so stdout is
/// read only when stderr says nothing.
fn key_error_text(attempt: &Attempt) -> String {
    key_error_source(&attempt.stderr_tail, || {
        tail(&attempt.stdout).unwrap_or_else(|e| {
            eprintln!(
                "amplihack: could not read this step's buffered output: {}",
                e.kind()
            );
            Vec::new()
        })
    })
}

/// [`key_error_text`] without the process: `stderr`, or the output of
/// `stdout` (called only when `stderr` is blank).
pub(crate) fn key_error_source(stderr: &[u8], stdout: impl FnOnce() -> Vec<u8>) -> String {
    let err = String::from_utf8_lossy(stderr);
    if !err.trim().is_empty() {
        return err.into_owned();
    }
    String::from_utf8_lossy(&stdout()).into_owned()
}

/// The stderr tail followed by the stdout tail, as the classifier reads it.
fn joined(stderr: &[u8], stdout: &[u8]) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(stderr),
        String::from_utf8_lossy(stdout)
    )
}

/// The reset to record when `text`, built by `joined`, is a usage limit.
fn classified_reset(text: &str, stderr: &[u8], stdout: &[u8], now: i64) -> Option<i64> {
    (classify_failure_text(text).0 == FailureClass::UsageLimit)
        .then(|| super::auto_switch::reset_or_default(stderr, stdout, now))
}

/// The last 64 KiB of the buffered stdout, read through the attempt's own
/// handle rather than by reopening its path.
fn tail(file: &fs::File) -> std::io::Result<Vec<u8>> {
    let mut f = file.try_clone()?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL as u64)))?;
    fsx::read_bounded(f, TAIL as u64)
}
