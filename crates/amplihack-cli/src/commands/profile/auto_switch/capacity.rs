//! Writer side of the no-capacity notice: computes
//! `<root>/.auto-switch.d/capacity.json` from readings the callers already
//! have (status line, `profile usage`, the rotator), and the one-line
//! warnings `amplihack claude` and `amplihack recipe run` print.
//!
//! Nothing here makes a request or changes a caller's output or exit code:
//! write problems go to the debug log only. The file holds profile names,
//! never account keys, e-mail addresses, tokens or paths. The reader is
//! `amplihack_utils::claude_capacity`. See "When no account has capacity" in
//! docs/reference/profile-command.md.

use super::candidate::{adjusted, is_exhausted};
use super::config::{self, Config};
use super::fsx;
use super::state::{Ledger, State, UsageFile};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use amplihack_utils::claude_capacity::{self as cap, CapacityFile, State as Cap};
use chrono::DateTime;
use std::collections::BTreeMap;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

/// A file whose reset is unknown is rewritten this often while the state
/// holds, so the reader's 5-hour bound does not expire a still-true state.
const HEARTBEAT_SECS: i64 = 3600;
const HARD_LIMIT: u8 = 100;

/// The rotator's own reading of one profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rotated {
    Measured {
        session: f64,
        week: f64,
    },
    /// The session or weekly limit is reached.
    Exhausted,
    /// The probe failed: counts as no reading.
    Failed,
}

/// What a writer knows about one profile.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// The `usage.json` reading of the account live in the profile.
    Usage(Reading),
    /// The rotator's reading. `stored` is the profile's `usage.json` entry,
    /// used only for reset times.
    Rotation {
        reading: Rotated,
        stored: Option<Reading>,
    },
}

/// One account that is over the threshold.
struct Over {
    hard: bool,
    /// When it has capacity again; `None` when unknown.
    reset: Option<i64>,
}

/// The time a set of over windows clears: the latest of their resets, or
/// `None` when any of them is unknown or there are none.
fn latest(resets: &[Option<i64>]) -> Option<i64> {
    if resets.is_empty() {
        return None;
    }
    resets
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()
}

fn future(w: Option<&Window>, now: i64) -> Option<&Window> {
    w.filter(|w| w.resets_at > now)
}

/// The resets of the windows of `r` at or above `threshold`.
fn over_resets(r: &Reading, threshold: u8, now: i64) -> Vec<Option<i64>> {
    [r.five_hour.as_ref(), r.seven_day.as_ref()]
        .into_iter()
        .filter(|w| adjusted(*w, now) >= f64::from(threshold))
        .map(|w| w.map(|w| w.resets_at))
        .collect()
}

fn classify(e: Option<&Evidence>, threshold: u8, now: i64) -> Option<Over> {
    match e? {
        Evidence::Usage(r) => {
            let limit_failure = r.source == ReadingSource::UsageLimitFailure
                && (future(r.five_hour.as_ref(), now).is_some()
                    || future(r.seven_day.as_ref(), now).is_some());
            let hard = is_exhausted(Some(r), HARD_LIMIT, now) || limit_failure;
            if !hard && !is_exhausted(Some(r), threshold, now) {
                return None;
            }
            let mut resets = over_resets(r, threshold, now);
            if resets.is_empty() {
                // A usage-limit failure below the threshold: its live windows.
                resets = [r.five_hour.as_ref(), r.seven_day.as_ref()]
                    .into_iter()
                    .filter_map(|w| future(w, now).map(|w| Some(w.resets_at)))
                    .collect();
            }
            Some(Over {
                hard,
                reset: latest(&resets),
            })
        }
        Evidence::Rotation { reading, stored } => {
            let stored_reset = |w: Option<&Window>| future(w, now).map(|w| w.resets_at);
            match *reading {
                Rotated::Failed => None,
                Rotated::Exhausted => Some(Over {
                    hard: true,
                    reset: stored
                        .as_ref()
                        .and_then(|r| latest(&over_resets(r, threshold, now))),
                }),
                Rotated::Measured { session, week } => {
                    let t = f64::from(threshold);
                    if session.max(week) < t {
                        return None;
                    }
                    let s = stored.as_ref();
                    let mut resets = Vec::new();
                    if session >= t {
                        resets.push(s.and_then(|r| stored_reset(r.five_hour.as_ref())));
                    }
                    if week >= t {
                        resets.push(s.and_then(|r| stored_reset(r.seven_day.as_ref())));
                    }
                    Some(Over {
                        hard: session.max(week) >= f64::from(HARD_LIMIT),
                        reset: latest(&resets),
                    })
                }
            }
        }
    }
}

/// The capacity file for `profiles` (minus `cfg.selection_exclude()`: the
/// excluded and the overflow profiles), keyed by profile name in `evidence`.
/// An overflow profile is plan capacity only through the rotator's credits
/// rule, which `refresh_from_rotation` applies with `on_credits`. Pure.
pub fn compute(
    profiles: &[ProfileName],
    evidence: &BTreeMap<String, Evidence>,
    cfg: &Config,
    now: i64,
    writer: &str,
) -> CapacityFile {
    let skip = cfg.selection_exclude();
    let eligible: Vec<&ProfileName> = profiles.iter().filter(|p| !skip.contains(p)).collect();
    let mut over: Vec<(String, Over)> = eligible
        .iter()
        .filter_map(|p| {
            classify(evidence.get(p.as_str()), cfg.threshold, now).map(|o| (p.to_string(), o))
        })
        .collect();
    over.sort_by(|a, b| a.0.cmp(&b.0));
    let all_over = !eligible.is_empty() && over.len() == eligible.len();
    let state = if !all_over {
        Cap::Ok
    } else if over.iter().all(|(_, o)| o.hard) {
        Cap::Exhausted
    } else {
        Cap::Low
    };
    let earliest_reset = match state {
        Cap::Ok => None,
        _ => over
            .iter()
            .filter_map(|(_, o)| o.reset)
            .min()
            .and_then(|r| DateTime::from_timestamp(r, 0)),
    };
    CapacityFile {
        version: cap::VERSION,
        state,
        threshold: cfg.threshold,
        over: over.into_iter().map(|(name, _)| name).collect(),
        earliest_reset,
        written_at: DateTime::from_timestamp(now, 0).unwrap_or_default(),
        writer: writer.to_string(),
    }
}

/// The profiles under `root`, or `None` (logged) when they cannot be listed.
fn profiles(root: &Path) -> Option<Vec<ProfileName>> {
    match ProfileStore::new(root.to_path_buf()).list() {
        Ok(names) => Some(
            names
                .iter()
                .filter_map(|n| ProfileName::parse(n).ok())
                .collect(),
        ),
        Err(e) => {
            tracing::debug!(op = "capacity list profiles", error = %e);
            None
        }
    }
}

/// Recomputes and writes the capacity file from `usage` (status line and
/// `profile usage`). Never fails, never prints.
pub fn refresh(state: &State, usage: &UsageFile, ledger: &Ledger, writer: &str) {
    // catch_unwind works only because the workspace builds with
    // `panic = "unwind"` (Cargo.toml): a bug here must not end the caller.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let eff = config::effective(&state.root);
        if !eff.on {
            return;
        }
        let Some(profiles) = profiles(&state.root) else {
            return;
        };
        let evidence = profiles
            .iter()
            .filter_map(|p| {
                let account = ledger.account_in(p.as_str());
                usage
                    .accounts
                    .get(account)
                    .map(|r| (p.to_string(), Evidence::Usage(r.clone())))
            })
            .collect();
        let now = fsx::now();
        write_if_changed(
            state,
            &compute(&profiles, &evidence, &eff.config, now, writer),
            now,
        );
    }));
}

/// Recomputes and writes the capacity file from the rotator's readings.
/// Reset times come from `usage.json`. Never fails, never prints.
///
/// `on_credits`: the live login runs on a credit-backed overflow profile
/// (issue #1606), so work can go on whatever the plan readings say. The
/// result is then `ok`, which replaces an existing file but never creates
/// one (an `ok` result never does).
pub fn refresh_from_rotation(
    root: &Path,
    readings: &[(ProfileName, Rotated)],
    now: i64,
    on_credits: bool,
) {
    // See `refresh` on catch_unwind.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let eff = config::effective(root);
        if !eff.on {
            return;
        }
        let state = match State::new(root) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(op = "capacity open state", error = %e);
                return;
            }
        };
        let Some(profiles) = profiles(&state.root) else {
            return;
        };
        let usage = state.load_usage();
        let ledger = state.load_ledger();
        let ledger = ledger.inspect_err(|e| tracing::debug!(op = "capacity ledger", error = %e));
        let ledger = ledger.unwrap_or_default();
        let evidence = readings
            .iter()
            .map(|(p, reading)| {
                let stored = usage.accounts.get(ledger.account_in(p.as_str())).cloned();
                (
                    p.to_string(),
                    Evidence::Rotation {
                        reading: *reading,
                        stored,
                    },
                )
            })
            .collect();
        let mut file = compute(&profiles, &evidence, &eff.config, now, "rotate");
        if on_credits {
            file.state = Cap::Ok;
            file.over.clear();
            file.earliest_reset = None;
        }
        write_if_changed(&state, &file, now);
    }));
}

/// Writes `new` unless the file already says the same thing. An `Ok` state
/// never creates the file; an unknown reset is rewritten once an hour.
fn write_if_changed(state: &State, new: &CapacityFile, now: i64) {
    let at = DateTime::from_timestamp(now, 0).unwrap_or_default();
    match cap::read_at(&state.root, at) {
        None if new.state == Cap::Ok => return,
        Some(old)
            if old.state == new.state
                && old.over == new.over
                && old.threshold == new.threshold
                && old.earliest_reset == new.earliest_reset =>
        {
            let heartbeat = new.state != Cap::Ok
                && new.earliest_reset.is_none()
                && now.saturating_sub(old.written_at.timestamp()) >= HEARTBEAT_SECS;
            if !heartbeat {
                return;
            }
        }
        _ => {}
    }
    if let Err(e) = state.ensure() {
        tracing::debug!(op = "capacity ensure state dir", error = %e);
        return;
    }
    let bytes = match serde_json::to_vec_pretty(new) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(op = "capacity serialize", error = %e);
            return;
        }
    };
    if let Err(e) = fsx::write_private_atomic(&state.dir, cap::FILE, &bytes) {
        tracing::debug!(op = "capacity write", kind = ?e.kind());
    }
}

/// The warning line for the current state, or `None` when there is nothing
/// to say (off, ok, missing, corrupt, past the reset, no root).
fn current_line() -> Option<String> {
    statusline_line(&cap::resolve_root()?)
}

/// `amplihack claude` / launch: one line to `out` when capacity is low or
/// exhausted. Silent for `-p` / `--print` and in nested recipe sessions.
pub fn launch_warning(args: &[String], out: &mut dyn Write) {
    if args.iter().any(|a| a == "-p" || a == "--print") || cap::nested_session() {
        return;
    }
    // See `refresh` on catch_unwind.
    if let Ok(Some(line)) = catch_unwind(current_line) {
        let _ = writeln!(out, "{line} — run '{}'", cap::INSPECT);
    }
}

/// `amplihack recipe run`: one line to `out` when capacity is low or
/// exhausted. Never refuses. Silent in nested recipe sessions.
pub fn recipe_warning(out: &mut dyn Write) {
    if cap::nested_session() {
        return;
    }
    // See `refresh` on catch_unwind.
    if let Ok(Some(line)) = catch_unwind(current_line) {
        let _ = writeln!(
            out,
            "amplihack recipe run: {line} — run '{}' (continuing anyway)",
            cap::INSPECT
        );
    }
}

/// The status-line warning for `root` (the wrapper's own root), or `None`.
pub fn statusline_line(root: &Path) -> Option<String> {
    let (state, file) = cap::current_now(root)?;
    cap::format_line(state, file.threshold, file.earliest_reset)
}

/// A writer that passes bytes through unchanged and remembers the last one,
/// so the status-line warning can start on a line of its own.
pub struct Tail<W: Write> {
    inner: W,
    last: Option<u8>,
}

impl<W: Write> Tail<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, last: None }
    }

    /// Writes `line` on its own line: a newline first only when the output
    /// so far is non-empty and does not end with one.
    pub fn finish_with(mut self, line: &str) {
        let sep = if self.last.is_some_and(|b| b != b'\n') {
            "\n"
        } else {
            ""
        };
        // fail-quiet: status-line output contract.
        let _ = writeln!(self.inner, "{sep}{line}").and_then(|()| self.inner.flush());
    }
}

impl<W: Write> Write for Tail<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n > 0 {
            self.last = Some(buf[n - 1]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
