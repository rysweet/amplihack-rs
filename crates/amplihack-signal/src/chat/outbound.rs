//! Outbound secret redaction, applied **before** chunking.
//!
//! An accepted agent turn is captured verbatim from `copilot` stdout and can
//! contain pasted or echoed credentials. Because the group may have more than
//! one member, secrets must be scrubbed before any byte leaves the machine.
//! [`redact_for_relay`] scrubs the high-frequency secret shapes; the chat
//! always pipes through [`redact_and_chunk`] so redaction happens over the
//! **whole** body first and a secret can never straddle (and survive in) a
//! chunk boundary.
//!
//! The canonical redactor now lives in the dependency-free leaf crate
//! [`amplihack_redact`] so both `amplihack-signal` and `amplihack-turn` can
//! call it without a dependency cycle (issues #1096 / #1103 / #1108). It is
//! re-exported here so every existing caller and import path is unchanged.

use super::chunk::chunk;

/// Scrub high-frequency secret shapes out of a body before it is relayed.
///
/// Re-exported from the canonical [`amplihack_redact`] leaf crate; the public
/// path (`amplihack_signal::chat::outbound::redact_for_relay`) is unchanged.
pub use amplihack_redact::redact_for_relay;

/// Redact secrets over the whole body, **then** split into Signal-sized chunks.
///
/// Redacting before chunking guarantees no individual outbound message can leak
/// a secret that would otherwise straddle a chunk boundary.
#[must_use]
pub fn redact_and_chunk(body: &str) -> Vec<String> {
    chunk(&redact_for_relay(body))
}
