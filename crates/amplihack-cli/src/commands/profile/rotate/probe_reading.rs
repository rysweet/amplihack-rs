//! The rotator's reading of one probe: the `Probed` value, the fixed
//! warning lines it writes to the daemon's `rotate.err`, and (issue #2910)
//! the label of a probe that said nothing about its login's room.

use super::super::decide::Probed;
use crate::commands::profile::usage_cmd::{self, ErrorReason, Probe};
use crate::commands::profile::usage_direct::reason::Reason;
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source;
use std::io::Write;

/// The reading for `p`. A failed or timed-out probe counts as "swap needed",
/// so its cause goes to stderr (the daemon's `rotate.err`); the text is fixed
/// and never Claude's output. Both read `Failed`: never a target and never
/// exhausted, and probed again next cycle. Both are also unknown, which
/// holds a move onto overflow. A profile with no login (issue #2271) is not:
/// see [`reading_to`]. A profile directory refused just before the
/// spawn is no exception (issue #1698): that login may have capacity. From
/// the next cycle the profile is skipped as `directory refused` before any
/// probe, and that skip holds the move until the directory is fixed.
pub(in crate::commands::profile::rotate) fn reading(who: &str, p: &Probe) -> Probed {
    reading_to(who, p, &mut std::io::stderr())
}

/// [`reading`] with its warnings written to `err`; a failed write panics, as
/// `eprintln!` does. A profile with no login (`NotSignedIn`, issue #2271)
/// reads what the old `claude` fallback read, a known `not_logged_in`:
/// `Failed`, so it is never chosen, but not unknown, so it does not hold the
/// move onto overflow. It writes no warning.
pub(super) fn reading_to(who: &str, p: &Probe, err: &mut dyn Write) -> Probed {
    const ERR: &str = "failed printing to stderr";
    // SR-10: logged whether or not the row fell back to `claude`.
    if p.direct_failure.is_some_and(usage_source::not_saved) {
        writeln!(err, "{}", usage_cmd::not_saved_warning(who)).expect(ERR);
    }
    if matches!(p.error, Some(ErrorReason::NotSignedIn(_))) {
        return Probed::from_report(&p.report);
    }
    if p.error.is_some() || p.report.state == UsageState::Error {
        let why = p
            .error
            .map_or_else(|| "unreadable /usage output".to_string(), |e| e.to_string());
        writeln!(err, "warning: rotate: probe of {who} failed: {why}").expect(ERR);
        return Probed::failed();
    }
    if p.report.state == UsageState::Timeout {
        writeln!(err, "warning: rotate: probe of {who} timed out").expect(ERR);
    }
    Probed::from_report(&p.report)
}

/// Issue #2910 (F7): why a probe said nothing about its login's room. Such a
/// login is *unknown*, never exhausted, so the rotator may use its last good
/// reading. The declaration order is the order the no-target line prints the
/// labels in (D9). Each label is fixed text, never a server's or a
/// credentials file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProbeLabel {
    /// The usage endpoint answered HTTP 429.
    Http429,
    /// The request failed in transport.
    Transport,
    /// The login's trusted identity could not be read, so it was not probed.
    IdentityUnavailable,
    /// The read ran out of time.
    Timeout,
    /// The read had not finished when the cycle stopped waiting for it.
    StillRunning,
    /// Another failure of the probe itself that says nothing about the
    /// login: a server error, an unreadable answer, `claude` not starting.
    ProbeError,
}

impl ProbeLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http429 => "HTTP 429",
            Self::Transport => "transport",
            Self::IdentityUnavailable => "identity unavailable",
            Self::Timeout => "timeout",
            Self::StillRunning => "still running",
            Self::ProbeError => "probe error",
        }
    }
}

/// The label of a direct read's failure reason (issue #2910, S2). A reason
/// about the login's account (an expired or refused token, a refresh that
/// failed, no credentials, a plain 4xx, credits billing) gives `None`, so
/// such a login is never chosen on an old reading. So does a text that is
/// not a [`Reason`]: the table fails closed.
///
/// The `match` names every [`Reason`] and has no wildcard, so a reason added
/// to `usage_direct::reason` does not compile until it is classified here.
fn direct_label(why: &str) -> Option<ProbeLabel> {
    match Reason::parse(why)? {
        Reason::Http429 => Some(ProbeLabel::Http429),
        Reason::Transport => Some(ProbeLabel::Transport),
        Reason::Timeout => Some(ProbeLabel::Timeout),
        Reason::Http3xx | Reason::Http5xx | Reason::Unparseable | Reason::RefreshBusy => {
            Some(ProbeLabel::ProbeError)
        }
        Reason::NoFile
        | Reason::Refused
        | Reason::NoToken
        | Reason::Expired
        | Reason::CreditsOn
        | Reason::RefreshRefused
        | Reason::RefreshFailed
        | Reason::RefreshNotSaved
        | Reason::NoRefreshToken
        | Reason::UnreadableCreds
        | Reason::Http401
        | Reason::Http403
        | Reason::Http4xx => None,
    }
}

/// The label of a failed probe; `None` for a probe that read its login, for
/// a login that is not signed in, excluded or whose directory was refused,
/// and for a failure that says something about the login's account (issue
/// #2910, F7, S2).
///
/// A direct read's reason decides, whether it is the probe's own error or
/// the cause of a fallback (`direct_failure`) that then failed too: the
/// fallback's own error (`claude not found`) hides the cause, and a cause
/// about the login's account gives no label whatever followed it. A fallback
/// that then timed out is a timeout. Without a direct reason, a probe that
/// could not start, read or wait for `claude` is a probe error, and one that
/// timed out is a timeout.
pub fn label_of(p: &Probe) -> Option<ProbeLabel> {
    let read = !matches!(p.report.state, UsageState::Error | UsageState::Timeout);
    if read && p.error.is_none() {
        return None;
    }
    if let Some(why) = p.direct_failure {
        let cause = direct_label(why)?;
        let timed_out = p.error.is_none() && p.report.state == UsageState::Timeout;
        return Some(if timed_out {
            ProbeLabel::Timeout
        } else {
            cause
        });
    }
    match p.error {
        Some(ErrorReason::Direct(why)) => direct_label(why),
        Some(
            ErrorReason::StartFailed
            | ErrorReason::ReadFailed
            | ErrorReason::WaitFailed
            | ErrorReason::NoClaude,
        ) => Some(ProbeLabel::ProbeError),
        Some(ErrorReason::NotSignedIn(_) | ErrorReason::Excluded | ErrorReason::ProfileRefused) => {
            None
        }
        None if p.report.state == UsageState::Timeout => Some(ProbeLabel::Timeout),
        None => None,
    }
}

#[cfg(test)]
#[path = "tests_issue_2910_label.rs"]
mod tests_issue_2910_label;
