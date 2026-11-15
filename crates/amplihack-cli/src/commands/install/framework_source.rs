//! Decide which source an install stages from (issue #1955).
//!
//! A source is used only when its version equals the binary's, or when the user
//! named it with `--local`. The rules are in
//! `docs/reference/framework-source-selection.md`.
//!
//! The gate checks the version a source declares about itself. It doesn't prove
//! where the source came from.

use super::InstallMode;
use super::clone::{
    self, CandidateVerdict, FrameworkRootOrigin, FrameworkRootQuery, FrameworkRootResolution,
    ResolutionNote,
};
use super::fetch_backoff;
use super::paths::sanitize_path;
use super::tag_fetch::{
    EXPLICIT_FETCH_BUDGET, FetchedWrongVersion, IMPLICIT_FETCH_BUDGET, SourceFetchFailed,
    TagFetcher, fetch_allowed,
};
use super::version_stamp::{self, VersionRelation};
use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Largest `Cargo.toml` or stamp a version is read from (SEC-11).
const VERSION_FILE_LIMIT: u64 = 64 * 1024;
/// SEC-9: `git describe` runs in directories the user may only be standing in.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(5);
const DESCRIBE_OUTPUT_LIMIT: u64 = 256;

/// Which rules apply to this install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourcePolicy {
    pub(super) local: Option<PathBuf>,
    pub(super) force_refresh: bool,
    pub(super) mode: InstallMode,
    /// Where a failed tag fetch is remembered ([`fetch_backoff`]). `None`
    /// disables the backoff; production passes `~/.amplihack/.tag-fetch-failed`.
    pub(super) fetch_backoff: Option<PathBuf>,
}

/// `Known` is built only from a successful semver parse (SEC-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceVersion {
    Known(semver::Version),
    Unknown,
}

impl SourceVersion {
    pub(super) fn parse(raw: &str) -> Self {
        semver::Version::parse(raw).map_or(Self::Unknown, Self::Known)
    }

    /// Semver precedence, so build metadata doesn't count.
    fn matches(&self, binary_version: &str) -> bool {
        match self {
            Self::Known(version) => {
                version_stamp::relation(binary_version, &version.to_string())
                    == VersionRelation::Equal
            }
            Self::Unknown => false,
        }
    }
}

/// `v1.2.0`, or `unknown`.
impl std::fmt::Display for SourceVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(version) => write!(f, "v{version}"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// `[workspace.package].version`, then `git describe`, then unknown.
pub(super) fn read_source_version(root: &Path) -> SourceVersion {
    read_source_version_with_git(root, clone::which_git().ok().as_deref())
}

/// [`read_source_version`] with the git binary passed in (`None`: no git).
pub(super) fn read_source_version_with_git(root: &Path, git: Option<&Path>) -> SourceVersion {
    match read_fetched_tree_version(root) {
        SourceVersion::Unknown => {
            git.map_or(SourceVersion::Unknown, |git| describe_exact_tag(root, git))
        }
        known => known,
    }
}

/// SEC-10: a fetched tree's version comes from `Cargo.toml` only.
pub(super) fn read_fetched_tree_version(root: &Path) -> SourceVersion {
    read_small_regular_file(&root.join("Cargo.toml"))
        .filter(|text| text.contains(clone::AMPLIHACK_WORKSPACE_MARKER))
        .and_then(|text| {
            let parsed: toml::Table = text.parse().ok()?;
            let version = parsed
                .get("workspace")?
                .get("package")?
                .get("version")?
                .as_str()?;
            Some(SourceVersion::parse(version))
        })
        .unwrap_or(SourceVersion::Unknown)
}

/// SEC-11: only a regular file within the limit, so a FIFO can't block a launch
/// and a symlink can't point the read elsewhere.
fn read_small_regular_file(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > VERSION_FILE_LIMIT {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(VERSION_FILE_LIMIT)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn describe_exact_tag(root: &Path, git: &Path) -> SourceVersion {
    let output = clone::run_bounded_git_with(
        git,
        &["describe", "--exact-match", "--tags", "--match", "v*"],
        root,
        DESCRIBE_TIMEOUT,
        // A tag on an unrelated repository above this directory says nothing
        // about the tree in it.
        &|command| {
            if let Some(parent) = root.parent() {
                command.env("GIT_CEILING_DIRECTORIES", parent);
            }
        },
    );
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            tracing::debug!("git describe in {}: {error:#}", root.display());
            return SourceVersion::Unknown;
        }
    };
    if !output.succeeded() || output.stdout_bytes > DESCRIBE_OUTPUT_LIMIT {
        return SourceVersion::Unknown;
    }
    output
        .stdout
        .trim()
        .strip_prefix('v')
        .map_or(SourceVersion::Unknown, SourceVersion::parse)
}

/// The binary doing the install, passed in so tests don't depend on the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BinaryIdentity {
    pub(super) version: String,
    pub(super) may_fetch: bool,
}

impl BinaryIdentity {
    pub(super) fn current() -> Self {
        Self {
            version: crate::VERSION.to_string(),
            may_fetch: fetch_allowed(
                crate::VERSION,
                option_env!("AMPLIHACK_RELEASE_VERSION").is_some(),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceOrigin {
    Local,
    Resolved(FrameworkRootOrigin),
    ReleaseTag,
}

/// The chosen source. `_fetched_dir` keeps a fetched tree alive until
/// `local_install_scoped` returns (SEC-13).
#[derive(Debug)]
pub(super) struct AcquiredSource {
    pub(super) root: PathBuf,
    pub(super) origin: SourceOrigin,
    pub(super) version: SourceVersion,
    pub(super) _fetched_dir: Option<tempfile::TempDir>,
}

impl AcquiredSource {
    /// The lines printed before staging.
    pub(super) fn describe(&self, binary_version: &str) -> Vec<String> {
        let from = match self.origin {
            SourceOrigin::Local => "--local",
            SourceOrigin::Resolved(origin) => origin.label(),
            SourceOrigin::ReleaseTag => "the release tag",
        };
        let mut lines = vec![format!(
            "📦 Using framework source {} ({}, from {from})",
            sanitize_path(&self.root),
            self.version
        )];
        if self.origin == SourceOrigin::Local && !self.version.matches(binary_version) {
            lines.push(format!(
                "⚠️  This source is {} but this binary is v{binary_version}. Staging it because \
                 --local was given.",
                self.version
            ));
        }
        lines
    }
}

/// A plain `install` refused a mismatched `AMPLIHACK_HOME` or cwd source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceRefused {
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) source_version: SourceVersion,
    pub(crate) binary_version: String,
}

impl std::fmt::Display for SourceRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = sanitize_path(&self.path);
        let quoted = shell_quote_if_needed(&path);
        write!(
            f,
            "refusing to stage the framework bundle from {path} ({}):\n  source version: {}\n  \
             binary version: v{}\nRun `amplihack install --local {quoted}` to stage this tree \
             anyway.",
            self.label, self.source_version, self.binary_version
        )
    }
}

impl std::error::Error for SourceRefused {}

/// Quote `text` for a POSIX shell when it holds anything but plain path
/// characters, so the printed hint can be pasted as it is.
fn shell_quote_if_needed(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@,=~".contains(c))
        && !text.starts_with('~');
    if plain {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// What a candidate with another version gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MismatchRule {
    /// A plain `install`: `AMPLIHACK_HOME` or the cwd is what the user pointed
    /// at, so staging something else instead would be surprising.
    RefuseUserSources,
    NoteAndContinue,
}

/// The stamp's first line, unparsed. `~/.amplihack` qualifies only on an exact
/// match, so a host moving between snapshots doesn't re-stage its old bundle.
fn read_stamp(staged: &Path) -> Option<String> {
    read_small_regular_file(&staged.join(version_stamp::STAMP_FILE))?
        .lines()
        .next()
        .map(|line| line.trim().to_string())
}

/// Resolve the local candidates, applying the version gate to each one.
pub(super) fn resolve_gated(
    query: &FrameworkRootQuery,
    binary_version: &str,
    rule: MismatchRule,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> FrameworkRootResolution {
    let gate = |candidate: &Path, origin: FrameworkRootOrigin| {
        let (version, matches) = if origin == FrameworkRootOrigin::PriorStagedInstall {
            let stamp = read_stamp(candidate);
            let version = stamp
                .as_deref()
                .map_or(SourceVersion::Unknown, SourceVersion::parse);
            (version, stamp.as_deref() == Some(binary_version))
        } else {
            let version = read_source_version(candidate);
            let matches = version.matches(binary_version);
            (version, matches)
        };
        if matches {
            return CandidateVerdict::Accept(version);
        }
        let user_pointed_here = matches!(
            origin,
            FrameworkRootOrigin::AmplihackHome | FrameworkRootOrigin::CurrentDirectory
        );
        if rule == MismatchRule::RefuseUserSources && user_pointed_here {
            CandidateVerdict::Refuse(SourceRefused {
                path: candidate.to_path_buf(),
                label: origin.label().to_string(),
                source_version: version,
                binary_version: binary_version.to_string(),
            })
        } else {
            CandidateVerdict::Skip(ResolutionNote::VersionMismatch {
                path: candidate.to_path_buf(),
                label: origin.label(),
                source_version: version,
                binary_version: binary_version.to_string(),
            })
        }
    };
    clone::resolve_framework_root_gated(&query.without_staged_home(), validate, &gate)
}

/// Decide the source. Every input is a parameter.
pub(super) fn acquire_framework_source(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    binary: &BinaryIdentity,
    fetcher: &dyn TagFetcher,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<AcquiredSource> {
    if let Some(source) = available_framework_source(policy, query, binary, validate)? {
        return Ok(source);
    }
    fetch_release_tag(policy, binary, fetcher, validate)
}

/// Assess local assets without fetching, caching or changing installation state.
/// The coordinator resolves release executables before acquiring missing assets.
pub(super) fn available_framework_source(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    binary: &BinaryIdentity,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Option<AcquiredSource>> {
    if let Some(local) = &policy.local {
        return Ok(Some(AcquiredSource {
            root: local.clone(),
            origin: SourceOrigin::Local,
            version: read_source_version(local),
            _fetched_dir: None,
        }));
    }

    let resolution = resolve_for_policy(policy, query, &binary.version, validate);
    for note in &resolution.notes {
        eprintln!("{note}");
    }
    if let Some(refused) = resolution.refused {
        return Err(refused.into());
    }
    if let Some(found) = resolution.resolved {
        return Ok(Some(AcquiredSource {
            root: found.root,
            origin: SourceOrigin::Resolved(found.origin),
            version: resolution.version.unwrap_or(SourceVersion::Unknown),
            _fetched_dir: None,
        }));
    }

    Ok(None)
}

/// Resolve the local candidates a policy without `--local` consults, with the
/// mismatch rule that policy implies. The notes are returned, not printed.
pub(super) fn resolve_for_policy(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    binary_version: &str,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> FrameworkRootResolution {
    let rule = if policy.mode == InstallMode::Explicit && !policy.force_refresh {
        MismatchRule::RefuseUserSources
    } else {
        MismatchRule::NoteAndContinue
    };
    // D6: `--force-refresh` keeps `update` independent of the cwd. It consults
    // `AMPLIHACK_HOME`, so a tag worktree works offline, and then the tag.
    if policy.force_refresh {
        let home_only = FrameworkRootQuery {
            amplihack_home: query.without_staged_home().amplihack_home,
            ..FrameworkRootQuery::default()
        };
        resolve_gated(&home_only, binary_version, rule, validate)
    } else {
        resolve_gated(query, binary_version, rule, validate)
    }
}

/// The line printed before a tag fetch, which says why the fetch runs.
pub(super) fn fetch_reason(version: &semver::Version, force_refresh: bool) -> String {
    if force_refresh {
        format!(
            "📥 Fetching the framework bundle from release tag v{version} because \
             --force-refresh was given."
        )
    } else {
        format!(
            "📥 Fetching the framework bundle from release tag v{version} because no local \
             source matches this binary."
        )
    }
}

fn fetch_release_tag(
    policy: &SourcePolicy,
    binary: &BinaryIdentity,
    fetcher: &dyn TagFetcher,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<AcquiredSource> {
    let version = semver::Version::parse(&binary.version)
        .ok()
        .filter(|_| binary.may_fetch)
        .ok_or_else(|| SourceFetchFailed::NotAllowed {
            version: binary.version.clone(),
        })?;
    let budget = match policy.mode {
        InstallMode::Explicit => EXPLICIT_FETCH_BUDGET,
        InstallMode::Implicit => IMPLICIT_FETCH_BUDGET,
    };
    // Only a launch backs off; a user who asked for an install gets a fetch.
    if policy.mode == InstallMode::Implicit
        && let Some(marker) = &policy.fetch_backoff
        && let Some(age) = fetch_backoff::recent_failure(marker, &binary.version, SystemTime::now())
    {
        return Err(SourceFetchFailed::Failed {
            version: binary.version.clone(),
            reason: fetch_backoff::skipped_reason(age),
        }
        .into());
    }
    let fetched_dir = tempfile::tempdir().context("failed to create a temp dir for the fetch")?;
    // Stderr, because a launch on an offline host repeats this and its stdout
    // may be going somewhere that parses it.
    eprintln!("{}", fetch_reason(&version, policy.force_refresh));
    let fetched = fetcher.fetch(&version, fetched_dir.path(), budget);
    if let Some(marker) = &policy.fetch_backoff {
        match &fetched {
            Ok(_) => fetch_backoff::clear(marker),
            Err(error)
                if matches!(
                    error.downcast_ref::<SourceFetchFailed>(),
                    Some(SourceFetchFailed::Failed { .. })
                ) =>
            {
                fetch_backoff::record_failure(marker, &binary.version, SystemTime::now());
            }
            Err(_) => {}
        }
    }
    let root = fetched?;

    let found = read_fetched_tree_version(&root);
    if !found.matches(&binary.version) {
        return Err(FetchedWrongVersion {
            expected: binary.version.clone(),
            found,
        }
        .into());
    }
    validate(&root).map_err(|error| {
        anyhow::anyhow!("the framework bundle from release tag v{version} is incompatible: {error}")
    })?;
    Ok(AcquiredSource {
        root,
        origin: SourceOrigin::ReleaseTag,
        version: found,
        _fetched_dir: Some(fetched_dir),
    })
}

/// With no source for this binary, a stamp that differs from it means a bundle
/// was kept under D1. A matching stamp means the source really is missing.
pub(super) fn bundle_kept_without_source(stamp: Option<&str>, binary_version: &str) -> bool {
    stamp != Some(binary_version)
}
