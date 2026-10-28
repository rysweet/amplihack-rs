//! What auto-switch reads to consider overflow profiles (issue #1606): their
//! order from `.auto-switch`, the spend blocks from the rotate ledger, and
//! the overflow gate from the `AMPLIHACK_ROTATE_OVERFLOW_*` variables. The
//! decision itself is `candidate::select_with_overflow`, which is pure.
//!
//! Every failure closes the gate: an unreadable rotate ledger (spend blocks
//! unknown) or an invalid variable means no overflow profile is chosen.

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
        let order: Vec<ProfileName> = cfg.overflow_order().into_iter().map(|e| e.name).collect();
        if order.is_empty() {
            return Self::none();
        }
        let gate = rotate::overflow_gate_from_env()
            .map_err(|e| tracing::debug!(op = "overflow gate", error = %e))
            .ok();
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
