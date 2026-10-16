//! Usage-limit reruns for nested `-p` launches (recipe agent steps). A
//! supervising amplihack runs the launch as a child, streams its stderr,
//! buffers its stdout, and when the attempt fails with what the recipe
//! runner's own classifier calls a usage limit, reruns it under the next
//! eligible profile. Only the final attempt's stdout is printed.

use super::SUPERVISED_ENV;
use super::candidate::{self, Pool};
use super::config::{self, Config};
use super::fsx;
use super::state::{Ledger, State};
use super::usage::{self, Reading, ReadingSource, Window};
use crate::commands::profile::PROFILE_ENV;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::recipe::run::{FailureClass, classify_failure_text};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const TAIL: usize = 64 * 1024;

/// An eligible profile for a nested launch: not excluded, not exhausted, not
/// in `skip`, holding its own login at home, and not hosting another account.
pub fn nested_candidate(
    state: &State,
    ledger: &Ledger,
    profiles: &[ProfileName],
    readings: &BTreeMap<String, Reading>,
    cfg: &Config,
    current: &str,
    skip: &[String],
) -> Option<ProfileName> {
    let pool = Pool {
        profiles,
        readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now: fsx::now(),
    };
    candidate::select(&pool, current, |c| {
        let n = c.as_str();
        !skip.iter().any(|s| s == n)
            && !ledger.hosts.contains_key(n)
            && ledger.leased_to(n).is_none()
            // macOS keeps logins in the Keychain: no file to look for.
            && (cfg!(target_os = "macos") || fsx::exists(&state.live_creds(n)))
    })
    .cloned()
}

pub fn profile_names(store: &ProfileStore) -> Vec<ProfileName> {
    let names = store.list().unwrap_or_else(|e| {
        eprintln!("auto-switch: cannot list profiles, so none can be substituted: {e}");
        Vec::new()
    });
    names
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect()
}

/// `at 93% of its five-hour limit`: the window that made `r` exhausted.
fn limit_reason(r: &Reading, now: i64) -> String {
    let five = candidate::adjusted(r.five_hour.as_ref(), now);
    let seven = candidate::adjusted(r.seven_day.as_ref(), now);
    let (pct, window) = if five >= seven {
        (five, "five-hour")
    } else {
        (seven, "seven-day")
    };
    format!("at {pct:.0}% of its {window} limit")
}

/// The note printed when a nested launch is moved to another profile.
pub fn substitution_note(from: &ProfileName, reason: &str, to: &ProfileName) -> String {
    format!(
        "amplihack: profile {:?} is {reason}; auto-switch is using profile {:?} for this launch",
        from.as_str(),
        to.as_str()
    )
}

/// The profile a nested launch inheriting `inherited` should use, when the
/// account live there is exhausted and another profile has quota left, with
/// the reason for the note.
pub fn substitute(
    store: &ProfileStore,
    state: &State,
    cfg: &Config,
    inherited: &ProfileName,
) -> Option<(ProfileName, String)> {
    if !state.exists() {
        return None; // no readings yet
    }
    let ledger = match state.load_ledger() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("auto-switch: no substitute for this launch: {e}");
            return None;
        }
    };
    let account = ledger.account_in(inherited.as_str()).to_string();
    let readings = state.load_usage_or_warn().accounts;
    let now = fsx::now();
    let reading = readings.get(&account)?;
    if !candidate::is_exhausted(Some(reading), cfg.threshold, now) {
        return None;
    }
    let reason = limit_reason(reading, now);
    let profiles = profile_names(store);
    let skip = vec![inherited.to_string()];
    nested_candidate(state, &ledger, &profiles, &readings, cfg, &account, &skip)
        .map(|c| (c, reason))
}

fn record_limit(state: &State, account: &str, reset: i64) {
    let _lock = match state.lock() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("auto-switch: usage limit for this account not recorded: {e}");
            return;
        }
    };
    let mut file = state.load_usage_or_warn();
    let now = fsx::now();
    let seven_day = file.accounts.get(account).and_then(|r| r.seven_day);
    file.accounts.insert(
        account.to_string(),
        Reading {
            five_hour: Some(Window {
                used_percentage: 100.0,
                resets_at: reset,
            }),
            seven_day,
            observed_at: now,
            source: ReadingSource::UsageLimitFailure,
        },
    );
    if let Err(e) = state.save_usage(&file) {
        eprintln!("auto-switch: {e}");
    }
}

struct Attempt {
    code: i32,
    signalled: bool,
    stderr_tail: Vec<u8>,
    stdout: fs::File,
    stdout_path: std::path::PathBuf,
}

/// Runs the launch under supervision and returns its final exit code.
/// `Ok(None)` when the supervisor could not start, so the caller launches
/// unsupervised; once an attempt has run, errors are returned instead, so a
/// step is never run twice by accident.
pub fn supervise(
    store: &ProfileStore,
    state: &State,
    cfg: &Config,
    inherited: ProfileName,
) -> Result<Option<i32>> {
    let rerun_dir = state.dir.join("rerun");
    let setup = state
        .ensure()
        .and_then(|()| state.ensure_sub(&rerun_dir))
        .map_err(anyhow::Error::from)
        .and_then(|()| std::env::current_exe().context("cannot find the amplihack binary"));
    let exe = match setup {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("warning: auto-switch cannot supervise this step ({e}); running it once");
            return Ok(None);
        }
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let signals = match Signals::register() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "warning: auto-switch cannot catch stop signals ({e}); running this step once"
            );
            return Ok(None);
        }
    };

    let mut profile = inherited.clone();
    if let Some((c, reason)) = substitute(store, state, cfg, &inherited) {
        eprintln!("{}", substitution_note(&inherited, &reason, &c));
        profile = c;
    }
    let mut tried: Vec<String> = Vec::new();
    let mut n = 0u32;
    loop {
        n += 1;
        let attempt = match run_attempt(
            &exe,
            &args,
            state,
            &rerun_dir,
            (profile != inherited).then_some(&profile),
            n,
            &signals,
        ) {
            Ok(a) => a,
            Err(e) if n == 1 && profile == inherited => {
                eprintln!("warning: auto-switch cannot supervise this step ({e}); running it once");
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let account = state
            .load_ledger()
            .map(|l| l.account_in(profile.as_str()).to_string());
        tried.push(profile.to_string());
        let limited = usage_limited(&attempt);
        let eff = limited.map(|_| config::effective(&state.root));
        if let (Some(reset), Some(eff)) = (limited, eff)
            && eff.on
        {
            // Without the ledger the account is unknown: recording the limit
            // under the profile name could mark the wrong account exhausted.
            let account = match account {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("auto-switch: no rerun: {e}");
                    return finish(attempt, None);
                }
            };
            record_limit(state, &account, reset);
            let cfg = eff.config;
            let next = match state.load_ledger() {
                Ok(ledger) => {
                    let readings = state.load_usage_or_warn().accounts;
                    nested_candidate(
                        state,
                        &ledger,
                        &profile_names(store),
                        &readings,
                        &cfg,
                        &account,
                        &tried,
                    )
                }
                Err(e) => {
                    eprintln!("auto-switch: no rerun: {e}");
                    None
                }
            };
            if let (Some(_), Some(sig)) = (&next, signals.received()) {
                eprintln!("auto-switch: no rerun: this step was told to stop");
                return finish(attempt, Some(128 + sig));
            }
            if let Some(next) = next {
                eprintln!(
                    "amplihack: profile {:?} hit its usage limit; rerunning this step under profile {:?} (auto-switch)",
                    profile.as_str(),
                    next.as_str()
                );
                drop(attempt.stdout);
                let _ = fs::remove_file(&attempt.stdout_path);
                profile = next;
                continue;
            }
        }
        return finish(attempt, None);
    }
}

/// Prints `attempt`'s buffered stdout and returns its exit code, or `code`
/// when given.
fn finish(attempt: Attempt, code: Option<i32>) -> Result<Option<i32>> {
    let mut out = attempt.stdout;
    out.seek(SeekFrom::Start(0))?;
    let mut stdout = std::io::stdout().lock();
    std::io::copy(&mut out, &mut stdout)?;
    stdout.flush()?;
    drop(out);
    let _ = fs::remove_file(&attempt.stdout_path);
    Ok(Some(code.unwrap_or(attempt.code)))
}

/// Stop signals, caught once for the whole supervised run. signal-hook does
/// not restore the default action on unregister, so handlers registered per
/// attempt would leave the supervisor deaf between attempts.
struct Signals {
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
    fn register() -> std::io::Result<Self> {
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
    fn received(&self) -> Option<i32> {
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

fn run_attempt(
    exe: &std::path::Path,
    args: &[std::ffi::OsString],
    state: &State,
    rerun_dir: &std::path::Path,
    switch_to: Option<&ProfileName>,
    n: u32,
    signals: &Signals,
) -> Result<Attempt> {
    let stdout_path = rerun_dir.join(format!("{}-{n}.out", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let stdout = opts.open(&stdout_path)?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env(SUPERVISED_ENV, "1")
        .stdin(Stdio::inherit())
        .stdout(stdout.try_clone()?)
        .stderr(Stdio::piped());
    if let Some(profile) = switch_to {
        cmd.env(PROFILE_ENV, profile.as_str())
            .env("CLAUDE_CONFIG_DIR", state.profile_dir(profile.as_str()));
    }
    let mut child = cmd
        .spawn()
        .context("failed to start the supervised launch")?;
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

    let status = loop {
        signals.forward(child.id() as libc::pid_t);
        if let Some(s) = child.try_wait()? {
            break s;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stderr_tail = reader.join().unwrap_or_default();
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

/// The reset time to record when `attempt` hit its usage limit, else `None`.
fn usage_limited(attempt: &Attempt) -> Option<i64> {
    if attempt.signalled || attempt.code == 0 {
        return None; // no need to read the output
    }
    let out_tail = tail(&attempt.stdout).unwrap_or_else(|e| {
        eprintln!("auto-switch: could not read this step's buffered output: {e}");
        Vec::new()
    });
    limit_reset(
        attempt.signalled,
        attempt.code,
        &attempt.stderr_tail,
        &out_tail,
        fsx::now(),
    )
}

/// An attempt hit its usage limit only when it was not killed by a signal,
/// exited non-zero, and the recipe runner's classifier calls its stderr tail
/// followed by its stdout tail `UsageLimit`. Work and environmental markers
/// outrank a usage limit there, so a real failure that also mentions one is
/// not rerun. Returns the reset time to record.
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
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stderr),
        String::from_utf8_lossy(stdout)
    );
    (classify_failure_text(&text).0 == FailureClass::UsageLimit)
        .then(|| usage::reset_or_default(stderr, stdout, now))
}

/// The last 64 KiB of the buffered stdout, read through the attempt's own
/// handle rather than by reopening its path.
fn tail(file: &fs::File) -> std::io::Result<Vec<u8>> {
    let mut f = file.try_clone()?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL as u64)))?;
    fsx::read_bounded(f, TAIL as u64)
}
