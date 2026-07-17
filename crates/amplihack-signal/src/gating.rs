//! Fail-closed inbound gating.
//!
//! An inbound [`Envelope`] becomes an operator instruction **only** if it passes
//! every check below. The gate supports both Signal deployment shapes:
//!
//! * **Single-number linked-device** (signal-cli is a linked device on the
//!   operator's *own* number). The operator types on their **primary phone**, so
//!   the group message reaches signal-cli as a `syncMessage.sentMessage`
//!   (`is_sync == true`) authored by the account itself with
//!   `source_device == 1`. signal-cli's own sends sync back from its linked
//!   device id (`own_device_id`, `>= 2`) and are rejected.
//! * **Dedicated-number** (signal-cli owns a separate number). The operator
//!   commands from a **different** allowlisted number, arriving as a normal
//!   `dataMessage` (`is_sync == false`).
//!
//! Checks (all must hold):
//! 1. `group_id` matches this session's group,
//! 2. a non-empty body is present,
//! 3. the body is not a recently-sent outbound body still inside the bounded
//!    echo-suppression TTL window,
//! 4. the sender is on the allowlist (an **empty allowlist denies everything**),
//! 5. authorization by envelope shape:
//!    - a `syncMessage` (the account's own transcript) is accepted **only** from
//!      the primary phone (`source_device == 1`), authored by the account, and
//!      never from signal-cli's own linked device (`own_device_id`) — this is
//!      the linked-device operator path,
//!    - a `dataMessage` is accepted from any allowlisted separate number; its
//!      device id is *not* gated because it belongs to that sender's own account.
//!
//! Checks 3 and 5 together are the dual guard against re-ingesting the bot's own
//! synced-back messages.

use crate::config::SignalConfig;
use crate::transport::Envelope;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Default echo-suppression window.
pub const DEFAULT_ECHO_TTL: Duration = Duration::from_secs(120);

/// The account owner's primary phone is always signal-cli device id `1`; every
/// linked device (signal-cli itself, Signal Desktop, ...) is `>= 2`. Only the
/// primary phone represents the human operator in a linked-device setup.
pub const PRIMARY_DEVICE_ID: u32 = 1;

/// Fail-closed inbound decision function with echo suppression.
pub struct Gate {
    ttl: Duration,
    group_id: String,
    allowlist: HashSet<String>,
    /// The account signal-cli owns; author of synced `sentMessage` transcripts.
    account: String,
    /// signal-cli's own linked-device id (`>= 2`) when configured, used to reject
    /// the bot's own synced-back echoes explicitly.
    own_device_id: Option<u32>,
    /// Recently-sent outbound bodies with their send time, for echo suppression.
    recent_outbound: Vec<(String, Instant)>,
}

impl Gate {
    /// Build a gate for `session_group_id` using the config's allowlist,
    /// account, and own-device id, with the [`DEFAULT_ECHO_TTL`] echo window.
    #[must_use]
    pub fn new(cfg: &SignalConfig, session_group_id: impl Into<String>) -> Self {
        Self::with_ttl(cfg, session_group_id, DEFAULT_ECHO_TTL)
    }

    /// Like [`Gate::new`] but with an explicit echo-suppression TTL.
    #[must_use]
    pub fn with_ttl(
        cfg: &SignalConfig,
        session_group_id: impl Into<String>,
        ttl: Duration,
    ) -> Self {
        Self {
            ttl,
            group_id: session_group_id.into(),
            allowlist: cfg.allowlist.iter().cloned().collect(),
            account: cfg.account.clone(),
            own_device_id: cfg.own_device_id,
            recent_outbound: Vec::new(),
        }
    }

    /// Record an outbound body into the echo-suppression window (`now`).
    pub fn record_outbound(&mut self, body: &str) {
        self.record_outbound_at(body, Instant::now());
    }

    /// Deterministic test seam for [`Gate::record_outbound`].
    pub fn record_outbound_at(&mut self, body: &str, at: Instant) {
        self.prune(at);
        self.recent_outbound.push((body.to_string(), at));
    }

    /// Evaluate an inbound envelope; returns `Some(instruction)` when accepted.
    pub fn evaluate(&mut self, env: &Envelope) -> Option<String> {
        self.evaluate_at(env, Instant::now())
    }

    /// Deterministic test seam for [`Gate::evaluate`] (explicit `now`).
    pub fn evaluate_at(&mut self, env: &Envelope, now: Instant) -> Option<String> {
        self.prune(now);

        // 1. Must be for this session's group.
        if env.group_id.as_deref() != Some(self.group_id.as_str()) {
            return None;
        }
        // 2. Must carry a non-empty body.
        let body = env.body.as_deref().filter(|b| !b.is_empty())?;
        // 3. Must not echo a recently-sent outbound body (bounded TTL window).
        if self
            .recent_outbound
            .iter()
            .any(|(recorded, _)| recorded == body)
        {
            return None;
        }
        // 4. Sender must be present and on the allowlist (empty allowlist = deny all).
        let src = env.source.as_deref()?;
        if !self.allowlist.contains(src) {
            return None;
        }
        // 5. Authorize by envelope shape.
        if env.is_sync {
            // Linked-device operator path: the account's own transcript sync.
            // Reject the bot's own linked-device echo explicitly...
            if self.own_device_id.is_some() && env.source_device == self.own_device_id {
                return None;
            }
            // ...only the primary phone (device 1) is the human operator...
            if env.source_device != Some(PRIMARY_DEVICE_ID) {
                return None;
            }
            // ...and a sync transcript is authored by the account itself.
            if src != self.account {
                return None;
            }
        }
        // A `dataMessage` (`is_sync == false`) from a separate allowlisted number
        // is accepted without device gating: the device belongs to that sender's
        // own account, not ours.
        Some(body.to_string())
    }

    /// Drop echo-suppression entries older than the TTL relative to `now`.
    fn prune(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.recent_outbound
            .retain(|(_, at)| now.saturating_duration_since(*at) < ttl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const GID: &str = "grp-abc123==";

    fn cfg(allowlist: &[&str], own_device: Option<u32>) -> SignalConfig {
        let mut env = HashMap::new();
        env.insert(
            crate::config::ENV_ENDPOINT.to_string(),
            "127.0.0.1:7583".to_string(),
        );
        env.insert(
            crate::config::ENV_ACCOUNT.to_string(),
            "+15551230000".to_string(),
        );
        env.insert(
            crate::config::ENV_ALLOWLIST.to_string(),
            allowlist.join(","),
        );
        if let Some(d) = own_device {
            env.insert(crate::config::ENV_OWN_DEVICE_ID.to_string(), d.to_string());
        }
        SignalConfig::from_sources(&env, None).expect("valid test config")
    }

    fn group_msg(sender: &str, device: u32, body: &str) -> Envelope {
        Envelope {
            source: Some(sender.to_string()),
            source_device: Some(device),
            group_id: Some(GID.to_string()),
            body: Some(body.to_string()),
            is_sync: false,
        }
    }

    /// A `syncMessage.sentMessage` transcript: authored by the account itself
    /// on some device, delivered to signal-cli as the account's own sync.
    fn sync_msg(device: u32, body: &str) -> Envelope {
        Envelope {
            source: Some("+15551230000".to_string()),
            source_device: Some(device),
            group_id: Some(GID.to_string()),
            body: Some(body.to_string()),
            is_sync: true,
        }
    }

    #[test]
    fn accepts_allowlisted_primary_device_group_message() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let env = group_msg("+15551230001", 1, "focus on failing test");
        assert_eq!(
            gate.evaluate(&env),
            Some("focus on failing test".to_string())
        );
    }

    #[test]
    fn rejects_sender_not_on_allowlist() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let env = group_msg("+15559999999", 1, "hi");
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn empty_allowlist_denies_everything() {
        let mut gate = Gate::new(&cfg(&[], None), GID);
        let env = group_msg("+15551230001", 1, "hi");
        assert_eq!(gate.evaluate(&env), None, "fail-closed: empty allowlist");
    }

    #[test]
    fn accepts_dedicated_number_regardless_of_device() {
        // Dedicated-number path: a separate allowlisted number commands via a
        // normal dataMessage. The device id belongs to *their* account, so it is
        // not gated.
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let env = group_msg("+15551230001", 4, "from their linked device");
        assert_eq!(
            gate.evaluate(&env),
            Some("from their linked device".to_string())
        );
    }

    #[test]
    fn accepts_primary_phone_sync_as_operator_input() {
        // Single-number linked-device path: the operator types on their primary
        // phone (device 1); it arrives as the account's own sync transcript.
        // This is the real-world setup and MUST be accepted.
        let mut gate = Gate::new(&cfg(&["+15551230000"], None), GID);
        let env = sync_msg(1, "run the tests again");
        assert_eq!(gate.evaluate(&env), Some("run the tests again".to_string()));
    }

    #[test]
    fn rejects_non_primary_device_sync_echo() {
        // The bot's own send syncs back from a linked device (>= 2). Without any
        // configured own_device_id, the device-1 requirement alone rejects it.
        let mut gate = Gate::new(&cfg(&["+15551230000"], None), GID);
        let env = sync_msg(2, "session started");
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn rejects_bot_own_linked_device_sync() {
        // With own_device_id configured, the bot's linked-device echo is rejected
        // explicitly by device match (defence in depth).
        let mut gate = Gate::new(&cfg(&["+15551230000"], Some(3)), GID);
        let env = sync_msg(3, "session started");
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn rejects_sync_not_authored_by_account() {
        // A sync transcript claiming a device-1 origin but authored by some other
        // (even allowlisted) number is not the operator's own phone.
        let mut gate = Gate::new(&cfg(&["+15551230000", "+15551230009"], None), GID);
        let mut env = sync_msg(1, "spoofed");
        env.source = Some("+15551230009".to_string());
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn rejects_empty_body() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let env = group_msg("+15551230001", 1, "");
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn rejects_message_for_other_group() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let mut env = group_msg("+15551230001", 1, "hi");
        env.group_id = Some("grp-OTHER==".to_string());
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn rejects_non_group_message() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let mut env = group_msg("+15551230001", 1, "hi");
        env.group_id = None;
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn suppresses_outbound_sync_echo_within_ttl() {
        // The bot's own outbound also syncs back from the primary phone's
        // perspective in some setups; the echo-suppression window is the guard
        // that stops re-ingesting our own words even on the device-1 path.
        let mut gate = Gate::with_ttl(&cfg(&["+15551230000"], None), GID, Duration::from_secs(60));
        let t0 = Instant::now();
        gate.record_outbound_at("session started", t0);
        let env = sync_msg(1, "session started");
        assert_eq!(gate.evaluate_at(&env, t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn suppresses_recent_outbound_echo_within_ttl() {
        let mut gate = Gate::with_ttl(&cfg(&["+15551230001"], None), GID, Duration::from_secs(60));
        let t0 = Instant::now();
        gate.record_outbound_at("session started", t0);
        // A message whose body equals a recent outbound body is an echo.
        let env = group_msg("+15551230001", 1, "session started");
        assert_eq!(gate.evaluate_at(&env, t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn echo_suppression_expires_after_ttl() {
        let mut gate = Gate::with_ttl(&cfg(&["+15551230001"], None), GID, Duration::from_secs(60));
        let t0 = Instant::now();
        gate.record_outbound_at("session started", t0);
        // Past the TTL the same body is a legitimate (if odd) instruction.
        let env = group_msg("+15551230001", 1, "session started");
        assert_eq!(
            gate.evaluate_at(&env, t0 + Duration::from_secs(61)),
            Some("session started".to_string())
        );
    }
}
