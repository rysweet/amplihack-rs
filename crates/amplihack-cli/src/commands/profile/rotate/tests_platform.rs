//! Issue #1700: the API-key tier in the rotator's overflow policy. Pure; no
//! I/O. See "API-key tier" in docs/reference/profile-command.md.
//!
//! Contract (additions to `tests_overflow`):
//! - `profile::overflow::PLATFORM_TIER == 10`, and
//!   `OverflowEntry::platform(name)` is an entry at that tier; `tiers()`
//!   appends one for the first API-key profile by name.
//! - `decide::Probed::platform()`: `Measured { session: 0, week: 0 }`,
//!   default windows, `spend_limit: false`. It is never a plan reading: a
//!   profile in the overflow order is never in the plan pool, so its 0/0
//!   never counts as plan capacity.
//! - `OverflowInput.platform_block: Option<&OverflowBlock>`: the API-key
//!   block; active while `now < until`. It covers the platform entry.
//! - Order: plan with capacity, then credit tiers 1..=9, then the API key,
//!   and only when the gate allows overflow.
//! - Off the API key, every cycle, ignoring hold and margins:
//!   plan with capacity (best by headroom) → `OverflowOut`, `Tag::Out`;
//!   else a usable credit profile (first by tier) → `OverflowOut`,
//!   `Tag::Out`; else, blocked, the best usable plan login (not exhausted,
//!   not failed) → `OverflowOut`; else, blocked, `Decision::Alert` with
//!   `Tag::On` (stay, no swap); else `Stay`, `Tag::On`.
//! - A failed plan probe is never capacity.

use super::decide::{Decision, Measured, Probed, Reading, Reason, Win, Windows};
use super::ledger::OverflowBlock;
use super::overflow::{OverflowInput, Tag, Verdict, decide_overflow};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{OverflowEntry, PLATFORM_TIER};
use std::collections::BTreeMap;

const NOW: i64 = 1_760_000_000;
const HOUR: i64 = 3600;
const FAR: Option<i64> = Some(NOW + 4 * HOUR);
const NEAR: Option<i64> = Some(NOW + 600);

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn win(used: f64, resets_at: Option<i64>) -> Option<Win> {
    Some(Win { used, resets_at })
}

fn measured(session: f64, week: f64, reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows {
            session: win(session, reset),
            week: win(week, reset),
        },
        spend_limit: false,
    }
}

fn exhausted(reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::EXHAUSTED,
        windows: Windows {
            session: win(100.0, reset),
            week: win(100.0, reset),
        },
        spend_limit: false,
    }
}

fn failed() -> Probed {
    Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: false,
    }
}

fn spend_limited() -> Probed {
    Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: true,
    }
}

fn key_block(reason: &str, until: i64) -> OverflowBlock {
    OverflowBlock {
        at: NOW - 60,
        until,
        reason: reason.into(),
    }
}

fn spend_block() -> OverflowBlock {
    OverflowBlock {
        at: NOW - 60,
        until: NOW + 10 * 86_400,
        reason: "spend_limit".into(),
    }
}

/// Plan: `work` (live unless changed), `spare`. Credit: `credits` (tier 1).
/// API key: `console` (tier 10).
struct Case {
    live_name: &'static str,
    live: Probed,
    candidates: Vec<Measured>,
    credits: bool,
    blocks: BTreeMap<String, OverflowBlock>,
    platform_block: Option<OverflowBlock>,
    swapped_at: Option<i64>,
    s: Settings,
}

impl Case {
    fn new(live_name: &'static str, live: Probed) -> Self {
        Self {
            live_name,
            live,
            candidates: Vec::new(),
            credits: true,
            blocks: BTreeMap::new(),
            platform_block: None,
            swapped_at: None,
            s: settings(),
        }
    }

    /// Live on the API key, as the rotator sees it.
    fn on_key() -> Self {
        Self::new("console", Probed::platform())
    }

    fn cand(mut self, name: &str, p: Probed) -> Self {
        self.candidates.push(Measured {
            name: n(name),
            reading: p.reading,
            windows: p.windows.clone(),
            spend_limit: p.spend_limit,
        });
        self
    }

    /// `console` as a candidate, with its synthetic reading.
    fn key(self) -> Self {
        self.cand("console", Probed::platform())
    }

    fn no_credits(mut self) -> Self {
        self.credits = false;
        self
    }

    fn run(&self) -> Verdict {
        let live_name = n(self.live_name);
        let mut overflow = Vec::new();
        if self.credits {
            overflow.push(OverflowEntry::new(n("credits"), 1));
        }
        overflow.push(OverflowEntry::platform(n("console")));
        decide_overflow(&OverflowInput {
            settings: &self.s,
            live_name: Some(&live_name),
            live: &self.live,
            candidates: &self.candidates,
            overflow: &overflow,
            blocks: &self.blocks,
            platform_block: self.platform_block.as_ref(),
            swapped_at: self.swapped_at,
            now: NOW,
        })
    }
}

fn swap(to: &str, reason: Reason) -> Decision {
    Decision::Swap { to: n(to), reason }
}

// --- The tier itself ----------------------------------------------------------------

#[test]
fn the_platform_tier_comes_after_every_credit_tier() {
    assert_eq!(PLATFORM_TIER, 10);
    let e = OverflowEntry::platform(n("console"));
    assert_eq!(e.tier, PLATFORM_TIER);
    assert_eq!(e.name, n("console"));
    assert!(e.tier > amplihack_utils::rotator_status::MAX_TIER);
}

#[test]
fn the_synthetic_reading_is_usable_and_never_a_spend_limit() {
    let p = Probed::platform();
    assert_eq!(
        p.reading,
        Reading::Measured {
            session: 0.0,
            week: 0.0
        }
    );
    assert_eq!(p.windows, Windows::default());
    assert!(!p.spend_limit);
}

// --- Onto the API key ------------------------------------------------------------------

#[test]
fn a_plan_login_with_capacity_wins_over_the_api_key() {
    let v = Case::new("work", measured(90.0, 40.0, FAR))
        .cand("spare", measured(10.0, 20.0, FAR))
        .cand("credits", measured(100.0, 100.0, FAR))
        .key()
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold));
    assert_eq!(v.tag, None);
}

#[test]
fn the_api_keys_zero_reading_is_never_plan_capacity() {
    // No plan login has capacity: console's synthetic 0/0 must not turn
    // into a threshold swap or an `upgrade`, and must not stop overflow.
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .no_credits()
        .key()
        .run();
    assert_eq!(v.decision, swap("console", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
    assert_eq!(v.next_plan_reset, FAR);
}

#[test]
fn a_usable_credit_profile_comes_before_the_api_key() {
    let v = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", measured(100.0, 100.0, FAR))
        .key()
        .run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowIn));
}

#[test]
fn the_api_key_follows_a_blocked_or_failed_credit_profile() {
    let mut blocked = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", measured(100.0, 100.0, FAR))
        .key();
    blocked.blocks.insert("credits".into(), spend_block());
    assert_eq!(blocked.run().decision, swap("console", Reason::OverflowIn));

    let failed_credit = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", failed())
        .key()
        .run();
    assert_eq!(failed_credit.decision, swap("console", Reason::OverflowIn));

    let spend = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", spend_limited())
        .key()
        .run();
    assert_eq!(spend.decision, swap("console", Reason::OverflowIn));
    assert_eq!(spend.new_blocks, vec![n("credits")], "credits blocked once");
}

#[test]
fn the_wait_gate_applies_to_the_api_key() {
    let v = Case::new("work", exhausted(NEAR))
        .cand("spare", exhausted(NEAR))
        .no_credits()
        .key()
        .run();
    assert!(
        !matches!(&v.decision, Decision::Swap { to, .. } if to == &n("console")),
        "{v:?}"
    );
    assert_eq!(v.decision, Decision::Alert);
}

#[test]
fn an_unknown_plan_reset_never_moves_onto_the_api_key_without_opt_in() {
    let mut c = Case::new("work", exhausted(None))
        .cand("spare", exhausted(None))
        .no_credits()
        .key();
    assert_eq!(c.run().decision, Decision::Alert);
    c.s.overflow_unknown_reset = true;
    assert_eq!(c.run().decision, swap("console", Reason::OverflowIn));
}

#[test]
fn a_failed_live_probe_never_moves_onto_the_api_key() {
    let v = Case::new("work", failed())
        .cand("spare", exhausted(FAR))
        .no_credits()
        .key()
        .run();
    assert!(
        !matches!(&v.decision, Decision::Swap { to, .. } if to == &n("console")),
        "{v:?}"
    );
}

#[test]
fn a_blocked_api_key_is_never_moved_onto() {
    for reason in ["key_exhausted", "key_revoked", "key_rate_limited"] {
        let mut c = Case::new("work", exhausted(FAR))
            .cand("spare", exhausted(FAR))
            .no_credits()
            .key();
        c.platform_block = Some(key_block(reason, NOW + HOUR));
        let v = c.run();
        assert_eq!(v.decision, Decision::Alert, "{reason}");
        assert_eq!(v.tag, None, "{reason}");
    }
}

#[test]
fn an_expired_key_block_no_longer_applies() {
    let mut c = Case::new("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .no_credits()
        .key();
    c.platform_block = Some(key_block("key_rate_limited", NOW));
    assert_eq!(c.run().decision, swap("console", Reason::OverflowIn));
}

#[test]
fn a_spend_limited_credit_profile_that_is_live_moves_on_to_the_api_key() {
    let v = Case::new("credits", spend_limited())
        .cand("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .key()
        .run();
    assert_eq!(v.decision, swap("console", Reason::OverflowIn));
    assert_eq!(v.new_blocks, vec![n("credits")]);
}

#[test]
fn a_usable_credit_profile_that_is_live_stays_and_never_moves_to_the_api_key() {
    let v = Case::new("credits", measured(100.0, 100.0, FAR))
        .cand("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .key()
        .run();
    assert_eq!(v.decision, Decision::Stay);
    assert_eq!(v.tag, Some(Tag::On));
}

/// Every combination: the API key is chosen only when no plan login has
/// capacity, no credit profile can be used, the key is not blocked and the
/// gate allows overflow.
#[test]
fn the_api_key_is_chosen_only_as_the_last_resort() {
    let plans = [
        ("capacity", measured(10.0, 20.0, FAR)),
        ("exhausted", exhausted(FAR)),
        ("failed", failed()),
    ];
    let credits = ["usable", "blocked", "failed", "none"];
    for (pn, plan) in &plans {
        for credit in credits {
            for key_blocked in [false, true] {
                let mut c = Case::new("work", exhausted(FAR)).cand("spare", plan.clone());
                match credit {
                    "usable" => c = c.cand("credits", measured(100.0, 100.0, FAR)),
                    "blocked" => {
                        c = c.cand("credits", measured(100.0, 100.0, FAR));
                        c.blocks.insert("credits".into(), spend_block());
                    }
                    "failed" => c = c.cand("credits", failed()),
                    _ => c = c.no_credits(),
                }
                c = c.key();
                if key_blocked {
                    c.platform_block = Some(key_block("key_exhausted", NOW + HOUR));
                }
                let v = c.run();
                let to_key =
                    matches!(&v.decision, Decision::Swap { to, .. } if to == &n("console"));
                let want = *pn != "capacity" && credit != "usable" && !key_blocked;
                assert_eq!(
                    to_key, want,
                    "plan {pn}, credit {credit}, blocked {key_blocked}: {v:?}"
                );
            }
        }
    }
}

// --- Off the API key ------------------------------------------------------------------

#[test]
fn off_the_api_key_to_a_plan_login_as_soon_as_it_has_capacity_ignoring_the_hold() {
    let mut c = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("spare", measured(84.0, 10.0, FAR))
        .cand("credits", measured(100.0, 100.0, FAR));
    c.swapped_at = Some(NOW - 60);
    let v = c.run();
    assert_eq!(v.decision, swap("spare", Reason::OverflowOut));
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn off_the_api_key_to_the_plan_login_with_the_most_headroom() {
    let v = Case::on_key()
        .cand("work", measured(60.0, 10.0, FAR))
        .cand("spare", measured(10.0, 10.0, FAR))
        .run();
    assert_eq!(v.decision, swap("spare", Reason::OverflowOut));
}

#[test]
fn a_failed_plan_probe_is_not_capacity() {
    let v = Case::on_key()
        .cand("work", failed())
        .cand("spare", exhausted(FAR))
        .no_credits()
        .run();
    assert_eq!(v.decision, Decision::Stay);
    assert_eq!(v.tag, Some(Tag::On));
}

#[test]
fn off_the_api_key_to_credits_when_no_plan_login_has_capacity() {
    let mut c = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", measured(100.0, 100.0, FAR));
    c.swapped_at = Some(NOW - 60);
    let v = c.run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowOut));
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn a_blocked_or_failed_credit_profile_is_not_a_way_off_the_api_key() {
    let mut c = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", measured(100.0, 100.0, FAR));
    c.blocks.insert("credits".into(), spend_block());
    assert_eq!(c.run().decision, Decision::Stay);

    let v = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("credits", failed())
        .run();
    assert_eq!(v.decision, Decision::Stay);
}

#[test]
fn staying_on_the_api_key_is_tagged_on() {
    let v = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .no_credits()
        .run();
    assert_eq!(v.decision, Decision::Stay);
    assert_eq!(v.tag, Some(Tag::On));
    assert_eq!(v.next_plan_reset, FAR);
    assert!(v.new_blocks.is_empty());
}

#[test]
fn a_blocked_api_key_moves_to_a_credit_profile() {
    let mut c = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("spare", exhausted(FAR))
        .cand("credits", measured(100.0, 100.0, FAR));
    c.platform_block = Some(key_block("key_exhausted", NOW + HOUR));
    let v = c.run();
    assert_eq!(v.decision, swap("credits", Reason::OverflowOut));
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn a_blocked_api_key_with_nowhere_to_go_alerts_and_stays() {
    let mut c = Case::on_key()
        .cand("work", exhausted(FAR))
        .cand("spare", failed())
        .no_credits();
    c.platform_block = Some(key_block("key_revoked", NOW + 62 * 86_400));
    let v = c.run();
    assert_eq!(v.decision, Decision::Alert, "no swap attempt");
    assert_eq!(v.tag, Some(Tag::On), "still on the API key");
    // The same verdict on the next cycle: no loop, no swap.
    assert_eq!(c.run(), v);
}
