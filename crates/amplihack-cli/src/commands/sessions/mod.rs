//! `amplihack sessions archive` / `restore` (issue #1746): moves old Claude
//! Code session logs out of `~/.claude/projects` into compressed archives,
//! and puts one back. The rotator runs the archive once a day
//! (`profile/rotate/sessions_daily.rs`). See
//! docs/reference/sessions-command.md.
//!
//! Session contents are copied byte for byte and never parsed or printed.
//! No message holds the text of an I/O or parser error: failures exit 1
//! through `std::process::exit` after one fixed line, never through `Err`.

pub(crate) mod archive;
pub(crate) mod config;
mod fmt;
pub(crate) mod live;
mod restore;
mod scan;
mod store;

#[cfg(test)]
mod tests_archive;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_fmt;
#[cfg(test)]
mod tests_live;
#[cfg(test)]
mod tests_scan;
#[cfg(test)]
mod tests_store;

pub(crate) use fmt::kind_word;

use crate::commands::profile::{ROOT_ENV, store_from_env};
use amplihack_utils::rotator_status::home_from_env;
use clap::Subcommand;
use std::io::ErrorKind;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum SessionsCommands {
    /// Archive old Claude Code sessions (a dry run unless --apply)
    Archive {
        /// A session is old when nothing in it was written for N days
        /// (default: sessions.archive_days, else 3)
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
        days: Option<u32>,
        /// Archive directory (default: sessions.archive_dest, else
        /// ~/.amplihack/session-archive)
        #[arg(long, value_name = "DIR")]
        dest: Option<PathBuf>,
        /// Archive and remove the originals; without it nothing changes
        #[arg(long)]
        apply: bool,
    },
    /// Restore one archived session into ~/.claude/projects
    Restore {
        /// The session id to restore
        session_id: String,
        /// Archive directory (default: sessions.archive_dest, else
        /// ~/.amplihack/session-archive)
        #[arg(long, value_name = "DIR")]
        dest: Option<PathBuf>,
    },
}

/// Prints `line` on stderr and exits 1.
fn fail(line: &str) -> ! {
    eprintln!("{line}");
    std::process::exit(1)
}

pub fn run_sessions(cmd: SessionsCommands) -> anyhow::Result<()> {
    // SAFETY: geteuid and getuid have no preconditions.
    if unsafe { libc::geteuid() != libc::getuid() } {
        fail("refusing to run with an effective user id different from the real user id");
    }
    let env = |k: &str| std::env::var_os(k);
    let mut warn = |l: &str| eprintln!("{l}");
    match cmd {
        SessionsCommands::Archive { days, dest, apply } => {
            let opts = config::resolve(days, dest, apply, home_from_env())
                .unwrap_or_else(|e| fail(&e.to_string()));
            // Without the profile root the live check cannot see every
            // profile's sessions, so it fails closed.
            let profile_root = match store_from_env() {
                Ok((store, _)) => store.root().to_path_buf(),
                Err(_) => {
                    let root =
                        env(ROOT_ENV).map_or_else(|| opts.home.join(".amplihack"), PathBuf::from);
                    fail(
                        &archive::ArchiveError::LiveUnreadable(root, ErrorKind::InvalidData)
                            .to_string(),
                    )
                }
            };
            let report = archive::run(&opts, &profile_root, &live::SystemProbe, &mut warn, &|| {
                false
            })
            .unwrap_or_else(|e| fail(&e.to_string()));
            println!("{}", archive::summary(&report, opts.days, opts.apply));
            if report.failed > 0 {
                std::process::exit(1);
            }
        }
        SessionsCommands::Restore { session_id, dest } => {
            let opts = config::resolve(Some(1), dest, true, home_from_env())
                .unwrap_or_else(|e| fail(&e.to_string()));
            let report = restore::run(&session_id, &opts.home, &opts.dest, &mut warn)
                .unwrap_or_else(|e| fail(&e.to_string()));
            println!("{}", report.summary());
            if report.conflicts > 0 {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
