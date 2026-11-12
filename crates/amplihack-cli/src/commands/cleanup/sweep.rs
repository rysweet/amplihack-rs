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
    CLEANUP_RECORD, OWNED_PATHS, OwnedKind, OwnedPath, RUN_JSON, RUN_SCHEMA_VERSION, RunReport,
    TargetOrigin, format_owned_line, now_rfc3339, read_capped_nofollow, read_owned_paths, short_id,
    write_atomic_0600,
};
use super::targets::{HOOK_TARGET_PREFIX, is_cargo_target, target_name};
use super::work_root::WorkRoot;
use crate::commands::hygiene::cleanup::{
    git_has_dirty_state, hardened_git, has_live_pid_marker, sanitized_stderr_short,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Size and age walks stop after this many entries.
const WALK_LIMIT: usize = 1_000_000;
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const TMP_PREFIXES: &[&str] = &["replay-", "release-", "amplihack-workflow-"];
pub(crate) const END_OF_RUN_DEADLINE: Duration = Duration::from_secs(120);
const RECORD_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The end-of-run sweep covers run-owned categories only (D5). Untagged
/// targets and the rest are left to `amplihack cleanup`.
pub(crate) const END_OF_RUN_CATEGORIES: [Category; 3] =
    [Category::RunTmp, Category::RunWorktree, Category::RunTarget];

pub(crate) const REASON_LOCK_HELD: &str = "cleanup lock held";
/// Claude Code sessions under `~/.claude/projects` belong to
/// `amplihack sessions archive` (#1746/#1766), which the login rotator runs
/// once a day. That command archives a whole session (its `.jsonl` and its
/// folder) as one restorable unit, refuses any session a live `claude`
/// process in any profile still names, and keeps the manifest that
/// `amplihack sessions restore` reads. Archiving the same files here, one by
/// one and by age alone, would split sessions, could take a file from a live
/// session, and would put them where `sessions restore` cannot find them, so
/// cleanup lists them and leaves them to that command.
pub(crate) const REASON_SESSIONS_ARCHIVE: &str = "archived by `amplihack sessions archive`";

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

/// What to do with an item. Every removal names the run folder whose
/// liveness is checked again at apply time, when the item belongs to a run.
#[derive(Debug, Clone)]
enum Op {
    Skip,
    /// Nothing to act on, but the sweep could not finish this part; reported
    /// as `failed` so the command does not claim success.
    Fail,
    RemoveRunTmp {
        run_dir: PathBuf,
        owned: Box<super::owned_run_dir::OwnedRunDir>,
    },
    RemoveWorktree {
        main: PathBuf,
        run_dir: PathBuf,
        /// This sweep also planned a rule (a) removal of the checkout's
        /// target as its own item, which reports it; rule (c) then leaves
        /// that target alone (issue #2039).
        rule_a_target: bool,
    },
    /// `tmp-leftover` and `untagged-target` folders, and the real folder of
    /// a legacy run target; never a link.
    RemoveDir {
        run_dir: Option<PathBuf>,
        cargo_lock: bool,
    },
    /// A run's pre-commit target, removed only through
    /// [`target_remove::remove`] (issue #2039).
    RemoveTarget {
        run_dir: PathBuf,
        shape: target_remove::Shape,
        /// Rule (a): remove only once this checkout is gone at apply time.
        after_checkout_gone: Option<PathBuf>,
    },
    Archive {
        relative: PathBuf,
    },
}

#[derive(Debug, Clone)]
struct Item {
    category: Category,
    /// The path as recorded or found; shown in reports.
    path: PathBuf,
    /// The path resolved at collection. Removal acts on this path only, and
    /// only while `path` still resolves to it.
    canonical: PathBuf,
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
            canonical: path.to_path_buf(),
            size_bytes: None,
            reason: reason.into(),
            run_id: run_id.map(String::from),
            op: Op::Skip,
        }
    }

    fn fail(category: Category, path: &Path, reason: impl Into<String>) -> Self {
        Self {
            op: Op::Fail,
            ..Self::skip(category, path, reason, None)
        }
    }
}

#[path = "execute.rs"]
mod execute;
#[path = "procs.rs"]
mod procs;
#[path = "target_remove.rs"]
mod target_remove;

use execute::{Refusal, execute, other_live_owner};
use procs::ProcScan;
use target_remove::{LinkRoots, Shape};

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

/// Checks a `runtime_dir` recorded in `run.json` (issue #2291, D9, S11)
/// before the sweep may treat it as the run's temporary folder. The first
/// failure, in the documented order, is the reason.
fn external_runtime_check(
    path: &Path,
    id: &str,
    work_root: &Path,
    euid: u32,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("not an absolute path".into());
    }
    let name = super::run_tmp_root::external_dir_name(id);
    if name.is_none() || path.file_name() != name.as_deref().map(std::ffi::OsStr::new) {
        return Err("wrong name".into());
    }
    let meta =
        fs::symlink_metadata(path).map_err(|error| format!("cannot be inspected: {error}"))?;
    if meta.file_type().is_symlink() {
        return Err("is a symbolic link".into());
    }
    if !meta.file_type().is_dir() {
        return Err("not a folder".into());
    }
    if meta.uid() != euid {
        return Err("owned by another user".into());
    }
    if canonical_or_self(path) != path {
        return Err("path is not canonical".into());
    }
    if path.starts_with(canonical_or_self(work_root)) || path.starts_with(work_root) {
        return Err("inside the work root".into());
    }
    Ok(())
}

/// Size and newest mtime of a folder tree, from one walk.
struct TreeStats {
    size: u64,
    newest: SystemTime,
}

/// Walks `path` once, not following symlinks. `None` past the limit or when
/// an entry cannot be read; callers treat that as "recent" and skip the path.
fn tree_stats(path: &Path) -> Option<TreeStats> {
    let mut stats = TreeStats {
        size: 0,
        newest: SystemTime::UNIX_EPOCH,
    };
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        seen += 1;
        if seen > WALK_LIMIT {
            return None;
        }
        let meta = fs::symlink_metadata(&p).ok()?;
        stats.size = stats.size.saturating_add(meta.len());
        if let Ok(m) = meta.modified()
            && m > stats.newest
        {
            stats.newest = m;
        }
        if meta.file_type().is_dir()
            && let Ok(rd) = fs::read_dir(&p)
        {
            stack.extend(rd.flatten().map(|e| e.path()));
        }
    }
    Some(stats)
}

/// Total bytes under `path`, not following symlinks. `None` past the limit.
fn tree_size(path: &Path) -> Option<u64> {
    tree_stats(path).map(|t| t.size)
}

/// How long `path` has been untouched, with its size, when that is at least
/// `keep`. A future time counts as recent.
fn idle_for(path: &Path, now: SystemTime, keep: Duration) -> Option<(Duration, u64)> {
    let stats = tree_stats(path)?;
    let age = now.duration_since(stats.newest).ok()?;
    (age >= keep).then_some((age, stats.size))
}

/// Why `dir` must be kept because of its `.cargo-lock` files, checked in
/// `dir` and one level below it: a build holds one (`in use: .cargo-lock is
/// held`), or a lock or folder could not be checked, with its own reason. A
/// folder that cannot be listed is refused: a lock below it is unseen. The
/// shared locks taken are released before this returns.
fn cargo_lock_refusal(dir: &Path) -> Option<String> {
    target_remove::lock_cargo_tree_with(dir, target_remove::LockMode::Shared)
        .err()
        .map(|refusal| refusal.to_string())
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
/// The regular files under `root`, plus one message per folder that could
/// not be listed and one if the walk stopped at [`WALK_LIMIT`].
fn walk_files(root: &Path) -> (Vec<(PathBuf, fs::Metadata)>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    if !is_real_dir(root) {
        return (out, problems);
    }
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(error) => {
                problems.push(format!("cannot list {}: {error}", dir.display()));
                continue;
            }
        };
        for entry in rd.flatten() {
            seen += 1;
            if seen > WALK_LIMIT {
                problems.push(format!(
                    "scan stopped after {WALK_LIMIT} entries; the rest was not checked"
                ));
                return (out, problems);
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
    (out, problems)
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

/// Why `worktree` must be kept for commits that may exist only locally, or
/// `None` when every commit is on a remote. With an upstream, commits ahead of
/// it count. When the upstream is gone (the branch was deleted after merge and
/// pruned), commits reachable from any remote-tracking ref count as pushed, so
/// a merged worktree is not kept for ever under a false "unpushed" reason.
fn unpushed_reason(worktree: &Path) -> Option<String> {
    let count = |range: &[&str]| -> Result<u64> {
        let mut command = hardened_git();
        command
            .arg("-C")
            .arg(worktree)
            .args(["rev-list", "--count"])
            .args(range);
        let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
        if !output.status.success() {
            bail!("{}", sanitized_stderr_short(&output.stderr));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().parse()?)
    };
    let mut upstream = hardened_git();
    upstream
        .arg("-C")
        .arg(worktree)
        .args(["rev-parse", "--verify", "--quiet", "@{upstream}"]);
    let has_upstream = crate::util::run_output_with_timeout(upstream, GIT_TIMEOUT)
        .is_ok_and(|output| output.status.success());
    let (result, label) = if has_upstream {
        (count(&["@{upstream}..HEAD"]), "has unpushed commits")
    } else {
        (
            count(&["HEAD", "--not", "--remotes"]),
            "has no upstream and commits on no remote branch",
        )
    };
    match result {
        Ok(0) => None,
        Ok(_) => Some(label.to_string()),
        Err(error) => Some(format!("cannot check for unpushed commits: {error:#}")),
    }
}

/// The branch checked out in `worktree`, or `None` for a detached HEAD.
fn checked_out_branch(worktree: &Path) -> Result<Option<String>> {
    let mut command = hardened_git();
    command
        .arg("-C")
        .arg(worktree)
        .args(["symbolic-ref", "--quiet", "HEAD"]);
    let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
    match output.status.code() {
        // Not `--short`: that prints `heads/<name>` when a tag shares the
        // name, and `gh pr list --head heads/<name>` then finds no PR.
        Some(0) => {
            let full = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(full.strip_prefix("refs/heads/").map(String::from))
        }
        // Exit 1 with --quiet: HEAD is not a symbolic ref (detached).
        Some(1) => Ok(None),
        _ => bail!(
            "git symbolic-ref failed: {}",
            sanitized_stderr_short(&output.stderr)
        ),
    }
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
    /// `Err` when `owned-paths` exists but cannot be read: the run's items
    /// are unknown, so nothing it may own is removed.
    owned: Result<Vec<OwnedPath>, String>,
    run_json_mtime: SystemTime,
}

impl RunInfo {
    fn owned(&self) -> &[OwnedPath] {
        self.owned.as_deref().unwrap_or(&[])
    }
}

/// The runs under `<work root>/runs/`. A missing folder means no runs; one
/// that exists but cannot be listed is an error, never an empty list.
fn scan_runs(root: &WorkRoot) -> Result<Vec<RunInfo>> {
    let runs_dir = root.runs_dir();
    if !is_real_dir(&runs_dir) {
        return Ok(Vec::new());
    }
    let rd =
        fs::read_dir(&runs_dir).with_context(|| format!("cannot list {}", runs_dir.display()))?;
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
        let owned = read_owned_paths(&dir).map_err(|e| e.to_string());
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
    Ok(runs)
}

// ----- collection -----------------------------------------------------------

/// Canonical folders under which cleanup may act (D9), worked out from the
/// settings. Collection and apply each build their own, so a root repointed
/// in between no longer covers what was collected.
struct ManagedRoots {
    worktrees: PathBuf,
    targets: PathBuf,
    runs: PathBuf,
    short_runtime: PathBuf,
    hook_base: Option<PathBuf>,
    tmp: PathBuf,
}

impl ManagedRoots {
    fn new(s: &Settings) -> Self {
        let root = &s.work_root;
        Self {
            worktrees: canonical_or_self(&root.worktrees_dir()),
            targets: canonical_or_self(&root.targets_dir()),
            runs: canonical_or_self(&root.runs_dir()),
            short_runtime: canonical_or_self(&root.short_runtime_dir()),
            hook_base: s.hook_cache_base.as_deref().map(canonical_or_self),
            tmp: canonical_or_self(&std::env::temp_dir()),
        }
    }
}

fn has_prefix(path: &Path, prefixes: &[&str]) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| prefixes.iter().any(|p| n.starts_with(p)))
}

/// True when the canonical `path` of an owned item lies where cleanup may
/// act: a target directly under `<work root>/targets/` or a hook cache
/// folder, a worktree under `<work root>/worktrees/<repo>/` or under
/// `<main>/worktrees/`, where `main` is the worktree's own main checkout.
fn is_contained(kind: OwnedKind, path: &Path, roots: &ManagedRoots, main: Option<&Path>) -> bool {
    match kind {
        OwnedKind::Target => {
            let parent = path.parent();
            parent == Some(roots.targets.as_path())
                || (parent.is_some()
                    && parent == roots.hook_base.as_deref()
                    && has_prefix(path, &[HOOK_TARGET_PREFIX]))
        }
        OwnedKind::Worktree => {
            let under_work = path
                .strip_prefix(&roots.worktrees)
                .is_ok_and(|rel| rel.components().count() >= 2);
            let under_main = main.is_some_and(|main| {
                path.strip_prefix(main.join("worktrees"))
                    .is_ok_and(|rel| rel.components().count() >= 1)
            });
            under_work || under_main
        }
    }
}

/// An owned path that passed the symlink, folder and containment checks.
struct Candidate<'a> {
    owned: &'a OwnedPath,
    /// For a linked target, its canonical destination; for a dangling link,
    /// where the link itself sits.
    canonical: PathBuf,
    /// For a worktree: its main checkout, from its own git common dir.
    main: Option<PathBuf>,
    /// For a target: what [`target_remove::inspect`] found.
    shape: Option<Shape>,
}

struct Collector<'a> {
    s: &'a Settings,
    roots: ManagedRoots,
    links: LinkRoots,
    now: SystemTime,
    /// Taken on first use: most categories never ask.
    procs: Option<ProcScan>,
    gh_cache: HashMap<(PathBuf, String), Result<Option<PrInfo>, String>>,
    common_cache: HashMap<PathBuf, Option<PathBuf>>,
    list_cache: HashMap<PathBuf, Result<Vec<PathBuf>, String>>,
    items: Vec<Item>,
}

impl Collector<'_> {
    fn wants(&self, category: Category) -> bool {
        self.s.categories.contains(&category)
    }

    fn in_use(&mut self, path: &Path) -> bool {
        self.procs.get_or_insert_with(ProcScan::take).in_use(path)
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

    /// The git common dir of the folder the run started in, if it still
    /// exists. Used only to list unrecorded worktrees, which are never removed.
    fn report_common_dir(&mut self, report: &RunReport) -> Option<PathBuf> {
        let cwd = PathBuf::from(&report.cwd);
        if report.cwd.is_empty() || !is_real_dir(&cwd) {
            return None;
        }
        self.common_dir(&cwd)
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

    /// The verdict for one contained, linked worktree of an inactive run,
    /// from its own branch. The PR is looked up in the worktree itself, never
    /// in a folder named only by `run.json`.
    fn worktree_verdict(
        &mut self,
        c: &Candidate<'_>,
        run: &RunInfo,
        report: &RunReport,
    ) -> Verdict {
        // No branch recorded: ask about the branch the worktree has checked
        // out, so an open PR on it still keeps the worktree. Only a detached
        // HEAD falls through to the no-PR rule.
        let checked_out;
        let branch = match c.owned.branch.as_deref() {
            Some(branch) => branch,
            None => match checked_out_branch(&c.canonical) {
                Ok(Some(branch)) => {
                    checked_out = branch;
                    checked_out.as_str()
                }
                Ok(None) => return self.no_pr(report, run),
                Err(error) => {
                    return Verdict::Skip {
                        reason: format!("cannot read the checked-out branch: {error:#}"),
                    };
                }
            },
        };
        if !is_valid_branch(branch) {
            return Verdict::Skip {
                reason: format!("recorded branch {branch:?} is not a valid branch name"),
            };
        }
        match self.pr_for(branch, &c.canonical) {
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

    /// Symlink, folder, containment and (for worktrees) linked-worktree
    /// checks. `Err` is the skip reason.
    fn check_owned<'o>(&mut self, o: &'o OwnedPath) -> Result<Candidate<'o>, String> {
        if o.kind == OwnedKind::Target {
            // A five-field entry names a target the hook may have found as a
            // dangling link; only the link is removed then. A legacy entry
            // never names one, and its real folder needs no Cargo shape, as
            // before #2039.
            let new_format = o.target.is_some();
            let shape = target_remove::inspect_entry(
                &o.path,
                &self.roots,
                &self.links,
                new_format,
                new_format,
            )
            .map_err(|r| r.to_string())?;
            let canonical = target_remove::resolved(&o.path, &shape).map_err(|r| r.to_string())?;
            return Ok(Candidate {
                owned: o,
                canonical,
                main: None,
                shape: Some(shape),
            });
        }
        let meta = fs::symlink_metadata(&o.path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err("symbolic link; not followed".into());
        }
        if !meta.file_type().is_dir() {
            return Err("not a folder".into());
        }
        let canonical = fs::canonicalize(&o.path).map_err(|e| format!("cannot resolve: {e}"))?;
        let outside = || "outside the folders cleanup manages".to_string();
        match o.kind {
            OwnedKind::Target => Err(outside()),
            OwnedKind::Worktree => {
                let common = self.common_dir(&canonical).ok_or("not a git worktree")?;
                let main = main_checkout(&common).ok_or("repository has no main checkout")?;
                if !is_contained(o.kind, &canonical, &self.roots, Some(&main)) {
                    return Err(outside());
                }
                let list = self.worktrees_of(&main)?;
                if list.first() == Some(&canonical) {
                    return Err("the main checkout is never removed".into());
                }
                if !list.iter().skip(1).any(|p| *p == canonical) {
                    return Err("not a linked worktree".into());
                }
                Ok(Candidate {
                    owned: o,
                    canonical,
                    main: Some(main),
                    shape: None,
                })
            }
        }
    }

    fn collect_runs(&mut self, runs: &[RunInfo]) {
        let want_tmp = self.wants(Category::RunTmp);
        let want_wt = self.wants(Category::RunWorktree);
        let want_tgt = self.wants(Category::RunTarget);
        if !(want_tmp || want_wt || want_tgt) {
            return;
        }
        let root = self.s.work_root.clone();

        for run in runs {
            if self.past_deadline() {
                break;
            }
            let id = run.id.as_str();
            if let Err(error) = &run.owned
                && (want_wt || want_tgt)
            {
                let category = if want_wt {
                    Category::RunWorktree
                } else {
                    Category::RunTarget
                };
                self.items.push(Item::skip(
                    category,
                    &run.dir.join(OWNED_PATHS),
                    format!("owned-paths unreadable: {error}"),
                    Some(id),
                ));
            }
            // Retain placement independently for each supported layout.
            let mut runtime_dirs = vec![
                (run.dir.join("runtime"), run.dir.clone()),
                (
                    root.short_runtime_dir().join(short_id(id)),
                    root.short_runtime_dir(),
                ),
            ];
            let existing: Vec<&OwnedPath> = {
                let mut seen = BTreeSet::new();
                run.owned()
                    .iter()
                    .filter(|o| fs::symlink_metadata(&o.path).is_ok())
                    .filter(|o| seen.insert(o.path.clone()))
                    .collect()
            };
            let category_of = |o: &OwnedPath| match o.kind {
                OwnedKind::Worktree => Category::RunWorktree,
                OwnedKind::Target => Category::RunTarget,
            };
            let wanted = |o: &OwnedPath| match o.kind {
                OwnedKind::Worktree => want_wt,
                OwnedKind::Target => want_tgt,
            };

            let report = match &run.report {
                Ok(report) if report.schema_version > RUN_SCHEMA_VERSION => Err(format!(
                    "run.json schema_version {} is newer than this release understands",
                    report.schema_version
                )),
                Ok(report) => Ok(report.clone()),
                Err(error) => Err(format!("run.json cannot be read: {error}")),
            };
            let report = match report {
                Ok(report) => report,
                Err(reason) => {
                    for o in existing.iter().filter(|o| wanted(o)) {
                        self.items
                            .push(Item::skip(category_of(o), &o.path, &reason, Some(id)));
                    }
                    continue;
                }
            };
            let active = run_is_active(&report);

            // Missing originals are considered only when verified creation
            // evidence can recover their nonce-bound interrupted claim.
            if want_tmp && let Some(recorded) = report.runtime_dir.as_deref().map(PathBuf::from) {
                let missing = matches!(fs::symlink_metadata(&recorded),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound);
                if missing {
                    if let Some(parent) = &report.runtime_root {
                        runtime_dirs.push((recorded, parent.clone()));
                    }
                } else {
                    match external_runtime_check(&recorded, id, &root.path, euid()) {
                        Ok(()) => {
                            if let Some(parent) = &report.runtime_root {
                                runtime_dirs.push((recorded, parent.clone()));
                            } else {
                                self.items.push(Item::skip(
                                    Category::RunTmp,
                                    &recorded,
                                    "no independent runtime root recorded",
                                    Some(id),
                                ));
                            }
                        }
                        Err(reason) => self.items.push(Item::skip(
                            Category::RunTmp,
                            &recorded,
                            format!("recorded temp folder refused: {reason}"),
                            Some(id),
                        )),
                    }
                }
            }

            if want_tmp {
                for (dir, expected_root) in &runtime_dirs {
                    let recovered = if is_real_dir(dir) {
                        None
                    } else if matches!(fs::symlink_metadata(dir),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound)
                    {
                        match super::owned_run_dir::OwnedRunDir::load(expected_root, dir, id) {
                            Ok(owned) => Some(owned),
                            Err(error) => {
                                tracing::debug!(path = ?dir, %error,
                                    "no verified runtime or interrupted claim to advertise");
                                continue;
                            }
                        }
                    } else {
                        continue;
                    };
                    if active {
                        self.items.push(Item::skip(
                            Category::RunTmp,
                            dir,
                            "run is active",
                            Some(id),
                        ));
                    } else if self.in_use(dir) {
                        self.items.push(Item::skip(
                            Category::RunTmp,
                            dir,
                            "in use by a running process",
                            Some(id),
                        ));
                    } else {
                        let reason = if report.ended_at.is_some() {
                            "run has ended"
                        } else {
                            "run is no longer active (interrupted)"
                        };
                        let authority = match recovered {
                            Some(owned) => Ok(owned),
                            None => super::owned_run_dir::OwnedRunDir::load(expected_root, dir, id),
                        };
                        let owned = match authority {
                            Ok(owned) => owned,
                            Err(error) => {
                                self.items.push(Item::skip(
                                    Category::RunTmp,
                                    dir,
                                    format!("ownership refused: {error:#}"),
                                    Some(id),
                                ));
                                continue;
                            }
                        };
                        let current = match owned.current_path() {
                            Ok(path) => path,
                            Err(error) => {
                                self.items.push(Item::skip(
                                    Category::RunTmp,
                                    dir,
                                    format!("ownership refused: {error:#}"),
                                    Some(id),
                                ));
                                continue;
                            }
                        };
                        if self.in_use(&current) {
                            self.items.push(Item::skip(
                                Category::RunTmp,
                                dir,
                                "in use by a running process",
                                Some(id),
                            ));
                            continue;
                        }
                        self.items.push(Item {
                            category: Category::RunTmp,
                            path: dir.clone(),
                            canonical: canonical_or_self(dir),
                            size_bytes: tree_size(dir),
                            reason: reason.to_string(),
                            run_id: Some(id.to_string()),
                            op: Op::RemoveRunTmp {
                                run_dir: run.dir.clone(),
                                owned: Box::new(owned),
                            },
                        });
                    }
                }
            }
            if existing.is_empty() {
                continue;
            }
            if active {
                for o in existing.iter().filter(|o| wanted(o)) {
                    self.items.push(Item::skip(
                        category_of(o),
                        &o.path,
                        "run is active",
                        Some(id),
                    ));
                }
                continue;
            }

            // Every existing path is checked, wanted or not: a worktree that
            // cannot be checked keeps the run's targets.
            let mut candidates = Vec::new();
            let mut unknown_worktree: Option<String> = None;
            for o in &existing {
                let checked = self.check_owned(o).and_then(|c| {
                    let other = other_live_owner(runs, &c.canonical, id)
                        .or_else(|| other_live_owner(runs, &o.path, id));
                    match other {
                        Some(reason) => Err(reason),
                        None => Ok(c),
                    }
                });
                match checked {
                    Ok(c) => candidates.push(c),
                    Err(reason) => {
                        if o.kind == OwnedKind::Worktree && unknown_worktree.is_none() {
                            unknown_worktree = Some(format!(
                                "worktree {} of this run is kept: {reason}",
                                o.path.display()
                            ));
                        }
                        if wanted(o) {
                            self.items
                                .push(Item::skip(category_of(o), &o.path, reason, Some(id)));
                        }
                    }
                }
            }

            // This run's worktree items start here; rule (a) marks them below.
            let first_item = self.items.len();
            // One verdict per worktree, from its own branch (D6).
            let mut verdicts: Vec<Verdict> = Vec::new();
            // Worktrees this sweep plans to remove, for rule (a) of #2039.
            let mut removed_worktrees: Vec<PathBuf> = Vec::new();
            for c in candidates
                .iter()
                .filter(|c| c.owned.kind == OwnedKind::Worktree)
            {
                let v = self.worktree_verdict(c, run, &report);
                verdicts.push(v.clone());
                if !want_wt {
                    continue;
                }
                let item = match v {
                    Verdict::Eligible { reason } => self.worktree_item(c, &run.dir, reason, id),
                    Verdict::Skip { reason } => {
                        Item::skip(Category::RunWorktree, &c.owned.path, reason, Some(id))
                    }
                };
                if matches!(item.op, Op::RemoveWorktree { .. }) {
                    removed_worktrees.push(c.canonical.clone());
                    removed_worktrees.push(c.owned.path.clone());
                }
                self.items.push(item);
            }
            if !want_tgt {
                continue;
            }

            // A run's targets go only when every worktree it still has may go.
            // With no worktree left, the no-PR rule dates them from the run.
            let target_verdict = if let Some(reason) = unknown_worktree {
                Verdict::Skip { reason }
            } else if let Some(Verdict::Skip { reason }) = verdicts
                .iter()
                .find(|v| !matches!(v, Verdict::Eligible { .. }))
            {
                Verdict::Skip {
                    reason: format!("a worktree of this run is kept: {reason}"),
                }
            } else if let Some(Verdict::Eligible { reason }) = verdicts.first() {
                Verdict::Eligible {
                    reason: reason.clone(),
                }
            } else {
                self.no_pr(&report, run)
            };
            for c in candidates
                .iter()
                .filter(|c| c.owned.kind == OwnedKind::Target)
            {
                let path = &c.owned.path;
                // Legacy lines keep the #1808 verdict; five-field lines go
                // only on an event (#2039, D4) and never by age.
                let verdict = match &c.owned.target {
                    None => match &target_verdict {
                        Verdict::Eligible { reason } => Ok((reason.clone(), None)),
                        Verdict::Skip { reason } => Err(reason.clone()),
                    },
                    Some(record) => new_target_verdict(
                        &record.checkout,
                        record.origin,
                        run,
                        want_wt,
                        &removed_worktrees,
                    ),
                };
                let item = match verdict {
                    Ok((reason, after_checkout_gone)) => {
                        target_item(c, &run.dir, reason, after_checkout_gone, id)
                    }
                    Err(reason) => Item::skip(Category::RunTarget, path, reason, Some(id)),
                };
                if let Op::RemoveTarget {
                    after_checkout_gone: Some(checkout),
                    ..
                } = &item.op
                {
                    mark_rule_a_target(&mut self.items[first_item..], checkout);
                }
                self.items.push(item);
            }
        }
    }

    fn worktree_item(
        &mut self,
        c: &Candidate<'_>,
        run_dir: &Path,
        reason: String,
        id: &str,
    ) -> Item {
        let category = Category::RunWorktree;
        let path = &c.owned.path;
        let Some(main) = c.main.clone() else {
            return Item::skip(category, path, "repository has no main checkout", Some(id));
        };
        if git_has_dirty_state(&c.canonical) {
            return Item::skip(category, path, "has uncommitted changes", Some(id));
        }
        if let Some(reason) = unpushed_reason(&c.canonical) {
            return Item::skip(category, path, reason, Some(id));
        }
        if self.in_use(&c.canonical) {
            return Item::skip(category, path, "in use by a running process", Some(id));
        }
        Item {
            category,
            path: path.clone(),
            canonical: c.canonical.clone(),
            size_bytes: tree_size(&c.canonical),
            reason,
            run_id: Some(id.to_string()),
            op: Op::RemoveWorktree {
                main,
                run_dir: run_dir.to_path_buf(),
                rule_a_target: false,
            },
        }
    }

    fn collect_unrecorded_worktrees(&mut self, runs: &[RunInfo]) {
        if !self.s.report_unrecorded || !self.wants(Category::RunWorktree) {
            return;
        }
        let recorded: BTreeSet<PathBuf> = runs
            .iter()
            .flat_map(|r| r.owned().iter())
            .filter(|o| o.kind == OwnedKind::Worktree)
            .flat_map(|o| [o.path.clone(), canonical_or_self(&o.path)])
            .collect();
        let mut mains = BTreeSet::new();
        for run in runs {
            if let Ok(report) = &run.report
                && let Some(common) = self.report_common_dir(report)
                && let Some(main) = main_checkout(&common)
            {
                mains.insert(main);
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
            .flat_map(|r| r.owned().iter())
            .filter(|o| o.kind == OwnedKind::Target)
            .flat_map(|o| [o.path.clone(), canonical_or_self(&o.path)])
            .collect();
        // A run whose owned-paths cannot be read may own any of these.
        let unreadable = runs
            .iter()
            .find(|r| r.owned.is_err())
            .map(|r| format!("owned-paths of run {} unreadable; it may own this", r.id));
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
            candidates.extend(
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| has_prefix(p, &[HOOK_TARGET_PREFIX])),
            );
        }
        candidates.sort();
        // A folder a managed link points at belongs to that link: removing
        // it alone would leave the link dangling. Links are not followed
        // here, so neither goes.
        let link_dests: BTreeSet<PathBuf> = candidates
            .iter()
            .filter(|p| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
            .filter_map(|p| fs::canonicalize(p).ok())
            .collect();
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
            if link_dests.contains(&canonical_or_self(&path)) {
                continue;
            }
            let Some((age, size)) = idle_for(&path, self.now, self.s.keep) else {
                continue;
            };
            if let Some(reason) = &unreadable {
                self.items
                    .push(Item::skip(Category::UntaggedTarget, &path, reason, None));
                continue;
            }
            if let Some(reason) = cargo_lock_refusal(&path) {
                self.items
                    .push(Item::skip(Category::UntaggedTarget, &path, reason, None));
                continue;
            }
            self.items.push(Item {
                category: Category::UntaggedTarget,
                canonical: canonical_or_self(&path),
                size_bytes: Some(size),
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
                    run_dir: None,
                    cargo_lock: true,
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
            let Some((age, size)) = idle_for(&path, self.now, self.s.keep) else {
                continue;
            };
            if has_live_pid_marker(&path) || self.in_use(&path) {
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
                canonical: canonical_or_self(&path),
                size_bytes: Some(size),
                reason: format!("untouched for {} hours", age.as_secs() / 3600),
                path,
                run_id: None,
                op: Op::RemoveDir {
                    run_dir: None,
                    cargo_lock: false,
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
            let (files, problems) = walk_files(&root);
            for problem in problems {
                self.items.push(Item::fail(category, &root, problem));
            }
            for (path, meta) in files {
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
                    canonical: path.clone(),
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
            let (files, problems) = walk_files(&dir);
            for problem in problems {
                self.items
                    .push(Item::fail(Category::SessionLog, &dir, problem));
            }
            for (path, meta) in files {
                let old = meta
                    .modified()
                    .ok()
                    .and_then(|m| self.now.duration_since(m).ok())
                    .is_some_and(|age| age >= self.s.log_age);
                if old {
                    let mut item =
                        Item::skip(Category::SessionLog, &path, REASON_SESSIONS_ARCHIVE, None);
                    item.size_bytes = Some(meta.len());
                    self.items.push(item);
                }
            }
        }
    }
}

/// The D4 table of issue #2039 for a five-field target line: the removal
/// reason and, for rule (a), the checkout that must be gone at apply time; or
/// the reason the target is kept.
///
/// This deliberately narrows issue #2039's first acceptance line ("after a
/// run ends, its target is gone") to "gone when the run's worktree is
/// removed": while a worktree is kept, such as an auto-drive PR worktree, its
/// target is kept too (`worktree kept`), because the next commit there would
/// otherwise rebuild it from nothing. See "When a target is removed" in
/// `docs/howto/clean-up-after-recipe-runs.md`.
///
/// `worktrees_selected` is false when the sweep does not include the
/// `run-worktree` category (`--only run-target`): no worktree is removed then,
/// so rule (a) cannot apply and the reason says why.
fn new_target_verdict(
    checkout: &Path,
    origin: TargetOrigin,
    run: &RunInfo,
    worktrees_selected: bool,
    removed_worktrees: &[PathBuf],
) -> Result<(String, Option<PathBuf>), String> {
    match fs::symlink_metadata(checkout) {
        // (b) The checkout is gone: an orphan, whatever its origin.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(("checkout gone".to_string(), None))
        }
        Err(error) => Err(format!("checkout state unknown: {error}")),
        Ok(_) => {
            if origin == TargetOrigin::Reused {
                return Err("reused target; checkout still exists".to_string());
            }
            let canonical = canonical_or_self(checkout);
            let same = |p: &Path| p == checkout || p == canonical;
            let owned = run.owned().iter().any(|o| {
                o.kind == OwnedKind::Worktree
                    && (same(&o.path) || same(&canonical_or_self(&o.path)))
            });
            if !owned {
                return Err("checkout not owned by this run".to_string());
            }
            // (a) The run's own worktree for this checkout goes in this sweep.
            if removed_worktrees.iter().any(|w| same(w)) {
                Ok((
                    "worktree removed by run cleanup".to_string(),
                    Some(checkout.to_path_buf()),
                ))
            } else if !worktrees_selected {
                Err("worktree kept: run-worktree not selected".to_string())
            } else {
                Err("worktree kept".to_string())
            }
        }
    }
}

/// Marks the worktree item for `checkout` among `items` as having a planned
/// rule (a) target removal, so rule (c) does not act on that target too.
fn mark_rule_a_target(items: &mut [Item], checkout: &Path) {
    let canonical = canonical_or_self(checkout);
    for item in items {
        if let Op::RemoveWorktree { rule_a_target, .. } = &mut item.op
            && (item.canonical == canonical || item.path == checkout)
        {
            *rule_a_target = true;
        }
    }
}

/// The folder a target shape occupies on disk: the folder itself, or a
/// link's destination. `None` for a dangling link.
fn target_dir<'a>(c: &'a Candidate<'_>) -> Option<&'a Path> {
    match c.shape.as_ref()? {
        Shape::Dir => Some(c.canonical.as_path()),
        Shape::Link { dest } => Some(dest.as_path()),
        Shape::DanglingLink => None,
    }
}

/// A run target eligible for removal, unless a build holds its lock now.
fn target_item(
    c: &Candidate<'_>,
    run_dir: &Path,
    reason: String,
    after_checkout_gone: Option<PathBuf>,
    id: &str,
) -> Item {
    let path = &c.owned.path;
    let Some(shape) = c.shape.clone() else {
        return Item::skip(Category::RunTarget, path, "not a folder", Some(id));
    };
    let dir = target_dir(c);
    if let Some(reason) = dir.and_then(cargo_lock_refusal) {
        return Item::skip(Category::RunTarget, path, reason, Some(id));
    }
    // A legacy real folder keeps its #1808 removal, which needs no Cargo
    // shape; it still holds the `.cargo-lock` locks while deleting.
    let op = if c.owned.target.is_none() && shape == Shape::Dir {
        Op::RemoveDir {
            run_dir: Some(run_dir.to_path_buf()),
            cargo_lock: true,
        }
    } else {
        Op::RemoveTarget {
            run_dir: run_dir.to_path_buf(),
            shape,
            after_checkout_gone,
        }
    };
    Item {
        category: Category::RunTarget,
        path: path.clone(),
        canonical: c.canonical.clone(),
        size_bytes: dir.and_then(tree_size),
        reason,
        run_id: Some(id.to_string()),
        op,
    }
}

fn collect(settings: &Settings) -> Result<Vec<Item>> {
    let runs = scan_runs(&settings.work_root)?;
    let roots = ManagedRoots::new(settings);
    let mut c = Collector {
        s: settings,
        links: LinkRoots::from_env(&roots),
        roots,
        now: SystemTime::now(),
        procs: None,
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
    Ok(c.items)
}

// ----- apply ----------------------------------------------------------------

/// Turns collected items into report entries, acting on them when
/// `settings.apply` and the lock is held.
fn apply_items(items: Vec<Item>, settings: &Settings, locked: bool) -> Vec<Entry> {
    // Taken just before the first action, so it is fresh and skipped when
    // nothing is acted on (the usual end-of-run case).
    let mut procs: Option<ProcScan> = None;
    let roots = ManagedRoots::new(settings);
    let links = LinkRoots::from_env(&roots);
    let mut entries = Vec::with_capacity(items.len());
    // Targets already reported by rule (c), so a later plan-time entry for
    // the same target is not reported a second time.
    let mut followed: HashSet<PathBuf> = HashSet::new();
    for item in items {
        if item.category == Category::RunTarget
            && matches!(item.op, Op::Skip)
            && followed.contains(&report_key(&item.path))
        {
            continue;
        }
        let mut follow_on = Vec::new();
        let archive = matches!(item.op, Op::Archive { .. });
        let (action, reason) = match (&item.op, settings.apply) {
            (Op::Skip, _) => (Action::Skipped, item.reason.clone()),
            (Op::Fail, _) => (Action::Failed, item.reason.clone()),
            (_, false) if archive => (Action::WouldArchive, item.reason.clone()),
            (_, false) => (Action::WouldRemove, item.reason.clone()),
            (_, true) if !locked => (Action::Skipped, REASON_LOCK_HELD.to_string()),
            (_, true) if settings.deadline.is_some_and(|d| Instant::now() >= d) => {
                (Action::Skipped, "sweep deadline reached".to_string())
            }
            (_, true) => match execute(
                &item,
                settings,
                &roots,
                &links,
                procs.get_or_insert_with(ProcScan::take),
                &mut follow_on,
            ) {
                Ok(()) if archive => (Action::Archived, item.reason.clone()),
                Ok(()) => (Action::Removed, item.reason.clone()),
                Err(error) if error.is::<Refusal>() => (Action::Skipped, format!("{error:#}")),
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
        // Rule (c) of #2039: the target of a worktree removed just now. Every
        // result is reported, a kept target with its reason; it never changes
        // the worktree's own result.
        for e in follow_on {
            if e.action != Action::Removed {
                tracing::warn!(
                    path = %e.path.display(),
                    action = e.action.as_str(),
                    reason = %e.reason,
                    "target of a removed worktree was not removed"
                );
            }
            followed.insert(report_key(&e.path));
            entries.push(e);
        }
    }
    // Sorted by category, then path; except that worktrees and run targets
    // keep the order they were processed in, so a worktree comes before the
    // target that went with it. The sort is stable.
    let key = |e: &Entry| -> (&'static str, Option<PathBuf>) {
        match e.category {
            Category::RunWorktree | Category::RunTarget => (Category::RunWorktree.as_str(), None),
            other => (other.as_str(), Some(e.path.clone())),
        }
    };
    entries.sort_by(|a, b| key(a).cmp(&key(b)));
    entries
}

/// `path` with its parent folder resolved, so a recorded target path and the
/// path rule (c) builds compare equal after the target is gone.
fn report_key(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical_or_self(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Removes, or with `settings.apply == false` reports, the pre-commit target
/// of the removed checkout `checkout` under each managed root (issue #2039,
/// rule c). Each entry has category `run-target`, reason `worktree removed`
/// and `run_id`. A target that is not there gives no entry.
fn worktree_target_entries(
    checkout: &Path,
    settings: &Settings,
    roots: &ManagedRoots,
    links: &LinkRoots,
    procs: &mut Option<ProcScan>,
    run_id: Option<&str>,
) -> Vec<Entry> {
    const REASON: &str = "worktree removed";
    let Some(name) = target_name(checkout) else {
        return Vec::new();
    };
    let mut paths = vec![settings.work_root.targets_dir().join(&name)];
    if let Some(base) = &settings.hook_cache_base {
        let path = base.join(&name);
        if canonical_or_self(base) != canonical_or_self(&settings.work_root.targets_dir()) {
            paths.push(path);
        }
    }
    // Listing the runs reads every run folder; most removed worktrees have
    // no target, so it happens only once a target is found.
    let mut runs: Option<Result<Vec<RunInfo>>> = None;
    let mut out = Vec::new();
    for path in paths {
        if fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let entry = |action: Action, reason: String, size: Option<u64>| Entry {
            category: Category::RunTarget,
            path: path.clone(),
            size_bytes: size,
            reason,
            action,
            run_id: run_id.map(String::from),
        };
        let checked = target_remove::inspect(&path, roots, links, true).and_then(|shape| {
            target_remove::resolved(&path, &shape).map(|resolved| (shape, resolved))
        });
        let (shape, resolved) = match checked {
            Ok(checked) => checked,
            Err(refusal) => {
                out.push(entry(Action::Skipped, refusal.to_string(), None));
                continue;
            }
        };
        let other = match runs.get_or_insert_with(|| scan_runs(&settings.work_root)) {
            Ok(runs) => {
                let own = run_id.unwrap_or("");
                other_live_owner(runs, &path, own)
                    .or_else(|| other_live_owner(runs, &resolved, own))
            }
            Err(error) => Some(format!("cannot list runs: {error:#}")),
        };
        if let Some(reason) = other {
            out.push(entry(Action::Skipped, reason, None));
            continue;
        }
        let dir = (!matches!(shape, Shape::DanglingLink)).then_some(resolved.as_path());
        // A held build lock is the more precise reason, so it is checked
        // before the process scan; `remove` takes the locks for real. It is
        // also checked before the size walk: a target in use is not walked.
        if let Some(reason) = dir.and_then(cargo_lock_refusal) {
            out.push(entry(Action::Skipped, reason, None));
            continue;
        }
        let size = dir.and_then(tree_size);
        if !settings.apply {
            out.push(entry(Action::WouldRemove, REASON.into(), size));
            continue;
        }
        if dir.is_some_and(|d| procs.get_or_insert_with(ProcScan::take).in_use(d)) {
            out.push(entry(
                Action::Skipped,
                "in use by a running process".into(),
                size,
            ));
            continue;
        }
        out.push(
            match target_remove::remove(&path, &shape, roots, links, true) {
                Ok(()) => {
                    tracing::info!(
                        path = %path.display(),
                        event = "c",
                        reason = REASON,
                        run_id = run_id.unwrap_or(""),
                        "removed pre-commit target"
                    );
                    entry(Action::Removed, REASON.into(), size)
                }
                Err(target_remove::RemoveError::Refused(refusal)) => {
                    entry(Action::Skipped, refusal.to_string(), size)
                }
                Err(target_remove::RemoveError::Io(error)) => {
                    tracing::warn!(path = %path.display(), %error, "cannot remove target");
                    entry(Action::Failed, format!("cannot remove: {error}"), size)
                }
            },
        );
    }
    out
}

/// `amplihack cleanup --worktree-target <checkout>`: the target of a
/// worktree that is already gone (issue #2039, D6). The caller has checked
/// that `checkout` is absolute and no longer exists. Does not take the work
/// root lock.
pub(crate) fn worktree_target_sweep(settings: &Settings, checkout: &Path) -> Vec<Entry> {
    let roots = ManagedRoots::new(settings);
    let links = LinkRoots::from_env(&roots);
    let mut procs = None;
    worktree_target_entries(checkout, settings, &roots, &links, &mut procs, None)
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

/// Appends one sweep to the run's `cleanup-record.json`. A missing record
/// starts empty; one that cannot be parsed is renamed aside with a warning so
/// its history is kept; any other read error is returned.
fn write_record(run_dir: &Path, id: &str, list: &[&Entry]) -> Result<()> {
    let path = run_dir.join(CLEANUP_RECORD);
    let empty = || CleanupRecord {
        run_id: id.to_string(),
        sweeps: Vec::new(),
    };
    let mut record = match read_capped_nofollow(&path, RECORD_MAX_BYTES) {
        Ok(bytes) => match serde_json::from_slice::<CleanupRecord>(&bytes) {
            Ok(record) => record,
            Err(error) => {
                let aside = run_dir.join(format!(
                    "{CLEANUP_RECORD}.corrupt-{}",
                    Utc::now().format("%Y%m%dT%H%M%S%.fZ")
                ));
                fs::rename(&path, &aside)
                    .with_context(|| format!("cannot move unreadable {} aside", path.display()))?;
                eprintln!(
                    "amplihack cleanup: warning: {} could not be parsed ({error}); kept as {} and a new record started",
                    path.display(),
                    aside.display()
                );
                empty()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => empty(),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
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

/// Drops paths that no longer exist from `owned-paths`, and removes the file
/// when nothing is left.
fn prune_owned_paths(run_dir: &Path) -> Result<()> {
    let owned = read_owned_paths(run_dir)?;
    let mut out = Vec::new();
    for item in owned
        .iter()
        .filter(|o| fs::symlink_metadata(&o.path).is_ok())
    {
        out.extend(format_owned_line(item));
    }
    let file = run_dir.join(OWNED_PATHS);
    if out.is_empty() {
        return match fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("cannot remove {}", file.display())),
        };
    }
    write_atomic_0600(&file, &out)
}

// ----- entry points ---------------------------------------------------------

/// Collects, applies (when `settings.apply` and `locked`) and records.
pub(crate) fn run_sweep(settings: &Settings, locked: bool) -> Result<Vec<Entry>> {
    let items = collect(settings)?;
    let entries = apply_items(items, settings, locked);
    if settings.apply && locked {
        write_records(&entries, &settings.work_root);
    }
    Ok(entries)
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
        categories: END_OF_RUN_CATEGORIES.into_iter().collect(),
        apply: true,
        report_unrecorded: false,
        deadline: Some(Instant::now() + END_OF_RUN_DEADLINE),
        recipe_log_dir: None,
        codex_sessions_dir: None,
        claude_projects_dir: None,
        hook_cache_base: super::hook_cache_base(),
    };
    let entries = run_sweep(&settings, true)?;
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

#[cfg(test)]
#[path = "tests_sweep.rs"]
mod tests;

#[cfg(test)]
#[path = "tests_owned_stale.rs"]
mod tests_owned_stale;
