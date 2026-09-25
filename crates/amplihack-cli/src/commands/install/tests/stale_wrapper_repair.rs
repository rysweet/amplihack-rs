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
    let manifest = fs::read_to_string(
        report
            .manifest_path
            .as_ref()
            .expect("quarantining wrappers must write a manifest"),
    )
    .unwrap();
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
fn no_shadowing_wrapper_does_not_create_empty_quarantine_run() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![preferred_bin],
    ))
    .expect("already Rust-first PATH should need no repair");

    assert!(report.neutralized.is_empty());
    assert!(
        report.manifest_path.is_none(),
        "no-op repair should not leave empty quarantine manifests behind"
    );
    assert!(
        !home.join(".amplihack/quarantine/stale-wrappers").exists(),
        "no-op repair should not create quarantine directories"
    );
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

/// Lay out the `node_modules/.bin/amplihack` shim exactly as
/// `npx --package=<amplihack-rs> -- amplihack install` does: a symlink into
/// the package's own `npm/bin/amplihack.js` wrapper, under `~/.npm/_npx/<hash>`.
#[cfg(unix)]
fn create_npx_shim(home: &Path, wrapper_source: &str) -> (PathBuf, PathBuf) {
    let npx_root = home.join(".npm/_npx/20a160db8db9e1ce/node_modules");
    let npx_bin = npx_root.join(".bin");
    let wrapper = npx_root.join("@rysweet/amplihack-rs/npm/bin/amplihack.js");
    write_executable(&wrapper, wrapper_source);
    fs::create_dir_all(&npx_bin).unwrap();
    let shim = npx_bin.join("amplihack");
    std::os::unix::fs::symlink("../@rysweet/amplihack-rs/npm/bin/amplihack.js", &shim).unwrap();
    (npx_bin, shim)
}

/// Issue #1480: the README quick-start (`npx ... amplihack install`) puts our
/// own npm wrapper shim first on PATH. The neutralizer must recognize it as a
/// transient npx shim, leave it alone, and not abort the install.
#[cfg(unix)]
#[test]
fn issue_1480_transient_npx_shim_for_our_wrapper_does_not_abort_install() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let wrapper_source = include_str!("../../../../../../npm/bin/amplihack.js");
    let (npx_bin, shim) = create_npx_shim(&home, wrapper_source);

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![npx_bin, preferred_bin],
    ))
    .expect("the transient npx shim for our own npm wrapper must not abort install");

    assert!(report.neutralized.is_empty());
    assert!(report.manifest_path.is_none());
    assert_eq!(report.skipped_transient_shims, vec![shim.clone()]);
    assert_eq!(
        report.resolved_after, shim,
        "while npx runs, its shim still resolves first; repair must accept that"
    );
    assert!(
        fs::symlink_metadata(&shim).is_ok(),
        "the npx shim belongs to the running npx process and must be left in place"
    );
}

/// Being under `_npx/` alone is not enough: an unrelated package's bin that
/// happens to be named `amplihack` is still an unknown executable.
#[cfg(unix)]
#[test]
fn issue_1480_npx_shim_for_unrelated_script_is_still_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let (npx_bin, shim) = create_npx_shim(&home, "#!/usr/bin/env node\nconsole.log('other');\n");

    let err = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![npx_bin, preferred_bin],
    ))
    .expect_err("an unrecognized script under _npx must still block repair");

    match err {
        StaleWrapperRepairError::UnknownShadowingExecutable { path, .. } => {
            assert_eq!(path, shim);
        }
        other => panic!("expected UnknownShadowingExecutable, got {other:?}"),
    }
}

/// Skipping the npx shim must not stop the repair: a stale Python wrapper
/// behind it on PATH is still quarantined.
#[cfg(unix)]
#[test]
fn issue_1480_npx_shim_does_not_hide_a_stale_wrapper_behind_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let uv_tools_bin = home.join(".local/share/uv/tools/amplihack/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let stale_wrapper = uv_tools_bin.join("amplihack");
    write_executable(
        &stale_wrapper,
        "#!/usr/bin/env python3\nimport sys\nfrom amplihack.cli import main\nsys.exit(main())\n",
    );
    let wrapper_source = include_str!("../../../../../../npm/bin/amplihack.js");
    let (npx_bin, shim) = create_npx_shim(&home, wrapper_source);

    let report = neutralize_shadowing_stale_wrappers(repair_config(
        &home,
        &preferred_rust,
        &preferred_rust,
        vec![npx_bin, uv_tools_bin, preferred_bin],
    ))
    .expect("a stale wrapper behind the npx shim is repairable");

    assert_eq!(report.skipped_transient_shims, vec![shim.clone()]);
    assert_eq!(report.neutralized.len(), 1);
    assert_eq!(report.neutralized[0].original_path, stale_wrapper);
    assert!(!stale_wrapper.exists());
    assert_eq!(report.resolved_after, shim);
}

/// The post-install PATH advisory must not contradict the npx notice by
/// telling the user to reorder PATH around a shim that disappears with npx.
#[cfg(unix)]
#[test]
fn issue_1480_path_advisory_does_not_flag_the_npx_shim() {
    use crate::path_conflicts::{PathAnalysisInput, analyze_path_conflicts};

    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let wrapper_source = include_str!("../../../../../../npm/bin/amplihack.js");
    let (npx_bin, _shim) = create_npx_shim(&home, wrapper_source);

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home.clone(),
        current_exe: preferred_rust,
        path_dirs: vec![npx_bin, preferred_bin],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    assert_eq!(
        super::super::binary::path_conflict_warning_after_install(&report),
        None
    );
}

/// Control for the test above: the same layout with an unrelated script under
/// `_npx` still gets the shadowing advisory.
#[cfg(unix)]
#[test]
fn issue_1480_path_advisory_still_flags_an_unrelated_npx_script() {
    use crate::path_conflicts::{PathAnalysisInput, analyze_path_conflicts};

    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let (npx_bin, _shim) = create_npx_shim(&home, "#!/usr/bin/env node\nconsole.log('other');\n");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home.clone(),
        current_exe: preferred_rust,
        path_dirs: vec![npx_bin, preferred_bin],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    let warning = super::super::binary::path_conflict_warning_after_install(&report)
        .expect("an unrelated executable shadowing ~/.local/bin must still be reported");
    assert!(warning.contains("shadows"));
}

/// Skipping the npx shim must not hide a persistent executable behind it:
/// once npx exits, that executable still shadows ~/.local/bin, so the
/// advisory has to name it.
#[cfg(unix)]
#[test]
fn issue_1480_path_advisory_still_flags_a_persistent_shadow_behind_the_npx_shim() {
    use crate::path_conflicts::{PathAnalysisInput, analyze_path_conflicts};

    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let wrapper_source = include_str!("../../../../../../npm/bin/amplihack.js");
    let (npx_bin, _shim) = create_npx_shim(&home, wrapper_source);
    let system_bin = temp.path().join("usr/local/bin");
    let persistent = create_exe_stub(&system_bin, "amplihack");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home.clone(),
        current_exe: preferred_rust,
        path_dirs: vec![npx_bin, system_bin, preferred_bin],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    let warning = super::super::binary::path_conflict_warning_after_install(&report)
        .expect("a persistent executable behind the npx shim still shadows ~/.local/bin");
    assert!(
        warning.contains(&persistent.display().to_string()),
        "advisory must name the persistent shadow, got: {warning}"
    );
    assert!(
        !warning.contains("_npx"),
        "advisory must not name the transient npx shim, got: {warning}"
    );
}

/// A distinct binary after ~/.local/bin is still reported as ambiguous when
/// the npx shim resolves first, exactly as it would be without npx.
#[cfg(unix)]
#[test]
fn issue_1480_path_advisory_keeps_ambiguity_warning_behind_the_npx_shim() {
    use crate::path_conflicts::{PathAnalysisInput, analyze_path_conflicts};

    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let preferred_bin = home.join(".local/bin");
    let preferred_rust = create_exe_stub(&preferred_bin, "amplihack");
    let wrapper_source = include_str!("../../../../../../npm/bin/amplihack.js");
    let (npx_bin, _shim) = create_npx_shim(&home, wrapper_source);
    let system_bin = temp.path().join("usr/bin");
    let later = create_exe_stub(&system_bin, "amplihack");

    let report = analyze_path_conflicts(&PathAnalysisInput {
        home_dir: home.clone(),
        current_exe: preferred_rust,
        path_dirs: vec![npx_bin, preferred_bin, system_bin],
        binary_names: vec!["amplihack".into()],
    })
    .unwrap();

    let warning = super::super::binary::path_conflict_warning_after_install(&report)
        .expect("a second distinct binary on PATH is still ambiguous");
    assert!(warning.contains("Multiple distinct"), "got: {warning}");
    assert!(
        warning.contains(&later.display().to_string()),
        "got: {warning}"
    );
    assert!(!warning.contains("_npx"), "got: {warning}");
}
