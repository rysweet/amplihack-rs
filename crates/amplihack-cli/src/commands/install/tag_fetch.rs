//! Fetch the tree of one exact release tag (issue #1955).
//!
//! The tree isn't checked against a published hash: its version is checked by
//! the caller, and the git route is limited by time only.

use super::clone;
use super::framework_source::SourceVersion;
use super::paths::sanitize_text;
use super::types::REPO_GIT_URL;
use anyhow::Result;
use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tar::Archive;

/// Shared across the git and tarball routes. The update's child install has
/// 300 s, so a longer fetch would be killed by its parent.
pub(super) const EXPLICIT_FETCH_BUDGET: Duration = Duration::from_secs(180);
pub(super) const IMPLICIT_FETCH_BUDGET: Duration = Duration::from_secs(60);

/// A failure reason ends up in a one-line warning on every launch.
const REASON_LIMIT: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TarballLimits {
    pub(super) max_download_bytes: u64,
    pub(super) max_unpacked_bytes: u64,
    pub(super) max_entries: u64,
}

pub(super) const TARBALL_LIMITS: TarballLimits = TarballLimits {
    max_download_bytes: crate::update::MAX_TAG_TARBALL_BYTES,
    max_unpacked_bytes: 1024 * 1024 * 1024,
    max_entries: 100_000,
};

/// Room per entry for its header, padding and a normal long-path metadata entry,
/// on top of the unpacked limit, when capping the decompressed stream.
const ENTRY_OVERHEAD_BYTES: u64 = 4096;

/// Fetches one exact tag into `destination` and returns the tree's root.
/// Injected in tests; there is no production env var.
pub(super) trait TagFetcher {
    fn fetch(
        &self,
        version: &semver::Version,
        destination: &Path,
        budget: Duration,
    ) -> Result<PathBuf>;
}

/// The only error self-heal may continue past, and only with a compatible
/// installed bundle (D1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceFetchFailed {
    /// The fetch ran and failed. `reason` is one line with no control characters.
    Failed { version: String, reason: String },
    /// This build may not fetch a tag (D2).
    NotAllowed { version: String },
}

impl std::fmt::Display for SourceFetchFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { version, reason } => write!(
                f,
                "could not fetch the framework bundle from release tag v{version}: {reason}\nThe \
                 installed bundle and version stamp were not changed.\nRun `amplihack install \
                 --local <PATH>` to install from a checkout."
            ),
            Self::NotAllowed { version } => write!(
                f,
                "this build ({version}) cannot fetch a release tag, and no local source matches \
                 it.\nRun `amplihack install --local <PATH>` to install from a checkout."
            ),
        }
    }
}

impl std::error::Error for SourceFetchFailed {}

/// A fetched tree whose version is not the binary's. A hard failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FetchedWrongVersion {
    pub(crate) expected: String,
    pub(crate) found: SourceVersion,
}

impl std::fmt::Display for FetchedWrongVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the tree fetched for release tag v{} declares version {}, so it was not staged",
            self.expected, self.found
        )
    }
}

impl std::error::Error for FetchedWrongVersion {}

/// D2: only a build with `AMPLIHACK_RELEASE_VERSION` set and no `+` metadata.
/// `v1.2.0` is a real tag, so a dev build fetching it would stage a very old
/// bundle and succeed.
pub(super) fn fetch_allowed(version: &str, release_env_set: bool) -> bool {
    release_env_set && semver::Version::parse(version).is_ok_and(|parsed| parsed.build.is_empty())
}

pub(super) type BoundedHttpGet = dyn Fn(&str, Duration) -> Result<Vec<u8>>;

/// The production [`TagFetcher`]: git first, then the tag tarball.
pub(super) struct ReleaseTagFetcher {
    pub(super) git: Option<PathBuf>,
    pub(super) http: Box<BoundedHttpGet>,
}

impl ReleaseTagFetcher {
    pub(super) fn production() -> Self {
        Self {
            git: clone::which_git().ok(),
            http: Box::new(crate::update::http_get_bounded),
        }
    }
}

impl TagFetcher for ReleaseTagFetcher {
    fn fetch(
        &self,
        version: &semver::Version,
        destination: &Path,
        budget: Duration,
    ) -> Result<PathBuf> {
        // SEC-7: checked here as well as in D2, because the refspec and the URL
        // are built from this. Semver limits the rest to `[0-9A-Za-z.-]`.
        if !version.build.is_empty() {
            return Err(SourceFetchFailed::NotAllowed {
                version: version.to_string(),
            }
            .into());
        }
        let deadline = Instant::now() + budget;
        let tag = format!("v{version}");
        let mut failures = Vec::new();

        match &self.git {
            Some(git) => match fetch_with_git(git, &tag, destination, deadline) {
                Ok(root) => return Ok(root),
                Err(reason) => failures.push(format!("git: {reason}")),
            },
            None => failures.push("git: not found on PATH".to_string()),
        }
        // A partial tree from the failed route must not be mistaken for the
        // tarball's.
        let tarball_result = clear_directory(destination).and_then(|()| {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("the time limit ran out".to_string());
            }
            fetch_tarball(&self.http, &tag, destination, remaining)
        });
        match tarball_result {
            Ok(root) => Ok(root),
            Err(reason) => {
                failures.push(format!("tarball: {reason}"));
                Err(SourceFetchFailed::Failed {
                    version: version.to_string(),
                    reason: one_line(&failures.join("; ")),
                }
                .into())
            }
        }
    }
}

/// SEC-14: no captured output in here, and nothing that can forge a line.
fn one_line(text: &str) -> String {
    sanitize_text(text).chars().take(REASON_LIMIT).collect()
}

fn clear_directory(dir: &Path) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("could not clear the fetch directory: {error}");
    fs::create_dir_all(dir).map_err(failed)?;
    for entry in fs::read_dir(dir).map_err(failed)? {
        let entry = entry.map_err(failed)?;
        let removed = if entry.file_type().map_err(failed)?.is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        removed.map_err(failed)?;
    }
    Ok(())
}

/// D7: `git clone --branch` prefers a branch with the same name, so the tag is
/// fetched by its full ref. No remote is configured and the URL is fixed.
fn fetch_with_git(
    git: &Path,
    tag: &str,
    destination: &Path,
    deadline: Instant,
) -> Result<PathBuf, String> {
    let work = destination.join("repo");
    fs::create_dir_all(&work).map_err(|error| format!("could not create a directory: {error}"))?;
    let refspec = format!("refs/tags/{tag}");
    let steps: [&[&str]; 3] = [
        &["init", "--quiet", "--template="],
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth",
            "1",
            "--",
            REPO_GIT_URL,
            &refspec,
        ],
        &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
    ];
    for step in steps {
        let name = step[0];
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("the time limit ran out before git {name}"));
        }
        let output = clone::run_bounded_git(git, step, &work, remaining)
            .map_err(|error| format!("could not run git {name}: {error:#}"))?;
        match output.status {
            None => return Err(format!("git {name} timed out")),
            Some(status) if !status.success() => {
                tracing::debug!("git {name} failed: {}", sanitize_text(output.stderr.trim()));
                return Err(format!("git {name} failed ({status})"));
            }
            Some(_) => {}
        }
    }
    if !work.join("amplifier-bundle").is_dir() {
        return Err("the fetched tree has no amplifier-bundle".to_string());
    }
    Ok(work)
}

fn fetch_tarball(
    http: &BoundedHttpGet,
    tag: &str,
    destination: &Path,
    budget: Duration,
) -> Result<PathBuf, String> {
    let url = format!("{REPO_GIT_URL}/archive/refs/tags/{tag}.tar.gz");
    let archive = http(&url, budget).map_err(|error| format!("{error:#}"))?;
    extract_tag_tarball(&archive, destination, &TARBALL_LIMITS)?;
    clone::find_framework_repo_root(destination).map_err(|error| format!("{error:#}"))
}

/// SEC-3: the bounded extractor used for the tag tarball.
///
/// Only regular files and directories are unpacked. Symlinks and hard links are
/// skipped rather than created, which is what makes it safe to write through
/// the destination, and the copy doesn't stage symlinks anyway. A path with `..`
/// or an absolute path fails the fetch. Setuid, setgid and sticky bits are not
/// kept.
pub(super) fn extract_tag_tarball(
    archive: &[u8],
    destination: &Path,
    limits: &TarballLimits,
) -> Result<(), String> {
    if archive.len() as u64 > limits.max_download_bytes {
        return Err(format!(
            "the archive is over the limit of {} bytes",
            limits.max_download_bytes
        ));
    }
    let unreadable = |error: std::io::Error| format!("the archive can't be read: {error}");
    // `tar` reads metadata entries into memory itself, so the checks below never
    // see them. Only a cap on the decompressed stream limits those.
    let stream_limit = limits
        .max_unpacked_bytes
        .saturating_add(limits.max_entries.saturating_mul(ENTRY_OVERHEAD_BYTES));
    let mut tarball = Archive::new(GzDecoder::new(archive).take(stream_limit));
    let mut entries = 0u64;
    let mut unpacked = 0u64;
    for entry in tarball.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        entries += 1;
        if entries > limits.max_entries {
            return Err(format!(
                "the archive has over {} entries",
                limits.max_entries
            ));
        }
        // Counts skipped entries too, whose bytes still have to be decompressed.
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > limits.max_unpacked_bytes {
            return Err(format!(
                "the archive unpacks to over {} bytes",
                limits.max_unpacked_bytes
            ));
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let rel = entry.path().map_err(unreadable)?.into_owned();
        if !rel
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err("the archive holds a path outside its root".to_string());
        }
        let target = destination.join(&rel);
        let write_failed = |error: std::io::Error| format!("could not unpack the archive: {error}");
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(write_failed)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(write_failed)?;
        }
        let mode = entry.header().mode().map_err(unreadable)?;
        let mut file = fs::File::create(&target).map_err(write_failed)?;
        std::io::copy(&mut entry, &mut file).map_err(write_failed)?;
        set_unpacked_mode(&file, mode).map_err(write_failed)?;
    }
    // The cap looks like a normal end of archive to `tar`, so a stream cut off
    // by it would otherwise pass as a complete but shorter tree.
    if tarball.into_inner().limit() == 0 {
        return Err(format!(
            "the archive decompresses to over {stream_limit} bytes"
        ));
    }
    Ok(())
}

/// Execute bits are what the mode rule reads later. The owner keeps read and
/// write so the copy can read the file and the temp dir can be removed.
#[cfg(unix)]
fn set_unpacked_mode(file: &fs::File, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode((mode & 0o777) | 0o600))
}

#[cfg(not(unix))]
fn set_unpacked_mode(_file: &fs::File, _mode: u32) -> std::io::Result<()> {
    Ok(())
}
