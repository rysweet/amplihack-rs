//! The rotate ledger's hand-change fields (issue #1693): their limits and the
//! checks a loaded ledger must pass. The fields themselves are on
//! `RotateLedger` (`guidance`, `home_unreadable_since`, `lost_login`); this
//! module keeps `ledger.rs` within its line budget, as `spend_blocks.rs` does
//! for the overflow fields.

use crate::commands::profile::name::ProfileName;

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
