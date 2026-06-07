use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::{
    HygieneCleanupArgs, MIN_CLEANUP_APPLY_OLDER_THAN_HOURS, MIN_CLEANUP_APPLY_OLDER_THAN_SECS,
};

#[derive(Clone, Debug, Serialize)]
struct CleanupItem {
    category: &'static str,
    classification: &'static str,
    path: PathBuf,
    age_seconds: Option<u64>,
    reason: String,
}

fn resolve_active_worktrees(
    config: &CleanupConfig,
    items: &mut Vec<CleanupItem>,
) -> Option<BTreeSet<PathBuf>> {
    if !config.worktrees && !config.cargo_targets {
        return Some(BTreeSet::new());
    }

    match active_worktrees(&config.repo) {
        Ok(paths) => Some(paths),
        Err(error) => {
            let reason = format!(
                "cannot determine active git worktrees for {}: {error}",
                config.repo.display()
            );
            if config.worktrees {
                items.push(skipped(
                    "worktrees",
                    config.repo.clone(),
                    "skipped_ambiguous",
                    None,
                    reason.clone(),
                ));
            }
            if config.cargo_targets {
                items.push(skipped(
                    "cargo-targets",
                    config.repo.clone(),
                    "skipped_ambiguous",
                    None,
                    reason,
                ));
            }
            None
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct CleanupSummary {
    candidates: usize,
    skipped: usize,
    deleted: usize,
    errors: usize,
}

#[derive(Debug, Serialize)]
struct CleanupReport {
    mode: &'static str,
    repo: PathBuf,
    older_than_seconds: Option<u64>,
    summary: CleanupSummary,
    items: Vec<CleanupItem>,
}

#[derive(Clone, Debug)]
struct CleanupConfig {
    worktrees: bool,
    cargo_targets: bool,
    sessions: bool,
    apply: bool,
    older_than: Option<Duration>,
    repo: PathBuf,
    repo_target: PathBuf,
    repo_target_canonical: Option<PathBuf>,
    format: String,
    include_skipped: bool,
}

#[derive(Debug)]
struct SessionGuards {
    protected_ids: BTreeSet<String>,
    runtime_locks: Option<PathBuf>,
}

pub fn run(args: HygieneCleanupArgs) -> Result<()> {
    let config = CleanupConfig::from_args(args)?;
    let (mut report, active_worktrees) = scan(&config);

    if config.apply {
        apply_deletions(&mut report, &config, &active_worktrees);
    }

    compute_summary(&mut report);
    print_report(&report, &config)?;

    if report.summary.errors > 0 {
        bail!(
            "hygiene cleanup encountered {} error(s)",
            report.summary.errors
        );
    }
    Ok(())
}

impl CleanupConfig {
    fn from_args(args: HygieneCleanupArgs) -> Result<Self> {
        let worktrees = args.all || args.worktrees;
        let cargo_targets = args.all || args.cargo_targets;
        let sessions = args.all || args.sessions;
        if !worktrees && !cargo_targets && !sessions {
            bail!(
                "hygiene cleanup requires at least one cleanup category: --worktrees, --cargo-targets, --sessions, or --all"
            );
        }

        let older_than = args
            .older_than
            .as_deref()
            .map(parse_duration)
            .transpose()
            .context("invalid --older-than guardrail")?;
        if args.apply && older_than.is_none() {
            bail!("hygiene cleanup --apply requires an --older-than guardrail");
        }
        if args.apply
            && older_than
                .map(|duration| duration.as_secs() < MIN_CLEANUP_APPLY_OLDER_THAN_SECS)
                .unwrap_or(false)
        {
            bail!(
                "hygiene cleanup --apply requires --older-than of at least {}h",
                MIN_CLEANUP_APPLY_OLDER_THAN_HOURS
            );
        }

        let repo_input = match args.repo {
            Some(path) => path,
            None => std::env::current_dir().context("resolve current directory for --repo")?,
        };
        let repo = canonicalize_existing_dir(&repo_input)
            .with_context(|| format!("canonicalize repository path {}", repo_input.display()))?;
        let repo_target = repo.join("target");
        let repo_target_canonical = canonicalize_existing_dir(&repo_target).ok();

        Ok(Self {
            worktrees,
            cargo_targets,
            sessions,
            apply: args.apply,
            older_than,
            repo_target,
            repo_target_canonical,
            repo,
            format: args.format,
            include_skipped: args.include_skipped,
        })
    }
}

fn deletion_guard(
    item: &CleanupItem,
    config: &CleanupConfig,
    active_worktrees: &BTreeSet<PathBuf>,
    session_guards: &SessionGuards,
    apply_time: SystemTime,
) -> Result<()> {
    if is_current_repo_protected_path(&item.path, config) {
        bail!("candidate is protected by current repository boundary");
    }
    let canonical = canonicalize_existing_dir(&item.path)
        .with_context(|| format!("canonicalize candidate {}", item.path.display()))?;
    if canonical != item.path {
        bail!(
            "candidate path changed during cleanup: {} -> {}",
            item.path.display(),
            canonical.display()
        );
    }
    if is_current_repo_protected_path(&canonical, config) {
        bail!("candidate is protected by current repository boundary");
    }
    if overlaps_active_worktree(&canonical, active_worktrees) {
        bail!("candidate overlaps an active git worktree");
    }
    let age = path_age_at(&canonical, apply_time);
    if is_recent(age, config.older_than) {
        bail!("candidate became newer than --older-than");
    }
    match item.category {
        "worktrees" if canonical.join(".git").exists() => {
            if git_has_dirty_state(&canonical) {
                bail!("worktree has dirty state");
            }
            if git_has_unpushed_commits(&canonical) {
                bail!("worktree has unpushed commits or no upstream");
            }
        }
        "sessions" if is_running_session(&canonical, session_guards) => {
            bail!("session became active");
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn parse_duration(value: &str) -> Result<Duration> {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        bail!("expected duration like 48h, 14d, or 8w");
    }
    let (digits, suffix) = trimmed.split_at(trimmed.len() - 1);
    let count: u64 = digits
        .parse()
        .with_context(|| format!("parse duration amount in {trimmed:?}"))?;
    if count == 0 {
        bail!("duration must be greater than zero");
    }
    let seconds = match suffix {
        "h" | "H" => count
            .checked_mul(60 * 60)
            .context("duration is too large")?,
        "d" | "D" => count
            .checked_mul(24 * 60 * 60)
            .context("duration is too large")?,
        "w" | "W" => count
            .checked_mul(7 * 24 * 60 * 60)
            .context("duration is too large")?,
        _ => bail!("unsupported duration suffix {suffix:?}; use h, d, or w"),
    };
    Ok(Duration::from_secs(seconds))
}

fn scan(config: &CleanupConfig) -> (CleanupReport, BTreeSet<PathBuf>) {
    let mut items = Vec::new();
    let scan_time = SystemTime::now();
    let active_worktrees = resolve_active_worktrees(config, &mut items);

    if let Some(active_worktrees) = active_worktrees.as_ref() {
        if config.worktrees {
            scan_worktrees(config, active_worktrees, scan_time, &mut items);
        }
        if config.cargo_targets {
            scan_cargo_targets(config, active_worktrees, scan_time, &mut items);
        }
    }
    if config.sessions {
        scan_sessions(config, scan_time, &mut items);
    }

    let active_worktrees = active_worktrees.unwrap_or_default();
    (
        CleanupReport {
            mode: if config.apply { "apply" } else { "dry-run" },
            repo: config.repo.clone(),
            older_than_seconds: config.older_than.map(|duration| duration.as_secs()),
            summary: CleanupSummary::default(),
            items,
        },
        active_worktrees,
    )
}

fn scan_worktrees(
    config: &CleanupConfig,
    active_worktrees: &BTreeSet<PathBuf>,
    scan_time: SystemTime,
    items: &mut Vec<CleanupItem>,
) {
    for root in worktree_roots(&config.repo) {
        for child in read_worktree_candidates(&root) {
            classify_path(
                "worktrees",
                child,
                config,
                active_worktrees,
                scan_time,
                items,
                true,
            );
        }
    }
}

fn scan_cargo_targets(
    config: &CleanupConfig,
    active_worktrees: &BTreeSet<PathBuf>,
    scan_time: SystemTime,
    items: &mut Vec<CleanupItem>,
) {
    let Some(parent) = config.repo.parent() else {
        return;
    };

    let mut targets = BTreeSet::new();
    for target in [
        config.repo_target.clone(),
        parent.join("target"),
        parent.join("debug").join("target"),
    ] {
        if target.is_dir() {
            targets.insert(target);
        }
    }

    for child in read_child_dirs(parent) {
        let target = child.join("target");
        if target.is_dir() {
            targets.insert(target);
        }
    }

    for target in targets {
        classify_path(
            "cargo-targets",
            target,
            config,
            active_worktrees,
            scan_time,
            items,
            false,
        );
    }
}

fn scan_sessions(config: &CleanupConfig, scan_time: SystemTime, items: &mut Vec<CleanupItem>) {
    let session_guards = SessionGuards::current();
    for root in session_roots() {
        for child in read_child_dirs(&root) {
            let canonical = match canonicalize_existing_dir(&child) {
                Ok(path) => path,
                Err(error) => {
                    items.push(skipped(
                        "sessions",
                        child,
                        "skipped_ambiguous",
                        None,
                        format!("cannot canonicalize session artifact: {error}"),
                    ));
                    continue;
                }
            };

            if is_running_session(&canonical, &session_guards) {
                items.push(skipped(
                    "sessions",
                    canonical,
                    "skipped_running_session",
                    None,
                    "running session marker or lock present",
                ));
                continue;
            }

            let age = path_age_at(&canonical, scan_time);
            if is_recent(age, config.older_than) {
                items.push(skipped(
                    "sessions",
                    canonical,
                    "skipped_recent",
                    age.map(|duration| duration.as_secs()),
                    "session artifact is newer than --older-than",
                ));
                continue;
            }

            items.push(candidate(
                "sessions",
                canonical,
                age.map(|duration| duration.as_secs()),
                "stale session artifact",
            ));
        }
    }
}

fn classify_path(
    category: &'static str,
    path: PathBuf,
    config: &CleanupConfig,
    active_worktrees: &BTreeSet<PathBuf>,
    scan_time: SystemTime,
    items: &mut Vec<CleanupItem>,
    check_git_state: bool,
) {
    if is_current_repo_protected_path(&path, config) {
        items.push(skipped(
            category,
            path,
            "skipped_current_repo",
            None,
            "current repository paths are protected",
        ));
        return;
    }

    let canonical = match canonicalize_existing_dir(&path) {
        Ok(path) => path,
        Err(error) => {
            items.push(skipped(
                category,
                path,
                "skipped_ambiguous",
                None,
                format!("cannot canonicalize path: {error}"),
            ));
            return;
        }
    };

    if is_current_repo_protected_path(&canonical, config) {
        items.push(skipped(
            category,
            canonical,
            "skipped_current_repo",
            None,
            "current repository paths are protected",
        ));
        return;
    }

    if overlaps_active_worktree(&canonical, active_worktrees) {
        items.push(skipped(
            category,
            canonical,
            "skipped_active_worktree",
            None,
            "active worktree paths and their parent directories are protected",
        ));
        return;
    }

    if check_git_state && canonical.join(".git").exists() {
        if git_has_dirty_state(&canonical) {
            items.push(skipped(
                category,
                canonical,
                "skipped_dirty",
                None,
                "worktree has uncommitted or untracked changes",
            ));
            return;
        }
        if git_has_unpushed_commits(&canonical) {
            items.push(skipped(
                category,
                canonical,
                "skipped_unpushed",
                None,
                "worktree has commits not reachable from upstream",
            ));
            return;
        }
    }

    let age = path_age_at(&canonical, scan_time);
    if is_recent(age, config.older_than) {
        items.push(skipped(
            category,
            canonical,
            "skipped_recent",
            age.map(|duration| duration.as_secs()),
            "path is newer than --older-than",
        ));
        return;
    }

    items.push(candidate(
        category,
        canonical,
        age.map(|duration| duration.as_secs()),
        "safe cleanup candidate",
    ));
}

fn apply_deletions(
    report: &mut CleanupReport,
    config: &CleanupConfig,
    active_worktrees: &BTreeSet<PathBuf>,
) {
    let session_guards = SessionGuards::current();
    let apply_time = SystemTime::now();
    for item in &mut report.items {
        if item.classification != "candidate" {
            continue;
        }
        if let Err(error) =
            deletion_guard(item, config, active_worktrees, &session_guards, apply_time)
        {
            item.classification = "error";
            item.reason = format!("candidate failed pre-delete safety check: {error}");
            continue;
        }
        match fs::remove_dir_all(&item.path) {
            Ok(()) => {
                item.classification = "deleted";
                item.reason = "deleted by --apply".to_string();
            }
            Err(error) => {
                item.classification = "error";
                item.reason = format!("failed to delete candidate: {error}");
            }
        }
    }
}

fn compute_summary(report: &mut CleanupReport) {
    report.summary = CleanupSummary::default();
    for item in &report.items {
        match item.classification {
            "candidate" => report.summary.candidates += 1,
            "deleted" => report.summary.deleted += 1,
            "error" => report.summary.errors += 1,
            _ => report.summary.skipped += 1,
        }
    }
}

fn print_report(report: &CleanupReport, config: &CleanupConfig) -> Result<()> {
    if config.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(report).context("serialize cleanup report")?
        );
        return Ok(());
    }

    println!("mode: {}", report.mode);
    println!("repo: {}", report.repo.display());
    println!();
    println!("category        action                  age_seconds  path");
    for item in &report.items {
        if item.classification.starts_with("skipped") && !config.include_skipped {
            continue;
        }
        let age = item
            .age_seconds
            .map(|age| age.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<15} {:<23} {:<12} {}",
            item.category,
            item.classification,
            age,
            item.path.display()
        );
        if item.classification == "error" || config.include_skipped {
            println!("  reason: {}", item.reason);
        }
    }
    println!(
        "summary: {} candidate, {} skipped, {} deleted, {} errors",
        report.summary.candidates,
        report.summary.skipped,
        report.summary.deleted,
        report.summary.errors
    );
    Ok(())
}

fn active_worktrees(repo: &Path) -> Result<BTreeSet<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("list")
        .arg("--porcelain")
        .output()
        .with_context(|| format!("run git worktree list for {}", repo.display()))?;
    if !output.status.success() {
        bail!(
            "git worktree list failed: {}",
            sanitized_stderr(&output.stderr)
        );
    }

    let paths: BTreeSet<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|path| canonicalize_existing_dir(Path::new(path)).ok())
        .collect();
    if paths.is_empty() {
        bail!("git worktree list returned no canonical worktree paths");
    }
    Ok(paths)
}

fn sanitized_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).replace(['\n', '\r'], " ");
    let sanitized = text
        .split_whitespace()
        .map(|part| {
            if part.starts_with("https://") && part.contains('@') {
                "https://REDACTED@"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if sanitized.is_empty() {
        "no stderr".to_string()
    } else {
        sanitized.chars().take(500).collect()
    }
}

fn git_has_dirty_state(path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("status")
        .arg("--porcelain")
        .output();
    match output {
        Ok(output) if output.status.success() => !output.stdout.is_empty(),
        _ => true,
    }
}

fn git_has_unpushed_commits(path: &Path) -> bool {
    let upstream = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("@{u}")
        .output();
    let Ok(upstream) = upstream else {
        return true;
    };
    if !upstream.status.success() {
        return true;
    }

    let count = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-list")
        .arg("--count")
        .arg("@{u}..HEAD")
        .output();
    match count {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim() != "0"
        }
        _ => true,
    }
}

fn overlaps_active_worktree(path: &Path, active_worktrees: &BTreeSet<PathBuf>) -> bool {
    path.ancestors()
        .any(|ancestor| active_worktrees.contains(ancestor))
        || active_worktrees
            .iter()
            .any(|active| active.starts_with(path))
}

fn session_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        home.join(".copilot").join("session-state"),
        home.join(".amplihack").join("session-state"),
        home.join(".claude").join("session-state"),
    ]
    .into_iter()
    .filter(|path| path.is_dir())
    .collect()
}

impl SessionGuards {
    fn current() -> Self {
        let runtime_locks = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".claude").join("runtime").join("locks"));
        Self {
            protected_ids: protected_session_ids(),
            runtime_locks,
        }
    }
}

fn is_running_session(path: &Path, guards: &SessionGuards) -> bool {
    if path.join("LOCK").exists() || path.join("lock").exists() || path.join(".lock").exists() {
        return true;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    if guards.protected_ids.contains(name) {
        return true;
    }
    if let Some(runtime_locks) = &guards.runtime_locks
        && runtime_locks.join(name).exists()
    {
        return true;
    }
    if has_live_pid_marker(path) {
        return true;
    }
    false
}

fn protected_session_ids() -> BTreeSet<String> {
    [
        "COPILOT_AGENT_SESSION_ID",
        "COPILOT_SESSION_ID",
        "AMPLIHACK_SESSION_ID",
        "CLAUDE_SESSION_ID",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .filter_map(|value| value.into_string().ok())
    .filter(|value| !value.trim().is_empty())
    .collect()
}

fn has_live_pid_marker(path: &Path) -> bool {
    ["pid", "PID", ".pid", "process.pid"]
        .into_iter()
        .map(|name| path.join(name))
        .any(|marker| marker_contains_live_pid(&marker))
}

fn marker_contains_live_pid(marker: &Path) -> bool {
    let Ok(text) = fs::read_to_string(marker) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    pid > 0 && Path::new("/proc").join(pid.to_string()).exists()
}

fn worktree_roots(repo: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.push(repo.join("worktrees"));
    if let Some(parent) = repo.parent() {
        roots.push(parent.join("worktrees"));
    }

    for ancestor in repo.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) == Some("worktrees") {
            roots.push(ancestor.to_path_buf());
            break;
        }
    }

    dedupe_existing_dirs(roots)
}

fn read_worktree_candidates(root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for child in read_child_dirs(root) {
        if child.join(".git").exists() {
            candidates.push(child);
            continue;
        }
        for grandchild in read_child_dirs(&child) {
            if grandchild.join(".git").exists() {
                candidates.push(grandchild);
            }
        }
    }
    candidates
}

fn dedupe_existing_dirs(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for path in paths {
        let Ok(canonical) = canonicalize_existing_dir(&path) else {
            continue;
        };
        if seen.insert(canonical.clone()) {
            result.push(canonical);
        }
    }
    result
}

fn read_child_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => Some(entry.path()),
            Ok(file_type) if file_type.is_symlink() && entry.path().is_dir() => Some(entry.path()),
            _ => None,
        })
        .collect()
}

fn canonicalize_existing_dir(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a directory",
        ))
    }
}

fn path_age_at(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    now.duration_since(modified).ok()
}

fn is_recent(age: Option<Duration>, older_than: Option<Duration>) -> bool {
    match (age, older_than) {
        (Some(age), Some(older_than)) => age < older_than,
        (None, Some(_)) => true,
        _ => false,
    }
}

fn candidate(
    category: &'static str,
    path: PathBuf,
    age_seconds: Option<u64>,
    reason: impl Into<String>,
) -> CleanupItem {
    CleanupItem {
        category,
        classification: "candidate",
        path,
        age_seconds,
        reason: reason.into(),
    }
}

fn skipped(
    category: &'static str,
    path: PathBuf,
    classification: &'static str,
    age_seconds: Option<u64>,
    reason: impl Into<String>,
) -> CleanupItem {
    CleanupItem {
        category,
        classification,
        path,
        age_seconds,
        reason: reason.into(),
    }
}

fn is_current_repo_protected_path(path: &Path, config: &CleanupConfig) -> bool {
    path == config.repo
        || path.starts_with(&config.repo_target)
        || config
            .repo_target_canonical
            .as_ref()
            .is_some_and(|target| path == target || path.starts_with(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_scanner_only_returns_git_worktree_leaves() {
        let temp = tempfile::tempdir().expect("tempdir");
        let grouped = temp.path().join("worktrees").join("feat").join("active");
        fs::create_dir_all(grouped.join(".git")).expect("create worktree marker");
        fs::create_dir_all(temp.path().join("worktrees").join("docs"))
            .expect("create grouping dir");

        let candidates = read_worktree_candidates(&temp.path().join("worktrees"));

        assert_eq!(candidates, vec![grouped]);
    }

    #[test]
    fn active_worktree_parent_directories_fail_closed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let parent = temp.path().join("worktrees").join("feat");
        let active = parent.join("current");
        fs::create_dir_all(active.join(".git")).expect("create active worktree");

        let config = CleanupConfig {
            worktrees: true,
            cargo_targets: false,
            sessions: false,
            apply: false,
            older_than: None,
            repo: active.clone(),
            repo_target: active.join("target"),
            repo_target_canonical: None,
            format: "text".to_string(),
            include_skipped: false,
        };
        let mut active_worktrees = BTreeSet::new();
        active_worktrees.insert(active);
        let mut items = Vec::new();

        classify_path(
            "worktrees",
            parent,
            &config,
            &active_worktrees,
            SystemTime::now(),
            &mut items,
            false,
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].classification, "skipped_active_worktree");
    }

    #[test]
    fn scanner_skips_cleanup_when_active_worktrees_cannot_be_discovered() {
        let temp = tempfile::tempdir().expect("tempdir");
        let stale = temp.path().join("worktrees").join("old");
        fs::create_dir_all(stale.join(".git")).expect("create stale-looking worktree");

        let config = CleanupConfig {
            worktrees: true,
            cargo_targets: true,
            sessions: false,
            apply: false,
            older_than: None,
            repo: temp.path().to_path_buf(),
            repo_target: temp.path().join("target"),
            repo_target_canonical: None,
            format: "text".to_string(),
            include_skipped: true,
        };

        let (report, _) = scan(&config);

        assert!(
            report
                .items
                .iter()
                .all(|item| item.classification != "candidate"),
            "cleanup must not produce deletion candidates when active worktree discovery fails"
        );
        assert!(
            report
                .items
                .iter()
                .any(|item| item.classification == "skipped_ambiguous"),
            "fail-closed discovery should be visible in the report"
        );
    }

    #[test]
    fn apply_requires_safe_age_guardrail() {
        let temp = tempfile::tempdir().expect("tempdir");
        let args = HygieneCleanupArgs {
            worktrees: true,
            cargo_targets: false,
            sessions: false,
            all: false,
            apply: true,
            older_than: Some("1h".to_string()),
            repo: Some(temp.path().to_path_buf()),
            format: "text".to_string(),
            include_skipped: false,
        };

        let error = CleanupConfig::from_args(args).expect_err("1h must be unsafe for --apply");

        assert!(
            error.to_string().contains("at least 48h"),
            "error must explain the minimum destructive age threshold: {error}"
        );
    }

    #[test]
    fn apply_accepts_minimum_safe_age_guardrail() {
        let temp = tempfile::tempdir().expect("tempdir");
        let args = HygieneCleanupArgs {
            worktrees: true,
            cargo_targets: false,
            sessions: false,
            all: false,
            apply: true,
            older_than: Some("48h".to_string()),
            repo: Some(temp.path().to_path_buf()),
            format: "text".to_string(),
            include_skipped: false,
        };

        let config = CleanupConfig::from_args(args).expect("48h is the minimum safe age");

        assert_eq!(
            config.older_than.map(|duration| duration.as_secs()),
            Some(MIN_CLEANUP_APPLY_OLDER_THAN_SECS)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_repo_target_is_skipped_before_canonicalization() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let outside_target = temp.path().join("outside-target");
        fs::create_dir_all(&repo).expect("create repo");
        fs::create_dir_all(&outside_target).expect("create outside target");
        std::os::unix::fs::symlink(&outside_target, repo.join("target"))
            .expect("create target symlink");
        let repo = repo.canonicalize().expect("canonical repo");
        let repo_target = repo.join("target");
        let config = CleanupConfig {
            worktrees: false,
            cargo_targets: true,
            sessions: false,
            apply: false,
            older_than: None,
            repo,
            repo_target: repo_target.clone(),
            repo_target_canonical: canonicalize_existing_dir(&repo_target).ok(),
            format: "text".to_string(),
            include_skipped: true,
        };
        let mut items = Vec::new();

        classify_path(
            "cargo-targets",
            repo_target.clone(),
            &config,
            &BTreeSet::new(),
            SystemTime::now(),
            &mut items,
            false,
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].classification, "skipped_current_repo");
        assert_eq!(items[0].path, repo_target);
    }

    #[cfg(unix)]
    #[test]
    fn deletion_guard_refuses_resolved_symlinked_repo_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let outside_target = temp.path().join("outside-target");
        fs::create_dir_all(&repo).expect("create repo");
        fs::create_dir_all(&outside_target).expect("create outside target");
        std::os::unix::fs::symlink(&outside_target, repo.join("target"))
            .expect("create target symlink");
        let repo = repo.canonicalize().expect("canonical repo");
        let repo_target = repo.join("target");
        let resolved_target = canonicalize_existing_dir(&repo_target).expect("canonical target");
        let config = CleanupConfig {
            worktrees: false,
            cargo_targets: true,
            sessions: false,
            apply: true,
            older_than: Some(Duration::from_secs(MIN_CLEANUP_APPLY_OLDER_THAN_SECS)),
            repo,
            repo_target,
            repo_target_canonical: Some(resolved_target.clone()),
            format: "text".to_string(),
            include_skipped: true,
        };
        let item = CleanupItem {
            category: "cargo-targets",
            classification: "candidate",
            path: resolved_target,
            age_seconds: None,
            reason: "pre-fix canonicalized candidate".to_string(),
        };

        let error = deletion_guard(
            &item,
            &config,
            &BTreeSet::new(),
            &SessionGuards {
                protected_ids: BTreeSet::new(),
                runtime_locks: None,
            },
            SystemTime::now(),
        )
        .expect_err("resolved repo/target symlink must remain protected");

        assert!(
            error.to_string().contains("current repository boundary"),
            "error must identify the protected boundary: {error}"
        );
    }
}
