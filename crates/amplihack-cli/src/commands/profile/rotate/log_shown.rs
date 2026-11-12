//! What the rotate log shows for each login (issue #1844): the windows as
//! read, so an exhausted login keeps its weekly figure. Issue #2013 (D7):
//! with each reading's age, whether it was too old to be used, why a probe
//! failed and how long a read still running has run. Display only: no
//! decision reads it.

use super::super::decide::{Probed, Reading};
use super::super::fresh;
use super::super::observe::Measurement;
use crate::commands::profile::name::ProfileName;

/// The longest failure reason shown, in characters.
pub const MAX_FAILURE: usize = 128;

/// What the log shows for one login (issue #1844): the reading the
/// decision used, and the session and weekly percentages as read. Display
/// only: no decision reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub reading: Reading,
    pub session: Option<f64>,
    pub week: Option<f64>,
    /// The read had not finished when the cycle stopped waiting for it.
    pub still_running: bool,
    /// Issue #2013: seconds since the reading was taken (or, for a result
    /// carried from an earlier cycle, since its read started); `None` when
    /// the time is not known, as for a [`Shown::of`] value.
    pub age: Option<i64>,
    /// Issue #2013: the reading is older than the age limit, has no reading
    /// time or one in the future, so no decision used it.
    pub stale: bool,
    /// Issue #2013: why the probe failed, through [`failure_text`].
    pub failure: Option<String>,
    /// Issue #2013: seconds a read still running has run, when known.
    pub running_for: Option<i64>,
}

/// `raw` as a failure reason may be shown: control and unprintable
/// (bidirectional-override) characters removed, cut to [`MAX_FAILURE`]
/// characters.
pub fn failure_text(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() && !super::super::ledger::unprintable(*c))
        .take(MAX_FAILURE)
        .collect()
}

impl Shown {
    /// The reading and the windows of one probe, with no time context.
    pub fn of(p: &Probed) -> Self {
        Self {
            reading: p.reading,
            session: p.windows.session.map(|w| w.used),
            week: p.windows.week.map(|w| w.used),
            still_running: p.still_running,
            age: None,
            stale: false,
            failure: p.failure.as_deref().map(failure_text),
            running_for: None,
        }
    }

    /// Issue #2013 (D7): one probe of a cycle measured from `measured_at`,
    /// shown at `now` against the age limit `max_age`.
    pub fn at(p: &Probed, measured_at: i64, now: i64, max_age: i64) -> Self {
        let since = |t: i64| now.saturating_sub(t).max(0);
        let read = fresh::read_at(p, measured_at);
        let figures = matches!(
            p.reading,
            Reading::Measured { .. } | Reading::Exhausted { .. }
        );
        Self {
            age: read.or(p.started_at).map(since),
            stale: figures && !p.still_running && fresh::stale(read, None, None, now, max_age),
            running_for: p.still_running.then_some(p.started_at).flatten().map(since),
            ..Self::of(p)
        }
    }

    /// A reading with no windows beyond what it holds itself.
    pub fn from_reading(reading: Reading) -> Self {
        let (session, week) = match reading {
            Reading::Measured { session, week } => (Some(session), Some(week)),
            Reading::Exhausted { session, week } => (session, week),
            Reading::Failed => (None, None),
        };
        Self {
            reading,
            session,
            week,
            still_running: false,
            age: None,
            stale: false,
            failure: None,
            running_for: None,
        }
    }

    /// Whether the log follows the cycle line with a `reading:` line for
    /// this login: it is stale, failed or still running.
    pub fn needs_reading_line(&self) -> bool {
        self.still_running || self.stale || self.reading == Reading::Failed
    }
}

/// What the log shows for the live login and each candidate (issue #1844),
/// at `now` against the age limit `max_age` (issue #2013): the windows read
/// when the measurement kept its probes, else the readings alone.
pub fn shown(m: &Measurement, now: i64, max_age: i64) -> (Shown, Vec<(ProfileName, Shown)>) {
    match &m.probed {
        Some((live, probes)) => (
            Shown::at(live, m.measured_at, now, max_age),
            probes
                .iter()
                .map(|(n, p)| (n.clone(), Shown::at(p, m.measured_at, now, max_age)))
                .collect(),
        ),
        None => (
            Shown::from_reading(m.live),
            m.others
                .iter()
                .map(|(n, r)| (n.clone(), Shown::from_reading(*r)))
                .collect(),
        ),
    }
}

/// The shown reading of `name`, `failed` when it was not measured.
pub fn shown_of(others: &[(ProfileName, Shown)], name: &ProfileName) -> Shown {
    others
        .iter()
        .find(|(n, _)| n == name)
        .map_or_else(|| Shown::of(&Probed::failed()), |(_, s)| s.clone())
}

/// Issue #2013 (D7): one login's reading as the log shows it:
/// `s=41% w=97% age=35s` when fresh, `s=41% w=97% stale age=7320s (not used)`
/// when too old, `probe failed: <reason>` with no percentages, and
/// `probe running since <n>s (not used)`. A value with no time context
/// ([`Shown::of`]) shows the percentages alone.
pub fn reading(r: &Shown) -> String {
    let secs = |v: Option<i64>| v.map_or_else(|| "?s".to_string(), |n| format!("{n}s"));
    if r.still_running {
        return format!("probe running since {} (not used)", secs(r.running_for));
    }
    if r.reading == Reading::Failed {
        let why = r.failure.as_deref().unwrap_or("no reading");
        return format!("probe failed: {why}");
    }
    let figures = super::msg::usage(r, " ");
    match (r.stale, r.age) {
        (true, age) => format!("{figures} stale age={} (not used)", secs(age)),
        (false, Some(age)) => format!("{figures} age={age}s"),
        (false, None) => figures,
    }
}

/// One login in the `cycle` line: `s=41%,w=97%,age=35s`, or `stale`,
/// `failed` or `running`, each of which [`reading_lines`] explains.
pub fn cell(r: &Shown) -> String {
    if r.still_running {
        return "running".to_string();
    }
    if r.reading == Reading::Failed {
        return "failed".to_string();
    }
    if r.stale {
        return "stale".to_string();
    }
    let figures = super::msg::usage(r, ",");
    r.age
        .map_or_else(|| figures.clone(), |age| format!("{figures},age={age}s"))
}

/// The `reading: <name> <reading>` lines written after the `cycle` line,
/// one for each login shown as stale, failed or running: the live login
/// first, then the others by name.
pub fn reading_lines(
    live: Option<&ProfileName>,
    live_shown: &Shown,
    others: &[(ProfileName, Shown)],
    platform: Option<&ProfileName>,
) -> Vec<String> {
    let mut sorted: Vec<&(ProfileName, Shown)> = others.iter().collect();
    sorted.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    live.map(|n| (n, live_shown))
        .into_iter()
        .chain(sorted.into_iter().map(|(n, s)| (n, s)))
        .filter(|(n, s)| Some(*n) != platform && s.needs_reading_line())
        .map(|(n, s)| format!("reading: {n} {}", reading(s)))
        .collect()
}

/// D4: `stay: no login with room; live <name> <reading>; <name> <reading>; ...`
/// with every candidate, stale and failed ones included, by name.
pub fn no_room(live: &ProfileName, live_shown: &Shown, others: &[(ProfileName, Shown)]) -> String {
    let mut sorted: Vec<&(ProfileName, Shown)> = others.iter().collect();
    sorted.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let mut out = format!(
        "stay: no login with room; live {live} {}",
        reading(live_shown)
    );
    for (n, s) in sorted {
        out.push_str(&format!("; {n} {}", reading(s)));
    }
    out
}
