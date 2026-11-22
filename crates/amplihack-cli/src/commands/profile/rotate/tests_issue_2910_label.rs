//! Issue #2910 (F7, A1, S2, R2): which failed probes make a login *unknown*,
//! and the fixed label each one shows. Wired from `probe_reading.rs`.
//!
//! A probe that is rate limited (HTTP 429), fails in transport, cannot read
//! the identity, times out or has not finished is unknown: it says nothing
//! about the login's room, so the rotator may use the login's last good
//! reading. A probe that says something about the login's account (token
//! expired, refused, signed out, an unreadable credentials file, a plain 4xx,
//! credits billing) is not unknown. It gets no label, so such a login can
//! never be chosen on an old reading.
//!
//! The label table is closed and fails closed: a text that is not a reason
//! gives no label. The reasons are one enum, `usage_direct::reason::Reason`,
//! declared in one list with their text constants, and `direct_label`
//! matches on it with no wildcard, so the compiler refuses a reason added
//! without a classification. The table below pins what each one is.
//!
//! API these tests assume (in `probe_reading.rs`):
//!
//! ```text
//! enum ProbeLabel { Http429, Transport, IdentityUnavailable, Timeout,
//!                   StillRunning, ProbeError }   // Copy, Eq, Ord, Hash, Debug
//! ProbeLabel::as_str(self) -> &'static str
//! label_of(p: &Probe) -> Option<ProbeLabel>
//! ```
//!
//! The declaration order of the variants is the order the summary line
//! prints them in (D9). No test reads a login, starts `claude` or uses the
//! network: every probe is built in memory.

use super::*;
use crate::commands::profile::usage_cmd::{ErrorReason, Probe};
use crate::commands::profile::usage_direct::reason::{self, Reason};
use crate::commands::profile::usage_parse::{UsageReport, UsageState};
use crate::commands::profile::usage_source::Spend;
use std::collections::BTreeSet;

fn probe(state: UsageState, error: Option<ErrorReason>, direct: Option<&'static str>) -> Probe {
    Probe {
        report: UsageReport::empty(state),
        error,
        raw: None,
        source: None,
        direct_failure: direct,
        spend: Spend::Unknown,
        extra_shape: None,
    }
}

/// The direct read failed with `why` and no fallback ran.
fn direct_error(why: &'static str) -> Probe {
    probe(UsageState::Error, Some(ErrorReason::Direct(why)), None)
}

/// The direct read failed with `why`, then the fallback found no `claude`.
fn fell_back(why: &'static str) -> Probe {
    probe(UsageState::Error, Some(ErrorReason::NoClaude), Some(why))
}

use ProbeLabel::{Http429, IdentityUnavailable, ProbeError, StillRunning, Timeout, Transport};

/// One row for every `Reason`: the label, or `None` for a reason that is not
/// a transient failure of the probe itself.
const TABLE: &[(Reason, Option<ProbeLabel>)] = &[
    (Reason::NoFile, None),
    (Reason::Refused, None),
    (Reason::NoToken, None),
    (Reason::Expired, None),
    (Reason::Timeout, Some(Timeout)),
    (Reason::Transport, Some(Transport)),
    (Reason::Unparseable, Some(ProbeError)),
    (Reason::CreditsOn, None),
    (Reason::RefreshBusy, Some(ProbeError)),
    (Reason::RefreshRefused, None),
    (Reason::RefreshFailed, None),
    (Reason::RefreshNotSaved, None),
    (Reason::NoRefreshToken, None),
    (Reason::UnreadableCreds, None),
    (Reason::Http401, None),
    (Reason::Http403, None),
    (Reason::Http429, Some(Http429)),
    (Reason::Http3xx, Some(ProbeError)),
    (Reason::Http4xx, None),
    (Reason::Http5xx, Some(ProbeError)),
];

// --- The closed table ----------------------------------------------------------

/// `Reason::ALL` comes from the same list as the enum, so this holds the
/// table above to every reason and to nothing else.
#[test]
fn the_table_has_one_row_for_every_reason() {
    for r in Reason::ALL {
        let rows = TABLE.iter().filter(|(t, _)| t == r).count();
        assert_eq!(rows, 1, "reason {r:?} needs exactly one row: classify it");
    }
    assert_eq!(TABLE.len(), Reason::ALL.len());
}

/// What a direct read reports is the text: each one names its reason and no
/// other, and the constant the reader writes is that text.
#[test]
fn each_reason_text_reads_back_as_that_reason() {
    for r in Reason::ALL {
        assert_eq!(Reason::parse(r.as_str()), Some(*r), "{r:?}");
    }
    assert_eq!(reason::HTTP_429, Reason::Http429.as_str());
    assert_eq!(reason::NO_FILE, "no credentials file");
    assert_eq!(reason::UNREADABLE_CREDS, "unreadable credentials");
    assert_eq!(Reason::parse("HTTP 429"), None, "the label is not a reason");
    assert_eq!(Reason::parse(""), None);
}

#[test]
fn a_direct_read_failing_with_each_reason_gets_the_label_in_the_table() {
    for (r, want) in TABLE {
        let why = r.as_str();
        assert_eq!(label_of(&direct_error(why)), *want, "direct read: {why}");
    }
}

#[test]
fn a_direct_failure_that_led_to_a_fallback_gets_the_same_label() {
    // The probe's own error is `claude not found`, which alone is a probe
    // error; the direct read's reason is the cause and decides the label.
    for (r, want) in TABLE {
        let why = r.as_str();
        assert_eq!(label_of(&fell_back(why)), *want, "fallback after: {why}");
    }
}

#[test]
fn a_reason_the_table_does_not_name_gets_no_label() {
    assert_eq!(label_of(&direct_error("a reason added next year")), None);
    assert_eq!(label_of(&fell_back("a reason added next year")), None);
    assert_eq!(label_of(&direct_error("")), None);
}

// --- Errors that are not a direct read -----------------------------------------------

#[test]
fn the_probe_errors_that_name_no_login_are_probe_errors() {
    for e in [
        ErrorReason::StartFailed,
        ErrorReason::ReadFailed,
        ErrorReason::WaitFailed,
        ErrorReason::NoClaude,
    ] {
        let p = probe(UsageState::Error, Some(e), None);
        assert_eq!(label_of(&p), Some(ProbeError), "{e:?}");
    }
}

#[test]
fn a_refused_profile_directory_a_signed_out_login_and_an_excluded_login_get_no_label() {
    let refused = probe(UsageState::Error, Some(ErrorReason::ProfileRefused), None);
    assert_eq!(label_of(&refused), None);
    for why in [reason::NO_TOKEN, reason::NO_REFRESH_TOKEN, reason::NO_FILE] {
        let p = probe(
            UsageState::NotLoggedIn,
            Some(ErrorReason::NotSignedIn(why)),
            None,
        );
        assert_eq!(label_of(&p), None, "not signed in ({why})");
    }
    let excluded = probe(UsageState::Error, Some(ErrorReason::Excluded), None);
    assert_eq!(label_of(&excluded), None);
}

#[test]
fn a_credential_failure_behind_a_fallback_error_is_never_labelled() {
    // The direct read found an expired token; the fallback then failed too.
    // The login needs signing in again, which is not a transient failure.
    for why in [
        reason::EXPIRED,
        reason::HTTP_401,
        reason::HTTP_403,
        reason::REFUSED,
        reason::REFRESH_REFUSED,
        reason::REFRESH_FAILED,
        reason::REFRESH_NOT_SAVED,
        reason::UNREADABLE_CREDS,
        reason::NO_FILE,
        reason::NO_TOKEN,
        reason::NO_REFRESH_TOKEN,
    ] {
        assert_eq!(label_of(&fell_back(why)), None, "{why}");
        assert_eq!(label_of(&direct_error(why)), None, "{why}");
    }
}

#[test]
fn a_fallback_with_no_direct_cause_is_a_probe_error() {
    for e in [ErrorReason::NoClaude, ErrorReason::StartFailed] {
        assert_eq!(
            label_of(&probe(UsageState::Error, Some(e), None)),
            Some(ProbeError)
        );
    }
}

// --- Probes that did not fail with an error --------------------------------------------

#[test]
fn a_probe_that_timed_out_without_an_error_is_a_timeout() {
    assert_eq!(
        label_of(&probe(UsageState::Timeout, None, None)),
        Some(Timeout)
    );
}

#[test]
fn a_probe_that_read_the_login_gets_no_label() {
    for state in [
        UsageState::Ok,
        UsageState::SessionExhausted,
        UsageState::WeeklyExhausted,
        UsageState::SpendLimit,
    ] {
        assert_eq!(label_of(&probe(state, None, None)), None, "{state:?}");
    }
    // A direct failure the fallback recovered from is not a failure now.
    let recovered = probe(UsageState::Ok, None, Some(reason::HTTP_429));
    assert_eq!(label_of(&recovered), None);
}

#[test]
fn an_error_state_with_no_stated_cause_gets_no_label() {
    // `reading_to` writes `unreadable /usage output` for it. Nothing says it
    // is transient, so the table does not label it.
    assert_eq!(label_of(&probe(UsageState::Error, None, None)), None);
}

// --- The labels ------------------------------------------------------------------------------

#[test]
fn each_label_is_fixed_text() {
    assert_eq!(Http429.as_str(), "HTTP 429");
    assert_eq!(Transport.as_str(), "transport");
    assert_eq!(IdentityUnavailable.as_str(), "identity unavailable");
    assert_eq!(Timeout.as_str(), "timeout");
    assert_eq!(StillRunning.as_str(), "still running");
    assert_eq!(ProbeError.as_str(), "probe error");
}

/// The summary prints one part per label in this order (D9).
#[test]
fn the_labels_sort_in_the_order_the_summary_prints_them() {
    let set: BTreeSet<ProbeLabel> = [
        ProbeError,
        StillRunning,
        Timeout,
        IdentityUnavailable,
        Transport,
        Http429,
    ]
    .into_iter()
    .collect();
    let order: Vec<&str> = set.iter().map(|l| l.as_str()).collect();
    assert_eq!(
        order,
        [
            "HTTP 429",
            "transport",
            "identity unavailable",
            "timeout",
            "still running",
            "probe error"
        ]
    );
}

#[test]
fn a_label_is_plain_data() {
    // Copy, Eq and Hash: the cycle keeps labels in side lists and counts them.
    let a = Http429;
    let b = a;
    assert_eq!(a, b);
    let mut h = std::collections::HashSet::new();
    h.insert(a);
    assert!(h.contains(&b));
}

/// The rotate log must never carry text a server or a credentials file chose.
#[test]
fn no_label_carries_anything_but_its_own_fixed_text() {
    for l in [
        Http429,
        Transport,
        IdentityUnavailable,
        Timeout,
        StillRunning,
        ProbeError,
    ] {
        let t = l.as_str();
        assert!(t.is_ascii() && !t.contains('\n') && t.len() <= 24, "{t:?}");
    }
}

/// A direct read that failed for a transient cause, then a fallback that ran
/// out of time: the probe timed out, so its label is the timeout. A cause
/// about the login's account still gives no label, whatever followed it.
#[test]
fn a_fallback_that_timed_out_after_a_transient_direct_failure_is_a_timeout() {
    for why in [reason::UNPARSEABLE, reason::HTTP_5XX, reason::HTTP_429] {
        let p = probe(UsageState::Timeout, None, Some(why));
        assert_eq!(label_of(&p), Some(Timeout), "after {why}");
    }
    for why in [reason::EXPIRED, reason::HTTP_401, reason::NO_TOKEN] {
        let p = probe(UsageState::Timeout, None, Some(why));
        assert_eq!(label_of(&p), None, "after {why}");
    }
}
