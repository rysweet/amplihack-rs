//! Organisation restrictions apply after eligibility and before ranking.
use serde::{Deserialize, Serialize};

/// Only safe, present identifiers establish organisation affinity.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 320 && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Affinity {
    Same,
    Crossing,
    Unresolved,
}
impl Affinity {
    pub fn between(from: Option<&str>, to: Option<&str>) -> Self {
        match (from.filter(|v| valid_id(v)), to.filter(|v| valid_id(v))) {
            (Some(a), Some(b)) if a == b => Self::Same,
            (Some(_), Some(_)) => Self::Crossing,
            _ => Self::Unresolved,
        }
    }
    /// The kind word in the rotate log's `affinity:` line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Crossing => "crossing",
            Self::Unresolved => "unresolved",
        }
    }
    /// The rotate-log line for a swap from profile `from` to profile `to`:
    /// `affinity: <crossing|unresolved> <from>-><to>: <warning>`. `None`
    /// for a swap within one organisation, which logs nothing.
    ///
    /// A profile with no organisation record is `unresolved`, so every swap
    /// to or from such a profile writes this line and its warning. That is
    /// intended: affinity cannot be established without the record.
    pub fn log_line(self, from: &str, to: &str) -> Option<String> {
        self.warning()
            .map(|w| format!("affinity: {} {from}->{to}: {w}", self.as_str()))
    }
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::Same => None,
            Self::Crossing => Some(
                "Cross-organisation swap: earlier thinking becomes unavailable on the next turn. The visible conversation remains and the session can continue.",
            ),
            Self::Unresolved => Some(
                "Organisation affinity is unresolved: earlier thinking may become unavailable on the next turn. The visible conversation remains and the session can continue.",
            ),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    CapacityRelief,
    Upgrade,
    PlanRoom,
}
pub struct Candidate<'a> {
    pub org: Option<&'a str>,
    pub eligible: bool,
    /// The capacity-relief selector, offered only this target, picks it.
    pub capacity: bool,
    /// The plan-room fallback, offered only this target, picks it. A login
    /// can have room without strict capacity.
    pub room: bool,
}
/// The permitted pool, in the existing ranking order.
///
/// Only a confirmed crossing (both organisations known and different) is
/// restricted. An unknown organisation on either side is not a crossing, so
/// missing metadata never keeps a session on a login it would otherwise
/// leave, such as a paid overflow login once a plan login recovers. Such a
/// swap still publishes the "unresolved" warning.
///
/// - With a known source and an eligible same-organisation target with
///   capacity, only same-organisation targets are permitted.
/// - Otherwise every target that is not a confirmed crossing is permitted.
/// - A confirmed crossing is permitted only for necessary capacity relief
///   when no same-organisation target has capacity.
pub fn allowed_indices(
    source: Option<&str>,
    purpose: Purpose,
    necessary: bool,
    candidates: &[Candidate<'_>],
) -> Vec<usize> {
    let affinity = |c: &Candidate<'_>| Affinity::between(source, c.org);
    let usable = |c: &Candidate<'_>| c.eligible && c.capacity;
    let same = candidates
        .iter()
        .any(|c| usable(c) && affinity(c) == Affinity::Same);
    let crossing_allowed = !same && purpose == Purpose::CapacityRelief && necessary;
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            usable(c)
                && match affinity(c) {
                    Affinity::Same => true,
                    Affinity::Unresolved => !same,
                    Affinity::Crossing => crossing_allowed,
                }
        })
        .map(|(i, _)| i)
        .collect()
}

/// Whether a preference-only move (plan room or upgrade) may go ahead:
/// it never makes a confirmed crossing.
pub fn preference_permitted(source: Option<&str>, target: Option<&str>) -> bool {
    Affinity::between(source, target) != Affinity::Crossing
}

/// Both pools one swap decision needs, from one set of candidates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pools {
    /// Targets the selector may rank for this decision's purpose.
    pub allowed: Vec<usize>,
    /// Targets the plan-room fallback may use, worked out from `room` in
    /// place of `capacity`: a same-organisation login without room never
    /// excludes an unknown-organisation login with room.
    pub plan_room: Vec<usize>,
}

/// Why one swap decision may move the session. Every selector (the
/// rotator, the auto-switch threshold swap and the nested rerun) works this
/// out with [`Need::of`], so the rule lives in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// The live plan login still has capacity: any move is an upgrade and
    /// never makes a confirmed crossing.
    Upgrade,
    /// The live plan login is exhausted: capacity relief, which may cross
    /// when no same-organisation login has capacity.
    Relief,
    /// The session is on a paid overflow login, blocked or not. Leaving it
    /// is cost relief: it may cross when no same-organisation plan login
    /// has capacity, and its room fallback (a plan login below 100%) may
    /// cross on the same terms, because staying keeps spending money.
    LeaveOverflow,
}
impl Need {
    /// `on_overflow`: the live login is a paid overflow login.
    /// `exhausted`: the live plan login needs capacity relief.
    pub fn of(on_overflow: bool, exhausted: bool) -> Self {
        match (on_overflow, exhausted) {
            (true, _) => Self::LeaveOverflow,
            (false, true) => Self::Relief,
            (false, false) => Self::Upgrade,
        }
    }
}

/// Work out [`Pools`] once per decision.
pub fn pools(source: Option<&str>, need: Need, candidates: &[Candidate<'_>]) -> Pools {
    let (purpose, necessary) = match need {
        Need::Upgrade => (Purpose::Upgrade, false),
        Need::Relief | Need::LeaveOverflow => (Purpose::CapacityRelief, true),
    };
    let room: Vec<_> = candidates
        .iter()
        .map(|c| Candidate {
            org: c.org,
            eligible: c.eligible,
            capacity: c.room,
            room: c.room,
        })
        .collect();
    // Plan room from a plan login is a preference and never crosses. From
    // an overflow login it is the way off paid spend.
    let plan_room = match need {
        Need::LeaveOverflow => allowed_indices(source, Purpose::CapacityRelief, true, &room),
        Need::Upgrade | Need::Relief => allowed_indices(source, Purpose::PlanRoom, false, &room),
    };
    Pools {
        allowed: allowed_indices(source, purpose, necessary, candidates),
        plan_room,
    }
}
