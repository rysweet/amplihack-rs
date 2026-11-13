//! `amplihack profile rotate` (#1592): keeps the login in `~/.claude` on the account with the
//! most capacity left; `profile switch` shares the swap. See docs/reference/profile-command.md.
#[cfg(unix)]
mod account;
#[cfg(unix)]
mod adopt;
#[cfg(unix)]
mod alert;
#[cfg(unix)]
mod autostart;
#[cfg(unix)]
pub(super) mod backup;
#[cfg(unix)]
mod candidates;
#[cfg(unix)]
mod capacity;
#[cfg(unix)]
pub(in crate::commands::profile) mod change_log;
#[cfg(unix)]
mod control;
#[cfg(unix)]
pub(super) mod cred_identity;
#[cfg(unix)]
pub(super) mod custody;
#[cfg(unix)]
mod cycle;
#[cfg(unix)]
mod cycle_attempts;
#[cfg(unix)]
pub(super) mod daemon;
#[cfg(unix)]
mod damp;
#[cfg(unix)]
pub(super) mod decide;
#[cfg(unix)]
mod dry_run;
#[cfg(unix)]
mod duplicate;
#[cfg(unix)]
mod exhausted;
#[cfg(unix)]
mod fresh;
#[cfg(unix)]
mod fresh_msg;
#[cfg(unix)]
mod gate_record;
#[cfg(unix)]
mod guide;
#[cfg(unix)]
mod hand_fields;
#[cfg(unix)]
mod handfix;
#[cfg(unix)]
mod handfix_msg;
#[cfg(unix)]
mod handfix_plan;
#[cfg(unix)]
mod held;
#[cfg(unix)]
mod history;
#[cfg(unix)]
mod hook_heal;
#[cfg(unix)]
pub(super) mod identify;
#[cfg(unix)]
mod identity_claims;
#[cfg(unix)]
mod inflight;
#[cfg(unix)]
pub(super) mod key_check;
#[cfg(unix)]
pub(super) mod ledger;
#[cfg(unix)]
mod log;
#[cfg(unix)]
mod login_identity;
#[cfg(unix)]
pub(super) mod login_state;
#[cfg(unix)]
mod lost;
#[cfg(unix)]
mod measure;
#[cfg(unix)]
mod observe;
#[cfg(unix)]
mod older_readers;
#[cfg(unix)]
mod overflow;
#[cfg(unix)]
mod overflow_msg;
#[cfg(unix)]
mod overflow_record;
#[cfg(unix)]
mod owner;
#[cfg(unix)]
mod pin;
#[cfg(unix)]
pub(super) mod plan_room;
#[cfg(unix)]
pub(super) mod platform_block;
#[cfg(unix)]
mod precheck;
#[cfg(unix)]
mod prelude;
#[cfg(unix)]
mod recover;
#[cfg(unix)]
mod revalidate;
#[cfg(unix)]
pub(super) mod seed;
#[cfg(unix)]
mod sessions_daily;
#[cfg(unix)]
mod settings;
#[cfg(unix)]
mod spend_blocks;
#[cfg(unix)]
mod spending;
#[cfg(unix)]
mod swap;
#[cfg(unix)]
mod swap_authorize;
#[cfg(unix)]
mod swap_back;
#[cfg(unix)]
mod swap_check;
#[cfg(unix)]
mod swap_expect;
#[cfg(unix)]
mod swap_finish;
#[cfg(unix)]
mod swap_record;
#[cfg(unix)]
mod switch;
#[cfg(unix)]
mod verified_dirs;
pub use super::rotate_args::{RotateAction, RotateArgs, TuneFlags};
pub use candidates::login_unusable;
pub use overflow_msg::{CheckNote, alert_key_held_session, per_token_off_key, per_token_reason};
pub use overflow_record::append_to_log;
#[cfg(all(test, unix))]
pub(super) use swap::DOCUMENTED as DOCUMENTED_REFUSALS;
#[cfg(all(test, unix))]
mod tests;
#[cfg(all(test, unix))]
mod tests_autostart;
#[cfg(all(test, unix))]
mod tests_daemon;
#[cfg(all(test, unix))]
mod tests_handfix;
// These cycle suites already gate every test on Linux; their fixtures belong
// under the same gate. Mixed-platform suites below keep their portable tests.
#[cfg(all(test, target_os = "linux"))]
mod tests_handfix_cycle;
#[cfg(all(test, target_os = "linux"))]
mod tests_handfix_overflow;
#[cfg(all(test, target_os = "linux"))]
mod tests_home_unreadable;
#[cfg(all(test, unix))]
mod tests_identify;
#[cfg(all(test, unix))]
mod tests_issue_1804;
#[cfg(all(test, target_os = "linux"))]
mod tests_issue_1804_cycle;
#[cfg(all(test, target_os = "linux"))]
mod tests_issue_1845_cycle;
#[cfg(all(test, unix))]
mod tests_issue_1845_swap;
#[cfg(all(test, unix))]
mod tests_issue_1870;
#[cfg(all(test, unix))]
mod tests_issue_1870_cycle;
#[cfg(all(test, unix))]
mod tests_issue_1870_probe;
#[cfg(all(test, unix))]
mod tests_issue_1886;
#[cfg(all(test, unix))]
mod tests_issue_1886_cycle;
#[cfg(all(test, unix))]
mod tests_key_check;
#[cfg(all(test, unix))]
mod tests_log;
#[cfg(all(test, unix))]
mod tests_measure;
#[cfg(all(test, unix))]
mod tests_pin;
#[cfg(all(test, target_os = "linux"))]
mod tests_pin_overflow;
#[cfg(all(test, unix))]
mod tests_recover;
#[cfg(all(test, target_os = "linux"))]
mod tests_revalidate;
#[cfg(all(test, unix))]
mod tests_revalidate_cause;
#[cfg(all(test, unix))]
mod tests_revalidate_unit;
#[cfg(all(test, unix))]
mod tests_sessions_daily;
#[cfg(test)]
mod tests_source_scan;
#[cfg(all(test, unix))]
mod tests_swap;
#[cfg(all(test, unix))]
mod tests_swap_refused;
#[cfg(all(test, unix))]
mod tests_switch;
#[cfg(all(test, unix))]
mod tests_switch_candidates;
#[cfg(all(test, unix))]
mod tests_switch_hold;
#[cfg(all(test, unix))]
mod tests_switch_race;
#[cfg(unix)]
pub(super) use self::{autostart::autostart_line, identify::excluded_offline::*};
#[cfg(unix)]
pub(super) use self::{identify::is_live, observe::Probes, platform_block::platform_live_line};
use super::{name::ProfileName, root_config::RootSource, store::ProfileStore};
#[cfg(all(test, unix))]
pub(super) use gate_record::effective_at as overflow_gate_at;
#[cfg(unix)]
pub(super) use gate_record::{GateSource, effective as overflow_gate, line as overflow_gate_line};
#[cfg(unix)]
pub(crate) use {autostart::autostart_on_launch, older_readers::warn_foreign_rotator};
#[cfg(unix)]
pub(super) use {control::clear_overflow_block, control::overflow_blocks, log::timestamp};
/// Warn when another binary's rotator ignores the switch hold (#1691).
#[cfg(unix)]
const FOREIGN_IGNORES_HOLD: &str = "runs another amplihack binary and ignores the switch \
    hold; restart it: amplihack profile rotate stop && amplihack profile rotate start";
/// Switch-hold warning, comparing the running rotator with this executable.
#[cfg(unix)]
fn foreign_note_for(
    rs: &ledger::RotateState,
    proc_root: &std::path::Path,
    exe: Option<&std::path::Path>,
) -> Option<String> {
    rs.foreign_rotator_at(proc_root, exe).map(|pid| {
        format!("amplihack: the Claude login rotator (pid {pid}) {FOREIGN_IGNORES_HOLD}")
    })
}

/// Warn after a successful switch when the running rotator ignores the hold.
#[cfg(unix)]
fn warn_switch_hold(root: &std::path::Path) {
    let exe = std::env::current_exe().ok();
    let rs = ledger::RotateState::new(root);
    if let Some(note) = foreign_note_for(&rs, &daemon::proc_root(), exe.as_deref()) {
        eprintln!("{note}");
    }
}

/// Starts the rotator from a launch when auto-switch is on; see
/// `autostart`. Nothing to start off Unix.
#[cfg(not(unix))]
pub(crate) fn autostart_on_launch(_docker_flag: bool) {}

pub fn run(store: &ProfileStore, source: RootSource, args: RotateArgs) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let env = |k: &str| std::env::var_os(k);
        let home = settings::home_dir();
        match args.action {
            None => {
                let s = settings::resolve(&args.tune, None, &env)?;
                let mode = if args.dry_run {
                    cycle::Mode::DryRun
                } else {
                    cycle::Mode::OneShot
                };
                if mode == cycle::Mode::OneShot {
                    warn_foreign_rotator(Some(store.root()));
                }
                let mut ctx = cycle::Ctx::new(store.clone(), home?, s).with_rereads();
                hook_heal::after_cycle(cycle::run_cycle(&mut ctx, mode)?, &ctx);
            }
            Some(RotateAction::Start(w)) => {
                if w.supervised.is_some() {
                    let why = "--supervised applies to rotate watch only";
                    return Err(super::ProfileError::RotateRefused(why.into()).into());
                }
                let s = settings::resolve(&w.tune, w.interval.as_deref(), &env)?;
                daemon::start(store, &home?, &s)?;
            }
            Some(RotateAction::Watch(w)) => {
                owner::check_flag(&w.supervised)?;
                let s = settings::resolve(&w.tune, w.interval.as_deref(), &env)?;
                daemon::watch(store, home?, s, w.spawned, w.supervised.clone())?;
            }
            Some(RotateAction::Stop) => control::stop(store, home.as_deref().ok())?,
            Some(RotateAction::Status) => control::status(store, home.as_deref().ok(), source),
            Some(RotateAction::Autostart { state }) => autostart::cli(store, state.as_deref())?,
            Some(RotateAction::Owner { action }) => owner::cli(store, action.is_some())?,
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (store, source, args);
        Err(super::ProfileError::RotateUnsupported.into())
    }
}

/// `profile switch NAME` swaps the login and pins rotation. A later failed
/// write is an error, not a refusal. If the login cannot move, NAME is still
/// made the default for new launches and printed, then the refusal exits 1.
pub(in crate::commands::profile) fn switch_live(
    store: &ProfileStore,
    name: &ProfileName,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let home = settings::home_dir()?;
        // Judged before the move, from the profile's own login (or from
        // `~/.claude` when it is already live), as the kind guard does.
        let platform = super::kind::kind_of(store, name, &home) == super::kind::Kind::Platform;
        let note = || {
            if platform {
                println!("{}", log::msg::switch_platform_note(name));
            }
        };
        match switch::switch_live(store, &home, name)? {
            Ok(switch::Switched::DefaultOnly { reason, held }) => {
                println!("Default for new launches: {name}");
                match held {
                    Some(h) => println!(
                        "Live login not moved; rotation still held on {h} until \
                         `amplihack profile switch --none`."
                    ),
                    None => println!("Live login not moved; rotation not held."),
                }
                Err(super::ProfileError::RotateRefused(reason).into())
            }
            Ok(switch::Switched::Moved { from, to }) => {
                println!("Live login moved: {from} -> {to}");
                print_held(store, name);
                note();
                Ok(())
            }
            Ok(switch::Switched::AlreadyLive(n)) => {
                println!("{n} is already the live login; nothing moved.");
                print_held(store, name);
                note();
                Ok(())
            }
            Err(f) => Err(anyhow::anyhow!(
                "the live login moved to {to}, but writing the {what} failed: {err}; \
                 run `amplihack profile switch {to}` again",
                to = f.to,
                what = f.what,
                err = f.err
            )),
        }
    }
    #[cfg(not(unix))]
    {
        // No live login can be moved here, but the default for new launches
        // is still set, as `switch` always did.
        crate::commands::profile::pointer::write(store.root(), name)?;
        println!("Default for new launches: {name}");
        println!("Live login not moved; rotation not held.");
        Err(super::ProfileError::RotateUnsupported.into())
    }
}

/// The last two lines of a switch that moved the login or found it live.
#[cfg(unix)]
fn print_held(store: &ProfileStore, name: &ProfileName) {
    println!("Default for new launches: {name}");
    println!("Rotation held on {name} until `amplihack profile switch --none`.");
    warn_switch_hold(store.root());
}

/// What `profile switch --none` removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Released {
    /// Something was at the rotation pin path.
    pub pin: bool,
    /// Something was at the default pointer path.
    pub default: bool,
}

/// Removes the rotation pin and default pointer under the rotate lock.
pub(in crate::commands::profile) fn release(
    store: &ProfileStore,
) -> Result<Released, super::ProfileError> {
    #[cfg(unix)]
    {
        switch::release(store)
    }
    #[cfg(not(unix))]
    {
        let default = super::pointer::exists(store.root());
        super::pointer::clear(store.root())?;
        Ok(Released {
            pin: false,
            default,
        })
    }
}
