//! What auto-switch reads to consider overflow profiles (issue #1606): their
//! order from `.auto-switch`, the spend blocks from the rotate ledger, and
//! the overflow gate from the running rotator's record (`rotate::gate_record`;
//! the `AMPLIHACK_ROTATE_*` variables only when no rotator runs). The
//! decision itself is `candidate::select_with_overflow`, which is pure.
//!
//! Every failure closes the gate: an unreadable rotate ledger (spend blocks
//! unknown), a running rotator without a usable record, or an invalid
//! variable means no overflow profile is chosen.
//!
//! Issue #1700: the order is the rotator's (`overflow::tiers`): the credit
//! profiles, then the first API-key profile, whose block counts as one of
//! the spend blocks. A corrupt `.auto-switch` gives no order at all.

use super::candidate::OverflowPool;
use super::config::Config;
use crate::commands::profile::kind;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::Gate;
use crate::commands::profile::rotate;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd;
use std::path::Path;

pub struct OverflowInputs {
    pub order: Vec<ProfileName>,
    pub blocked: Vec<String>,
    pub gate: Option<Gate>,
    /// The API-key profile, last in `order`.
    pub platform: Option<ProfileName>,
}

/// The overflow order for `cfg` under `root`: its entries without API-key
/// profiles, then the first API-key profile by name; and that profile.
fn order_of(
    root: &Path,
    cfg: &Config,
    home: Option<&Path>,
) -> (Vec<ProfileName>, Option<ProfileName>) {
    let store = ProfileStore::new(root.to_path_buf());
    let kinds = kind::Ctx::load(&store, home);
    if kinds.corrupt() {
        return (Vec::new(), None);
    }
    let platforms = kinds.platform_profiles(&store);
    let mut order: Vec<ProfileName> = cfg
        .overflow_order()
        .into_iter()
        .map(|e| e.name)
        .filter(|n| !platforms.contains(n))
        .collect();
    let platform = platforms.into_iter().next();
    order.extend(platform.clone());
    (order, platform)
}

impl OverflowInputs {
    /// Reads what `cfg` needs; reads nothing when it has no overflow
    /// profiles.
    pub fn load(root: &Path, cfg: &Config, now: i64) -> Self {
        let (order, platform) = order_of(root, cfg, usage_cmd::home_dir().as_deref());
        if order.is_empty() {
            return Self::none();
        }
        let (gate, source) = rotate::overflow_gate(root);
        Self::with_gate(root, order, platform, gate, &source, now)
    }

    /// [`Self::load`] with the gate already resolved (tests resolve it
    /// against a fake `/proc`).
    #[cfg(test)]
    pub fn load_with_gate(
        root: &Path,
        cfg: &Config,
        gate: (Option<Gate>, rotate::GateSource),
        now: i64,
    ) -> Self {
        // Tests: no home, so nothing outside the root is read.
        let (order, platform) = order_of(root, cfg, None);
        if order.is_empty() {
            return Self::none();
        }
        Self::with_gate(root, order, platform, gate.0, &gate.1, now)
    }

    fn with_gate(
        root: &Path,
        order: Vec<ProfileName>,
        platform: Option<ProfileName>,
        gate: Option<Gate>,
        source: &rotate::GateSource,
        now: i64,
    ) -> Self {
        if let rotate::GateSource::Closed(why) = source {
            tracing::debug!(op = "overflow gate", closed = %why);
        }
        match rotate::ledger::RotateState::new(root).load_ledger() {
            Ok(l) => {
                let mut blocked: Vec<String> = l
                    .as_ref()
                    .map(|l| rotate::ledger::active_blocks(l, now).into_keys().collect())
                    .unwrap_or_default();
                if let Some(p) = &platform
                    && l.as_ref()
                        .is_some_and(|l| rotate::platform_block::active(l, now).is_some())
                {
                    blocked.push(p.to_string());
                }
                Self {
                    order,
                    blocked,
                    gate,
                    platform,
                }
            }
            Err(e) => {
                tracing::debug!(op = "overflow spend blocks", error = %e);
                Self {
                    order,
                    blocked: Vec::new(),
                    gate: None,
                    platform,
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
            platform: None,
        }
    }

    pub fn pool(&self) -> OverflowPool<'_> {
        OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: self.gate,
            platform: self.platform.as_ref(),
        }
    }
}
