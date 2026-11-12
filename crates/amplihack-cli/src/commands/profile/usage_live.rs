//! Whether the profile `profile usage` marked live in `~/.claude` is still
//! the live one after the probes (issue #1693). Issue #1886: `usage` goes by
//! the ledgers as [`detect`] reads them and makes no identity read; whose
//! login `~/.claude` holds is settled by the rotator's hand-change recovery,
//! from what the credential returns, never from `.claude.json`. Read-only.

use super::super::store::ProfileStore;
use super::{Live, detect};
use std::path::Path;

/// Whether `live` is still the live profile, by the same evidence. A
/// profile that became live after `live` was detected, or a swap away and
/// back under a ledger without `since`, is not seen.
pub(in crate::commands::profile) fn still_live(
    store: &ProfileStore,
    home: Option<&Path>,
    live: &Live,
) -> bool {
    detect(store, home).0.is_some_and(|now| now == *live)
}
