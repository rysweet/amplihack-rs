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
use std::path::PathBuf;
use store::ProfileStore;

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
    #[error("refusing symlinked or out-of-root path {0:?}")]
    SymlinkRefused(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
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
            Self::SharedAssetLimit(_) => "trim the oversized directory under ~/.claude and retry",
            Self::SymlinkRefused(_) => "remove the symlink and re-add the profile",
            Self::Io(_) => "check permissions on the profile root",
        }
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

fn resolve_launch_profile(
    store: &ProfileStore,
    flag: Option<&str>,
) -> Result<Option<(ProfileName, PathBuf)>, ProfileError> {
    let name = match flag {
        Some(raw) => ProfileName::parse(raw)?,
        None => match pointer::read(store.root())? {
            Some(n) => n,
            None => return Ok(None),
        },
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
    if profile.is_none() {
        return Ok(());
    }
    if docker {
        return Err(ProfileError::UnsupportedMode("--docker".into()));
    }
    if docker_env {
        return Err(ProfileError::UnsupportedMode(
            "docker (env-activated)".into(),
        ));
    }
    if auto {
        return Err(ProfileError::UnsupportedMode("--auto".into()));
    }
    Ok(())
}

fn docker_env_active() -> bool {
    crate::docker::DockerDetector
        .activation_source(false)
        .is_some()
}

/// Profiles are not applied in Docker; fail closed rather than drop the profile.
pub(super) fn reject_docker_env() -> Result<()> {
    validate_launch_flags(Some(""), false, docker_env_active(), false).map_err(with_hint)
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
/// the default pointer, then none. Docker and auto launches ignore the pointer.
pub fn launch_profile_dir(flag: Option<&str>, docker: bool, auto: bool) -> Result<Option<PathBuf>> {
    let docker_env = !docker && docker_env_active();
    validate_launch_flags(flag, docker, docker_env, auto).map_err(with_hint)?;
    if docker || auto {
        return Ok(None);
    }
    let store = ProfileStore::from_env().map_err(with_hint)?;
    let resolved = resolve_launch_profile(&store, flag).map_err(with_hint)?;
    if docker_env && let Some((name, _)) = &resolved {
        eprintln!("warning: default profile '{name}' is not applied in Docker (env-activated)");
        return Ok(None);
    }
    Ok(resolved.map(|(_, dir)| dir))
}
