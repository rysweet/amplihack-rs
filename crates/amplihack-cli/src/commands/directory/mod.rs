//! `amplihack directory`: search the index of installed skills and agents
//! (issue #3220).
//!
//! The index is written by `amplihack install` (see [`index`]). This command
//! only reads it: neither subcommand writes anything. It finds the index in
//! the installer's own staging folder, through the same function the
//! installer uses, so the two cannot disagree on its place.
//!
//! See docs/reference/directory-command.md.

pub mod index;

#[cfg(test)]
mod tests;

use anyhow::{Result, bail};
use clap::Subcommand;
use index::{Entry, EntryKind};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

#[derive(Subcommand, Debug)]
pub enum DirectoryCommands {
    /// Print every skill and agent whose name or description contains all
    /// the words, ignoring letter case. With no words, print the names only.
    ///
    /// Each match is one tab-separated line: type, name, description, path.
    Find {
        /// Words to look for. A quoted argument with spaces is matched as given.
        words: Vec<String>,
    },
    /// Print every entry of the index, in the same form as `find`.
    List,
}

/// Run a `directory` subcommand against the index of the installed framework.
pub fn run(command: DirectoryCommands) -> Result<()> {
    let staged = crate::commands::install::paths::staging_claude_dir()?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    run_in(&staged, command, &mut out)
}

/// Run `command` as [`run`] does and return what it printed to stdout.
#[cfg(test)]
pub(crate) fn run_for_test(command: DirectoryCommands) -> Result<String> {
    let staged = crate::commands::install::paths::staging_claude_dir()?;
    let mut out = Vec::new();
    run_in(&staged, command, &mut out)?;
    Ok(String::from_utf8(out)?)
}

fn run_in(staged: &Path, command: DirectoryCommands, out: &mut impl Write) -> Result<()> {
    let path = index::index_path(staged);
    let Some(entries) = index::read_index(&path)? else {
        bail!(
            "the skills and agents index is missing at {}; run `amplihack install` to write it",
            crate::commands::install::paths::sanitize_path(&path)
        );
    };
    match command {
        DirectoryCommands::List => print_entries(entries.iter(), out),
        DirectoryCommands::Find { words } if words.is_empty() => print_names(&entries, out),
        DirectoryCommands::Find { words } => {
            let matches: Vec<&Entry> = entries.iter().filter(|e| e.matches_all(&words)).collect();
            if matches.is_empty() {
                eprintln!("No skill or agent matches all of: {}", words.join(" "));
                return Ok(());
            }
            print_entries(matches.into_iter(), out)
        }
    }
}

fn print_entries<'a>(entries: impl Iterator<Item = &'a Entry>, out: &mut impl Write) -> Result<()> {
    for entry in entries {
        writeln!(
            out,
            "{}\t{}\t{}\t{}",
            entry.kind.label(),
            entry.name,
            entry.description,
            entry.path.display()
        )?;
    }
    Ok(())
}

fn print_names(entries: &[Entry], out: &mut impl Write) -> Result<()> {
    for (kind, heading) in [(EntryKind::Skill, "skills:"), (EntryKind::Agent, "agents:")] {
        let names: BTreeSet<&str> = entries
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.name.as_str())
            .collect();
        writeln!(out, "{heading}")?;
        for name in names {
            writeln!(out, "  {name}")?;
        }
    }
    Ok(())
}
