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
//! Issue #1804: only an API-key profile that `overflow=` marks is in it, and
//! every API-key profile is in `not_plan`, so none counts as a plan login.
//! Kinds that cannot be read mean no profile is known to be free of per-token
//! billing, so `kinds_unknown` stops every selection and capacity write.

use super::candidate::OverflowPool;
use super::config::Config;
use crate::commands::profile::kind::Ctx;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{self, Gate};
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
    /// Every API-key profile, marked or not: none is a plan login.
    pub not_plan: Vec<ProfileName>,
    /// Kinds could not be read: `order` and `not_plan` are empty, and
    /// nothing may be selected.
    pub kinds_unknown: bool,
}

/// What [`order_of`] reads from `.auto-switch` and the profiles.
struct Order {
    order: Vec<ProfileName>,
    platform: Option<ProfileName>,
    not_plan: Vec<ProfileName>,
}

/// The overflow order for `cfg` under `root`, by the rotator's rule
/// (`overflow::tiers_of`): its credit entries, then the first marked
/// API-key profile by name; that profile; and every API-key profile. An
/// API-key profile that `overflow=` does not mark is not in the order
/// (issue #1804). `None` when kinds cannot be read or the root cannot be
/// listed. Parses `.claude.json`
/// only for a profile with no login file, so this stays cheap on the status
/// line.
fn order_of(root: &Path, cfg: &Config, home: Option<&Path>) -> Option<Order> {
    let store = ProfileStore::new(root.to_path_buf());
    let kinds = Ctx::load(&store, home);
    if kinds.corrupt() {
        return None;
    }
    let not_plan = kinds.try_platform_profiles(&store)?;
    let (tiers, _) = overflow::tiers_of(cfg.overflow_order(), &not_plan);
    let platform = tiers
        .iter()
        .find(|e| e.tier == overflow::PLATFORM_TIER)
        .map(|e| e.name.clone());
    Some(Order {
        order: tiers.into_iter().map(|e| e.name).collect(),
        platform,
        not_plan,
    })
}

impl OverflowInputs {
    /// Reads what `cfg` needs. Kinds are always read, since an API-key
    /// profile must stay out of the plan logins with nothing marked; the gate
    /// and spend blocks are read only with overflow profiles.
    pub fn load(root: &Path, cfg: &Config, now: i64) -> Self {
        let Some(o) = order_of(root, cfg, usage_cmd::home_dir().as_deref()) else {
            return Self::unknown();
        };
        if o.order.is_empty() {
            return Self::without_order(o);
        }
        let (gate, source) = rotate::overflow_gate(root);
        Self::with_gate(root, o, gate, &source, now)
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
        let Some(o) = order_of(root, cfg, None) else {
            return Self::unknown();
        };
        if o.order.is_empty() {
            return Self::without_order(o);
        }
        Self::with_gate(root, o, gate.0, &gate.1, now)
    }

    fn with_gate(
        root: &Path,
        Order {
            order,
            platform,
            not_plan,
        }: Order,
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
                    not_plan,
                    kinds_unknown: false,
                }
            }
            Err(e) => {
                tracing::debug!(op = "overflow spend blocks", error = %e);
                Self {
                    order,
                    blocked: Vec::new(),
                    gate: None,
                    platform,
                    not_plan,
                    kinds_unknown: false,
                }
            }
        }
    }

    /// No overflow profiles, and `o.not_plan` still kept from the plan logins.
    fn without_order(o: Order) -> Self {
        Self {
            order: Vec::new(),
            blocked: Vec::new(),
            gate: None,
            platform: None,
            not_plan: o.not_plan,
            kinds_unknown: false,
        }
    }

    /// Kinds could not be read: nothing may be selected.
    fn unknown() -> Self {
        Self {
            order: Vec::new(),
            blocked: Vec::new(),
            gate: None,
            platform: None,
            not_plan: Vec::new(),
            kinds_unknown: true,
        }
    }

    /// Whether a move onto `target` passes the login check (issue #1804):
    /// always for a profile that is not the API-key one. A failure is
    /// logged with the profile and the reason, never a value.
    pub fn key_check_passes(&self, root: &Path, target: &ProfileName) -> bool {
        if self.platform.as_ref() != Some(target) {
            return true;
        }
        let store = ProfileStore::new(root.to_path_buf());
        match rotate::key_check::configured(&store, target) {
            Ok(_) => true,
            Err(reason) => {
                tracing::warn!(
                    profile = %target,
                    %reason,
                    "auto-switch: API key profile failed its login check; not moving onto it"
                );
                false
            }
        }
    }

    pub fn pool(&self) -> OverflowPool<'_> {
        OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: self.gate,
            platform: self.platform.as_ref(),
            not_plan: &self.not_plan,
            kinds_unknown: self.kinds_unknown,
        }
    }
}
