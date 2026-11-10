//! Issue #2297: installation health is stronger than a successful version probe.
//! Exercise the public discovery boundary so tests remain independent of validator types.
#![cfg(unix)]
use amplihack_utils::launch_target::{
    InstallDecision, TargetSource, decide_install, resolve_from_candidates,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn tree(root: &Path, complete: bool) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let codex = root.join("codex");
    executable(
        &codex,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.166.0; exit 0; fi\nprintf session > \"${0%/*}/session\"\nexit 0\n",
    );
    if complete {
        executable(&root.join("codex-code-mode-host"), "#!/bin/sh\nexit 0\n");
    }
    codex
}

#[test]
fn version_success_does_not_accept_missing_code_mode_host() {
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(temp.path(), false);
    let result = resolve_from_candidates("codex", &[(codex.clone(), TargetSource::Path)]);
    assert!(
        result.target.is_none(),
        "version-only success accepted an incomplete install"
    );
    let report = result.rejection_report("codex", "@openai/codex");
    assert!(report.contains(&codex.display().to_string()), "{report}");
    assert!(report.contains("codex-code-mode-host"), "{report}");
    assert!(
        !temp.path().join("session").exists(),
        "validation must not start agent work"
    );
}

#[test]
fn complete_tree_is_accepted_and_retains_launch_path() {
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(temp.path(), true);
    let result = resolve_from_candidates("codex", &[(codex.clone(), TargetSource::Path)]);
    assert_eq!(result.target.expect("complete installation").path, codex);
    assert!(!temp.path().join("session").exists());
}

#[test]
fn symlink_checks_helpers_beside_native_binary_not_beside_link() {
    let temp = tempfile::tempdir().unwrap();
    let native = tree(&temp.path().join("native"), false);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let link = bin.join("codex");
    symlink(&native, &link).unwrap();
    executable(&bin.join("codex-code-mode-host"), "#!/bin/sh\nexit 0\n");
    let rejected = resolve_from_candidates("codex", &[(link.clone(), TargetSource::Path)]);
    assert!(
        rejected.target.is_none(),
        "unrelated helper beside symlink is not usable"
    );
    executable(
        &native.parent().unwrap().join("codex-code-mode-host"),
        "#!/bin/sh\nexit 0\n",
    );
    let accepted = resolve_from_candidates("codex", &[(link.clone(), TargetSource::Path)]);
    assert_eq!(accepted.target.expect("repaired symlink target").path, link);
}

#[test]
fn helper_must_be_an_executable_regular_file() {
    for directory in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let codex = tree(temp.path(), false);
        let helper = temp.path().join("codex-code-mode-host");
        if directory {
            fs::create_dir(&helper).unwrap();
        } else {
            fs::write(&helper, "not executable").unwrap();
        }
        let result = resolve_from_candidates("codex", &[(codex, TargetSource::Path)]);
        assert!(
            result.target.is_none(),
            "invalid helper was accepted: directory={directory}"
        );
    }
}

#[test]
fn incomplete_managed_install_routes_to_existing_install_policy() {
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(temp.path(), false);
    let result = resolve_from_candidates("codex", &[(codex, TargetSource::AmplihackPrefix)]);
    assert_eq!(
        decide_install("codex", &result, Some("0.166.0"), None),
        InstallDecision::InstallMissing
    );
}

#[test]
fn broken_authoritative_override_does_not_fall_through_to_complete_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let broken = tree(&temp.path().join("override"), false);
    let good = tree(&temp.path().join("managed"), true);
    let result = resolve_from_candidates(
        "codex",
        &[
            (
                broken.clone(),
                TargetSource::ExplicitOverride {
                    user_supplied: true,
                },
            ),
            (good, TargetSource::AmplihackPrefix),
        ],
    );
    assert!(
        result.target.is_none(),
        "explicit broken override must stop startup"
    );
    let report = result.rejection_report("codex", "@openai/codex");
    assert!(
        report.contains(&broken.display().to_string()) && report.contains("codex-code-mode-host"),
        "{report}"
    );
}

#[test]
fn older_standalone_needs_no_code_mode_host() {
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(temp.path(), false);
    assert!(
        amplihack_utils::codex_installation::validate_codex_installation(&codex, "0.98.0").is_ok()
    );
}

#[test]
fn unverified_versions_are_inconclusive_and_do_not_buy_repair() {
    use amplihack_utils::codex_installation::validate_codex_installation;
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(temp.path(), true);
    for version in ["0.120.0", "0.167.0", "1.0.0", "0.166.0-alpha.1"] {
        let error = validate_codex_installation(&codex, version).unwrap_err();
        assert!(!error.repairable, "{version}: {error}");
        assert!(error.to_string().contains("inconclusive"));
    }
}

#[test]
fn earliest_verified_host_only_release_requires_helper() {
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(temp.path(), false);
    let error = amplihack_utils::codex_installation::validate_codex_installation(&codex, "0.147.0")
        .unwrap_err();
    assert!(error.repairable && error.to_string().contains("codex-code-mode-host"));
}

fn npm_tree(root: &Path, version: &str, external: bool) -> (PathBuf, PathBuf) {
    let package = root.join("node_modules/@openai/codex");
    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(
        package.join("package.json"),
        format!(
            r#"{{"name":"@openai/codex","version":"{version}","bin":{{"codex":"bin/codex.js"}}}}"#
        ),
    )
    .unwrap();
    let wrapper = package.join("bin/codex.js");
    executable(&wrapper, "#!/bin/sh\nexit 0\n");
    let (os, target_os) = match std::env::consts::OS {
        "linux" => ("linux", "unknown-linux-musl"),
        "macos" => ("darwin", "apple-darwin"),
        other => panic!("unsupported test platform: {other}"),
    };
    let (arch, rust_arch) = match std::env::consts::ARCH {
        "x86_64" => ("x64", "x86_64"),
        "aarch64" => ("arm64", "aarch64"),
        other => panic!("unsupported test architecture: {other}"),
    };
    if version != "0.98.0" {
        fs::write(package.join("package.json"), format!(r#"{{"name":"@openai/codex","version":"{version}","bin":{{"codex":"bin/codex.js"}},"optionalDependencies":{{"@openai/codex-{os}-{arch}":"npm:@openai/codex@{version}-{os}-{arch}"}}}}"#)).unwrap();
    }
    let vendor = if external {
        let platform = root.join(format!("node_modules/@openai/codex-{os}-{arch}"));
        fs::create_dir_all(&platform).unwrap();
        fs::write(
            platform.join("package.json"),
            format!(r#"{{"name":"@openai/codex","version":"{version}-{os}-{arch}"}}"#),
        )
        .unwrap();
        platform.join("vendor")
    } else {
        package.join("vendor")
    };
    let native = vendor
        .join(format!("{rust_arch}-{target_os}"))
        .join(if version == "0.98.0" { "codex" } else { "bin" });
    tree(&native, version != "0.98.0");
    (wrapper, native.join("codex"))
}

#[test]
fn official_npm_layouts_validate_native_and_retain_wrapper() {
    use amplihack_utils::codex_installation::validate_codex_installation;
    for (version, external) in [
        ("0.98.0", false),
        ("0.147.0", true),
        ("0.166.0", false),
        ("0.166.0", true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (wrapper, native) = npm_tree(temp.path(), version, external);
        let accepted = validate_codex_installation(&wrapper, version).unwrap();
        assert_eq!(accepted.launch_path, wrapper);
        assert_eq!(accepted.native_path, native);
        if version != "0.98.0" {
            fs::remove_file(native.parent().unwrap().join("codex-code-mode-host")).unwrap();
            // A helper beside the wrapper cannot repair the selected native tree.
            executable(
                &wrapper.parent().unwrap().join("codex-code-mode-host"),
                "#!/bin/sh\nexit 0\n",
            );
            assert!(
                validate_codex_installation(&wrapper, version)
                    .unwrap_err()
                    .repairable
            );
        }
    }
}

#[test]
fn malformed_or_oversized_wrapper_metadata_is_rejected() {
    use amplihack_utils::codex_installation::validate_codex_installation;
    for metadata in ["{".to_owned(), " ".repeat(65537)] {
        let temp = tempfile::tempdir().unwrap();
        let (wrapper, _) = npm_tree(temp.path(), "0.166.0", true);
        fs::write(
            wrapper
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("package.json"),
            metadata,
        )
        .unwrap();
        assert!(validate_codex_installation(&wrapper, "0.166.0").is_err());
    }
}

#[test]
fn packaged_resource_host_is_checked_before_sibling_host() {
    use amplihack_utils::codex_installation::validate_codex_installation;
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(&temp.path().join("bin"), true);
    fs::write(temp.path().join("codex-package.json"), "{}").unwrap();
    let resources = temp.path().join("codex-resources");
    fs::create_dir(&resources).unwrap();
    let host = resources.join("codex-code-mode-host");
    fs::write(&host, "not executable").unwrap();
    assert!(validate_codex_installation(&codex, "0.166.0").is_err());
    executable(&host, "#!/bin/sh\nexit 0\n");
    fs::remove_file(codex.parent().unwrap().join("codex-code-mode-host")).unwrap();
    assert!(validate_codex_installation(&codex, "0.166.0").is_ok());
}

#[test]
fn diagnostics_do_not_allow_paths_to_forge_report_lines() {
    let temp = tempfile::tempdir().unwrap();
    let codex = tree(&temp.path().join("bad\n\x1b[31mFORGED"), false);
    let result = resolve_from_candidates("codex", &[(codex, TargetSource::Path)]);
    let detail = &result.rejected[0].1.explain();
    assert!(
        !detail.contains('\n') && !detail.contains('\x1b'),
        "{detail:?}"
    );
}

#[test]
fn codex_memo_does_not_hide_filesystem_damage_or_repair() {
    let temp = tempfile::tempdir().unwrap();
    tree(temp.path(), true);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_memo_contract", "--nocapture"])
        .env_clear()
        .env("CODEX_MEMO_CASE", "1")
        .env("HOME", temp.path())
        .env("PATH", temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn child_memo_contract() {
    if std::env::var_os("CODEX_MEMO_CASE").is_none() {
        return;
    }
    use amplihack_utils::launch_target::{OverrideOrigin, resolve};
    let root = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(resolve("codex", OverrideOrigin::User).target.is_some());
    fs::remove_file(root.join("codex-code-mode-host")).unwrap();
    assert!(resolve("codex", OverrideOrigin::User).target.is_none());
    executable(&root.join("codex-code-mode-host"), "#!/bin/sh\nexit 0\n");
    assert!(resolve("codex", OverrideOrigin::User).target.is_some());
}

#[test]
fn selected_broken_platform_package_cannot_use_a_healthy_bundled_tree() {
    use amplihack_utils::codex_installation::validate_codex_installation;
    let temp = tempfile::tempdir().unwrap();
    let (wrapper, native) = npm_tree(temp.path(), "0.166.0", true);
    npm_tree(temp.path(), "0.166.0", false);
    fs::remove_file(native.parent().unwrap().join("codex-code-mode-host")).unwrap();
    let error = validate_codex_installation(&wrapper, "0.166.0").unwrap_err();
    assert!(error.repairable && error.to_string().contains(&native.display().to_string()));
}

#[test]
fn unsupported_wrapper_dependency_selection_is_inconclusive() {
    use amplihack_utils::codex_installation::validate_codex_installation;
    let temp = tempfile::tempdir().unwrap();
    let (wrapper, _) = npm_tree(temp.path(), "0.166.0", true);
    let path = wrapper
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("package.json");
    let metadata = fs::read_to_string(&path)
        .unwrap()
        .replace("npm:@openai/codex@", "npm:other-package@");
    fs::write(path, metadata).unwrap();
    let error = validate_codex_installation(&wrapper, "0.166.0").unwrap_err();
    assert!(!error.repairable);
    assert!(error.to_string().contains("dependency selection"));
}
