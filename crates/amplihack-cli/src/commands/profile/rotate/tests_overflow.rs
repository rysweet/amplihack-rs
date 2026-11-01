//! `rotate::overflow`: the credit-backed overflow policy (issue #1606).
//!
//! Contract (pure; no I/O):
//! - `decide::Win { used: f64, resets_at: Option<i64> }` and
//!   `decide::Windows { session: Option<Win>, week: Option<Win> }`
//!   (`Default` = both `None`).
//! - `decide::Probed { reading: Reading, windows: Windows, spend_limit: bool,
//!   unknown: bool }` with `Probed::from_report(&UsageReport)`. `reading` is
//!   exactly `Reading::from_report` (a spend limit still reads `Failed`);
//!   `windows` keeps `used_percentage` and `resets_at` for every state,
//!   exhausted ones included; `spend_limit` is `state == SpendLimit`;
//!   `unknown` is `state` `timeout` or `error` (issue #1698).
//! - `decide::Measured { name, reading, windows, spend_limit, unknown }`.
//! - `decide::Reason::{OverflowIn, OverflowOut}`, `as_str` `overflow-in`
//!   and `overflow-out`.
//! - `overflow::Tag::{In, Out, On}` with `as_str` `in`, `out`, `on`.
//! - `profile::overflow::Gate { wait, unknown_reset, session_at, week_at }`,
//!   built by `overflow::gate(&Settings)`: `plan_capacity(session, week)` is
//!   both below their thresholds; `far(earliest, now)` is
//!   `earliest - now > wait`, and `None` (no reset known) is far only with
//!   `unknown_reset` (`--overflow-unknown-reset 1`; default off).
//! - `overflow::decide_overflow(&OverflowInput) -> Verdict` where
//!   `OverflowInput { settings, live_name: Option<&ProfileName>, live:
//!   &Probed, candidates: &[Measured], refused: &[ProfileName], overflow:
//!   &[OverflowEntry], blocks: &BTreeMap<String, OverflowBlock>, swapped_at,
//!   now }` and
//!   `Verdict { decision: Decision, tag: Option<Tag>, new_blocks:
//!   Vec<ProfileName> }`.
//!
//! Policy (docs/reference/profile-command.md, "Overflow"):
//! - A plan login has capacity when its probe read `ok` and its session and
//!   week use are below `session_at` and `week_at`. Margins and the hold play
//!   no part in that test.
//! - Onto credits only when the live plan login is at a threshold, no plan
//!   login has capacity, and the earliest plan reset is more than
//!   `overflow_wait` away (unknown: only with the opt-in). A failed live
//!   probe never moves onto overflow, and neither does a plan candidate's
//!   unknown probe or a plan profile skipped as `directory refused` (issue
//!   #1698). This replaces the ALERT and also a
//!   threshold swap onto a plan login without capacity; that swap stands
//!   when the gate does not allow overflow.
//! - Overflow profiles are tried by tier, then by the order given.
//! - Off credits as soon as a plan login has capacity, ignoring hold, margin
//!   and upgrade margin.
//! - A spend-limit reading on an overflow profile blocks it once.
//!
//! "Hold" here is `--hold`, the minimum time after a rotator swap. A
//! `profile switch` pin wins over every rule above, because the cycle stops
//! on it before this policy runs; that needs a cycle, not this pure
//! function, so its tests are in `tests_pin_overflow.rs`.

use super::decide::{Decision, Measured, Probed, Reading, Reason, Win, Windows, decide};
use super::ledger::OverflowBlock;
use super::measure::reading;
use super::overflow::{OverflowInput, Tag, Verdict, decide_overflow, gate};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use crate::commands::profile::usage_cmd::{ErrorReason, Probe};
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::collections::BTreeMap;

const NOW: i64 = 1_760_000_000;
const HOUR: i64 = 3600;
const WAIT: u64 = 1800;
/// A plan reset well beyond the wait.
const FAR: Option<i64> = Some(NOW + 4 * HOUR);

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: WAIT,
        overflow_unknown_reset: false,
    }
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn win(used: f64, resets_at: Option<i64>) -> Option<Win> {
    Some(Win { used, resets_at })
}

/// A measured plan reading with reset times.
fn measured(session: f64, week: f64, s_reset: Option<i64>, w_reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows {
            session: win(session, s_reset),
            week: win(week, w_reset),
        },
        spend_limit: false,
        unknown: false,
    }
}

/// An exhausted reading (session at 100%) with reset times.
fn exhausted(s_reset: Option<i64>, w_reset: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Exhausted,
        windows: Windows {
            session: win(100.0, s_reset),
            week: win(100.0, w_reset),
        },
        spend_limit: false,
        unknown: false,
    }
}

/// A probe that read a lasting state (`not_logged_in`, `unparsed`): failed,
/// but not unknown.
fn failed() -> Probed {
    Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

/// A probe that timed out (issue #1698): unknown.
fn timed_out() -> Probed {
    Probed::from_report(&UsageReport::empty(UsageState::Timeout))
}

fn spend_limited() -> Probed {
    Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: true,
        unknown: false,
    }
}

fn cand(name: &str, p: &Probed) -> Measured {
    Measured {
        name: n(name),
        reading: p.reading,
        windows: p.windows.clone(),
        spend_limit: p.spend_limit,
        unknown: p.unknown,
    }
}

fn swap(to: &str, reason: Reason) -> Decision {
    Decision::Swap { to: n(to), reason }
}

fn block(at: i64, until: i64) -> OverflowBlock {
    OverflowBlock {
        at,
        until,
        reason: "spend_limit".into(),
    }
}

struct Case<'a> {
    live_name: &'a str,
    live: Probed,
    candidates: Vec<Measured>,
    refused: Vec<ProfileName>,
    overflow: Vec<&'a str>,
    blocks: BTreeMap<String, OverflowBlock>,
    swapped_at: Option<i64>,
    s: Settings,
}

impl<'a> Case<'a> {
    fn new(live_name: &'a str, live: Probed) -> Self {
        Self {
            live_name,
            live,
            candidates: Vec::new(),
            refused: Vec::new(),
            overflow: Vec::new(),
            blocks: BTreeMap::new(),
            swapped_at: None,
            s: settings(),
        }
    }

    fn cand(mut self, name: &str, p: Probed) -> Self {
        self.candidates.push(cand(name, &p));
        self
    }

    fn refused(mut self, names: &[&str]) -> Self {
        self.refused = names.iter().map(|x| n(x)).collect();
        self
    }

    fn overflow(mut self, names: &[&'a str]) -> Self {
        self.overflow = names.to_vec();
        self
    }

    /// `name` or `name:tier`, as in `.auto-switch`.
    fn run(&self) -> Verdict {
        let live_name = n(self.live_name);
        let overflow: Vec<OverflowEntry> = self
            .overflow
            .iter()
            .map(|o| {
                let (name, tier) =
                    amplihack_utils::rotator_status::parse_overflow_entry(o).unwrap();
                OverflowEntry::new(n(&name), tier)
            })
            .collect();
        decide_overflow(&OverflowInput {
            settings: &self.s,
            live_name: Some(&live_name),
            live: &self.live,
            candidates: &self.candidates,
            refused: &self.refused,
            overflow: &overflow,
            blocks: &self.blocks,
            platform_block: None,
            swapped_at: self.swapped_at,
            now: NOW,
        })
    }
}

// --- Probed ----------------------------------------------------------------

fn uw(label: &str, used: f64, resets_at: Option<i64>) -> UsageWindow {
    UsageWindow {
        label: label.into(),
        used_percentage: used,
        resets_text: resets_at.map(|_| "3pm (UTC)".into()),
        resets_at,
    }
}

fn report(
    state: UsageState,
    s: Option<(f64, Option<i64>)>,
    w: Option<(f64, Option<i64>)>,
) -> UsageReport {
    UsageReport {
        state,
        session: s.map(|(u, r)| uw("session", u, r)),
        week_all: w.map(|(u, r)| uw("all models", u, r)),
        week_models: Vec::new(),
        session_limit: true,
    }
}

#[test]
fn probed_keeps_reset_times_for_exhausted_readings() {
    let p = Probed::from_report(&report(
        UsageState::SessionExhausted,
        Some((100.0, Some(NOW + HOUR))),
        Some((40.0, Some(NOW + 50 * HOUR))),
    ));
    assert_eq!(p.reading, Reading::Exhausted);
    assert!(!p.spend_limit);
    assert_eq!(p.windows.session, win(100.0, Some(NOW + HOUR)));
    assert_eq!(p.windows.week, win(40.0, Some(NOW + 50 * HOUR)));
}

#[test]
fn probed_reading_matches_reading_from_report_for_every_state() {
    for state in [
        UsageState::Ok,
        UsageState::SessionExhausted,
        UsageState::WeeklyExhausted,
        UsageState::SpendLimit,
        UsageState::NotLoggedIn,
        UsageState::Timeout,
        UsageState::Unparsed,
        UsageState::Error,
    ] {
        let r = report(state, Some((10.0, None)), Some((20.0, None)));
        let p = Probed::from_report(&r);
        assert_eq!(p.reading, Reading::from_report(&r), "{state:?}");
        assert_eq!(p.spend_limit, state == UsageState::SpendLimit, "{state:?}");
    }
}

#[test]
fn a_spend_limit_still_reads_failed() {
    let p = Probed::from_report(&report(UsageState::SpendLimit, None, None));
    assert_eq!(
        p.reading,
        Reading::Failed,
        "non-overflow paths stay as today"
    );
    assert!(p.spend_limit);
}

#[test]
fn new_reasons_and_tags_have_stable_names() {
    assert_eq!(Reason::OverflowIn.as_str(), "overflow-in");
    assert_eq!(Reason::OverflowOut.as_str(), "overflow-out");
    assert_eq!(Tag::In.as_str(), "in");
    assert_eq!(Tag::Out.as_str(), "out");
    assert_eq!(Tag::On.as_str(), "on");
}

// --- The shared gate --------------------------------------------------------

#[test]
fn the_gate_is_strictly_beyond_the_wait() {
    let g = gate(&settings());
    assert_eq!(
        g,
        Gate {
            wait: 1800,
            unknown_reset: false,
            session_at: 85,
            week_at: 90,
        }
    );
    assert!(!g.far(Some(NOW + 1800), NOW), "== wait stays");
    assert!(g.far(Some(NOW + 1801), NOW));
    assert!(!g.far(Some(NOW + 60), NOW));
    assert!(!g.far(Some(NOW - 60), NOW), "a past reset");
    let zero = Gate { wait: 0, ..g };
    assert!(zero.far(Some(NOW + 1), NOW), "wait 0");
    assert!(!zero.far(Some(NOW), NOW));
}

#[test]
fn an_unknown_reset_is_far_only_with_the_opt_in() {
    let g = gate(&settings());
    assert!(
        !g.far(None, NOW),
        "the default never spends on an unknown reset"
    );
    let opt_in = Gate {
        unknown_reset: true,
        ..g
    };
    assert!(opt_in.far(None, NOW));
}

#[test]
fn the_gate_never_allows_overflow_while_a_plan_profile_has_capacity() {
    let g = Gate {
        wait: 0,
        unknown_reset: true,
        ..gate(&settings())
    };
    assert!(!g.allows(true, Some(NOW + 100 * HOUR), NOW));
    assert!(!g.allows(true, None, NOW));
    assert!(g.allows(false, Some(NOW + 1), NOW));
}

// --- P1: no overflow profiles means today's decide() -----------------------

#[test]
fn p1_without_overflow_profiles_the_verdict_is_decide() {
    let far = Some(NOW + 50 * HOUR);
    let cases: Vec<Case> = vec![
        Case::new("work", measured(90.0, 40.0, far, far))
            .cand("spare", measured(10.0, 20.0, far, far)),
        Case::new("work", measured(90.0, 40.0, far, far)).cand("spare", exhausted(far, far)),
        Case::new("work", measured(90.0, 40.0, far, far)).cand("spare", failed()),
        Case::new("work", measured(10.0, 10.0, far, far))
            .cand("spare", measured(0.0, 0.0, far, far)),
        Case::new("work", measured(10.0, 10.0, far, far))
            .cand("spare", measured(5.0, 5.0, far, far)),
        Case::new("work", exhausted(far, far)).cand("spare", measured(50.0, 50.0, far, far)),
        Case::new("work", exhausted(FAR, FAR)).cand("spare", exhausted(None, None)),
        Case::new("work", failed()).cand("spare", measured(10.0, 10.0, far, far)),
        Case::new("work", spend_limited()).cand("spare", measured(10.0, 10.0, far, far)),
        Case::new("work", measured(85.0, 0.0, far, far))
            .cand("spare", measured(84.0, 0.0, far, far)),
        Case::new("work", measured(90.0, 40.0, far, far)),
        Case {
            swapped_at: Some(NOW - 10),
            ..Case::new("work", measured(10.0, 10.0, far, far))
                .cand("spare", measured(0.0, 0.0, far, far))
        },
    ];
    for (i, c) in cases.iter().enumerate() {
        let v = c.run();
        let want = decide(&c.s, &c.live.reading, &c.candidates, c.swapped_at, NOW);
        assert_eq!(v.decision, want, "case {i}");
        assert_eq!(v.tag, None, "case {i}");
        assert!(v.new_blocks.is_empty(), "case {i}");
    }
}

#[test]
fn p1_an_unmarked_profile_with_a_spend_limit_is_not_blocked() {
    let v = Case::new("work", measured(90.0, 40.0, None, None))
        .cand("spare", spend_limited())
        .run();
    assert!(v.new_blocks.is_empty());
    assert_eq!(v.decision, Decision::Alert);
}

// --- Onto credits ----------------------------------------------------------

fn all_exhausted_far() -> Case<'static> {
    let far = Some(NOW + 5 * HOUR);
    Case::new("work", exhausted(far, far))
        .cand("spare", exhausted(far, far))
        .cand(
            "cred",
            exhausted(Some(NOW + 10 * HOUR), Some(NOW + 100 * HOUR)),
        )
        .overflow(&["cred"])
}

#[test]
fn moves_onto_overflow_when_every_plan_login_is_out_and_the_reset_is_far() {
    let v = all_exhausted_far().run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
    assert!(v.new_blocks.is_empty());
}

#[test]
fn never_moves_onto_overflow_while_a_plan_login_has_capacity() {
    let far = Some(NOW + 5 * HOUR);
    let v = Case::new("work", measured(90.0, 40.0, far, far))
        .cand("spare", measured(10.0, 20.0, far, far))
        .cand("cred", measured(0.0, 0.0, far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold));
    assert_eq!(v.tag, None);
}

#[test]
fn never_upgrades_onto_overflow_below_the_thresholds() {
    let far = Some(NOW + 5 * HOUR);
    let v = Case::new("work", measured(60.0, 60.0, far, far))
        .cand("cred", measured(0.0, 0.0, far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(
        v.decision,
        Decision::Stay,
        "an overflow profile is never an upgrade"
    );
    assert_eq!(v.tag, None);
}

#[test]
fn a_plan_login_below_the_thresholds_but_short_of_the_margin_alerts_without_overflow() {
    // work at the session threshold (headroom 15); spare below both
    // thresholds (headroom 16) but only 1 point better: no plan swap, and
    // spare still has capacity, so no credits either.
    let far = Some(NOW + 5 * HOUR);
    let v = Case::new("work", measured(85.0, 0.0, far, far))
        .cand("spare", measured(84.0, 0.0, far, far))
        .cand("cred", exhausted(far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
}

#[test]
fn a_reset_exactly_at_the_wait_stays_on_the_plan() {
    let at_wait = Some(NOW + WAIT as i64);
    let v = Case::new("work", exhausted(at_wait, at_wait))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Alert, "today's no-capacity behaviour");
    assert_eq!(v.tag, None);
}

#[test]
fn a_reset_one_second_past_the_wait_moves_onto_overflow() {
    let past = Some(NOW + WAIT as i64 + 1);
    let v = Case::new("work", exhausted(past, past))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
}

#[test]
fn the_earliest_plan_reset_decides_the_wait() {
    // work resets far away, but spare's session resets in ten minutes.
    // spare at 96% has 4 points of headroom, short of --margin 5, so the
    // plan rule alone gives no swap (at 95% it would swap to spare).
    let far = Some(NOW + 50 * HOUR);
    let v = Case::new("work", exhausted(far, far))
        .cand("spare", measured(96.0, 10.0, Some(NOW + 600), far))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.tag, None, "a plan login is back within the wait");
    assert_eq!(v.decision, Decision::Alert);
}

#[test]
fn only_the_windows_at_the_threshold_set_the_regain_time() {
    // Session at the threshold resets in 10 minutes; the week is at 40% and
    // its far reset does not matter.
    let v = Case::new(
        "work",
        measured(90.0, 40.0, Some(NOW + 600), Some(NOW + 100 * HOUR)),
    )
    .cand("cred", exhausted(None, None))
    .overflow(&["cred"])
    .run();
    assert_eq!(v.tag, None);
    assert_eq!(v.decision, Decision::Alert);
}

#[test]
fn with_both_windows_at_the_threshold_the_later_reset_counts() {
    let v = Case::new(
        "work",
        measured(90.0, 95.0, Some(NOW + 600), Some(NOW + 100 * HOUR)),
    )
    .cand("cred", exhausted(None, None))
    .overflow(&["cred"])
    .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn an_unknown_reset_never_moves_onto_overflow_by_default() {
    // No plan login reports a reset time: the default is to stay on the
    // plan and alert, not to spend.
    let v = Case::new("work", exhausted(None, None))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
}

#[test]
fn an_unknown_reset_moves_onto_overflow_with_the_opt_in() {
    let mut c = Case::new("work", exhausted(None, None))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"]);
    c.s.overflow_unknown_reset = true;
    let v = c.run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.next_plan_reset, None, "the log line says unknown");
}

#[test]
fn one_known_reset_is_enough_and_unknown_ones_never_shorten_the_wait() {
    // work's reset is unknown; spare's is known and far: the gate uses it.
    let v = Case::new("work", exhausted(None, None))
        .cand("spare", exhausted(FAR, FAR))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.next_plan_reset, FAR);
}

#[test]
fn a_failed_plan_candidate_never_shortens_the_wait() {
    // work resets soon; spare failed (no reset known). The earliest known
    // reset is work's, inside the wait.
    let soon = Some(NOW + 600);
    let v = Case::new("work", exhausted(soon, soon))
        .cand("spare", failed())
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.tag, None);
    assert_eq!(v.decision, Decision::Alert);
    // With work's reset far, a spare whose probe timed out holds overflow
    // back: it may have capacity (issue #1698).
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("spare", timed_out())
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.tag, None);
    assert_eq!(v.decision, Decision::Alert);
    // A lasting failure adds nothing, so overflow starts.
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("spare", failed())
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
}

// --- Issue #1698: an unknown plan probe blocks the move onto overflow -----

#[test]
fn every_plan_probe_unknown_gives_the_decide_result_not_overflow() {
    // The defect: the plan logins' probes timed out under load, and the
    // rotator moved work onto credits although they may have capacity.
    let run = |live: Probed, opt_in: bool| {
        let mut c = Case::new("work", live)
            .cand("spare", timed_out())
            .cand("extra", timed_out())
            .cand("cred", exhausted(None, None))
            .overflow(&["cred"]);
        c.s.overflow_unknown_reset = opt_in;
        let v = c.run();
        let want = decide(&c.s, &c.live.reading, &c.candidates[..2], c.swapped_at, NOW);
        (v, want)
    };
    for (live, opt_in) in [
        (exhausted(None, None), true),
        (exhausted(FAR, FAR), false),
        (exhausted(FAR, FAR), true),
        (measured(90.0, 40.0, FAR, FAR), false),
    ] {
        let (v, want) = run(live, opt_in);
        assert_eq!(v.decision, Decision::Alert);
        assert_eq!(v.decision, want);
        assert_eq!(v.tag, None);
        assert_eq!(v.next_plan_reset, None);
        assert!(v.new_blocks.is_empty());
    }
}

#[test]
fn one_unknown_plan_probe_among_exhausted_ones_blocks_overflow() {
    let case = |extra: Probed| {
        Case::new("work", exhausted(FAR, FAR))
            .cand("spare", exhausted(FAR, FAR))
            .cand("extra", extra)
            .cand("cred", exhausted(None, None))
            .overflow(&["cred"])
            .run()
    };
    let v = case(timed_out());
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
    let v = case(exhausted(FAR, FAR));
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
}

#[test]
fn an_unknown_plan_probe_keeps_a_threshold_swap_off_overflow() {
    // decide swaps onto spare, itself at a threshold; with extra unknown that
    // swap stands instead of overflow.
    let v = Case::new("work", measured(95.0, 40.0, FAR, FAR))
        .cand("spare", measured(86.0, 10.0, FAR, FAR))
        .cand("extra", timed_out())
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold));
    assert_eq!(v.tag, None);
}

#[test]
fn a_lasting_plan_failure_does_not_block_overflow() {
    // Logged out, unreadable or at the spend limit: Failed, but what the
    // login reported, so overflow is not held back on every cycle.
    for state in [
        UsageState::NotLoggedIn,
        UsageState::Unparsed,
        UsageState::SpendLimit,
    ] {
        let spare = Probed::from_report(&UsageReport::empty(state));
        assert_eq!(spare.reading, Reading::Failed, "{state:?}");
        assert!(!spare.unknown, "{state:?}");
        let v = Case::new("work", exhausted(FAR, FAR))
            .cand("spare", spare)
            .cand("cred", exhausted(None, None))
            .overflow(&["cred"])
            .run();
        assert_eq!(v.decision, swap("cred", Reason::OverflowIn), "{state:?}");
        assert_eq!(v.tag, Some(Tag::In));
    }
}

#[test]
fn a_refused_plan_profile_directory_blocks_overflow() {
    // A directory refused just before the spawn reads `error`; that login
    // may have capacity, so the move onto credits is held in this cycle.
    let refused = Probe {
        report: UsageReport::empty(UsageState::Error),
        error: Some(ErrorReason::ProfileRefused),
        raw: None,
    };
    let spare = reading("spare", &refused);
    assert!(spare.unknown);
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("spare", spare)
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
    assert!(v.new_blocks.is_empty());
}

#[test]
fn a_plan_profile_skipped_as_directory_refused_blocks_overflow() {
    // Its login cannot be checked, so it may have capacity. The skip comes
    // back every cycle until the directory is fixed, and so does the hold.
    let case = || {
        Case::new("work", exhausted(FAR, FAR))
            .cand("cred", exhausted(None, None))
            .overflow(&["cred"])
    };
    let v = case().refused(&["odd"]).run();
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
    assert!(v.new_blocks.is_empty());
    // Fixed (no longer skipped as refused): the move goes ahead.
    let v = case().run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn an_overflow_profile_skipped_as_directory_refused_holds_nothing() {
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred", "odd:2"])
        .refused(&["odd"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn an_unknown_overflow_probe_is_skipped_for_the_next() {
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", timed_out())
        .cand("cred2", exhausted(None, None))
        .overflow(&["cred", "cred2"])
        .run();
    assert_eq!(v.decision, swap("cred2", Reason::OverflowIn));
}

#[test]
fn a_blocked_live_overflow_moves_on_despite_an_unknown_plan_probe() {
    // Decision 7: only the move from a plan login onto credits is held; a
    // live overflow profile that cannot spend still moves to the next.
    let mut c = Case::new("cred", exhausted(None, None))
        .cand("cred2", exhausted(None, None))
        .cand("spare", exhausted(FAR, FAR))
        .cand("extra", timed_out())
        .overflow(&["cred", "cred2"]);
    c.blocks
        .insert("cred".into(), block(NOW - HOUR, NOW + 24 * HOUR));
    let v = c.run();
    assert_eq!(v.decision, swap("cred2", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn a_failed_live_probe_never_moves_onto_overflow() {
    for live in [failed(), spend_limited()] {
        let v = Case::new("work", live)
            .cand("cred", exhausted(None, None))
            .overflow(&["cred"])
            .run();
        assert_ne!(v.decision, swap("cred", Reason::OverflowIn));
        assert_eq!(v.tag, None);
        assert!(v.new_blocks.is_empty(), "work is not an overflow profile");
    }
}

#[test]
fn a_failed_overflow_candidate_is_not_eligible() {
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", failed())
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
}

#[test]
fn an_overflow_profile_absent_from_the_candidates_is_not_chosen() {
    // Excluded, leased or parked profiles never reach the candidate list.
    let v = Case::new("work", exhausted(FAR, FAR))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Alert);
}

#[test]
fn a_lower_tier_is_tried_first_whatever_the_order_given() {
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("key", measured(0.0, 0.0, None, None))
        .cand("cred", measured(0.0, 0.0, None, None))
        .overflow(&["key:2", "cred"])
        .run();
    assert_eq!(
        v.decision,
        swap("cred", Reason::OverflowIn),
        "tier 1 before tier 2"
    );
    // With tier 1 blocked, tier 2 is used.
    let mut c = Case::new("work", exhausted(FAR, FAR))
        .cand("key", measured(0.0, 0.0, None, None))
        .cand("cred", measured(0.0, 0.0, None, None))
        .overflow(&["key:2", "cred"]);
    c.blocks
        .insert("cred".into(), block(NOW - 60, NOW + 86_400));
    assert_eq!(c.run().decision, swap("key", Reason::OverflowIn));
}

#[test]
fn config_order_ranks_overflow_profiles() {
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("alpha", measured(0.0, 0.0, None, None))
        .cand("zeta", exhausted(None, None))
        .overflow(&["zeta", "alpha"])
        .run();
    assert_eq!(
        v.decision,
        swap("zeta", Reason::OverflowIn),
        "first eligible in config order"
    );
}

#[test]
fn overflow_candidates_never_count_as_plan_capacity() {
    // cred reads well under the thresholds, but it is overflow: work must
    // still go onto credits by the overflow rule, not a plan threshold swap.
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", measured(0.0, 0.0, None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
}

// --- While on credits ------------------------------------------------------

#[test]
fn moves_off_overflow_as_soon_as_a_plan_login_has_capacity() {
    let v = Case {
        swapped_at: Some(NOW - 10),
        ..Case::new("cred", exhausted(None, None))
            .cand("spare", measured(10.0, 10.0, None, None))
            .overflow(&["cred"])
    }
    .run();
    assert_eq!(
        v.decision,
        swap("spare", Reason::OverflowOut),
        "the hold does not apply"
    );
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn moving_off_overflow_ignores_margin_and_upgrade_margin() {
    // The overflow login reads only 5% used, so a plan login with 16 points
    // of headroom is a loss by headroom; it still wins because it is free.
    let v = Case::new("cred", measured(5.0, 5.0, None, None))
        .cand("spare", measured(84.0, 0.0, None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("spare", Reason::OverflowOut));
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn moving_off_overflow_picks_the_best_plan_login_with_capacity() {
    let v = Case::new("cred", exhausted(None, None))
        .cand("spare", measured(60.0, 10.0, None, None))
        .cand("extra", measured(10.0, 10.0, None, None))
        .cand("over", measured(86.0, 0.0, None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("extra", Reason::OverflowOut));
}

#[test]
fn a_plan_login_at_a_threshold_is_no_reason_to_leave_overflow() {
    let v = Case::new("cred", exhausted(None, None))
        .cand("spare", measured(85.0, 0.0, None, None))
        .cand("extra", measured(0.0, 90.0, None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, Decision::Stay);
    assert_eq!(v.tag, Some(Tag::On));
}

#[test]
fn stays_on_overflow_while_no_plan_login_has_capacity() {
    let v = Case::new("cred", exhausted(None, None))
        .cand("spare", exhausted(Some(NOW + 600), None))
        .cand("extra", failed())
        .overflow(&["cred"])
        .run();
    assert_eq!(
        v.decision,
        Decision::Stay,
        "a near reset does not end credits early"
    );
    assert_eq!(v.tag, Some(Tag::On));
    assert!(v.new_blocks.is_empty());
}

#[test]
fn p2_no_flapping_after_moving_off_overflow() {
    // Cycle 1: on credits, spare has capacity -> out.
    let v = Case::new("cred", exhausted(None, None))
        .cand("spare", measured(84.0, 0.0, None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("spare", Reason::OverflowOut));
    // Cycle 2: spare is live and unchanged; cred reads idle. Below the
    // thresholds nothing moves onto credits.
    let v = Case {
        swapped_at: Some(NOW),
        ..Case::new("spare", measured(84.0, 0.0, None, None))
            .cand("cred", measured(0.0, 0.0, None, None))
            .overflow(&["cred"])
    }
    .run();
    assert_eq!(v.decision, Decision::Stay);
    assert_eq!(v.tag, None);
}

#[test]
fn p2_no_flapping_after_moving_onto_overflow() {
    // Cycle 1 moved onto cred because work and spare were exhausted.
    // Cycle 2 sees the same plan readings: stay on.
    let v = Case {
        swapped_at: Some(NOW - 600),
        ..Case::new("cred", exhausted(None, None))
            .cand("work", exhausted(None, None))
            .cand("spare", exhausted(None, None))
            .overflow(&["cred"])
    }
    .run();
    assert_eq!(v.decision, Decision::Stay);
    assert_eq!(v.tag, Some(Tag::On));
}

// --- Spend limit -----------------------------------------------------------

#[test]
fn a_spend_limit_on_the_live_overflow_moves_to_the_next_overflow() {
    let v = Case::new("cred", spend_limited())
        .cand("cred2", exhausted(None, None))
        .cand("spare", exhausted(FAR, FAR))
        .overflow(&["cred", "cred2"])
        .run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    assert_eq!(v.decision, swap("cred2", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
    // With no plan reset known, the next overflow profile is not started.
    let v = Case::new("cred", spend_limited())
        .cand("cred2", exhausted(None, None))
        .cand("spare", exhausted(None, None))
        .overflow(&["cred", "cred2"])
        .run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    assert_ne!(v.decision, swap("cred2", Reason::OverflowIn));
}

#[test]
fn a_spend_limit_on_the_live_overflow_falls_back_to_the_best_plan_login() {
    let v = Case::new("cred", spend_limited())
        .cand("spare", measured(95.0, 50.0, None, None))
        .cand("extra", measured(99.0, 50.0, None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    match &v.decision {
        Decision::Swap { to, .. } => assert_eq!(to.as_str(), "spare"),
        other => panic!("expected a swap to spare, got {other:?}"),
    }
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn a_spend_limit_on_the_live_overflow_with_nowhere_to_go_alerts() {
    let v = Case::new("cred", spend_limited())
        .cand("spare", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    assert_eq!(v.decision, Decision::Alert);
}

#[test]
fn a_spend_limit_on_a_candidate_blocks_it_and_skips_it() {
    let v = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", spend_limited())
        .cand("cred2", exhausted(None, None))
        .overflow(&["cred", "cred2"])
        .run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    assert_eq!(v.decision, swap("cred2", Reason::OverflowIn));
}

#[test]
fn a_spend_limit_on_a_candidate_is_blocked_even_when_no_move_happens() {
    // A plan login has capacity, so nothing moves onto credits, but the
    // evidence is still recorded so a later cycle cannot swap onto it.
    let v = Case::new("work", measured(10.0, 10.0, None, None))
        .cand("cred", spend_limited())
        .overflow(&["cred"])
        .run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    assert_eq!(v.tag, None);
}

#[test]
fn an_active_block_makes_the_profile_ineligible_and_is_not_repeated() {
    let mut c = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"]);
    c.blocks
        .insert("cred".into(), block(NOW - HOUR, NOW + 24 * HOUR));
    let v = c.run();
    assert_eq!(v.decision, Decision::Alert);
    assert!(v.new_blocks.is_empty());

    // A still-blocked live overflow that reads the limit again: no new block.
    let mut c = Case::new("cred", spend_limited())
        .cand("spare", exhausted(None, None))
        .overflow(&["cred"]);
    c.blocks
        .insert("cred".into(), block(NOW - HOUR, NOW + 24 * HOUR));
    let v = c.run();
    assert!(v.new_blocks.is_empty(), "alert once per block");
}

#[test]
fn a_blocked_live_overflow_moves_away_without_a_new_reading() {
    let mut c = Case::new("cred", exhausted(None, None))
        .cand("cred2", exhausted(None, None))
        .cand("spare", exhausted(FAR, FAR))
        .overflow(&["cred", "cred2"]);
    c.blocks
        .insert("cred".into(), block(NOW - HOUR, NOW + 24 * HOUR));
    let v = c.run();
    assert_eq!(v.decision, swap("cred2", Reason::OverflowIn));
}

#[test]
fn an_expired_block_is_ignored() {
    let mut c = Case::new("work", exhausted(FAR, FAR))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"]);
    c.blocks
        .insert("cred".into(), block(NOW - 40 * 24 * HOUR, NOW));
    let v = c.run();
    assert_eq!(
        v.decision,
        swap("cred", Reason::OverflowIn),
        "until is exclusive"
    );
}

#[test]
fn a_block_for_an_unmarked_profile_changes_nothing() {
    let far = Some(NOW + 50 * HOUR);
    let mut c = Case::new("work", measured(90.0, 40.0, far, far))
        .cand("spare", measured(10.0, 20.0, far, far));
    c.blocks
        .insert("spare".into(), block(NOW - HOUR, NOW + 24 * HOUR));
    let v = c.run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold));
}

// --- Fail-closed and fallback cases (issue #1606, step 6) ------------------

#[test]
fn either_plan_limit_takes_away_capacity() {
    // `spare` is over the session threshold only, `extra` over the week
    // threshold only, and neither has enough more headroom than `work` for a
    // threshold swap: no plan login has capacity, so credits are used.
    let far = Some(NOW + 5 * HOUR);
    let v = Case::new("work", measured(90.0, 10.0, far, far))
        .cand("spare", measured(86.0, 10.0, far, far))
        .cand("extra", measured(10.0, 91.0, far, far))
        .cand("cred", exhausted(far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
}

#[test]
fn a_plan_login_over_a_threshold_is_used_only_when_overflow_is_not_allowed() {
    // `spare` at 86% session is itself at `--session-at` (85), so it is not
    // plan capacity: with the plan reset far away the rotator moves onto
    // credits, as auto-switch does on the same readings. Swapping onto
    // `spare` would only meet the threshold again next cycle.
    let far = Some(NOW + 5 * HOUR);
    let v = Case::new("work", exhausted(far, far))
        .cand("spare", measured(86.0, 10.0, far, far))
        .cand("cred", exhausted(far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
    assert_eq!(v.tag, Some(Tag::In));
    // With the reset near, the gate does not allow overflow, and `decide`'s
    // free swap onto `spare` stands: it costs nothing.
    let near = Some(NOW + 600);
    let v = Case::new("work", exhausted(near, near))
        .cand("spare", measured(86.0, 10.0, near, near))
        .cand("cred", exhausted(far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold));
    assert_eq!(v.tag, None);
}

#[test]
fn a_threshold_swap_onto_a_plan_login_with_capacity_is_never_replaced() {
    let far = Some(NOW + 5 * HOUR);
    let v = Case::new("work", exhausted(far, far))
        .cand("spare", measured(84.0, 10.0, far, far))
        .cand("cred", measured(0.0, 0.0, far, far))
        .overflow(&["cred"])
        .run();
    assert_eq!(v.decision, swap("spare", Reason::Threshold));
    assert_eq!(v.tag, None);
}

#[test]
fn every_overflow_profile_blocked_means_no_move_onto_overflow() {
    let mut c = Case::new("work", exhausted(FAR, FAR))
        .cand("spare", exhausted(None, None))
        .cand("cred", exhausted(None, None))
        .cand("cred2", exhausted(None, None))
        .overflow(&["cred", "cred2"]);
    for o in ["cred", "cred2"] {
        c.blocks
            .insert(o.into(), block(NOW - HOUR, NOW + 24 * HOUR));
    }
    let v = c.run();
    assert_eq!(v.decision, Decision::Alert);
    assert_eq!(v.tag, None);
    assert!(v.new_blocks.is_empty());
}

#[test]
fn a_blocked_overflow_profile_is_never_reselected() {
    // `cred` is live and just hit its spend limit; `cred2` is blocked
    // already. The fallback is the best plan login, never either of them.
    let mut c = Case::new("cred", spend_limited())
        .cand("cred2", exhausted(None, None))
        .cand("spare", measured(95.0, 50.0, None, None))
        .overflow(&["cred", "cred2"]);
    c.blocks
        .insert("cred2".into(), block(NOW - HOUR, NOW + 24 * HOUR));
    let v = c.run();
    assert_eq!(v.new_blocks, vec![n("cred")]);
    assert_eq!(v.decision, swap("spare", Reason::OverflowOut));
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn a_known_reset_inside_the_wait_keeps_the_plan_even_with_overflow_free() {
    // Only the session window is at its threshold, and it resets in ten
    // minutes.
    let soon = Some(NOW + 10 * 60);
    let v = Case::new("work", measured(90.0, 40.0, soon, Some(NOW + 50 * HOUR)))
        .cand(
            "spare",
            exhausted(Some(NOW + 50 * HOUR), Some(NOW + 50 * HOUR)),
        )
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"])
        .run();
    assert_eq!(
        v.decision,
        Decision::Alert,
        "ten minutes is inside the wait"
    );
    assert_eq!(v.tag, None);
}

#[test]
fn a_zero_wait_moves_onto_overflow_as_soon_as_capacity_is_gone() {
    let mut c = Case::new("work", exhausted(Some(NOW + 60), Some(NOW + 60)))
        .cand("spare", exhausted(Some(NOW + 60), Some(NOW + 60)))
        .cand("cred", exhausted(None, None))
        .overflow(&["cred"]);
    c.s.overflow_wait = 0;
    let v = c.run();
    assert_eq!(v.decision, swap("cred", Reason::OverflowIn));
}

#[test]
fn moving_back_off_overflow_ignores_a_recent_swap() {
    // Swapped onto credits a minute ago; the hold is an hour. A plan login
    // with capacity still wins at once.
    let mut c = Case::new("cred", exhausted(None, None))
        .cand("spare", measured(10.0, 10.0, None, None))
        .overflow(&["cred"]);
    c.swapped_at = Some(NOW - 60);
    let v = c.run();
    assert_eq!(v.decision, swap("spare", Reason::OverflowOut));
    assert_eq!(v.tag, Some(Tag::Out));
}

// `.default` (D7) is not tested here. The pointer is not an input to the
// policy, so a unit case cannot set it. The cycle-level test that writes
// `.default` and compares verdicts with and without it is
// `tests/profile_overflow.rs::the_default_pointer_does_not_change_the_rotators_verdict`.
