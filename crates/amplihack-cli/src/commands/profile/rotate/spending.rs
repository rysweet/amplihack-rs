//! One effective spending rule for automatic decisions and explicit switching.
use super::{custody, ledger::RotateLedger, observe::Snap};
use crate::commands::profile::{name::ProfileName, store::ProfileStore};

pub(super) fn refusal(
    store: &ProfileStore,
    snap: &Snap,
    ledger: Option<&RotateLedger>,
    name: &ProfileName,
    now: i64,
) -> Option<String> {
    let origin = match custody::origin(store, name) {
        Ok(origin) => origin,
        Err(e) => {
            return Some(format!(
                "profile {name} recovery provenance cannot be verified: {e}; repair its custody before switching"
            ));
        }
    };
    if origin.as_ref().is_some_and(|origin| {
        snap.overflow
            .iter()
            .find(|entry| &entry.name == origin)
            .is_some_and(|original| {
                !snap
                    .overflow
                    .iter()
                    .any(|entry| &entry.name == name && entry.tier == original.tier)
            })
    }) {
        return Some(format!(
            "profile {name} must retain its origin's overflow protection; repair its overflow configuration before switching"
        ));
    }
    let ledger = ledger?;
    for protected in std::iter::once(name).chain(origin.as_ref()) {
        if let Some(block) = ledger
            .overflow_blocked
            .get(protected.as_str())
            .filter(|b| now < b.until)
        {
            return Some(format!(
                "{} (effective spending protection for {name})",
                super::log::msg::switch_spend_blocked(protected, block.until)
            ));
        }
    }
    None
}
