//! The rotator's reading of one probe: the `Probed` value and the fixed
//! warning lines it writes to the daemon's `rotate.err`.

use super::super::decide::Probed;
use crate::commands::profile::usage_cmd::{self, ErrorReason, Probe};
use crate::commands::profile::usage_parse::UsageState;
use crate::commands::profile::usage_source;
use std::io::Write;

/// The reading for `p`. A failed or timed-out probe counts as "swap needed",
/// so its cause goes to stderr (the daemon's `rotate.err`); the text is fixed
/// and never Claude's output. Both read `Failed`: never a target and never
/// exhausted, and probed again next cycle. Both are also unknown, which
/// blocks a move onto overflow. A profile directory refused just before the
/// spawn is no exception (issue #1698): that login may have capacity. From
/// the next cycle the profile is skipped as `directory refused` before any
/// probe, and that skip holds the move until the directory is fixed.
pub(in crate::commands::profile::rotate) fn reading(who: &str, p: &Probe) -> Probed {
    reading_to(who, p, &mut std::io::stderr())
}

/// [`reading`] with its warnings written to `err`; a failed write panics, as
/// `eprintln!` does. A profile with no login (`NotSignedIn`, issue #2271)
/// reads `Failed`, so it is never chosen, but writes no warning.
pub(super) fn reading_to(who: &str, p: &Probe, err: &mut dyn Write) -> Probed {
    const ERR: &str = "failed printing to stderr";
    // SR-10: logged whether or not the row fell back to `claude`.
    if p.direct_failure.is_some_and(usage_source::not_saved) {
        writeln!(err, "{}", usage_cmd::not_saved_warning(who)).expect(ERR);
    }
    if matches!(p.error, Some(ErrorReason::NotSignedIn(_))) {
        return Probed::failed();
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
