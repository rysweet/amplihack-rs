//! One notice for a narrowing install or launch publication.
//!
//! Publishers collect completed removals while an installation operation is
//! active. Its final writer saves one block and may print that same block.
//! List-setting commands do not start an operation and never write notices.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

use amplihack_utils::list_selection::ListSelection;
use anyhow::{Context, Result, bail};

use crate::claude_plugin::PluginRegistration;
use crate::commands::directory::index::{self, EntryKind};

const NOTICE: &str = "list-changes.txt";

#[derive(Default)]
struct Removals {
    skills: BTreeSet<String>,
    agents: BTreeSet<String>,
}

#[derive(Default)]
struct Publication {
    staged: PathBuf,
    tools: BTreeMap<&'static str, Removals>,
}

thread_local! {
    static PUBLICATION: RefCell<Option<Publication>> = const { RefCell::new(None) };
}

/// Nested launch repairs contribute to their enclosing installation operation.
pub(crate) struct Operation(bool);

pub(crate) fn begin() -> Operation {
    Operation(PUBLICATION.with(|publication| {
        let mut publication = publication.borrow_mut();
        if publication.is_some() {
            false
        } else {
            *publication = Some(Publication::default());
            true
        }
    }))
}

impl Drop for Operation {
    fn drop(&mut self) {
        if self.0 {
            let publication = PUBLICATION.with(|publication| publication.take());
            if let Some(publication) = publication {
                let eligible = eligible_in(
                    std::io::stderr().is_terminal(),
                    std::env::var_os("AMPLIHACK_RECIPE_RUN_ID").is_some_and(|id| !id.is_empty()),
                );
                warn(write_notice(&publication, &mut std::io::stderr(), eligible));
            }
        }
    }
}

/// Notice failures never change the result of a completed publication.
pub(crate) fn record(staged: &Path, tool: &'static str, report: &PluginRegistration) {
    PUBLICATION.with(|publication| {
        let mut publication = publication.borrow_mut();
        let Some(publication) = publication.as_mut() else {
            return;
        };
        if report.removed_skills.is_empty() && report.removed_agents.is_empty() {
            return;
        }
        match bundled_removals(staged, tool, report, &crate::user_config::list_selection()) {
            Ok(removals) if !removals.skills.is_empty() || !removals.agents.is_empty() => {
                publication.staged = staged.to_path_buf();
                let counts = publication.tools.entry(tool).or_default();
                counts.skills.extend(removals.skills);
                counts.agents.extend(removals.agents);
            }
            Ok(_) => {}
            Err(error) => warn(Err(error)),
        }
    });
}

fn bundled_removals(
    staged: &Path,
    tool: &str,
    report: &PluginRegistration,
    selection: &ListSelection,
) -> Result<Removals> {
    let mut removed = Removals::default();
    for entry in index::collect_entries(staged)? {
        match entry.kind {
            EntryKind::Skill if !selection.includes_skill(&entry.name) => {
                let category = entry
                    .path
                    .strip_prefix(staged.join("skills"))?
                    .components()
                    .next()
                    .context("skill has no directory")?
                    .as_os_str()
                    .to_string_lossy();
                if report.removed_skills.contains(&entry.name)
                    || (tool == "Copilot" && report.removed_skills.contains(category.as_ref()))
                {
                    removed.skills.insert(entry.name);
                }
            }
            EntryKind::Agent => {
                let identity = entry
                    .path
                    .strip_prefix(staged.join("agents"))?
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                let identity = identity.strip_prefix("amplihack/").unwrap_or(&identity);
                let published = match tool {
                    "Copilot" => identity.rsplit('/').next().unwrap_or(identity).to_owned(),
                    "Codex" => identity.replace('/', "-"),
                    _ => identity.to_owned(),
                };
                if !selection.includes_agent(identity) && report.removed_agents.contains(&published)
                {
                    removed.agents.insert(identity.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok(removed)
}

fn eligible_in(terminal: bool, recipe_step: bool) -> bool {
    terminal && !recipe_step
}

fn warn(result: Result<()>) {
    if let Err(error) = result {
        let _ = writeln!(
            std::io::stderr(),
            "Warning: could not save or print list changes: {error:#}"
        );
    }
}

fn write_notice(publication: &Publication, out: &mut impl Write, eligible: bool) -> Result<()> {
    if publication.tools.is_empty() {
        return Ok(());
    }
    let dir = publication.staged.join("install");
    let path = dir.join(NOTICE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => bail!("list notice destination must be a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut text = String::from(
        "amplihack now lists a smaller selection. These entries remain installed in the complete framework.\n",
    );
    for (tool, removed) in &publication.tools {
        text.push_str(&format!(
            "{tool}: {} skills and {} agents left its lists.\n",
            removed.skills.len(),
            removed.agents.len()
        ));
    }
    text.push_str("Find an entry: amplihack directory find <words>\nKeep an extra entry listed: amplihack directory enable <name>\nRestore the full lists: amplihack config set lists.set all\nIn Codex, generated command skills leave with default and return with lists.set all. They cannot be enabled individually.\n");
    fs::create_dir_all(&dir)?;
    let mut file = tempfile::NamedTempFile::new_in(&dir)?;
    file.write_all(text.as_bytes())?;
    match file.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    if eligible {
        out.write_all(text.as_bytes())?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
