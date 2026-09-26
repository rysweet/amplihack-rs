use anyhow::Result;
use serde::Serialize;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleWrapperNeutralizerConfig {
    pub(crate) home_dir: PathBuf,
    pub(crate) current_exe: PathBuf,
    pub(crate) preferred_rust_binary: PathBuf,
    pub(crate) path_dirs: Vec<PathBuf>,
    pub(crate) binary_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleWrapperNeutralizerReport {
    pub(crate) neutralized: Vec<NeutralizedWrapper>,
    pub(crate) manifest_path: Option<PathBuf>,
    pub(crate) resolved_after: PathBuf,
    /// Transient `npx` shims for our own npm wrapper that sit ahead of the
    /// Rust binary on PATH, shadowing it only while the launching `npx`
    /// process runs (issue #1480). They are left in place and reported so the
    /// caller can warn. Empty when the Rust binary is not on PATH at all.
    pub(crate) skipped_transient_shims: Vec<PathBuf>,
    /// Persistent launchers for our own npm wrapper (`npm install -g`, a
    /// project's `node_modules/.bin`, `pnpm add -g`) that sit ahead of the
    /// Rust binary on PATH (issue #1496). They keep shadowing it after
    /// install, so they are left in place, never treated as unknown, and the
    /// post-install PATH advisory tells the user which binary wins.
    pub(crate) persistent_npm_launchers: Vec<PathBuf>,
}

/// How long a launcher for our npm wrapper stays on PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NpmLauncherKind {
    /// `npx` / `pnpm dlx` / `bunx` cache entry: gone once that command exits.
    Transient,
    /// `npm install -g`, a project dependency, `pnpm add -g`: stays.
    Persistent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NeutralizedWrapper {
    pub(crate) original_path: PathBuf,
    pub(crate) quarantine_path: PathBuf,
    pub(crate) kind: NeutralizedWrapperKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NeutralizedWrapperKind {
    StalePythonWrapper,
    StaleUvxWrapper,
}

impl NeutralizedWrapperKind {
    fn as_manifest_kind(self) -> &'static str {
        match self {
            Self::StalePythonWrapper => "stale-python-wrapper",
            Self::StaleUvxWrapper => "stale-uvx-wrapper",
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum StaleWrapperRepairError {
    #[error(
        "unknown executable {path} shadows the Rust amplihack binary at {preferred}; leaving it untouched"
    )]
    UnknownShadowingExecutable { path: PathBuf, preferred: PathBuf },
    #[error(
        "inaccessible executable {path} shadows the Rust amplihack binary at {preferred}: {source}"
    )]
    InaccessibleShadowingExecutable {
        path: PathBuf,
        preferred: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "stale wrapper repair failed: {resolved_after} still resolves before the Rust amplihack binary at {preferred}"
    )]
    RustBinaryStillShadowed {
        resolved_after: PathBuf,
        preferred: PathBuf,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathCandidateKind {
    CurrentRustBinary,
    PreferredRustBinary,
    StalePythonWrapper,
    StaleUvxWrapper,
    NpmLauncher(NpmLauncherKind),
    UnknownExecutable,
    Inaccessible(String),
}

#[derive(Debug, Serialize)]
struct Manifest {
    generated_at_unix_secs: u64,
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Serialize)]
struct ManifestEntry {
    original_path: String,
    quarantine_path: String,
    kind: String,
    size: u64,
    modified_unix_secs: Option<u64>,
    action: &'static str,
}

pub(crate) fn neutralize_shadowing_stale_wrappers(
    config: StaleWrapperNeutralizerConfig,
) -> Result<StaleWrapperNeutralizerReport, StaleWrapperRepairError> {
    let preferred = config
        .preferred_rust_binary
        .canonicalize()
        .unwrap_or_else(|_| config.preferred_rust_binary.clone());
    let current = config
        .current_exe
        .canonicalize()
        .unwrap_or_else(|_| config.current_exe.clone());
    let candidates = executable_path_candidates(&config);
    let preferred_index = candidates.iter().position(|candidate| {
        same_path(candidate, &config.preferred_rust_binary) || same_path(candidate, &preferred)
    });
    let preferred_on_path = preferred_index.is_some();
    let shadowing_candidates = match preferred_index {
        Some(index) => &candidates[..index],
        None => candidates.as_slice(),
    };

    let mut neutralized = Vec::new();
    let mut skipped_transient_shims = Vec::new();
    let mut persistent_npm_launchers = Vec::new();
    let mut manifest_entries = Vec::new();
    let mut run_dir = None;

    for (counter, candidate) in shadowing_candidates.iter().enumerate() {
        let kind = classify_path_candidate(candidate, &preferred, &current, &config.home_dir)
            .map_err(
                |source| StaleWrapperRepairError::InaccessibleShadowingExecutable {
                    path: candidate.clone(),
                    preferred: config.preferred_rust_binary.clone(),
                    source,
                },
            )?;
        match kind {
            PathCandidateKind::PreferredRustBinary | PathCandidateKind::CurrentRustBinary => {}
            PathCandidateKind::NpmLauncher(kind) => {
                // Only a launcher ahead of the Rust binary on PATH shadows it.
                if preferred_on_path {
                    match kind {
                        NpmLauncherKind::Transient => {
                            skipped_transient_shims.push(candidate.clone());
                        }
                        NpmLauncherKind::Persistent => {
                            persistent_npm_launchers.push(candidate.clone());
                        }
                    }
                }
            }
            PathCandidateKind::StalePythonWrapper | PathCandidateKind::StaleUvxWrapper => {
                let wrapper_kind = match kind {
                    PathCandidateKind::StalePythonWrapper => {
                        NeutralizedWrapperKind::StalePythonWrapper
                    }
                    PathCandidateKind::StaleUvxWrapper => NeutralizedWrapperKind::StaleUvxWrapper,
                    _ => unreachable!("matched stale wrapper kinds only"),
                };
                if run_dir.is_none() {
                    run_dir = Some(quarantine_run_dir(&config.home_dir)?);
                }
                let Some(quarantine_root) = run_dir.as_ref() else {
                    return Err(io::Error::other("quarantine run dir was not initialized").into());
                };
                let quarantine_path = quarantine_path_for(quarantine_root, candidate, counter);
                if let Some(parent) = quarantine_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let metadata = fs::symlink_metadata(candidate)?;
                quarantine_file(candidate, &quarantine_path)?;
                manifest_entries.push(ManifestEntry {
                    original_path: candidate.display().to_string(),
                    quarantine_path: quarantine_path.display().to_string(),
                    kind: wrapper_kind.as_manifest_kind().to_string(),
                    size: metadata.len(),
                    modified_unix_secs: metadata.modified().ok().and_then(system_time_secs),
                    action: "quarantined",
                });
                neutralized.push(NeutralizedWrapper {
                    original_path: candidate.clone(),
                    quarantine_path,
                    kind: wrapper_kind,
                });
            }
            PathCandidateKind::UnknownExecutable => {
                if preferred_on_path {
                    return Err(StaleWrapperRepairError::UnknownShadowingExecutable {
                        path: candidate.clone(),
                        preferred: config.preferred_rust_binary.clone(),
                    });
                }
            }
            PathCandidateKind::Inaccessible(reason) => {
                if preferred_on_path {
                    return Err(StaleWrapperRepairError::InaccessibleShadowingExecutable {
                        path: candidate.clone(),
                        preferred: config.preferred_rust_binary.clone(),
                        source: io::Error::other(reason),
                    });
                }
            }
        }
    }

    let manifest_path = match run_dir {
        Some(run_dir) => {
            let manifest_path = run_dir.join("manifest.json");
            fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&Manifest {
                    generated_at_unix_secs: now_secs(),
                    entries: manifest_entries,
                })?,
            )?;
            Some(manifest_path)
        }
        None => None,
    };

    let resolved_after = if preferred_on_path {
        resolve_binary_on_path(&config).unwrap_or_else(|| config.preferred_rust_binary.clone())
    } else {
        config.preferred_rust_binary.clone()
    };
    if preferred_on_path && !same_path(&resolved_after, &config.preferred_rust_binary) {
        let resolved_kind =
            classify_path_candidate(&resolved_after, &preferred, &current, &config.home_dir)
                .map_err(
                    |source| StaleWrapperRepairError::InaccessibleShadowingExecutable {
                        path: resolved_after.clone(),
                        preferred: config.preferred_rust_binary.clone(),
                        source,
                    },
                )?;
        if !matches!(
            resolved_kind,
            PathCandidateKind::PreferredRustBinary
                | PathCandidateKind::CurrentRustBinary
                | PathCandidateKind::NpmLauncher(_)
        ) {
            return Err(StaleWrapperRepairError::RustBinaryStillShadowed {
                resolved_after,
                preferred: config.preferred_rust_binary,
            });
        }
    }

    Ok(StaleWrapperNeutralizerReport {
        neutralized,
        manifest_path,
        resolved_after,
        skipped_transient_shims,
        persistent_npm_launchers,
    })
}

fn executable_path_candidates(config: &StaleWrapperNeutralizerConfig) -> Vec<PathBuf> {
    config
        .path_dirs
        .iter()
        .map(|dir| dir.join(&config.binary_name))
        .filter(|path| is_executable_file(path))
        .collect()
}

fn resolve_binary_on_path(config: &StaleWrapperNeutralizerConfig) -> Option<PathBuf> {
    executable_path_candidates(config).into_iter().next()
}

fn classify_path_candidate(
    path: &Path,
    preferred: &Path,
    current: &Path,
    home: &Path,
) -> io::Result<PathCandidateKind> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if canonical == *preferred || same_path(path, preferred) {
        return Ok(PathCandidateKind::PreferredRustBinary);
    }
    if canonical == *current || same_path(path, current) {
        return Ok(PathCandidateKind::CurrentRustBinary);
    }

    if let Some(kind) = npm_launcher(path, &canonical) {
        return Ok(PathCandidateKind::NpmLauncher(kind));
    }

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        if !is_safe_wrapper_location(&canonical, home) {
            return Ok(PathCandidateKind::UnknownExecutable);
        }
    } else if !is_safe_wrapper_location(path, home) {
        return Ok(PathCandidateKind::UnknownExecutable);
    }

    let content = match read_prefix(path) {
        Ok(content) => content,
        Err(err) => return Ok(PathCandidateKind::Inaccessible(err.to_string())),
    };
    if is_stale_uvx_wrapper(&content) {
        return Ok(PathCandidateKind::StaleUvxWrapper);
    }
    if is_stale_python_wrapper(&content) {
        return Ok(PathCandidateKind::StalePythonWrapper);
    }
    Ok(PathCandidateKind::UnknownExecutable)
}

fn is_safe_wrapper_location(path: &Path, home: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let rel = relative.to_string_lossy().replace('\\', "/");
    rel.starts_with(".local/share/uv/")
        || rel.starts_with(".cache/uv/")
        || rel.starts_with(".amplihack/")
}

/// Recognize a package-manager launcher for this package's own
/// `npm/bin/amplihack.js` wrapper (issues #1480, #1496), whatever put it on
/// PATH: the `node_modules/.bin/amplihack` symlink that `npx`, `bunx`,
/// `npm install -g` or a project dependency create, or the `sh` shim script
/// that `pnpm` writes (`exec node "$basedir/../.pnpm/…/npm/bin/amplihack.js"`).
/// The launcher must resolve to a file that really is our wrapper; a
/// look-alike path is not enough. The kind says whether it outlives the
/// command that created it.
fn npm_launcher(path: &Path, canonical: &Path) -> Option<NpmLauncherKind> {
    let target = npm_launcher_target(path, canonical)?;
    let target_text = target.to_string_lossy().replace('\\', "/");
    if !target_text.ends_with("/npm/bin/amplihack.js") {
        return None;
    }
    if !read_prefix(&target).is_ok_and(|content| is_amplihack_npm_wrapper(&content)) {
        return None;
    }
    Some(if is_transient_launcher_location(path) {
        NpmLauncherKind::Transient
    } else {
        NpmLauncherKind::Persistent
    })
}

/// The script a launcher runs: the symlink target, or for an `sh` shim the
/// first `"$basedir/…/*.js"` it `exec`s, resolved next to the shim.
fn npm_launcher_target(path: &Path, canonical: &Path) -> Option<PathBuf> {
    let is_symlink = fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink());
    if is_symlink {
        return Some(canonical.to_path_buf());
    }
    let content = read_prefix(path).ok()?;
    if !content.starts_with("#!/bin/sh") && !content.starts_with("#!/usr/bin/env sh") {
        return None;
    }
    let shim_dir = path.parent()?;
    for line in content.lines() {
        let Some(mut rest) = line.trim_start().strip_prefix("exec ") else {
            continue;
        };
        while let Some(start) = rest.find("\"$basedir/") {
            let after = &rest[start + "\"$basedir/".len()..];
            let Some(end) = after.find('"') else { break };
            let relative = &after[..end];
            if relative.ends_with(".js") {
                return fs::canonicalize(shim_dir.join(relative)).ok();
            }
            rest = &after[end + 1..];
        }
    }
    None
}

/// Launchers under a package-manager run cache disappear with the command
/// that made them. Only the documented shapes count, anchored on the
/// `node_modules/.bin/<name>` tail, so a persistent launcher under a directory
/// that merely contains `dlx-` or `bunx-` in its name is not mistaken for one:
///
/// - `npx`:      `.../_npx/<hash>/node_modules/.bin/<name>`
/// - `pnpm dlx`: `.../pnpm/dlx/<hash>/<id>/node_modules/.bin/<name>`
/// - `bunx`:     `.../bunx-<uid>-<pkg>@<ver>/node_modules/.bin/<name>`
/// - `yarn dlx`: `.../xfs-<hash>/dlx-<pid>/.../node_modules/.bin/<name>`
fn is_transient_launcher_location(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let Some(bin_at) = parts
        .len()
        .checked_sub(3)
        .filter(|&i| parts[i] == "node_modules" && parts[i + 1] == ".bin")
    else {
        return false;
    };
    let ancestors = &parts[..bin_at];
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());

    // npx: `_npx/<hash>` directly above node_modules.
    if bin_at >= 2 && ancestors[bin_at - 2] == "_npx" && !ancestors[bin_at - 1].is_empty() {
        return true;
    }
    // pnpm dlx: `pnpm/dlx/<hash>/<id>` directly above node_modules.
    if bin_at >= 4 && ancestors[bin_at - 4] == "pnpm" && ancestors[bin_at - 3] == "dlx" {
        return true;
    }
    // bunx: `bunx-<uid>-...` directly above node_modules.
    if bin_at >= 1
        && ancestors[bin_at - 1]
            .strip_prefix("bunx-")
            .and_then(|rest| rest.split_once('-'))
            .is_some_and(|(uid, _)| all_digits(uid))
    {
        return true;
    }
    // yarn dlx: an `xfs-<hash>` temp dir with a `dlx-<pid>` project inside it.
    if let Some(xfs) = ancestors.iter().position(|p| p.starts_with("xfs-"))
        && ancestors[xfs + 1..]
            .iter()
            .any(|p| p.strip_prefix("dlx-").is_some_and(all_digits))
    {
        return true;
    }
    false
}

/// [`npm_launcher`] for a PATH entry that has not been resolved yet, so the
/// post-install PATH advisory can recognize the same launchers.
pub(crate) fn npm_launcher_path(path: &Path) -> Option<NpmLauncherKind> {
    let canonical = fs::canonicalize(path).ok()?;
    npm_launcher(path, &canonical)
}

fn is_amplihack_npm_wrapper(content: &str) -> bool {
    content.contains("ensureNativeBinaries") && content.contains("amplihack npm wrapper")
}

fn read_prefix(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.by_ref().take(64 * 1024).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn is_stale_python_wrapper(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    (lower.starts_with("#!")
        && lower
            .lines()
            .next()
            .is_some_and(|line| line.contains("python")))
        && (content.contains("from amplihack")
            || content.contains("import amplihack")
            || content.contains("load_entry_point")
            || content.contains("amplihack.cli"))
}

fn is_stale_uvx_wrapper(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    ((lower.contains("uvx") || lower.contains("uv tool"))
        && lower.contains("amplihack")
        && (lower.starts_with("#!") || lower.contains("generated")))
        || (lower.starts_with("#!")
            && lower.contains("exec")
            && lower.contains(".local/bin/amplihack"))
}

fn quarantine_run_dir(home: &Path) -> io::Result<PathBuf> {
    let dir = home
        .join(".amplihack")
        .join("quarantine")
        .join("stale-wrappers")
        .join(format!("{}-{}", now_secs(), std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn quarantine_path_for(run_dir: &Path, original: &Path, counter: usize) -> PathBuf {
    let sanitized = original
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("__");
    run_dir.join(format!(
        "{counter:03}-{}",
        sanitize_path_segment(if sanitized.is_empty() {
            "amplihack".into()
        } else {
            sanitized
        })
    ))
}

fn sanitize_path_segment(segment: String) -> String {
    segment
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn quarantine_file(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
            fs::copy(source, destination)?;
            fs::remove_file(source)
        }
        Err(err) => Err(err),
    }
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn system_time_secs(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn now_secs() -> u64 {
    system_time_secs(SystemTime::now()).unwrap_or(0)
}
