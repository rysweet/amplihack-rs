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
mod autostart;
#[cfg(unix)]
mod backup;
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
mod guide;
#[cfg(unix)]
mod handfix;
#[cfg(unix)]
pub(super) mod identify;
#[cfg(unix)]
mod ledger;
#[cfg(unix)]
mod log;
#[cfg(unix)]
mod lost;
#[cfg(unix)]
mod measure;
#[cfg(unix)]
mod observe;
#[cfg(unix)]
mod pin;
#[cfg(unix)]
mod precheck;
#[cfg(unix)]
mod prelude;
#[cfg(unix)]
mod recover;
#[cfg(unix)]
mod settings;
#[cfg(unix)]
mod swap;
#[cfg(unix)]
mod switch;
#[cfg(all(test, unix))]
pub(super) use swap::DOCUMENTED as DOCUMENTED_REFUSALS;

#[cfg(all(test, unix))]
mod tests_autostart;
#[cfg(all(test, unix))]
mod tests_daemon;
#[cfg(all(test, unix))]
mod tests_damp;
#[cfg(all(test, unix))]
mod tests_decide;
#[cfg(all(test, unix))]
mod tests_handfix;
#[cfg(all(test, unix))]
mod tests_handfix_cycle;
#[cfg(all(test, unix))]
mod tests_home_unreadable;
#[cfg(all(test, unix))]
mod tests_identify;
#[cfg(all(test, unix))]
mod tests_ledger;
#[cfg(all(test, unix))]
mod tests_live_readers;
#[cfg(all(test, unix))]
mod tests_log;
#[cfg(all(test, unix))]
mod tests_pin;
#[cfg(all(test, unix))]
mod tests_recover;
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
pub(crate) use autostart::autostart_on_launch;

/// `amplihack install` and one-shot `rotate` (#1659): one line on stderr when
/// the rotator for `root` (default: this environment's root) runs another
/// amplihack binary, which keeps reading the ledger in its own format until
/// it is restarted.
#[cfg(unix)]
pub(crate) fn warn_foreign_rotator(root: Option<&std::path::Path>) {
    let root = match root {
        Some(r) => Some(r.to_path_buf()),
        None => amplihack_utils::rotator_status::resolve_root(),
    };
    if let Some(pid) = root.and_then(|r| ledger::RotateState::new(&r).foreign_rotator()) {
        eprintln!("{}", ledger::foreign_rotator_note(pid));
    }
}

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

/// The `autostart:` status line for `root`, for `auto-switch status`.
#[cfg(unix)]
pub(in crate::commands::profile) fn autostart_line(root: &std::path::Path) -> String {
    autostart::status_line(autostart::effective(root, &|k| std::env::var_os(k)))
}

/// `profile rotate`: one cycle, or a subcommand. Tuning flags of `start` and
/// `watch` go after the subcommand.
#[derive(Debug, clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RotateArgs {
    #[command(subcommand)]
    pub action: Option<RotateAction>,
    /// Measure and decide, print what would happen, and change nothing
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub tune: TuneFlags,
}

#[derive(Debug, clap::Subcommand)]
pub enum RotateAction {
    /// Start `rotate watch` as a detached background process
    Start(WatchFlags),
    /// Run a cycle every --interval seconds in the foreground
    Watch(WatchFlags),
    /// Stop the background rotator
    Stop,
    /// Show whether the rotator runs, the live profile and the last cycle
    Status,
    /// Show, or set, whether launches start the rotator while auto-switch is on
    Autostart {
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
    },
}

/// Thresholds and margins. Kept as text so the error can name the flag; see
/// `settings::resolve`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct TuneFlags {
    /// Swap when the live session usage reaches N% (default 85)
    #[arg(long, value_name = "N")]
    pub session_at: Option<String>,
    /// Swap when the live weekly usage reaches N% (default 90)
    #[arg(long, value_name = "N")]
    pub week_at: Option<String>,
    /// Points of headroom the target needs over the live login (default 5)
    #[arg(long, value_name = "N")]
    pub margin: Option<String>,
    /// Points of headroom that justify a swap below the thresholds (default 30)
    #[arg(long, value_name = "N")]
    pub upgrade_margin: Option<String>,
    /// Seconds after a swap during which no upgrade happens; 0 disables (default 3600)
    #[arg(long, value_name = "SECS")]
    pub hold: Option<String>,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct WatchFlags {
    #[command(flatten)]
    pub tune: TuneFlags,
    /// Seconds between cycles (default 600)
    #[arg(long, value_name = "SECS")]
    pub interval: Option<String>,
    /// Set by `start`: the pid file is already written
    #[arg(long, hide = true)]
    pub spawned: bool,
}

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
        match switch::switch_live(store, &home_dir()?, name)? {
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
                Ok(())
            }
            Ok(switch::Switched::AlreadyLive(n)) => {
                println!("{n} is already the live login; nothing moved.");
                print_held(store, name);
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
