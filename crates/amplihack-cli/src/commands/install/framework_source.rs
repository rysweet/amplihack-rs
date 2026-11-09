//! Decide which source an install stages from (issue #1955).
//!
//! A source is used only when its version equals the binary's, or when the user
//! named it with `--local`. The rules are in
//! `docs/reference/framework-source-selection.md`.
//!
//! The gate checks the version a source declares about itself. In a release
//! build it also compares the source's `amplifier-bundle/` with the release
//! manifest embedded in the binary (issue #1972), which shows the source matches
//! that release but doesn't prove where the release came from.

use super::InstallMode;
use super::bundle_manifest;
use super::clone::{
    self, CandidateVerdict, FrameworkRootOrigin, FrameworkRootQuery, FrameworkRootResolution,
    ResolutionNote,
};
use super::fetch_backoff;
use super::paths::sanitize_path;
use super::release_manifest::{ManifestFinding, ReleaseManifest};
use super::tag_fetch::{
    EXPLICIT_FETCH_BUDGET, FetchedManifestMismatch, FetchedWrongVersion, IMPLICIT_FETCH_BUDGET,
    SourceFetchFailed, TagFetcher, fetch_allowed,
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
    /// Issue #1972 (M3): a `--local` source at the binary's version that does
    /// not match the embedded release manifest. Always `None` for other origins.
    pub(super) manifest_warning: Option<String>,
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

    /// Warnings printed to stderr before staging (issue #1972).
    pub(super) fn warnings(&self) -> Vec<String> {
        self.manifest_warning.iter().cloned().collect()
    }
}

/// Message M3: `--local` stages a mismatched tree anyway.
fn local_manifest_warning(root: &Path, version: &str, finding: &ManifestFinding) -> String {
    let subject = format!(
        "⚠️  The --local source {} declares v{version}, but it",
        sanitize_path(root)
    );
    format!(
        "{}\nStaging it anyway because --local was given.",
        finding.render(&subject, version)
    )
}

/// Why resolution stopped at a source the user pointed at (issue #1972 adds
/// the release manifest case). Converts to the inner error, so callers keep
/// downcasting to [`SourceRefused`] or [`ReleaseManifestRefused`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Refusal {
    Version(SourceRefused),
    /// Boxed, so `Result<_, Refusal>` stays small.
    ReleaseManifest(Box<ReleaseManifestRefused>),
}

impl From<Refusal> for anyhow::Error {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Version(refused) => refused.into(),
            Refusal::ReleaseManifest(refused) => (*refused).into(),
        }
    }
}

/// Message M1: a plain `install` refused an `AMPLIHACK_HOME` or cwd source that
/// declares the binary's version but does not match the embedded manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseManifestRefused {
    pub(super) path: PathBuf,
    pub(super) label: String,
    pub(super) binary_version: String,
    pub(super) finding: ManifestFinding,
}

impl std::fmt::Display for ReleaseManifestRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = sanitize_path(&self.path);
        let quoted = shell_quote_if_needed(&path);
        let subject = format!(
            "refusing to stage the framework bundle from {path} ({}): it declares v{} but its \
             amplifier-bundle/",
            self.label, self.binary_version
        );
        write!(
            f,
            "{}\nNothing was deployed and the installed bundle is unchanged.\nRun `amplihack \
             install --local {quoted}` to stage this tree anyway.",
            self.finding.render(&subject, &self.binary_version)
        )
    }
}

impl std::error::Error for ReleaseManifestRefused {}

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

/// Resolve the local candidates, applying the version gate to each one. With a
/// release manifest, a candidate that passes the version gate is also hashed
/// and compared with it, and a mismatch takes the same branch as a version
/// mismatch (issue #1972).
pub(super) fn resolve_gated(
    query: &FrameworkRootQuery,
    binary_version: &str,
    rule: MismatchRule,
    release: Option<&ReleaseManifest>,
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
        let user_pointed_here = matches!(
            origin,
            FrameworkRootOrigin::AmplihackHome | FrameworkRootOrigin::CurrentDirectory
        );
        let refuse = rule == MismatchRule::RefuseUserSources && user_pointed_here;
        if matches {
            let Some(finding) = release.and_then(|release| release.check_candidate(candidate))
            else {
                return CandidateVerdict::Accept(version);
            };
            return if refuse {
                CandidateVerdict::Refuse(Refusal::ReleaseManifest(Box::new(
                    ReleaseManifestRefused {
                        path: candidate.to_path_buf(),
                        label: origin.label().to_string(),
                        binary_version: binary_version.to_string(),
                        finding,
                    },
                )))
            } else {
                CandidateVerdict::Skip(ResolutionNote::ReleaseManifestMismatch {
                    path: candidate.to_path_buf(),
                    label: origin.label(),
                    binary_version: binary_version.to_string(),
                    finding,
                })
            };
        }
        if refuse {
            CandidateVerdict::Refuse(Refusal::Version(SourceRefused {
                path: candidate.to_path_buf(),
                label: origin.label().to_string(),
                source_version: version,
                binary_version: binary_version.to_string(),
            }))
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

/// Decide the source. Every input is a parameter, including the release
/// manifest (`None`: this binary carries none).
pub(super) fn acquire_framework_source(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    binary: &BinaryIdentity,
    release: Option<&ReleaseManifest>,
    fetcher: &dyn TagFetcher,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<AcquiredSource> {
    if let Some(local) = &policy.local {
        let version = read_source_version(local);
        // D13: only a tree that claims to be this release is hashed; `--local`
        // stages it whatever the result.
        let manifest_warning = release
            .filter(|_| version.matches(&binary.version))
            .and_then(|release| {
                release
                    .check_candidate(local)
                    .map(|finding| local_manifest_warning(local, &release.version, &finding))
            });
        return Ok(AcquiredSource {
            root: local.clone(),
            origin: SourceOrigin::Local,
            version,
            _fetched_dir: None,
            manifest_warning,
        });
    }

    let resolution = resolve_for_policy(policy, query, &binary.version, release, validate);
    for note in &resolution.notes {
        eprintln!("{note}");
    }
    if let Some(refused) = resolution.refused {
        return Err(refused.into());
    }
    if let Some(found) = resolution.resolved {
        return Ok(AcquiredSource {
            root: found.root,
            origin: SourceOrigin::Resolved(found.origin),
            version: resolution.version.unwrap_or(SourceVersion::Unknown),
            _fetched_dir: None,
            manifest_warning: None,
        });
    }

    fetch_release_tag(policy, binary, release, fetcher, validate)
}

/// Resolve the local candidates a policy without `--local` consults, with the
/// mismatch rule that policy implies. The notes are returned, not printed.
pub(super) fn resolve_for_policy(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    binary_version: &str,
    release: Option<&ReleaseManifest>,
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
        resolve_gated(&home_only, binary_version, rule, release, validate)
    } else {
        resolve_gated(query, binary_version, rule, release, validate)
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
    release: Option<&ReleaseManifest>,
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
    // Issue #1972 D12: the manifest is compared before validation, so nothing
    // in a mismatched tree is read beyond its hashes. Inside the fetcher, so a
    // git checkout that fails it is replaced by the tag tarball before the
    // mismatch counts against the release (see `ReleaseTagFetcher`).
    let verify = |root: &Path| -> Result<()> {
        let found = read_fetched_tree_version(root);
        if !found.matches(&binary.version) {
            return Err(FetchedWrongVersion {
                expected: binary.version.clone(),
                found,
            }
            .into());
        }
        let Some(release) = release else {
            return Ok(());
        };
        let actual =
            bundle_manifest::of_source(&root.join("amplifier-bundle")).with_context(|| {
                format!(
                    "could not hash the tree fetched for release tag v{version} to compare it \
                     with the release manifest embedded in this binary; nothing was staged"
                )
            })?;
        match release.check(&actual) {
            Some(diff) => Err(FetchedManifestMismatch {
                version: binary.version.clone(),
                diff,
            }
            .into()),
            None => Ok(()),
        }
    };
    let fetched = fetcher.fetch_verified(&version, fetched_dir.path(), budget, &verify);
    if let Some(marker) = &policy.fetch_backoff {
        match &fetched {
            Err(error) => match error.downcast_ref::<SourceFetchFailed>() {
                Some(SourceFetchFailed::Failed { .. }) => {
                    fetch_backoff::record_failure(marker, &binary.version, SystemTime::now());
                }
                Some(SourceFetchFailed::NotAllowed { .. }) => {}
                // The tag was reached and its tree rejected: not a network
                // failure, so it neither starts nor extends a backoff.
                None => fetch_backoff::clear(marker),
            },
            Ok(_) => fetch_backoff::clear(marker),
        }
    }
    // A rejected tree is deleted here, when `fetched_dir` drops.
    let root = fetched?;
    let found = read_fetched_tree_version(&root);
    validate(&root).map_err(|error| {
        anyhow::anyhow!("the framework bundle from release tag v{version} is incompatible: {error}")
    })?;
    Ok(AcquiredSource {
        root,
        origin: SourceOrigin::ReleaseTag,
        version: found,
        _fetched_dir: Some(fetched_dir),
        manifest_warning: None,
    })
}

/// With no source for this binary, a stamp that differs from it means a bundle
/// was kept under D1. A matching stamp means the source really is missing.
pub(super) fn bundle_kept_without_source(stamp: Option<&str>, binary_version: &str) -> bool {
    stamp != Some(binary_version)
}
