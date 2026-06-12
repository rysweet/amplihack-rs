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
            &["diff", "--cached", "--name-only", "-z"],
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
        scan_ignored_present(&repo, &allowlist, &mut violations, &mut seen)?;
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

    let allowlist_path = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    ArtifactAllowlist::load(&allowlist_path)
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

    for raw in output.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(raw);
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

fn rule_for_path(path: &str, source: ArtifactSource) -> Option<&'static ArtifactRule> {
    if path_has_component(path, "node_modules") {
        return rule("node-modules");
    }
    if path == "dist/plugin.js" || path.ends_with("/dist/plugin.js") {
        return rule("plugin-bundle");
    }
    if path == ".claude/runtime" || path.starts_with(".claude/runtime/") {
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
    if matches!(normalized.as_str(), "*" | "**" | "**/*" | "*/**") {
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
    matches!(
        pattern,
        "node_modules"
            | "node_modules/**"
            | "recipe-runner.log"
            | "plan.md"
            | ".copilot/session-state"
            | ".copilot/session-state/**"
            | ".amplihack/session-state"
            | ".amplihack/session-state/**"
            | "dist"
            | "dist/**"
            | "build"
            | "build/**"
            | "coverage"
            | "coverage/**"
            | ".claude/runtime"
            | ".claude/runtime/**"
            | "worktrees"
            | "worktrees/**"
            | ".cache"
            | ".cache/**"
            | ".next"
            | ".next/**"
            | "out"
            | "out/**"
            | "logs"
            | "logs/**"
            | "outputs"
            | "outputs/**"
    )
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
