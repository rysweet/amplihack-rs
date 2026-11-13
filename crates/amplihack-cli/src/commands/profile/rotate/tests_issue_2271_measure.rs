//! Issue #2271 (A3): the rotator's reading of a profile that is not signed
//! in.
//!
//! Contract (`rotate::measure`, with the reading in `probe_reading.rs`):
//! - `reading_to(who: &str, p: &Probe, err: &mut dyn Write) -> Probed`
//!   is `pub(super)` and does what `reading` does, writing its warning lines
//!   to `err`. `reading(who, p)` is `reading_to(who, p, &mut io::stderr())`,
//!   so the daemon's `rotate.err` is unchanged for every other probe.
//! - A probe whose error is `ErrorReason::NotSignedIn(_)` (built by
//!   `Probe::not_signed_in`) reads a known `not_logged_in`, as the `claude`
//!   fallback did before: `Failed`, not unknown, no windows, `headroom()` of
//!   `None`. The rotator therefore never chooses it, never counts it as
//!   exhausted, reads it again next cycle, and does not let it hold the move
//!   onto overflow (`tests_overflow.rs`). Only a failed or timed-out probe is
//!   unknown.
//! - It writes nothing to `err`: no `warning: rotate: probe of <who> failed`
//!   line, since a profile with no login to use is not a failed probe.
//! - Every other failure still writes its line: an error row writes
//!   `warning: rotate: probe of <who> failed: <why>` and a timeout writes
//!   `warning: rotate: probe of <who> timed out`.
//!
//! The probes are built in memory: no `claude`, no network and no
//! credentials file.

use super::super::decide::{Probed, Reading, Windows};
use super::probe_reading::reading_to;
use crate::commands::profile::usage_cmd::{ErrorReason, Probe};
use crate::commands::profile::usage_direct::reason;
use crate::commands::profile::usage_parse::{UsageReport, UsageState};
use crate::commands::profile::usage_source::Spend;

/// The three reasons `usage_source` treats as "not signed in".
const SIGNED_OUT: [&str; 3] = [reason::NO_TOKEN, reason::NO_REFRESH_TOKEN, reason::NO_FILE];

fn read(who: &str, p: &Probe) -> (Probed, String) {
    let mut err = Vec::new();
    let probed = reading_to(who, p, &mut err);
    (probed, String::from_utf8(err).expect("warnings are UTF-8"))
}

fn errored(e: ErrorReason) -> Probe {
    Probe {
        report: UsageReport::empty(UsageState::Error),
        error: Some(e),
        raw: None,
        source: None,
        direct_failure: None,
        spend: Spend::Unknown,
        extra_shape: None,
    }
}

fn timed_out() -> Probe {
    Probe {
        error: None,
        report: UsageReport::empty(UsageState::Timeout),
        ..errored(ErrorReason::ReadFailed)
    }
}

#[test]
fn a_not_signed_in_probe_reads_failed_with_no_headroom() {
    for why in SIGNED_OUT {
        let (p, _) = read("nosign", &Probe::not_signed_in(why));
        assert_eq!(p.reading, Reading::Failed, "{why}");
        assert!(!p.unknown, "{why}: a known not_logged_in, not unknown");
        let before = Probed::from_report(&UsageReport::empty(UsageState::NotLoggedIn));
        assert_eq!(p, before, "{why}: what the claude fallback read");
        assert_eq!(p.reading.headroom(), None, "{why}: never a target");
        assert_eq!(p.windows, Windows::default(), "{why}");
        assert!(!p.spend_limit, "{why}");
    }
}

#[test]
fn a_not_signed_in_probe_writes_no_warning() {
    for why in SIGNED_OUT {
        let (_, err) = read("nosign", &Probe::not_signed_in(why));
        assert_eq!(err, "", "{why}: nothing for rotate.err");
    }
}

#[test]
fn a_failed_probe_still_writes_its_warning() {
    for e in [
        ErrorReason::ReadFailed,
        ErrorReason::StartFailed,
        ErrorReason::Direct(reason::REFRESH_FAILED),
        ErrorReason::Direct(reason::EXPIRED),
    ] {
        let (p, err) = read("broken", &errored(e));
        assert_eq!(p, Probed::failed(), "{e:?}");
        let want = format!("warning: rotate: probe of broken failed: {e}\n");
        assert_eq!(err, want, "{e:?}");
    }
}

#[test]
fn a_timed_out_probe_still_writes_its_warning() {
    let (p, err) = read("slow", &timed_out());
    assert_eq!(p.reading, Reading::Failed);
    assert!(p.unknown, "a timed-out probe stays unknown");
    assert_eq!(err, "warning: rotate: probe of slow timed out\n");
}

/// The warning is removed only for `NotSignedIn`; a direct error carrying
/// the same reason string (Direct mode before #2271) is not exempt.
#[test]
fn only_the_not_signed_in_error_is_quiet() {
    let (_, err) = read("plain", &errored(ErrorReason::Direct(reason::NO_TOKEN)));
    assert!(
        err.starts_with("warning: rotate: probe of plain failed: "),
        "{err:?}"
    );
}

/// A credentials file that could not be read is a failure, not "not signed
/// in": it writes its warning and stays unknown, so it holds overflow.
#[test]
fn an_unreadable_credentials_file_warns_and_is_unknown() {
    let (p, err) = read("odd", &Probe::direct_error(reason::UNREADABLE_CREDS));
    assert_eq!(p, Probed::failed());
    assert!(p.unknown);
    assert_eq!(
        err,
        "warning: rotate: probe of odd failed: direct read: unreadable credentials\n"
    );
}

/// The quiet row never holds a path, token or server text: the only text
/// it could carry is the fixed reason.
#[test]
fn a_not_signed_in_probe_carries_only_the_fixed_reason() {
    for why in SIGNED_OUT {
        let p = Probe::not_signed_in(why);
        assert_eq!(p.error, Some(ErrorReason::NotSignedIn(why)));
        assert_eq!(p.direct_failure, Some(why));
        assert_eq!(p.report.state, UsageState::NotLoggedIn);
    }
}
