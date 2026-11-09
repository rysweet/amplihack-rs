//! Issue #2018: `amplihack recipe steps [--orphans] [-f table|json]`.
//!
//! Lists every step record under `~/.amplihack/runtime/recipe-steps` with
//! its state (live, orphan, stale or unreadable). It only reads: it never
//! sends a signal and never removes or changes a file.
//!
//! Exit codes (D9): 0 when the listing completed, including a missing or
//! empty root; 1 when `HOME` is unusable or the root is a symlink, not a
//! directory, or not owned by this user; 2 for a usage error (clap).

use anyhow::Result;

#[cfg(unix)]
pub fn run_steps(orphans: bool, format: &str) -> Result<()> {
    imp::run_steps(orphans, format)
}

#[cfg(not(unix))]
pub fn run_steps(_orphans: bool, _format: &str) -> Result<()> {
    anyhow::bail!("amplihack recipe steps is available on Unix only")
}

#[cfg(unix)]
mod imp {
    use crate::command_error::exit_error;
    use crate::commands::install::paths::{sanitize_path, sanitize_text};
    use crate::step_record::{self, Listed, StepRecord, SystemProbe, classify};
    use anyhow::Result;
    use serde::Serialize;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    const HEADER: [&str; 10] = [
        "STATE", "RUN ID", "TOOL", "LAUNCHER", "AGENT", "PARENT", "CREATED", "CWD", "RECORD",
        "REASON",
    ];

    /// One output entry. JSON always carries all four keys.
    #[derive(Serialize)]
    struct Entry {
        state: &'static str,
        path: String,
        record: Option<StepRecord>,
        reason: Option<String>,
        #[serde(skip)]
        raw_path: PathBuf,
    }

    pub(super) fn run_steps(orphans: bool, format: &str) -> Result<()> {
        let Some(home) = crate::commands::recipe::home_dir()
            .ok()
            .filter(|home| home.is_absolute())
        else {
            return refuse("HOME is not set");
        };
        let root = step_record::root(&home);
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        if let Ok(meta) = std::fs::symlink_metadata(&root) {
            let safe = meta.file_type().is_dir() && meta.uid() == uid;
            if !safe {
                return refuse(&format!("unsafe directory {}", sanitize_path(&root)));
            }
        }

        let entries: Vec<Entry> = step_record::list_as(&root, uid)
            .into_iter()
            .map(entry)
            .filter(|e| !orphans || matches!(e.state, "orphan" | "unreadable"))
            .collect();

        if format == "json" {
            println!("{}", serde_json::to_string_pretty(&entries)?);
        } else {
            print!("{}", render_table(&root, &entries));
        }
        Ok(())
    }

    /// The one stderr line for exit 1.
    fn refuse(reason: &str) -> Result<()> {
        eprintln!("amplihack: cannot list step records: {reason}");
        Err(exit_error(1))
    }

    fn entry(listed: Listed) -> Entry {
        match listed {
            Listed::Record { path, record } => Entry {
                state: classify(&record, &SystemProbe).as_str(),
                path: path.to_string_lossy().into_owned(),
                record: Some(record),
                reason: None,
                raw_path: path,
            },
            Listed::Unreadable { path, reason } => Entry {
                state: "unreadable",
                path: path.to_string_lossy().into_owned(),
                record: None,
                reason: Some(reason),
                raw_path: path,
            },
        }
    }

    /// The run id from the entry's path: the first component under the
    /// root, or `-` for the root itself.
    fn run_id_cell(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .ok()
            .and_then(|rest| rest.components().next())
            .map_or_else(
                || "-".to_string(),
                |c| c.as_os_str().to_string_lossy().into_owned(),
            )
    }

    fn row(root: &Path, entry: &Entry) -> [String; 10] {
        let dash = || "-".to_string();
        let (tool, launcher, agent, parent, created, cwd) = match &entry.record {
            Some(r) => (
                r.tool.clone(),
                r.launcher.pid.to_string(),
                r.agent.pid.to_string(),
                r.parent.map_or_else(dash, |p| p.pid.to_string()),
                r.created_at.clone(),
                r.cwd.clone(),
            ),
            None => (dash(), dash(), dash(), dash(), dash(), dash()),
        };
        [
            entry.state.to_string(),
            run_id_cell(root, &entry.raw_path),
            tool,
            launcher,
            agent,
            parent,
            created,
            cwd,
            entry.path.clone(),
            entry.reason.clone().unwrap_or_else(dash),
        ]
        .map(|cell| sanitize_text(&cell))
    }

    /// Columns padded to their widest cell and separated by two spaces.
    /// Nothing at all, not even the header, when there are no entries.
    fn render_table(root: &Path, entries: &[Entry]) -> String {
        if entries.is_empty() {
            return String::new();
        }
        let mut rows = vec![HEADER.map(str::to_string)];
        rows.extend(entries.iter().map(|e| row(root, e)));
        let mut widths = [0usize; 10];
        for r in &rows {
            for (w, cell) in widths.iter_mut().zip(r) {
                *w = (*w).max(cell.chars().count());
            }
        }
        let mut out = String::new();
        for r in &rows {
            let mut line = String::new();
            for (i, cell) in r.iter().enumerate() {
                line.push_str(cell);
                if i + 1 < r.len() {
                    let pad = widths[i] - cell.chars().count() + 2;
                    line.extend(std::iter::repeat_n(' ', pad));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}
