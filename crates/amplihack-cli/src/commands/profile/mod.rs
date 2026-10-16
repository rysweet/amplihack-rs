//! `amplihack profile`: manual Claude Code login profiles.
//!
//! A profile is a directory under the profile root that is handed to Claude
//! Code as `CLAUDE_CONFIG_DIR`. amplihack never reads, copies or refreshes
//! credentials, and never changes the parent environment.

mod name;
mod pointer;
mod run;
mod store;

#[cfg(test)]
mod tests_commands;
#[cfg(test)]
mod tests_docs_sync;
#[cfg(test)]
mod tests_name;
#[cfg(test)]
mod tests_pointer;
#[cfg(test)]
mod tests_run;
#[cfg(test)]
mod tests_seed;
#[cfg(test)]
mod tests_store;
#[cfg(test)]
mod tests_verify;

use anyhow::Result;
use clap::Subcommand;
use name::ProfileName;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::PathBuf;
use store::ProfileStore;
pub(crate) use store::ROOT_ENV;

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
            Self::InvalidRoot(_) => "set AMPLIHACK_CLAUDE_PROFILE_ROOT to an absolute path",
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
            _ => "not usable".into(),
        }
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
    /// List profiles; the default is marked with '*'
    List,
    /// Show the default profile name and directory
    Show,
    /// Make a profile the default for `launch` and `claude`
    Switch { name: String },
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
}

pub fn run_profile(cmd: ProfileCommands) -> Result<()> {
    handle(cmd).map_err(|e| match e.downcast::<ProfileError>() {
        Ok(pe) => with_hint(pe),
        Err(other) => other,
    })
}

fn handle(cmd: ProfileCommands) -> Result<()> {
    let store = ProfileStore::from_env()?;
    match cmd {
        ProfileCommands::Add { name } => {
            let dir = store.add(&ProfileName::parse(&name)?)?;
            println!("created profile {name} at {}", dir.display());
            println!("next: amplihack profile login {name}");
        }
        ProfileCommands::List => {
            let out = render_list(&store)?;
            if out.is_empty() {
                println!("no profiles; create one with `amplihack profile add <name>`");
            } else {
                print!("{out}");
            }
        }
        ProfileCommands::Show => print!("{}", render_show(&store)?),
        ProfileCommands::Switch { name } => {
            let name = ProfileName::parse(&name)?;
            store.resolve(&name)?;
            pointer::write(store.root(), &name)?;
            println!("default profile: {name}");
        }
        ProfileCommands::Remove { name, yes } => {
            let name = ProfileName::parse(&name)?;
            store.remove(&name, yes)?;
            println!("removed profile {name}");
        }
        ProfileCommands::Login { name } => run::login(&store, &ProfileName::parse(&name)?)?,
        ProfileCommands::Run { name, args } => run::run(&store, &ProfileName::parse(&name)?, args)?,
    }
    Ok(())
}

fn render_list(store: &ProfileStore) -> Result<String, ProfileError> {
    let default = pointer::read(store.root())?;
    let mut out = String::new();
    for n in store.list()? {
        let mark = if default.as_ref().is_some_and(|d| d.as_str() == n) {
            '*'
        } else {
            ' '
        };
        out.push_str(&format!("{mark} {n}\n"));
    }
    Ok(out)
}

fn render_show(store: &ProfileStore) -> Result<String, ProfileError> {
    match pointer::read(store.root())? {
        None => Ok("none\n".to_string()),
        Some(name) => {
            let dir = store.resolve(&name)?;
            Ok(format!("name: {name}\ndir: {}\n", dir.display()))
        }
    }
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
pub(crate) fn verify_launch_profile_dir(dir: &std::path::Path) -> Result<()> {
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
    Ok(())
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
        ProfileStore::from_env,
    )
    .map_err(with_hint)
}

/// `launch_profile_dir` with the environment passed in, so tests do not depend
/// on the caller's environment. `open_store` is only called when a profile may
/// be applied.
fn launch_profile_dir_with(
    flag: Option<&str>,
    inherited: Option<&OsStr>,
    docker: bool,
    docker_env: bool,
    auto: bool,
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
    Ok(resolved.map(|(_, dir)| dir))
}
