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
        matches_known_path(candidate, &config.preferred_rust_binary, &preferred)
    });
    let preferred_on_path = preferred_index.is_some();
    let shadowing_candidates = match preferred_index {
        Some(index) => &candidates[..index],
        None => candidates.as_slice(),
    };

    let mut neutralized = Vec::new();
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
    if preferred_on_path
        && !matches_known_path(&resolved_after, &config.preferred_rust_binary, &preferred)
    {
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
            PathCandidateKind::PreferredRustBinary | PathCandidateKind::CurrentRustBinary
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
    if canonical == *preferred || path == preferred {
        return Ok(PathCandidateKind::PreferredRustBinary);
    }
    if canonical == *current || path == current {
        return Ok(PathCandidateKind::CurrentRustBinary);
    }

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        if !is_safe_wrapper_location(path, home) || !is_safe_wrapper_location(&canonical, home) {
            return Ok(PathCandidateKind::UnknownExecutable);
        }
    } else if !is_safe_wrapper_location(path, home) {
        return Ok(PathCandidateKind::UnknownExecutable);
    }

    let content = match read_prefix(path) {
        Ok(content) => content,
        Err(err) => return Ok(PathCandidateKind::Inaccessible(err.to_string())),
    };
    if let Some(kind) = classify_wrapper_content(&content) {
        return Ok(kind);
    }
    Ok(PathCandidateKind::UnknownExecutable)
}

fn is_safe_wrapper_location(path: &Path, home: &Path) -> bool {
    if is_under_default_denied_system_prefix(path) {
        return false;
    }
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let rel = relative.to_string_lossy().replace('\\', "/");
    rel.starts_with(".local/share/uv/")
        || rel.starts_with(".cache/uv/")
        || rel.starts_with(".amplihack/")
}

fn is_under_default_denied_system_prefix(path: &Path) -> bool {
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    crate::path_conflicts::default_denied_system_prefixes()
        .into_iter()
        .any(|prefix| {
            if path.starts_with(&prefix) {
                return true;
            }
            let canonical_prefix = prefix.canonicalize().unwrap_or(prefix);
            canonical_path.starts_with(canonical_prefix)
        })
}

fn read_prefix(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(64 * 1024);
    file.by_ref().take(64 * 1024).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn classify_wrapper_content(content: &str) -> Option<PathCandidateKind> {
    let lower = content.to_ascii_lowercase();
    if is_stale_uvx_wrapper_lowercase(&lower) {
        return Some(PathCandidateKind::StaleUvxWrapper);
    }
    if is_stale_python_wrapper_lowercase(&lower) {
        return Some(PathCandidateKind::StalePythonWrapper);
    }
    None
}

fn is_stale_python_wrapper_lowercase(lower: &str) -> bool {
    (lower.starts_with("#!")
        && lower
            .lines()
            .next()
            .is_some_and(|line| line.contains("python")))
        && (lower.contains("from amplihack")
            || lower.contains("import amplihack")
            || lower.contains("load_entry_point")
            || lower.contains("amplihack.cli"))
}

fn is_stale_uvx_wrapper_lowercase(lower: &str) -> bool {
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
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn matches_known_path(path: &Path, original: &Path, canonical: &Path) -> bool {
    if path == original || path == canonical {
        return true;
    }
    path.canonicalize().is_ok_and(|path| path == canonical)
}

fn system_time_secs(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn now_secs() -> u64 {
    system_time_secs(SystemTime::now()).unwrap_or(0)
}
