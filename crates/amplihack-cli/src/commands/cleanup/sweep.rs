//! Collect and apply cleanup candidates (issue #1808, C4, D5, D6, D9).
//!
//! Containment in the managed roots, at the expected depth, is the only
//! authority to delete. `owned-paths` is a request, not a permission. Each
//! removal resolves the path again, refuses symbolic links and repeats the
//! liveness, Cargo-lock and cleanliness checks immediately before acting.

use super::Category;
use super::archive::archive_file;
use super::eligibility::{
    PrInfo, Verdict, is_valid_branch, is_valid_id, lookup_pr, no_pr_verdict, pr_verdict,
    run_is_active,
};
use super::run_dir::{
    CLEANUP_RECORD, OwnedKind, OwnedPath, RUN_JSON, RUN_SCHEMA_VERSION, RunReport,
    format_owned_line, now_rfc3339, read_capped_nofollow, read_owned_paths, short_id,
    write_atomic_0600,
};
use super::work_root::WorkRoot;
use crate::commands::hygiene::cleanup::{
    git_has_dirty_state, git_has_unpushed_commits, hardened_git, sanitized_stderr_short,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Size and age walks stop after this many entries.
const WALK_LIMIT: usize = 1_000_000;
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const HOOK_TARGET_PREFIX: &str = "amplihack-precommit-target-";
const TMP_PREFIXES: &[&str] = &["replay-", "release-", "amplihack-workflow-"];
pub(crate) const END_OF_RUN_DEADLINE: Duration = Duration::from_secs(120);
const RECORD_MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) const REASON_LOCK_HELD: &str = "cleanup lock held";
pub(crate) const REASON_AWAITING_1766: &str = "awaiting sessions archive (#1766)";

/// Everything a sweep needs to know.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub work_root: WorkRoot,
    pub keep: Duration,
    pub log_age: Duration,
    pub archive_dir: PathBuf,
    pub categories: BTreeSet<Category>,
    pub apply: bool,
    /// Report linked worktrees that no run recorded (skipped).
    pub report_unrecorded: bool,
    pub deadline: Option<Instant>,
    pub recipe_log_dir: Option<PathBuf>,
    pub codex_sessions_dir: Option<PathBuf>,
    pub claude_projects_dir: Option<PathBuf>,
    pub hook_cache_base: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    WouldRemove,
    Removed,
    WouldArchive,
    Archived,
    Skipped,
    Failed,
}

impl Action {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WouldRemove => "would-remove",
            Self::Removed => "removed",
            Self::WouldArchive => "would-archive",
            Self::Archived => "archived",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub category: Category,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
    pub reason: String,
    pub action: Action,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone)]
enum Op {
    Skip,
    RemoveRunTmp {
        run_dir: PathBuf,
    },
    RemoveWorktree {
        main: PathBuf,
    },
    RemoveDir {
        cargo_lock: bool,
        check_in_use: bool,
    },
    Archive {
        relative: PathBuf,
    },
}

#[derive(Debug, Clone)]
struct Item {
    category: Category,
    path: PathBuf,
    size_bytes: Option<u64>,
    reason: String,
    run_id: Option<String>,
    op: Op,
}

impl Item {
    fn skip(
        category: Category,
        path: &Path,
        reason: impl Into<String>,
        run_id: Option<&str>,
    ) -> Self {
        Self {
            category,
            path: path.to_path_buf(),
            size_bytes: None,
            reason: reason.into(),
            run_id: run_id.map(String::from),
            op: Op::Skip,
        }
    }
}

// ----- processes ------------------------------------------------------------

/// Working folders and open files of every other process this user can see.
struct ProcScan {
    available: bool,
    paths: Vec<PathBuf>,
}

impl ProcScan {
    fn take() -> Self {
        let Ok(entries) = fs::read_dir("/proc") else {
            return Self {
                available: false,
                paths: Vec::new(),
            };
        };
        let me = std::process::id().to_string();
        let mut paths = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == me || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let base = entry.path();
            if let Ok(cwd) = fs::read_link(base.join("cwd")) {
                paths.push(cwd);
            }
            if let Ok(fds) = fs::read_dir(base.join("fd")) {
                for fd in fds.flatten() {
                    if let Ok(target) = fs::read_link(fd.path())
                        && target.is_absolute()
                    {
                        paths.push(target);
                    }
                }
            }
        }
        Self {
            available: true,
            paths,
        }
    }

    /// Whether a process has its working folder in `path` or a file open in it.
    /// Without `/proc`, everything counts as in use.
    fn in_use(&self, path: &Path) -> bool {
        if !self.available {
            return true;
        }
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.paths
            .iter()
            .any(|p| p.starts_with(&canonical) || p.starts_with(path))
    }
}

// ----- filesystem helpers ---------------------------------------------------

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// A real folder: exists, is a directory and is not a symlink.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

fn canonical_or_self(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Total bytes under `path`, not following symlinks. `None` past the limit.
fn tree_size(path: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        seen += 1;
        if seen > WALK_LIMIT {
            return None;
        }
        let meta = fs::symlink_metadata(&p).ok()?;
        if meta.file_type().is_dir() {
            total = total.saturating_add(meta.len());
            if let Ok(rd) = fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    Some(total)
}

/// The newest mtime under `path`, not following symlinks. `None` past the
/// limit, which callers treat as "recent".
fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        seen += 1;
        if seen > WALK_LIMIT {
            return None;
        }
        let meta = fs::symlink_metadata(&p).ok()?;
        if let Ok(m) = meta.modified()
            && m > newest
        {
            newest = m;
        }
        if meta.file_type().is_dir()
            && let Ok(rd) = fs::read_dir(&p)
        {
            stack.extend(rd.flatten().map(|e| e.path()));
        }
    }
    Some(newest)
}

/// Whether `path` was untouched for at least `keep`. A future time counts as
/// recent.
fn idle_for(path: &Path, now: SystemTime, keep: Duration) -> Option<Duration> {
    let newest = newest_mtime(path)?;
    let age = now.duration_since(newest).ok()?;
    (age >= keep).then_some(age)
}

/// Whether a build holds `.cargo-lock` in `dir` or one level below it.
fn cargo_lock_held(dir: &Path) -> bool {
    let mut locks = vec![dir.join(".cargo-lock")];
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                locks.push(entry.path().join(".cargo-lock"));
            }
        }
    }
    locks.iter().any(|lock| lock_is_held(lock))
}

fn lock_is_held(lock: &Path) -> bool {
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock)
    else {
        return false;
    };
    use std::os::fd::AsRawFd;
    // SAFETY: the fd is valid for the life of `file`; closing it releases the lock.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    rc != 0
}

/// Takes an exclusive, non-blocking lock on `path` (created 0600, never
/// truncated). `Ok(None)` when another process holds it.
pub(crate) fn try_lock(path: &Path) -> Result<Option<fs::File>> {
    use std::os::fd::AsRawFd;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    // SAFETY: the fd is valid for the life of `file`.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else {
        Err(error).with_context(|| format!("cannot lock {}", path.display()))
    }
}

/// [`try_lock`], retried until `wait` has passed.
pub(crate) fn lock_with_timeout(path: &Path, wait: Duration) -> Result<Option<fs::File>> {
    let until = Instant::now() + wait;
    loop {
        if let Some(file) = try_lock(path)? {
            return Ok(Some(file));
        }
        if Instant::now() >= until {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Files under `root`, not following symlinks, with their path relative to it.
fn walk_files(root: &Path) -> Vec<(PathBuf, fs::Metadata)> {
    let mut out = Vec::new();
    if !is_real_dir(root) {
        return out;
    }
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            seen += 1;
            if seen > WALK_LIMIT {
                return out;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_dir() {
                stack.push(path);
            } else if meta.file_type().is_file() {
                out.push((path, meta));
            }
        }
    }
    out
}

// ----- git helpers ----------------------------------------------------------

fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let mut command = hardened_git();
    command
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-common-dir"]);
    let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let raw = PathBuf::from(text.trim());
    let abs = if raw.is_absolute() {
        raw
    } else {
        dir.join(raw)
    };
    fs::canonicalize(abs).ok()
}

/// The main checkout of the repository at `dir` (`None` for a bare repo).
fn main_checkout(common_dir: &Path) -> Option<PathBuf> {
    (common_dir.file_name()? == ".git").then(|| common_dir.parent().map(Path::to_path_buf))?
}

/// `git worktree list --porcelain` paths of `main`, first entry first.
fn worktree_list(main: &Path) -> Result<Vec<PathBuf>> {
    let mut command = hardened_git();
    command
        .arg("-C")
        .arg(main)
        .args(["worktree", "list", "--porcelain"]);
    let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
    if !output.status.success() {
        bail!(
            "git worktree list failed: {}",
            sanitized_stderr_short(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|p| canonical_or_self(Path::new(p)))
        .collect())
}

// ----- runs -----------------------------------------------------------------

struct RunInfo {
    id: String,
    dir: PathBuf,
    report: Result<RunReport, String>,
    owned: Vec<OwnedPath>,
    run_json_mtime: SystemTime,
}

fn scan_runs(root: &WorkRoot) -> Vec<RunInfo> {
    let runs_dir = root.runs_dir();
    if !is_real_dir(&runs_dir) {
        return Vec::new();
    }
    let Ok(rd) = fs::read_dir(&runs_dir) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for entry in rd.flatten() {
        let Some(id) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let dir = entry.path();
        if !is_valid_id(&id) || !is_real_dir(&dir) {
            continue;
        }
        let report = RunReport::read(&dir).map_err(|e| format!("{e:#}"));
        let owned = read_owned_paths(&dir).unwrap_or_default();
        let run_json_mtime = fs::symlink_metadata(dir.join(RUN_JSON))
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| SystemTime::now());
        runs.push(RunInfo {
            id,
            dir,
            report,
            owned,
            run_json_mtime,
        });
    }
    runs.sort_by(|a, b| a.id.cmp(&b.id));
    runs
}

// ----- collection -----------------------------------------------------------

struct Collector<'a> {
    s: &'a Settings,
    now: SystemTime,
    procs: ProcScan,
    gh_cache: HashMap<(PathBuf, String), Result<Option<PrInfo>, String>>,
    common_cache: HashMap<PathBuf, Option<PathBuf>>,
    list_cache: HashMap<PathBuf, Result<Vec<PathBuf>, String>>,
    items: Vec<Item>,
}

impl Collector<'_> {
    fn wants(&self, category: Category) -> bool {
        self.s.categories.contains(&category)
    }

    fn past_deadline(&self) -> bool {
        self.s.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn common_dir(&mut self, dir: &Path) -> Option<PathBuf> {
        if let Some(hit) = self.common_cache.get(dir) {
            return hit.clone();
        }
        let value = git_common_dir(dir);
        self.common_cache.insert(dir.to_path_buf(), value.clone());
        value
    }

    fn worktrees_of(&mut self, main: &Path) -> Result<Vec<PathBuf>, String> {
        if let Some(hit) = self.list_cache.get(main) {
            return hit.clone();
        }
        let value = worktree_list(main).map_err(|e| format!("{e:#}"));
        self.list_cache.insert(main.to_path_buf(), value.clone());
        value
    }

    fn pr_for(&mut self, branch: &str, repo_dir: &Path) -> Result<Option<PrInfo>, String> {
        // Keyed by repository as well: two checkouts can share a branch name.
        let key = (repo_dir.to_path_buf(), branch.to_string());
        if let Some(hit) = self.gh_cache.get(&key) {
            return hit.clone();
        }
        let value = lookup_pr(branch, repo_dir).map_err(|e| format!("{e:#}"));
        self.gh_cache.insert(key, value.clone());
        value
    }

    /// The run-level verdict for an inactive run.
    fn run_verdict(&mut self, run: &RunInfo, report: &RunReport) -> Verdict {
        let branch = run
            .owned
            .iter()
            .filter(|o| o.kind == OwnedKind::Worktree)
            .find_map(|o| o.branch.clone());
        let Some(branch) = branch else {
            return self.no_pr(report, run);
        };
        if !is_valid_branch(&branch) {
            return Verdict::Skip {
                reason: format!("recorded branch {branch:?} is not a valid branch name"),
            };
        }
        let repo_dir = run
            .owned
            .iter()
            .filter(|o| o.kind == OwnedKind::Worktree && is_real_dir(&o.path))
            .map(|o| o.path.clone())
            .next()
            .or_else(|| Some(PathBuf::from(&report.cwd)).filter(|p| is_real_dir(p)));
        let Some(repo_dir) = repo_dir else {
            return Verdict::Skip {
                reason: "no repository folder left to look up the PR in".to_string(),
            };
        };
        match self.pr_for(&branch, &repo_dir) {
            Ok(Some(pr)) => pr_verdict(&pr, Utc::now(), self.s.keep),
            Ok(None) => self.no_pr(report, run),
            Err(error) => Verdict::Skip {
                reason: format!("gh failed: {error}"),
            },
        }
    }

    fn no_pr(&self, report: &RunReport, run: &RunInfo) -> Verdict {
        let ended = report
            .ended_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        no_pr_verdict(ended, run.run_json_mtime, self.now, self.s.keep)
    }

    fn collect_runs(&mut self, runs: &[RunInfo]) {
        let want_tmp = self.wants(Category::RunTmp);
        let want_wt = self.wants(Category::RunWorktree);
        let want_tgt = self.wants(Category::RunTarget);
        if !(want_tmp || want_wt || want_tgt) {
            return;
        }
        let root = self.s.work_root.clone();
        let worktrees_root = canonical_or_self(&root.worktrees_dir());
        let targets_root = canonical_or_self(&root.targets_dir());
        let hook_base = self.s.hook_cache_base.as_deref().map(canonical_or_self);

        for run in runs {
            if self.past_deadline() {
                break;
            }
            let id = run.id.as_str();
            let runtime_dirs = [
                run.dir.join("runtime"),
                root.short_runtime_dir().join(short_id(id)),
            ];
            let existing: Vec<&OwnedPath> = {
                let mut seen = BTreeSet::new();
                run.owned
                    .iter()
                    .filter(|o| fs::symlink_metadata(&o.path).is_ok())
                    .filter(|o| seen.insert(o.path.clone()))
                    .filter(|o| match o.kind {
                        OwnedKind::Worktree => want_wt,
                        OwnedKind::Target => want_tgt,
                    })
                    .collect()
            };
            let category_of = |o: &OwnedPath| match o.kind {
                OwnedKind::Worktree => Category::RunWorktree,
                OwnedKind::Target => Category::RunTarget,
            };

            let report = match &run.report {
                Ok(report) if report.schema_version > RUN_SCHEMA_VERSION => {
                    let reason = format!(
                        "run.json schema_version {} is newer than this release understands",
                        report.schema_version
                    );
                    for o in &existing {
                        self.items
                            .push(Item::skip(category_of(o), &o.path, &reason, Some(id)));
                    }
                    continue;
                }
                Ok(report) => report.clone(),
                Err(error) => {
                    let reason = format!("run.json cannot be read: {error}");
                    for o in &existing {
                        self.items
                            .push(Item::skip(category_of(o), &o.path, &reason, Some(id)));
                    }
                    continue;
                }
            };
            let active = run_is_active(&report);

            if want_tmp {
                for dir in runtime_dirs.iter().filter(|d| is_real_dir(d)) {
                    if active {
                        self.items.push(Item::skip(
                            Category::RunTmp,
                            dir,
                            "run is active",
                            Some(id),
                        ));
                    } else {
                        let reason = if report.ended_at.is_some() {
                            "run has ended"
                        } else {
                            "run is no longer active (interrupted)"
                        };
                        self.items.push(Item {
                            category: Category::RunTmp,
                            path: dir.clone(),
                            size_bytes: tree_size(dir),
                            reason: reason.to_string(),
                            run_id: Some(id.to_string()),
                            op: Op::RemoveRunTmp {
                                run_dir: run.dir.clone(),
                            },
                        });
                    }
                }
            }
            if existing.is_empty() {
                continue;
            }

            let run_main = {
                let cwd = PathBuf::from(&report.cwd);
                if report.cwd.is_empty() || !is_real_dir(&cwd) {
                    None
                } else {
                    self.common_dir(&cwd)
                }
            };
            let mut verdict: Option<Verdict> = active.then_some(Verdict::Active);
            for o in existing {
                let category = category_of(o);
                let meta = match fs::symlink_metadata(&o.path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if meta.file_type().is_symlink() {
                    self.items.push(Item::skip(
                        category,
                        &o.path,
                        "symbolic link; not followed",
                        Some(id),
                    ));
                    continue;
                }
                if !meta.file_type().is_dir() {
                    self.items
                        .push(Item::skip(category, &o.path, "not a folder", Some(id)));
                    continue;
                }
                let canonical = canonical_or_self(&o.path);
                let contained = match o.kind {
                    OwnedKind::Target => {
                        let parent = canonical.parent().map(Path::to_path_buf);
                        parent.as_deref() == Some(targets_root.as_path())
                            || (parent.as_deref() == hook_base.as_deref()
                                && canonical
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .is_some_and(|n| n.starts_with(HOOK_TARGET_PREFIX)))
                    }
                    OwnedKind::Worktree => {
                        let under_work = canonical
                            .strip_prefix(&worktrees_root)
                            .is_ok_and(|rel| rel.components().count() >= 2);
                        let under_main =
                            run_main
                                .as_deref()
                                .and_then(main_checkout)
                                .is_some_and(|main| {
                                    canonical
                                        .strip_prefix(main.join("worktrees"))
                                        .is_ok_and(|rel| rel.components().count() >= 1)
                                });
                        under_work || under_main
                    }
                };
                if !contained {
                    self.items.push(Item::skip(
                        category,
                        &o.path,
                        "outside the folders cleanup manages",
                        Some(id),
                    ));
                    continue;
                }
                let v = match &verdict {
                    Some(v) => v.clone(),
                    None => {
                        let v = self.run_verdict(run, &report);
                        verdict = Some(v.clone());
                        v
                    }
                };
                let reason = match v {
                    Verdict::Eligible { reason } => reason,
                    Verdict::Skip { reason } => {
                        self.items
                            .push(Item::skip(category, &o.path, reason, Some(id)));
                        continue;
                    }
                    Verdict::Active => {
                        self.items
                            .push(Item::skip(category, &o.path, "run is active", Some(id)));
                        continue;
                    }
                };
                let item = match o.kind {
                    OwnedKind::Target => {
                        if cargo_lock_held(&o.path) {
                            Item::skip(category, &o.path, "in use: .cargo-lock is held", Some(id))
                        } else {
                            Item {
                                category,
                                path: o.path.clone(),
                                size_bytes: tree_size(&o.path),
                                reason,
                                run_id: Some(id.to_string()),
                                op: Op::RemoveDir {
                                    cargo_lock: true,
                                    check_in_use: true,
                                },
                            }
                        }
                    }
                    OwnedKind::Worktree => {
                        self.worktree_item(o, &canonical, run_main.as_deref(), reason, id)
                    }
                };
                self.items.push(item);
            }
        }
    }

    fn worktree_item(
        &mut self,
        o: &OwnedPath,
        canonical: &Path,
        run_common: Option<&Path>,
        reason: String,
        id: &str,
    ) -> Item {
        let category = Category::RunWorktree;
        let Some(own_common) = self.common_dir(&o.path) else {
            return Item::skip(category, &o.path, "not a git worktree", Some(id));
        };
        if let Some(run_common) = run_common
            && run_common != own_common
        {
            return Item::skip(
                category,
                &o.path,
                "belongs to a different repository than the run",
                Some(id),
            );
        }
        let Some(main) = main_checkout(&own_common) else {
            return Item::skip(
                category,
                &o.path,
                "repository has no main checkout",
                Some(id),
            );
        };
        match self.worktrees_of(&main) {
            Ok(list) => {
                if list.first().map(PathBuf::as_path) == Some(canonical) {
                    return Item::skip(
                        category,
                        &o.path,
                        "the main checkout is never removed",
                        Some(id),
                    );
                }
                if !list.iter().skip(1).any(|p| p == canonical) {
                    return Item::skip(category, &o.path, "not a linked worktree", Some(id));
                }
            }
            Err(error) => return Item::skip(category, &o.path, error, Some(id)),
        }
        if git_has_dirty_state(&o.path) {
            return Item::skip(category, &o.path, "has uncommitted changes", Some(id));
        }
        if git_has_unpushed_commits(&o.path) {
            return Item::skip(category, &o.path, "has unpushed commits", Some(id));
        }
        if self.procs.in_use(&o.path) {
            return Item::skip(category, &o.path, "in use by a running process", Some(id));
        }
        Item {
            category,
            path: o.path.clone(),
            size_bytes: tree_size(&o.path),
            reason,
            run_id: Some(id.to_string()),
            op: Op::RemoveWorktree { main },
        }
    }

    fn collect_unrecorded_worktrees(&mut self, runs: &[RunInfo]) {
        if !self.s.report_unrecorded || !self.wants(Category::RunWorktree) {
            return;
        }
        let recorded: BTreeSet<PathBuf> = runs
            .iter()
            .flat_map(|r| r.owned.iter())
            .filter(|o| o.kind == OwnedKind::Worktree)
            .flat_map(|o| [o.path.clone(), canonical_or_self(&o.path)])
            .collect();
        let mut mains = BTreeSet::new();
        for run in runs {
            if let Ok(report) = &run.report {
                let cwd = PathBuf::from(&report.cwd);
                if !report.cwd.is_empty()
                    && is_real_dir(&cwd)
                    && let Some(common) = self.common_dir(&cwd)
                    && let Some(main) = main_checkout(&common)
                {
                    mains.insert(main);
                }
            }
        }
        let worktrees_root = canonical_or_self(&self.s.work_root.worktrees_dir());
        for main in mains {
            let Ok(list) = self.worktrees_of(&main) else {
                continue;
            };
            let main_base = main.join("worktrees");
            for wt in list.iter().skip(1) {
                let managed = wt.starts_with(&worktrees_root) || wt.starts_with(&main_base);
                if managed && !recorded.contains(wt) && is_real_dir(wt) {
                    self.items.push(Item::skip(
                        Category::RunWorktree,
                        wt,
                        "not recorded in owned-paths",
                        None,
                    ));
                }
            }
        }
    }

    fn collect_untagged_targets(&mut self, runs: &[RunInfo]) {
        if !self.wants(Category::UntaggedTarget) {
            return;
        }
        let recorded: BTreeSet<PathBuf> = runs
            .iter()
            .flat_map(|r| r.owned.iter())
            .filter(|o| o.kind == OwnedKind::Target)
            .flat_map(|o| [o.path.clone(), canonical_or_self(&o.path)])
            .collect();
        let mut candidates = Vec::new();
        let targets = self.s.work_root.targets_dir();
        if is_real_dir(&targets)
            && let Ok(rd) = fs::read_dir(&targets)
        {
            candidates.extend(rd.flatten().map(|e| e.path()));
        }
        if let Some(base) = &self.s.hook_cache_base
            && is_real_dir(base)
            && canonical_or_self(base) != canonical_or_self(&targets)
            && let Ok(rd) = fs::read_dir(base)
        {
            candidates.extend(rd.flatten().map(|e| e.path()).filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(HOOK_TARGET_PREFIX))
            }));
        }
        candidates.sort();
        for path in candidates {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_dir() || meta.uid() != euid() {
                continue;
            }
            if recorded.contains(&path) || recorded.contains(&canonical_or_self(&path)) {
                continue;
            }
            let Some(age) = idle_for(&path, self.now, self.s.keep) else {
                continue;
            };
            if cargo_lock_held(&path) {
                self.items.push(Item::skip(
                    Category::UntaggedTarget,
                    &path,
                    "in use: .cargo-lock is held",
                    None,
                ));
                continue;
            }
            self.items.push(Item {
                category: Category::UntaggedTarget,
                size_bytes: tree_size(&path),
                reason: format!(
                    "not recorded by any run; untouched for {} hours{}",
                    age.as_secs() / 3600,
                    if super::targets::is_tagged(&path) {
                        ""
                    } else {
                        "; no CACHEDIR.TAG"
                    }
                ),
                path,
                run_id: None,
                op: Op::RemoveDir {
                    cargo_lock: true,
                    check_in_use: true,
                },
            });
        }
    }

    fn collect_tmp_leftovers(&mut self) {
        if !self.wants(Category::TmpLeftover) || euid() == 0 {
            return;
        }
        let tmp = std::env::temp_dir();
        let Ok(tmp_meta) = fs::metadata(&tmp) else {
            return;
        };
        if tmp_meta.uid() != euid() && tmp_meta.mode() & 0o1000 == 0 {
            return;
        }
        let Ok(rd) = fs::read_dir(&tmp) else { return };
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| TMP_PREFIXES.iter().any(|pre| n.starts_with(pre)))
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_dir() || meta.uid() != euid() {
                continue;
            }
            let Some(age) = idle_for(&path, self.now, self.s.keep) else {
                continue;
            };
            if self.procs.in_use(&path) {
                self.items.push(Item::skip(
                    Category::TmpLeftover,
                    &path,
                    "in use by a running process",
                    None,
                ));
                continue;
            }
            self.items.push(Item {
                category: Category::TmpLeftover,
                size_bytes: tree_size(&path),
                reason: format!("untouched for {} hours", age.as_secs() / 3600),
                path,
                run_id: None,
                op: Op::RemoveDir {
                    cargo_lock: false,
                    check_in_use: true,
                },
            });
        }
    }

    fn collect_logs(&mut self) {
        let want_recipe = self.wants(Category::RecipeLog);
        let want_session = self.wants(Category::SessionLog);
        let mut sources: Vec<(Category, PathBuf, &str)> = Vec::new();
        if want_recipe && let Some(dir) = &self.s.recipe_log_dir {
            sources.push((Category::RecipeLog, dir.clone(), "recipe-logs"));
        }
        if want_session && let Some(dir) = &self.s.codex_sessions_dir {
            sources.push((Category::SessionLog, dir.clone(), "codex-sessions"));
        }
        for (category, root, prefix) in sources {
            for (path, meta) in walk_files(&root) {
                if meta.uid() != euid() {
                    continue;
                }
                let Some(age) = meta
                    .modified()
                    .ok()
                    .and_then(|m| self.now.duration_since(m).ok())
                    .filter(|age| *age >= self.s.log_age)
                else {
                    continue;
                };
                let Ok(rel) = path.strip_prefix(&root) else {
                    continue;
                };
                self.items.push(Item {
                    category,
                    size_bytes: Some(meta.len()),
                    reason: format!("modified {} days ago", age.as_secs() / 86_400),
                    run_id: None,
                    op: Op::Archive {
                        relative: Path::new(prefix).join(rel),
                    },
                    path,
                });
            }
        }
        if want_session && let Some(dir) = self.s.claude_projects_dir.clone() {
            for (path, meta) in walk_files(&dir) {
                let old = meta
                    .modified()
                    .ok()
                    .and_then(|m| self.now.duration_since(m).ok())
                    .is_some_and(|age| age >= self.s.log_age);
                if old {
                    let mut item =
                        Item::skip(Category::SessionLog, &path, REASON_AWAITING_1766, None);
                    item.size_bytes = Some(meta.len());
                    self.items.push(item);
                }
            }
        }
    }
}

fn collect(settings: &Settings) -> Vec<Item> {
    let runs = scan_runs(&settings.work_root);
    let mut c = Collector {
        s: settings,
        now: SystemTime::now(),
        procs: ProcScan::take(),
        gh_cache: HashMap::new(),
        common_cache: HashMap::new(),
        list_cache: HashMap::new(),
        items: Vec::new(),
    };
    c.collect_runs(&runs);
    c.collect_unrecorded_worktrees(&runs);
    c.collect_untagged_targets(&runs);
    c.collect_tmp_leftovers();
    c.collect_logs();
    c.items
}

// ----- apply ----------------------------------------------------------------

fn remove_real_dir(path: &Path, expected: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("{} is gone", path.display()))?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        bail!("{} is no longer a real folder", path.display());
    }
    if canonical_or_self(path) != expected {
        bail!("{} now resolves somewhere else", path.display());
    }
    fs::remove_dir_all(path).with_context(|| format!("cannot remove {}", path.display()))
}

fn execute(item: &Item, settings: &Settings, procs: &ProcScan) -> Result<()> {
    let expected = canonical_or_self(&item.path);
    match &item.op {
        Op::Skip => Ok(()),
        Op::RemoveRunTmp { run_dir } => {
            if let Ok(report) = RunReport::read(run_dir)
                && run_is_active(&report)
            {
                bail!("the run became active");
            }
            remove_real_dir(&item.path, &expected)
        }
        Op::RemoveDir {
            cargo_lock,
            check_in_use,
        } => {
            if *cargo_lock && cargo_lock_held(&item.path) {
                bail!("in use: .cargo-lock is held");
            }
            if *check_in_use && procs.in_use(&item.path) {
                bail!("in use by a running process");
            }
            remove_real_dir(&item.path, &expected)
        }
        Op::RemoveWorktree { main } => {
            let meta = fs::symlink_metadata(&item.path)?;
            if meta.file_type().is_symlink() {
                bail!("{} became a symbolic link", item.path.display());
            }
            if git_has_dirty_state(&item.path) {
                bail!("has uncommitted changes");
            }
            if git_has_unpushed_commits(&item.path) {
                bail!("has unpushed commits");
            }
            if procs.in_use(&item.path) {
                bail!("in use by a running process");
            }
            let mut command = hardened_git();
            command
                .arg("-C")
                .arg(main)
                .args(["worktree", "remove", "--"])
                .arg(&item.path);
            let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
            if !output.status.success() {
                bail!(
                    "git worktree remove failed: {}",
                    sanitized_stderr_short(&output.stderr)
                );
            }
            Ok(())
        }
        Op::Archive { relative } => {
            archive_file(&item.path, &settings.archive_dir, relative).map(|_| ())
        }
    }
}

/// Turns collected items into report entries, acting on them when
/// `settings.apply` and the lock is held.
fn apply_items(items: Vec<Item>, settings: &Settings, locked: bool) -> Vec<Entry> {
    let procs = if settings.apply && locked {
        Some(ProcScan::take())
    } else {
        None
    };
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let archive = matches!(item.op, Op::Archive { .. });
        let (action, reason) = match (&item.op, settings.apply) {
            (Op::Skip, _) => (Action::Skipped, item.reason.clone()),
            (_, false) if archive => (Action::WouldArchive, item.reason.clone()),
            (_, false) => (Action::WouldRemove, item.reason.clone()),
            (_, true) if !locked => (Action::Skipped, REASON_LOCK_HELD.to_string()),
            (_, true) if settings.deadline.is_some_and(|d| Instant::now() >= d) => {
                (Action::Skipped, "sweep deadline reached".to_string())
            }
            (_, true) => match execute(&item, settings, procs.as_ref().expect("taken when locked"))
            {
                Ok(()) if archive => (Action::Archived, item.reason.clone()),
                Ok(()) => (Action::Removed, item.reason.clone()),
                Err(error) => {
                    tracing::warn!(path = %item.path.display(), error = %format!("{error:#}"), "cleanup action failed");
                    (Action::Failed, format!("{error:#}"))
                }
            },
        };
        entries.push(Entry {
            category: item.category,
            path: item.path,
            size_bytes: item.size_bytes,
            reason,
            action,
            run_id: item.run_id,
        });
    }
    entries.sort_by(|a, b| (a.category.as_str(), &a.path).cmp(&(b.category.as_str(), &b.path)));
    entries
}

// ----- cleanup record -------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct RecordItem {
    kind: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecordSweep {
    cleaned_at: String,
    removed: Vec<RecordItem>,
    skipped: Vec<RecordItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CleanupRecord {
    run_id: String,
    sweeps: Vec<RecordSweep>,
}

/// Appends a sweep to `cleanup-record.json` of every run that lost something,
/// and drops removed paths from its `owned-paths`.
fn write_records(entries: &[Entry], root: &WorkRoot) {
    let mut by_run: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in entries {
        if let Some(id) = e.run_id.as_deref() {
            by_run.entry(id).or_default().push(e);
        }
    }
    for (id, list) in by_run {
        if !list.iter().any(|e| e.action == Action::Removed) || !is_valid_id(id) {
            continue;
        }
        let run_dir = root.runs_dir().join(id);
        if !is_real_dir(&run_dir) {
            continue;
        }
        if let Err(error) = write_record(&run_dir, id, &list) {
            eprintln!(
                "amplihack cleanup: warning: could not write the cleanup record of run {id}: {error:#}"
            );
        }
        if let Err(error) = prune_owned_paths(&run_dir) {
            eprintln!(
                "amplihack cleanup: warning: could not update owned-paths of run {id}: {error:#}"
            );
        }
    }
}

fn write_record(run_dir: &Path, id: &str, list: &[&Entry]) -> Result<()> {
    let path = run_dir.join(CLEANUP_RECORD);
    let mut record = match read_capped_nofollow(&path, RECORD_MAX_BYTES) {
        Ok(bytes) => serde_json::from_slice::<CleanupRecord>(&bytes).unwrap_or(CleanupRecord {
            run_id: id.to_string(),
            sweeps: Vec::new(),
        }),
        Err(_) => CleanupRecord {
            run_id: id.to_string(),
            sweeps: Vec::new(),
        },
    };
    record.run_id = id.to_string();
    let item = |e: &Entry, with_reason: bool| RecordItem {
        kind: e.category.as_str().to_string(),
        path: e.path.display().to_string(),
        size_bytes: if with_reason { None } else { e.size_bytes },
        reason: with_reason.then(|| e.reason.clone()),
    };
    record.sweeps.push(RecordSweep {
        cleaned_at: now_rfc3339(),
        removed: list
            .iter()
            .filter(|e| e.action == Action::Removed)
            .map(|e| item(e, false))
            .collect(),
        skipped: list
            .iter()
            .filter(|e| matches!(e.action, Action::Skipped | Action::Failed))
            .map(|e| item(e, true))
            .collect(),
    });
    let mut bytes = serde_json::to_vec_pretty(&record)?;
    bytes.push(b'\n');
    write_atomic_0600(&path, &bytes)
}

fn prune_owned_paths(run_dir: &Path) -> Result<()> {
    let owned = read_owned_paths(run_dir)?;
    let mut out = Vec::new();
    for item in owned
        .iter()
        .filter(|o| fs::symlink_metadata(&o.path).is_ok())
    {
        out.extend(format_owned_line(item));
    }
    write_atomic_0600(&run_dir.join(super::run_dir::OWNED_PATHS), &out)
}

// ----- entry points ---------------------------------------------------------

/// Collects, applies (when `settings.apply` and `locked`) and records.
pub(crate) fn run_sweep(settings: &Settings, locked: bool) -> Vec<Entry> {
    let items = collect(settings);
    let entries = apply_items(items, settings, locked);
    if settings.apply && locked {
        write_records(&entries, &settings.work_root);
    }
    entries
}

/// The best-effort end-of-run sweep (D5). Never returns an error and never
/// waits for a held lock.
pub(crate) fn end_of_run(work_root: &WorkRoot) {
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| end_of_run_inner(work_root)));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!("amplihack: warning: end-of-run cleanup: {error:#}"),
        Err(_) => {
            eprintln!("amplihack: warning: end-of-run cleanup panicked; nothing else was affected")
        }
    }
}

fn end_of_run_inner(work_root: &WorkRoot) -> Result<()> {
    let keep = match super::keep_hours_from_env() {
        Ok(hours) => hours,
        Err(error) => {
            eprintln!("amplihack: warning: {error:#}; using the default keep period");
            super::DEFAULT_KEEP_HOURS
        }
    };
    let Some(_lock) = try_lock(&work_root.lock_path())? else {
        eprintln!("amplihack: end-of-run cleanup skipped: {REASON_LOCK_HELD}");
        return Ok(());
    };
    let settings = Settings {
        work_root: work_root.clone(),
        keep: Duration::from_secs(keep * 3600),
        log_age: Duration::MAX,
        archive_dir: work_root.path.join("archive"),
        categories: [
            Category::RunTmp,
            Category::RunWorktree,
            Category::RunTarget,
            Category::UntaggedTarget,
        ]
        .into_iter()
        .collect(),
        apply: true,
        report_unrecorded: false,
        deadline: Some(Instant::now() + END_OF_RUN_DEADLINE),
        recipe_log_dir: None,
        codex_sessions_dir: None,
        claude_projects_dir: None,
        hook_cache_base: super::hook_cache_base(),
    };
    let entries = run_sweep(&settings, true);
    for e in &entries {
        match e.action {
            Action::Removed => {
                tracing::info!(path = %e.path.display(), category = e.category.as_str(), "end-of-run cleanup removed")
            }
            Action::Failed => eprintln!(
                "amplihack: warning: end-of-run cleanup could not remove {}: {}",
                e.path.display(),
                e.reason
            ),
            _ => {}
        }
    }
    Ok(())
}
