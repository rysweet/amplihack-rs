//! The lines of target choice (issue #2910, D9 to D11): the ALERT a cycle
//! with no move writes (the no-target line, in one of two wordings, or a stay
//! note), the `reject:` line, the unconfirmed suffix of a swap, and the
//! reason a login carries on the `cycle` line. See "Rotate no target" and
//! "Rotate confirmation" in docs/reference/profile-command.md.
//!
//! Every text is fixed or built from a profile name, a whole number, a time
//! to the minute or a label this binary owns. None carries what a server, a
//! credentials file or a stored reason chose.

use super::super::alert::NO_CAPACITY;
use super::super::fresh::MAX_REREADS_PER_CYCLE;
use super::super::identify::LiveId;
use super::super::log::msg::pct;
use super::super::log::{Shown, msg};
use super::super::measure::ProbeLabel;
use super::super::observe::{Snap, live_name};
use super::super::overflow::Tag;
use super::super::settings::Settings;
use super::Reading;
use super::target::{NoMove, Unfit};
use super::target_confirm::Why;
use super::target_seen::{Basis, Seen};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// The ALERT key kind of the no-target line when no login has capacity.
pub const KEY_NO_CAPACITY: &str = NO_CAPACITY;
/// The key kind of the no-target line in its `no capacity known` wording.
pub const KEY_KNOWN: &str = "no-capacity/known";
/// The key kinds of the stay notes.
pub const KEY_STAY: &str = "no-capacity/stay";
pub const KEY_HELD: &str = "no-capacity/held";
pub const KEY_UNCONFIRMED: &str = "no-capacity/unconfirmed";

/// A time no calendar can show.
const NO_TIME: &str = "an unknown time";

/// One ALERT-branch line: the ALERT kind (which decides whether it may be
/// advisory), the kind its key is made from, the text, and whether it is a
/// stay note (printed as `would stay (...)` by `--dry-run`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: &'static str,
    pub key_kind: &'static str,
    pub text: String,
    pub stay: bool,
}

/// `t` as UTC to the minute, `%Y-%m-%dT%H:%MZ`, with the seconds dropped; one
/// fixed text for a time outside the calendar's range.
pub fn utc_minute(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(
        || NO_TIME.to_string(),
        |d| d.format("%Y-%m-%dT%H:%MZ").to_string(),
    )
}

fn minutes(age: u64) -> u64 {
    age / 60
}

/// `reject: <name> <reason>` (D11).
pub fn reject(who: &ProfileName, why: &Why, week_max: u8) -> String {
    let reason = match why {
        Why::Unfit(Unfit::Full) => "no room on a fresh reading".to_string(),
        Why::Unfit(Unfit::AboveCeiling) => {
            format!("week above ceiling {week_max}% on a fresh reading")
        }
        Why::Unfit(Unfit::SpendLimit) => "spend limit on a fresh reading".to_string(),
        Why::Unfit(Unfit::NoReading) => "no usable reading".to_string(),
        Why::Unknown(label) => format!("reading unknown ({})", label.as_str()),
    };
    format!("reject: {who} {reason}")
}

/// The suffix of a swap made without a fresh reading of its target (D15).
pub fn unconfirmed(label: ProbeLabel) -> String {
    format!(" unconfirmed ({})", label.as_str())
}

/// The stay note for a cycle that did not move although the live login has
/// room (D10); `None` for [`NoMove::NoTarget`], which is the no-target line.
pub fn stay_note(m: &NoMove, s: &Settings) -> Option<String> {
    match m {
        NoMove::NoTarget => None,
        NoMove::LessRoom { most, room, needs } => {
            // A figure the cycle did not measure is said in words, not as 0.
            let room = room.map_or_else(|| "unknown".to_string(), |r| r.to_string());
            let best = most.map_or_else(
                || "only a login in another organisation has room".to_string(),
                |m| format!("best other login has room {m}"),
            );
            Some(format!(
                "stay: live room {room}, {best}, a move needs {needs}"
            ))
        }
        NoMove::Held { to, session, since } => Some(format!(
            "stay: held after last swap ({since}s of {}s): {to} is at s={}%, at or above --session-at",
            s.hold,
            pct(*session)
        )),
        NoMove::Unconfirmed => Some("stay: no candidate confirmed by a fresh reading".to_string()),
    }
}

/// The counts of the no-target line (D9) and whether every login counted is
/// known to have no room (a window at 100%, a spend limit or a block).
fn counts(seen: &Seen) -> (String, bool) {
    let mut unknown: BTreeMap<ProbeLabel, usize> = BTreeMap::new();
    // Issue #2910: plan logins the cycle did not read, by the reason its own
    // `skip:` line gives. Any of these makes the stronger wording untrue.
    let mut not_read: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut failed, mut blocked, mut exhausted, mut above, mut not_tried) = (0, 0, 0, 0, 0);
    let mut earliest: Option<i64> = None;
    let mut other = 0;
    for (name, st) in &seen.logins {
        if seen.not_tried.contains(name) {
            not_tried += 1;
            continue;
        }
        if let Some((until, _)) = st.block {
            blocked += 1;
            earliest = Some(earliest.map_or(until, |e| e.min(until)));
            continue;
        }
        match (&st.basis, st.fit) {
            (Basis::Unknown(label), _) => *unknown.entry(*label).or_insert(0) += 1,
            (Basis::IdentityUnavailable, _) => {
                *unknown.entry(ProbeLabel::IdentityUnavailable).or_insert(0) += 1;
            }
            (Basis::NotRead(reason), _) => *not_read.entry(reason).or_insert(0) += 1,
            (Basis::Failed, _) | (_, Err(Unfit::NoReading)) => failed += 1,
            (_, Err(Unfit::Full | Unfit::SpendLimit)) => exhausted += 1,
            (_, Err(Unfit::AboveCeiling)) => above += 1,
            (_, Ok(_)) => other += 1,
        }
    }
    let mut parts: Vec<String> = unknown
        .iter()
        .map(|(label, n)| format!("{n} unknown ({})", label.as_str()))
        .collect();
    for (reason, n) in &not_read {
        parts.push(format!("{n} not read ({reason})"));
    }
    let mut push = |n: usize, text: String| {
        if n > 0 {
            parts.push(format!("{n} {text}"));
        }
    };
    push(failed, "failed".to_string());
    let when = earliest.map_or_else(|| NO_TIME.to_string(), utc_minute);
    push(blocked, format!("blocked until {when}"));
    push(exhausted, "exhausted".to_string());
    push(above, format!("above week ceiling {}%", seen.week_max));
    push(
        not_tried,
        format!("not tried (re-read limit {MAX_REREADS_PER_CYCLE})"),
    );
    let known =
        unknown.is_empty() && not_read.is_empty() && failed + above + not_tried + other == 0;
    let text = if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    };
    (text, known)
}

/// The no-target line (D9): `no profile has capacity` only when every other
/// plan login is known to have no room, `no capacity known` otherwise.
pub fn summary(live: &str, reading: &Reading, seen: &Seen) -> (&'static str, String) {
    let (counts, known) = counts(seen);
    let (key_kind, wording) = if known {
        (KEY_NO_CAPACITY, "no profile has capacity")
    } else {
        (KEY_KNOWN, "no capacity known")
    };
    let figures = msg::usage(&Shown::from_reading(*reading), " ");
    let text = format!("ALERT: {wording} live={live} {figures} (candidates: {counts})");
    (key_kind, text)
}

/// The line of a cycle that ends on [`super::Decision::Alert`]: the
/// excluded live login, the plan logins that kept work off the API key and
/// the blocked API key as before; then a stay note when the live login has
/// room and a reason not to move; else the no-target line.
pub fn alert_line(
    snap: &Snap,
    tag: Option<Tag>,
    live: &Reading,
    key_held: &[ProfileName],
    seen: &Seen,
    s: &Settings,
    reason: impl FnOnce() -> String,
) -> Line {
    let other = |kind: &'static str, text: String| Line {
        kind,
        key_kind: kind,
        text,
        stay: false,
    };
    // Issue #2218: an excluded live login with no profile to move to. It
    // was not measured, so the reason is the exclusion, not its room.
    if snap.live_excluded
        && let LiveId::Known(n) = &snap.live
    {
        return other("excluded-live", msg::alert_excluded_live(n));
    }
    let name = live_name(&snap.live).map_or("-", ProfileName::as_str);
    if !key_held.is_empty() {
        return other("key-held", msg::alert_key_held(name, key_held));
    }
    if let LiveId::Known(n) = &snap.live
        && tag == Some(Tag::On)
        && snap.platform_live()
    {
        return other("platform-blocked", msg::alert_platform_stuck(&reason(), n));
    }
    if let Some(why) = &seen.why
        && let Some(text) = stay_note(why, s)
    {
        let key_kind = match why {
            NoMove::Held { .. } => KEY_HELD,
            NoMove::Unconfirmed => KEY_UNCONFIRMED,
            NoMove::LessRoom { .. } | NoMove::NoTarget => KEY_STAY,
        };
        return Line {
            kind: NO_CAPACITY,
            key_kind,
            text,
            stay: true,
        };
    }
    let (key_kind, text) = summary(name, live, seen);
    Line {
        kind: NO_CAPACITY,
        key_kind,
        text,
        stay: false,
    }
}

/// The reason `name` carries on the `cycle` line (D11): an unknown reading
/// with its label and any last good reading, or, after the figures, an
/// active block with its fixed kind label, or a week above the ceiling.
pub fn annotate(seen: &Seen, name: &ProfileName, shown: &mut Shown) {
    let Some(st) = seen.logins.get(name) else {
        return;
    };
    match &st.basis {
        Basis::LastGood {
            label,
            reading,
            age,
        } => {
            shown.label = Some(label.as_str());
            shown.note = Some(format!(
                "last {} {}m ago",
                msg::usage(&Shown::from_reading(*reading), ","),
                minutes(*age)
            ));
        }
        Basis::Unknown(label) => shown.label = Some(label.as_str()),
        // A login the cycle did not read is not on the `cycle` line at all:
        // its `skip:` line says why, and the no-target counts carry it.
        Basis::Failed | Basis::NotRead(_) | Basis::IdentityUnavailable => {}
        Basis::Probe => {
            if let Some((until, kind)) = st.block {
                shown.note = Some(format!("blocked until {} ({kind})", utc_minute(until)));
            } else if st.fit == Err(Unfit::AboveCeiling) {
                shown.note = Some(format!("above ceiling {}%", seen.week_max));
            }
        }
    }
}

/// The whole `swap` line (F8): the target figures of [`swap_target`], the
/// shared `swap` text and the unconfirmed suffix when this cycle accepted
/// `to` without a fresh reading. A cycle emits it as it stands and
/// `--dry-run` prefixes it with `would `, so the two cannot drift apart.
pub fn swap_line(
    seen: &Seen,
    from: &ProfileName,
    to: &ProfileName,
    live: &Shown,
    target: Shown,
    reason: super::Reason,
) -> String {
    let target = swap_target(seen, to, target);
    let mut line = msg::swap(from, to, live, &target, reason);
    if let Some((_, label)) = seen.unconfirmed.as_ref().filter(|(n, _)| n == to) {
        line.push_str(&unconfirmed(*label));
    }
    line
}

/// The target figures of a `swap` line: a target chosen on its last good
/// reading shows that reading and its age; any other shows its figures as
/// read, with no `cycle`-line reason. Only [`swap_line`] builds the line.
fn swap_target(seen: &Seen, to: &ProfileName, shown: Shown) -> Shown {
    match seen.logins.get(to).map(|st| &st.basis) {
        Some(Basis::LastGood { reading, age, .. }) => Shown {
            note: Some(format!("last {}m ago", minutes(*age))),
            ..Shown::from_reading(*reading)
        },
        _ => Shown {
            label: None,
            note: None,
            ..shown
        },
    }
}

#[cfg(test)]
#[path = "tests_issue_2910_lines.rs"]
mod tests_issue_2910_lines;
