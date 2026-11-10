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
    pub capacity: bool,
}
/// Preserve the existing ranking order within the permitted pool.
pub fn allowed_indices(
    source: Option<&str>,
    purpose: Purpose,
    necessary: bool,
    candidates: &[Candidate<'_>],
) -> Vec<usize> {
    let same = candidates
        .iter()
        .any(|c| c.eligible && c.capacity && Affinity::between(source, c.org) == Affinity::Same);
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.eligible
                && c.capacity
                && (Affinity::between(source, c.org) == Affinity::Same
                    || (!same && purpose == Purpose::CapacityRelief && necessary))
        })
        .map(|(i, _)| i)
        .collect()
}
