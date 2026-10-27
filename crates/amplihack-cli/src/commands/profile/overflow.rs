//! `amplihack profile overflow` (issue #1606): marks profiles as
//! credit-backed overflow for the rotator, and the one reader of that mark.
//! The marks are the `overflow=` key of `<root>/.auto-switch`; spend blocks
//! live in the rotate ledger. See "Overflow commands" in
//! docs/reference/profile-command.md.
//!
//! `add` and `remove` change the file through `config::update`, under the
//! auto-switch state lock, so a concurrent `auto-switch exclude` or `on`, or
//! a rotator cycle, cannot lose either change. When the lock is busy for
//! longer than `config::CONFIG_LOCK_WAIT` they fail with a retry message and
//! change nothing. Output is printed after the lock is released.

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use std::path::Path;

#[derive(Debug, clap::Subcommand)]
pub enum OverflowCommand {
    /// Mark a profile as credit-backed overflow, used only when every plan login is exhausted
    Add { name: String },
    /// Remove the overflow mark from a profile
    Remove { name: String },
    /// List the overflow profiles in the order the rotator tries them, with any spend block
    List,
    /// Lift the rotator's spend-limit block on a profile
    Clear { name: String },
}

/// The overflow profiles under `root`, in config order. Empty when
/// `.auto-switch` is missing or corrupt, so a damaged file never spends.
#[cfg(unix)]
pub fn overflow_names(root: &Path) -> Vec<ProfileName> {
    use super::auto_switch::config::{self, FileState};
    match config::read(root) {
        FileState::Valid(c) => c.overflow,
        FileState::Missing | FileState::Corrupt => Vec::new(),
    }
}

/// No auto-switch settings off Unix: no overflow profiles.
#[cfg(not(unix))]
pub fn overflow_names(_root: &Path) -> Vec<ProfileName> {
    Vec::new()
}

#[cfg(unix)]
pub fn run(store: &ProfileStore, cmd: OverflowCommand) -> Result<(), ProfileError> {
    use super::auto_switch::config::{self, ReadMode};
    let root = store.root();
    match cmd {
        OverflowCommand::Add { name } => {
            let name = ProfileName::parse(&name)?;
            // An existing profile directory, not a symlink.
            store.resolve(&name)?;
            let mut added = false;
            let c = config::update(root, ReadMode::Editable, |c| {
                if !c.overflow.contains(&name) {
                    c.overflow.push(name.clone());
                    added = true;
                }
                Ok(())
            })?;
            if added {
                println!("overflow: added {name}");
                println!(
                    "note: {name} is used only when every plan login is exhausted, and work on it is billed to that account's usage credits."
                );
                println!(
                    "note: set a spend limit for the organization at https://claude.ai/settings/usage; the rotator is not a spending cap."
                );
            } else {
                println!("overflow: {name} is already marked");
            }
            if c.exclude.contains(&name) {
                eprintln!(
                    "warning: {name} is in the auto-switch exclude list; excluded profiles are never used, even as overflow"
                );
            }
            if super::pointer::read(root).ok().flatten().as_ref() == Some(&name) {
                eprintln!(
                    "warning: the default profile is {name}; launches use it directly and spend credits whatever the rotator does (run: amplihack profile switch --none)"
                );
            }
        }
        OverflowCommand::Remove { name } => {
            let name = ProfileName::parse(&name)?;
            let mut removed = false;
            config::update(root, ReadMode::Editable, |c| {
                if c.overflow.contains(&name) {
                    c.overflow.retain(|n| n != &name);
                    removed = true;
                }
                Ok(())
            })?;
            if removed {
                println!("overflow: removed {name}");
            } else {
                println!("overflow: {name} is not marked");
            }
        }
        OverflowCommand::List => {
            let c = editable(root)?;
            if c.overflow.is_empty() {
                println!("overflow: none");
                return Ok(());
            }
            let blocks = match super::rotate::overflow_blocks(root, now()) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("warning: spend blocks not shown: {e}");
                    Default::default()
                }
            };
            for n in &c.overflow {
                let mut line = n.to_string();
                if store.resolve(n).is_err() {
                    line.push_str("  missing (no profile directory)");
                }
                if let Some(until) = blocks.get(n.as_str()) {
                    line.push_str(&format!(
                        "  blocked until {} (spend_limit)",
                        super::rotate::timestamp(*until)
                    ));
                }
                println!("{line}");
            }
        }
        OverflowCommand::Clear { name } => {
            let name = ProfileName::parse(&name)?;
            if super::rotate::clear_overflow_block(root, &name)? {
                println!("overflow: cleared the spend block on {name}");
            } else {
                println!("overflow: {name} has no spend block");
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn run(_store: &ProfileStore, _cmd: OverflowCommand) -> Result<(), ProfileError> {
    Err(ProfileError::RotateUnsupported)
}

/// The settings to list: the file's, or the defaults when it is missing.
/// A corrupt file is an error.
#[cfg(unix)]
fn editable(root: &Path) -> Result<super::auto_switch::config::Config, ProfileError> {
    use super::auto_switch::config::{self, Config, FileState};
    match config::read(root) {
        FileState::Valid(c) => Ok(c),
        FileState::Missing => Ok(Config::default()),
        FileState::Corrupt => Err(ProfileError::AutoSwitchConfig(format!(
            "{} is unreadable or invalid",
            config::FILE
        ))),
    }
}

#[cfg(unix)]
fn now() -> i64 {
    super::auto_switch::kit::now()
}
