//! The Codex decision: pure, the rules of "Codex decision" in order.

use super::lines::clean_label;
use amplihack_utils::codex_capacity::{AccountResult, Outcome, headroom, next_choice};

/// Everything the decision looks at.
#[derive(Debug, Clone)]
pub(super) struct DecideInput<'a> {
    /// This cycle's readings.
    pub rows: &'a [AccountResult],
    /// The live account's label, when the link points at one.
    pub live: Option<&'a str>,
    /// The credit floor.
    pub floor: f64,
    /// `codex-state.json` holds an open journal.
    pub journal_open: bool,
    /// The last completed Codex swap.
    pub last_swap_at: Option<i64>,
    pub now: i64,
    /// `--hold` seconds; 0 turns the hold off.
    pub hold: u64,
    /// `--margin` points.
    pub margin: u8,
}

/// Why nothing is swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoneReason {
    NoRoom,
    NoLive,
    Recovering,
    Held,
    Damped,
}

impl NoneReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoRoom => "no-room",
            Self::NoLive => "no-live",
            Self::Recovering => "recovering",
            Self::Held => "held",
            Self::Damped => "damped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Decision {
    Keep,
    Swap { from: String, to: String },
    None(NoneReason),
}

impl Decision {
    /// The cycle line's `-> ...` part, labels cleaned.
    pub(super) fn text(&self) -> String {
        match self {
            Self::Keep => "keep".to_string(),
            Self::Swap { from, to } => format!("swap {}->{}", clean_label(from), clean_label(to)),
            Self::None(r) => format!("none({})", r.as_str()),
        }
    }
}

/// The room of the row labelled `label`; `-1` when it is missing or could
/// not be read.
fn room_of(rows: &[AccountResult], label: &str, floor: f64) -> f64 {
    rows.iter()
        .find(|r| r.label == label)
        .and_then(|r| match &r.outcome {
            Outcome::Ok(reading) => Some(headroom(reading, floor)),
            _ => None,
        })
        .unwrap_or(-1.0)
}

/// The first rule that applies decides.
pub(super) fn decide(i: &DecideInput<'_>) -> Decision {
    // An interrupted swap is recovered on the next cycle, whatever the room
    // and even when the link is missing or dangling.
    if i.journal_open {
        return Decision::None(NoneReason::Recovering);
    }
    let Some(choice) = next_choice(i.rows, i.floor) else {
        return Decision::None(NoneReason::NoRoom);
    };
    let Some(live) = i.live else {
        return Decision::None(NoneReason::NoLive);
    };
    let to = &i.rows[choice].label;
    if to == live {
        return Decision::Keep;
    }
    let live_room = room_of(i.rows, live, i.floor);
    if live_room > 0.0 {
        let held = i.hold > 0
            && i.last_swap_at
                .is_some_and(|at| i.now.saturating_sub(at) < i.hold.min(i64::MAX as u64) as i64);
        if held {
            return Decision::None(NoneReason::Held);
        }
        let gain = room_of(i.rows, to, i.floor) - live_room;
        if gain < f64::from(i.margin) {
            return Decision::None(NoneReason::Damped);
        }
    }
    Decision::Swap {
        from: live.to_string(),
        to: to.clone(),
    }
}
