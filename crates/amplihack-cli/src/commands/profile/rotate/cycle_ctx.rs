//! The state of a rotate cycle and of a watch process's retained readings.

use super::super::cred_identity::{DEFAULT_READ, ReadFn};
use super::super::fresh::RereadFn;
use super::super::identify::Skip;
use super::super::key_check::{self, KeyCheckFn};
use super::super::ledger::RotateState;
use super::super::revalidate;
use super::super::settings::Settings;
use super::MeasureFn;
use crate::commands::profile::auto_switch::kit::{Liveness, now, scan_system};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::no_refresh_dirs;
use crate::commands::profile::usage_source::SourceEnv;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cycle state and the watch process's retained readings.
pub struct Ctx {
    pub store: ProfileStore,
    pub rs: RotateState,
    pub home: PathBuf,
    pub settings: Settings,
    pub stop: Option<Arc<AtomicBool>>,
    pub(in crate::commands::profile::rotate) last_skips: Option<Vec<(ProfileName, Skip)>>,
    pub(in crate::commands::profile::rotate) last_proto: Option<&'static str>,
    pub(super) last_gate: Option<String>,
    pub(in crate::commands::profile::rotate) failed_live: u32,
    /// Finds running sessions; tests inject synthetic liveness.
    pub(in crate::commands::profile::rotate) scan: fn(&Path) -> Liveness,
    /// Tests inject measurement while the lock is released.
    pub(in crate::commands::profile::rotate) measure: Option<MeasureFn>,
    /// Freshness re-read by the configured method; None reads nothing.
    pub(in crate::commands::profile::rotate) reread: Option<RereadFn>,
    /// Replaces a held cycle's spend-limit probe when set; only tests do.
    pub(in crate::commands::profile::rotate) probe_held: Option<super::super::held::HeldProbeFn>,
    /// Waits before step 6 reads the state again; tests act in it instead.
    pub(in crate::commands::profile::rotate) pause: fn(&Ctx),
    /// Login check before a move onto the API-key profile.
    pub(in crate::commands::profile::rotate) key_check: KeyCheckFn,
    /// The `$HOME` key setting `note_home_key` last logged.
    pub(in crate::commands::profile::rotate) home_key: Option<&'static str>,
    /// Usage reads carried across watch cycles.
    pub(in crate::commands::profile::rotate) in_flight:
        Option<std::sync::Mutex<super::super::measure::Reads>>,
    /// Bounded login identity reader, injected by tests.
    pub(in crate::commands::profile::rotate) identity: ReadFn,
    pub(in crate::commands::profile::rotate) identity_budget: super::super::login_identity::Budget,
    /// Issue #1962: `--dry-run`. Every read is direct, with no refresh, so a
    /// token is never refreshed and `claude` never starts.
    pub(in crate::commands::profile::rotate) read_only: bool,
    /// The seed note this process last logged; a repeat is not logged.
    pub(in crate::commands::profile::rotate) last_seed_note: Option<String>,
    /// Issue #2013 (D12): the clock freshness, the alert limit and the home
    /// repair read. The system clock; only `#[cfg(test)]` code replaces it.
    pub(in crate::commands::profile::rotate) clock: fn() -> i64,
    /// When the current cycle's locked prelude started, by [`Self::clock`]:
    /// alert-limit keys not seen since then are cleared when it is saved.
    pub(in crate::commands::profile::rotate) cycle_started_at: i64,
}

impl Ctx {
    pub fn new(store: ProfileStore, home: PathBuf, settings: Settings) -> Self {
        let rs = RotateState::new(store.root());
        Self {
            store,
            rs,
            home,
            settings,
            stop: None,
            last_skips: None,
            last_proto: None,
            last_gate: None,
            failed_live: 0,
            scan: scan_system,
            measure: None,
            reread: None,
            probe_held: None,
            pause: |_| std::thread::sleep(revalidate::REREAD_PAUSE),
            key_check: key_check::reachable,
            home_key: None,
            in_flight: None,
            identity: DEFAULT_READ,
            identity_budget: super::super::login_identity::Budget::new(),
            read_only: false,
            last_seed_note: None,
            clock: now,
            cycle_started_at: 0,
        }
    }

    /// The probe mode a read-only run forces, whatever the environment says.
    pub(in crate::commands::profile::rotate) fn read_only_mode(&self) -> Option<ProbeMode> {
        self.read_only.then_some(ProbeMode::Direct)
    }

    /// `env`, direct only in a read-only run, so `claude` never starts.
    /// Issue #2013 (D5): a passive reading is accepted only within the age
    /// limit ([`super::super::fresh::max_age`]), so the cycle never measures
    /// with one it may not decide on.
    pub(in crate::commands::profile::rotate) fn read_env(&self, mut env: SourceEnv) -> SourceEnv {
        if let Some(mode) = self.read_only_mode() {
            env.mode = mode;
        }
        let cap = u64::try_from(super::super::fresh::max_age(&self.settings)).unwrap_or(0);
        env.passive_max_age = env.passive_max_age.min(cap);
        env
    }

    /// The directories a read may not refresh. `None` refreshes none, as a
    /// read-only run must.
    pub(in crate::commands::profile::rotate) fn no_refresh_dirs(&self) -> Option<Vec<String>> {
        if self.read_only {
            return None;
        }
        no_refresh_dirs(&self.store)
    }

    /// The time now, by the injected clock (D12).
    pub(in crate::commands::profile::rotate) fn now(&self) -> i64 {
        (self.clock)()
    }

    pub(in crate::commands::profile::rotate) fn stopping(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }
}
