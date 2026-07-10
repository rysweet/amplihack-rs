use super::{PathConflictReport, binary_filename};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(unix))]
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BinaryProbe {
    pub(crate) writable: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TargetDecisionInput {
    pub(crate) report: PathConflictReport,
    pub(crate) candidate_probes: BTreeMap<PathBuf, BinaryProbe>,
    pub(crate) denied_system_prefixes: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallTargetDecision {
    CurrentExeDir {
        install_dir: PathBuf,
        reason: String,
    },
    PreferredUserBin {
        install_dir: PathBuf,
        reason: String,
    },
    ManualRepairRequired {
        guidance: String,
        conflicts: Vec<PathBuf>,
    },
}

impl From<TargetDecisionInput> for InstallTargetDecision {
    fn from(input: TargetDecisionInput) -> Self {
        match decide_update_install_target(input) {
            Ok(decision) => decision,
            Err(err) => InstallTargetDecision::ManualRepairRequired {
                guidance: format!("manual repair required before update can continue: {err}"),
                conflicts: Vec::new(),
            },
        }
    }
}

pub(crate) fn update_path_conflict_notice(
    report: &PathConflictReport,
    decision: &InstallTargetDecision,
) -> Option<String> {
    let InstallTargetDecision::PreferredUserBin { install_dir, .. } = decision else {
        return None;
    };

    let mut notice = String::new();
    for (binary_name, resolution) in &report.resolutions {
        if !resolution.is_shadowed_by_earlier_path_entry {
            let user_target = install_dir.join(binary_filename(binary_name));
            if resolution.resolved.path != user_target
                && resolution.preferred_user_candidate.is_none()
            {
                notice.push_str(&format!(
                    "  ⚠️  PATH conflict: `{binary_name}` resolves to {} instead of the user-local target {}\n",
                    resolution.resolved.path.display(),
                    user_target.display()
                ));
                notice.push_str(
                    "     The updated binary may not run until the user-local install is earlier in PATH.\n",
                );
            }
            continue;
        }
        let Some(preferred) = resolution.preferred_user_candidate.as_ref() else {
            continue;
        };
        notice.push_str(&format!(
            "  ⚠️  PATH conflict: {} appears before {}\n",
            resolution.resolved.path.display(),
            preferred.path.display()
        ));
        notice.push_str(&format!(
            "     `{binary_name}` may continue to resolve to the stale earlier PATH candidate until PATH is repaired.\n"
        ));
    }

    if notice.is_empty() {
        return None;
    }

    notice.push_str(&format!(
        "     Updating user-local targets under: {}\n",
        install_dir.display()
    ));
    notice.push_str(
        "     To run the updated binaries first, move ~/.local/bin earlier in PATH or remove stale system copies with sudo.",
    );
    Some(notice)
}

pub(crate) fn decide_update_install_target(
    input: TargetDecisionInput,
) -> Result<InstallTargetDecision> {
    let current_exe_dir = input
        .report
        .current_exe
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("current executable has no parent directory"))?;
    let denied_system_prefixes = canonical_denied_prefixes(&input.denied_system_prefixes);
    let current_exe_denied =
        is_under_denied_prefix(&input.report.current_exe, &denied_system_prefixes);
    let current_exe_writable = input
        .candidate_probes
        .get(&input.report.current_exe)
        .map(|probe| probe.writable)
        .unwrap_or_else(|| path_is_writable(&input.report.current_exe));
    let preferred_user_bin_writable = path_is_writable(&input.report.preferred_user_bin);

    if !is_under_denied_prefix(&input.report.preferred_user_bin, &denied_system_prefixes)
        && preferred_user_bin_writable
        && (current_exe_denied || report_has_shadowing(&input.report))
    {
        return Ok(InstallTargetDecision::PreferredUserBin {
            install_dir: input.report.preferred_user_bin,
            reason: "user-level binaries are preferred over unsafe or shadowed PATH candidates"
                .to_string(),
        });
    }

    if !current_exe_denied && current_exe_writable {
        return Ok(InstallTargetDecision::CurrentExeDir {
            install_dir: current_exe_dir,
            reason: "current executable directory is writable and not system-managed".to_string(),
        });
    }

    let conflicts = conflict_paths(&input.report);
    Ok(InstallTargetDecision::ManualRepairRequired {
        guidance: manual_repair_guidance(
            &input.report.preferred_user_bin,
            &input.report.current_exe,
            &conflicts,
            current_exe_denied,
        ),
        conflicts,
    })
}

pub(crate) fn default_denied_system_prefixes() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        vec![
            PathBuf::from("/usr/local"),
            PathBuf::from("/usr"),
            PathBuf::from("/bin"),
            PathBuf::from("/sbin"),
            PathBuf::from("/opt"),
        ]
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

pub(crate) fn binary_probe_without_exec(path: &Path) -> BinaryProbe {
    BinaryProbe {
        writable: path_is_writable(path),
    }
}

pub(crate) fn probe_candidates_without_exec(
    report: &PathConflictReport,
) -> BTreeMap<PathBuf, BinaryProbe> {
    let mut probes = BTreeMap::new();
    probes.insert(
        report.current_exe.clone(),
        binary_probe_without_exec(&report.current_exe),
    );
    for resolution in report.resolutions.values() {
        for candidate in &resolution.canonical_candidates {
            probes
                .entry(candidate.path.clone())
                .or_insert_with(|| binary_probe_without_exec(&candidate.path));
        }
    }
    for name in ["amplihack", "amplihack-hooks"] {
        let path = report.preferred_user_bin.join(binary_filename(name));
        probes
            .entry(path.clone())
            .or_insert_with(|| binary_probe_without_exec(&path));
    }
    probes
}

fn report_has_shadowing(report: &PathConflictReport) -> bool {
    report
        .resolutions
        .values()
        .any(|resolution| resolution.is_shadowed_by_earlier_path_entry)
}

fn conflict_paths(report: &PathConflictReport) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for resolution in report.resolutions.values() {
        if resolution.is_shadowed_by_earlier_path_entry || resolution.has_ambiguous_candidates {
            for candidate in &resolution.canonical_candidates {
                paths.insert(candidate.path.clone());
            }
        }
    }
    if paths.is_empty() {
        paths.insert(report.current_exe.clone());
    }
    paths.into_iter().collect()
}

fn manual_repair_guidance(
    preferred_user_bin: &Path,
    current_exe: &Path,
    conflicts: &[PathBuf],
    current_exe_denied: bool,
) -> String {
    let mut guidance = String::new();
    guidance.push_str("manual repair required: amplihack will not write to privileged system locations automatically.\n");
    if current_exe_denied {
        guidance.push_str(&format!(
            "The running amplihack binary is in a system-managed location: {}\n",
            current_exe.display()
        ));
    }
    if !conflicts.is_empty() {
        guidance.push_str("Conflicting PATH candidates:\n");
        for conflict in conflicts {
            guidance.push_str(&format!("  - {}\n", conflict.display()));
        }
    }
    guidance.push_str(&format!(
        "Use the user-level binaries in {} by moving `$HOME/.local/bin` earlier in PATH:\n  export PATH=\"$HOME/.local/bin:$PATH\"\n",
        preferred_user_bin.display()
    ));
    guidance.push_str("If an older /usr/local/bin or other system binary shadows the user install, remove or rename it manually, for example:\n  sudo rm /usr/local/bin/amplihack /usr/local/bin/amplihack-hooks\n");
    guidance
}

fn canonical_denied_prefixes(prefixes: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    prefixes
        .iter()
        .map(|prefix| {
            let canonical = prefix.canonicalize().unwrap_or_else(|_| prefix.clone());
            (prefix.clone(), canonical)
        })
        .collect()
}

fn is_under_denied_prefix(path: &Path, prefixes: &[(PathBuf, PathBuf)]) -> bool {
    if prefixes.iter().any(|(prefix, _)| path.starts_with(prefix)) {
        return true;
    }

    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    prefixes
        .iter()
        .any(|(_, canonical_prefix)| canonical_path.starts_with(canonical_prefix))
}

fn path_is_writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        let target = nearest_existing_path(path);
        return is_writable_by_current_user(target);
    }

    #[cfg(not(unix))]
    {
        return if path.exists() {
            fs::OpenOptions::new().write(true).open(path).is_ok()
        } else {
            path.parent()
                .is_some_and(|parent| fs::metadata(parent).is_ok())
        };
    }

    #[cfg(unix)]
    fn is_writable_by_current_user(target: &Path) -> bool {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let Ok(c_path) = CString::new(target.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: libc::access only reads the NUL-terminated path pointer for the
        // duration of the call; `c_path` is alive and immutable for that duration.
        unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
    }

    #[cfg(unix)]
    fn nearest_existing_path(path: &Path) -> &Path {
        let mut target = path;
        while !target.exists() {
            let Some(parent) = target.parent() else {
                break;
            };
            target = parent;
        }
        target
    }
}
