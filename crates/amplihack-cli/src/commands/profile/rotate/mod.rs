//! `amplihack profile rotate` (issue #1592): keeps the login in `~/.claude`
//! on the account with the most capacity left, by moving logins between
//! `~/.claude` and the profiles. One-shot, `--dry-run`, and a background
//! `watch` with `start`, `stop` and `status`, which launches start on their
//! own while auto-switch is on (`autostart`). `profile switch NAME` moves a
//! login with the same swap and pins rotation to it (`switch`, `pin`). See
//! the Rotate section of docs/reference/profile-command.md.
//!
//! Login files are never opened, read, copied or hashed: they are checked
//! with `lstat` and moved with rename and exchange calls only.

#[cfg(unix)]
mod alert;
#[cfg(unix)]
mod autostart;
#[cfg(unix)]
mod backup;
#[cfg(unix)]
mod candidates;
#[cfg(unix)]
mod control;
#[cfg(unix)]
mod cycle;
#[cfg(unix)]
mod daemon;
#[cfg(unix)]
mod damp;
#[cfg(unix)]
mod decide;
#[cfg(unix)]
mod dry_run;
#[cfg(unix)]
mod gate_record;
#[cfg(unix)]
mod guide;
#[cfg(unix)]
mod hand_fields;
#[cfg(unix)]
mod handfix;
#[cfg(unix)]
mod held;
#[cfg(unix)]
mod holders;
#[cfg(unix)]
pub(super) mod identify;
#[cfg(unix)]
pub(super) mod key_check;
#[cfg(unix)]
pub(super) mod ledger;
#[cfg(unix)]
mod log;
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
mod settings;
#[cfg(unix)]
mod spend_blocks;
#[cfg(unix)]
mod swap;
#[cfg(unix)]
mod swap_check;
#[cfg(unix)]
mod switch;
#[cfg(all(test, unix))]
pub(super) use swap::DOCUMENTED as DOCUMENTED_REFUSALS;

pub use super::rotate_args::{RotateAction, RotateArgs, TuneFlags};

#[cfg(all(test, unix))]
mod tests_autostart;
#[cfg(all(test, unix))]
mod tests_daemon;
#[cfg(all(test, unix))]
mod tests_damp;
#[cfg(all(test, unix))]
mod tests_decide;
#[cfg(all(test, unix))]
mod tests_gate_record;
#[cfg(all(test, unix))]
mod tests_handfix;
#[cfg(all(test, unix))]
mod tests_handfix_cycle;
#[cfg(all(test, unix))]
mod tests_handfix_overflow;
#[cfg(all(test, unix))]
mod tests_home_unreadable;
#[cfg(all(test, unix))]
mod tests_identify;
#[cfg(all(test, unix))]
mod tests_issue_1804;
#[cfg(all(test, unix))]
mod tests_issue_1804_cycle;
#[cfg(all(test, unix))]
mod tests_issue_1804_msg;
#[cfg(all(test, unix))]
mod tests_key_check;
#[cfg(all(test, unix))]
mod tests_ledger;
#[cfg(all(test, unix))]
mod tests_live_readers;
#[cfg(all(test, unix))]
mod tests_log;
#[cfg(all(test, unix))]
mod tests_overflow;
#[cfg(all(test, unix))]
mod tests_pin;
#[cfg(all(test, target_os = "linux"))]
mod tests_pin_overflow;
#[cfg(all(test, unix))]
mod tests_pin_platform;
#[cfg(all(test, unix))]
mod tests_platform;
#[cfg(all(test, unix))]
mod tests_platform_block;
#[cfg(all(test, unix))]
mod tests_platform_identify;
#[cfg(all(test, unix))]
mod tests_recover;
#[cfg(all(test, unix))]
mod tests_revalidate;
#[cfg(all(test, unix))]
mod tests_revalidate_cause;
#[cfg(all(test, unix))]
mod tests_revalidate_unit;
#[cfg(all(test, unix))]
mod tests_settings;
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

use super::name::ProfileName;
use super::root_config::RootSource;
use super::store::ProfileStore;

#[cfg(unix)]
pub(in crate::commands::profile) use autostart::autostart_line;
#[cfg(unix)]
pub(crate) use autostart::autostart_on_launch;
#[cfg(unix)]
pub(super) use control::{clear_overflow_block, overflow_blocks};
#[cfg(unix)]
pub(super) use log::timestamp;
#[cfg(unix)]
pub(crate) use older_readers::warn_foreign_rotator;
#[cfg(unix)]
pub(super) use platform_block::platform_live_line;

// Auto-switch's overflow gate (`gate_record`): the running rotator's own
// settings, or, when no rotator runs, the `AMPLIHACK_ROTATE_*` variables.
#[cfg(all(test, unix))]
pub(super) use gate_record::effective_at as overflow_gate_at;
#[cfg(unix)]
pub(super) use gate_record::{GateSource, effective as overflow_gate, line as overflow_gate_line};

/// The tail of the warning printed after `profile switch NAME` (issue
/// #1691) while a rotator from another amplihack binary runs: one started
/// before the upgrade does not read the switch hold and may swap away.
#[cfg(unix)]
const FOREIGN_IGNORES_HOLD: &str = "runs another amplihack binary and ignores the switch \
    hold; restart it: amplihack profile rotate stop && amplihack profile rotate start";

/// The switch-hold warning for the rotator of `rs`, judged against
/// `proc_root` and this process's executable `exe`; `None` when no rotator
/// runs or it runs this binary.
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

/// Whether rotate live-profile detection names `name` as the login in
/// `<home>/.claude` (issue #1700: the launch guard's "held"). Any error, a
/// hand change, an ambiguous or an adopted login is not.
#[cfg(unix)]
pub(super) fn is_live(
    store: &super::store::ProfileStore,
    home: Option<&std::path::Path>,
    name: &super::name::ProfileName,
) -> bool {
    let Some(home) = home else {
        return false;
    };
    let rs = ledger::RotateState::new(store.root());
    matches!(identify::live(store, &rs, home), Ok((identify::LiveId::Known(n), _)) if &n == name)
}

/// After a successful `profile switch NAME`: one line on stderr when the
/// running rotator would ignore the hold. The switch itself succeeded, so
/// this never changes the exit code.
#[cfg(unix)]
fn warn_switch_hold(root: &std::path::Path) {
    let exe = std::env::current_exe().ok();
    let rs = ledger::RotateState::new(root);
    if let Some(note) = foreign_note_for(&rs, std::path::Path::new("/proc"), exe.as_deref()) {
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
        let home = home_dir();
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
                let mut ctx = cycle::Ctx::new(store.clone(), home?, s);
                cycle::run_cycle(&mut ctx, mode)?;
            }
            Some(RotateAction::Start(w)) => {
                let s = settings::resolve(&w.tune, w.interval.as_deref(), &env)?;
                daemon::start(store, &home?, &s)?;
            }
            Some(RotateAction::Watch(w)) => {
                let s = settings::resolve(&w.tune, w.interval.as_deref(), &env)?;
                daemon::watch(store, home?, s, w.spawned)?;
            }
            Some(RotateAction::Stop) => control::stop(store, home.as_deref().ok())?,
            Some(RotateAction::Status) => control::status(store, home.as_deref().ok(), source),
            Some(RotateAction::Autostart { state }) => autostart::cli(store, state.as_deref())?,
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (store, source, args);
        Err(super::ProfileError::RotateUnsupported.into())
    }
}

/// `$HOME`, which holds the live `~/.claude`.
#[cfg(unix)]
fn home_dir() -> Result<std::path::PathBuf, super::ProfileError> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| super::ProfileError::RotateRefused("HOME is not set".into()))
}

/// `profile switch NAME` (issue #1691): moves NAME's login into `~/.claude`
/// with the rotator's swap, pins rotation to it and makes it the default.
/// Prints three lines. When the login moved but a later write failed, the
/// error says so; it is not a refusal, since something did change. When the
/// live login cannot be moved, NAME is still made the default for new
/// launches, that is printed, and the refusal exits non-zero.
pub(in crate::commands::profile) fn switch_live(
    store: &ProfileStore,
    name: &ProfileName,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let home = home_dir()?;
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

/// `profile switch --none`: removes the rotation pin and the default
/// pointer as one operation under the rotate lock; what was there. Off Unix
/// there is no rotator and so no pin, and only the pointer is removed.
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
