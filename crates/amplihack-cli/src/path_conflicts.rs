use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BinaryProbe {
    pub(crate) version: Option<String>,
    pub(crate) writable: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TargetDecisionInput {
    pub(crate) report: PathConflictReport,
    pub(crate) current_version: String,
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
    let preferred_user_bin_denied =
        is_under_denied_prefix(&input.report.preferred_user_bin, &denied_system_prefixes);
    let current_exe_writable = input
        .candidate_probes
        .get(&input.report.current_exe)
        .map(|probe| probe.writable)
        .unwrap_or_else(|| path_is_writable(&input.report.current_exe));

    if !preferred_user_bin_denied && preferred_user_bin_is_safe_current(&input) {
        return Ok(InstallTargetDecision::PreferredUserBin {
            install_dir: input.report.preferred_user_bin,
            reason: "current writable user-level binaries are preferred for updates".to_string(),
        });
    }

    if input.report.preferred_user_bin.exists()
        && !preferred_user_bin_denied
        && user_bin_pair_exists_and_is_writable(
            &input.report.preferred_user_bin,
            &input.candidate_probes,
        )
        && (current_exe_denied || report_has_shadowing(&input.report))
    {
        return Ok(InstallTargetDecision::PreferredUserBin {
            install_dir: input.report.preferred_user_bin,
            reason: "writable user-level binaries are preferred over unsafe system PATH candidates"
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
        version: None,
        writable: path_is_writable(path),
    }
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

fn preferred_user_bin_is_safe_current(input: &TargetDecisionInput) -> bool {
    ["amplihack", "amplihack-hooks"].into_iter().all(|name| {
        let path = input.report.preferred_user_bin.join(binary_filename(name));
        input.candidate_probes.get(&path).is_some_and(|probe| {
            probe.writable && probe.version.as_deref() == Some(&input.current_version)
        })
    })
}

fn user_bin_pair_exists_and_is_writable(
    user_bin: &Path,
    probes: &BTreeMap<PathBuf, BinaryProbe>,
) -> bool {
    ["amplihack", "amplihack-hooks"].into_iter().all(|name| {
        let path = user_bin.join(binary_filename(name));
        if !path.is_file() {
            return false;
        }
        probes
            .get(&path)
            .map(|probe| probe.writable)
            .unwrap_or_else(|| path_is_writable(&path))
    })
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

fn path_is_writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let target = if path.exists() {
            path
        } else if let Some(parent) = path.parent() {
            parent
        } else {
            return false;
        };
        let Ok(c_path) = CString::new(target.as_os_str().as_bytes()) else {
            return false;
        };
        unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
    }

    #[cfg(not(unix))]
    {
        if path.exists() {
            fs::OpenOptions::new().write(true).open(path).is_ok()
        } else {
            path.parent()
                .is_some_and(|parent| fs::metadata(parent).is_ok())
        }
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

#[cfg(test)]
mod tests {
    use super::{
        BinaryProbe, InstallTargetDecision, PathAnalysisInput, TargetDecisionInput,
        analyze_path_conflicts, decide_update_install_target,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn write_executable(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn analysis_input(
        home: &Path,
        current_exe: &Path,
        path_dirs: Vec<PathBuf>,
    ) -> PathAnalysisInput {
        PathAnalysisInput {
            home_dir: home.to_path_buf(),
            current_exe: current_exe.to_path_buf(),
            path_dirs,
            binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
        }
    }

    #[test]
    fn detects_usr_local_shadowing_user_bin_from_path_order_not_current_exe() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let usr_local_bin = temp.path().join("usr/local/bin");

        let user_amplihack = write_executable(&user_bin, "amplihack");
        write_executable(&user_bin, "amplihack-hooks");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        let system_hooks = write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &user_amplihack,
            vec![usr_local_bin.clone(), user_bin.clone()],
        ))
        .expect("PATH analysis should be pure and deterministic");

        let amplihack = report
            .resolution("amplihack")
            .expect("amplihack should have a resolution");
        assert_eq!(amplihack.resolved.path, system_amplihack);
        assert_eq!(amplihack.resolved.path_index, 0);
        assert_eq!(
            amplihack.preferred_user_candidate.as_ref().map(|c| &c.path),
            Some(&user_amplihack)
        );
        assert!(
            amplihack.is_shadowed_by_earlier_path_entry,
            "PATH conflicts must be based on command resolution order, not current_exe()"
        );

        let hooks = report
            .resolution("amplihack-hooks")
            .expect("hooks should have a resolution");
        assert_eq!(hooks.resolved.path, system_hooks);
        assert!(
            hooks.is_shadowed_by_earlier_path_entry,
            "amplihack-hooks shadowing must be detected independently"
        );
    }

    #[test]
    fn canonical_duplicate_detection_does_not_treat_symlink_alias_as_ambiguity() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let aliases = temp.path().join("aliases");
        let user_amplihack = write_executable(&user_bin, "amplihack");

        fs::create_dir_all(&aliases).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&user_amplihack, aliases.join("amplihack")).unwrap();

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &user_amplihack,
            vec![user_bin.clone(), aliases.clone()],
        ))
        .expect("PATH analysis should tolerate symlink aliases");

        let amplihack = report.resolution("amplihack").unwrap();
        assert_eq!(
            amplihack.canonical_candidates.len(),
            1,
            "raw PATH aliases to the same canonical file must collapse to one candidate"
        );
        assert!(
            !amplihack.has_ambiguous_candidates,
            "a symlink alias to the same binary is not an ambiguous duplicate"
        );
    }

    #[cfg(unix)]
    #[test]
    fn alias_to_user_bin_is_treated_as_preferred_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let aliases = temp.path().join("aliases");
        let user_amplihack = write_executable(&user_bin, "amplihack");

        fs::create_dir_all(&aliases).unwrap();
        std::os::unix::fs::symlink(&user_amplihack, aliases.join("amplihack")).unwrap();

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &user_amplihack,
            vec![aliases.clone()],
        ))
        .unwrap();

        let amplihack = report.resolution("amplihack").unwrap();
        assert_eq!(
            amplihack.preferred_user_candidate.as_ref().map(|c| &c.path),
            Some(&aliases.join("amplihack")),
            "PATH aliases that canonicalize to ~/.local/bin must count as the preferred user binary"
        );
        assert!(
            !amplihack.is_shadowed_by_earlier_path_entry,
            "a PATH alias that resolves to the user binary should not produce repair noise"
        );
    }

    #[cfg(unix)]
    #[test]
    fn system_binary_shadowing_alias_to_user_bin_is_reported_as_shadowing() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let aliases = temp.path().join("aliases");
        let usr_local_bin = temp.path().join("usr/local/bin");
        let user_amplihack = write_executable(&user_bin, "amplihack");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");

        fs::create_dir_all(&aliases).unwrap();
        std::os::unix::fs::symlink(&user_amplihack, aliases.join("amplihack")).unwrap();

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone(), aliases.clone()],
        ))
        .unwrap();

        let amplihack = report.resolution("amplihack").unwrap();
        assert_eq!(amplihack.resolved.path, system_amplihack);
        assert!(
            amplihack.is_shadowed_by_earlier_path_entry,
            "system binaries before a PATH alias to ~/.local/bin must still be reported as shadowing"
        );
    }

    #[test]
    fn distinct_duplicate_candidates_are_reported_as_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let other_bin = temp.path().join("other/bin");
        let user_amplihack = write_executable(&user_bin, "amplihack");
        let other_amplihack = write_executable(&other_bin, "amplihack");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &user_amplihack,
            vec![user_bin.clone(), other_bin],
        ))
        .expect("PATH analysis should find all candidates");

        let amplihack = report.resolution("amplihack").unwrap();
        assert_eq!(amplihack.canonical_candidates.len(), 2);
        assert!(
            amplihack.has_ambiguous_candidates,
            "two different executable candidates for amplihack must be surfaced as ambiguous"
        );
        assert!(
            amplihack
                .canonical_candidates
                .iter()
                .any(|candidate| candidate.path == other_amplihack)
        );
    }

    #[test]
    fn update_prefers_current_writable_user_bin_over_unwritable_shadowing_system_bin() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let usr_local_bin = temp.path().join("usr/local/bin");
        let user_amplihack = write_executable(&user_bin, "amplihack");
        let user_hooks = write_executable(&user_bin, "amplihack-hooks");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone(), user_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        probes.insert(
            user_amplihack.clone(),
            BinaryProbe {
                version: Some("0.9.71".into()),
                writable: true,
            },
        );
        probes.insert(
            user_hooks.clone(),
            BinaryProbe {
                version: Some("0.9.71".into()),
                writable: true,
            },
        );
        probes.insert(
            system_amplihack.clone(),
            BinaryProbe {
                version: Some("0.9.60".into()),
                writable: false,
            },
        );

        let decision = decide_update_install_target(TargetDecisionInput {
            report,
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .expect("target selection should be pure");

        assert_eq!(
            decision,
            InstallTargetDecision::PreferredUserBin {
                install_dir: user_bin,
                reason: "current writable user-level binaries are preferred for updates".into(),
            }
        );
    }

    #[test]
    fn root_owned_system_candidate_requires_manual_repair_without_privileged_copy_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let usr_local_bin = temp.path().join("usr/local/bin");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        probes.insert(
            system_amplihack.clone(),
            BinaryProbe {
                version: Some("0.9.60".into()),
                writable: false,
            },
        );

        let decision = decide_update_install_target(TargetDecisionInput {
            report,
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .expect("manual repair is a valid decision, not an IO error");

        let InstallTargetDecision::ManualRepairRequired { guidance, .. } = decision else {
            panic!("unwritable denied system install must require manual repair, got {decision:?}");
        };
        assert!(guidance.contains("sudo"));
        assert!(guidance.contains("/usr/local/bin") || guidance.contains("usr/local/bin"));
        assert!(guidance.contains("~/.local/bin") || guidance.contains(".local/bin"));
        assert!(
            !guidance.contains("Permission denied copying"),
            "guidance must avoid the old misleading temp-copy failure wording: {guidance}"
        );
    }

    #[test]
    fn writable_empty_user_bin_does_not_silently_repair_denied_system_install() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let usr_local_bin = temp.path().join("usr/local/bin");
        fs::create_dir_all(&user_bin).unwrap();
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        probes.insert(
            system_amplihack,
            BinaryProbe {
                version: Some("0.9.60".into()),
                writable: false,
            },
        );

        let decision = decide_update_install_target(TargetDecisionInput {
            report,
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .unwrap();

        assert!(
            matches!(decision, InstallTargetDecision::ManualRepairRequired { .. }),
            "a writable user-bin directory without existing user-level binaries is not enough to redirect updates away from a denied system install: {decision:?}"
        );
    }

    #[test]
    fn denied_system_prefix_is_never_selected_even_when_probe_says_writable() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let usr_local_bin = temp.path().join("usr/local/bin");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        probes.insert(
            system_amplihack,
            BinaryProbe {
                version: Some("0.9.71".into()),
                writable: true,
            },
        );

        let decision = decide_update_install_target(TargetDecisionInput {
            report,
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .unwrap();

        assert!(
            matches!(decision, InstallTargetDecision::ManualRepairRequired { .. }),
            "automatic updates must not target denied system prefixes even when the current process can write there: {decision:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_user_bin_symlink_to_denied_prefix_requires_manual_repair() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let local_parent = home.join(".local");
        let preferred_user_bin = home.join(".local/bin");
        let usr_local_bin = temp.path().join("usr/local/bin");
        fs::create_dir_all(&local_parent).unwrap();
        fs::create_dir_all(&usr_local_bin).unwrap();
        std::os::unix::fs::symlink(&usr_local_bin, &preferred_user_bin).unwrap();
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        let system_hooks = write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![preferred_user_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        for path in [
            preferred_user_bin.join("amplihack"),
            preferred_user_bin.join("amplihack-hooks"),
        ] {
            probes.insert(
                path,
                BinaryProbe {
                    version: Some("0.9.71".into()),
                    writable: true,
                },
            );
        }
        probes.insert(
            system_amplihack,
            BinaryProbe {
                version: Some("0.9.71".into()),
                writable: true,
            },
        );
        probes.insert(
            system_hooks,
            BinaryProbe {
                version: Some("0.9.71".into()),
                writable: true,
            },
        );

        let decision = decide_update_install_target(TargetDecisionInput {
            report,
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .unwrap();

        assert!(
            matches!(decision, InstallTargetDecision::ManualRepairRequired { .. }),
            "preferred user-bin paths that canonicalize into denied prefixes must not be selected: {decision:?}"
        );
    }

    #[test]
    fn update_notice_reports_shadowed_user_local_repair_without_permission_noise() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let usr_local_bin = temp.path().join("usr/local/bin");
        let user_amplihack = write_executable(&user_bin, "amplihack");
        let user_hooks = write_executable(&user_bin, "amplihack-hooks");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone(), user_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        for path in [user_amplihack, user_hooks] {
            probes.insert(
                path,
                BinaryProbe {
                    version: Some("0.9.71".into()),
                    writable: true,
                },
            );
        }

        let decision = decide_update_install_target(TargetDecisionInput {
            report: report.clone(),
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .unwrap();

        let notice = super::update_path_conflict_notice(&report, &decision)
            .expect("shadowed user-local repair should produce update guidance");
        assert!(notice.contains("PATH conflict"));
        assert!(notice.contains(&usr_local_bin.join("amplihack").display().to_string()));
        assert!(notice.contains(&user_bin.join("amplihack").display().to_string()));
        assert!(notice.contains("Updating user-local targets under"));
        assert!(notice.contains("sudo"));
        assert!(
            !notice.contains("Permission denied copying") && !notice.contains("failed to copy"),
            "repair guidance must replace temp-copy permission failures, got: {notice}"
        );
    }

    #[test]
    fn update_notice_reports_user_local_target_missing_from_path_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        let usr_local_bin = temp.path().join("usr/local/bin");
        let user_amplihack = write_executable(&user_bin, "amplihack");
        let user_hooks = write_executable(&user_bin, "amplihack-hooks");
        let system_amplihack = write_executable(&usr_local_bin, "amplihack");
        write_executable(&usr_local_bin, "amplihack-hooks");

        let report = analyze_path_conflicts(&analysis_input(
            &home,
            &system_amplihack,
            vec![usr_local_bin.clone()],
        ))
        .unwrap();
        let mut probes = BTreeMap::new();
        for path in [user_amplihack, user_hooks] {
            probes.insert(
                path,
                BinaryProbe {
                    version: Some("0.9.60".into()),
                    writable: true,
                },
            );
        }
        probes.insert(
            system_amplihack,
            BinaryProbe {
                version: Some("0.9.60".into()),
                writable: false,
            },
        );

        let decision = decide_update_install_target(TargetDecisionInput {
            report: report.clone(),
            current_version: "0.9.71".into(),
            candidate_probes: probes,
            denied_system_prefixes: vec![temp.path().join("usr/local")],
        })
        .unwrap();

        let notice = super::update_path_conflict_notice(&report, &decision)
            .expect("updating a user-local target that is not on PATH must produce guidance");
        assert!(notice.contains("instead of the user-local target"));
        assert!(notice.contains(&usr_local_bin.join("amplihack").display().to_string()));
        assert!(notice.contains(&user_bin.join("amplihack").display().to_string()));
        assert!(notice.contains("earlier in PATH"));
        assert!(
            !notice.contains("Permission denied copying") && !notice.contains("failed to copy"),
            "PATH guidance must not regress to copy-failure noise, got: {notice}"
        );
    }
}
