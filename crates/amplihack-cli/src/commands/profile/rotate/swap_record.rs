//! The swap record (issue #2085): `<rotate dir>/swap.json`, written once per
//! rotator swap so the limit-resume hook can resume sessions stopped by a
//! usage limit. The schema and the hook's reader are in
//! `amplihack_utils::rotator_status::swap_record`. Only a cycle's swap
//! writes one: a `profile switch` and an auto-switch swap never do (D1, D2).

use super::decide::Reading;
use super::ledger::RotateState;
use crate::commands::profile::ProfileError;
use amplihack_utils::rotator_status::swap_record::{SWAP_FILE, SwapReading, SwapRecord};
use serde::{Deserialize, Serialize};

/// Original measurement and publication identity, saved before movement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Completion {
    reading: Option<SwapReading>,
    from_org: Option<String>,
    to_org: Option<String>,
    record: Option<SwapRecord>,
    completed_at: Option<i64>,
    overflow: Option<String>,
}
impl Completion {
    pub(super) fn placed(&mut self, at: i64) {
        self.completed_at.get_or_insert(at);
    }
    pub(super) fn valid(&self, from: &str, to: &str) -> bool {
        let orgs = [&self.from_org, &self.to_org].into_iter().all(|v| {
            v.as_deref()
                .is_none_or(amplihack_utils::organisation_affinity::valid_id)
        });
        orgs && self
            .overflow
            .as_deref()
            .is_none_or(|t| super::ledger::OVERFLOW_TAGS.contains(&t))
            && self.reading.as_ref().is_none_or(|r| r.measured_at > 0)
            && self.completed_at.is_none_or(|t| t > 0)
            && self.record.as_ref().is_none_or(|r| {
                r.seq > 0
                    && r.from_login == from
                    && r.live_login == to
                    && Some(r.swapped_at) == self.completed_at
                    && Some(&r.reading) == self.reading.as_ref()
                    && r.from_org == self.from_org
                    && r.to_org == self.to_org
                    && r.affinity
                        == Some(amplihack_utils::organisation_affinity::Affinity::between(
                            self.from_org.as_deref(),
                            self.to_org.as_deref(),
                        ))
            })
    }
    pub(super) fn new(
        reading: &Reading,
        measured_at: i64,
        orgs: (Option<&str>, Option<&str>),
        tag: Option<super::overflow::Tag>,
    ) -> Self {
        let reading = match *reading {
            Reading::Measured { session, week } if session.is_finite() && week.is_finite() => {
                Some(SwapReading {
                    session: percent(session),
                    week: percent(week),
                    measured_at,
                })
            }
            _ => None,
        };
        Self {
            reading,
            from_org: orgs
                .0
                .filter(|id| amplihack_utils::organisation_affinity::valid_id(id))
                .map(str::to_owned),
            to_org: orgs
                .1
                .filter(|id| amplihack_utils::organisation_affinity::valid_id(id))
                .map(str::to_owned),
            record: None,
            completed_at: None,
            overflow: tag.map(|t| t.as_str().to_string()),
        }
    }
}

/// Called after placement is complete and ownership is committed. Persist the
/// event identity before publishing it, so replay never increments seq twice.
pub(super) fn complete(
    rs: &RotateState,
    j: &mut super::ledger::RotateJournal,
    at: i64,
) -> Result<(), ProfileError> {
    use super::ledger::LastCycle;
    let Some(c) = &mut j.completion else {
        return Ok(());
    };
    let completed = *c.completed_at.get_or_insert(at);
    let mut l = rs.load_ledger()?.unwrap_or_default();
    if l.live.as_deref() != Some(j.to.as_str()) {
        return Err(ProfileError::RotateInconsistent(
            "completion ownership is not committed".into(),
        ));
    }
    if l.last_cycle
        .as_ref()
        .is_some_and(|v| v.outcome == super::ledger::SWITCHED && v.at > j.started_at)
    {
        return Err(ProfileError::RotateInconsistent(
            "newer manual-switch record preserved; completion retained".into(),
        ));
    }
    l.since = Some(completed);
    l.swapped_at = Some(completed);
    if !super::ledger::newer_than(
        l.last_cycle.as_ref(),
        c.reading.as_ref().map_or(j.started_at, |r| r.measured_at),
    ) || l
        .last_cycle
        .as_ref()
        .is_some_and(|v| v.outcome == "swapped")
    {
        l.last_cycle = Some(LastCycle {
            at: completed,
            outcome: "swapped".into(),
            measured_at: Some(c.reading.as_ref().map_or(j.started_at, |r| r.measured_at)),
            overflow: c.overflow.clone(),
        });
        rs.save_ledger(&l)?;
    }
    if c.record.is_none()
        && let Some(reading) = &c.reading
    {
        let previous = rs
            .read(&rs.dir.join(SWAP_FILE), "swap record")?
            .map(|raw| amplihack_utils::rotator_status::swap_record::parse_swap(&raw, at).ok_or(()))
            .transpose()
            .map_err(|_| {
                ProfileError::RotateInconsistent(
                    "previous swap record is unreadable; completion retained".into(),
                )
            })?;
        c.record = Some(SwapRecord {
            seq: previous
                .map_or(Some(1), |p| p.seq.checked_add(1))
                .ok_or_else(|| {
                    ProfileError::RotateInconsistent(
                        "swap event sequence exhausted; completion retained".into(),
                    )
                })?,
            from_login: j.from.clone(),
            live_login: j.to.clone(),
            swapped_at: completed,
            reading: reading.clone(),
            from_org: c.from_org.clone(),
            to_org: c.to_org.clone(),
            affinity: Some(amplihack_utils::organisation_affinity::Affinity::between(
                c.from_org.as_deref(),
                c.to_org.as_deref(),
            )),
        });
    }
    rs.save_journal(j)?;
    if let Some(record) = j.completion.as_ref().and_then(|c| c.record.as_ref()) {
        let bytes =
            serde_json::to_vec(record).map_err(|e| ProfileError::Io(std::io::Error::other(e)))?;
        if rs.read(&rs.dir.join(SWAP_FILE), "swap record")?.as_deref() != Some(bytes.as_slice()) {
            rs.write(SWAP_FILE, &bytes)?;
        }
        crate::commands::profile::auto_switch::kit::abort_point("swap-event-published");
    }
    rs.delete_journal()
}

/// A percentage as stored: rounded up and clamped to `0..=255`. The caller
/// has checked it is finite.
fn percent(p: f64) -> u8 {
    p.ceil().clamp(0.0, 255.0) as u8
}
