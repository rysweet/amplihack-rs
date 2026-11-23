//! Which other login's reset sets an earliest-reset wait (issue #2967).
//!
//! Pure: it is given the instants and picks the minimum, a tie going to the
//! first profile name, so the same inputs always name the same login. The
//! caller has already left out every login that is under the threshold, tried, barred or
//! without a reset, and every instant at or before now; nothing is filtered
//! here.
//!
//! Tested in `tests_earliest.rs`.

use super::super::name::ProfileName;

/// The login with the smallest instant, and that instant.
pub(crate) fn earliest(others: &[(ProfileName, i64)]) -> Option<(ProfileName, i64)> {
    others
        .iter()
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.as_str().cmp(b.0.as_str())))
        .cloned()
}
