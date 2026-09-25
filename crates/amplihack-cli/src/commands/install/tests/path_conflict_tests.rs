use super::helpers::create_exe_stub;
use super::*;
use crate::path_conflicts::{PathAnalysisInput, analyze_path_conflicts};
use std::fs;

#[test]
fn install_warns_when_user_level_amplihack_is_shadowed_by_earlier_path_entry() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    let system_bin = temp.path().join("usr/local/bin");
    let local_amplihack = create_exe_stub(&local_bin, "amplihack");
    create_exe_stub(&local_bin, "amplihack-hooks");
    let system_amplihack = create_exe_stub(&system_bin, "amplihack");
    create_exe_stub(&system_bin, "amplihack-hooks");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: temp.path().to_path_buf(),
        current_exe: local_amplihack.clone(),
        path_dirs: vec![system_bin.clone(), local_bin.clone()],
        binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
    })
    .unwrap();

    let warning = binary::path_conflict_warning_after_install(&report)
        .expect("shadowed user-level install should produce actionable warning text");

    crate::test_support::restore_home(previous);

    assert!(warning.contains("shadows"));
    assert!(warning.contains(&system_amplihack.display().to_string()));
    assert!(warning.contains(&local_amplihack.display().to_string()));
    assert!(warning.contains("export PATH=\"$HOME/.local/bin:$PATH\""));
    assert!(
        !warning.contains("Permission denied"),
        "install guidance must explain PATH order, not imply a failed privileged write"
    );
}

#[test]
fn install_warns_for_python_shadow_with_long_shebang_line() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    let system_bin = temp.path().join("usr/local/bin");
    let local_amplihack = create_exe_stub(&local_bin, "amplihack");
    create_exe_stub(&local_bin, "amplihack-hooks");
    let system_amplihack = create_exe_stub(&system_bin, "amplihack");
    create_exe_stub(&system_bin, "amplihack-hooks");
    fs::write(
        &system_amplihack,
        format!("#!{}python\nprint('shadow')\n", "/".repeat(300)),
    )
    .unwrap();

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: temp.path().to_path_buf(),
        current_exe: local_amplihack.clone(),
        path_dirs: vec![system_bin.clone(), local_bin.clone()],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    let warning = binary::path_conflict_warning_after_install(&report)
        .expect("shadowed Python script should produce Python-specific warning text");

    crate::test_support::restore_home(previous);

    assert!(warning.contains("A Python `amplihack` script"));
    assert!(warning.contains("pip uninstall amplihack"));
    assert!(warning.contains(&system_amplihack.display().to_string()));
}

#[test]
fn install_uses_generic_shadow_warning_for_large_non_python_binary() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    let system_bin = temp.path().join("usr/local/bin");
    let local_amplihack = create_exe_stub(&local_bin, "amplihack");
    create_exe_stub(&local_bin, "amplihack-hooks");
    let system_amplihack = create_exe_stub(&system_bin, "amplihack");
    create_exe_stub(&system_bin, "amplihack-hooks");
    fs::write(&system_amplihack, vec![b'x'; 2 * 1024 * 1024]).unwrap();

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: temp.path().to_path_buf(),
        current_exe: local_amplihack,
        path_dirs: vec![system_bin, local_bin],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    let warning = binary::path_conflict_warning_after_install(&report)
        .expect("shadowed non-Python binary should still produce generic warning text");

    crate::test_support::restore_home(previous);

    assert!(warning.contains("shadows the user-level binary"));
    assert!(!warning.contains("Python `amplihack` script"));
    assert!(!warning.contains("pip uninstall amplihack"));
}

#[test]
fn install_does_not_warn_when_path_aliases_resolve_to_same_user_binary() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    let alias_bin = temp.path().join("alias-bin");
    let local_amplihack = create_exe_stub(&local_bin, "amplihack");
    create_exe_stub(&local_bin, "amplihack-hooks");
    fs::create_dir_all(&alias_bin).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&local_amplihack, alias_bin.join("amplihack")).unwrap();
        std::os::unix::fs::symlink(
            local_bin.join("amplihack-hooks"),
            alias_bin.join("amplihack-hooks"),
        )
        .unwrap();
    }

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: temp.path().to_path_buf(),
        current_exe: local_amplihack,
        path_dirs: vec![alias_bin, local_bin],
        binary_names: vec!["amplihack".into(), "amplihack-hooks".into()],
    })
    .unwrap();

    let warning = binary::path_conflict_warning_after_install(&report);

    crate::test_support::restore_home(previous);

    assert!(
        warning.is_none(),
        "canonical symlink aliases to the same user-level binaries must not create noisy install warnings"
    );
}

#[test]
fn install_warns_when_user_level_hooks_binary_is_shadowed() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    let system_bin = temp.path().join("usr/local/bin");
    create_exe_stub(&local_bin, "amplihack");
    let local_hooks = create_exe_stub(&local_bin, "amplihack-hooks");
    create_exe_stub(&system_bin, "amplihack");
    let system_hooks = create_exe_stub(&system_bin, "amplihack-hooks");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: temp.path().to_path_buf(),
        current_exe: local_bin.join("amplihack"),
        path_dirs: vec![system_bin.clone(), local_bin.clone()],
        binary_names: vec!["amplihack-hooks".into()],
    })
    .unwrap();

    let warning = binary::path_conflict_warning_after_install(&report)
        .expect("shadowed hooks binary should produce actionable warning text");

    crate::test_support::restore_home(previous);

    assert!(warning.contains("amplihack-hooks"));
    assert!(warning.contains(&system_hooks.display().to_string()));
    assert!(warning.contains(&local_hooks.display().to_string()));
    assert!(warning.contains("export PATH=\"$HOME/.local/bin:$PATH\""));
}

#[test]
fn install_warns_when_multiple_distinct_binary_candidates_create_ambiguity() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local/bin");
    let other_bin = temp.path().join("other/bin");
    let local_amplihack = create_exe_stub(&local_bin, "amplihack");
    let other_amplihack = create_exe_stub(&other_bin, "amplihack");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: temp.path().to_path_buf(),
        current_exe: local_amplihack.clone(),
        path_dirs: vec![local_bin, other_bin],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    let warning = binary::path_conflict_warning_after_install(&report)
        .expect("distinct duplicate candidates should produce ambiguity guidance");

    crate::test_support::restore_home(previous);

    assert!(warning.contains("Multiple distinct `amplihack` binaries"));
    assert!(warning.contains(&local_amplihack.display().to_string()));
    assert!(warning.contains(&other_amplihack.display().to_string()));
    assert!(warning.contains("Remove stale candidates or reorder PATH"));
}
