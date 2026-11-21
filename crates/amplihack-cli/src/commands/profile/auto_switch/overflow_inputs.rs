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
//! Issue #1700: the order is the rotator's (`overflow::tiers_of`): the credit
//! profiles, then the first API-key profile, whose block counts as one of
//! the spend blocks. A corrupt `.auto-switch` gives no order at all.
//! Issue #1804: only an API-key profile that `overflow=` marks is in it, and
//! every API-key profile is in `not_plan`, so none counts as a plan login.
//! Kinds that cannot be read mean no profile is known to be free of per-token
//! billing, so `kinds_unknown` stops every selection and capacity write.
//! `no_login` lists the profiles the rotator skips as `no login` or
//! `not oauth`, so a missing reading means the same thing to both, and the
//! logins the rotator last measured with no room (`rotate::no_room`).

use super::candidate::OverflowPool;
use super::config::Config;
use super::own_login::OwnLogins;
use crate::commands::profile::keychain::Cause;
use crate::commands::profile::kind::{self, Ctx, Resolved};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{self, Gate};
use crate::commands::profile::rotate::ledger::RotateState;
use crate::commands::profile::rotate::{self, CheckNote};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd;
use std::path::Path;

/// Why a move onto a profile can find no login ([`login_gap`]). The two are
/// kept apart because the remedies differ: a profile with no login is logged
/// in, whereas a Keychain that could not be read is unlocked (issue #2885).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginGap {
    /// The profile holds no login of its own.
    NoLogin,
    /// macOS: no login file, and the Keychain could not be read, so whether
    /// the profile holds a login is not known. It is passed over all the
    /// same, and never reported as [`LoginGap::NoLogin`].
    KeychainUnreadable(Cause),
}

impl LoginGap {
    /// What `profile overflow add` says, after `warning: `, about a marked
    /// profile nothing can move onto. The Keychain text names the cause and
    /// the fixed remedy, and nothing about the account.
    pub fn overflow_warning(self, name: &ProfileName) -> String {
        match self {
            Self::NoLogin => format!(
                "{name} has no login file, so it cannot be used as overflow until it has one: neither the rotator nor auto-switch can move onto it, and the capacity file does not count it"
            ),
            Self::KeychainUnreadable(cause) => format!(
                "{name} has no login file and the macOS Keychain could not be read ({cause}), so it cannot be used as overflow until the Keychain can be read: auto-switch passes it over, and the capacity file does not count it. Unlock the login Keychain; 'amplihack profile auto-switch status' then shows where its login is"
            ),
        }
    }
}

/// Why a move onto `name` can find no login, or `None` when it can find one:
/// the profile holds one of its own, or the rotator has already moved it
/// into `~/.claude`. Selection asks the first half
/// (`rerun::nested_candidate`) and a swap needs it too.
///
/// A login of its own is a login file. On macOS, where the login is in the
/// Keychain, it is what the launch guard counts (issue #2885): an item in
/// the Keychain for a profile with an identifiable account, or an API-key
/// profile. A Keychain that cannot be read is no login, and is named as the
/// reason. The Keychain is asked once, with the process's own home.
pub fn login_gap(root: &Path, name: &ProfileName) -> Option<LoginGap> {
    let store = ProfileStore::new(root.to_path_buf());
    let home = usage_cmd::home_dir();
    gap_of(root, &OwnLogins::new(&store, home.as_deref()), name)
}

/// [`login_gap`] with the own-login answers shared between calls.
fn gap_of(root: &Path, own: &OwnLogins<'_>, name: &ProfileName) -> Option<LoginGap> {
    let takes = own.of(name).holds()
        || RotateState::new(root)
            .load_ledger()
            .ok()
            .flatten()
            .is_some_and(|l| l.live.as_deref() == Some(name.as_str()));
    if takes {
        return None;
    }
    // `of` is memoised, so this asks the Keychain nothing more.
    Some(
        own.unreadable(name)
            .map_or(LoginGap::NoLogin, LoginGap::KeychainUnreadable),
    )
}

pub struct OverflowInputs {
    pub order: Vec<ProfileName>,
    pub blocked: Vec<String>,
    pub gate: Option<Gate>,
    /// The API-key profile, last in `order`.
    pub platform: Option<ProfileName>,
    /// Every API-key profile, marked or not: none is a plan login.
    pub not_plan: Vec<ProfileName>,
    /// See [`OverflowPool::no_login`]. Worked out only with a marked key.
    pub no_login: Vec<ProfileName>,
    /// Kinds could not be read: `order` and `not_plan` are empty, and
    /// nothing may be selected.
    pub kinds_unknown: bool,
}

/// What [`order_of`] reads from `.auto-switch` and the profiles.
struct Order {
    order: Vec<ProfileName>,
    platform: Option<ProfileName>,
    not_plan: Vec<ProfileName>,
    no_login: Vec<ProfileName>,
}

/// The profiles whose missing reading says nothing. Two kinds:
///
/// - Those the rotator's login tests skip as `no login` or `not oauth`
///   (`rotate::login_unusable`), tested on the kinds `order_of` has already
///   resolved, so no login or settings file is read twice. The rotator's
///   live profile stays out of that test: its login is in `~/.claude`, so
///   its own directory has none. macOS keeps logins in the Keychain, so a
///   missing file proves nothing there and that test lists nothing, which
///   only holds the key back. No Keychain question is asked for this list
///   (issue #2885), so no Keychain answer can release the hold on the key.
/// - Those the rotator's last measuring cycle found with no room, while that
///   record holds (`rotate::no_room`). Auto-switch cannot read the live
///   login itself: the status line records only a session's own
///   `CLAUDE_CONFIG_DIR` profile and `profile usage` probes each profile's
///   own directory. The rotator writes that record to its own ledger every
///   cycle, whatever the capacity state and whether or not it could use the
///   API key, and no other writer rewrites it. It names every login the
///   cycle measured, so a login the rotator has just moved off stays
///   measured until the record expires; after that, and for a login the
///   rotator found with room, the login is unread until a reading is
///   recorded for it (a session on it, or `profile usage`), the rotator
///   measures it full again, or it is excluded.
///
/// Only profiles with no reading of their own consult this list
/// (`candidate::unread_plan`), so it never overrides a recorded reading.
fn no_login_of(
    root: &Path,
    store: &ProfileStore,
    kinds: &Ctx,
    resolved: &[(ProfileName, Resolved)],
    now: i64,
) -> Vec<ProfileName> {
    let live = kinds.ledger_live();
    let mut skipped: Vec<ProfileName> = if kinds.macos() {
        Vec::new()
    } else {
        resolved
            .iter()
            .filter(|(n, _)| live != Some(n))
            .filter(|(n, r)| rotate::login_unusable(store, n, r))
            .map(|(n, _)| n.clone())
            .collect()
    };
    for n in rotator_no_room(root, now) {
        if !skipped.contains(&n) {
            skipped.push(n);
        }
    }
    skipped
}

/// The profiles the rotator's ledger records as measured with no room, while
/// that record holds at `now`. An unreadable ledger, or none, records none,
/// so every unread login stays unread.
fn rotator_no_room(root: &Path, now: i64) -> Vec<ProfileName> {
    let ledger = match RotateState::new(root).load_ledger() {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!(op = "rotator no-room record", error = %e);
            None
        }
    };
    let Some(l) = ledger else {
        return Vec::new();
    };
    rotate::plan_room::no_room::holding(l.no_room.as_ref(), now)
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect()
}

/// The overflow order for `cfg` under `root`, by the rotator's rule
/// (`overflow::tiers_of`): its credit entries, then the first marked
/// API-key profile by name; that profile; and every API-key profile. An
/// API-key profile that `overflow=` does not mark is not in the order
/// (issue #1804). `None` when kinds cannot be read or the root cannot be
/// listed. Each profile's kind is resolved once ([`Ctx::try_resolve_all`]),
/// which parses `.claude.json` only for a profile with no login file. With
/// a marked API key, `no_login` adds a directory check and a metadata read
/// of each login file, and one read of the rotate ledger.
fn order_of(root: &Path, cfg: &Config, home: Option<&Path>, now: i64) -> Option<Order> {
    let store = ProfileStore::new(root.to_path_buf());
    let kinds = Ctx::load(&store, home);
    if kinds.corrupt() {
        return None;
    }
    // Each profile's kind is read once, for `not_plan` and `no_login` both.
    let resolved = kinds.try_resolve_all(&store)?;
    let not_plan = kind::platforms_of(&resolved);
    let (tiers, _) = overflow::tiers_of(cfg.overflow_order(), &not_plan);
    let platform = tiers
        .iter()
        .find(|e| e.tier == overflow::PLATFORM_TIER)
        .map(|e| e.name.clone());
    // Only the API key waits for unread logins: skip the reads without one.
    let no_login = match platform {
        Some(_) => no_login_of(root, &store, &kinds, &resolved, now),
        None => Vec::new(),
    };
    Some(Order {
        order: tiers.into_iter().map(|e| e.name).collect(),
        platform,
        not_plan,
        no_login,
    })
}

impl OverflowInputs {
    /// Reads what `cfg` needs. Kinds are always read, since an API-key
    /// profile must stay out of the plan logins with nothing marked; the gate
    /// and spend blocks are read only with overflow profiles.
    pub fn load(root: &Path, cfg: &Config, now: i64) -> Self {
        Self::load_at(root, cfg, now, usage_cmd::home_dir().as_deref())
    }

    /// [`Self::load`] with the home directory given, for a caller that holds
    /// its own.
    pub fn load_at(root: &Path, cfg: &Config, now: i64, home: Option<&Path>) -> Self {
        let Some(o) = order_of(root, cfg, home, now) else {
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
        let Some(o) = order_of(root, cfg, None, now) else {
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
            no_login,
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
                    no_login,
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
                    no_login,
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
            no_login: Vec::new(),
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
            no_login: Vec::new(),
            kinds_unknown: true,
        }
    }

    /// What a move onto `target` passed, or `None` when the login check
    /// refuses it (issue #1804). Only the API-key profile has that check: any
    /// other profile passes, as [`CheckNote::NoSpendBlock`]. Auto-switch runs
    /// an API-key profile only with `CLAUDE_CONFIG_DIR` set to its directory,
    /// so the profile's own files are the right ones here; the rotator, which
    /// moves into `~/.claude`, uses `key_check::reachable`. A failure is
    /// logged with the profile and the reason, never a value.
    pub fn check_note(&self, root: &Path, target: &ProfileName) -> Option<CheckNote> {
        if self.platform.as_ref() != Some(target) {
            return Some(CheckNote::NoSpendBlock);
        }
        let store = ProfileStore::new(root.to_path_buf());
        match rotate::key_check::configured(&store, target) {
            Ok(detail) => Some(CheckNote::Key(detail)),
            Err(reason) => {
                tracing::warn!(
                    profile = %target,
                    %reason,
                    "auto-switch: API key profile failed its login check; not moving onto it"
                );
                None
            }
        }
    }

    /// Whether the capacity file may count the API-key profile (issue
    /// #1804), by the check of whoever would make the move: `reachable` with
    /// a rotator running, so the status line and the rotator write the same
    /// state, else the profile's own files. No home directory fails.
    pub fn key_counts(&self, root: &Path, home: Option<&Path>, rotator: bool) -> bool {
        let Some(key) = &self.platform else {
            return true;
        };
        if !rotator {
            // Not `check_note`: this runs on every status line.
            let store = ProfileStore::new(root.to_path_buf());
            return rotate::key_check::configured(&store, key).is_ok();
        }
        home.is_some_and(|h| rotate::key_check::reachable(h).is_ok())
    }

    /// Takes the API-key profile out of what the capacity file counts, as a
    /// block does. Only the capacity writers call this; the selection runs
    /// its own check.
    pub fn refuse_key(&mut self) {
        if let Some(key) = &self.platform {
            self.blocked.push(key.to_string());
        }
    }

    /// Takes every overflow profile for which [`login_gap`] names a reason
    /// out of what the capacity file counts, so the file does not say `ok`
    /// for a profile selection would pass over. Only the capacity writers
    /// call this.
    ///
    /// Returns the profiles taken out because their Keychain could not be
    /// read, each with the cause (issue #2885). Such a profile may well hold
    /// a login, and the launch guard and `auto-switch status` say so, so the
    /// writer hands the cause to its caller (`capacity::keychain_notes`)
    /// rather than leave the profile looking like one with no login.
    #[must_use]
    pub fn refuse_no_login(&mut self, root: &Path) -> Vec<(ProfileName, Cause)> {
        let store = ProfileStore::new(root.to_path_buf());
        let home = usage_cmd::home_dir();
        let own = OwnLogins::new(&store, home.as_deref());
        let mut unreadable = Vec::new();
        for name in &self.order {
            let Some(gap) = gap_of(root, &own, name) else {
                continue;
            };
            if let LoginGap::KeychainUnreadable(cause) = gap {
                unreadable.push((name.clone(), cause));
            }
            self.blocked.push(name.to_string());
        }
        unreadable
    }

    pub fn pool(&self) -> OverflowPool<'_> {
        OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: self.gate,
            platform: self.platform.as_ref(),
            not_plan: &self.not_plan,
            no_login: &self.no_login,
            kinds_unknown: self.kinds_unknown,
        }
    }
}
