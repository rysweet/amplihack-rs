//! `amplihack profile usage` (issue #1578): asks `claude` for its usage under
//! each profile in parallel (the `get_usage` request, else `claude -p
//! /usage`; issue #1698), prints the session and weekly usage in the order
//! the rotator ranks headroom (issue #1813; the bar view is in
//! `usage_view`), and records what it read as auto-switch readings.
//!
//! Issue #1844: each row is read in order, a fresh passive reading, then
//! the OAuth usage endpoint, then `claude` (see `usage_source`), and says
//! which gave it. `claude` is looked up only when a row needs it.
//!
//! Claude's text is kept only with `--raw`; error rows carry a fixed
//! reason. The parent environment is never changed: each child gets
//! `CLAUDE_CONFIG_DIR` on its own `Command`, or, for the profile whose login
//! is live in `~/.claude` (see `default_live`), has it removed.
//!
//! Issue #2001: with no names given, every Codex account is also read live
//! (`amplihack_utils::codex_capacity`) on a thread started before the Claude
//! probes, and printed after the Claude output (`usage_codex`). Codex rows
//! never reach the recording, the history, the summary or the exit status.
//! `--json` stays the bare array of Claude rows it has always been, and does
//! not read Codex at all: scripts on the owner's host index it with `.[]` and
//! `.[0]`. The Codex rows are in `--summary-json`, which was already an
//! object, under `codex`.

use super::auto_switch::{self, Observation, ObservedWindow, RecordOutcome};
use super::capacity_summary::{self, Summary};
use super::capacity_weight;
use super::name::ProfileName;
use super::probe_limits::ProbeMode;
use super::probe_sandbox::{self, NO_PERSIST, Notice, ProbeSandbox};
use super::rotate::decide::Reading;
use super::store::ProfileStore;
use super::usage_codex::{self, CodexJsonRow, CodexView};
use super::usage_parse::{UsageReport, UsageState, UsageWindow, parse_usage};
use super::usage_request::ExtraUsageShape;
use super::usage_source::{self, ClaudeGate, LazyBinary, Plan, Source, SourceEnv, Spend};
use super::usage_view::{self, TimeView};
use super::{ProfileError, default_live, pointer, probe_limits, run, usage_history, usage_request};
use crate::binary_finder::BinaryInfo;
use amplihack_utils::codex_capacity;
use anyhow::Result;
use serde::Serialize;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
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
pub(super) const ENV_CREDENTIAL_VARS: [&str; 3] = [
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
    /// Issue #1844: a direct read failed with this fixed reason, and the
    /// mode starts no `claude`.
    Direct(&'static str),
    /// Issue #1844: a row needed `claude` and none was found.
    NoClaude,
    /// Issue #2218: the profile is in the auto-switch `exclude` list, so it
    /// is never read: no credential read, no request and no refresh. Not an
    /// error: [`error_warnings`] and `--json`'s `error` leave it out.
    Excluded,
}

impl ErrorReason {
    pub(super) fn as_str(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::ProfileRefused => "profile directory refused".into(),
            Self::StartFailed => "could not start claude".into(),
            Self::ReadFailed => "could not read claude output".into(),
            Self::WaitFailed => "could not wait for claude".into(),
            Self::NoClaude => "claude not found".into(),
            Self::Direct(why) => format!("direct read: {why}").into(),
            Self::Excluded => EXCLUDED_CELL.into(),
        }
    }
}

impl std::fmt::Display for ErrorReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
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
    /// Issue #1844: the step that gave the row; `None` when it was never
    /// read through `usage_source`.
    pub(super) source: Option<Source>,
    /// Issue #1844: the direct read's reason when it led to a fallback.
    pub(super) direct_failure: Option<&'static str>,
    /// Issue #1856: what is known of the usage-credit spend limit.
    pub(super) spend: Spend,
    /// Issue #1856: the redacted `extra_usage` shape of a credits-on direct
    /// row, for the `--raw` note only. Never serialised.
    pub(super) extra_shape: Option<ExtraUsageShape>,
}

impl Probe {
    pub(super) fn failed(reason: ErrorReason, raw: Option<Raw>) -> Self {
        Self {
            report: UsageReport::empty(UsageState::Error),
            error: Some(reason),
            raw,
            source: None,
            direct_failure: None,
            spend: Spend::Unknown,
            extra_shape: None,
        }
    }

    /// A row read without `claude`, from a source that has no spend limit
    /// to report.
    pub(super) fn read(report: UsageReport, source: Source) -> Self {
        Self {
            report,
            error: None,
            raw: None,
            source: Some(source),
            direct_failure: None,
            spend: Spend::NotApplicable,
            extra_shape: None,
        }
    }

    /// A successful direct read (issue #1856). With credits on, the spend
    /// is [`Spend::NotProbed`] until a spend probe says otherwise.
    #[cfg(unix)]
    pub(super) fn direct(d: super::usage_direct::Direct) -> Self {
        let mut p = Self::read(d.report, Source::Direct);
        if d.credits_on.is_some() {
            p.spend = Spend::NotProbed;
        }
        p.extra_shape = d.credits_on;
        p
    }

    /// Issue #2218: a profile in the auto-switch `exclude` list, which is
    /// never read. No windows, no source and no spend.
    pub(super) fn excluded() -> Self {
        let mut p = Self::failed(ErrorReason::Excluded, None);
        p.spend = Spend::NotApplicable;
        p
    }

    /// A direct read that failed in a mode that starts no `claude`.
    pub(super) fn direct_error(why: &'static str) -> Self {
        let mut p = Self::failed(ErrorReason::Direct(why), None);
        p.source = Some(Source::Direct);
        p.direct_failure = Some(why);
        p
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
    /// An API-key profile (issue #1700): not probed, billed per token.
    pub(super) api_key: bool,
    /// The API-key block on an API-key row: its reason and end.
    pub(super) api_key_blocked: Option<(String, i64)>,
    pub(super) report: UsageReport,
    pub(super) error: Option<ErrorReason>,
    pub(super) raw: Option<Raw>,
    /// Issue #1844: the step that gave the row; `None` for a row never read.
    pub(super) source: Option<Source>,
    /// Issue #1844: the direct read's reason when the row fell back.
    pub(super) direct_failure: Option<&'static str>,
    /// Issue #1856: what is known of the usage-credit spend limit. Shown
    /// only; it plays no part in ordering or recording.
    pub(super) spend: Spend,
    /// Issue #1856: the redacted `extra_usage` shape of a credits-on direct
    /// row, for the `--raw` note only.
    pub(super) extra_shape: Option<ExtraUsageShape>,
}

impl Row {
    /// Issue #2218: the profile is in the auto-switch `exclude` list and was
    /// not read.
    pub(super) fn is_excluded(&self) -> bool {
        self.error == Some(ErrorReason::Excluded)
    }

    /// A subscription login that was (or could have been) read: neither an
    /// API-key profile nor an excluded one. Only these are recorded, kept
    /// in the history and counted in the capacity summary.
    pub(super) fn is_login(&self) -> bool {
        !self.api_key && !self.is_excluded()
    }
}

/// How `profile usage` prints its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Output {
    /// The bar view (issue #1813), then the capacity summary (issue #1797).
    Bars,
    /// The `--table` text table, then the capacity summary.
    Table,
    /// The `--json` array of Claude rows. It never holds Codex rows (#2001).
    Json,
    /// One object: the Claude rows, the capacity summary and the Codex rows.
    SummaryJson,
}

/// The options of `profile usage`.
#[derive(Debug, Clone)]
pub(super) struct UsageOpts {
    pub(super) output: Output,
    /// The zone the bar view shows reset times in.
    pub(super) time: TimeView,
    pub(super) raw: bool,
    pub(super) timeout_secs: Option<u64>,
    pub(super) usable_session_pct: Option<u8>,
    /// Issue #1856: run a spend probe for each credits-on direct row.
    pub(super) spend: bool,
}

pub(super) fn run(store: &ProfileStore, names: &[String], opts: UsageOpts) -> Result<()> {
    let UsageOpts {
        output,
        time,
        raw,
        timeout_secs: timeout_flag,
        usable_session_pct,
        spend,
    } = opts;
    let json = matches!(output, Output::Json | Output::SummaryJson);
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
    // Issue #2218: an excluded profile is never read. Named, it is refused
    // on stderr, still listed as not probed, and the others are reported;
    // with no names it is listed as not probed. An unknown name has already
    // failed above.
    let excluded = exclude_list(store.root(), |w| eprintln!("{w}"))?;
    let refused: Vec<ProfileName> = if names.is_empty() {
        Vec::new()
    } else {
        profiles
            .iter()
            .filter(|p| excluded.contains(p))
            .cloned()
            .collect()
    };
    for p in &refused {
        eprintln!("{}", excluded_refusal(p));
    }
    // Issue #2001: names select Claude profiles only; with none, every
    // Codex account is read alongside the Claude probes. `--json` keeps its
    // array of Claude rows, so it has no place for Codex rows and skips the
    // read.
    let codex = (names.is_empty() && output != Output::Json).then(CodexRead::start);
    if profiles.is_empty() {
        let codex = codex.map(CodexRead::finish);
        print_empty(output, &time, codex.as_ref());
        return refusal_result(&refused);
    }
    let home = home_dir();
    let live = detect_live(store, home.as_deref(), &profiles);
    // Issue #2218: when `~/.claude` holds an excluded profile's login, by
    // the rotator's local proof, no other profile is read against it.
    let excluded_live = excluded_live(store, home.as_deref())?;
    let live = live.filter(|l| excluded_live.as_ref().is_none_or(|b| b == &l.name));
    // Issue #1700: API-key profiles are never probed; a probe costs tokens
    // or times out, and there are no plan limits to read. Issue #2218: an
    // excluded API-key profile shows as excluded, so it is not loaded here.
    let keyable: Vec<ProfileName> = profiles
        .iter()
        .filter(|p| !excluded.contains(p))
        .cloned()
        .collect();
    let keys = ApiKeys::load(store, home.as_deref(), &keyable);
    let (keyed, profiles): (Vec<ProfileName>, Vec<ProfileName>) =
        profiles.into_iter().partition(|p| keys.names.contains(p));
    let live_name = live.as_ref().map(|l| l.name.clone());
    // Read before probing: the account each row is recorded under must be
    // the one live when its probe started, so that a swap during the probe
    // shows up in the check under the lock as a moved account.
    let marks = read_marks(store);

    let (claude_bound, warning) = probe_limits::parallel_from_host(profiles.len());
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let (direct_bound, warning) = probe_limits::direct_parallel_from_env(profiles.len());
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let now = now_secs();
    let env = SourceEnv::from_env(mode, Duration::from_secs(secs));
    // Issue #1844: found only when a row falls back to `claude`.
    let reader = RowReader {
        store: store.clone(),
        env,
        binary: LazyBinary::claude(),
        gate: ClaudeGate::new(claude_bound),
        home: home.clone(),
        no_refresh: no_refresh_dirs(store),
        overflow: super::overflow::overflow_names(store.root()),
        excluded: Some(excluded),
        now,
        keep_raw: raw,
    };
    let pool = match mode {
        ProbeMode::Auto | ProbeMode::Text => claude_bound,
        ProbeMode::Ordered | ProbeMode::Direct => direct_bound,
    };
    let mut probes = pooled(profiles.len(), pool, |i| {
        let name = &profiles[i];
        reader.profile(name, live_name.as_ref() == Some(name))
    });
    // The login in `~/.claude` may have been swapped during the probe; its
    // numbers are shown but not recorded against the profile. Checked
    // before any spend probe, so that a long `--spend` pass cannot widen
    // the window in which a swap discards windows that were read correctly.
    let swapped = swapped_live(store, home.as_deref(), live.clone());
    // Taken once the window readings are in and before any spend probe:
    // the summary, `--summary-json`'s `at`, the ordering and the history
    // line are all as of when the windows were read, however long
    // `--spend` then takes. A passive reading's age is still judged at
    // `now`, the time it was accepted at.
    let at = now_secs();
    // Issue #1856: the spend of credits-on direct rows, read only when
    // `--spend` asks for it, only in the default order and only once every
    // row has its windows. `/usage` can take tens of seconds per profile on
    // a loaded host, so the default run starts no `claude` for these rows.
    if spend && mode == ProbeMode::Ordered {
        // The live row's spend probe reads `~/.claude`, so its result is
        // kept only while that login is still the one the windows were
        // read from: not swapped before the pass, nor during its probe.
        let still_live = || {
            swapped.is_none()
                && live
                    .as_ref()
                    .is_some_and(|l| default_live::still_live(store, home.as_deref(), l))
        };
        spend_pass(
            &reader,
            &profiles,
            live_name.as_ref(),
            &still_live,
            &mut probes,
            claude_bound,
        );
    }
    // Issue #2218: an excluded live login proven from local files only is
    // marked live, though nothing was read from it.
    let live_mark = live_name.as_ref().or(excluded_live.as_ref());
    let mut rows = build_rows(&marks, &profiles, probes, live_mark);
    rows.extend(keys.rows(&marks, &keyed));
    order(&mut rows, at);
    // Issue #1797: the capacity summary, from these rows only; no probe.
    // `--raw` is diagnostic output and gets neither summary nor history.
    // `None` when no row is a subscription login: no summary, no history.
    let current = usage_history::from_rows(now, at, &rows, swapped.as_ref());
    let summary = current
        .as_ref()
        .filter(|_| !raw && output != Output::Json)
        .map(|c| capacity_of(store, c, usable_session_pct, at));
    let codex = codex.map(CodexRead::finish);
    print_output(
        &rows,
        output,
        &time,
        summary.as_ref(),
        at,
        swapped.as_ref(),
        codex.as_ref(),
    );
    // SR-10: in every mode, with or without `--raw`.
    for warning in not_saved_warnings(&rows) {
        eprintln!("{warning}");
    }
    // Issue #1844: why a row fell back, for the detailed mode only.
    if raw {
        for note in source_notes(&rows) {
            eprintln!("{note}");
        }
    }
    #[cfg(unix)]
    {
        let st = super::auto_switch::kit::State::new(store.root()).ok();
        let list = super::duplicates::accounts(store, home.as_deref());
        let mut ws = super::duplicates::warnings(st.as_ref(), &list);
        ws.extend(super::duplicates::record_warnings(st.as_ref(), &list));
        for w in ws {
            eprintln!("warning: {w}");
        }
    }
    record(store, &rows, swapped.as_ref(), json);
    if let Some(c) = current.as_ref().filter(|_| !raw && names.is_empty()) {
        record_history(store, c, at);
    }
    // Issue #2218: once every other row is reported and recorded, a named
    // excluded profile makes the run exit non-zero (the refusals are
    // already on stderr).
    refusal_result(&refused)
}

/// The warning printed when `<root>/.auto-switch` exists but cannot be
/// read or parsed (issue #2218): which logins are excluded is unknown, so
/// every profile is treated as excluded and none is probed or refreshed.
pub(super) const EXCLUDE_UNREADABLE: &str = "warning: auto-switch config unreadable; \
     every profile treated as excluded, none probed or refreshed";

/// The warning printed when `<root>/.auto-switch` parses but is unsafe or
/// invalid for its other settings: its exclude list still applies.
pub(super) const EXCLUDE_UNTRUSTED: &str = "warning: auto-switch config unsafe or invalid; \
     exclude list applied, other settings ignored";

/// Issue #2218: the auto-switch `exclude` list under `root`, or `None` when
/// the file exists but cannot be read or parsed. `warn` gets
/// [`EXCLUDE_UNREADABLE`] or [`EXCLUDE_UNTRUSTED`] at most once.
#[cfg(unix)]
pub(super) fn read_exclude(
    root: &std::path::Path,
    warn: impl FnOnce(&str),
) -> Option<Vec<ProfileName>> {
    use super::auto_switch::config::{ExcludeFile, read_exclude};
    match warned(read_exclude(root), warn) {
        ExcludeFile::Read { names, .. } => Some(names),
        ExcludeFile::Unreadable => None,
    }
}

/// Passes `file` through after giving `warn` the line it calls for, if any.
#[cfg(unix)]
fn warned(
    file: super::auto_switch::config::ExcludeFile,
    warn: impl FnOnce(&str),
) -> super::auto_switch::config::ExcludeFile {
    use super::auto_switch::config::ExcludeFile;
    match &file {
        ExcludeFile::Read { trusted: false, .. } => warn(EXCLUDE_UNTRUSTED),
        ExcludeFile::Unreadable => warn(EXCLUDE_UNREADABLE),
        ExcludeFile::Read { .. } => {}
    }
    file
}

/// Issue #2218: the profiles to treat as excluded under `root`, read once
/// per command or rotate cycle: the `exclude` list, or every profile in
/// the store when the file cannot be read (fails closed: a refresh of an
/// excluded login signs out the other hosts that share it). The rule is
/// `config::excluded_from`, the body of `config::excluded_profiles`, which
/// `config::effective` follows too; this only adds the warning. A store
/// that cannot be listed is an error, never an empty list.
#[cfg(unix)]
pub(super) fn exclude_list(
    root: &std::path::Path,
    warn: impl FnOnce(&str),
) -> Result<Vec<ProfileName>, ProfileError> {
    use super::auto_switch::config::{excluded_from, read_exclude};
    // One read: warn about it, then apply the shared rule to it.
    excluded_from(warned(read_exclude(root), warn), root)
}

/// Auto-switch has no config on this platform, so nothing is excluded.
#[cfg(not(unix))]
pub(super) fn exclude_list(
    _: &std::path::Path,
    _: impl FnOnce(&str),
) -> Result<Vec<ProfileName>, ProfileError> {
    Ok(Vec::new())
}

/// Issue #2218: the excluded profile a rule of the local proof names as
/// the login in `<home>/.claude`; read from local files only.
#[cfg(unix)]
fn excluded_live(
    store: &ProfileStore,
    home: Option<&std::path::Path>,
) -> Result<Option<ProfileName>, ProfileError> {
    let rs = super::rotate::ledger::RotateState::new(store.root());
    let Some(home) = home else { return Ok(None) };
    Ok(super::rotate::live_excluded_offline(store, &rs, home)?.map(|(b, _)| b))
}

#[cfg(not(unix))]
fn excluded_live(
    _: &ProfileStore,
    _: Option<&std::path::Path>,
) -> Result<Option<ProfileName>, ProfileError> {
    Ok(None)
}

/// The stderr line for a named profile that is excluded (issue #2218).
pub(super) fn excluded_refusal(name: &ProfileName) -> String {
    format!(
        "{}: excluded (in auto-switch exclude list); not probed \
         (amplihack profile auto-switch exclude)",
        sanitize_display(name.as_str(), false)
    )
}

/// The exit of a run that named an excluded profile: status 1 with no
/// further message, since [`excluded_refusal`] already printed a line for
/// each refused name.
fn refusal_result(refused: &[ProfileName]) -> Result<()> {
    if refused.is_empty() {
        return Ok(());
    }
    Err(crate::command_error::exit_error(1))
}

/// Which login a spend probe reads: a profile, against `~/.claude` when
/// the flag is set.
#[derive(Debug, Clone, Copy)]
struct SpendTarget<'a>(&'a ProfileName, bool);

/// Issue #1856: with `--spend`, one spend-only `/usage` probe per
/// credits-on direct row, at most `bound` at once, each under the normal
/// per-probe timeout. Only each row's
/// `spend` changes: its windows, state, error and source stay as the
/// direct read gave them.
///
/// The live profile's probe reads `~/.claude`. It is started only while
/// `still_live` holds, and its result is dropped for `Unknown` unless it
/// holds again once it has finished, so a swap before or during the pass
/// never shows another login's spend on that row.
fn spend_pass(
    reader: &RowReader,
    profiles: &[ProfileName],
    live: Option<&ProfileName>,
    still_live: &(dyn Fn() -> bool + Sync),
    probes: &mut [Probe],
    bound: usize,
) {
    let needs = |p: &Probe| p.source == Some(Source::Direct) && p.extra_shape.is_some();
    let slots: Vec<usize> = (0..probes.len()).filter(|&i| needs(&probes[i])).collect();
    if slots.is_empty() {
        return;
    }
    let spends = pooled(slots.len(), bound, |j| {
        let i = slots[j];
        if live == Some(&profiles[i]) {
            guarded_spend(reader, SpendTarget(&profiles[i], true), still_live, || {
                spend_swap_note(&profiles[i])
            })
        } else {
            reader.spend(SpendTarget(&profiles[i], false))
        }
    });
    for (i, spend) in slots.into_iter().zip(spends) {
        probes[i].spend = spend;
    }
}

/// A spend read in `~/.claude`, kept only while `still()` holds before and
/// after the probe; `Unknown`, with one `note()` on stderr, otherwise.
fn guarded_spend(
    reader: &RowReader,
    target: SpendTarget<'_>,
    still: &(dyn Fn() -> bool + Sync),
    note: impl FnOnce() -> String,
) -> Spend {
    if still() {
        let spend = reader.spend(target);
        if still() {
            return spend;
        }
    }
    eprintln!("{}", note());
    Spend::Unknown
}

/// The capacity summary of `current`, printing the threshold, weights and
/// history warnings.
fn capacity_of(
    store: &ProfileStore,
    current: &usage_history::Snapshot,
    flag: Option<u8>,
    now: i64,
) -> Summary {
    let env = std::env::var_os(capacity_weight::ENV);
    let (threshold, warning) = capacity_weight::usable_threshold(flag, env.as_deref());
    let (weights, warnings) = capacity_weight::load(store.root());
    let (history, refused) =
        usage_history::read_recent(store.root(), now, capacity_summary::RATE_WINDOW);
    for w in warning.iter().chain(&warnings).chain(&refused) {
        eprintln!("{w}");
    }
    capacity_summary::compute(&capacity_summary::Inputs {
        current,
        weights: &weights,
        threshold,
        history: &history,
        now,
    })
}

/// Appends this run to the usage history (issue #1797). `append` applies
/// the recording rules: a trusted root, a private `.auto-switch.d` and no
/// credential variable. Called only for unfiltered runs without `--raw`
/// that have a subscription login. A failure is one stderr line and never
/// changes the exit status.
fn record_history(store: &ProfileStore, current: &usage_history::Snapshot, now: i64) {
    if let Err(e) = usage_history::append(
        store.root(),
        &current.entry(usage_history::Writer::Usage),
        now,
    ) && let Some(w) = e.warning()
    {
        eprintln!("{w}");
    }
}

/// `$HOME` as an absolute path; `None` when it is unset, empty or relative.
pub(super) fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|h| h.is_absolute())
}

/// The Codex read of one `profile usage` run (issue #2001), running on its
/// own thread from before the Claude probes until [`CodexRead::finish`].
struct CodexRead {
    root: std::path::PathBuf,
    timeout_secs: u64,
    handle: std::thread::JoinHandle<std::io::Result<Vec<codex_capacity::AccountResult>>>,
}

impl CodexRead {
    /// Prints the deadline warning, if any, and starts reading every Codex
    /// account under the profiles root, and `~/.codex`.
    fn start() -> Self {
        let (timeout_secs, warning) = codex_capacity::timeout_from_env();
        if let Some(w) = warning {
            eprintln!("{w}");
        }
        let home = home_dir();
        let root = codex_capacity::profiles_root(home.as_deref());
        let shown = root
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.codex-profiles"));
        let bin = codex_capacity::codex_bin();
        let handle = std::thread::spawn(move || {
            let accounts = match root.as_deref() {
                Some(r) => codex_capacity::list_accounts(r, home.as_deref())?,
                None => Vec::new(),
            };
            Ok(codex_capacity::read_all(
                &bin,
                &accounts,
                Duration::from_secs(timeout_secs),
            ))
        });
        Self {
            root: shown,
            timeout_secs,
            handle,
        }
    }

    /// Waits for the read. A root that could not be listed, or a read
    /// thread that panicked, gives no rows and one warning.
    fn finish(self) -> CodexView {
        let rows = match self.handle.join() {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                eprintln!(
                    "warning: cannot list Codex accounts under {}: {}",
                    sanitize_display(&self.root.to_string_lossy(), false),
                    sanitize_display(&e.to_string(), false)
                );
                Vec::new()
            }
            Err(_) => {
                eprintln!("warning: the Codex read failed");
                Vec::new()
            }
        };
        usage_codex::view(self.root, rows, self.timeout_secs)
    }
}

/// The `codex` rows of the JSON outputs: none when Codex was not read.
fn codex_json(codex: Option<&CodexView>) -> Vec<CodexJsonRow> {
    codex.map(usage_codex::json_rows).unwrap_or_default()
}

fn print_empty(output: Output, time: &TimeView, codex: Option<&CodexView>) {
    let rows = codex_json(codex);
    match output {
        Output::Json => println!("{}", render_json(&[])),
        Output::SummaryJson => {
            println!("{}", render_summary_json(now_secs(), &[], None, &rows))
        }
        Output::Bars | Output::Table => {
            println!("no profiles; create one with `amplihack profile add <name>`");
            print_codex_text(output, codex, time);
        }
    }
}

/// The Codex block after the Claude text output, in the bar or table form.
fn print_codex_text(output: Output, codex: Option<&CodexView>, time: &TimeView) {
    let Some(v) = codex else {
        return;
    };
    // One empty line between the Claude output and the Codex block.
    println!();
    match output {
        Output::Bars => {
            use std::io::IsTerminal;
            let colour = usage_view::colour_enabled(
                std::io::stdout().is_terminal(),
                std::env::var_os("NO_COLOR").as_deref(),
            );
            print!("{}", usage_codex::bars_block(v, time, colour));
        }
        Output::Table => print!("{}", usage_codex::table_block(v, time)),
        Output::Json | Output::SummaryJson => {}
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
/// (so none of them could be live). Issue #1886: by the ledgers only; no
/// identity read and no account comparison.
fn detect_live(
    store: &ProfileStore,
    home: Option<&std::path::Path>,
    profiles: &[ProfileName],
) -> Option<default_live::Live> {
    let (live, warning) = default_live::detect(store, home);
    if let Some(w) = warning.filter(|w| live_warning_applies(store, profiles, w)) {
        eprintln!("{w}");
    }
    live.filter(|l| profiles.contains(&l.name))
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
                    report: UsageReport::empty(UsageState::Ok),
                    error: None,
                    raw: None,
                    source: None,
                    direct_failure: None,
                    spend: Spend::NotApplicable,
                    extra_shape: None,
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
                api_key: false,
                api_key_blocked: None,
                report: p.report,
                error: p.error,
                raw: p.raw,
                source: p.source,
                direct_failure: p.direct_failure,
                spend: p.spend,
                extra_shape: p.extra_shape,
            }
        })
        .collect()
}

/// The bar view or table (each with the capacity summary) or JSON on
/// stdout, then the warnings on stderr. Colour is decided once, for the
/// bar view only (no-color.org).
fn print_output(
    rows: &[Row],
    output: Output,
    time: &TimeView,
    summary: Option<&Summary>,
    at: i64,
    swapped: Option<&ProfileName>,
    codex: Option<&CodexView>,
) {
    let codex_rows = codex_json(codex);
    match output {
        Output::Bars => {
            use std::io::IsTerminal;
            let colour = usage_view::colour_enabled(
                std::io::stdout().is_terminal(),
                std::env::var_os("NO_COLOR").as_deref(),
            );
            print!("{}", usage_view::render_bars(rows, time, colour));
            if let Some(s) = summary {
                print!("{}", summary_block(s));
            }
            print_codex_text(output, codex, time);
        }
        Output::Table => {
            print!("{}", render_table(rows));
            if let Some(s) = summary {
                print!("{}", summary_block(s));
            }
            print_codex_text(output, codex, time);
        }
        Output::Json => println!("{}", render_json(rows)),
        Output::SummaryJson => {
            println!("{}", render_summary_json(at, rows, summary, &codex_rows))
        }
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
        "note: the login in ~/.claude changed while profile \"{}\" was probed; its usage was not recorded this run",
        sanitize_display(name.as_str(), false)
    )
}

/// Issue #1856: the note for a live profile whose `--spend` probe was not
/// started, or was dropped, because the login in `~/.claude` changed. Its
/// windows were read before the change and are recorded as usual.
pub(super) fn spend_swap_note(name: &ProfileName) -> String {
    format!(
        "note: the login in ~/.claude changed while profile \"{}\" was probed for its spend; its spend is unknown this run",
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
// Reading in order (issue #1844)
// ---------------------------------------------------------------------------

/// What every row of one `profile usage` run, or one rotate cycle, shares.
/// Owned, so a rotate cycle can hand it to reads that outlast it.
pub(super) struct RowReader {
    pub(super) store: ProfileStore,
    pub(super) env: SourceEnv,
    pub(super) binary: LazyBinary,
    pub(super) gate: ClaudeGate,
    /// `$HOME`, whose `.claude` holds the live login.
    pub(super) home: Option<std::path::PathBuf>,
    /// Directories whose token is never refreshed; `None` when the leases
    /// cannot be read, which refreshes none.
    pub(super) no_refresh: Option<Vec<String>>,
    pub(super) overflow: Vec<ProfileName>,
    /// Issue #2218: the auto-switch `exclude` list. These profiles are never
    /// read: no credential read, no request and no refresh. `None` when the
    /// list cannot be worked out, which counts every profile as excluded.
    pub(super) excluded: Option<Vec<ProfileName>>,
    pub(super) now: i64,
    pub(super) keep_raw: bool,
}

impl RowReader {
    /// Issue #2218: whether `name` is in the auto-switch `exclude` list.
    pub(super) fn is_excluded(&self, name: &ProfileName) -> bool {
        self.excluded.as_ref().is_none_or(|l| l.contains(name))
    }

    /// One profile, against `~/.claude` when `live`. An excluded profile is
    /// [`Probe::excluded`], decided before anything is read.
    pub(super) fn profile(&self, name: &ProfileName, live: bool) -> Probe {
        if self.is_excluded(name) {
            return Probe::excluded();
        }
        let claude = |m: ProbeMode| {
            usage_source::fallback_with(&self.binary, |bin| {
                self.gate.run(|| {
                    probe_profile(
                        &self.store,
                        bin,
                        name,
                        live,
                        m,
                        self.env.timeout,
                        self.now,
                        self.keep_raw,
                    )
                })
            })
        };
        let Some(plan) = self.plan(name, live) else {
            return Probe::failed(ErrorReason::ProfileRefused, None);
        };
        self.finish(usage_source::read(
            &self.env,
            &plan,
            self.store.root(),
            self.now,
            claude,
        ))
    }

    /// How `name` is read; `None` when its directory is refused.
    fn plan(&self, name: &ProfileName, live: bool) -> Option<Plan> {
        let creds_dir = if live {
            self.home.as_ref().map(|h| h.join(".claude"))
        } else if matches!(self.env.mode, ProbeMode::Auto | ProbeMode::Text) {
            // Never read: the named modes go straight to `claude`.
            Some(self.store.root().join(name.as_str()))
        } else {
            match self.store.verify_profile_dir(name) {
                Ok(dir) => Some(dir),
                Err(_) => return None,
            }
        };
        let refresh_ok = self
            .no_refresh
            .as_ref()
            .is_some_and(|dirs| !dirs.iter().any(|d| d == name.as_str()));
        Some(Plan {
            // With no home, the live login cannot be found: the direct read
            // finds no file and the row falls back.
            creds_dir: creds_dir
                .unwrap_or_else(|| self.store.root().join(name.as_str()).join(".no-home")),
            passive: Some(name.clone()),
            // SR-3(d): never the live login's token in a usage read. Only the
            // rotate prelude refreshes it, once, when it has expired (#1753).
            allow_refresh: refresh_ok && !live,
            overflow: self.overflow.contains(name),
        })
    }

    /// The `~/.claude` row of a login no profile holds: no passive step and
    /// no refresh.
    pub(super) fn home(&self) -> Probe {
        let claude = |m: ProbeMode| {
            usage_source::fallback_with(&self.binary, |bin| {
                self.gate
                    .run(|| probe_home(bin, m, self.env.timeout, self.now, self.keep_raw))
            })
        };
        let Some(home) = &self.home else {
            return self.finish(claude(match self.env.mode {
                ProbeMode::Text => ProbeMode::Text,
                _ => ProbeMode::Auto,
            }));
        };
        let plan = Plan {
            creds_dir: home.join(".claude"),
            passive: None,
            allow_refresh: false,
            overflow: false,
        };
        self.finish(usage_source::read(
            &self.env,
            &plan,
            self.store.root(),
            self.now,
            claude,
        ))
    }

    /// Issue #1856: the spend limit of a credits-on login, from one
    /// `/usage` run under the same per-probe timeout as a fallback. Its
    /// output is never kept and only the spend leaves here. A probe that
    /// was asked for but could not start (no `claude` found) is `Unknown`,
    /// like one that ran and gave no reading: `NotProbed` is kept for rows
    /// no probe was asked for.
    fn spend(&self, target: SpendTarget<'_>) -> Spend {
        if self.is_excluded(target.0) {
            return Spend::NotApplicable;
        }
        let Some(bin) = self.binary.get() else {
            return Spend::Unknown;
        };
        self.gate.run(|| {
            let timeout = self.env.timeout;
            let SpendTarget(name, live) = target;
            let p = probe_profile(
                &self.store,
                bin,
                name,
                live,
                ProbeMode::Text,
                timeout,
                self.now,
                false,
            );
            p.spend
        })
    }

    /// With `--raw`, a row read without `claude` gets an empty `raw`, which
    /// the table shows as one fixed line.
    fn finish(&self, mut p: Probe) -> Probe {
        if self.keep_raw
            && p.raw.is_none()
            && matches!(p.source, Some(Source::Passive | Source::Direct))
        {
            p.raw = Some(Raw {
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        p
    }
}

/// The directories whose token a direct read may not refresh (SR-3(d)):
/// any that hosts another account's login or has lent its own. `None`
/// when the leases cannot be read.
pub(super) fn no_refresh_dirs(store: &ProfileStore) -> Option<Vec<String>> {
    let view = auto_switch::lease_view(store.root()).ok()?;
    Some(view.map_or_else(Vec::new, |v| {
        let mut dirs: Vec<String> = Vec::new();
        for (dir, live) in &v.hosts {
            if dir != live {
                dirs.push(dir.clone());
                dirs.push(live.clone());
            }
        }
        dirs
    }))
}

// ---------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------

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

/// One login probed by `mode` within one deadline (issue #1698), with the
/// sandbox detected for this process (issue #1974); see
/// [`probe_with_sandbox`].
pub(super) fn probe_with(
    build: impl Fn(&[String]) -> Result<Command, ProfileError>,
    mode: ProbeMode,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
) -> Result<Probe, ProfileError> {
    let home = home_dir().unwrap_or_default();
    probe_with_sandbox(
        build,
        probe_sandbox::detected(),
        &home,
        &probe_sandbox::NOTICE,
        mode,
        timeout,
        now,
        keep_raw,
    )
}

/// One login probed by `mode` within one deadline (issue #1698). `build`
/// makes the command for the given arguments and checks the profile
/// directory each time it is called; a refusal is returned for the caller
/// to report.
///
/// Both argument sets end with `--no-session-persistence`, and every
/// command runs through [`probe_sandbox::wrap`] (issue #1974); a fallback
/// is noted once through `notice`.
///
/// In `Auto` the `get_usage` request runs first. Its answer is used when
/// [`usage_request::parse_answer`] accepts it, however the attempt ended.
/// Otherwise, if the attempt ended before the deadline and nothing cancelled
/// it, `claude -p /usage` runs with a new command in the time left; else the
/// probe is `timeout`. At most one `claude` runs at a time, and `--raw`
/// shows the attempt that gave the row.
#[allow(clippy::too_many_arguments)]
pub(super) fn probe_with_sandbox(
    build: impl Fn(&[String]) -> Result<Command, ProfileError>,
    sandbox: &ProbeSandbox,
    home: &Path,
    notice: &Notice,
    mode: ProbeMode,
    timeout: Duration,
    now: i64,
    keep_raw: bool,
) -> Result<Probe, ProfileError> {
    // `wrap` runs before `attempt` sets stdio and the process group, so
    // `bwrap` leads the group and the group kill still ends `claude`.
    let prepared = |args: &[String]| -> Result<Command, ProfileError> {
        let (cmd, fallback) = probe_sandbox::wrap(build(args)?, sandbox, home);
        if let Some(f) = fallback {
            notice.note(f);
        }
        Ok(cmd)
    };
    let text_args = ["-p", "/usage", NO_PERSIST].map(String::from);
    if mode == ProbeMode::Text {
        return Ok(probe(prepared(&text_args)?, timeout, now, keep_raw));
    }
    let request_args: Vec<String> = usage_request::ARGS
        .iter()
        .copied()
        .chain([NO_PERSIST])
        .map(String::from)
        .collect();
    let cmd = prepared(&request_args)?;
    let deadline = Instant::now() + timeout;
    let line = format!("{}\n", usage_request::REQUEST_LINE);
    let a = attempt(cmd, Some(line.as_bytes()), deadline);
    // Issue #1856: an accepted answer has credits off, so there is no
    // spend limit to read; a timed-out request tells nothing of it.
    let row = |report, output: Option<Raw>, spend| Probe {
        report,
        error: None,
        raw: output.filter(|_| keep_raw),
        source: None,
        direct_failure: None,
        spend,
        extra_shape: None,
    };
    let timed_out = UsageReport::empty(UsageState::Timeout);
    // A cancel is a daemon stopping: its rows are discarded anyway.
    if matches!(a.ended, Ended::Cancelled) {
        return Ok(row(timed_out, a.output, Spend::Unknown));
    }
    let answer = a
        .output
        .as_ref()
        .and_then(|o| usage_request::parse_answer(&o.stdout, now));
    if let Some(report) = answer {
        return Ok(row(report, a.output, Spend::NotApplicable));
    }
    if matches!(a.ended, Ended::Deadline)
        || PROBE_CANCEL.load(Ordering::Relaxed)
        || Instant::now() >= deadline
    {
        return Ok(row(timed_out, a.output, Spend::Unknown));
    }
    let cmd = prepared(&text_args)?;
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
    let mut p = Probe {
        report,
        error: None,
        raw: a.output.filter(|_| keep_raw),
        source: None,
        direct_failure: None,
        spend: Spend::Unknown,
        extra_shape: None,
    };
    p.spend = Spend::of_text(&p);
    p
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
    // Never pid 0 (our own group) or 1 (`kill(-1)` is every process).
    let Some(pid) = i32::try_from(child.id()).ok().filter(|p| *p > 1) else {
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

/// The order every view prints (issue #1813): usable rows by the
/// rotator's headroom, most first, then usable rows with no headroom;
/// exhausted rows by when they are back (unknown last); API-key rows;
/// unavailable rows by state. Every tie goes to the name, as in the
/// rotator's `best()`.
pub(super) fn order(rows: &mut [Row], now: i64) {
    rows.sort_by(|a, b| {
        row_key(a, now)
            .cmp(&row_key(b, now))
            .then_with(|| a.profile.cmp(&b.profile))
    });
}

/// The line that states the order, printed last by the bar view and the
/// table.
pub(super) const ORDER_LINE: &str =
    "order: rotator pick (most room in the worse of session and week, then name)";

/// [`sort_key`], with API-key rows (issue #1700) after every usable and
/// exhausted row and before the unavailable ones, and excluded rows (issue
/// #2218) after all of them.
fn row_key(r: &Row, now: i64) -> (u8, i64) {
    // Issue #2218: excluded rows last, after the unavailable ones.
    if r.is_excluded() {
        return (u8::MAX, 0);
    }
    if r.api_key {
        return (2, 0);
    }
    match sort_key(&r.report, now) {
        (g, m) if g >= 2 => (g + 1, m),
        k => k,
    }
}

/// Rank of a usable row with no headroom: after every row that has one.
const NO_HEADROOM: i64 = 101;

/// `(group, rank)`; a smaller rank sorts first within the group.
///
/// Usable rows rank by `100 - headroom`, where headroom is the rotator's
/// [`Reading::headroom`]: the floored points left in the worse of the
/// session and all-models week windows, a missing session counting as 0%.
/// One deliberate difference (D2): a window whose reset time has already
/// passed counts as 0% used here, because it has reset since it was read;
/// the rotator reads it as probed. The row still shows the probed value.
/// An `ok` row with no week window is a failed probe to the rotator, which
/// never picks it, so it ranks after every row that has a headroom (D3), as
/// does a NaN reading.
fn sort_key(r: &UsageReport, now: i64) -> (u8, i64) {
    match r.state {
        UsageState::Ok => {
            let Some(week) = &r.week_all else {
                return (0, NO_HEADROOM);
            };
            let used = |w: &UsageWindow| {
                if w.resets_at.is_none_or(|t| t > now) {
                    w.used_percentage
                } else {
                    0.0
                }
            };
            let reading = Reading::Measured {
                session: r.session.as_ref().map_or(0.0, used),
                week: used(week),
            };
            match reading.headroom() {
                Some(h) => (0, 100 - i64::from(h)),
                None => (0, NO_HEADROOM),
            }
        }
        UsageState::SessionExhausted | UsageState::WeeklyExhausted | UsageState::SpendLimit => {
            (1, back_at(r).unwrap_or(i64::MAX))
        }
        UsageState::NotLoggedIn => (2, 0),
        UsageState::Timeout => (2, 1),
        UsageState::Unparsed => (2, 2),
        UsageState::Error => (2, 3),
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
    let parts = marks_parts(row, true);
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(" ")
    }
}

/// The row's marks, each sanitised for display; empty when there are none.
/// `with_live: false` leaves out `live in ~/.claude`, which the bar view
/// shows as `(live)` beside the name instead.
pub(super) fn marks_parts(row: &Row, with_live: bool) -> Vec<String> {
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
    if row.live_in && with_live {
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
    if row.source == Some(Source::Fallback) {
        parts.push("(fallback)".to_string());
    }
    parts.iter().map(|p| sanitize_display(p, false)).collect()
}

/// UTC time as `YYYY-MM-DDTHH:MM:SSZ`, the rotate log's format.
pub(super) fn utc_time(t: i64) -> String {
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
    const HEADER: [&str; 8] = [
        "PROFILE",
        "MARKS",
        "STATE",
        "SESSION",
        "SESSION RESETS",
        "WEEK",
        "WEEK RESETS",
        "SPEND",
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
    let cells: Vec<[String; 8]> = rows
        .iter()
        .map(|r| {
            [
                sanitize_display(&r.profile, false),
                sanitize_display(&marks(r), false),
                state_label(r).to_string(),
                if r.is_excluded() {
                    EXCLUDED_CELL.to_string()
                } else if r.api_key {
                    API_KEY_CELL.to_string()
                } else if no_session_limit(r) {
                    NO_SESSION_LIMIT_CELL.to_string()
                } else {
                    pct(&r.report.session)
                },
                resets(&r.report.session),
                pct(&r.report.week_all),
                resets(&r.report.week_all),
                r.spend.as_str().to_string(),
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
        raw_lines(&mut out, row);
    }
    out.push_str(&footers(rows));
    out.push_str(ORDER_LINE);
    out.push('\n');
    out
}

/// The `--raw` text under a row, when it was kept.
pub(super) fn raw_lines(out: &mut String, row: &Row) {
    if let Some(raw) = &row.raw {
        match row.source {
            Some(s @ (Source::Passive | Source::Direct)) => {
                out.push_str(&format!("  ({}: no claude output)\n", s.as_str()));
            }
            _ => {
                raw_block(out, "stdout", &raw.stdout);
                raw_block(out, "stderr", &raw.stderr);
            }
        }
    }
}

/// The overflow, API-key `live:` and `blocked:` lines after the rows of
/// either human view.
pub(super) fn footers(rows: &[Row]) -> String {
    let mut out = String::new();
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
            "blocked: the API key until {} ({})\n",
            utc_time(*until),
            sanitize_display(reason, false)
        ));
    }
    out
}

/// The `SESSION` cell of an API-key row.
pub(super) const API_KEY_CELL: &str = "api-key (billed per token)";
/// The `SESSION` cell (and the bar view's span) of a profile in the
/// auto-switch `exclude` list (issue #2218).
pub(super) const EXCLUDED_CELL: &str = "excluded (not probed)";
/// The `SESSION` cell of a plan reading with no session window (#1818).
pub(super) const NO_SESSION_LIMIT_CELL: &str = "no session limit";

/// A usable reading that showed no session window (#1818).
pub(super) fn no_session_limit(r: &Row) -> bool {
    !r.report.session_limit
        && matches!(r.report.state, UsageState::Ok | UsageState::WeeklyExhausted)
}

/// The `STATE` of a row: `excluded` for a profile in the auto-switch
/// `exclude` list (issue #2218), `api_key` for an API-key profile.
pub(super) fn state_label(r: &Row) -> &'static str {
    if r.is_excluded() {
        "excluded"
    } else if r.api_key {
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
    /// `null` for an excluded row (issue #2218), whose account is not read.
    account: Option<&'a str>,
    default: bool,
    leased_to: Option<&'a str>,
    live_in: Option<&'static str>,
    overflow: bool,
    overflow_blocked_until: Option<i64>,
    kind: &'static str,
    api_key_blocked: Option<ApiKeyBlock<'a>>,
    state: &'static str,
    source: Option<&'static str>,
    /// Issue #1856: always present. The `extra_usage` shape never is.
    spend: &'static str,
    session: Option<&'a UsageWindow>,
    session_limit: bool,
    week_all: Option<&'a UsageWindow>,
    week_models: &'a [UsageWindow],
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw: Option<&'a Raw>,
}

/// The stable interface: one array of Claude rows in display order. Issue
/// #2001 leaves it an array, with no Codex rows: host scripts read it with
/// `.[]` and `.[0]`, and send stderr to `/dev/null`, so an object here would
/// make them report no capacity without a message. Codex rows are in
/// `--summary-json` only. Strings use JSON escaping and are not otherwise
/// cleaned.
pub(super) fn render_json(rows: &[Row]) -> String {
    // Plain structs of strings, numbers and options always serialise.
    serde_json::to_string_pretty(&json_rows(rows)).expect("usage rows serialise to JSON")
}

/// The capacity summary after the table: a blank line, then six lines.
pub(super) fn summary_block(s: &Summary) -> String {
    let mut out = String::from("\n");
    for l in s.text_lines() {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

#[derive(Serialize)]
struct SummaryOutput<'a> {
    at: String,
    profiles: Vec<JsonRow<'a>>,
    summary: Option<&'a Summary>,
    codex: &'a [CodexJsonRow],
}

/// `--summary-json` (issue #1797): `at` as ISO UTC, `profiles` exactly the
/// array of `--json`, the summary, `null` when there is none, and (issue
/// #2001) `codex`, the Codex rows, which `--json` does not hold.
pub(super) fn render_summary_json(
    at: i64,
    rows: &[Row],
    summary: Option<&Summary>,
    codex: &[CodexJsonRow],
) -> String {
    let out = SummaryOutput {
        at: utc_time(at),
        profiles: json_rows(rows),
        summary,
        codex,
    };
    serde_json::to_string_pretty(&out).expect("usage summary serialises to JSON")
}

/// The rows as `--json` prints them.
fn json_rows(rows: &[Row]) -> Vec<JsonRow<'_>> {
    rows.iter()
        .map(|r| JsonRow {
            profile: &r.profile,
            account: (!r.is_excluded()).then_some(r.account.as_str()),
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
            source: r.source.map(Source::as_str),
            spend: r.spend.as_str(),
            session: r.report.session.as_ref(),
            session_limit: r.report.session_limit,
            week_all: r.report.week_all.as_ref(),
            week_models: &r.report.week_models,
            error: r
                .error
                .filter(|e| *e != ErrorReason::Excluded)
                .map(|e| e.to_string()),
            raw: r.raw.as_ref(),
        })
        .collect()
}

/// One stderr line per error row. An excluded row (issue #2218) is not an
/// error.
pub(super) fn error_warnings(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter(|r| !r.is_excluded())
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

/// The notes on where a row's figures came from, printed with `--raw`
/// only. Issue #1844: one line per row that fell back after a direct read
/// failed, naming the fixed reason. Issue #1856: two lines per credits-on
/// direct row, how its spend was read and the redacted `extra_usage` shape.
pub(super) fn source_notes(rows: &[Row]) -> Vec<String> {
    let mut notes = Vec::new();
    for r in rows {
        let name = sanitize_display(&r.profile, false);
        match (r.source, &r.extra_shape) {
            (Some(Source::Fallback), _) => {
                if let Some(why) = r.direct_failure {
                    notes.push(format!(
                        "note: profile \"{name}\": direct read gave {why}; using claude"
                    ));
                }
            }
            (Some(Source::Direct), Some(shape)) => {
                let ending = match r.spend {
                    Spend::Ok | Spend::LimitReached => "spend from claude",
                    Spend::Unknown => "spend probe failed",
                    Spend::NotProbed | Spend::NotApplicable => "spend not probed",
                };
                notes.push(format!(
                    "note: profile \"{name}\": direct read gave usage credits on; windows direct, {ending}"
                ));
                notes.push(format!("note: profile \"{name}\": {}", shape.line()));
            }
            _ => {}
        }
    }
    notes
}

/// SR-10: the fixed line for a profile whose refresh request may have
/// reached the token server but whose answer was not saved: the write
/// failed, the answer was unusable, or it did not arrive before the
/// deadline. Its refresh token on disk may have been retired, so the user
/// has to log in again.
pub(super) fn not_saved_warning(profile: &str) -> String {
    format!(
        "warning: profile \"{}\": token refresh may not have been saved; run claude /login for this profile",
        sanitize_display(profile, false)
    )
}

/// One [`not_saved_warning`] per row whose direct read gave
/// `refresh not saved`, whatever the mode or `--raw`.
pub(super) fn not_saved_warnings(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.direct_failure.is_some_and(usage_source::not_saved))
        .map(|r| not_saved_warning(&r.profile))
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
        // Issue #2218: an excluded row has no reading to record.
        .filter(|r| r.is_login())
        // Issue #1844: a passive row is a stored reading; recording it again
        // would keep it fresh for ever.
        .filter(|r| r.source != Some(Source::Passive))
        .filter(|r| {
            matches!(
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
