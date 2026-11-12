//! ALERT de-duplication for the rotate cycle: the key an ALERT is stored
//! under in `ledger.alert`, whether a line is written or only printed, and
//! recording a cycle that ends on an ALERT. An advisory ALERT (#1907) is
//! stored in `ledger.advisory_alert` instead, which does not block a resume.

use super::cycle::{Ctx, CycleOutcome, save};
use super::ledger::{RotateLedger, safe_account};
use super::log::Sink;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;

/// The ALERT kind for "no profile has capacity". The only kind that may be
/// advisory (issue #1907, `capacity::Room::alert`).
pub const NO_CAPACITY: &str = "no-capacity";

pub fn alert_key(kind: &str, live: Option<&ProfileName>) -> String {
    format!("{kind}:{}", live.map_or("-", ProfileName::as_str))
}

/// Whether an ALERT with `key` is to be written: only when neither alert
/// field holds it already. Stores `key` in `alert` and clears
/// `advisory_alert`, so a key that turns hard is not logged twice.
pub fn should_alert(l: &mut RotateLedger, key: &str) -> bool {
    let new = !holds(l, key);
    l.alert = Some(key.to_string());
    l.advisory_alert = None;
    new
}

/// [`should_alert`] for an advisory ALERT (issue #1907): stores `key` in
/// `advisory_alert` and clears `alert`, which would block a resume.
pub fn should_alert_advisory(l: &mut RotateLedger, key: &str) -> bool {
    let new = !holds(l, key);
    l.advisory_alert = Some(key.to_string());
    l.alert = None;
    new
}

fn holds(l: &RotateLedger, key: &str) -> bool {
    l.alert.as_deref() == Some(key) || l.advisory_alert.as_deref() == Some(key)
}

/// Clears both alert fields.
pub fn clear_alert(l: &mut RotateLedger) {
    l.alert = None;
    l.advisory_alert = None;
}

/// Whether a loaded ledger's alert fields are acceptable: each a safe
/// account-like value, and never both set.
pub fn valid_alerts(l: &RotateLedger) -> bool {
    let ok = |v: Option<&str>| v.is_none_or(safe_account);
    ok(l.alert.as_deref())
        && ok(l.advisory_alert.as_deref())
        && !(l.alert.is_some() && l.advisory_alert.is_some())
}

/// Logs the ALERT only when its key changed; the one-shot command still
/// prints it every time, so a repeated run does not look like success.
pub fn alert_once(out: &mut Sink, l: &mut RotateLedger, key: &str, t: i64, line: &str) {
    if should_alert(l, key) {
        out.emit(t, line);
    } else {
        out.print(t, line);
    }
}

/// Writes an ALERT once per key and records the cycle as alerted.
pub(super) fn alert(
    ctx: &Ctx,
    out: &mut Sink,
    kind: &str,
    live: Option<&ProfileName>,
    line: &str,
) -> Result<(), ProfileError> {
    let t = now();
    let mut l = ctx.rs.load_ledger()?.unwrap_or_default();
    if kind == "trusted-identity"
        || kind == "account_mismatch"
        || kind == super::seed::SAME_ACCOUNT
        || line.contains("trusted login identity")
    {
        // Always written, unlike `alert_once`, but stored as `should_alert`
        // stores it: `advisory_alert` is cleared too, since a ledger with
        // both fields set fails `valid_alerts` and stops every later load.
        should_alert(&mut l, &alert_key(kind, live));
        out.emit(t, line);
    } else {
        alert_once(out, &mut l, &alert_key(kind, live), t, line);
    }
    save(ctx, &mut l, CycleOutcome::Alerted, t, None, None)
}

#[cfg(test)]
#[path = "tests_identity_after_advisory.rs"]
mod tests_identity_after_advisory;
