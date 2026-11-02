//! Issue #1804 (D4, docs change 5): the log lines written for a move onto a
//! per-token profile, and the legacy unmarked live API key (D6). Pure; no
//! I/O.
//!
//! Contract (`overflow_msg`, re-exported through `log::msg`):
//! - The earliest plan reset, `None` when unknown, and
//!   `CheckNote::{Key(&'static str), Credit}`.
//! - `per_token_reason(to: &ProfileName, tier: u8, earliest: Option<i64>, check:
//!   CheckNote) -> String`, one line:
//!   `per-token <to>: no measured plan login below 100%, 0 not measured; <gate>; marked overflow
//!   tier <tier>; <check>`, where `<gate>` is
//!   `earliest reset <RFC3339> beyond overflow_wait` or `reset unknown`
//!   and `<check>` is `login check passed (<detail>)` or `probe ok, no
//!   spend limit`. It takes no string read from a profile file.
//! - `onto_platform(to)` is `moved ~/.claude onto API key profile <to>
//!   (billed per token)` with nothing appended.
//! - `key_check_refused(name, reason)` and `would_refuse(name, reason)` are
//!   `key_check_refused <name>: <reason>` and `would_refuse <name>: <reason>`.
//! - `onto_overflow` and `stay_on_overflow` give the rule as `no plan login
//!   below 100%`, not `no plan profile has capacity`.
//!
//! Contract (`Snap`), legacy live state: an unmarked API-key profile left
//! live by an older release is adopted for the exit rules only. It is
//! reported in `decision_overflow` at the API-key tier so the rotator can
//! leave it, but `platform()` stays `None`, so it never counts as a mark
//! and is never a target.

use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{OverflowEntry, PLATFORM_TIER};
use crate::commands::profile::rotate::decide::{
    Decision, Measured, Probed, Reading, Reason, Win, Windows,
};
use crate::commands::profile::rotate::identify::LiveId;
use crate::commands::profile::rotate::log::{msg, timestamp};
use crate::commands::profile::rotate::observe::Snap;
use crate::commands::profile::rotate::overflow::{OverflowInput, Tag, decide_overflow};
use crate::commands::profile::rotate::overflow_msg::{
    CheckNote, per_token_off_key, per_token_reason,
};
use crate::commands::profile::rotate::pin::PinState;
use crate::commands::profile::rotate::settings::Settings;
use std::collections::BTreeMap;

const NOW: i64 = 1_793_389_376; // 2026-10-30T19:42:56Z
const FAR: Option<i64> = Some(NOW + 4 * 3600);

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

// --- per_token_reason (D4) ------------------------------------------------------------

#[test]
fn the_api_key_line_names_the_rule_gate_mark_and_check_in_order() {
    let at = NOW + 4 * 3600;
    let line = per_token_reason(
        &n("api-key"),
        PLATFORM_TIER,
        Some(at),
        0,
        CheckNote::Key("primaryApiKey set"),
    );
    assert_eq!(
        line,
        format!(
            "per-token api-key: no measured plan login below 100%, 0 not measured; earliest reset {} beyond overflow_wait; marked overflow tier 10; \
             login check passed (primaryApiKey set)",
            timestamp(at)
        )
    );
}

#[test]
fn an_unknown_reset_is_named_as_such() {
    let line = per_token_reason(
        &n("api-key"),
        PLATFORM_TIER,
        None,
        0,
        CheckNote::Key("apiKeyHelper set"),
    );
    assert_eq!(
        line,
        "per-token api-key: no measured plan login below 100%, 0 not measured; reset unknown; marked overflow \
         tier 10; login check passed (apiKeyHelper set)"
    );
    assert!(!line.contains("beyond overflow_wait"), "{line}");
}

#[test]
fn a_credit_line_names_its_tier_and_the_probe_not_a_login_check() {
    let at = NOW + 3600;
    let line = per_token_reason(&n("team-credits"), 2, Some(at), 0, CheckNote::Credit);
    assert_eq!(
        line,
        format!(
            "per-token team-credits: no measured plan login below 100%, 0 not measured; earliest reset {} beyond overflow_wait; marked overflow tier 2; \
             probe ok, no spend limit",
            timestamp(at)
        )
    );
    assert!(!line.contains("login check"), "{line}");
}

#[test]
fn the_per_token_line_counts_the_plan_logins_it_could_not_see() {
    let line = per_token_reason(&n("credits"), 1, None, 2, CheckNote::Credit);
    assert!(
        line.starts_with("per-token credits: no measured plan login below 100%, 2 not measured; "),
        "{line}"
    );
}

#[test]
fn a_move_off_the_key_makes_no_claim_about_plan_room() {
    // A plan login with room can be refused as a target (a session on it),
    // so this line must not say that none has room.
    let line = per_token_off_key(&n("credits"), 1, CheckNote::Credit);
    assert!(!line.contains("below 100%"), "{line}");
}

#[test]
fn the_key_held_alert_names_the_logins_not_measured() {
    assert_eq!(
        msg::alert_key_held("work", &[n("ccapstone"), n("spare")]),
        "ALERT: API key not used live=work: no measured plan login has room, \
         but these were not measured and may have: ccapstone, spare"
    );
}

#[test]
fn a_detected_mark_says_what_it_allows_and_how_to_undo_it() {
    let line = msg::detected_mark(&n("old-credits"));
    assert!(line.contains("detected as an API-key profile"), "{line}");
    assert!(line.contains("per-token billing"), "{line}");
    assert!(
        line.ends_with("amplihack profile overflow remove old-credits"),
        "{line}"
    );
}

#[test]
fn the_per_token_line_is_a_single_line() {
    for check in [CheckNote::Key("ANTHROPIC_API_KEY set"), CheckNote::Credit] {
        for gate in [Some(NOW), None] {
            let line = per_token_reason(&n("api-key"), PLATFORM_TIER, gate, 0, check);
            assert!(!line.contains('\n') && !line.contains('\r'), "{line:?}");
        }
    }
}

// --- The other overflow lines -----------------------------------------------------------

#[test]
fn the_moved_line_carries_no_reasons() {
    assert_eq!(
        msg::onto_platform(&n("api-key")),
        "moved ~/.claude onto API key profile api-key (billed per token)"
    );
}

#[test]
fn the_refusal_lines_use_the_documented_keys() {
    assert_eq!(
        msg::key_check_refused(&n("api-key"), "no setting"),
        "key_check_refused api-key: no setting"
    );
    assert_eq!(
        msg::would_refuse(&n("api-key"), "no setting"),
        "would_refuse api-key: no setting"
    );
}

#[test]
fn the_overflow_on_line_states_the_room_rule() {
    assert_eq!(
        msg::onto_overflow(&n("extra"), None),
        "overflow: on extra; no measured plan login below 100%, next plan reset unknown"
    );
    let at = NOW + 3600;
    assert_eq!(
        msg::onto_overflow(&n("extra"), Some(at)),
        format!(
            "overflow: on extra; no measured plan login below 100%, next plan reset {}",
            timestamp(at)
        )
    );
}

#[test]
fn the_staying_line_states_the_room_rule() {
    assert_eq!(
        msg::stay_on_overflow(&n("extra"), None),
        "overflow: staying on extra; no measured plan login below 100%, next plan reset unknown"
    );
    assert!(!msg::stay_on_overflow(&n("extra"), Some(NOW)).contains("has capacity"));
}

#[test]
fn the_not_marked_skip_names_overflow_add() {
    let s = msg::platform_not_marked(&n("api-key"));
    assert!(s.contains("api-key is not marked"), "{s}");
    assert!(s.contains("amplihack profile overflow add api-key"), "{s}");
}

// --- Legacy: an unmarked API key left live (D6) ------------------------------------------

fn snap_on_unmarked_key() -> Snap {
    Snap {
        live: LiveId::Known(n("api-key")),
        proto: None,
        account: None,
        candidates: vec![n("work"), n("spare")],
        skips: Vec::new(),
        sees_sessions: true,
        overflow: Vec::new(),
        pin: PinState::None,
        ignored: Vec::new(),
        unmarked: vec![n("api-key")],
        detected_mark: None,
        home_no_oauth: true,
        candidate_accounts: Vec::new(),
    }
}

#[test]
fn an_unmarked_live_api_key_is_adopted_for_the_exit_rules_only() {
    let s = snap_on_unmarked_key();
    assert_eq!(s.platform(), None, "an unmarked key is never a mark");
    assert_eq!(s.adopted_platform(), Some(&n("api-key")));
    assert_eq!(
        s.decision_overflow(),
        vec![OverflowEntry::platform(n("api-key"))]
    );
}

#[test]
fn nothing_is_adopted_while_a_plan_login_is_live() {
    let mut s = snap_on_unmarked_key();
    s.live = LiveId::Known(n("work"));
    s.home_no_oauth = false;
    assert_eq!(s.adopted_platform(), None);
    assert!(s.decision_overflow().is_empty());
}

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 120,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn measured(session: f64, week: f64) -> Probed {
    let win = |used| {
        Some(Win {
            used,
            resets_at: FAR,
        })
    };
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows {
            session: win(session),
            week: win(week),
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
    }
}

fn exhausted() -> Probed {
    let win = |used| {
        Some(Win {
            used,
            resets_at: FAR,
        })
    };
    Probed {
        reading: Reading::Exhausted,
        windows: Windows {
            session: win(100.0),
            week: win(100.0),
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
    }
}

fn decide_on_unmarked(
    candidates: &[(&str, Probed)],
) -> crate::commands::profile::rotate::overflow::Verdict {
    let s = settings();
    let snap = snap_on_unmarked_key();
    let overflow = snap.decision_overflow();
    let cands: Vec<Measured> = candidates
        .iter()
        .map(|(c, p)| Measured::new(n(c), p))
        .collect();
    let live = n("api-key");
    decide_overflow(&OverflowInput {
        settings: &s,
        live_name: Some(&live),
        live: &Probed::platform(),
        candidates: &cands,
        overflow: &overflow,
        blocks: &BTreeMap::new(),
        platform_block: None,
        unmeasured_plan: &[],
        refused: &[],
        swapped_at: Some(NOW - 60),
        now: NOW,
    })
}

#[test]
fn the_rotator_leaves_an_unmarked_live_api_key_for_a_plan_login_with_room() {
    // 92/40 at --session-at 85: no capacity, but room. D6 says the rotator
    // leaves the unmarked key as soon as a plan login has room.
    let v = decide_on_unmarked(&[("work", measured(92.0, 40.0)), ("spare", exhausted())]);
    assert_eq!(
        v.decision,
        Decision::Swap {
            to: n("work"),
            reason: Reason::OverflowOut
        },
        "{v:?}"
    );
    assert_eq!(v.tag, Some(Tag::Out));
}

#[test]
fn with_no_plan_room_the_rotator_holds_the_unmarked_key_and_names_no_target() {
    let v = decide_on_unmarked(&[("work", exhausted()), ("spare", exhausted())]);
    assert!(
        !matches!(v.decision, Decision::Swap { .. }),
        "no target exists: {v:?}"
    );
}

#[test]
fn a_move_off_the_api_key_onto_credits_names_no_wait_gate_it_did_not_pass() {
    assert_eq!(
        per_token_off_key(&n("team-credits"), 2, CheckNote::Credit),
        "per-token team-credits: no plan login took the move; \
         leaving the API key, credits cost less; \
         marked overflow tier 2; probe ok, no spend limit"
    );
}
