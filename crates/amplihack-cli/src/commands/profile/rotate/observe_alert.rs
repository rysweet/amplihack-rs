//! The ALERT for a live login that cannot be used (moved from `observe`).
//! Issue #2013 (D10): a refusal is written as one line with its kind, the
//! evidence from each source and the action that fixes it.

use super::super::refusal::{self, Cause};
use super::{LiveId, Snap, live_name, msg};

/// An ALERT for a live login that cannot be used.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveAlert {
    /// The ledger's `alert` key kind.
    pub kind: &'static str,
    pub line: String,
    /// Issue #2013 (D11): the alert-limit key of a refusal; `None` for an
    /// ALERT written once per ledger key.
    pub limit: Option<String>,
    /// The refusal reason, for the one-shot command's error.
    pub reason: Option<String>,
}

/// The ALERT for a live login that cannot be used, if any. Refusals use the
/// D10 line with the snapshot's evidence (issue #2013).
pub fn live_alert(snap: &Snap) -> Option<LiveAlert> {
    let live = live_name(&snap.live);
    let refuse = |kind: &'static str, cause: Cause, reason: String| LiveAlert {
        kind,
        line: refusal::line(cause, live, &reason, &snap.evidence),
        limit: Some(cause.key(live)),
        reason: Some(reason),
    };
    if let Some(reason) = &snap.identity_refusal {
        let kind = if reason.starts_with(super::super::swap_expect::ACCOUNT_MISMATCH) {
            "account_mismatch"
        } else {
            "trusted-identity"
        };
        return Some(refuse(kind, Cause::of(reason), reason.clone()));
    }
    if !snap.home_no_oauth && snap.trusted_live.is_none() && !matches!(snap.live, LiveId::NoLogin) {
        let text = snap.identity_cause.map_or_else(
            || super::super::login_identity::UNAVAILABLE.to_string(),
            super::super::login_identity::unavailable_line,
        );
        // A new cause is a new refusal, written at once: the fixed cause text
        // is part of its alert-limit key.
        let mut a = refuse("trusted-identity", Cause::IdentityUnavailable, text);
        if let (Some(key), Some(cause)) = (a.limit.as_mut(), snap.identity_cause) {
            key.push(':');
            key.push_str(cause);
        }
        return Some(a);
    }
    if let LiveId::HomeUnreadable(_) = &snap.live {
        let text = "~/.claude.json cannot be parsed as a JSON object".to_string();
        return Some(refuse("home-unreadable", Cause::HomeUnreadable, text));
    }
    live_alert_for(&snap.live, snap.home_no_oauth).map(|(kind, line)| LiveAlert {
        kind,
        line,
        limit: None,
        reason: None,
    })
}

/// [`live_alert`] for a live login `live`; `home_no_oauth` is
/// [`Snap::home_no_oauth`].
pub fn live_alert_for(live: &LiveId, home_no_oauth: bool) -> Option<(&'static str, String)> {
    match live {
        LiveId::NoLogin => Some(("no-login", msg::NO_LOGIN.to_string())),
        LiveId::HandChanged(n) => Some(("hand-change", msg::hand_change(n))),
        LiveId::HomeUnreadable(n) => Some(("home-unreadable", msg::home_unreadable(n))),
        LiveId::Ambiguous if home_no_oauth => {
            Some(("ambiguous", msg::AMBIGUOUS_PLATFORM.to_string()))
        }
        LiveId::Ambiguous => Some(("ambiguous", msg::AMBIGUOUS.to_string())),
        LiveId::Known(_) | LiveId::Adopt => None,
    }
}
