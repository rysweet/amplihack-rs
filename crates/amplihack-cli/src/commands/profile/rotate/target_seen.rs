//! What a cycle saw of each login and why it was or was not a target (issue
//! #2910): the probe labels and last good readings the decision uses
//! ([`Extra`]), and the standing of every plan login for the cycle line, the
//! no-target line and the stay notes ([`Seen`]).
//!
//! [`Extra`] is built once a cycle at the edge ([`Extra::for_cycle`], the one
//! history read) and passed to every decision of the cycle, so the decision
//! itself opens no file and the live step, the gate and revalidation all
//! decide on the same last good readings. None of these types is persisted.
//!
//! Issue #3105: [`Extra`] also holds the newest reading time the rotator
//! already has for each candidate (the watch's kept readings and the history
//! points read for the cycle). A dated reading older than that is
//! superseded: never ranked, never substituted and never swapped on.

use super::super::identify::Skip;
use super::super::ledger::OverflowBlock;
use super::super::measure::ProbeLabel;
use super::super::observe::Snap;
use super::last_good::{LastGood, last_good};
use super::target::{Block, NoMove, Unfit, fit_blocked, standing_block};
use super::{Measured, Probed, Reading};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::OverflowEntry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The cycle's extra inputs to the decision.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extra {
    /// The label of each candidate whose probe said nothing about its room.
    pub labels: BTreeMap<ProfileName, ProbeLabel>,
    /// Candidates rejected at confirmation for an unknown reading: their
    /// last good reading is no longer used in this cycle.
    pub rejected: BTreeSet<ProfileName>,
    /// Last good readings of the labelled candidates; `None` when the history
    /// was not read, so nothing is substituted.
    pub last_good: Option<BTreeMap<ProfileName, LastGood>>,
    /// The newest reading time already held for each candidate (issue
    /// #3105): its kept reading, or its history point if that is newer.
    pub newest: BTreeMap<ProfileName, i64>,
}

impl Extra {
    /// The cycle's `Extra`: `labels` from the measurement, and the last good
    /// readings of the labelled candidates that are neither the live login
    /// nor an overflow profile, from one read of the usage history. With no
    /// such candidate the history is not read. `kept` is when the watch took
    /// the reading it keeps of each login (`Stored::times`, empty for the
    /// one-shot `rotate`); with the history points it gives
    /// [`newest`](Self::newest), for the same candidates.
    pub fn for_cycle(
        root: &Path,
        labels: Vec<(ProfileName, ProbeLabel)>,
        kept: Vec<(ProfileName, i64)>,
        overflow: &[OverflowEntry],
        live: Option<&ProfileName>,
        now: i64,
    ) -> Self {
        let candidate = |n: &ProfileName| Some(n) != live && !overflow.iter().any(|e| &e.name == n);
        let labels: BTreeMap<ProfileName, ProbeLabel> = labels.into_iter().collect();
        let names: Vec<ProfileName> = labels.keys().filter(|n| candidate(n)).cloned().collect();
        let last_good = (!names.is_empty()).then(|| last_good(root, &names, now));
        let mut newest: BTreeMap<ProfileName, i64> =
            kept.into_iter().filter(|(n, _)| candidate(n)).collect();
        for (n, lg) in last_good.iter().flatten() {
            let at = newest.entry(n.clone()).or_insert(lg.at);
            *at = (*at).max(lg.at);
        }
        Self {
            labels,
            last_good,
            newest,
            ..Self::default()
        }
    }

    /// How many seconds a reading of `name` taken at `at` is older than the
    /// newest one already held for it; `None` when it is not strictly older.
    pub fn older_by(&self, name: &ProfileName, at: i64) -> Option<u64> {
        let newest = *self.newest.get(name)?;
        (at < newest).then(|| newest.abs_diff(at))
    }

    /// The last good reading that stands in for `name`'s failed probe `p`:
    /// only for an unknown, labelled probe that read nothing, never for a
    /// login rejected at confirmation, and never when a newer reading of
    /// the login is already held (issue #3105).
    pub fn substitute(&self, name: &ProfileName, p: &Probed) -> Option<&LastGood> {
        if p.reading != Reading::Failed || !p.unknown || p.spend_limit {
            return None;
        }
        if self.rejected.contains(name) || !self.labels.contains_key(name) {
            return None;
        }
        let lg = self.last_good.as_ref()?.get(name)?;
        self.older_by(name, lg.at).is_none().then_some(lg)
    }
}

/// Where the reading the cycle used for a login came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Basis {
    /// The cycle's own probe or re-read.
    Probe,
    /// The probe said nothing (`label`); its last good reading, `age`
    /// seconds old, stands in.
    LastGood {
        label: ProbeLabel,
        reading: Reading,
        age: u64,
    },
    /// The probe said nothing and there is no last good reading.
    Unknown(ProbeLabel),
    /// The probe failed for a reason about the login, or with no label.
    Failed,
    /// The login was not probed because its identity could not be read.
    IdentityUnavailable,
    /// The cycle did not read this plan login at all, for the reason the
    /// `skip:` line of the same cycle gives (issue #2910): a directory that
    /// failed the ownership and mode check, a session running on it, a lease,
    /// a host, or a parked login. Its room is not known, so the cycle cannot
    /// say that no login has room.
    NotRead(&'static str),
}

/// One plan login's standing in the cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct Standing {
    pub basis: Basis,
    /// Its room now, or why it is not eligible, on the reading used (a
    /// blocked login is [`Unfit::Blocked`] whatever the reading).
    pub fit: Result<u8, Unfit>,
    /// An active recorded block.
    pub block: Option<Block>,
}

/// What the cycle saw, for its lines and for the swap step.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub extra: Extra,
    /// Every plan login the cycle measured, and every one it skipped without
    /// reading (issue #2910), so a login whose room was never read is counted.
    pub logins: BTreeMap<ProfileName, Standing>,
    /// The decision may make a confirmed organisation crossing.
    pub may_cross: bool,
    /// Why an ALERT decision did not move.
    pub why: Option<NoMove>,
    /// Candidates left untried at the re-read limit.
    pub not_tried: BTreeSet<ProfileName>,
    /// `--target-week-max`, for the lines.
    pub week_max: u8,
}

/// The reading the decision uses for `name`: the probe's, or the last good
/// reading standing in for it. A probe dated before a reading already held
/// for the login (a passive reading older than the watch's kept one) is
/// superseded and reads as nothing (issue #3105).
pub fn reading_used(extra: &Extra, name: &ProfileName, p: &Probed) -> Reading {
    if p.observed_at
        .is_some_and(|at| extra.older_by(name, at).is_some())
    {
        return Reading::Failed;
    }
    extra.substitute(name, p).map_or(p.reading, |lg| lg.reading)
}

/// The cycle's [`Seen`] for the plan candidates in `probes` and the plan
/// logins `snap` skipped as `no account`. `may_cross` and `why` are set by
/// the caller.
pub fn build(
    snap: &Snap,
    extra: Extra,
    probes: &[(ProfileName, Probed)],
    blocks: &BTreeMap<String, OverflowBlock>,
    week_max: u8,
    now: i64,
) -> Seen {
    let overflow = snap.decision_overflow();
    let is_overflow = |n: &ProfileName| overflow.iter().any(|e| &e.name == n);
    let mut logins = BTreeMap::new();
    for (name, p) in probes.iter().filter(|(n, _)| !is_overflow(n)) {
        let block = standing_block(blocks, name, now);
        let label = extra.labels.get(name).copied();
        let basis = match (extra.substitute(name, p), label) {
            (Some(lg), Some(label)) => Basis::LastGood {
                label,
                reading: lg.reading,
                // A reading dated after `now` (a clock step back) is 0 old.
                age: u64::try_from(now.saturating_sub(lg.at)).unwrap_or(0),
            },
            _ if p.reading != Reading::Failed || p.spend_limit => Basis::Probe,
            (_, Some(label)) => Basis::Unknown(label),
            (_, None) => Basis::Failed,
        };
        let mut m = Measured::new(name.clone(), p);
        m.reading = reading_used(&extra, name, p);
        let fit = fit_blocked(&m, block, week_max);
        logins.insert(name.clone(), Standing { basis, fit, block });
    }
    // Issue #2910: every plan login the cycle did not read, with the reason
    // the cycle's own `skip:` line gives. Without these a cycle could log
    // `skip: x (directory refused)` and then claim `no profile has capacity`
    // in the same breath, which is the wording the deputy reported on 22
    // November. `no account` keeps its own basis, which the line already
    // reports as `unknown (identity unavailable)`.
    let reason_of =
        |name: &ProfileName| snap.skips.iter().find(|(s, _)| s == name).map(|(_, k)| *k);
    let unread = snap
        .unmeasured_plan()
        .into_iter()
        .chain(snap.refused_plan());
    for name in unread {
        let basis = match reason_of(&name) {
            Some(Skip::NoAccount) => Basis::IdentityUnavailable,
            Some(k) => Basis::NotRead(k.as_str()),
            None => Basis::NotRead(Skip::Refused.as_str()),
        };
        logins.entry(name).or_insert(Standing {
            basis,
            fit: Err(Unfit::NoReading),
            block: None,
        });
    }
    Seen {
        extra,
        logins,
        week_max,
        ..Seen::default()
    }
}

#[cfg(test)]
#[path = "tests_issue_2910_cycle.rs"]
mod tests_issue_2910_cycle;
#[cfg(test)]
#[path = "tests_issue_2910_deployed.rs"]
mod tests_issue_2910_deployed;
/// The shared fixture of the cycle tests: also used by `target_confirm`'s.
#[cfg(test)]
#[path = "tests_issue_2910_fixture.rs"]
pub(in crate::commands::profile::rotate) mod tests_issue_2910_fixture;
#[cfg(test)]
#[path = "tests_issue_3105_cycle.rs"]
mod tests_issue_3105_cycle;
