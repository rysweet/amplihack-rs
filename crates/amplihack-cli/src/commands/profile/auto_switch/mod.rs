//! Automatic switching between Claude login profiles (issue #1569).
//!
//! Off by default. When on, a status-line wrapper records each account's
//! usage, a session whose account reaches the threshold has its login swapped
//! for one with quota left (a move, never a copy), nested launches start
//! under a profile with quota left, and a `-p` step that hits its usage limit
//! is rerun under the next profile. See docs/reference/profile-command.md.
//!
//! This file is the interface the rest of amplihack uses. Everything that
//! touches the filesystem is Unix-only; other platforms get no-ops.

#[cfg(unix)]
mod candidate;
#[cfg(unix)]
mod cli;
#[cfg(unix)]
mod config;
#[cfg(unix)]
mod fsx;
#[cfg(unix)]
mod liveness;
#[cfg(unix)]
mod rerun;
#[cfg(unix)]
mod state;
#[cfg(unix)]
mod statusline;
#[cfg(unix)]
mod swap;
#[cfg(unix)]
mod usage;

#[cfg(all(test, unix))]
mod tests_candidate;
#[cfg(all(test, unix))]
mod tests_config;
#[cfg(test)]
mod tests_contract;
#[cfg(all(test, unix))]
mod tests_liveness;
#[cfg(all(test, unix))]
mod tests_usage;

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use std::path::Path;

/// Set on a launch running under the rerun supervisor, so it does not
/// supervise itself. Removed before Claude starts.
pub const SUPERVISED_ENV: &str = "AMPLIHACK_AUTO_SWITCH_SUPERVISED";

#[cfg(unix)]
pub use cli::AutoSwitchCommand;

#[cfg(not(unix))]
#[derive(Debug, clap::Subcommand)]
pub enum AutoSwitchCommand {
    /// Not available on this platform
    Status,
}

/// What a lease check is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A launch; `join` when it inherited the profile from the session whose
    /// `CLAUDE_CONFIG_DIR` is this profile's directory.
    Launch {
        join: bool,
    },
    Login,
    Remove,
}

pub fn handle(store: &ProfileStore, cmd: AutoSwitchCommand) -> anyhow::Result<()> {
    #[cfg(unix)]
    return cli::handle(store, cmd);
    #[cfg(not(unix))]
    {
        let _ = (store, cmd);
        Err(ProfileError::AutoSwitchUnsupported.into())
    }
}

/// The hidden `profile statusline` command; returns the exit code.
pub fn statusline(root: &str, owner: &str) -> i32 {
    #[cfg(unix)]
    return statusline::run(root, owner);
    #[cfg(not(unix))]
    {
        let _ = (root, owner);
        0
    }
}

/// Refuses an action on `name` while its login is away from home, after
/// finishing any interrupted swap and returning ended sessions. Without a
/// state directory this is one metadata check.
pub fn guard(store: &ProfileStore, name: &ProfileName, action: Action) -> Result<(), ProfileError> {
    #[cfg(unix)]
    {
        if !store.root().join(".auto-switch.d").exists() {
            return Ok(());
        }
        let state = state::State::new(store.root())?;
        swap::guard(&state, name, action)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, name, action);
        Ok(())
    }
}

/// Keeps swaps out of a profile between the pre-spawn lease check and the
/// moment Claude's process exists, so liveness scans see the session. Drop it
/// once the spawn returns.
#[must_use = "dropping the hold re-opens the window before the spawn"]
pub struct LaunchHold {
    #[cfg(unix)]
    _lock: Option<state::Lock>,
}

/// [`guard`] for a launch, keeping the state lock held until the returned
/// value is dropped.
pub fn guard_launch(
    store: &ProfileStore,
    name: &ProfileName,
    join: bool,
) -> Result<LaunchHold, ProfileError> {
    #[cfg(unix)]
    {
        if !store.root().join(".auto-switch.d").exists() {
            return Ok(LaunchHold { _lock: None });
        }
        let state = state::State::new(store.root())?;
        let lock = swap::guard_held(&state, name, Action::Launch { join })?;
        Ok(LaunchHold { _lock: lock })
    }
    #[cfg(not(unix))]
    {
        let _ = (store, name, join);
        Ok(LaunchHold {})
    }
}

/// Whether this process's `CLAUDE_CONFIG_DIR` is `dir` (whole canonical
/// paths), i.e. it runs inside the session using that directory.
pub fn inside_session(dir: &Path) -> bool {
    let Some(own) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) else {
        return false;
    };
    match (
        std::path::PathBuf::from(own).canonicalize(),
        dir.canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Whether `AMPLIHACK_CLAUDE_AUTO_SWITCH=1` turns the switch on here: the only
/// way to know it is on when the profile root itself cannot be opened.
pub fn env_forces_on() -> bool {
    #[cfg(unix)]
    return config::env_toggle(std::env::var_os(config::TOGGLE_ENV).as_deref())
        == config::EnvToggle::On;
    #[cfg(not(unix))]
    false
}

fn supervised() -> bool {
    std::env::var_os(SUPERVISED_ENV).is_some_and(|v| !v.is_empty())
}

/// For a launch that inherited `inherited`: another profile to use when the
/// switch is on and the account live there is exhausted. Prints one note.
pub fn substitute_nested(store: &ProfileStore, inherited: &ProfileName) -> Option<ProfileName> {
    #[cfg(unix)]
    {
        if supervised() {
            return None; // the supervisor already chose
        }
        let eff = config::effective(store.root());
        if !eff.on {
            return None;
        }
        let state = match state::State::new(store.root()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "warning: auto-switch could not check profile {:?}: {e}",
                    inherited.as_str()
                );
                return None;
            }
        };
        let (c, reason) = rerun::substitute(store, &state, &eff.config, inherited)?;
        eprintln!("{}", rerun::substitution_note(inherited, &reason, &c));
        Some(c)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, inherited);
        None
    }
}

/// `substitute_nested` by name, for `multitask` workstream scripts.
pub fn substitute_name(inherited: &str) -> Option<String> {
    let store = match ProfileStore::from_env() {
        Ok(store) => store,
        Err(e) => {
            if env_forces_on() {
                eprintln!("warning: auto-switch could not open the profile root: {e}");
            }
            return None;
        }
    };
    let name = ProfileName::parse(inherited).ok()?;
    substitute_nested(&store, &name).map(|n| n.to_string())
}

/// Returns logins once a launched Claude has exited. Problems are warnings:
/// the next launch, login or `status` retries.
pub fn return_after_session(dir: &Path) {
    #[cfg(unix)]
    {
        let Some(root) = dir.parent() else {
            return;
        };
        if !root.join(".auto-switch.d").exists() {
            return;
        }
        let result = state::State::new(root).and_then(|s| {
            let _lock = s.lock()?;
            swap::recover(&s).map(|_| ())
        });
        if let Err(e) = result {
            eprintln!("warning: auto-switch could not return logins: {e}");
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Runs a nested `-p` launch under the usage-limit supervisor when the switch
/// is on. `None` when the launch should run normally (switch off, or already
/// supervised). The caller has checked the flags and arguments.
pub fn supervise(store: &ProfileStore, inherited: &str) -> anyhow::Result<Option<i32>> {
    #[cfg(unix)]
    {
        if supervised() {
            return Ok(None);
        }
        let Ok(name) = ProfileName::parse(inherited) else {
            return Ok(None); // the normal launch reports the bad value
        };
        let eff = config::effective(store.root());
        if !eff.on {
            return Ok(None);
        }
        let state = state::State::new(store.root())?;
        rerun::supervise(store, &state, &eff.config, name)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, inherited);
        Ok(None)
    }
}
