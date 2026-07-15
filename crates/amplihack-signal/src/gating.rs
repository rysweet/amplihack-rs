//! Trust boundary for inbound Signal messages.
//!
//! Inbound text becomes agent instructions, so the gate is the security-critical
//! seam. It is **infallible** — it returns a [`GateDecision`], never an error —
//! and it is fail-closed by construction: acceptance requires ALL of:
//! 1. sender in the (non-empty) allowlist,
//! 2. `sourceDevice == 1` (operator's primary phone),
//! 3. the message's group id matches this session's group,
//! 4. the body is not a recently-sent outbound (TTL echo suppression).
//!
//! An empty allowlist therefore rejects everything. The echo window is bounded
//! by a TTL (evicted on insert/lookup), never an arbitrary fixed message cap.

use crate::transport::IncomingMessage;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// The primary-device id: the operator's phone. Only device 1 is trusted.
pub const PRIMARY_DEVICE_ID: u32 = 1;

/// Why a message was rejected. Distinct variants aid redacted logging/metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Sender not in the allowlist (includes the empty-allowlist case).
    NotInAllowlist,
    /// Not from the operator's primary device (device != 1).
    NotPrimaryDevice,
    /// Group id did not match this session's group.
    GroupMismatch,
    /// Body matched a recently-sent outbound within the TTL window (loop guard).
    EchoSuppressed,
}

/// The gate's verdict for a single inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Accept and inject as an operator instruction.
    Accept,
    /// Reject, with reason.
    Reject(RejectReason),
}

/// Fail-closed inbound gate bound to one session group.
#[allow(dead_code)] // fields are consumed by the P3 implementation of the stubbed methods
pub struct Gate {
    allowlist: HashSet<String>,
    group_id: String,
    echo_ttl: Duration,
    recent_outbound: HashMap<String, Instant>,
}

impl Gate {
    /// Create a gate for `group_id` trusting exactly `allowlist`.
    pub fn new(
        _allowlist: impl IntoIterator<Item = String>,
        _group_id: impl Into<String>,
        _echo_ttl: Duration,
    ) -> Self {
        todo!("construct gate (P3)")
    }

    /// Record an outbound body just posted by the bot, so its synced-back copy
    /// is suppressed for the TTL window. Evicts expired entries.
    pub fn record_outbound(&mut self, _body: &str, _now: Instant) {
        todo!("record outbound for echo suppression (P3)")
    }

    /// Evaluate an inbound message. Infallible.
    pub fn evaluate(&mut self, _msg: &IncomingMessage, _now: Instant) -> GateDecision {
        todo!("evaluate allowlist AND device AND group AND echo (P3)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(source: &str, device: u32, group: &str, body: &str) -> IncomingMessage {
        IncomingMessage {
            source_number: source.into(),
            source_device: device,
            group_id: Some(group.into()),
            body: body.into(),
        }
    }

    const GID: &str = "SESSION_GROUP_ID_AAA==";
    const OP: &str = "+15551239999";

    fn gate_with(allow: &[&str]) -> Gate {
        Gate::new(
            allow.iter().map(|s| s.to_string()),
            GID,
            Duration::from_secs(30),
        )
    }

    #[test]
    fn happy_path_accepts_primary_device_allowlisted_matching_group() {
        let mut g = gate_with(&[OP]);
        let d = g.evaluate(&msg(OP, 1, GID, "do the thing"), Instant::now());
        assert_eq!(d, GateDecision::Accept);
    }

    #[test]
    fn empty_allowlist_rejects_everything() {
        let mut g = gate_with(&[]);
        let d = g.evaluate(&msg(OP, 1, GID, "do the thing"), Instant::now());
        assert_eq!(d, GateDecision::Reject(RejectReason::NotInAllowlist));
    }

    #[test]
    fn sender_not_in_allowlist_rejected() {
        let mut g = gate_with(&[OP]);
        let d = g.evaluate(&msg("+15550000001", 1, GID, "do the thing"), Instant::now());
        assert_eq!(d, GateDecision::Reject(RejectReason::NotInAllowlist));
    }

    #[test]
    fn non_primary_device_rejected() {
        let mut g = gate_with(&[OP]);
        let d = g.evaluate(&msg(OP, 2, GID, "synced-back bot echo"), Instant::now());
        assert_eq!(d, GateDecision::Reject(RejectReason::NotPrimaryDevice));
    }

    #[test]
    fn group_mismatch_rejected() {
        let mut g = gate_with(&[OP]);
        let d = g.evaluate(&msg(OP, 1, "OTHER_GROUP==", "hi"), Instant::now());
        assert_eq!(d, GateDecision::Reject(RejectReason::GroupMismatch));
    }

    #[test]
    fn missing_group_id_rejected_as_mismatch() {
        let mut g = gate_with(&[OP]);
        let mut m = msg(OP, 1, GID, "hi");
        m.group_id = None;
        let d = g.evaluate(&m, Instant::now());
        assert_eq!(d, GateDecision::Reject(RejectReason::GroupMismatch));
    }

    #[test]
    fn recent_outbound_is_echo_suppressed_within_ttl() {
        let mut g = gate_with(&[OP]);
        let t0 = Instant::now();
        g.record_outbound("session started", t0);
        let d = g.evaluate(&msg(OP, 1, GID, "session started"), t0);
        assert_eq!(d, GateDecision::Reject(RejectReason::EchoSuppressed));
    }

    #[test]
    fn outbound_accepted_after_ttl_expires() {
        let mut g = Gate::new([OP.to_string()], GID, Duration::from_secs(30));
        let t0 = Instant::now();
        g.record_outbound("session started", t0);
        // Same body arriving after the TTL window is no longer suppressed.
        let later = t0 + Duration::from_secs(31);
        let d = g.evaluate(&msg(OP, 1, GID, "session started"), later);
        assert_eq!(d, GateDecision::Accept);
    }

    #[test]
    fn allowlist_check_precedes_device_check() {
        // A non-allowlisted, non-primary-device message reports NotInAllowlist:
        // the cheapest/outermost authz check wins, keeping ordering deterministic.
        let mut g = gate_with(&[OP]);
        let d = g.evaluate(&msg("+15550000002", 5, GID, "x"), Instant::now());
        assert_eq!(d, GateDecision::Reject(RejectReason::NotInAllowlist));
    }
}
