//! ALERT de-duplication for the rotate cycle: the key an ALERT is stored
//! under in `ledger.alert`, and whether a line is written or only printed.

use super::ledger::RotateLedger;
use super::log::Sink;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;

pub fn alert_key(kind: &str, live: Option<&ProfileName>) -> String {
    format!("{kind}:{}", live.map_or("-", ProfileName::as_str))
}

/// Whether an ALERT with `key` is to be written: only when the stored key
/// differs. Stores `key`.
pub fn should_alert(l: &mut RotateLedger, key: &str) -> bool {
    if l.alert.as_deref() == Some(key) {
        return false;
    }
    l.alert = Some(key.to_string());
    true
}

pub fn clear_alert(l: &mut RotateLedger) {
    l.alert = None;
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

/// The ALERT key kind for a refused swap: its reason, without control
/// characters and short enough that the key passes the ledger's 320-byte
/// check with any profile name.
pub fn refusal_kind(e: &ProfileError) -> String {
    let reason: String = e.to_string().chars().filter(|c| !c.is_control()).collect();
    let mut kind = String::from("refused ");
    for c in reason.chars() {
        if kind.len() + c.len_utf8() > 200 {
            break;
        }
        kind.push(c);
    }
    kind
}
