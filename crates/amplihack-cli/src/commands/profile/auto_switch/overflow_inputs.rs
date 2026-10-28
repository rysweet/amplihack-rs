//! What auto-switch reads to consider overflow profiles (issue #1606): their
//! order from `.auto-switch`, the spend blocks from the rotate ledger, and
//! the overflow gate from the running rotator's record (`rotate::gate_record`;
//! the `AMPLIHACK_ROTATE_*` variables only when no rotator runs). The
//! decision itself is `candidate::select_with_overflow`, which is pure.
//!
//! Every failure closes the gate: an unreadable rotate ledger (spend blocks
//! unknown), a running rotator without a usable record, or an invalid
//! variable means no overflow profile is chosen.

use super::candidate::OverflowPool;
use super::config::Config;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::Gate;
use crate::commands::profile::rotate;
use std::path::Path;

pub struct OverflowInputs {
    pub order: Vec<ProfileName>,
    pub blocked: Vec<String>,
    pub gate: Option<Gate>,
}

impl OverflowInputs {
    /// Reads what `cfg` needs; reads nothing when it has no overflow
    /// profiles.
    pub fn load(root: &Path, cfg: &Config, now: i64) -> Self {
        if cfg.overflow.is_empty() {
            return Self::none();
        }
        Self::load_with_gate(root, cfg, rotate::overflow_gate(root), now)
    }

    /// [`Self::load`] with the gate already resolved (tests resolve it
    /// against a fake `/proc`).
    pub fn load_with_gate(
        root: &Path,
        cfg: &Config,
        gate: (Option<Gate>, rotate::GateSource),
        now: i64,
    ) -> Self {
        let order: Vec<ProfileName> = cfg.overflow_order().into_iter().map(|e| e.name).collect();
        if order.is_empty() {
            return Self::none();
        }
        Self::with_gate(root, order, gate.0, &gate.1, now)
    }

    fn with_gate(
        root: &Path,
        order: Vec<ProfileName>,
        gate: Option<Gate>,
        source: &rotate::GateSource,
        now: i64,
    ) -> Self {
        if let rotate::GateSource::Closed(why) = source {
            tracing::debug!(op = "overflow gate", closed = %why);
        }
        match rotate::overflow_blocks(root, now) {
            Ok(b) => Self {
                order,
                blocked: b.into_keys().collect(),
                gate,
            },
            Err(e) => {
                tracing::debug!(op = "overflow spend blocks", error = %e);
                Self {
                    order,
                    blocked: Vec::new(),
                    gate: None,
                }
            }
        }
    }

    /// No overflow profiles.
    pub fn none() -> Self {
        Self {
            order: Vec::new(),
            blocked: Vec::new(),
            gate: None,
        }
    }

    pub fn pool(&self) -> OverflowPool<'_> {
        OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: self.gate,
        }
    }
}
