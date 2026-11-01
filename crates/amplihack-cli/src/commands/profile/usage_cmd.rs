//! `amplihack profile usage` (issue #1578): asks `claude` for its usage under
//! each profile in parallel (the `get_usage` request, else `claude -p
//! /usage`; issue #1698), prints the session and weekly usage ordered by
//! capacity, and records what it read as auto-switch readings.
//!
//! Claude's text is kept only with `--raw`; error rows carry one of four
//! fixed reasons. The parent environment is never changed: each child gets
//! `CLAUDE_CONFIG_DIR` on its own `Command`, or, for the profile whose login
//! is live in `~/.claude` (see `default_live`), has it removed.

use super::auto_switch::{self, Observation, ObservedWindow, RecordOutcome};
use super::name::ProfileName;
use super::probe_limits::ProbeMode;
use super::store::ProfileStore;
use super::usage_parse::{UsageReport, UsageState, UsageWindow, parse_usage};
use super::{ProfileError, default_live, pointer, probe_limits, run, usage_request};
use crate::binary_finder::BinaryInfo;
use anyhow::Result;
use serde::Serialize;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
pub(super) enum ErrorReason {
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
pub(super) struct Raw {
    pub(super) stdout: String,
    pub(super) stderr: String,
}

/// The outcome of one probe.
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
pub(super) struct Row {
    pub(super) profile: String,
    /// The account live in the profile's directory.
    pub(super) account: String,
    pub(super) default: bool,
    /// The directory this profile's login is lent to.
    pub(super) leased_to: Option<String>,
    /// The login is live in `~/.claude` and the numbers were read there.
    pub(super) live_in: bool,
    /// Marked as credit-backed overflow in `<root>/.auto-switch`.
    pub(super) overflow: bool,
    /// The end of the rotator's active spend block on this overflow
    /// profile, epoch seconds.
    pub(super) overflow_blocked_until: Option<i64>,
    /// The `~/.claude` row: a login no profile holds (issue #1693). Shown,
    /// never recorded.
    pub(super) live_in_home: bool,
    /// An API-key profile (issue #1700): not probed, billed per token.
    pub(super) api_key: bool,
    /// The API-key block on an API-key row: its reason and end.
    pub(super) api_key_blocked: Option<(String, i64)>,
    pub(super) report: UsageReport,
    pub(super) error: Option<ErrorReason>,
    pub(super) raw: Option<Raw>,
}

pub(super) fn run(
    store: &ProfileStore,
    names: &[String],
    json: bool,
    raw: bool,
    timeout_flag: Option<u64>,
) -> Result<()> {
    super::reject_docker_env()?;
    let (mode, warning) = probe_limits::probe_mode_from_env();
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let (secs, warning) = probe_limits::timeout_from_env(timeout_flag);
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
    // Issue #1700: API-key profiles are never probed; a probe costs tokens
    // or times out, and there are no plan limits to read.
    let keys = ApiKeys::load(store, home.as_deref(), &profiles);
    let (keyed, profiles): (Vec<ProfileName>, Vec<ProfileName>) =
        profiles.into_iter().partition(|p| keys.names.contains(p));
    let live_name = live.as_ref().map(|l| l.name.clone());
    // Read before probing: the account each row is recorded under must be
    // the one live when its probe started, so that a swap during the probe
    // shows up in the check under the lock as a moved account.
    let marks = read_marks(store);

    let (parallel, warning) = probe_limits::parallel_from_host(profiles.len());
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let now = now_secs();
    let probes = probe_all(
        store,
        &binary,
        &profiles,
        live_name.as_ref(),
        mode,
        Duration::from_secs(secs),
        now,
        raw,
        parallel,
    );
    // The login in `~/.claude` may have been swapped during the probe; its
    // numbers are shown but not recorded against the profile.
    let swapped = swapped_live(store, home.as_deref(), live);
    let mut rows = build_rows(&marks, &profiles, probes, live_name.as_ref());
    rows.extend(keys.rows(&marks, &keyed));
    // Only when every profile is listed: naming profiles asks about those.
    if home_row && names.is_empty() {
        let p = probe_home(&binary, mode, Duration::from_secs(secs), now, raw);
        rows.push(home_row_of(p));
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
        api_key: false,
        api_key_blocked: None,
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

/// The selected API-key profiles (issue #1700), which of them is live in
/// `~/.claude`, and the API-key block.
#[derive(Default)]
struct ApiKeys {
    names: Vec<ProfileName>,
    live: Option<ProfileName>,
    block: Option<(String, i64)>,
}

impl ApiKeys {
    #[cfg(unix)]
    fn load(
        store: &ProfileStore,
        home: Option<&std::path::Path>,
        selected: &[ProfileName],
    ) -> Self {
        let names: Vec<ProfileName> = super::kind::Ctx::load(store, home)
            .platform_profiles(store)
            .into_iter()
            .filter(|p| selected.contains(p))
            .collect();
        if names.is_empty() {
            return Self::default();
        }
        let live = names
            .iter()
            .find(|n| super::rotate::is_live(store, home, n))
            .cloned();
        let ledger = super::rotate::ledger::RotateState::new(store.root())
            .load_ledger()
            .unwrap_or_else(|e| {
                eprintln!(
                    "warning: could not read the rotate ledger ({e}); no API-key block shown"
                );
                None
            });
        let block = ledger.and_then(|l| {
            super::rotate::platform_block::active(&l, now_secs())
                .map(|b| (b.reason.clone(), b.until))
        });
        Self { names, live, block }
    }

    #[cfg(not(unix))]
    fn load(_: &ProfileStore, _: Option<&std::path::Path>, _: &[ProfileName]) -> Self {
        Self::default()
    }

    /// One row per API-key profile, with no reading.
    fn rows(&self, marks: &Marks, names: &[ProfileName]) -> Vec<Row> {
        names
            .iter()
            .map(|name| {
                let n = name.as_str();
                Row {
                    profile: n.to_string(),
                    account: n.to_string(),
                    default: marks.default.as_ref() == Some(name),
                    leased_to: None,
                    live_in: self.live.as_ref() == Some(name),
                    overflow: false,
                    overflow_blocked_until: None,
                    api_key: true,
                    api_key_blocked: self.block.clone(),
                    live_in_home: false,
                    report: UsageReport::empty(UsageState::Ok),
                    error: None,
                    raw: None,
                }
            })
            .collect()
    }
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
                api_key: false,
                api_key_blocked: None,
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

/// Probes every profile by `mode`, at most `parallel` at once (at least
/// one); `live` is probed against `~/.claude`. Results are in the order of
/// `profiles`.
#[allow(clippy::too_many_arguments)]
pub(super) fn probe_all(
    store: &ProfileStore,
    binary: &BinaryInfo,
    profiles: &[ProfileName],
    live: Option<&ProfileName>,
    mode: ProbeMode,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
    parallel: usize,
) -> Vec<Probe> {
    pooled(profiles.len(), parallel, |i| {
        let name = &profiles[i];
        probe_profile(
            store,
            binary,
            name,
            live == Some(name),
            mode,
            timeout,
            now,
            keep_raw,
        )
    })
}

/// Probes one profile by `mode`, against `~/.claude` when `live`. A profile
/// marked as overflow is probed with `/usage` only, the one method that
/// reports the spend limit. The command is built inside the probe, right
/// before each spawn, so the directory check is as late as possible.
#[allow(clippy::too_many_arguments)]
pub(super) fn probe_profile(
    store: &ProfileStore,
    binary: &BinaryInfo,
    name: &ProfileName,
    live: bool,
    mode: ProbeMode,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
) -> Probe {
    let build = if live {
        run::build_default_dir_command
    } else {
        run::build_profile_command
    };
    let mode = if super::overflow::overflow_names(store.root()).contains(name) {
        ProbeMode::Text
    } else {
        mode
    };
    match probe_with(
        |a| build(binary, store, name, a),
        mode,
        timeout,
        now,
        keep_raw,
    ) {
        Ok(p) => p,
        // Reported as its fixed reason (and a stderr warning); the detail
        // can name paths.
        Err(_) => Probe::failed(ErrorReason::ProfileRefused, None),
    }
}

/// Probes the login in `~/.claude` with `CLAUDE_CONFIG_DIR` removed: the
/// `~/.claude` row of a login no profile holds, and the rotator's live login
/// when no profile owns it.
pub(super) fn probe_home(
    binary: &BinaryInfo,
    mode: ProbeMode,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
) -> Probe {
    probe_with(
        |a| Ok(run::build_home_command(binary, a)),
        mode,
        timeout,
        now,
        keep_raw,
    )
    // The home command is never refused; mapped as any refusal is.
    .unwrap_or_else(|_| Probe::failed(ErrorReason::ProfileRefused, None))
}

/// One login probed by `mode` within one deadline (issue #1698). `build`
/// makes the command for the given arguments and checks the profile
/// directory each time it is called; a refusal is returned for the caller
/// to report.
///
/// In `Auto` the `get_usage` request runs first. Its answer is used when
/// [`usage_request::parse_answer`] accepts it, however the attempt ended.
/// Otherwise, if the attempt ended before the deadline and nothing cancelled
/// it, `claude -p /usage` runs with a new command in the time left; else the
/// probe is `timeout`. At most one `claude` runs at a time, and `--raw`
/// shows the attempt that gave the row.
pub(super) fn probe_with(
    build: impl Fn(&[String]) -> Result<Command, ProfileError>,
    mode: ProbeMode,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
) -> Result<Probe, ProfileError> {
    let text_args = ["-p".to_string(), "/usage".to_string()];
    if mode == ProbeMode::Text {
        return Ok(probe(build(&text_args)?, timeout, now, keep_raw));
    }
    let cmd = build(&usage_request::ARGS.map(String::from))?;
    let deadline = Instant::now() + timeout;
    let line = format!("{}\n", usage_request::REQUEST_LINE);
    let a = attempt(cmd, Some(line.as_bytes()), deadline);
    let row = |report, output: Option<Raw>| Probe {
        report,
        error: None,
        raw: output.filter(|_| keep_raw),
    };
    let timed_out = UsageReport::empty(UsageState::Timeout);
    // A cancel is a daemon stopping: its rows are discarded anyway.
    if matches!(a.ended, Ended::Cancelled) {
        return Ok(row(timed_out, a.output));
    }
    let answer = a
        .output
        .as_ref()
        .and_then(|o| usage_request::parse_answer(&o.stdout, now));
    if let Some(report) = answer {
        return Ok(row(report, a.output));
    }
    if matches!(a.ended, Ended::Deadline)
        || PROBE_CANCEL.load(Ordering::Relaxed)
        || Instant::now() >= deadline
    {
        return Ok(row(timed_out, a.output));
    }
    let cmd = build(&text_args)?;
    Ok(text_probe(attempt(cmd, None, deadline), now, keep_raw))
}

/// `job(0)` to `job(n - 1)`, at most `parallel` at once (at least one).
/// Jobs start in index order, each on the first worker free, so no worker
/// sits idle while a job waits. Results are in index order; a panic in a
/// job propagates.
pub(super) fn pooled<T: Send>(
    n: usize,
    parallel: usize,
    job: impl Fn(usize) -> T + Sync,
) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..n.min(parallel.max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let r = job(i);
                    results.lock().unwrap_or_else(PoisonError::into_inner)[i] = Some(r);
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
        .map(|r| r.expect("every job runs"))
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

/// One `claude -p /usage` run of `cmd` with stdin from `/dev/null`, killed
/// at the deadline. See [`attempt`].
pub(super) fn probe(cmd: Command, timeout: Duration, now: i64, keep_raw: bool) -> Probe {
    text_probe(attempt(cmd, None, Instant::now() + timeout), now, keep_raw)
}

/// The row a `/usage` attempt gives: parsed after a clean exit, `timeout` at
/// the deadline or on a cancel, and its fixed reason otherwise. Output that
/// may be cut short is never parsed.
fn text_probe(a: Attempt, now: i64, keep_raw: bool) -> Probe {
    let report = match a.ended {
        Ended::Exited => {
            let o = a.output.as_ref();
            parse_usage(
                o.map_or("", |o| &o.stdout),
                o.map_or("", |o| &o.stderr),
                now,
            )
        }
        Ended::Deadline | Ended::Cancelled => UsageReport::empty(UsageState::Timeout),
        Ended::Failed(reason) => return Probe::failed(reason, a.output.filter(|_| keep_raw)),
    };
    Probe {
        report,
        error: None,
        raw: a.output.filter(|_| keep_raw),
    }
}

/// How one run of `claude` ended.
#[derive(Debug, Clone, Copy)]
enum Ended {
    /// It exited and both pipes closed within [`READ_GRACE`].
    Exited,
    Deadline,
    /// [`PROBE_CANCEL`] was set.
    Cancelled,
    Failed(ErrorReason),
}

/// One run: how it ended, and up to 64 KiB of each stream; `None` when no
/// stream was read (a start failure, or pipes that could not be taken).
struct Attempt {
    ended: Ended,
    output: Option<Raw>,
}

/// Runs `cmd` until it exits or `deadline` passes, reading up to 64 KiB of
/// each output stream. With `input`, stdin is a pipe that gets those bytes
/// in one write and is then closed; without, stdin is `/dev/null`, so the
/// child never reads the terminal. The exit status is not used.
///
/// On Unix the child leads its own process group, and the whole group is
/// killed before the child is reaped on every path, so processes `claude`
/// started (MCP servers, for example) do not outlive it.
fn attempt(mut cmd: Command, input: Option<&[u8]>, deadline: Instant) -> Attempt {
    let stdin = if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    cmd.stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let failed = |reason, output| Attempt {
        ended: Ended::Failed(reason),
        output,
    };
    let Ok(mut child) = cmd.spawn() else {
        return failed(ErrorReason::StartFailed, None);
    };
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        stop(&mut child);
        return failed(ErrorReason::ReadFailed, None);
    };
    let (out, err) = (Stream::spawn(out), Stream::spawn(err));
    let output = |out: &Stream, err: &Stream| {
        Some(Raw {
            stdout: out.text(),
            stderr: err.text(),
        })
    };
    if let Some(bytes) = input {
        // One write of less than PIPE_BUF into an empty pipe cannot block.
        // A child that closed stdin gives EPIPE, not a SIGPIPE that would
        // kill this process, only because the Rust runtime ignores SIGPIPE.
        // The handle is dropped here, before the poll loop, so `claude`
        // sees the end of its input. The reason is never shown: in `auto`
        // a failed request only means the `/usage` fallback.
        let written = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(bytes).is_ok());
        if !written {
            stop(&mut child);
            return failed(ErrorReason::StartFailed, output(&out, &err));
        }
    }

    loop {
        let ended = match exited(&mut child) {
            Ok(true) => {
                // The child is not reaped yet, so its pid (the group id)
                // cannot have been reused: anything it left in its group
                // goes too, then it is reaped.
                kill_group(&child);
                let _ = child.wait();
                break;
            }
            Ok(false) if PROBE_CANCEL.load(Ordering::Relaxed) => Ended::Cancelled,
            Ok(false) if Instant::now() >= deadline => Ended::Deadline,
            Ok(false) => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(_) => Ended::Failed(ErrorReason::WaitFailed),
        };
        stop(&mut child);
        return Attempt {
            ended,
            output: output(&out, &err),
        };
    }
    let exited = Instant::now();
    while !(out.done() && err.done()) && exited.elapsed() < READ_GRACE {
        std::thread::sleep(Duration::from_millis(10));
    }
    // A pipe still open means the output may be cut short.
    let ended = if !(out.done() && err.done()) || out.failed() || err.failed() {
        Ended::Failed(ErrorReason::ReadFailed)
    } else {
        Ended::Exited
    };
    Attempt {
        ended,
        output: output(&out, &err),
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

/// Sends SIGKILL to the process group the child leads (see [`attempt`]). The
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
        let (ga, gb) = (row_key(a, now), row_key(b, now));
        ga.0.cmp(&gb.0)
            .then(ga.1.total_cmp(&gb.1))
            .then_with(|| a.profile.cmp(&b.profile))
    });
}

/// [`sort_key`], with API-key rows (issue #1700) after every usable and
/// exhausted row and before the unavailable ones.
fn row_key(r: &Row, now: i64) -> (u8, f64) {
    if r.api_key {
        return (2, 0.0);
    }
    match sort_key(&r.report, now) {
        (g, m) if g >= 2 => (g + 1, m),
        k => k,
    }
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
    if let Some((reason, until)) = &row.api_key_blocked {
        parts.push(format!("blocked until {} ({reason})", utc_time(*until)));
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
pub(super) fn format_pct(p: f64) -> String {
    if p.fract() == 0.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

/// Replaces control characters (C0, DEL, C1) and bidirectional overrides so
/// untrusted text cannot drive the terminal. `\n` survives only when
/// `keep_newlines`.
pub(super) fn sanitize_display(s: &str, keep_newlines: bool) -> String {
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
                state_label(r).to_string(),
                if r.api_key {
                    API_KEY_CELL.to_string()
                } else if no_session_limit(r) {
                    NO_SESSION_LIMIT_CELL.to_string()
                } else {
                    pct(&r.report.session)
                },
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
    if rows.iter().any(|r| r.api_key && r.live_in) {
        out.push_str("live: work is running on the API key (billed per token)\n");
    }
    if let Some((reason, until)) = rows.iter().find_map(|r| r.api_key_blocked.as_ref()) {
        out.push_str(&format!(
            "blocked: the API key until {} ({reason})\n",
            utc_time(*until)
        ));
    }
    out
}

/// The `SESSION` cell of an API-key row.
const API_KEY_CELL: &str = "api-key (billed per token)";
/// The `SESSION` cell of a plan reading with no session window (#1818).
const NO_SESSION_LIMIT_CELL: &str = "no session limit";

/// A usable reading that showed no session window (#1818).
fn no_session_limit(r: &Row) -> bool {
    !r.report.session_limit
        && matches!(r.report.state, UsageState::Ok | UsageState::WeeklyExhausted)
}

/// The `STATE` of a row: `api_key` for an API-key profile.
fn state_label(r: &Row) -> &'static str {
    if r.api_key {
        "api_key"
    } else {
        r.report.state.as_str()
    }
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
struct ApiKeyBlock<'a> {
    reason: &'a str,
    until: i64,
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
    kind: &'static str,
    api_key_blocked: Option<ApiKeyBlock<'a>>,
    state: &'static str,
    session: Option<&'a UsageWindow>,
    session_limit: bool,
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
            kind: if r.api_key { "api-key" } else { "subscription" },
            api_key_blocked: r
                .api_key_blocked
                .as_ref()
                .map(|(reason, until)| ApiKeyBlock {
                    reason,
                    until: *until,
                }),
            state: state_label(r),
            session: r.report.session.as_ref(),
            session_limit: r.report.session_limit,
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
        .filter(|r| !r.api_key)
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
