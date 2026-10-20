//! `amplihack profile rotate` (issue #1592): keeps the login in `~/.claude`
//! on the account with the most capacity left, by moving logins between
//! `~/.claude` and the profiles. One-shot, `--dry-run`, and a background
//! `watch` with `start`, `stop` and `status`, which launches start on their
//! own while auto-switch is on (`autostart`). See the Rotate section of
//! docs/reference/profile-command.md.
//!
//! Login files are never opened, read, copied or hashed: they are checked
//! with `lstat` and moved with rename and exchange calls only.

#[cfg(unix)]
mod autostart;
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
mod identify;
#[cfg(unix)]
mod ledger;
#[cfg(unix)]
mod log;
#[cfg(unix)]
mod measure;
#[cfg(unix)]
mod observe;
#[cfg(unix)]
mod recover;
#[cfg(unix)]
mod settings;
#[cfg(unix)]
mod swap;
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
mod tests_identify;
#[cfg(all(test, unix))]
mod tests_ledger;
#[cfg(all(test, unix))]
mod tests_live_readers;
#[cfg(all(test, unix))]
mod tests_log;
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

use super::root_config::RootSource;
use super::store::ProfileStore;

#[cfg(unix)]
pub(crate) use autostart::autostart_on_launch;

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
        use super::ProfileError;
        let env = |k: &str| std::env::var_os(k);
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| ProfileError::RotateRefused("HOME is not set".into()));
        match args.action {
            None => {
                let s = settings::resolve(&args.tune, None, &env)?;
                let mode = if args.dry_run {
                    cycle::Mode::DryRun
                } else {
                    cycle::Mode::OneShot
                };
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
