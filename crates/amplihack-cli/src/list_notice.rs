//! Save actual list migrations and show them once at an eligible terminal.
//!
//! Publishers report completed removals. A fresh installation has nothing to
//! report. The readable history and display position are separate from tool
//! ownership records, so older installers can still read those records.

use std::{
    fs::{self, OpenOptions},
    io::{IsTerminal, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use fs2::FileExt;

use crate::claude_plugin::PluginRegistration;

const HISTORY: &str = "list-changes.txt";
const SHOWN: &str = "list-changes-shown.json";

/// Record only entries actually removed by an existing publisher.
pub(crate) fn record(staged: &Path, tool: &str, report: &PluginRegistration) -> Result<()> {
    if report.removed_skills.is_empty() && report.removed_agents.is_empty() {
        return Ok(());
    }
    let dir = staged.join("install");
    with_lock(&dir, || {
        let path = dir.join(HISTORY);
        let mut text = read_optional(&path)?.unwrap_or_default();
        writeln!(
            text,
            "amplihack now lists a smaller selection for {tool}: {} skills and {} agents left its lists. They remain installed in the complete framework.",
            report.removed_skills.len(),
            report.removed_agents.len(),
        )?;
        if !report.preserved.is_empty() {
            writeln!(text, "Edited installer-owned skills left in place:")?;
            for entry in &report.preserved {
                writeln!(
                    text,
                    "  {}",
                    crate::commands::install::paths::sanitize_text(&entry.name)
                )?;
            }
        }
        writeln!(text, "Find an entry: amplihack directory find <words>")?;
        writeln!(
            text,
            "Keep an extra entry listed: amplihack directory enable <name>"
        )?;
        writeln!(
            text,
            "Restore the full lists: amplihack config set lists.set all\n"
        )?;
        replace(&path, &text)
    })?;
    report_display_error(show_pending_at(staged, &mut std::io::stderr(), eligible()));
    Ok(())
}

fn eligible() -> bool {
    eligible_in(
        std::io::stderr().is_terminal(),
        std::env::var_os("AMPLIHACK_RECIPE_RUN_ID").is_some_and(|id| !id.is_empty()),
    )
}

fn eligible_in(terminal: bool, recipe_step: bool) -> bool {
    terminal && !recipe_step
}

/// Called by ordinary commands as well as publishers, to show deferred changes.
/// Display failures are warnings so inspection and recovery remain available.
pub(crate) fn show_pending() {
    if !eligible() {
        return;
    }
    report_display_error(
        crate::commands::install::paths::staging_claude_dir()
            .and_then(|staged| show_pending_at(&staged, &mut std::io::stderr(), true)),
    );
}

fn report_display_error(result: Result<()>) {
    // Preserve strict validation and pending state without blocking recovery
    // commands or publication that has already saved its notice history.
    if let Err(error) = result {
        eprintln!("Warning: could not display list changes: {error:#}");
    }
}

fn show_pending_at(staged: &Path, out: &mut impl Write, eligible: bool) -> Result<()> {
    let dir = staged.join("install");
    if !eligible || !dir.join(HISTORY).try_exists()? {
        return Ok(());
    }
    with_lock(&dir, || {
        let text = read_optional(&dir.join(HISTORY))?.unwrap_or_default();
        let shown: usize = read_optional(&dir.join(SHOWN))?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .context("cannot read the list notice display position")?
            .unwrap_or(0);
        ensure!(
            shown <= text.len(),
            "list notice history was shortened; review its display record"
        );
        if shown != text.len() {
            out.write_all(&text[shown..])?;
            out.flush()?;
            replace(&dir.join(SHOWN), &serde_json::to_vec(&text.len())?)?;
        }
        Ok(())
    })
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "list notice must be a regular file"
            );
            Ok(Some(fs::read(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let _ = read_optional(path)?;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("list notice has no parent")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).context("saving list notice")?;
    Ok(())
}

fn with_lock<T>(dir: &Path, action: impl FnOnce() -> Result<T>) -> Result<T> {
    fs::create_dir_all(dir)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600);
    }
    let lock = options.open(dir.join("list-changes.lock"))?;
    ensure!(
        lock.metadata()?.is_file(),
        "list notice lock must be a regular file"
    );
    lock.lock_exclusive()?;
    action()
}

#[cfg(test)]
mod tests;
