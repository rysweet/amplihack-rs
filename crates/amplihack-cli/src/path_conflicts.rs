use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod target;
#[cfg(test)]
pub(crate) use target::BinaryProbe;
pub(crate) use target::{
    InstallTargetDecision, TargetDecisionInput, decide_update_install_target,
    default_denied_system_prefixes, probe_candidates_without_exec, update_path_conflict_notice,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathAnalysisInput {
    pub(crate) home_dir: PathBuf,
    pub(crate) current_exe: PathBuf,
    pub(crate) path_dirs: Vec<PathBuf>,
    pub(crate) binary_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BinaryCandidate {
    pub(crate) path: PathBuf,
    pub(crate) canonical_path: PathBuf,
    pub(crate) path_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BinaryResolution {
    pub(crate) resolved: BinaryCandidate,
    pub(crate) preferred_user_candidate: Option<BinaryCandidate>,
    pub(crate) canonical_candidates: Vec<BinaryCandidate>,
    pub(crate) is_shadowed_by_earlier_path_entry: bool,
    pub(crate) has_ambiguous_candidates: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathConflictReport {
    pub(crate) home_dir: PathBuf,
    pub(crate) current_exe: PathBuf,
    pub(crate) preferred_user_bin: PathBuf,
    resolutions: BTreeMap<String, BinaryResolution>,
}

impl PathConflictReport {
    pub(crate) fn resolution(&self, binary_name: &str) -> Option<&BinaryResolution> {
        self.resolutions.get(binary_name)
    }
}

pub(crate) fn analyze_path_conflicts(input: &PathAnalysisInput) -> Result<PathConflictReport> {
    let preferred_user_bin = preferred_user_bin(&input.home_dir);
    let mut resolutions = BTreeMap::new();

    for binary_name in &input.binary_names {
        let filename = binary_filename(binary_name);
        let preferred_binary_path = preferred_user_bin.join(&filename);
        let preferred_canonical_path = preferred_binary_path
            .canonicalize()
            .unwrap_or_else(|_| preferred_binary_path.clone());
        let candidates = path_candidates(&filename, &input.path_dirs)?;
        if candidates.is_empty() {
            continue;
        }

        let resolved = candidates[0].clone();
        let preferred_user_candidate = candidates
            .iter()
            .find(|candidate| {
                candidate.path == preferred_binary_path
                    || candidate.canonical_path == preferred_canonical_path
            })
            .cloned();
        let canonical_candidates = collapse_canonical_candidates(&candidates);
        let preferred_is_shadowed = preferred_user_candidate.as_ref().is_some_and(|preferred| {
            resolved.path_index < preferred.path_index
                && resolved.canonical_path != preferred.canonical_path
        });

        resolutions.insert(
            binary_name.clone(),
            BinaryResolution {
                resolved,
                preferred_user_candidate,
                has_ambiguous_candidates: canonical_candidates.len() > 1,
                canonical_candidates,
                is_shadowed_by_earlier_path_entry: preferred_is_shadowed,
            },
        );
    }

    Ok(PathConflictReport {
        home_dir: input.home_dir.clone(),
        current_exe: input.current_exe.clone(),
        preferred_user_bin,
        resolutions,
    })
}

pub(crate) fn analyze_current_process_path_conflicts(
    home_dir: PathBuf,
    current_exe: PathBuf,
) -> Result<PathConflictReport> {
    let path_dirs = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    analyze_path_conflicts(&PathAnalysisInput {
        home_dir,
        current_exe,
        path_dirs,
        binary_names: vec!["amplihack".to_string(), "amplihack-hooks".to_string()],
    })
}

pub(crate) fn preferred_user_bin(home_dir: &Path) -> PathBuf {
    home_dir.join(".local").join("bin")
}

pub(crate) fn binary_filename(name: &str) -> String {
    if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn path_candidates(filename: &str, path_dirs: &[PathBuf]) -> Result<Vec<BinaryCandidate>> {
    let mut candidates = Vec::with_capacity(path_dirs.len().min(4));

    for (path_index, dir) in path_dirs.iter().enumerate() {
        let path = dir.join(filename);
        if !is_executable_file(&path) {
            continue;
        }
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        candidates.push(BinaryCandidate {
            path,
            canonical_path,
            path_index,
        });
    }

    Ok(candidates)
}

fn collapse_canonical_candidates(candidates: &[BinaryCandidate]) -> Vec<BinaryCandidate> {
    let mut seen = BTreeSet::new();
    let mut collapsed = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if seen.insert(candidate.canonical_path.clone()) {
            collapsed.push(candidate.clone());
        }
    }
    collapsed
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
