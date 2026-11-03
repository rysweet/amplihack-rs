//! Issue #1880: the stored count of consecutive failed live probes, the
//! ledger's `failed_live` field. It holds a profile name and a count only,
//! never an account, a key or probe text, and it is keyed by the profile
//! name alone (#1886). See "Stored failed-probe count" in
//! docs/reference/profile-command.md.
//!
//! The field is read leniently: a value that is not a valid record loads as
//! absent and never fails the rest of the ledger, which the limit-resume
//! hook also reads. The serde error is dropped unread, since its text can
//! quote the stored value. An absent record only delays a failed-probe swap.

use super::cycle::Ctx;
use super::identify::LiveId;
use super::ledger::RotateLedger;
use crate::commands::profile::name::ProfileName;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The highest count a ledger may hold; a larger one loads as this.
pub const MAX_STORED: u32 = 1000;

/// The stored record: which live profile failed, and how many cycles in a
/// row. Unknown fields make the record invalid, so it loads as absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedLive {
    pub profile: String,
    pub count: u32,
}

/// The record as a file may hold it: a count wider than `u32`, capped on load.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    profile: String,
    count: u64,
}

/// The serde `with` writer for `RotateLedger::failed_live`.
pub fn serialize<S: Serializer>(v: &Option<FailedLive>, s: S) -> Result<S::Ok, S::Error> {
    v.serialize(s)
}

/// The serde `with` reader for `RotateLedger::failed_live`: an invalid value
/// is `None`, never an error.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<FailedLive>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(d)?;
    Ok(raw
        .and_then(|v| serde_json::from_value::<Stored>(v).ok())
        .and_then(sanitise))
}

/// A valid profile name and a count of at least 1, capped at [`MAX_STORED`].
fn sanitise(s: Stored) -> Option<FailedLive> {
    ProfileName::parse(&s.profile).ok()?;
    if s.count == 0 {
        return None;
    }
    let count = u32::try_from(s.count).map_or(MAX_STORED, |c| c.min(MAX_STORED));
    Some(FailedLive {
        profile: s.profile,
        count,
    })
}

/// The count stored for `live`: exact, case-sensitive match on
/// `LiveId::Known` only; 0 for any other identity, profile or no ledger.
pub fn prior(l: Option<&RotateLedger>, live: &LiveId) -> u32 {
    let LiveId::Known(name) = live else {
        return 0;
    };
    l.and_then(|l| l.failed_live.as_ref())
        .filter(|f| f.profile == name.as_str())
        .map_or(0, |f| f.count)
}

/// [`prior`] from the ledger on disk; 0 when the ledger cannot be read. The
/// decision reads the ledger again and stops the cycle on that error, so a
/// 0 here never leads to a swap.
pub fn load(ctx: &Ctx, live: &LiveId) -> u32 {
    match ctx.rs.load_ledger() {
        Ok(l) => prior(l.as_ref(), live),
        Err(_) => 0,
    }
}

/// The only writer: sets `failed_live` to `{live, count}`, clears it for a
/// count of 0 or a completed swap (`swapped`), and leaves it unchanged when
/// the live identity is not `Known`.
pub fn record(l: &mut RotateLedger, live: &LiveId, count: u32, swapped: bool) {
    if swapped {
        l.failed_live = None;
        return;
    }
    let LiveId::Known(name) = live else {
        return;
    };
    l.failed_live = (count > 0).then(|| FailedLive {
        profile: name.as_str().to_string(),
        count: count.min(MAX_STORED),
    });
}
