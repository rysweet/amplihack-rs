//! The rotate ledger's hand-change fields (issues #1693 and #1886): their
//! limits and the checks a loaded ledger must pass. The fields themselves are
//! on `RotateLedger` (`guidance`, `home_unreadable_since`, `lost_login`,
//! `hand_suspect`); this
//! module keeps `ledger.rs` within its line budget, as `spend_blocks.rs` does
//! for the overflow fields.

use crate::commands::profile::name::ProfileName;
use serde::{Deserialize, Serialize};

/// Limits on the hand-change fields (issue #1693).
pub const MAX_GUIDANCE_LINES: usize = 16;
pub const MAX_GUIDANCE_LINE: usize = 1024;
pub const MAX_LOST_LOGIN: usize = 64;
/// How long, in seconds, `~/.claude.json` may have no readable account before
/// the `home-unreadable` ALERT stops saying "the next cycle reads it again"
/// and names a fix. Claude rewrites the file often, so one failed read is
/// normal; five minutes of them is not (#1702 review). With the default
/// 600-second interval the second such cycle escalates; a cycle never
/// escalates on its own first read.
pub const HOME_UNREADABLE_GRACE: i64 = 300;
/// Issue #1886: how long, in seconds after the ledger's `swapped_at`,
/// hand-change recovery makes no identity read. Running sessions can still
/// write the previous account into `~/.claude.json` and refresh the token
/// they hold for some minutes after a swap. With `--interval 120` this spans
/// five cycles.
pub const SWAP_GRACE: i64 = 600;

/// Issue #1886: the first credential reading that `~/.claude` holds the account of
/// `profile`'s own login, at `first_seen` (epoch seconds). A later cycle, at
/// least one interval on, must read the same before anything moves. Holds no
/// reading and no account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandSuspect {
    pub profile: String,
    pub first_seen: i64,
}

/// A suspicion must name a valid profile and a time after the epoch.
pub fn valid_hand_suspect(s: Option<&HandSuspect>) -> bool {
    s.is_none_or(|s| ProfileName::parse(&s.profile).is_ok() && s.first_seen > 0)
}

/// A ledger name, when present, must be a valid profile name.
pub fn valid_name(v: Option<&str>) -> bool {
    v.is_none_or(|n| ProfileName::parse(n).is_ok())
}

/// A control or bidirectional-override character: never stored in, or
/// printed from, a guidance line or a command.
pub fn unprintable(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub fn valid_guidance(g: Option<&[String]>) -> bool {
    g.is_none_or(|g| {
        g.len() <= MAX_GUIDANCE_LINES
            && g.iter()
                .all(|l| l.len() <= MAX_GUIDANCE_LINE && !l.chars().any(unprintable))
    })
}

pub fn valid_lost_login(v: &[String]) -> bool {
    v.len() <= MAX_LOST_LOGIN
        && v.iter()
            .enumerate()
            .all(|(i, n)| ProfileName::parse(n).is_ok() && !v[..i].contains(n))
}
