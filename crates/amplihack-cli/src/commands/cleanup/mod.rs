//! `amplihack cleanup`: remove or archive what recipe runs leave behind
//! (issue #1808). The specification is
//! `docs/howto/clean-up-after-recipe-runs.md`.

pub(crate) mod archive;
pub(crate) mod eligibility;
pub(crate) mod run_dir;
pub(crate) mod sweep;
pub(crate) mod targets;
pub(crate) mod work_root;

use crate::command_error::exit_error;
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sweep::{Entry, Settings};

pub(crate) const DEFAULT_KEEP_HOURS: u64 = 24;
const MAX_KEEP_HOURS: u64 = 87_600;
const DEFAULT_LOG_DAYS: u64 = 30;
const MAX_LOG_DAYS: u64 = 36_500;
const KEEP_HOURS_ENV: &str = "AMPLIHACK_CLEANUP_KEEP_HOURS";
const LOG_DAYS_ENV: &str = "AMPLIHACK_LOG_ARCHIVE_DAYS";
const ARCHIVE_DIR_ENV: &str = "AMPLIHACK_LOG_ARCHIVE_DIR";
pub(crate) const CLEANUP_ON_EXIT_ENV: &str = "AMPLIHACK_CLEANUP_ON_EXIT";
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A cleanup category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum)]
pub enum Category {
    RunTmp,
    RunWorktree,
    RunTarget,
    UntaggedTarget,
    TmpLeftover,
    RecipeLog,
    SessionLog,
}

impl Category {
    pub(crate) const ALL: [Category; 7] = [
        Self::RunTmp,
        Self::RunWorktree,
        Self::RunTarget,
        Self::UntaggedTarget,
        Self::TmpLeftover,
        Self::RecipeLog,
        Self::SessionLog,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RunTmp => "run-tmp",
            Self::RunWorktree => "run-worktree",
            Self::RunTarget => "run-target",
            Self::UntaggedTarget => "untagged-target",
            Self::TmpLeftover => "tmp-leftover",
            Self::RecipeLog => "recipe-log",
            Self::SessionLog => "session-log",
        }
    }

    fn is_log(self) -> bool {
        matches!(self, Self::RecipeLog | Self::SessionLog)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Remove or archive what recipe runs leave behind. Without --apply this is
/// a dry run that writes nothing.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct CleanupArgs {
    #[command(subcommand)]
    pub command: Option<CleanupCommand>,
    /// Remove and archive. Without it the command is a dry run.
    #[arg(long)]
    pub apply: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
    /// Overrides AMPLIHACK_CLEANUP_KEEP_HOURS (default 24).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(0..=87_600))]
    pub keep_hours: Option<u64>,
    /// Overrides AMPLIHACK_LOG_ARCHIVE_DAYS (default 30).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(0..=36_500))]
    pub log_days: Option<u64>,
    /// Limit to one category. Repeat for several.
    #[arg(
        long,
        value_enum,
        value_name = "CATEGORY",
        conflicts_with = "skip_logs"
    )]
    pub only: Vec<Category>,
    /// Leave recipe-log and session-log alone.
    #[arg(long)]
    pub skip_logs: bool,
}

#[derive(Debug, Subcommand)]
pub enum CleanupCommand {
    /// Restore archived logs from a manifest or one archive. Never overwrites.
    Restore {
        /// MANIFEST.jsonl, or one `.tar.gz` archive listed in it.
        path: PathBuf,
        /// Restore under this folder instead of the original paths.
        #[arg(long)]
        dest: Option<PathBuf>,
    },
}

/// Entry point for `amplihack cleanup`.
pub fn run(args: CleanupArgs) -> Result<()> {
    let outcome = match args.command {
        Some(CleanupCommand::Restore { path, dest }) => run_restore(&path, dest.as_deref()),
        None => run_cleanup(&args),
    };
    match outcome {
        Ok(true) => Ok(()),
        Ok(false) => Err(exit_error(1)),
        Err(error) => {
            eprintln!("amplihack cleanup: {error:#}");
            Err(exit_error(1))
        }
    }
}

fn bounded_env(name: &str, max: u64, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Ok(value) if value.trim().is_empty() => Ok(default),
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(n) if n <= max => Ok(n),
            _ => bail!("{name} must be a whole number from 0 to {max}, got {value:?}"),
        },
        Err(std::env::VarError::NotUnicode(_)) => bail!("{name} is not valid UTF-8"),
    }
}

/// `AMPLIHACK_CLEANUP_KEEP_HOURS`, or the default.
pub(crate) fn keep_hours_from_env() -> Result<u64> {
    bounded_env(KEEP_HOURS_ENV, MAX_KEEP_HOURS, DEFAULT_KEEP_HOURS)
}

/// The base folder `git-hook-cargo.sh` uses when `AMPLIHACK_WORK_ROOT` is
/// unset: `${XDG_CACHE_HOME:-$HOME/.cache}/amplihack`.
pub(crate) fn hook_cache_base() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::user_config::home_dir().map(|h| h.join(".cache")))?;
    base.is_absolute().then(|| base.join("amplihack"))
}

struct LogRoots {
    recipe: Option<PathBuf>,
    codex: Option<PathBuf>,
    claude: Option<PathBuf>,
}

fn log_roots() -> LogRoots {
    let home = crate::user_config::home_dir();
    let recipe = crate::user_config::recipe_log_dir().or_else(|| {
        home.as_ref()
            .map(|h| h.join(".amplihack").join("recipe-logs"))
    });
    let codex = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".codex")))
        .map(|c| c.join("sessions"));
    let claude = home.map(|h| h.join(".claude").join("projects"));
    LogRoots {
        recipe,
        codex,
        claude,
    }
}

impl LogRoots {
    fn all(&self) -> Vec<PathBuf> {
        [&self.recipe, &self.codex, &self.claude]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }
}

/// The canonical form of `path`, resolving the deepest existing ancestor and
/// appending the rest, so a folder that does not exist yet can be compared.
fn resolve_partial(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while std::fs::symlink_metadata(&existing).is_err() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for part in rest.into_iter().rev() {
        out.push(part);
    }
    out
}

fn check_archive_overlap(archive: &Path, roots: &[PathBuf]) -> Result<()> {
    let a = resolve_partial(archive);
    for root in roots {
        let r = resolve_partial(root);
        if a.starts_with(&r) || r.starts_with(&a) {
            bail!(
                "archive folder {} overlaps log folder {}; choose another AMPLIHACK_LOG_ARCHIVE_DIR",
                archive.display(),
                root.display()
            );
        }
    }
    Ok(())
}

fn run_cleanup(args: &CleanupArgs) -> Result<bool> {
    let work_root = work_root::resolve_from_env()?;
    let keep_hours = match args.keep_hours {
        Some(h) => h,
        None => keep_hours_from_env()?,
    };
    let log_days = match args.log_days {
        Some(d) => d,
        None => bounded_env(LOG_DAYS_ENV, MAX_LOG_DAYS, DEFAULT_LOG_DAYS)?,
    };
    let archive_dir = match std::env::var_os(ARCHIVE_DIR_ENV).filter(|v| !v.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                bail!("{ARCHIVE_DIR_ENV} must be an absolute path");
            }
            dir
        }
        None => work_root.path.join("archive"),
    };
    let mut categories: BTreeSet<Category> = if args.only.is_empty() {
        Category::ALL.into_iter().collect()
    } else {
        args.only.iter().copied().collect()
    };
    if args.skip_logs {
        categories.retain(|c| !c.is_log());
    }
    let roots = log_roots();
    if categories.iter().any(|c| c.is_log()) {
        check_archive_overlap(&archive_dir, &roots.all())?;
    }
    let settings = Settings {
        work_root: work_root.clone(),
        keep: Duration::from_secs(keep_hours * 3600),
        log_age: Duration::from_secs(log_days * 86_400),
        archive_dir,
        categories,
        apply: args.apply,
        report_unrecorded: true,
        deadline: None,
        recipe_log_dir: roots.recipe.clone(),
        codex_sessions_dir: roots.codex.clone(),
        claude_projects_dir: roots.claude.clone(),
        hook_cache_base: hook_cache_base(),
    };

    let lock = if args.apply {
        work_root.ensure_created()?;
        sweep::lock_with_timeout(&work_root.lock_path(), LOCK_WAIT)?
    } else {
        None
    };
    if args.apply && lock.is_none() {
        eprintln!(
            "amplihack cleanup: another cleanup holds {}; every item is skipped",
            work_root.lock_path().display()
        );
    }
    let entries = sweep::run_sweep(&settings, lock.is_some())?;
    drop(lock);

    match args.format {
        OutputFormat::Json => print_json(&entries, &settings)?,
        OutputFormat::Text => print_text(&entries),
    }
    Ok(!entries.iter().any(|e| e.action == sweep::Action::Failed))
}

fn print_json(entries: &[Entry], settings: &Settings) -> Result<()> {
    let list: Vec<_> = entries
        .iter()
        .map(|e| {
            json!({
                "category": e.category.as_str(),
                "path": e.path.display().to_string(),
                "size_bytes": e.size_bytes,
                "reason": e.reason,
                "action": e.action.as_str(),
                "run_id": e.run_id,
            })
        })
        .collect();
    let report = json!({
        "dry_run": !settings.apply,
        "work_root": settings.work_root.path.display().to_string(),
        "entries": list,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// `1.8 MiB`, `612 KiB`, `40 B`.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn print_text(entries: &[Entry]) {
    if entries.is_empty() {
        println!("Nothing to clean up.");
        return;
    }
    println!("{:<15} {:<30} {:>8}  PATH", "CATEGORY", "ACTION", "SIZE");
    for e in entries {
        let size = e.size_bytes.map_or_else(|| "-".to_string(), human_size);
        println!(
            "{:<15} {:<30} {:>8}  {}",
            e.category.as_str(),
            e.action.as_str(),
            size,
            e.path.display()
        );
        println!("{:<15} reason: {}", "", e.reason);
    }
}

fn run_restore(path: &Path, dest: Option<&Path>) -> Result<bool> {
    let roots = log_roots().all();
    let outcome = archive::restore(path, dest, &roots).context("restore failed")?;
    for restored in &outcome.restored {
        println!("restored {}", restored.display());
    }
    for error in &outcome.errors {
        eprintln!("amplihack cleanup: {error}");
    }
    Ok(outcome.errors.is_empty())
}

/// Whether the end-of-run sweep is enabled (`AMPLIHACK_CLEANUP_ON_EXIT`).
pub(crate) fn cleanup_on_exit_enabled() -> bool {
    std::env::var(CLEANUP_ON_EXIT_ENV).map_or(true, |v| v.trim() != "0")
}

#[cfg(test)]
#[path = "tests_cli.rs"]
mod tests_cli;
