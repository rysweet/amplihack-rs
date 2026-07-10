use super::super::stale_wrappers::{
    NeutralizedWrapperKind, StaleWrapperNeutralizerConfig, StaleWrapperRepairError,
    neutralize_shadowing_stale_wrappers,
};
use super::helpers::create_exe_stub;
use std::fs;
use std::path::{Path, PathBuf};

fn write_executable(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn repair_config(
    home: &Path,
    current_rust: &Path,
    preferred_rust: &Path,
    path_dirs: Vec<PathBuf>,
) -> StaleWrapperNeutralizerConfig {
    StaleWrapperNeutralizerConfig {
        home_dir: home.to_path_buf(),
        current_exe: current_rust.to_path_buf(),
        preferred_rust_binary: preferred_rust.to_path_buf(),
        path_dirs,
        binary_name: "amplihack".to_string(),
    }
}

#[test]
fn quarantines_shadowing_python_entrypoint_wrapper_and_records_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let uv_tools_bin = home.join(".local/share/uv/tools/amplihack/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let stale_wrapper = uv_tools_bin.join("amplihack");
    write_executable(
        &stale_wrapper,
        "#!/usr/bin/env python3\n# -*- coding: utf-8 -*-\nimport sys\nfrom amplihack.cli import main\nsys.exit(main())\n",
    );

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![uv_tools_bin.clone(), preferred_bin.clone()],
    ))
    .expect("positively identified stale Python wrappers in user-controlled uv locations should be quarantined");

    assert!(
        !stale_wrapper.exists(),
        "shadowing stale wrapper must no longer exist at its original PATH location"
    );
    assert_eq!(report.resolved_after, preferred_rust);
    assert_eq!(report.neutralized.len(), 1);
    assert_eq!(
        report.neutralized[0].kind,
        NeutralizedWrapperKind::StalePythonWrapper
    );
    assert!(report.neutralized[0].quarantine_path.is_file());
    assert!(
        report.neutralized[0]
            .quarantine_path
            .starts_with(home.join(".amplihack/quarantine/stale-wrappers")),
        "quarantine path must remain under ~/.amplihack/quarantine/stale-wrappers, got {}",
        report.neutralized[0].quarantine_path.display()
    );
    let manifest = fs::read_to_string(&report.manifest_path).unwrap();
    assert!(manifest.contains(&stale_wrapper.display().to_string()));
    assert!(manifest.contains("stale-python-wrapper"));
    assert!(manifest.contains("quarantined"));
}

#[test]
fn quarantines_shadowing_uvx_script_wrapper_without_touching_rust_binary() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let uvx_bin = home.join(".cache/uv/archive-v0/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let stale_wrapper = uvx_bin.join("amplihack");
    write_executable(
        &stale_wrapper,
        "#!/bin/sh\n# uvx generated shim\nexec uvx --from amplihack amplihack \"$@\"\n",
    );

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![uvx_bin, preferred_bin.clone()],
    ))
    .expect("positively identified stale uvx wrappers should be quarantined");

    assert!(!stale_wrapper.exists());
    assert!(
        preferred_rust.exists(),
        "neutralizing stale wrappers must never move the preferred Rust binary"
    );
    assert_eq!(report.neutralized.len(), 1);
    assert_eq!(
        report.neutralized[0].kind,
        NeutralizedWrapperKind::StaleUvxWrapper
    );
}

#[test]
fn unknown_shadowing_executable_is_reported_and_not_modified() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let other_bin = home.join("bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let unknown = other_bin.join("amplihack");
    write_executable(&unknown, "#!/bin/sh\necho unrelated-user-tool\n");
    let before = fs::read(&unknown).unwrap();

    let err = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![other_bin, preferred_bin],
    ))
    .expect_err("unknown shadowing executables must block repair instead of being deleted");

    match err {
        StaleWrapperRepairError::UnknownShadowingExecutable { path, .. } => {
            assert_eq!(path, unknown);
        }
        other => panic!("expected UnknownShadowingExecutable, got {other:?}"),
    }
    assert_eq!(
        fs::read(&unknown).unwrap(),
        before,
        "unknown executables must be left byte-for-byte untouched"
    );
}

#[test]
fn quarantines_stale_uvx_wrapper_even_before_local_bin_is_on_current_path() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let uvx_bin = home.join(".cache/uv/archive-v0/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let stale_wrapper = uvx_bin.join("amplihack");
    write_executable(
        &stale_wrapper,
        "#!/bin/sh\n# uvx generated shim\nexec uvx --from amplihack amplihack \"$@\"\n",
    );

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![uvx_bin],
    ))
    .expect("safe stale uvx wrappers should be quarantined even before profile changes affect the current PATH");

    assert!(!stale_wrapper.exists());
    assert_eq!(report.resolved_after, preferred_rust);
    assert_eq!(report.neutralized.len(), 1);
}

#[test]
fn quarantines_uv_cache_shim_that_delegates_to_user_local_amplihack() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let uv_cache_bin = home.join(".cache/uv/archive-v0/hash/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let stale_wrapper = uv_cache_bin.join("amplihack");
    write_executable(
        &stale_wrapper,
        "#!/usr/bin/env bash\nexec \"$HOME/.local/bin/amplihack\" \"$@\"\n",
    );

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![uv_cache_bin, preferred_bin.clone()],
    ))
    .expect("uv-cache shim that shadows the preferred Rust binary should be quarantined");

    assert!(!stale_wrapper.exists());
    assert_eq!(
        report.neutralized[0].kind,
        NeutralizedWrapperKind::StaleUvxWrapper
    );
    assert_eq!(report.resolved_after, preferred_rust);
}

#[test]
fn update_repair_uses_preserved_parent_path_even_when_runtime_path_is_repaired() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let uvx_bin = home.join(".cache/uv/archive-v0/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let stale_wrapper = uvx_bin.join("amplihack");
    write_executable(
        &stale_wrapper,
        "#!/bin/sh\n# uvx generated shim\nexec uvx --from amplihack amplihack \"$@\"\n",
    );

    let parent_path = std::env::join_paths([uvx_bin.clone(), preferred_bin.clone()]).unwrap();
    let path_dirs = std::env::split_paths(&parent_path).collect();
    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        path_dirs,
    ))
    .expect("update repair must inspect the parent PATH that had stale wrappers before PATH was repaired for subprocess execution");

    assert!(!stale_wrapper.exists());
    assert_eq!(report.neutralized.len(), 1);
    assert_eq!(report.resolved_after, preferred_rust);
}
