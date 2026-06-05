use super::super::install::{
    BinaryInstallPlan, InstallArchiveLayout, plan_downloaded_binary_install,
    probe_candidates_for_update_decision,
};
use crate::path_conflicts::{
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

#[test]
fn production_probe_prefers_byte_identical_current_user_bin_even_without_path_shadowing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
    let other_bin = temp.path().join("other/bin");
    let current_exe = write_executable(&other_bin, "amplihack");
    let current_hooks = write_executable(&other_bin, "amplihack-hooks");
    fs::create_dir_all(&user_bin).unwrap();
    fs::copy(&current_exe, user_bin.join("amplihack")).unwrap();
    fs::copy(&current_hooks, user_bin.join("amplihack-hooks")).unwrap();

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home,
        current_exe,
        path_dirs: vec![other_bin],
        binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
    })
    .unwrap();
    let probes = probe_candidates_for_update_decision(&report, "0.9.71");

    let decision = decide_update_install_target(TargetDecisionInput {
        report,
        current_version: "0.9.71".into(),
        candidate_probes: probes,
        denied_system_prefixes: Vec::new(),
    })
    .unwrap();

    assert_eq!(
        decision,
        InstallTargetDecision::PreferredUserBin {
            install_dir: user_bin,
            reason: "current writable user-level binaries are preferred for updates".into(),
        }
    );
}

#[cfg(unix)]
#[test]
fn production_probe_does_not_mark_symlinked_user_bin_current() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
    let other_bin = temp.path().join("other/bin");
    let current_exe = write_executable(&other_bin, "amplihack");
    let current_hooks = write_executable(&other_bin, "amplihack-hooks");
    fs::create_dir_all(&user_bin).unwrap();
    std::os::unix::fs::symlink(&current_exe, user_bin.join("amplihack")).unwrap();
    std::os::unix::fs::symlink(&current_hooks, user_bin.join("amplihack-hooks")).unwrap();

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home,
        current_exe,
        path_dirs: vec![other_bin.clone()],
        binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
    })
    .unwrap();
    let probes = probe_candidates_for_update_decision(&report, "0.9.71");

    assert_eq!(
        probes
            .get(&user_bin.join("amplihack"))
            .and_then(|probe| probe.version.as_deref()),
        None,
        "preferred user-bin symlinks must not be treated as verified current binaries"
    );
}

#[test]
fn downloaded_update_plan_uses_preferred_user_bin_when_system_binary_shadows_current_user_install()
{
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_bin = home.join(".local/bin");
    let usr_local_bin = temp.path().join("usr/local/bin");
    let archive = temp.path().join("archive");

    let current_exe = write_executable(&usr_local_bin, "amplihack");
    let user_amplihack = write_executable(&user_bin, "amplihack");
    let user_hooks = write_executable(&user_bin, "amplihack-hooks");
    let new_amplihack = write_executable(&archive, "amplihack");
    let new_hooks = write_executable(&archive, "amplihack-hooks");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home,
        current_exe: current_exe.clone(),
        path_dirs: vec![usr_local_bin.clone(), user_bin.clone()],
        binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
    })
    .unwrap();
    let mut probes = BTreeMap::new();
    probes.insert(
        current_exe,
        BinaryProbe {
            version: Some("0.9.60".into()),
            writable: false,
        },
    );
    probes.insert(
        user_amplihack.clone(),
        BinaryProbe {
            version: Some("0.9.71".into()),
            writable: true,
        },
    );
    probes.insert(
        user_hooks,
        BinaryProbe {
            version: Some("0.9.71".into()),
            writable: true,
        },
    );

    let decision = InstallTargetDecision::from(TargetDecisionInput {
        report,
        current_version: "0.9.71".into(),
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    });

    let plan = plan_downloaded_binary_install(
        InstallArchiveLayout {
            amplihack: new_amplihack,
            hooks: new_hooks,
        },
        decision,
    )
    .expect("safe user-level target should produce an install plan");

    assert_eq!(
        plan,
        BinaryInstallPlan {
            amplihack_destination: user_bin.join("amplihack"),
            hooks_destination: user_bin.join("amplihack-hooks"),
        }
    );
}

#[test]
fn downloaded_update_plan_returns_manual_repair_before_copying_into_denied_system_dir() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let usr_local_bin = temp.path().join("usr/local/bin");
    let archive = temp.path().join("archive");

    let current_exe = write_executable(&usr_local_bin, "amplihack");
    write_executable(&usr_local_bin, "amplihack-hooks");
    let new_amplihack = write_executable(&archive, "amplihack");
    let new_hooks = write_executable(&archive, "amplihack-hooks");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home,
        current_exe: current_exe.clone(),
        path_dirs: vec![usr_local_bin.clone()],
        binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
    })
    .unwrap();
    let mut probes = BTreeMap::new();
    probes.insert(
        current_exe,
        BinaryProbe {
            version: Some("0.9.60".into()),
            writable: false,
        },
    );

    let decision = InstallTargetDecision::from(TargetDecisionInput {
        report,
        current_version: "0.9.71".into(),
        candidate_probes: probes,
        denied_system_prefixes: vec![temp.path().join("usr/local")],
    });

    let err = plan_downloaded_binary_install(
        InstallArchiveLayout {
            amplihack: new_amplihack,
            hooks: new_hooks,
        },
        decision,
    )
    .expect_err("denied system target must not produce a copy plan");

    let message = err.to_string();
    assert!(message.contains("manual repair"));
    assert!(message.contains("sudo"));
    assert!(
        !message.contains(".tmp") && !message.contains("Permission denied"),
        "update must fail before temp-copying into /usr/local/bin, got: {message}"
    );
}
