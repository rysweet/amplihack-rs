//! Artifact Guard for generated/runtime repository pollution.
//!
//! The guard scans Git-visible paths plus targeted ignored-present artifact
//! roots. It reports violations only; it never deletes, unstages, or rewrites
//! repository files.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactGuardConfig {
    repo: PathBuf,
    mode: ArtifactGuardMode,
    allowlist: Option<PathBuf>,
}

impl ArtifactGuardConfig {
    pub fn new(repo: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
            mode: ArtifactGuardMode::All,
            allowlist: None,
        }
    }

    pub fn with_mode(mut self, mode: ArtifactGuardMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_allowlist(mut self, allowlist: PathBuf) -> Self {
        self.allowlist = Some(allowlist);
        self
    }

    pub fn mode(&self) -> ArtifactGuardMode {
        self.mode
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArtifactGuardMode {
    #[default]
    All,
    Staged,
    Worktree,
    PreCommit,
    PrePublish,
}

impl ArtifactGuardMode {
    pub fn parse(value: &str) -> Result<Self, ArtifactGuardError> {
        match value {
            "all" => Ok(Self::All),
            "staged" => Ok(Self::Staged),
            "worktree" => Ok(Self::Worktree),
            "pre-commit" => Ok(Self::PreCommit),
            "pre-publish" => Ok(Self::PrePublish),
            other => Err(ArtifactGuardError::InvalidMode(other.to_string())),
        }
    }

    fn scans_staged(self) -> bool {
        matches!(
            self,
            Self::All | Self::Staged | Self::PreCommit | Self::PrePublish
        )
    }

    fn scans_worktree(self) -> bool {
        matches!(
            self,
            Self::All | Self::Worktree | Self::PreCommit | Self::PrePublish
        )
    }

    /// Whether this mode should flag `IgnoredPresent` artifacts (gitignored +
    /// present-on-disk cache dirs such as `.pytest_cache/` or `node_modules/`).
    ///
    /// These paths can never be committed or published, so pre-commit and
    /// pre-publish (fail-closed publish gates) MUST NOT block on them
    /// (issue #928). Only the auditing modes flag them.
    fn scans_ignored_present(self) -> bool {
        matches!(self, Self::All | Self::Worktree)
    }
}

impl fmt::Display for ArtifactGuardMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::All => "all",
            Self::Staged => "staged",
            Self::Worktree => "worktree",
            Self::PreCommit => "pre-commit",
            Self::PrePublish => "pre-publish",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArtifactSource {
    Staged,
    Tracked,
    Untracked,
    IgnoredPresent,
}

impl fmt::Display for ArtifactSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Staged => "staged",
            Self::Tracked => "tracked",
            Self::Untracked => "untracked",
            Self::IgnoredPresent => "ignored-present",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRule {
    pub id: &'static str,
    pub description: &'static str,
    remediation: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactViolation {
    pub path: String,
    pub source: ArtifactSource,
    pub rule_id: String,
    pub remediation: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactGuardReport {
    pub repo: PathBuf,
    pub mode: ArtifactGuardMode,
    pub violations: Vec<ArtifactViolation>,
}

impl ArtifactGuardReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn has_violations(&self) -> bool {
        !self.is_clean()
    }
}

#[derive(Debug, Error)]
pub enum ArtifactGuardError {
    #[error("artifact guard repo must resolve inside a git worktree: {0}")]
    NotGitWorktree(String),
    #[error("artifact guard git command failed: git {args} (exit {code:?}): {stderr}")]
    Git {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("artifact guard invalid mode: {0}")]
    InvalidMode(String),
    #[error("artifact guard allowlist error: {0}")]
    Allowlist(String),
    #[error("artifact guard path escaped repository root: {0}")]
    PathEscape(String),
    #[error("artifact guard I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactAllowlist {
    entries: Vec<AllowlistEntry>,
}

impl ArtifactAllowlist {
    pub fn load(path: &Path) -> Result<Self, ArtifactGuardError> {
        let content = fs::read_to_string(path).map_err(|error| {
            ArtifactGuardError::Allowlist(format!("read allowlist {}: {error}", path.display()))
        })?;
        Self::parse(&content, path)
    }

    pub fn parse(content: &str, path: &Path) -> Result<Self, ArtifactGuardError> {
        let mut entries = Vec::new();
        for (index, raw) in content.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            validate_allowlist_entry(line, path, index + 1)?;
            entries.push(AllowlistEntry::new(line));
        }

        if entries.is_empty() {
            return Err(ArtifactGuardError::Allowlist(format!(
                "{} contains no reviewed entries; empty allowlists are unsafe",
                path.display()
            )));
        }

        entries.sort_by(|left, right| left.pattern.cmp(&right.pattern));
        entries.dedup_by(|left, right| left.pattern == right.pattern);
        Ok(Self { entries })
    }

    fn is_allowed(&self, path: &str) -> bool {
        self.entries.iter().any(|entry| entry.matches(path))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AllowlistEntry {
    pattern: String,
}

impl AllowlistEntry {
    fn new(pattern: &str) -> Self {
        Self {
            pattern: normalize_slashes(pattern),
        }
    }

    fn matches(&self, path: &str) -> bool {
        glob_matches(&self.pattern, path)
    }
}

const REMEDIATION: &str = "remove the artifact from the parent worktree, or move generated/runtime output into an ignored isolated directory outside the parent worktree; if intentional, add a narrow reviewed .amplihack-artifact-allowlist entry";

const DEFAULT_RULES: &[ArtifactRule] = &[
    ArtifactRule {
        id: "node-modules",
        description: "Node dependency tree",
        remediation: REMEDIATION,
    },
    ArtifactRule {
        id: "plugin-bundle",
        description: "Generated plugin bundle",
        remediation: REMEDIATION,
    },
    ArtifactRule {
        id: "claude-runtime",
        description: "Claude runtime state",
        remediation: REMEDIATION,
    },
    ArtifactRule {
        id: "nested-worktree",
        description: "Nested worktree under parent repository",
        remediation: REMEDIATION,
    },
    ArtifactRule {
        id: "cache-artifact",
        description: "Cache directory artifact",
        remediation: REMEDIATION,
    },
    ArtifactRule {
        id: "build-artifact",
        description: "Build output artifact",
        remediation: REMEDIATION,
    },
    ArtifactRule {
        id: "workflow-session-artifact",
        description: "Workflow/session runtime artifact",
        remediation: REMEDIATION,
    },
];

pub fn scan_artifacts(
    config: &ArtifactGuardConfig,
) -> Result<ArtifactGuardReport, ArtifactGuardError> {
    let repo = resolve_repo_root(&config.repo)?;
    let allowlist = load_effective_allowlist(config, &repo)?;
    let mut violations = Vec::new();
    let mut seen = BTreeSet::new();

    if config.mode.scans_staged() {
        scan_git_paths(
            &repo,
            &[
                "diff",
                "--cached",
                "--diff-filter=ACMRTUXB",
                "--name-only",
                "-z",
            ],
            ArtifactSource::Staged,
            &allowlist,
            &mut violations,
            &mut seen,
        )?;
    }

    if config.mode.scans_worktree() {
        scan_git_paths(
            &repo,
            &["ls-files", "-z"],
            ArtifactSource::Tracked,
            &allowlist,
            &mut violations,
            &mut seen,
        )?;
        scan_git_paths(
            &repo,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            ArtifactSource::Untracked,
            &allowlist,
            &mut violations,
            &mut seen,
        )?;
        if config.mode.scans_ignored_present() {
            scan_ignored_present(&repo, &allowlist, &mut violations, &mut seen)?;
        }
    }

    violations.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.source.cmp(&right.source))
            .then(left.rule_id.cmp(&right.rule_id))
    });

    Ok(ArtifactGuardReport {
        repo,
        mode: config.mode,
        violations,
    })
}

fn resolve_repo_root(repo: &Path) -> Result<PathBuf, ArtifactGuardError> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| ArtifactGuardError::NotGitWorktree(error.to_string()))?;
    if !output.status.success() {
        return Err(ArtifactGuardError::NotGitWorktree(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let root = PathBuf::from(stdout.trim());
    root.canonicalize()
        .map_err(|error| ArtifactGuardError::NotGitWorktree(error.to_string()))
}

fn load_effective_allowlist(
    config: &ArtifactGuardConfig,
    repo: &Path,
) -> Result<ArtifactAllowlist, ArtifactGuardError> {
    let Some(path) = config.allowlist.as_ref().cloned().or_else(|| {
        let default = repo.join(".amplihack-artifact-allowlist");
        default.exists().then_some(default)
    }) else {
        return Ok(ArtifactAllowlist::default());
    };

    let allowlist_path = resolve_allowlist_path(&path, repo)?;
    ArtifactAllowlist::load(&allowlist_path)
}

fn resolve_allowlist_path(path: &Path, repo: &Path) -> Result<PathBuf, ArtifactGuardError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    };
    let canonical = candidate.canonicalize().map_err(|error| {
        ArtifactGuardError::Allowlist(format!(
            "resolve allowlist {}: {error}",
            candidate.display()
        ))
    })?;
    if !canonical.starts_with(repo) {
        return Err(ArtifactGuardError::PathEscape(format!(
            "allowlist {} resolved outside repository root {}",
            canonical.display(),
            repo.display()
        )));
    }
    Ok(canonical)
}

fn scan_git_paths(
    repo: &Path,
    args: &[&str],
    source: ArtifactSource,
    allowlist: &ArtifactAllowlist,
    violations: &mut Vec<ArtifactViolation>,
    seen: &mut BTreeSet<(ArtifactSource, String)>,
) -> Result<(), ArtifactGuardError> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| ArtifactGuardError::Git {
            args: args.join(" "),
            code: None,
            stderr: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(ArtifactGuardError::Git {
            args: args.join(" "),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    for raw in output.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(raw);
        add_violation_if_prohibited(&path, source, allowlist, violations, seen);
    }
    Ok(())
}

fn scan_ignored_present(
    repo: &Path,
    allowlist: &ArtifactAllowlist,
    violations: &mut Vec<ArtifactViolation>,
    seen: &mut BTreeSet<(ArtifactSource, String)>,
) -> Result<(), ArtifactGuardError> {
    let args = [
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "-z",
    ];
    let output = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| ArtifactGuardError::Git {
            args: args.join(" "),
            code: None,
            stderr: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(ArtifactGuardError::Git {
            args: args.join(" "),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    // Files inside legitimately-registered sibling task worktrees (concurrent
    // recipe runs nested under `<repo>/worktrees/`) are NOT leaked pollution and
    // must not be flagged as `nested-worktree` violations (issue #857). Genuine
    // leaked directories under `worktrees/` (not registered git worktrees) are
    // still flagged. `git ls-files --others --ignored` recurses into these
    // gitignored nested worktrees, so we filter their paths out explicitly.
    let registered_nested = registered_nested_worktree_prefixes(repo);

    for raw in output.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(raw);
        if is_inside_registered_nested_worktree(&path, &registered_nested) {
            continue;
        }
        add_violation_if_prohibited(
            &path,
            ArtifactSource::IgnoredPresent,
            allowlist,
            violations,
            seen,
        );
    }
    Ok(())
}

/// Repo-relative path prefixes (each ending in `/`) of git worktrees that are
/// registered under `<repo>/worktrees/`. These are legitimate concurrent task
/// worktrees created by other recipe runs — not leaked scratch — so the artifact
/// guard must never flag their contents (issue #857). Best-effort: returns empty
/// on any git error so the guard fails safe (keeps scanning).
fn registered_nested_worktree_prefixes(repo: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let repo_canon = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let mut prefixes = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(raw) = line.strip_prefix("worktree ") else {
            continue;
        };
        let wt = Path::new(raw);
        let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf());
        let Ok(rel) = wt_canon.strip_prefix(&repo_canon) else {
            continue;
        };
        let rel = normalize_slashes(rel.to_string_lossy().trim_end_matches('/'));
        // Only worktrees nested under `<repo>/worktrees/` — never the repo root
        // itself (rel == "") nor unrelated paths.
        if rel.starts_with("worktrees/") {
            prefixes.push(format!("{rel}/"));
        }
    }
    prefixes
}

/// Whether `raw_path` (a repo-relative scan path) lies inside one of the
/// `registered` nested-worktree prefixes. See [`registered_nested_worktree_prefixes`].
fn is_inside_registered_nested_worktree(raw_path: &str, registered: &[String]) -> bool {
    if registered.is_empty() {
        return false;
    }
    let path = normalize_slashes(raw_path.trim_end_matches('/'));
    registered
        .iter()
        .any(|prefix| path == prefix.trim_end_matches('/') || path.starts_with(prefix.as_str()))
}

fn add_violation_if_prohibited(
    raw_path: &str,
    source: ArtifactSource,
    allowlist: &ArtifactAllowlist,
    violations: &mut Vec<ArtifactViolation>,
    seen: &mut BTreeSet<(ArtifactSource, String)>,
) {
    let path = normalize_slashes(raw_path.trim_end_matches('/'));
    if path.is_empty() || allowlist.is_allowed(&path) {
        return;
    }
    let Some(rule) = rule_for_path(&path, source) else {
        return;
    };
    if seen.insert((source, path.clone())) {
        violations.push(ArtifactViolation {
            path,
            source,
            rule_id: rule.id.to_string(),
            remediation: rule.remediation.to_string(),
        });
    }
}

/// The two runtime bookkeeping files the amplihack launcher and session tracker
/// force into `<repo>/.claude/runtime/` as part of every launch. These are the
/// launcher's OWN state and are exempt in *every* git source (including staged),
/// because the launcher itself may stage them and flagging them turned the
/// end-of-run guard into a hang (issue #807). The rest of the runtime tree is
/// handled by [`is_claude_runtime_path`], which is exempt only for the
/// untracked/ignored-present output that recipes unavoidably produce.
const LAUNCHER_OWNED_RUNTIME_FILES: &[&str] = &[
    ".claude/runtime/launcher_context.json",
    ".claude/runtime/sessions.jsonl",
];

/// Whether `path` is one of the launcher-owned bookkeeping files exempt in all
/// git sources. See [`LAUNCHER_OWNED_RUNTIME_FILES`].
fn is_launcher_owned_runtime_file(path: &str) -> bool {
    LAUNCHER_OWNED_RUNTIME_FILES.contains(&path)
}

/// Whether `path` is the `.claude/runtime` directory or anything beneath it.
///
/// The whole `<repo>/.claude/runtime/` subtree is runtime bookkeeping that the
/// launcher, session tracker, and *every* agent's PostToolUse metrics hook write
/// continuously as a normal, unavoidable part of running an agent: session logs,
/// metrics (`metrics/post_tool_use_metrics.jsonl`, appended on every tool call),
/// locks, and power-steering state. This tree is `.gitignore`d and tool-generated.
///
/// The guard exempts it — but only when it appears as **untracked or
/// ignored-present** output (see [`rule_for_path`]). Flagging that output turned
/// the clean end-of-run guard step into a hard failure (issue #807 — and again
/// when it blocked the metrics file), which left `recipe-runner-rs` and its
/// child agents hung *after* the work was already committed and pushed,
/// discarding completed recipe work. Because the subtree is gitignored, an
/// `.amplihack-artifact-allowlist` entry cannot rescue it either
/// (`.claude/runtime` is a root-prohibited broad exemption; see
/// [`is_root_prohibited_exemption`]).
///
/// A **staged or tracked** `.claude/runtime/` path is not covered by this
/// exemption: deliberately committing runtime state into the published tree is
/// genuine pollution and is still blocked as `claude-runtime`.
fn is_claude_runtime_path(path: &str) -> bool {
    path == ".claude/runtime" || path.starts_with(".claude/runtime/")
}

fn rule_for_path(path: &str, source: ArtifactSource) -> Option<&'static ArtifactRule> {
    // Launcher-owned bookkeeping is exempt regardless of git source.
    if is_launcher_owned_runtime_file(path) {
        return None;
    }
    // The rest of the runtime tree is exempt only as the untracked/ignored
    // output every recipe unavoidably produces; staged/tracked runtime state
    // still falls through to the `claude-runtime` block below.
    if is_claude_runtime_path(path)
        && matches!(
            source,
            ArtifactSource::Untracked | ArtifactSource::IgnoredPresent
        )
    {
        return None;
    }
    if path_has_component(path, "node_modules") {
        return rule("node-modules");
    }
    if path == "dist/plugin.js" || path.ends_with("/dist/plugin.js") {
        return rule("plugin-bundle");
    }
    if is_claude_runtime_path(path) {
        return rule("claude-runtime");
    }
    if path == "worktrees" || path.starts_with("worktrees/") {
        return rule("nested-worktree");
    }
    if is_workflow_session_artifact_path(path) {
        return rule("workflow-session-artifact");
    }
    if is_cache_path(path) {
        return rule("cache-artifact");
    }
    if is_build_artifact_path(path, source) {
        return rule("build-artifact");
    }
    None
}

fn is_workflow_session_artifact_path(path: &str) -> bool {
    path == "recipe-runner.log"
        || path == "plan.md"
        || path == ".copilot/session-state"
        || path.starts_with(".copilot/session-state/")
        || path == ".amplihack/session-state"
        || path.starts_with(".amplihack/session-state/")
}

fn rule(id: &str) -> Option<&'static ArtifactRule> {
    DEFAULT_RULES.iter().find(|rule| rule.id == id)
}

fn path_has_component(path: &str, component: &str) -> bool {
    path.split('/').any(|part| part == component)
}

fn path_has_any_component(path: &str, components: &[&str]) -> bool {
    path.split('/').any(|part| components.contains(&part))
}

fn is_cache_path(path: &str) -> bool {
    path_has_any_component(
        path,
        &[
            ".cache",
            ".npm",
            ".pnpm-store",
            ".turbo",
            ".parcel-cache",
            ".pytest_cache",
        ],
    ) || path == ".yarn/cache"
        || path.starts_with(".yarn/cache/")
        || path.contains("/.yarn/cache/")
        || path == ".next/cache"
        || path.starts_with(".next/cache/")
        || path.contains("/.next/cache/")
}

fn is_build_artifact_path(path: &str, source: ArtifactSource) -> bool {
    if path_has_component(path, "target") {
        return source != ArtifactSource::IgnoredPresent;
    }
    if path == "index.scip" || path.ends_with("/index.scip") {
        return true;
    }
    if path_has_any_component(
        path,
        &["dist", "build", "coverage", "out", "logs", "outputs"],
    ) {
        return true;
    }
    path == ".next" || path.starts_with(".next/") || path.contains("/.next/")
}

fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

fn validate_allowlist_entry(
    entry: &str,
    path: &Path,
    line: usize,
) -> Result<(), ArtifactGuardError> {
    let unsafe_reason = |reason: &str| {
        ArtifactGuardError::Allowlist(format!(
            "{}:{line} unsafe allowlist entry `{entry}`: {reason}",
            path.display()
        ))
    };

    if entry.starts_with('/') {
        return Err(unsafe_reason("absolute paths are not allowed"));
    }
    if entry.contains('\\') {
        return Err(unsafe_reason("use / separators, not backslashes"));
    }
    let normalized = normalize_slashes(entry.trim_end_matches('/'));
    if normalized.is_empty() {
        return Err(unsafe_reason("empty entries are not allowed"));
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.contains(&"..") {
        return Err(unsafe_reason("parent traversal is not allowed"));
    }
    if matches!(normalized.as_str(), "*" | "**" | "**/*" | "*/**")
        || (!normalized.contains('/') && contains_glob_wildcard(&normalized))
    {
        return Err(unsafe_reason("repository-wide patterns are not allowed"));
    }
    if is_root_prohibited_exemption(&normalized) {
        return Err(unsafe_reason(
            "root artifact directories cannot be allowlisted broadly",
        ));
    }
    if normalized.starts_with("**/node_modules") || normalized.contains("/node_modules/**") {
        return Err(unsafe_reason(
            "node_modules directory exemptions are too broad",
        ));
    }
    Ok(())
}

fn is_root_prohibited_exemption(pattern: &str) -> bool {
    const ROOT_ARTIFACTS: &[&str] = &[
        "node_modules",
        "recipe-runner.log",
        "plan.md",
        ".copilot/session-state",
        ".amplihack/session-state",
        "dist",
        "build",
        "coverage",
        ".claude/runtime",
        "worktrees",
        ".cache",
        ".next",
        "out",
        "logs",
        "outputs",
    ];

    ROOT_ARTIFACTS.iter().any(|root| {
        root_artifact_suffix(pattern, root).is_some_and(|suffix| {
            suffix.is_empty() || suffix == "**" || contains_glob_wildcard(suffix)
        })
    })
}

fn root_artifact_suffix<'a>(pattern: &'a str, root: &str) -> Option<&'a str> {
    if pattern == root {
        return Some("");
    }
    pattern.strip_prefix(root)?.strip_prefix('/')
}

fn contains_glob_wildcard(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    glob_parts_match(&pattern_parts, &path_parts)
}

fn glob_parts_match(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        return glob_parts_match(&pattern[1..], path)
            || (!path.is_empty() && glob_parts_match(pattern, &path[1..]));
    }
    if path.is_empty() {
        return false;
    }
    segment_matches(pattern[0], path[0]) && glob_parts_match(&pattern[1..], &path[1..])
}

fn segment_matches(pattern: &str, text: &str) -> bool {
    segment_match_bytes(pattern.as_bytes(), text.as_bytes())
}

fn segment_match_bytes(pattern: &[u8], text: &[u8]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    match pattern[0] {
        b'*' => {
            segment_match_bytes(&pattern[1..], text)
                || (!text.is_empty() && segment_match_bytes(pattern, &text[1..]))
        }
        b'?' => !text.is_empty() && segment_match_bytes(&pattern[1..], &text[1..]),
        byte => {
            !text.is_empty() && byte == text[0] && segment_match_bytes(&pattern[1..], &text[1..])
        }
    }
}

#[cfg(test)]
#[path = "tests/artifact_guard_tests.rs"]
mod tests;
