//! `amplihack profile usage` (issue #1578): runs `claude -p /usage` under
//! each profile in parallel, prints the session and weekly usage ordered by
//! capacity, and records what it read as auto-switch readings.
//!
//! Claude's text is kept only with `--raw`; error rows carry one of four
//! fixed reasons. The parent environment is never changed: each child gets
//! `CLAUDE_CONFIG_DIR` on its own `Command`, or, for the profile whose login
//! is live in `~/.claude` (see `default_live`), has it removed.

use super::auto_switch::{self, Observation, ObservedWindow, RecordOutcome};
use super::name::ProfileName;
use super::store::ProfileStore;
use super::usage_parse::{UsageReport, UsageState, UsageWindow, parse_usage};
use super::{default_live, pointer, run};
use crate::binary_finder::BinaryInfo;
use anyhow::Result;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) const TIMEOUT_ENV: &str = "AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub(super) const MAX_TIMEOUT_SECS: u64 = 600;
/// Probes running at once.
const MAX_PARALLEL: usize = 8;
/// Bytes kept from each of stdout and stderr; the rest is read and dropped.
const STREAM_CAP: usize = 64 * 1024;
/// How long to wait for the pipes to close after the child exits. Only a
/// process that left the child's group can still hold them open; it is not
/// waited for, and the probe fails with [`ErrorReason::ReadFailed`].
const READ_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(25);
/// Reset text and labels are cut to this many characters in the table.
const MAX_TEXT: usize = 64;
/// Set by `profile rotate watch` on SIGTERM or SIGINT: running probes stop.
pub(super) static PROBE_CANCEL: std::sync::LazyLock<Arc<AtomicBool>> =
    std::sync::LazyLock::new(Arc::default);
/// With one of these set, `claude` may report that credential's usage rather
/// than the profile login's, so nothing is recorded.
const ENV_CREDENTIAL_VARS: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Why a probe could not run. Fixed text: never an I/O error or a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorReason {
    ProfileRefused,
    StartFailed,
    ReadFailed,
    WaitFailed,
}

impl ErrorReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ProfileRefused => "profile directory refused",
            Self::StartFailed => "could not start claude",
            Self::ReadFailed => "could not read claude output",
            Self::WaitFailed => "could not wait for claude",
        }
    }
}

/// What `claude` printed, kept only with `--raw`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Raw {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// The outcome of one `claude -p /usage` run.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Probe {
    pub(super) report: UsageReport,
    pub(super) error: Option<ErrorReason>,
    pub(super) raw: Option<Raw>,
}

impl Probe {
    fn failed(reason: ErrorReason, raw: Option<Raw>) -> Self {
        Self {
            report: UsageReport::empty(UsageState::Error),
            error: Some(reason),
            raw,
        }
    }
}

/// One output row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) profile: String,
    /// The account live in the profile's directory.
    pub(crate) account: String,
    pub(crate) default: bool,
    /// The directory this profile's login is lent to.
    pub(crate) leased_to: Option<String>,
    /// The login is live in `~/.claude` and the numbers were read there.
    pub(crate) live_in: bool,
    /// Marked as credit-backed overflow in `<root>/.auto-switch`.
    pub(crate) overflow: bool,
    /// The end of the rotator's active spend block on this overflow
    /// profile, epoch seconds.
    pub(crate) overflow_blocked_until: Option<i64>,
    /// The `~/.claude` row: a login no profile holds (issue #1693). Shown,
    /// never recorded.
    pub(crate) live_in_home: bool,
    pub(crate) report: UsageReport,
    pub(crate) error: Option<ErrorReason>,
    pub(crate) raw: Option<Raw>,
}

pub(super) fn run(
    store: &ProfileStore,
    names: &[String],
    json: bool,
    raw: bool,
    timeout_flag: Option<u64>,
) -> Result<()> {
    super::reject_docker_env()?;
    let (secs, warning) = timeout_secs(timeout_flag, std::env::var_os(TIMEOUT_ENV).as_deref());
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    // Shown whether or not auto-switch is set up: every row may hold that
    // credential's numbers.
    if let Some(var) = env_credential_var(|k| std::env::var_os(k)) {
        eprintln!("{}", env_credential_warning(var));
    }
    let profiles = selected_profiles(store, names)?;
    if profiles.is_empty() {
        print_empty(json);
        return Ok(());
    }
    let binary = run::resolve_claude_binary()?;
    let home = home_dir();
    let (live, home_row) = detect_live(store, home.as_deref(), &profiles);
    let live_name = live.as_ref().map(|l| l.name.clone());
    // Read before probing: the account each row is recorded under must be
    // the one live when its probe started, so that a swap during the probe
    // shows up in the check under the lock as a moved account.
    let marks = read_marks(store);

    let now = now_secs();
    let probes = probe_all(
        store,
        &binary,
        &profiles,
        live_name.as_ref(),
        Duration::from_secs(secs),
        now,
        raw,
        &PROBE_CANCEL,
    );
    // The login in `~/.claude` may have been swapped during the probe; its
    // numbers are shown but not recorded against the profile.
    let swapped = swapped_live(store, home.as_deref(), live);
    let mut rows = build_rows(&marks, &profiles, probes, live_name.as_ref());
    // Only when every profile is listed: naming profiles asks about those.
    if home_row && names.is_empty() {
        let cmd = run::build_home_command(&binary, &["-p".into(), "/usage".into()]);
        rows.push(home_row_of(probe(cmd, Duration::from_secs(secs), now, raw)));
    }
    order(&mut rows, now_secs());
    print_output(&rows, json, swapped.as_ref());
    #[cfg(unix)]
    for (a, b) in super::duplicates::pairs(&super::duplicates::accounts(store, home.as_deref())) {
        eprintln!("warning: {}", super::duplicates::warning(&a, &b));
    }
    record(store, &rows, swapped.as_ref(), json);
    Ok(())
}

/// Issue #1710: the rows `profile usage` would show, for `workstreams
/// status`. Probes like [`run`], at most `timeout` each, and stops when
/// `cancel` is set, killing each probe's process group. Never records or
/// persists anything and prints nothing; refuses to run where profiles are
/// not applied (Docker). The `~/.claude` row is added only when `names` is
/// empty, as in [`run`].
pub(crate) fn probe_rows(
    store: &ProfileStore,
    binary: &BinaryInfo,
    names: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<Row>> {
    super::reject_docker_env()?;
    let profiles = selected_profiles(store, names)?;
    if profiles.is_empty() {
        return Ok(Vec::new());
    }
    let home = home_dir();
    // Detection warnings are for `profile usage`; this caller shows rows only.
    let (live, home_row) = match default_live::detect_for_usage(store, home.as_deref()) {
        default_live::UsageLive::Profile(name) => (Some(name), false),
        default_live::UsageLive::Home => (None, true),
        default_live::UsageLive::Detected(live, _) => (live.map(|l| l.name), false),
    };
    let live = live.filter(|n| profiles.contains(n));
    let marks = Marks {
        default: pointer::read(store.root()).ok().flatten(),
        lease: auto_switch::lease_view(store.root()).ok().flatten(),
    };
    let now = now_secs();
    let probes = probe_all(
        store,
        binary,
        &profiles,
        live.as_ref(),
        timeout,
        now,
        false,
        cancel,
    );
    let mut rows = build_rows(&marks, &profiles, probes, live.as_ref());
    if home_row && names.is_empty() && !cancel.load(Ordering::Relaxed) {
        let cmd = run::build_home_command(binary, &["-p".into(), "/usage".into()]);
        rows.push(home_row_of(probe_cancellable(
            cmd, timeout, now, false, cancel,
        )));
    }
    Ok(rows)
}

/// Issue #1710: every profile in the store for this environment, with its
/// directory, for `workstreams status`. Refuses where profiles are not
/// applied (Docker). The error text names no path.
pub(crate) fn listed_profiles() -> Result<Vec<(String, std::path::PathBuf)>> {
    super::reject_docker_env()?;
    let store = ProfileStore::from_env().map_err(super::with_hint)?;
    let mut out = Vec::new();
    for raw in store.list().map_err(super::with_hint)? {
        let name = ProfileName::parse(&raw)?;
        // A profile that no longer resolves still gets a row, with no
        // directory to inspect.
        let dir = store.resolve(&name).unwrap_or_default();
        out.push((raw, dir));
    }
    Ok(out)
}

/// Issue #1710: [`probe_rows`] against the store for this environment and
/// the `claude` binary `profile usage` would use.
pub(crate) fn probe_listed(
    names: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<Row>> {
    let store = ProfileStore::from_env().map_err(super::with_hint)?;
    let binary = run::resolve_claude_binary()?;
    probe_rows(&store, &binary, names, timeout, cancel)
}

/// `$HOME` as an absolute path; `None` when it is unset, empty or relative.
pub(super) fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|h| h.is_absolute())
}

fn print_empty(json: bool) {
    if json {
        println!("[]");
    } else {
        println!("no profiles; create one with `amplihack profile add <name>`");
    }
}

/// Records every row's numbers except a swapped profile's.
fn record(store: &ProfileStore, rows: &[Row], swapped: Option<&ProfileName>, json: bool) {
    let mut observed = observations(rows);
    observed.retain(|o| swapped.is_none_or(|s| o.profile != s.as_str()));
    persist(store, &observed, json);
}

/// The selected profile whose login is live in `~/.claude`, printing at
/// most one warning, and none when every selected profile has its own login
/// (so none of them could be live); and whether `~/.claude` holds a login no
/// profile has, after a manual `/login` (issue #1693).
fn detect_live(
    store: &ProfileStore,
    home: Option<&std::path::Path>,
    profiles: &[ProfileName],
) -> (Option<default_live::Live>, bool) {
    let (live, warning) = match default_live::detect_for_usage(store, home) {
        default_live::UsageLive::Profile(name) => (Some(default_live::Live::holder(name)), None),
        default_live::UsageLive::Home => return (None, true),
        default_live::UsageLive::Detected(live, warning) => (live, warning),
    };
    if let Some(w) = warning.filter(|w| live_warning_applies(store, profiles, w)) {
        eprintln!("{w}");
    }
    (live.filter(|l| profiles.contains(&l.name)), false)
}

/// The `~/.claude` row, probed with no `CLAUDE_CONFIG_DIR`.
fn home_row_of(p: Probe) -> Row {
    Row {
        profile: default_live::LABEL.to_string(),
        account: default_live::LABEL.to_string(),
        default: false,
        leased_to: None,
        live_in: true,
        // A login no profile holds is never marked overflow.
        overflow: false,
        overflow_blocked_until: None,
        live_in_home: true,
        report: p.report,
        error: p.error,
        raw: p.raw,
    }
}

/// Whether a live-detection warning can concern any selected profile. The
/// own-login warning always can: the profile it is about has a login, so
/// [`default_live::could_be_live`] says nothing about it.
pub(super) fn live_warning_applies(
    store: &ProfileStore,
    profiles: &[ProfileName],
    warning: &str,
) -> bool {
    warning == default_live::OWN_LOGIN_WARNING
        || profiles
            .iter()
            .any(|n| default_live::could_be_live(store, n))
}

/// The live profile, if the login in `~/.claude` was swapped while it was
/// probed. Repeats the detection, so it misses a profile that became live
/// during the run and a swap away and back under a ledger without `since`.
fn swapped_live(
    store: &ProfileStore,
    home: Option<&std::path::Path>,
    live: Option<default_live::Live>,
) -> Option<ProfileName> {
    live.filter(|l| !default_live::still_live(store, home, l))
        .map(|l| l.name)
}

/// The default profile and the auto-switch leases, each `None` (with a
/// warning) when it cannot be read, and the overflow profiles with the
/// rotator's active spend blocks.
struct Marks {
    default: Option<ProfileName>,
    lease: Option<auto_switch::LeaseView>,
    overflow: Vec<ProfileName>,
    blocked: std::collections::BTreeMap<String, i64>,
}

fn read_marks(store: &ProfileStore) -> Marks {
    let default = pointer::read(store.root()).unwrap_or_else(|e| {
        eprintln!("warning: could not read the default profile ({e}); no default mark");
        None
    });
    let lease = auto_switch::lease_view(store.root()).unwrap_or_else(|e| {
        eprintln!("warning: could not read auto-switch leases ({e}); no lease marks");
        None
    });
    let overflow = super::overflow::overflow_names(store.root());
    let blocked = overflow_blocks(store, &overflow);
    Marks {
        default,
        lease,
        overflow,
        blocked,
    }
}

/// The active spend blocks, read only when a profile is marked overflow.
#[cfg(unix)]
fn overflow_blocks(
    store: &ProfileStore,
    overflow: &[ProfileName],
) -> std::collections::BTreeMap<String, i64> {
    if overflow.is_empty() {
        return Default::default();
    }
    super::rotate::overflow_blocks(store.root(), now_secs()).unwrap_or_else(|e| {
        eprintln!("warning: could not read the rotate ledger ({e}); no spend-block marks");
        Default::default()
    })
}

#[cfg(not(unix))]
fn overflow_blocks(
    _store: &ProfileStore,
    _overflow: &[ProfileName],
) -> std::collections::BTreeMap<String, i64> {
    Default::default()
}

/// One row per probe, with the default, lease and `~/.claude` marks.
fn build_rows(
    marks: &Marks,
    profiles: &[ProfileName],
    probes: Vec<Probe>,
    live: Option<&ProfileName>,
) -> Vec<Row> {
    let Marks {
        default,
        lease,
        overflow,
        blocked,
    } = marks;
    profiles
        .iter()
        .zip(probes)
        .map(|(name, p)| {
            let n = name.as_str();
            Row {
                profile: n.to_string(),
                account: lease.as_ref().map_or(n, |l| l.account_in(n)).to_string(),
                default: default.as_ref() == Some(name),
                leased_to: lease
                    .as_ref()
                    .and_then(|l| l.leased_to(n))
                    .map(str::to_string),
                live_in: live == Some(name),
                overflow: overflow.contains(name),
                overflow_blocked_until: overflow
                    .contains(name)
                    .then(|| blocked.get(n).copied())
                    .flatten(),
                live_in_home: false,
                report: p.report,
                error: p.error,
                raw: p.raw,
            }
        })
        .collect()
}

/// The table or JSON on stdout, then the warnings on stderr.
fn print_output(rows: &[Row], json: bool, swapped: Option<&ProfileName>) {
    if json {
        println!("{}", render_json(rows));
    } else {
        print!("{}", render_table(rows));
    }
    for w in error_warnings(rows) {
        eprintln!("{w}");
    }
    if let Some(p) = swapped {
        eprintln!("{}", swap_warning(p));
    }
}

/// The one place the swap message is built; the name is sanitized like
/// every other displayed value.
pub(super) fn swap_warning(name: &ProfileName) -> String {
    format!(
        "warning: the login in ~/.claude changed while profile \"{}\" was probed; its usage was not recorded",
        sanitize_display(name.as_str(), false)
    )
}

/// Named profiles (validated, deduplicated, each must exist), or every
/// profile. Order does not matter here: rows are sorted later.
fn selected_profiles(store: &ProfileStore, names: &[String]) -> Result<Vec<ProfileName>> {
    let mut out: Vec<ProfileName> = Vec::new();
    if names.is_empty() {
        for n in store.list()? {
            out.push(ProfileName::parse(&n)?);
        }
        return Ok(out);
    }
    for raw in names {
        let name = ProfileName::parse(raw)?;
        if out.contains(&name) {
            continue;
        }
        store.resolve(&name)?;
        out.push(name);
    }
    Ok(out)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The probe deadline: the flag, then the environment variable, then 30. A
/// bad variable gives the warning to print, with the value escaped and cut.
pub(super) fn timeout_secs(flag: Option<u64>, env: Option<&OsStr>) -> (u64, Option<String>) {
    if let Some(secs) = flag {
        return (secs, None);
    }
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (DEFAULT_TIMEOUT_SECS, None);
    };
    let text = raw.to_string_lossy();
    match text.parse::<u64>() {
        Ok(secs) if (1..=MAX_TIMEOUT_SECS).contains(&secs) => (secs, None),
        _ => {
            let shown: String = text.chars().take(32).collect();
            let warning = format!(
                "warning: ignored {TIMEOUT_ENV} {shown:?} (use 1 to {MAX_TIMEOUT_SECS}); using {DEFAULT_TIMEOUT_SECS}"
            );
            (DEFAULT_TIMEOUT_SECS, Some(warning))
        }
    }
}

/// The first credential variable set to a non-empty value, by name only.
pub(super) fn env_credential_var(get: impl Fn(&str) -> Option<OsString>) -> Option<&'static str> {
    ENV_CREDENTIAL_VARS
        .into_iter()
        .find(|k| get(k).is_some_and(|v| !v.is_empty()))
}

/// Names the variable, never its value.
pub(super) fn env_credential_warning(var: &str) -> String {
    format!("warning: {var} is set; claude may report that credential's usage, not each profile's")
}

// ---------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------

/// Probes every profile, at most [`MAX_PARALLEL`] at once; `live` is probed
/// against `~/.claude`. Results are in the order of `profiles`. Setting
/// `cancel` stops every running probe and fails the ones not yet started.
#[allow(clippy::too_many_arguments)]
pub(super) fn probe_all(
    store: &ProfileStore,
    binary: &BinaryInfo,
    profiles: &[ProfileName],
    live: Option<&ProfileName>,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
    cancel: &AtomicBool,
) -> Vec<Probe> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Probe>>> = Mutex::new(vec![None; profiles.len()]);
    let args = ["-p".to_string(), "/usage".to_string()];
    std::thread::scope(|s| {
        for _ in 0..profiles.len().min(MAX_PARALLEL) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(name) = profiles.get(i) else {
                        break;
                    };
                    // Built here, right before the spawn, so the directory
                    // check is as late as possible.
                    let build = if live == Some(name) {
                        run::build_default_dir_command
                    } else {
                        run::build_profile_command
                    };
                    let p = match build(binary, store, name, &args) {
                        Ok(_) if cancel.load(Ordering::Relaxed) => Probe {
                            report: UsageReport::empty(UsageState::Timeout),
                            error: None,
                            raw: None,
                        },
                        Ok(cmd) => probe_cancellable(cmd, timeout, now, keep_raw, cancel),
                        // Reported as its fixed reason (and a stderr
                        // warning); the detail can name paths.
                        Err(_) => Probe::failed(ErrorReason::ProfileRefused, None),
                    };
                    results.lock().unwrap_or_else(PoisonError::into_inner)[i] = Some(p);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .into_iter()
        // Each index is taken by exactly one worker, and a worker panic
        // propagates out of the scope, so every slot is filled.
        .map(|p| p.expect("every profile is probed"))
        .collect()
}

/// One stream, read on a detached thread so a process holding the pipe open
/// never blocks the caller.
#[derive(Default)]
struct Stream {
    kept: Mutex<Vec<u8>>,
    done: AtomicBool,
    failed: AtomicBool,
}

impl Stream {
    fn spawn(mut source: impl Read + Send + 'static) -> Arc<Self> {
        let stream = Arc::new(Self::default());
        let shared = Arc::clone(&stream);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match source.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut kept = shared.kept.lock().unwrap_or_else(PoisonError::into_inner);
                        let room = STREAM_CAP.saturating_sub(kept.len());
                        kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        shared.failed.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            shared.done.store(true, Ordering::Release);
        });
        stream
    }

    fn done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    fn text(&self) -> String {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&kept).into_owned()
    }
}

/// Runs `cmd` with stdin from `/dev/null`, reads up to 64 KiB of each output
/// stream, and kills it at the deadline. The exit status is not used.
///
/// On Unix the child leads its own process group, and the whole group is
/// killed when the probe ends, so processes `claude` started (MCP servers,
/// for example) do not outlive it.
pub(super) fn probe(cmd: Command, timeout: Duration, now: i64, keep_raw: bool) -> Probe {
    probe_cancellable(cmd, timeout, now, keep_raw, &PROBE_CANCEL)
}

/// [`probe`], stopped early as a timeout when `cancel` is set.
fn probe_cancellable(
    mut cmd: Command,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
    cancel: &AtomicBool,
) -> Probe {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let start = Instant::now();
    let Ok(mut child) = cmd.spawn() else {
        return Probe::failed(ErrorReason::StartFailed, None);
    };
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        stop(&mut child);
        return Probe::failed(ErrorReason::ReadFailed, None);
    };
    let (out, err) = (Stream::spawn(out), Stream::spawn(err));
    let raw = |out: &Stream, err: &Stream| {
        keep_raw.then(|| Raw {
            stdout: out.text(),
            stderr: err.text(),
        })
    };

    loop {
        match exited(&mut child) {
            Ok(true) => {
                // The child is not reaped yet, so its pid (the group id)
                // cannot have been reused: anything it left in its group
                // goes too, then it is reaped.
                kill_group(&child);
                let _ = child.wait();
                break;
            }
            Ok(false) if start.elapsed() >= timeout || cancel.load(Ordering::Relaxed) => {
                stop(&mut child);
                return Probe {
                    report: UsageReport::empty(UsageState::Timeout),
                    error: None,
                    raw: raw(&out, &err),
                };
            }
            Ok(false) => std::thread::sleep(POLL),
            Err(_) => {
                stop(&mut child);
                return Probe::failed(ErrorReason::WaitFailed, raw(&out, &err));
            }
        }
    }
    let exited = Instant::now();
    while !(out.done() && err.done()) && exited.elapsed() < READ_GRACE {
        std::thread::sleep(Duration::from_millis(10));
    }
    // A pipe still open means the output may be cut short; never parse it.
    if !(out.done() && err.done()) || out.failed() || err.failed() {
        return Probe::failed(ErrorReason::ReadFailed, raw(&out, &err));
    }
    let (stdout, stderr) = (out.text(), err.text());
    Probe {
        report: parse_usage(&stdout, &stderr, now),
        error: None,
        raw: keep_raw.then_some(Raw { stdout, stderr }),
    }
}

/// Kills the child's process group, then kills and reaps the child. These
/// fail only when there is nothing left to kill or reap.
fn stop(child: &mut std::process::Child) {
    kill_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether the child has exited, without reaping it on Unix, so that
/// [`kill_group`] still targets its own group.
#[cfg(unix)]
fn exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    let pid = libc::id_t::from(child.id());
    // SAFETY: an all-zero `siginfo_t` is a valid value for waitid to fill.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `info` is a valid, writable siginfo_t; `pid` is our own
        // unreaped child. WNOWAIT leaves it waitable for `Child::wait`.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            // SAFETY: waitid succeeded, so `si_pid` is initialised; it is 0
            // when the child has not exited yet (WNOHANG).
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

#[cfg(not(unix))]
fn exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    child.try_wait().map(|s| s.is_some())
}

/// Sends SIGKILL to the process group the child leads (see [`probe`]). The
/// group id is the child's pid, and every caller runs this before the child
/// is reaped, so the pid cannot have been reused and this cannot reach
/// another process group.
#[cfg(unix)]
fn kill_group(child: &std::process::Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: `kill` has no memory-safety preconditions. A negative pid
    // targets the group; ESRCH (group already empty) is ignored.
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn kill_group(_child: &std::process::Child) {}

// ---------------------------------------------------------------------------
// Ordering and marks
// ---------------------------------------------------------------------------

/// Usable rows by the higher used window (a missing or already reset window
/// counts as 0), then exhausted rows by when they are back (unknown last),
/// then unavailable rows by state; ties by name.
pub(super) fn order(rows: &mut [Row], now: i64) {
    rows.sort_by(|a, b| {
        let (ga, gb) = (sort_key(&a.report, now), sort_key(&b.report, now));
        ga.0.cmp(&gb.0)
            .then(ga.1.total_cmp(&gb.1))
            .then_with(|| a.profile.cmp(&b.profile))
    });
}

/// `(group, metric)`; a smaller metric sorts first within the group.
fn sort_key(r: &UsageReport, now: i64) -> (u8, f64) {
    match r.state {
        UsageState::Ok => {
            let used = |w: &Option<UsageWindow>| match w {
                Some(w) if w.resets_at.is_none_or(|t| t > now) => w.used_percentage,
                _ => 0.0,
            };
            (0, used(&r.session).max(used(&r.week_all)))
        }
        UsageState::SessionExhausted | UsageState::WeeklyExhausted | UsageState::SpendLimit => {
            (1, back_at(r).map_or(f64::INFINITY, |t| t as f64))
        }
        UsageState::NotLoggedIn => (2, 0.0),
        UsageState::Timeout => (2, 1.0),
        UsageState::Unparsed => (2, 2.0),
        UsageState::Error => (2, 3.0),
    }
}

/// When every window at 100% or more has reset: the latest of their resets.
/// `None` when one has no reset time, when none is at 100% or more, or for a
/// spend limit, which those resets do not lift.
fn back_at(r: &UsageReport) -> Option<i64> {
    if r.state == UsageState::SpendLimit {
        return None;
    }
    let blocking: Vec<&UsageWindow> = [&r.session, &r.week_all]
        .into_iter()
        .flatten()
        .filter(|w| w.used_percentage >= 100.0)
        .collect();
    if blocking.is_empty() {
        return None;
    }
    blocking
        .iter()
        .map(|w| w.resets_at)
        .collect::<Option<Vec<i64>>>()?
        .into_iter()
        .max()
}

/// `*` for the default, `leased→<dir>`, `hosts:<account>`, `live in
/// ~/.claude`, `overflow[, blocked until <time>]`; `-` for none.
pub(super) fn marks(row: &Row) -> String {
    let mut parts = Vec::new();
    if row.default {
        parts.push("*".to_string());
    }
    if let Some(dir) = &row.leased_to {
        parts.push(format!("leased→{dir}"));
    }
    if row.account != row.profile {
        parts.push(format!("hosts:{}", row.account));
    }
    if row.live_in {
        // The wording auto-switch uses for a login live in another directory.
        parts.push(format!("live in {}", default_live::LABEL));
    }
    if row.overflow {
        parts.push(match row.overflow_blocked_until {
            Some(t) => format!("overflow, blocked until {}", utc_time(t)),
            None => "overflow".to_string(),
        });
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(" ")
    }
}

/// UTC time as `YYYY-MM-DDTHH:MM:SSZ`, the rotate log's format.
fn utc_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// A percentage as a whole number when whole, otherwise to one decimal.
pub(crate) fn format_pct(p: f64) -> String {
    if p.fract() == 0.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

/// Replaces control characters (C0, DEL, C1) and bidirectional overrides so
/// untrusted text cannot drive the terminal. `\n` survives only when
/// `keep_newlines`.
pub(crate) fn sanitize_display(s: &str, keep_newlines: bool) -> String {
    s.chars()
        .map(|c| match c {
            '\n' if keep_newlines => '\n',
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => '?',
            c if c.is_control() => '?',
            c => c,
        })
        .collect()
}

fn clip(s: &str) -> String {
    let clean = sanitize_display(s, false);
    if clean.chars().count() <= MAX_TEXT {
        return clean;
    }
    let mut out: String = clean.chars().take(MAX_TEXT).collect();
    out.push('…');
    out
}

/// The human table. Its layout is not a stable interface; `--json` is.
pub(super) fn render_table(rows: &[Row]) -> String {
    const HEADER: [&str; 7] = [
        "PROFILE",
        "MARKS",
        "STATE",
        "SESSION",
        "SESSION RESETS",
        "WEEK",
        "WEEK RESETS",
    ];
    let pct = |w: &Option<UsageWindow>| {
        w.as_ref()
            .map_or("-".into(), |w| format_pct(w.used_percentage))
    };
    let resets = |w: &Option<UsageWindow>| {
        w.as_ref()
            .and_then(|w| w.resets_text.as_deref())
            .map_or("-".into(), clip)
    };
    let cells: Vec<[String; 7]> = rows
        .iter()
        .map(|r| {
            [
                sanitize_display(&r.profile, false),
                sanitize_display(&marks(r), false),
                r.report.state.as_str().to_string(),
                pct(&r.report.session),
                resets(&r.report.session),
                pct(&r.report.week_all),
                resets(&r.report.week_all),
            ]
        })
        .collect();
    let mut widths = HEADER.map(|h| h.chars().count());
    for row in &cells {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |cols: &[String]| {
        let mut s = String::new();
        for (c, w) in cols.iter().zip(widths) {
            s.push_str(c);
            s.extend(std::iter::repeat_n(
                ' ',
                w.saturating_sub(c.chars().count()) + 2,
            ));
        }
        format!("{}\n", s.trim_end())
    };
    let mut out = line(&HEADER.map(String::from));
    for (row, c) in rows.iter().zip(&cells) {
        out.push_str(&line(c));
        if let Some(raw) = &row.raw {
            raw_block(&mut out, "stdout", &raw.stdout);
            raw_block(&mut out, "stderr", &raw.stderr);
        }
    }
    if let Some(r) = rows
        .iter()
        .find(|r| r.live_in && r.overflow && plan_limit_reached(r))
    {
        out.push_str(&format!(
            "running on overflow: {}\n",
            sanitize_display(&r.profile, false)
        ));
    }
    out
}

/// The row's own probe in this run shows a plan limit reached: an
/// exhausted state, or the session or all-models weekly window at 100%.
/// Either is enough. These are the two windows the rotator's capacity test
/// (`plan_usable` in rotate/overflow.rs) reads; a per-model weekly window is
/// not. A failed or spend-limited probe is not evidence of credits.
fn plan_limit_reached(r: &Row) -> bool {
    let full = |w: &UsageWindow| w.used_percentage >= 100.0;
    match r.report.state {
        UsageState::SessionExhausted | UsageState::WeeklyExhausted => true,
        UsageState::Ok => {
            r.report.session.as_ref().is_some_and(full)
                || r.report.week_all.as_ref().is_some_and(full)
        }
        _ => false,
    }
}

fn raw_block(out: &mut String, name: &str, text: &str) {
    out.push_str(&format!("  raw {name}:\n"));
    if text.is_empty() {
        out.push_str("    (empty)\n");
        return;
    }
    let clean = sanitize_display(&text.replace("\r\n", "\n"), true);
    for l in clean.lines() {
        out.push_str(format!("    {l}").trim_end());
        out.push('\n');
    }
}

#[derive(Serialize)]
struct JsonRow<'a> {
    profile: &'a str,
    account: &'a str,
    default: bool,
    leased_to: Option<&'a str>,
    live_in: Option<&'static str>,
    overflow: bool,
    overflow_blocked_until: Option<i64>,
    state: UsageState,
    session: Option<&'a UsageWindow>,
    week_all: Option<&'a UsageWindow>,
    week_models: &'a [UsageWindow],
    error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw: Option<&'a Raw>,
}

/// The stable interface: a JSON array of rows in display order. Strings use
/// JSON escaping and are not otherwise cleaned.
pub(super) fn render_json(rows: &[Row]) -> String {
    let out: Vec<JsonRow<'_>> = rows
        .iter()
        .map(|r| JsonRow {
            profile: &r.profile,
            account: &r.account,
            default: r.default,
            leased_to: r.leased_to.as_deref(),
            live_in: r.live_in.then_some(default_live::LABEL),
            overflow: r.overflow,
            overflow_blocked_until: r.overflow_blocked_until,
            state: r.report.state,
            session: r.report.session.as_ref(),
            week_all: r.report.week_all.as_ref(),
            week_models: &r.report.week_models,
            error: r.error.map(ErrorReason::as_str),
            raw: r.raw.as_ref(),
        })
        .collect();
    // Plain structs of strings, numbers and options always serialise.
    serde_json::to_string_pretty(&out).expect("usage rows serialise to JSON")
}

/// One stderr line per error row.
pub(super) fn error_warnings(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| {
            r.error.map(|e| {
                format!(
                    "warning: profile \"{}\": {}",
                    sanitize_display(&r.profile, false),
                    e.as_str()
                )
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// Rows auto-switch may record: the usable and exhausted ones. A spend limit
/// is not tied to the windows, so it is left out.
pub(super) fn observations(rows: &[Row]) -> Vec<Observation> {
    let observed = |w: &Option<UsageWindow>| {
        w.as_ref().map(|w| ObservedWindow {
            used_percentage: w.used_percentage,
            resets_at: w.resets_at,
        })
    };
    rows.iter()
        .filter(|r| {
            !r.live_in_home
                && matches!(
                    r.report.state,
                    UsageState::Ok | UsageState::SessionExhausted | UsageState::WeeklyExhausted
                )
        })
        .map(|r| Observation {
            profile: r.profile.clone(),
            account: r.account.clone(),
            session: observed(&r.report.session),
            week: observed(&r.report.week_all),
        })
        .collect()
}

/// The stderr lines for a recording outcome. The not-set-up note is for
/// people (table mode) and only where auto-switch exists.
pub(super) fn persist_messages(outcome: &RecordOutcome, json: bool) -> Vec<String> {
    match outcome {
        RecordOutcome::NotSetUp if json || !cfg!(unix) => Vec::new(),
        RecordOutcome::NotSetUp => {
            vec!["note: auto-switch not set up; usage record not updated".to_string()]
        }
        RecordOutcome::Busy => {
            vec!["auto-switch: state is busy; usage record not updated".to_string()]
        }
        RecordOutcome::SwapInProgress => {
            vec!["auto-switch: swap in progress; usage record not updated".to_string()]
        }
        RecordOutcome::Recorded { moved, .. } => moved
            .iter()
            .map(|p| {
                format!(
                    "auto-switch: profile \"{}\" changed accounts during the probe; its usage was not recorded",
                    sanitize_display(p, false)
                )
            })
            .collect(),
    }
}

/// Records the readings after output is printed. Every failure is a warning.
fn persist(store: &ProfileStore, observed: &[Observation], json: bool) {
    let root = store.root();
    let outcome = if !auto_switch::recording_enabled(root) {
        RecordOutcome::NotSetUp
    } else if let Some(var) = env_credential_var(|k| std::env::var_os(k)) {
        eprintln!("auto-switch: {var} is set; usage record not updated");
        return;
    } else {
        match auto_switch::record_usage(root, observed) {
            Ok(outcome) => outcome,
            Err(e) => {
                eprintln!("auto-switch: {e}; usage record not updated");
                return;
            }
        }
    };
    for m in persist_messages(&outcome, json) {
        eprintln!("{m}");
    }
}
