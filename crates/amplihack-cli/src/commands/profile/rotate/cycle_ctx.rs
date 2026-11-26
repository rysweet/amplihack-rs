//! The state of a rotate cycle and of a watch process's retained readings.

use super::super::cred_identity::{DEFAULT_READ, ReadFn};
use super::super::fresh::{RereadFn, live};
use super::super::identify::Skip;
use super::super::key_check::{self, KeyCheckFn};
use super::super::ledger::RotateState;
use super::super::log::Sink;
use super::super::revalidate;
use super::super::settings::Settings;
use super::MeasureFn;
use crate::commands::profile::auto_switch::kit::{
    Liveness, LoginExchange, host_exchange, scan_system,
};
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
    /// The view of running sessions (a `SessionView`): the host's process
    /// table by default. A test sets a function that returns a table it
    /// wrote in memory.
    pub(in crate::commands::profile::rotate) scan: fn(&Path) -> Liveness,
    /// The atomic exchange of two logins: the host's by default, which
    /// only Linux has. A test sets a fake it holds (issue #2935).
    pub(in crate::commands::profile::rotate) exchange: Arc<dyn LoginExchange>,
    /// Tests inject measurement while the lock is released.
    pub(in crate::commands::profile::rotate) measure: Option<MeasureFn>,
    /// Freshness re-read by the configured method; None reads nothing.
    pub(in crate::commands::profile::rotate) reread: Option<RereadFn>,
    /// Issue #2211: re-read of the live login in `~/.claude`, never its
    /// profile directory; None reads nothing.
    pub(in crate::commands::profile::rotate) reread_live: Option<RereadFn>,
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
    /// Issue #2885: the unreadable-Keychain lines the last cycle's capacity
    /// write gave ([`Ctx::note_keychain`]).
    pub(in crate::commands::profile::rotate) keychain_notes: Vec<String>,
    /// Issue #2820: the `unreadable process` lines the last cycle's first
    /// snapshot gave ([`Ctx::note_unreadable`]). Lines as logged, already
    /// masked; never the raw arguments.
    pub(in crate::commands::profile::rotate) unreadable_notes: Vec<String>,
    /// Issue #2211: the live login's recent readings, kept across cycles.
    pub(in crate::commands::profile::rotate) live_series: live::Series,
    /// Issue #2211: this cycle's projected or substituted live figures.
    pub(in crate::commands::profile::rotate) live_outlook: Option<live::Outlook>,
    /// Issue #2909: the next cycle follows a limit record or a recorded
    /// reset, so it reads the live login and takes the other logins' figures
    /// from `stored` where it can (`event_read`). Only the watch loop sets it.
    pub(in crate::commands::profile::rotate) event: bool,
    /// Issue #2909: the last reading this process took of each login.
    pub(in crate::commands::profile::rotate) stored: super::event_read::Stored,
    /// The clock of the decision; tests set a synthetic one.
    pub(in crate::commands::profile::rotate) now: fn() -> i64,
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
            exchange: host_exchange(),
            measure: None,
            reread: None,
            reread_live: None,
            probe_held: None,
            pause: |_| std::thread::sleep(revalidate::REREAD_PAUSE),
            key_check: key_check::reachable,
            home_key: None,
            in_flight: None,
            identity: DEFAULT_READ,
            identity_budget: super::super::login_identity::Budget::new(),
            read_only: false,
            last_seed_note: None,
            keychain_notes: Vec::new(),
            unreadable_notes: Vec::new(),
            live_series: live::Series::default(),
            live_outlook: None,
            event: false,
            stored: super::event_read::Stored::default(),
            now: crate::commands::profile::auto_switch::kit::now,
        }
    }

    /// Installs the configured re-read hooks for the profile copies (issue
    /// #1870) and for the live login (issue #2211).
    pub(in crate::commands::profile::rotate) fn with_rereads(mut self) -> Self {
        self.reread = Some(super::super::fresh::reread_as_configured);
        self.reread_live = Some(super::super::fresh::reread_live_as_configured);
        self
    }

    /// The probe mode a read-only run forces, whatever the environment says.
    pub(in crate::commands::profile::rotate) fn read_only_mode(&self) -> Option<ProbeMode> {
        self.read_only.then_some(ProbeMode::Direct)
    }

    /// `env`, direct only in a read-only run, so `claude` never starts.
    pub(in crate::commands::profile::rotate) fn read_env(&self, mut env: SourceEnv) -> SourceEnv {
        if let Some(mode) = self.read_only_mode() {
            env.mode = mode;
        }
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

    /// Issue #2885: logs the lines a cycle's capacity write handed back
    /// (`overflow_record::refresh_capacity`), one for each overflow profile
    /// it left out because its macOS Keychain could not be read, so the log
    /// does not leave such a profile looking like one with no login. A line
    /// the previous cycle gave is not logged again: a Keychain that stays
    /// locked is logged once, and one locked again after a cycle in which it
    /// answered is logged again.
    pub(in crate::commands::profile::rotate) fn note_keychain(
        &mut self,
        out: &mut Sink,
        t: i64,
        notes: Vec<String>,
    ) {
        emit_new(out, t, &mut self.keychain_notes, notes);
    }

    /// Issue #2820: logs the `unreadable process` lines of a cycle's first
    /// snapshot (`msg::unreadable_lines`). A line the previous cycle gave is
    /// not logged again, so a long-lived process is named once; one that
    /// went away and came back is named again, as is every one after a
    /// restart of the rotator.
    pub(in crate::commands::profile::rotate) fn note_unreadable(
        &mut self,
        out: &mut Sink,
        t: i64,
        lines: Vec<String>,
    ) {
        emit_new(out, t, &mut self.unreadable_notes, lines);
    }

    pub(in crate::commands::profile::rotate) fn stopping(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }
}

/// Emits each of `lines` that is not in `prev`, then makes `lines` the new
/// `prev`: the log-once rule of the notes a cycle repeats.
fn emit_new(out: &mut Sink, t: i64, prev: &mut Vec<String>, lines: Vec<String>) {
    for line in lines.iter().filter(|l| !prev.contains(l)) {
        out.emit(t, line);
    }
    *prev = lines;
}
