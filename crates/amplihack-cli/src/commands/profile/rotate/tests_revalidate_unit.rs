//! Issue #1721: the pure parts of step 6's revalidation.
//!
//! Contract:
//! - `revalidate::changed(first: &Snap, fresh: &Snap, decision: &Decision,
//!   journal: bool) -> Vec<Changed>`, with `Changed::{Journal, Live, Pin,
//!   Overflow, Target(Option<Skip>), TargetIdentity}` and
//!   `Changed::label(&self) -> String`.
//!   `Live` covers `live`, `account`, `proto` and `home_no_oauth`; `Overflow`
//!   covers `overflow` and `ignored`; `Target` applies to a swap only, and
//!   only to its target. `TargetIdentity` is a swap's target that is still a
//!   candidate with another account in `candidate_accounts`, by
//!   `Account::same`. `candidates`, `skips`, `sees_sessions` and
//!   `candidate_accounts` of any other profile are not compared.
//! - `msg::changed(&[Changed])`, `msg::reconsidered(&ProfileName, &str)` and
//!   `msg::alert_no_capacity_for(live, reading, others)`.
//! - `Measurement::probed` holds the probes `decide_measured` decided on.

use super::decide::{Decision, Reading, Reason};
use super::identify::{Account, LiveId, Skip};
use super::log::msg;
use super::observe::{Snap, decide_measured, snapshot};
use super::pin::PinState;
use super::revalidate::{Changed, changed};
use super::tests_revalidate_cause::{ctx_with, env3, ok, probes_for, spent};
use super::tests_swap::n;
use crate::commands::profile::auto_switch::kit::{State, now};
use crate::commands::profile::overflow::OverflowEntry;

fn base() -> Snap {
    Snap {
        live: LiveId::Known(n("work")),
        proto: None,
        account: Some("u-work".into()),
        candidates: vec![n("spare"), n("spare2"), n("third")],
        skips: vec![(n("work"), Skip::Live)],
        sees_sessions: true,
        overflow: Vec::new(),
        pin: PinState::None,
        ignored: Vec::new(),
        home_no_oauth: false,
        candidate_accounts: ["spare", "spare2", "third"]
            .iter()
            .map(|c| (n(c), acct(&format!("u-{c}"), &format!("{c}@example.com"))))
            .collect(),
    }
}

/// An account with this uuid and e-mail.
fn acct(uuid: &str, email: &str) -> Option<Account> {
    let raw = serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email}});
    Account::from_claude_json(raw.to_string().as_bytes())
}

/// `s` with `name`'s account replaced by `account`.
fn with_account(mut s: Snap, name: &str, account: Option<Account>) -> Snap {
    for (c, a) in &mut s.candidate_accounts {
        if c == &n(name) {
            *a = account.clone();
        }
    }
    s
}

fn to_spare() -> Decision {
    Decision::Swap {
        to: n("spare"),
        reason: Reason::Threshold,
    }
}

fn all_decisions() -> [Decision; 3] {
    [to_spare(), Decision::Stay, Decision::Alert]
}

/// `fresh` with `name` moved from the candidates to the skips.
fn skipped(mut s: Snap, name: &str, why: Skip) -> Snap {
    s.candidates.retain(|c| c != &n(name));
    s.skips.push((n(name), why));
    s
}

// --- `changed` ---------------------------------------------------------------

#[test]
fn nothing_changed_is_an_empty_list_for_every_decision() {
    for d in all_decisions() {
        assert_eq!(changed(&base(), &base(), &d, false), [], "{d:?}");
    }
}

#[test]
fn a_journal_is_always_a_change() {
    for d in all_decisions() {
        assert_eq!(changed(&base(), &base(), &d, true), [Changed::Journal]);
    }
}

#[test]
fn every_live_field_is_a_live_change_for_every_decision() {
    let edits: [fn(&mut Snap); 4] = [
        |s| s.live = LiveId::Known(n("third")),
        |s| s.account = Some("u-other".into()),
        |s| s.proto = Some("prototype"),
        |s| s.home_no_oauth = true,
    ];
    for (i, edit) in edits.iter().enumerate() {
        let mut fresh = base();
        edit(&mut fresh);
        for d in all_decisions() {
            assert_eq!(
                changed(&base(), &fresh, &d, false),
                [Changed::Live],
                "edit {i}, {d:?}"
            );
        }
    }
}

#[test]
fn a_live_login_lost_to_an_unreadable_home_is_a_live_change() {
    let mut fresh = base();
    fresh.live = LiveId::HomeUnreadable(n("work"));
    assert!(changed(&base(), &fresh, &to_spare(), false).contains(&Changed::Live));
}

#[test]
fn a_pin_is_a_pin_change_for_every_decision() {
    let mut fresh = base();
    fresh.pin = PinState::Pinned(n("work"));
    for d in all_decisions() {
        assert_eq!(changed(&base(), &fresh, &d, false), [Changed::Pin]);
    }
}

#[test]
fn overflow_entries_and_ignored_names_are_overflow_changes() {
    let mut a = base();
    a.overflow.push(OverflowEntry::new(n("spare2"), 1));
    let mut b = base();
    b.ignored.push(n("third"));
    for fresh in [a, b] {
        for d in all_decisions() {
            assert_eq!(changed(&base(), &fresh, &d, false), [Changed::Overflow]);
        }
    }
}

#[test]
fn a_swap_target_that_is_skipped_names_its_reason() {
    for why in [
        Skip::Refused,
        Skip::NoLogin,
        Skip::Excluded,
        Skip::SessionRunning,
        Skip::Leased,
        Skip::Hosting,
        Skip::Parked,
        Skip::NoAccount,
        Skip::NotOAuth,
        Skip::Live,
    ] {
        let fresh = skipped(base(), "spare", why);
        assert_eq!(
            changed(&base(), &fresh, &to_spare(), false),
            [Changed::Target(Some(why))],
            "{why:?}"
        );
    }
}

#[test]
fn a_swap_target_in_neither_list_is_gone() {
    let mut fresh = base();
    fresh.candidates.retain(|c| c != &n("spare"));
    assert_eq!(
        changed(&base(), &fresh, &to_spare(), false),
        [Changed::Target(None)]
    );
}

#[test]
fn a_swap_target_with_another_account_is_a_target_identity_change() {
    let other = acct("u-intruder", "intruder@example.com");
    for (what, account) in [("another account", other), ("no account", None)] {
        let fresh = with_account(base(), "spare", account);
        assert_eq!(
            changed(&base(), &fresh, &to_spare(), false),
            [Changed::TargetIdentity],
            "{what}"
        );
    }
    let first = with_account(base(), "spare", None);
    assert_eq!(
        changed(&first, &base(), &to_spare(), false),
        [Changed::TargetIdentity],
        "an account where there was none"
    );
}

#[test]
fn the_same_account_read_again_is_not_a_change() {
    // `Account::same` decides: the uuid (and organisation), not the e-mail.
    let renamed = with_account(base(), "spare", acct("u-spare", "new@example.com"));
    assert_eq!(changed(&base(), &renamed, &to_spare(), false), []);
    // An API-key profile may have no account at either read.
    let none = with_account(base(), "spare", None);
    assert_eq!(changed(&none, &none, &to_spare(), false), []);
}

#[test]
fn a_target_that_left_the_candidates_is_not_also_an_identity_change() {
    let fresh = with_account(skipped(base(), "spare", Skip::NoAccount), "spare", None);
    assert_eq!(
        changed(&base(), &fresh, &to_spare(), false),
        [Changed::Target(Some(Skip::NoAccount))]
    );
}

#[test]
fn only_the_swap_target_account_is_compared() {
    let other = acct("u-intruder", "intruder@example.com");
    let fresh = with_account(base(), "spare2", other.clone());
    for d in all_decisions() {
        assert_eq!(changed(&base(), &fresh, &d, false), [], "{d:?}");
    }
    let target = with_account(base(), "spare", other);
    for d in [Decision::Stay, Decision::Alert] {
        assert_eq!(changed(&base(), &target, &d, false), [], "{d:?}");
    }
}

#[test]
fn the_snapshot_records_each_candidate_account() {
    let (e, _, _) = env3();
    let ctx = ctx_with(&e, |_, _| unreachable!());
    let state = State::new(ctx.store.root()).unwrap();
    let snap = snapshot(&ctx, &state).unwrap();
    let names: Vec<_> = snap
        .candidate_accounts
        .iter()
        .map(|(c, _)| c.clone())
        .collect();
    assert_eq!(names, snap.candidates, "one entry per candidate, in order");
    for (c, a) in &snap.candidate_accounts {
        let key = format!("u-{c}");
        assert!(a.as_ref().is_some_and(|a| a.matches_key(&key)), "{c}");
    }
}

#[test]
fn other_profiles_churn_is_never_a_change() {
    let mut added = base();
    added.candidates.push(n("newcomer"));
    let lost = skipped(base(), "spare2", Skip::NoLogin);
    let parked = skipped(base(), "third", Skip::Parked);
    let busy = skipped(
        skipped(base(), "spare2", Skip::SessionRunning),
        "third",
        Skip::Leased,
    );
    for fresh in [added, lost, parked, busy] {
        for d in all_decisions() {
            assert_eq!(changed(&base(), &fresh, &d, false), [], "{d:?}");
        }
    }
}

#[test]
fn losing_sight_of_sessions_alone_is_not_a_change() {
    let mut fresh = base();
    fresh.sees_sessions = false;
    for d in all_decisions() {
        assert_eq!(changed(&base(), &fresh, &d, false), [], "{d:?}");
    }
}

#[test]
fn a_stay_or_an_alert_ignores_the_would_be_target() {
    let fresh = skipped(base(), "spare", Skip::SessionRunning);
    for d in [Decision::Stay, Decision::Alert] {
        assert_eq!(changed(&base(), &fresh, &d, false), [], "{d:?}");
    }
}

#[test]
fn several_changes_are_all_reported() {
    let mut fresh = skipped(base(), "spare", Skip::Live);
    fresh.live = LiveId::Known(n("spare"));
    fresh.pin = PinState::Pinned(n("spare"));
    let got = changed(&base(), &fresh, &to_spare(), true);
    for want in [
        Changed::Journal,
        Changed::Live,
        Changed::Pin,
        Changed::Target(Some(Skip::Live)),
    ] {
        assert!(got.contains(&want), "{want:?} missing from {got:?}");
    }
}

// --- Labels and log lines ----------------------------------------------------

#[test]
fn labels_are_fixed_text_or_a_skip_reason() {
    assert_eq!(Changed::Journal.label(), "journal");
    assert_eq!(Changed::Live.label(), "live");
    assert_eq!(Changed::Pin.label(), "pin");
    assert_eq!(Changed::Overflow.label(), "overflow");
    assert_eq!(Changed::Target(None).label(), "target eligibility: gone");
    assert_eq!(Changed::TargetIdentity.label(), "target identity");
    for why in [
        Skip::SessionRunning,
        Skip::Excluded,
        Skip::NoLogin,
        Skip::NotOAuth,
    ] {
        assert_eq!(
            Changed::Target(Some(why)).label(),
            format!("target eligibility: {}", why.as_str())
        );
    }
}

#[test]
fn the_skip_line_lists_the_labels_after_the_old_text() {
    assert_eq!(msg::changed(&[]), msg::CHANGED);
    let line = msg::changed(&[Changed::Live, Changed::Target(Some(Skip::SessionRunning))]);
    assert_eq!(
        line,
        "skip: state changed during measurement (live, target eligibility: session running)"
    );
    assert!(line.starts_with(msg::CHANGED), "older readers still match");
    assert_eq!(
        msg::changed(&[Changed::Journal]),
        "skip: state changed during measurement (journal)"
    );
}

#[test]
fn the_reconsidered_line_is_exact() {
    assert_eq!(
        msg::reconsidered(&n("spare"), Skip::SessionRunning.as_str()),
        "revalidate: target spare no longer eligible (session running); reconsidered"
    );
    assert_eq!(
        msg::reconsidered(&n("spare"), "gone"),
        "revalidate: target spare no longer eligible (gone); reconsidered"
    );
    assert_eq!(
        msg::reconsidered(&n("spare"), "account changed"),
        "revalidate: target spare no longer eligible (account changed); reconsidered"
    );
}

// --- Test 11: the ALERT's candidate counts ----------------------------------

#[test]
fn the_no_capacity_alert_counts_failed_and_exhausted_candidates() {
    let live = Reading::Measured {
        session: 99.0,
        week: 50.0,
    };
    let old = "ALERT: no profile has capacity live=work s=99% w=50%";
    assert_eq!(msg::alert_no_capacity_for("work", &live, &[]), old);
    assert_eq!(msg::alert_no_capacity(&n("work"), &live), old);
    let others = [
        (n("a"), Reading::Failed),
        (n("b"), Reading::Failed),
        (n("c"), Reading::Exhausted),
    ];
    assert_eq!(
        msg::alert_no_capacity_for("work", &live, &others),
        format!("{old} (candidates: 2 failed, 1 exhausted)")
    );
}

#[test]
fn the_alert_has_no_suffix_when_every_candidate_measured() {
    let live = Reading::Exhausted;
    let others = [(
        n("a"),
        Reading::Measured {
            session: 95.0,
            week: 10.0,
        },
    )];
    let line = msg::alert_no_capacity_for("work", &live, &others);
    assert_eq!(line, msg::alert_no_capacity_for("work", &live, &[]));
    assert!(!line.contains("candidates:"), "{line}");
}

#[test]
fn the_alert_suffix_names_no_profile() {
    let others = [(n("secretname"), Reading::Failed)];
    let line = msg::alert_no_capacity_for("-", &Reading::Exhausted, &others);
    assert!(
        line.ends_with(" (candidates: 1 failed, 0 exhausted)"),
        "{line}"
    );
    assert!(!line.contains("secretname"), "{line}");
}

// --- `Measurement::probed` ---------------------------------------------------

#[test]
fn decide_measured_keeps_the_probes_it_decided_on() {
    let (e, _, _) = env3();
    let ctx = ctx_with(&e, |_, _| unreachable!());
    let state = State::new(ctx.store.root()).unwrap();
    let snap = snapshot(&ctx, &state).unwrap();
    let probes = probes_for(&snap, &[("spare", ok(0.0, 43.0)), ("third", spent())]);
    let m = decide_measured(&ctx, &snap, spent(), probes.clone(), now()).unwrap();
    assert_eq!(m.probed, Some((spent(), probes)));
}
