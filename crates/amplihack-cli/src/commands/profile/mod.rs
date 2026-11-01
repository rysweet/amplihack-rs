//! `amplihack profile`: manual Claude Code login profiles.
//!
//! A profile is a directory under the profile root that is handed to Claude
//! Code as `CLAUDE_CONFIG_DIR`. amplihack never copies credentials and never
//! changes the parent environment. `profile usage` and the rotator read a
//! profile's OAuth token in memory to ask the usage endpoint, and may
//! refresh it once under Claude's lock (`usage_direct`, issue #1844). With
//! auto-switch on (off by default) amplihack may move a login file between
//! profile directories; see `auto_switch`.

#[cfg(unix)]
mod attempt;
mod auto_switch;
mod default_live;
#[cfg(unix)]
mod duplicates;
#[cfg(unix)]
mod kind;
#[cfg(unix)]
mod limit_wait;
#[cfg(unix)]
mod login_check;
mod name;
mod overflow;
mod pointer;
mod probe_limits;
mod root_cmd;
mod root_config;
mod rotate;
mod rotate_args;
mod run;
mod settings_refresh;
mod shared_dirs;
mod store;
mod supervise_gate;
mod usage_cmd;
#[cfg(unix)]
mod usage_direct;
mod usage_parse;
#[cfg(unix)]
mod usage_passive;
mod usage_request;
mod usage_source;

#[cfg(test)]
mod tests_commands;
#[cfg(all(test, unix))]
mod tests_default_live;
#[cfg(test)]
mod tests_docs_sync;
#[cfg(test)]
mod tests_docs_sync_platform;
#[cfg(test)]
mod tests_docs_sync_probe;
#[cfg(all(test, unix))]
mod tests_duplicates;
#[cfg(all(test, unix))]
mod tests_fake_http;
#[cfg(all(test, unix))]
mod tests_kind;
#[cfg(all(test, unix))]
mod tests_login_check;
#[cfg(test)]
mod tests_name;
#[cfg(test)]
mod tests_pointer;
#[cfg(test)]
mod tests_probe_limits;
#[cfg(test)]
mod tests_root_config;
#[cfg(test)]
mod tests_root_save;
#[cfg(test)]
mod tests_run;
#[cfg(test)]
mod tests_seed;
#[cfg(all(test, unix))]
mod tests_settings_refresh;
#[cfg(all(test, unix))]
mod tests_shared_dirs;
#[cfg(test)]
mod tests_store;
#[cfg(test)]
mod tests_usage_cmd;
#[cfg(all(test, unix))]
mod tests_usage_direct;
#[cfg(all(test, unix))]
mod tests_usage_direct_scan;
#[cfg(test)]
mod tests_usage_parse;
#[cfg(all(test, unix))]
mod tests_usage_passive;
#[cfg(test)]
mod tests_usage_request;
#[cfg(all(test, unix))]
mod tests_usage_source;
#[cfg(test)]
mod tests_verify;

use anyhow::Result;
/// File helpers shared with the usage-limit wait (#1618).
#[cfg(unix)]
use auto_switch::fsx;
pub(crate) use auto_switch::{
    launch_warning as capacity_launch_warning, recipe_warning as capacity_recipe_warning,
};
use clap::Subcommand;
use name::ProfileName;
#[cfg(test)]
use root_cmd::{render_list, render_show};
pub(crate) use rotate::autostart_on_launch;
#[cfg(unix)]
pub(crate) use rotate::warn_foreign_rotator;
pub(crate) use settings_refresh::{Refresh, refresh_profile_settings};
use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::PathBuf;
use store::ProfileStore;
pub(crate) use store::ROOT_ENV;
pub use supervise_gate::maybe_supervise;

/// Name of the profile a parent launch resolved. Every launch that applies a
/// profile sets it on its child, so nested `amplihack` launches keep the same
/// profile even if the default pointer changes (issue #1567).
pub const PROFILE_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE";

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid profile name {0:?}")]
    InvalidName(String),
    #[error("profile {0:?} not found")]
    NotFound(String),
    #[error("profile {0:?} already exists")]
    AlreadyExists(String),
    #[error("removing a profile requires confirmation")]
    ConfirmationRequired,
    #[error("--profile cannot be combined with {0}")]
    UnsupportedMode(String),
    #[error("default profile {0:?} no longer exists")]
    PointerDangling(String),
    #[error("default profile pointer is unreadable: {0}")]
    PointerCorrupt(String),
    #[error("invalid profile root: {0:?}")]
    InvalidRoot(String),
    #[error("saved profile root in {path:?} is invalid: {reason}")]
    InvalidSavedRoot { path: String, reason: String },
    #[error("invalid shared Claude settings: {0}")]
    InvalidSharedSettings(String),
    #[error("shared Claude asset source is not a directory: {0:?}")]
    InvalidSharedSource(String),
    #[error("shared Claude assets exceed the copy limit: {0}")]
    SharedAssetLimit(String),
    #[error("profile root cannot be inside shared Claude assets at {0:?}")]
    SharedSourceContainsRoot(String),
    #[error(
        "profile seeding failed ({seed_error}); cleanup of {path:?} also failed: {cleanup_error}"
    )]
    SeedCleanupFailed {
        path: String,
        seed_error: String,
        cleanup_error: String,
    },
    #[error("refusing symlinked or out-of-root path {0:?}")]
    SymlinkRefused(String),
    #[error("profile directory has unsafe ownership or permissions: {0:?}")]
    UnsafePermissions(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("profile {profile:?}: its login is in use by a session in profile {host:?}")]
    Leased { profile: String, host: String },
    #[error(
        "profile {profile:?}: its directory holds the login of profile {live:?} for a running session"
    )]
    HostsForeignAccount { profile: String, live: String },
    #[error("auto-switch settings: {0}")]
    AutoSwitchConfig(String),
    #[error("auto-switch state is inconsistent: {0}")]
    AutoSwitchInvariant(String),
    #[error("auto-switch install: {0}")]
    AutoSwitchInstall(String),
    #[error("auto-switch is not available on this platform")]
    AutoSwitchUnsupported,
    #[error("{0}")]
    RotateConfig(String),
    #[error("the external login rotator holds <root>/.rotator/lock")]
    RotatePrototypeRunning,
    #[error("swap refused: {0}")]
    RotateRefused(String),
    #[error("rotate state is inconsistent: {0}")]
    RotateInconsistent(String),
    #[error("profile rotate is not available on this platform")]
    RotateUnsupported,
    #[error("rotate: {0}")]
    RotateDaemon(String),
    #[error(
        "profile {0} is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login {0}', or run 'amplihack profile kind set {0} api-key' if per-token billing is intended."
    )]
    SubscriptionNoLogin(String),
    #[error(
        "profile {0} is an API-key profile; the API-key tier is fixed at 10, so --tier cannot be set"
    )]
    PlatformTierFixed(String),
    /// `switch` onto an API-key profile while the API-key block is active.
    #[error(
        "profile {name}: the API key is blocked ({reason}) until {}",
        block_end(*.until)
    )]
    PlatformBlocked {
        name: String,
        reason: String,
        until: i64,
    },
    #[error(
        "inherited AMPLIHACK_CLAUDE_PROFILE {value}: {}",
        .source.inherited_reason()
    )]
    Inherited {
        /// Escaped and shortened by `escape_for_error`.
        value: String,
        source: Box<ProfileError>,
    },
}

impl ProfileError {
    pub fn hint(&self) -> &'static str {
        match self {
            Self::InvalidName(_) => {
                "names match [A-Za-z0-9][A-Za-z0-9._-]{0,63}, without '..' or a trailing '.'"
            }
            Self::NotFound(_) | Self::AlreadyExists(_) => "run `amplihack profile list`",
            Self::ConfirmationRequired => "re-run with --yes",
            Self::UnsupportedMode(_) => "launch without --docker/--auto, or without --profile",
            Self::PointerDangling(_) => "run `amplihack profile switch <name>` or add the profile",
            Self::PointerCorrupt(_) => "re-run `amplihack profile switch <name>`",
            Self::InvalidRoot(_) => {
                "use an absolute path: set AMPLIHACK_CLAUDE_PROFILE_ROOT to one, or run: amplihack profile root <absolute dir>"
            }
            Self::InvalidSavedRoot { .. } => {
                "fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>"
            }
            Self::InvalidSharedSettings(_) => "repair ~/.claude/settings.json and retry",
            Self::InvalidSharedSource(_) => "ensure ~/.claude is a directory and retry",
            Self::SharedAssetLimit(_) => {
                "reduce oversized files or directories under ~/.claude and retry"
            }
            Self::SharedSourceContainsRoot(_) => "choose a profile root outside ~/.claude",
            Self::SeedCleanupFailed { .. } => {
                "remove the reported partial directory before retrying"
            }
            Self::SymlinkRefused(_) => "remove the symlink and re-add the profile",
            Self::UnsafePermissions(_) => {
                "ensure the profile is owned by your user and run `chmod go-rwx <profile-directory>`"
            }
            Self::Io(_) => "check permissions on the profile root",
            Self::Leased { .. } | Self::HostsForeignAccount { .. } => {
                "wait for that session to end, or run `amplihack profile auto-switch status`"
            }
            Self::AutoSwitchConfig(_) => {
                "remove <root>/.auto-switch and run `amplihack profile auto-switch on` or `off`"
            }
            Self::AutoSwitchInvariant(_) => {
                "see the Recovery section of docs/howto/claude-profiles.md"
            }
            Self::AutoSwitchInstall(_) => {
                "fix or remove the reported file, then run `amplihack profile auto-switch repair`"
            }
            Self::AutoSwitchUnsupported => "auto-switch needs Linux or another Unix; leave it off",
            Self::RotateConfig(_) => {
                "run `amplihack profile rotate --help` for the accepted values"
            }
            Self::RotatePrototypeRunning => {
                "stop the external rotator (tmux session claude-rotator) first"
            }
            Self::RotateRefused(_) => {
                "nothing was moved; see \"Swap refused\" in docs/howto/claude-profile-rotation.md"
            }
            Self::RotateInconsistent(_) => {
                "see the Recovery section of docs/howto/claude-profile-rotation.md"
            }
            Self::RotateUnsupported => "rotate needs Linux or another Unix",
            Self::RotateDaemon(_) => "run `amplihack profile rotate status`",
            Self::SubscriptionNoLogin(_) => {
                "amplihack profile login <name>, or: amplihack profile kind set <name> api-key"
            }
            Self::PlatformTierFixed(_) => {
                "run `amplihack profile overflow add <name>` without --tier"
            }
            Self::PlatformBlocked { .. } => "amplihack profile overflow clear --platform",
            Self::Inherited { source, .. } if matches!(**source, Self::UnsupportedMode(_)) => {
                "unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto"
            }
            Self::Inherited { source, .. }
                if matches!(**source, Self::InvalidName(_) | Self::NotFound(_)) =>
            {
                "unset AMPLIHACK_CLAUDE_PROFILE, or create the profile with `amplihack profile add <name>`"
            }
            // The profile exists but cannot be used: the cause's own hint applies.
            Self::Inherited { source, .. } => source.hint(),
        }
    }

    /// Fixed reason shown in an `Inherited` error. The value is printed once,
    /// escaped, so nothing that embeds the raw value is repeated here. An I/O
    /// error shows only its `ErrorKind`, which never contains the value.
    fn inherited_reason(&self) -> Cow<'static, str> {
        match self {
            Self::InvalidName(_) => "invalid name".into(),
            Self::NotFound(_) => "not found".into(),
            Self::SymlinkRefused(_) => "refused path".into(),
            Self::UnsafePermissions(_) => "unsafe permissions".into(),
            Self::Io(e) => format!("i/o error on profile root ({})", e.kind()).into(),
            Self::UnsupportedMode(_) => "not applied with --docker, --auto or Docker".into(),
            // Profile names here are validated, so the message is safe to repeat.
            Self::Leased { .. } | Self::HostsForeignAccount { .. } => self.to_string().into(),
            _ => "not usable".into(),
        }
    }
}

/// When an API-key block ends, in the rotate log's time format.
fn block_end(until: i64) -> String {
    #[cfg(unix)]
    {
        rotate::timestamp(until)
    }
    #[cfg(not(unix))]
    {
        // No rotator records a block here; the epoch seconds are exact.
        until.to_string()
    }
}

/// Escapes a value for an error message (no raw control bytes) and
/// cuts the escaped form to 64 characters, marking a cut with '…'.
pub(crate) fn escape_for_error(raw: &str) -> String {
    const MAX: usize = 64;
    // Escaping never shortens a character, so MAX + 1 raw chars are enough to
    // tell whether the escaped form needs cutting.
    let head: String = raw.chars().take(MAX + 1).collect();
    let escaped = format!("{head:?}");
    if escaped.chars().count() <= MAX {
        return escaped;
    }
    let mut out: String = escaped.chars().take(MAX).collect();
    out.push('…');
    out
}

fn inherited_error(raw: &str, source: ProfileError) -> ProfileError {
    ProfileError::Inherited {
        value: escape_for_error(raw),
        source: Box::new(source),
    }
}

pub(crate) fn with_hint(e: ProfileError) -> anyhow::Error {
    anyhow::anyhow!("{e}\nhint: {}", e.hint())
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommands {
    /// Create a profile and copy allowlisted shared Claude assets (does not run Claude)
    Add { name: String },
    /// List profiles after a `root:` line; the default is marked with '*'
    List,
    /// Show the default profile name and directory
    Show,
    /// Move a profile's login into ~/.claude, make it the default and hold
    /// rotation on it; --none releases rotation and clears the default
    Switch {
        #[arg(required_unless_present = "none")]
        name: Option<String>,
        /// Clear the default and release rotation: `launch` and `claude` then
        /// use no profile, and the rotator picks the login again
        #[arg(long, conflicts_with = "name")]
        none: bool,
    },
    /// Delete a profile directory
    Remove {
        name: String,
        /// Confirm deletion
        #[arg(long)]
        yes: bool,
    },
    /// Run `claude` interactively under a profile so you can log in
    Login { name: String },
    /// Launch Claude under a profile; arguments after `--` go to Claude
    Run {
        name: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show each profile's session and weekly usage, most capacity first
    Usage {
        /// Profiles to probe (default: all)
        names: Vec<String>,
        /// Print a JSON array instead of the table
        #[arg(long)]
        json: bool,
        /// Also print what `claude` wrote to stdout and stderr
        #[arg(long)]
        raw: bool,
        /// Seconds each probe may run (default 120, max 600; or AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS)
        #[arg(long, hide = true, value_parser = clap::value_parser!(u64).range(1..=probe_limits::MAX_TIMEOUT_SECS))]
        timeout_secs: Option<u64>,
    },
    /// Switch accounts automatically when one nears its usage limit
    AutoSwitch {
        #[command(subcommand)]
        command: auto_switch::AutoSwitchCommand,
    },
    /// Print the profile root and its source, or save <dir> as the root
    Root { dir: Option<String> },
    /// Keep the login in ~/.claude on the profile with the most capacity left
    Rotate(Box<rotate::RotateArgs>),
    /// Mark profiles as credit-backed overflow for the rotator (costs real money)
    Overflow {
        #[command(subcommand)]
        command: overflow::OverflowCommand,
    },
    /// Show or override each profile's kind: subscription or api-key (billed per token)
    #[cfg(unix)]
    Kind {
        #[command(subcommand)]
        command: kind::KindCommand,
    },
    /// Status-line wrapper run by Claude Code (installed by `auto-switch install`)
    #[command(hide = true)]
    Statusline {
        #[arg(long)]
        root: String,
        #[arg(long)]
        owner: String,
    },
}

pub fn run_profile(cmd: ProfileCommands) -> Result<()> {
    handle(cmd).map_err(|e| match e.downcast::<ProfileError>() {
        Ok(pe) => with_hint(pe),
        Err(other) => other,
    })
}

fn handle(cmd: ProfileCommands) -> Result<()> {
    // The wrapper's output belongs to Claude's status line: no error lines on
    // stdout, and its exit code is the original status line's.
    if let ProfileCommands::Statusline { root, owner } = &cmd {
        std::process::exit(auto_switch::statusline(root, owner));
    }
    // Before the root is resolved, so it can repair a damaged saved root.
    if let ProfileCommands::Root { dir } = cmd {
        return Ok(root_cmd::run(dir)?);
    }
    let (store, source) = root_config::store_from_env()?;
    match cmd {
        ProfileCommands::Add { name } => {
            let dir = store.add(&ProfileName::parse(&name)?)?;
            println!("created profile {name} at {}", dir.display());
            println!("next: amplihack profile login {name}");
            root_cmd::record_after_add(&store);
        }
        ProfileCommands::List => {
            println!("{}", root_cmd::root_line(store.root(), source));
            let mut warnings = Vec::new();
            let out = root_cmd::render_list_with(
                &store,
                usage_cmd::home_dir().as_deref(),
                &mut warnings,
            )?;
            if out.is_empty() {
                println!("no profiles; create one with `amplihack profile add <name>`");
            } else {
                print!("{out}");
            }
            for w in warnings {
                eprintln!("{w}");
            }
        }
        ProfileCommands::Show => {
            let mut warnings = Vec::new();
            print!(
                "{}",
                root_cmd::render_show_with(
                    &store,
                    usage_cmd::home_dir().as_deref(),
                    &mut warnings
                )?
            );
            for w in warnings {
                eprintln!("{w}");
            }
        }
        ProfileCommands::Switch { none: true, .. } => println!("{}", clear_default(&store)?),
        ProfileCommands::Switch { name, .. } => {
            let name = ProfileName::parse(name.as_deref().unwrap_or_default())?;
            store.resolve(&name)?;
            // Issue #1700: the rotator's live login counts as held here, since
            // `launch` and `claude` redirect that profile to `~/.claude`.
            #[cfg(unix)]
            kind_guard(&store, &name, usage_cmd::home_dir().as_deref(), true)?;
            rotate::switch_live(&store, &name)?;
        }
        ProfileCommands::Remove { name, yes } => {
            let name = ProfileName::parse(&name)?;
            if yes {
                auto_switch::guard(&store, &name, auto_switch::Action::Remove)?;
            }
            store.remove(&name, yes)?;
            println!("removed profile {name}");
        }
        ProfileCommands::Login { name } => {
            let name = ProfileName::parse(&name)?;
            store.resolve(&name)?;
            auto_switch::guard(&store, &name, auto_switch::Action::Login)?;
            #[cfg(unix)]
            if kind::Ctx::load(&store, usage_cmd::home_dir().as_deref()).kind(&store, &name)
                == kind::Kind::Platform
            {
                println!("{}", kind::LOGIN_NOTE);
            }
            // Login spawns claude itself, not through the pre-spawn check.
            shared_dirs::ensure_for(&store, &name);
            #[cfg(unix)]
            login_check::login_checked(&store, &name, usage_cmd::home_dir().as_deref(), || {
                run::login(&store, &name)
            })?;
            #[cfg(not(unix))]
            run::login(&store, &name)?;
        }
        ProfileCommands::Run { name, args } => {
            let name = ProfileName::parse(&name)?;
            store.resolve(&name)?;
            // `run` is never redirected to `~/.claude`, so the rotator's
            // live login does not count as held.
            #[cfg(unix)]
            kind_guard(&store, &name, usage_cmd::home_dir().as_deref(), false)?;
            auto_switch::guard(&store, &name, auto_switch::Action::Launch { join: false })?;
            run::run(&store, &name, args)?
        }
        ProfileCommands::Usage {
            names,
            json,
            raw,
            timeout_secs,
        } => usage_cmd::run(&store, &names, json, raw, timeout_secs)?,
        ProfileCommands::AutoSwitch { command } => auto_switch::handle(&store, command)?,
        ProfileCommands::Rotate(args) => rotate::run(&store, source, *args)?,
        ProfileCommands::Overflow { command } => overflow::run(&store, command)?,
        #[cfg(unix)]
        ProfileCommands::Kind { command } => {
            kind::run(&store, command, usage_cmd::home_dir().as_deref())?
        }
        ProfileCommands::Statusline { .. } | ProfileCommands::Root { .. } => {
            unreachable!("handled above")
        }
    }
    Ok(())
}

/// The launch guard (issue #1700): refuses a subscription profile with no
/// login of its own, before anything is set or spawned, unless its login is
/// held for it: leased or hosted by auto-switch, or, with `rotator_live`,
/// the rotator's identified live login.
#[cfg(unix)]
fn kind_guard(
    store: &ProfileStore,
    name: &ProfileName,
    home: Option<&std::path::Path>,
    rotator_live: bool,
) -> Result<(), ProfileError> {
    kind::launch_guard(store, name, home, || {
        auto_switch::login_held_elsewhere(store, name)
            || (rotator_live && rotate::is_live(store, home, name))
    })
}

/// Printed by `profile switch --none`.
const CLEARED: &str = "Default profile cleared.";
const NOT_SET: &str = "No default profile was set.";

const RELEASED: &str = "Rotation released; the rotator picks the best login on its next cycle.";

/// Releases the rotation pin and removes the default pointer, valid or not,
/// as one locked operation; the message to print. When the pin cannot be
/// released, nothing changes.
fn clear_default(store: &ProfileStore) -> Result<String, ProfileError> {
    store.check_root_not_symlink()?;
    let released = rotate::release(store)?;
    let mut out = String::from(if released.default { CLEARED } else { NOT_SET });
    if released.pin {
        out.push('\n');
        out.push_str(RELEASED);
    }
    Ok(out)
}

/// The one line printed when a launch of the rotator's live profile runs
/// against `~/.claude`.
fn live_launch_note(name: &str) -> String {
    format!(
        "note: profile '{name}' is the rotator's live login, which is in ~/.claude; \
         launching against ~/.claude."
    )
}

/// Resolution order: `--profile` flag, then a non-empty inherited
/// `AMPLIHACK_CLAUDE_PROFILE`, then the default pointer, then none. A flag
/// never reads the inherited value or the pointer; an inherited value never
/// reads the pointer and fails closed if it cannot be used. Docker and
/// `--auto` are handled by the caller (`launch_profile_dir_with`), which
/// refuses an inherited value in those modes before this is reached.
fn resolve_launch_profile(
    store: &ProfileStore,
    flag: Option<&str>,
    inherited: Option<&str>,
) -> Result<Option<(ProfileName, PathBuf)>, ProfileError> {
    if let Some(raw) = flag {
        let name = ProfileName::parse(raw)?;
        let dir = store.resolve(&name)?;
        return Ok(Some((name, dir)));
    }
    if let Some(raw) = inherited.filter(|v| !v.is_empty()) {
        return ProfileName::parse(raw)
            .and_then(|name| store.resolve(&name).map(|dir| (name, dir)))
            .map(Some)
            .map_err(|e| inherited_error(raw, e));
    }
    let Some(name) = pointer::read(store.root())? else {
        return Ok(None);
    };
    let dir = store.resolve(&name)?;
    Ok(Some((name, dir)))
}

fn validate_launch_flags(
    profile: Option<&str>,
    docker: bool,
    docker_env: bool,
    auto: bool,
) -> Result<(), ProfileError> {
    match profile {
        Some(_) => unsupported_mode(docker, docker_env, auto).map_or(Ok(()), Err),
        None => Ok(()),
    }
}

/// The launch mode a profile cannot be applied in, if any.
fn unsupported_mode(docker: bool, docker_env: bool, auto: bool) -> Option<ProfileError> {
    let mode = if docker {
        "--docker"
    } else if docker_env {
        "docker (env-activated)"
    } else if auto {
        "--auto"
    } else {
        return None;
    };
    Some(ProfileError::UnsupportedMode(mode.into()))
}

fn docker_env_active() -> bool {
    crate::docker::DockerDetector
        .activation_source(false)
        .is_some()
}

/// Profiles are not applied in Docker; fail closed rather than drop the profile.
pub(super) fn reject_docker_env() -> Result<()> {
    match unsupported_mode(false, docker_env_active(), false) {
        Some(e) => Err(with_hint(e)),
        None => Ok(()),
    }
}

/// Re-validates a profile directory immediately before the child is spawned.
/// This narrows, but does not close, the window before `exec`.
///
/// Holds the auto-switch lock until the returned value is dropped, which the
/// caller does right after the spawn returns.
pub(crate) fn verify_launch_profile_dir(dir: &std::path::Path) -> Result<auto_switch::LaunchHold> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ProfileError::SymlinkRefused(dir.display().to_string()))
        .and_then(ProfileName::parse)
        .map_err(with_hint)?;
    let store = ProfileStore::from_env().map_err(with_hint)?;
    let verified = store.verify_profile_dir(&name).map_err(with_hint)?;
    if verified != dir {
        return Err(with_hint(ProfileError::SymlinkRefused(
            dir.display().to_string(),
        )));
    }
    // Issue #1700: checked again right before the spawn.
    #[cfg(unix)]
    kind_guard(&store, &name, usage_cmd::home_dir().as_deref(), false).map_err(with_hint)?;
    let join = LAUNCH_MAY_JOIN.load(std::sync::atomic::Ordering::Relaxed)
        && auto_switch::inside_session(dir);
    let hold = auto_switch::guard_launch(&store, &name, join).map_err(with_hint)?;
    // Issue #1634: the shared session links, under the hold. Never fails.
    shared_dirs::ensure_for(&store, &name);
    Ok(hold)
}

/// Set when this process's launch profile was inherited (tier 2), the only
/// case in which a launch may join a session whose directory hosts another
/// account. Read again by the pre-spawn check.
static LAUNCH_MAY_JOIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Returns logins borrowed by the session that just ended in `dir`.
pub(crate) fn return_after_session(dir: &std::path::Path) {
    auto_switch::return_after_session(dir);
}

/// Variable that marks a launch already running under the rerun supervisor.
pub(crate) const AUTO_SWITCH_SUPERVISED_ENV: &str = auto_switch::SUPERVISED_ENV;

/// `multitask` seam: the profile a workstream inheriting `inherited` should
/// export, when auto-switch picks another one.
pub(crate) fn substitute_inherited_profile(inherited: &str) -> Option<String> {
    auto_switch::substitute_name(inherited)
}

/// `agent supervise` seam (issue #1760): the config directory a launch with
/// `flag` and no inherited `AMPLIHACK_CLAUDE_PROFILE` resolves to, as a
/// resumed pane runs it. That is the flag's profile, else the default
/// pointer's; `None` means `~/.claude` (neither is set, or the profile is the
/// rotator's live login). Prints nothing, takes no lock and refuses nothing
/// that only the launch itself checks. An error is a whole sentence.
pub(crate) fn resume_profile_dir(
    flag: Option<&str>,
    home: Option<&std::path::Path>,
) -> std::result::Result<Option<PathBuf>, String> {
    let store = ProfileStore::from_env().map_err(|e| e.to_string())?;
    let Some((name, dir)) =
        resolve_launch_profile(&store, flag, None).map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if default_live::could_be_live(&store, &name)
        && default_live::live_name(&store, home).is_some_and(|l| l == name)
    {
        return Ok(None);
    }
    Ok(Some(dir))
}

/// Profile directory for an interactive `launch`/`claude`: `--profile`, then
/// an inherited `AMPLIHACK_CLAUDE_PROFILE`, then the default pointer, then
/// none. Docker and auto launches refuse a non-empty inherited value and skip
/// the pointer.
pub fn launch_profile_dir(flag: Option<&str>, docker: bool, auto: bool) -> Result<Option<PathBuf>> {
    let docker_env = !docker && docker_env_active();
    let inherited = std::env::var_os(PROFILE_ENV);
    launch_profile_dir_with(
        flag,
        inherited.as_deref(),
        docker,
        docker_env,
        auto,
        usage_cmd::home_dir().as_deref(),
        ProfileStore::from_env,
    )
    .map_err(with_hint)
}

/// `launch_profile_dir` with the environment passed in, so tests do not depend
/// on the caller's environment. `open_store` is only called when a profile may
/// be applied. `home` is `$HOME`, for finding the rotator's live profile.
fn launch_profile_dir_with(
    flag: Option<&str>,
    inherited: Option<&OsStr>,
    docker: bool,
    docker_env: bool,
    auto: bool,
    home: Option<&std::path::Path>,
    open_store: impl FnOnce() -> Result<ProfileStore, ProfileError>,
) -> Result<Option<PathBuf>, ProfileError> {
    validate_launch_flags(flag, docker, docker_env, auto)?;
    // An explicit flag never reads the inherited value.
    let inherited = inherited.filter(|v| flag.is_none() && !v.is_empty());
    // An inherited profile cannot be applied in Docker or with --auto. Refuse
    // rather than run on the default account, before the value is parsed or
    // the store is opened.
    if let Some(raw) = inherited
        && let Some(mode) = unsupported_mode(docker, docker_env, auto)
    {
        return Err(inherited_error(&raw.to_string_lossy(), mode));
    }
    if docker || auto {
        return Ok(None);
    }
    let inherited = inherited
        .map(|v| {
            v.to_str().ok_or_else(|| {
                let lossy = v.to_string_lossy();
                inherited_error(&lossy, ProfileError::InvalidName(lossy.to_string()))
            })
        })
        .transpose()?;
    let store = open_store()?;
    let resolved = resolve_launch_profile(&store, flag, inherited)?;
    if docker_env && let Some((name, _)) = &resolved {
        eprintln!("warning: default profile '{name}' is not applied in Docker (env-activated)");
        return Ok(None);
    }
    let Some((name, dir)) = resolved else {
        return Ok(None);
    };
    // The rotator's live profile has its login in `~/.claude`, not in its
    // directory: launch with no profile, as Claude then uses `~/.claude`.
    // Only a directory without its own login can be live, so one lstat
    // skips reading the ledgers and every profile's `.claude.json` on the
    // usual launch.
    if default_live::could_be_live(&store, &name) {
        let (live, warning) = default_live::detect(&store, home);
        if let Some(w) = warning {
            eprintln!("{w}");
        }
        if live.is_some_and(|l| l.name == name) {
            eprintln!("{}", live_launch_note(name.as_str()));
            return Ok(None);
        }
    }
    // Auto-switch (#1569): only an inherited profile is ever redirected or
    // allowed to join a hosted session; a flag or the pointer never is.
    let is_inherited = flag.is_none() && inherited.is_some();
    if is_inherited && let Some(c) = auto_switch::substitute_nested(&store, &name) {
        let dir = store.resolve(&c)?;
        auto_switch::guard(&store, &c, auto_switch::Action::Launch { join: false })?;
        return Ok(Some(dir));
    }
    // Issue #1700: a subscription profile with no login of its own would
    // run on the Claude Platform API key.
    #[cfg(unix)]
    kind_guard(&store, &name, home, false)?;
    let join = is_inherited && auto_switch::inside_session(&dir);
    LAUNCH_MAY_JOIN.store(is_inherited, std::sync::atomic::Ordering::Relaxed);
    auto_switch::guard(&store, &name, auto_switch::Action::Launch { join })?;
    Ok(Some(dir))
}
