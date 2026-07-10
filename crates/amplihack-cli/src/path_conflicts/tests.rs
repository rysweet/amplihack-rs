use super::{
    BinaryProbe, InstallTargetDecision, PathAnalysisInput, TargetDecisionInput,
    analyze_path_conflicts, decide_update_install_target, probe_candidates_without_exec,
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

fn analysis_input(home: &Path, current_exe: &Path, path_dirs: Vec<PathBuf>) -> PathAnalysisInput {
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
fn update_prefers_current_writable_user_bin_with_production_probes_over_shadowing_system_bin() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
    let usr_local_bin = temp.path().join("usr/local/bin");
    write_executable(&user_bin, "amplihack");
    write_executable(&user_bin, "amplihack-hooks");
    let system_amplihack = write_executable(&usr_local_bin, "amplihack");
    write_executable(&usr_local_bin, "amplihack-hooks");

    let report = analyze_path_conflicts(&analysis_input(
        &home,
        &system_amplihack,
        vec![usr_local_bin.clone(), user_bin.clone()],
    ))
    .unwrap();
    let probes = probe_candidates_without_exec(&report);

    let decision = decide_update_install_target(TargetDecisionInput {
        report,
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    })
    .expect("target selection should be pure");

    assert_eq!(
        decision,
        InstallTargetDecision::PreferredUserBin {
            install_dir: user_bin,
            reason: "user-level binaries are preferred over unsafe or shadowed PATH candidates"
                .into(),
        }
    );
}

#[test]
fn update_prefers_writable_user_bin_not_on_path_when_current_exe_is_denied() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
    let usr_local_bin = temp.path().join("usr/local/bin");
    write_executable(&user_bin, "amplihack");
    write_executable(&user_bin, "amplihack-hooks");
    let system_amplihack = write_executable(&usr_local_bin, "amplihack");
    write_executable(&usr_local_bin, "amplihack-hooks");

    let report = analyze_path_conflicts(&analysis_input(
        &home,
        &system_amplihack,
        vec![usr_local_bin.clone()],
    ))
    .unwrap();
    let probes = probe_candidates_without_exec(&report);

    let decision = decide_update_install_target(TargetDecisionInput {
        report,
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    })
    .expect("target selection should use the real non-exec probe contract");

    assert_eq!(
        decision,
        InstallTargetDecision::PreferredUserBin {
            install_dir: user_bin,
            reason: "user-level binaries are preferred over unsafe or shadowed PATH candidates"
                .into(),
        }
    );
}

#[test]
fn denied_system_candidate_repairs_to_user_bin_without_privileged_copy_attempt() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
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
    probes.insert(system_amplihack.clone(), BinaryProbe { writable: false });

    let decision = decide_update_install_target(TargetDecisionInput {
        report,
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    })
    .expect("target selection should be pure");

    assert_eq!(
        decision,
        InstallTargetDecision::PreferredUserBin {
            install_dir: user_bin,
            reason: "user-level binaries are preferred over unsafe or shadowed PATH candidates"
                .into(),
        },
        "denied system installs must repair by writing the user-level target, not by copying into /usr/local"
    );
}

#[test]
fn empty_user_bin_is_valid_repair_target_for_denied_system_install() {
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
    probes.insert(system_amplihack, BinaryProbe { writable: false });

    let decision = decide_update_install_target(TargetDecisionInput {
        report,
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    })
    .unwrap();

    assert_eq!(
        decision,
        InstallTargetDecision::PreferredUserBin {
            install_dir: user_bin,
            reason: "user-level binaries are preferred over unsafe or shadowed PATH candidates"
                .into(),
        },
        "a user-bin directory without existing binaries must be enough to redirect updates away from a denied system install"
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
    probes.insert(system_amplihack, BinaryProbe { writable: true });

    let decision = decide_update_install_target(TargetDecisionInput {
        report,
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    })
    .unwrap();

    assert!(
        matches!(decision, InstallTargetDecision::PreferredUserBin { .. }),
        "automatic updates must target user-level repair, not denied system prefixes, even when the current process can write there: {decision:?}"
    );
}

#[test]
fn update_notice_reports_shadowed_user_local_repair_without_permission_noise() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
    let usr_local_bin = temp.path().join("usr/local/bin");
    write_executable(&user_bin, "amplihack");
    write_executable(&user_bin, "amplihack-hooks");
    let system_amplihack = write_executable(&usr_local_bin, "amplihack");
    write_executable(&usr_local_bin, "amplihack-hooks");

    let report = analyze_path_conflicts(&analysis_input(
        &home,
        &system_amplihack,
        vec![usr_local_bin.clone(), user_bin.clone()],
    ))
    .unwrap();
    let probes = probe_candidates_without_exec(&report);

    let decision = decide_update_install_target(TargetDecisionInput {
        report: report.clone(),
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
    write_executable(&user_bin, "amplihack");
    write_executable(&user_bin, "amplihack-hooks");
    let system_amplihack = write_executable(&usr_local_bin, "amplihack");
    write_executable(&usr_local_bin, "amplihack-hooks");

    let report = analyze_path_conflicts(&analysis_input(
        &home,
        &system_amplihack,
        vec![usr_local_bin.clone()],
    ))
    .unwrap();
    let probes = probe_candidates_without_exec(&report);

    let decision = decide_update_install_target(TargetDecisionInput {
        report: report.clone(),
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
