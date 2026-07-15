//! Fail-closed inbound gating.
//!
//! An inbound [`Envelope`] is accepted **only** if it passes *all* of:
//!
//! 1. `groupId` matches this session's group,
//! 2. sender is on the allowlist (an **empty allowlist denies everything**),
//! 3. sender device id equals the expected device (`own_device_id`, default 1),
//! 4. it is not the account's own synced message (`is_sync == false`), and
//! 5. its body is not a recently-sent outbound body still inside the bounded
//!    echo-suppression TTL window.
//!
//! Rules 4 and 5 are a deliberate **dual guard** against re-ingesting the bot's
//! own synced-back messages.

use crate::config::SignalConfig;
use crate::transport::Envelope;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Default echo-suppression window.
pub const DEFAULT_ECHO_TTL: Duration = Duration::from_secs(120);

/// Fail-closed inbound decision function with echo suppression.
pub struct Gate {
    ttl: Duration,
    group_id: String,
    allowlist: HashSet<String>,
    device: u32,
    /// Recently-sent outbound bodies with their send time, for echo suppression.
    recent_outbound: Vec<(String, Instant)>,
}

impl Gate {
    /// Build a gate for `session_group_id` using the config's allowlist and
    /// expected device id, with the [`DEFAULT_ECHO_TTL`] echo window.
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
            device: cfg.effective_device_id(),
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
        // 2. Must not be the account's own synced-back message.
        if env.is_sync {
            return None;
        }
        // 3. Sender must be on the allowlist (empty allowlist denies everything).
        match env.source.as_deref() {
            Some(src) if self.allowlist.contains(src) => {}
            _ => return None,
        }
        // 4. Sender device must equal the expected (anti-spoof) device.
        if env.source_device != Some(self.device) {
            return None;
        }
        // 5. Must carry a body.
        let body = env.body.as_deref()?;
        // 6. Must not echo a recently-sent outbound body (bounded TTL window).
        if self
            .recent_outbound
            .iter()
            .any(|(recorded, _)| recorded == body)
        {
            return None;
        }
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
    fn rejects_non_primary_device_by_default() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], None), GID);
        let env = group_msg("+15551230001", 2, "from linked device");
        assert_eq!(gate.evaluate(&env), None);
    }

    #[test]
    fn honors_configured_own_device_id() {
        let mut gate = Gate::new(&cfg(&["+15551230001"], Some(3)), GID);
        let ok = group_msg("+15551230001", 3, "from device 3");
        assert_eq!(gate.evaluate(&ok), Some("from device 3".to_string()));
        let bad = group_msg("+15551230001", 1, "from device 1");
        assert_eq!(gate.evaluate(&bad), None);
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
    fn rejects_sync_message_even_if_allowlisted() {
        // The account's own number is (implicitly) trusted, but a synced copy
        // of our own message must never be treated as an operator instruction.
        let mut gate = Gate::new(&cfg(&["+15551230000"], None), GID);
        let mut env = group_msg("+15551230000", 1, "session started");
        env.is_sync = true;
        assert_eq!(gate.evaluate(&env), None);
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
