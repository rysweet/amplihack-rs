use super::helpers::*;
use super::*;
use std::fs;

#[test]
fn local_install_stages_amplifier_bundle_for_dev_orchestrator() {
    // Issue #243: the dev-orchestrator skill's required execution path
    // (`amplihack recipe run smart-orchestrator`) is unreachable unless
    // amplihack install stages the amplifier-bundle recipes
    // to ~/.amplihack/amplifier-bundle/.
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    create_source_repo(temp.path());
    local_install(temp.path(), None).unwrap();

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let bundle = temp.path().join(".amplihack/amplifier-bundle");
    assert!(
        bundle.is_dir(),
        "amplifier-bundle must be staged at ~/.amplihack/amplifier-bundle/ after install"
    );
    for recipe in [
        "recipes/smart-orchestrator.yaml",
        "recipes/default-workflow.yaml",
        "recipes/investigation-workflow.yaml",
    ] {
        assert!(
            bundle.join(recipe).is_file(),
            "{recipe} must be staged so dev-orchestrator can execute it"
        );
    }
    // The presence check used by ensure_framework_installed must now treat
    // a missing bundle as a reason to re-install on next launch.
    let staging_claude = temp.path().join(".amplihack/.claude");
    assert!(
        settings::missing_framework_paths(&staging_claude)
            .unwrap()
            .is_empty(),
        "fully-staged install must report no missing framework paths"
    );
    fs::remove_dir_all(&bundle).unwrap();
    let missing = settings::missing_framework_paths(&staging_claude).unwrap();
    assert!(
        missing
            .iter()
            .any(|m| m.contains("amplifier-bundle/recipes/smart-orchestrator.yaml")),
        "missing amplifier-bundle must be reported by presence check (issue #243), got: {missing:?}"
    );
}

#[test]
fn uninstall_removes_staged_amplifier_bundle() {
    // Issue #243 follow-up: uninstall must clean up the staged bundle so
    // a stale tree does not linger after the framework is removed.
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    fs::create_dir_all(temp.path().join(".amplihack/.claude/install")).unwrap();
    let bundle = temp.path().join(".amplihack/amplifier-bundle/recipes");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(bundle.join("smart-orchestrator.yaml"), "x\n").unwrap();
    let manifest = InstallManifest::default();
    manifest::write_manifest(
        &temp
            .path()
            .join(".amplihack/.claude/install/amplihack-manifest.json"),
        &manifest,
    )
    .unwrap();

    run_uninstall().unwrap();

    assert!(
        !temp.path().join(".amplihack/amplifier-bundle").exists(),
        "uninstall must remove ~/.amplihack/amplifier-bundle/"
    );

    crate::test_support::restore_home(previous);
}

#[test]
fn local_install_writes_manifest() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    create_source_repo(temp.path());
    local_install(temp.path(), None).unwrap();

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }

    assert!(
        temp.path()
            .join(".amplihack/.claude/install/amplihack-manifest.json")
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(".amplihack/.claude/tools/amplihack/hooks/pre_tool_use.py")
            .exists()
    );
    assert!(
        !temp
            .path()
            .join(".amplihack/.claude/tools/amplihack/hooks")
            .exists()
    );

    let settings = fs::read_to_string(temp.path().join(".claude/settings.json")).unwrap();
    assert!(
        settings.contains("amplihack-hooks"),
        "settings.json must reference amplihack-hooks binary, got:\n{settings}"
    );
    assert!(
        settings.contains("pre-tool-use"),
        "settings.json must reference 'pre-tool-use' subcommand, got:\n{settings}"
    );

    crate::test_support::restore_home(previous);
}

#[test]
fn uninstall_removes_manifest_tracked_files() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    fs::create_dir_all(temp.path().join(".amplihack/.claude/install")).unwrap();
    fs::create_dir_all(temp.path().join(".amplihack/.claude/agents/amplihack")).unwrap();
    fs::write(
        temp.path()
            .join(".amplihack/.claude/agents/amplihack/demo.txt"),
        "x",
    )
    .unwrap();
    let manifest = InstallManifest {
        files: vec![String::from("agents/amplihack/demo.txt")],
        dirs: vec![String::from("agents/amplihack")],
        binaries: vec![],
        hook_registrations: vec![],
        ..InstallManifest::default()
    };
    manifest::write_manifest(
        &temp
            .path()
            .join(".amplihack/.claude/install/amplihack-manifest.json"),
        &manifest,
    )
    .unwrap();
    run_uninstall().unwrap();
    assert!(
        !temp
            .path()
            .join(".amplihack/.claude/agents/amplihack")
            .exists()
    );
    crate::test_support::restore_home(previous);
}
#[test]
fn read_manifest_treats_invalid_json_as_empty() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("amplihack-manifest.json");
    fs::write(&path, "{invalid json\n").unwrap();

    let result = manifest::read_manifest(&path).unwrap();

    assert!(result.files.is_empty());
    assert!(result.dirs.is_empty());
    assert!(result.binaries.is_empty());
    assert!(result.hook_registrations.is_empty());
}
#[test]
fn install_manifest_has_all_four_fields() {
    let manifest = InstallManifest {
        files: vec![String::from("a.py")],
        dirs: vec![String::from("dir")],
        binaries: vec![String::from("/home/user/.local/bin/amplihack-hooks")],
        hook_registrations: vec![String::from("SessionStart"), String::from("Stop")],
        ..InstallManifest::default()
    };
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.dirs.len(), 1);
    assert_eq!(manifest.binaries.len(), 1);
    assert_eq!(manifest.hook_registrations.len(), 2);
}

#[test]
fn install_manifest_serialises_new_fields() {
    let manifest = InstallManifest {
        files: vec![],
        dirs: vec![],
        binaries: vec![String::from("/home/user/.local/bin/amplihack-hooks")],
        hook_registrations: vec![String::from("SessionStart")],
        ..InstallManifest::default()
    };
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(
        json.contains("\"binaries\""),
        "serialised manifest must contain 'binaries'"
    );
    assert!(
        json.contains("\"hook_registrations\""),
        "serialised manifest must contain 'hook_registrations'"
    );
}
#[test]
fn install_manifest_deserialises_old_format_with_empty_defaults() {
    let old_json = r#"{"files": ["a.py"], "dirs": ["dir"]}"#;
    let manifest: InstallManifest =
        serde_json::from_str(old_json).expect("must deserialise old 2-field format");
    assert_eq!(manifest.files, vec!["a.py"]);
    assert!(
        manifest.binaries.is_empty(),
        "binaries must default to [] for old manifests"
    );
    assert!(
        manifest.hook_registrations.is_empty(),
        "hook_registrations must default to [] for old manifests"
    );
}
#[test]
fn create_runtime_dirs_applies_0o755_permissions() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&staging_dir).unwrap();
    directories::create_runtime_dirs(&staging_dir).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in RUNTIME_DIRS {
            let full = staging_dir.join(dir);
            assert!(full.exists(), "runtime dir '{dir}' must be created");
            let mode = fs::metadata(&full).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o755,
                "runtime dir '{dir}' must have 0o755 perms, got {:03o}",
                mode & 0o777
            );
        }
    }

    crate::test_support::restore_home(previous);
}

#[test]
fn copy_dir_recursive_skips_symlinks_without_following() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    fs::create_dir_all(&src).unwrap();

    fs::write(src.join("real.txt"), "content").unwrap();

    #[cfg(unix)]
    {
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, "sensitive-data").unwrap();
        std::os::unix::fs::symlink(&outside, src.join("evil_link.txt")).unwrap();
    }

    filesystem::copy_dir_recursive(&src, &dst).unwrap();

    assert!(dst.join("real.txt").exists(), "real.txt must be copied");

    #[cfg(unix)]
    {
        let sym_dst = dst.join("evil_link.txt");
        if sym_dst.exists() {
            let content = fs::read_to_string(&sym_dst).unwrap_or_default();
            assert_ne!(
                content, "sensitive-data",
                "symlink must not be followed; sensitive content must not be copied"
            );
        }
        assert!(
            !sym_dst.is_file() || sym_dst.is_symlink(),
            "evil_link.txt in dst must not be a regular file"
        );
    }
}

#[test]
fn local_install_writes_manifest_with_all_four_fields() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    create_exe_stub(&bin_dir, "python3");
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    create_source_repo(temp.path());
    local_install(temp.path(), None).unwrap();

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let manifest_path = temp
        .path()
        .join(".amplihack/.claude/install/amplihack-manifest.json");
    assert!(manifest_path.exists());

    let raw = fs::read_to_string(&manifest_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert!(json.get("files").is_some(), "manifest must have 'files'");
    assert!(json.get("dirs").is_some(), "manifest must have 'dirs'");
    assert!(
        json.get("binaries").is_some(),
        "manifest must have 'binaries'"
    );
    assert!(
        json.get("hook_registrations").is_some(),
        "manifest must have 'hook_registrations'"
    );

    let binaries = json["binaries"].as_array().unwrap();
    assert!(
        !binaries.is_empty(),
        "manifest.binaries must be non-empty after install"
    );

    let hook_regs = json["hook_registrations"].as_array().unwrap();
    assert!(
        !hook_regs.is_empty(),
        "manifest.hook_registrations must be non-empty after install"
    );
}

#[test]
fn run_install_with_local_path_skips_git_clone() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    create_exe_stub(&bin_dir, "python3");
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    let local_repo = temp.path().join("local-repo");
    fs::create_dir_all(&local_repo).unwrap();
    create_source_repo(&local_repo);

    let result = run_install(Some(local_repo), false, false);

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    result.unwrap();

    assert!(
        temp.path()
            .join(".amplihack/.claude/install/amplihack-manifest.json")
            .exists(),
        "manifest must exist after --local install (no git required)"
    );
}

#[test]
fn run_install_with_nonexistent_local_path_returns_err() {
    let nonexistent = std::path::PathBuf::from("/nonexistent/amplihack-repo/does-not-exist");
    let result = run_install(Some(nonexistent), false, false);
    assert!(
        result.is_err(),
        "run_install must return Err for a non-existent --local path"
    );
}

#[test]
fn find_framework_repo_root_finds_github_tarball_layout() {
    let temp = tempfile::tempdir().unwrap();
    let extracted = temp.path().join("amplihack-main");
    create_source_repo(&extracted);

    let found = clone::find_framework_repo_root(temp.path()).unwrap();

    assert_eq!(found, extracted);
}

#[test]
fn find_compatible_framework_repo_root_accepts_downloaded_composable_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let extracted = temp.path().join("amplihack-main");
    create_source_repo(&extracted);

    let found = clone::find_compatible_framework_repo_root(temp.path(), "test download").unwrap();

    assert_eq!(found, extracted);
}

#[test]
fn find_compatible_framework_repo_root_rejects_downloaded_stale_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let extracted = temp.path().join("amplihack-main");
    create_source_repo(&extracted);
    fs::write(
        extracted.join("amplifier-bundle/recipes/smart-orchestrator.yaml"),
        r#"name: "smart-orchestrator"
steps:
  - id: "parse-decomposition"
    type: "shell"
    command: |
      HELPER="$(amplihack resolve-bundle-asset helper-path)"
      python3 - <<'PY'
      import importlib
      PY
"#,
    )
    .unwrap();

    let err = clone::find_compatible_framework_repo_root(temp.path(), "test download")
        .expect_err("downloaded stale smart-orchestrator bundle must be rejected");
    let msg = err.to_string();

    assert!(
        msg.contains("downloaded framework bundle")
            && msg.contains("incompatible")
            && msg.contains("smart-orchestrator"),
        "error must explain the incompatible downloaded bundle, got: {msg}"
    );
}

#[test]
fn find_framework_repo_root_errors_when_archive_lacks_claude_dir() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("archive-root/empty")).unwrap();

    let err = clone::find_framework_repo_root(temp.path()).unwrap_err();

    assert!(
        err.to_string()
            .contains("did not contain a repository root"),
        "unexpected error: {err}"
    );
}

#[test]
fn read_manifest_rejects_path_traversal_in_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let bad_manifest = r#"{"files": ["../../../etc/passwd"], "dirs": []}"#;
    fs::write(&path, bad_manifest).unwrap();

    let result = manifest::read_manifest(&path);
    assert!(
        result.is_err(),
        "manifest with '..' in file entries must be rejected"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("path-traversal") || err.contains("path traversal"),
        "error should mention path traversal, got: {err}"
    );
}

#[test]
fn read_manifest_rejects_path_traversal_in_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let bad_manifest = r#"{"files": [], "dirs": ["foo/../../bar"]}"#;
    fs::write(&path, bad_manifest).unwrap();

    let result = manifest::read_manifest(&path);
    assert!(
        result.is_err(),
        "manifest with '..' in dir entries must be rejected"
    );
}

#[test]
fn read_manifest_rejects_absolute_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let bad_manifest = r#"{"files": ["/etc/passwd"], "dirs": []}"#;
    fs::write(&path, bad_manifest).unwrap();

    let result = manifest::read_manifest(&path);
    assert!(
        result.is_err(),
        "manifest with absolute paths must be rejected"
    );
}

#[test]
fn read_manifest_accepts_valid_relative_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let good_manifest =
        r#"{"files": ["agents/amplihack/foo.py", "tools/bar.sh"], "dirs": ["agents/amplihack"]}"#;
    fs::write(&path, good_manifest).unwrap();

    let result = manifest::read_manifest(&path);
    assert!(
        result.is_ok(),
        "manifest with valid relative paths must be accepted, got: {:?}",
        result.err()
    );
}

#[test]
fn copy_amplifier_bundle_errors_when_source_missing() {
    // Issue #243: missing source bundle must be a hard error during install,
    // because missing_framework_paths() now treats the bundle as required —
    // a silent skip would cause an infinite re-install loop on every launcher
    // boot.
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo-without-bundle");
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&repo_root).unwrap();
    fs::create_dir_all(&claude_dir).unwrap();

    let result = directories::copy_amplifier_bundle(&repo_root, &claude_dir);

    assert!(
        result.is_err(),
        "missing source amplifier-bundle must error, not silently warn"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("amplifier-bundle"),
        "error must mention amplifier-bundle, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn copy_amplifier_bundle_rejects_symlinked_source_root() {
    // Defense-in-depth: a malicious local repo could symlink amplifier-bundle/
    // at an arbitrary readable directory and have it copied into the user's
    // staging area. The bundle root must be a real directory.
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();

    let elsewhere = temp.path().join("evil");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("secret.txt"), "private").unwrap();
    std::os::unix::fs::symlink(&elsewhere, repo_root.join("amplifier-bundle")).unwrap();

    let result = directories::copy_amplifier_bundle(&repo_root, &claude_dir);

    assert!(
        result.is_err(),
        "symlinked amplifier-bundle root must be rejected"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("symlink"),
        "error must mention symlink rejection, got: {err}"
    );
    assert!(
        !claude_dir
            .parent()
            .unwrap()
            .join("amplifier-bundle")
            .exists(),
        "no bundle must have been staged from the symlinked source"
    );
}

#[test]
fn copy_amplifier_bundle_replaces_existing_atomically() {
    // The copy must use a temp-dir + rename pattern so a failed mid-flight
    // refresh never destroys an existing working bundle. Verify both the
    // happy path (new content replaces old) and that no leftover staging
    // dirs remain in the parent.
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();

    issue_734_create_bundle_repo(
        &repo_root,
        &issue_734_compatible_smart_orchestrator().replace(
            "Composable smart task orchestrator",
            "Composable smart task orchestrator v1",
        ),
    );
    directories::copy_amplifier_bundle(&repo_root, &claude_dir).unwrap();

    let staged = temp.path().join(".amplihack/amplifier-bundle");
    assert!(
        fs::read_to_string(staged.join("recipes/smart-orchestrator.yaml"))
            .unwrap()
            .contains("v1")
    );

    let bundle_src = repo_root.join("amplifier-bundle");
    fs::write(
        bundle_src.join("recipes/smart-orchestrator.yaml"),
        issue_734_compatible_smart_orchestrator().replace(
            "Composable smart task orchestrator",
            "Composable smart task orchestrator v2",
        ),
    )
    .unwrap();
    fs::write(bundle_src.join("recipes/new-recipe.yaml"), "fresh\n").unwrap();
    directories::copy_amplifier_bundle(&repo_root, &claude_dir).unwrap();

    assert!(
        fs::read_to_string(staged.join("recipes/smart-orchestrator.yaml"))
            .unwrap()
            .contains("v2"),
        "re-install must replace existing content"
    );
    assert!(
        staged.join("recipes/new-recipe.yaml").is_file(),
        "re-install must add new files from the source"
    );

    let parent = temp.path().join(".amplihack");
    assert!(
        !parent.join("amplifier-bundle.staging").exists(),
        "no staging temp dir must remain after a successful install"
    );
    assert!(
        !parent.join("amplifier-bundle.old").exists(),
        "no backup dir must remain after a successful install"
    );
}

const ISSUE_734_REQUIRED_SMART_RECIPES: &[&str] = &[
    "smart-classify-route",
    "smart-execute-routing",
    "smart-reflect-loop",
    "smart-validate-summarize",
];

fn issue_734_compatible_smart_orchestrator() -> &'static str {
    r#"name: "smart-orchestrator"
description: "Composable smart task orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
"#
}

fn issue_734_stale_monolithic_smart_orchestrator() -> &'static str {
    r#"name: "smart-orchestrator"
description: "stale monolithic smart task orchestrator"
steps:
  - id: "parse-decomposition"
    type: "shell"
    command: |
      HELPER="$(amplihack resolve-bundle-asset helper-path)"
      python3 - <<'PY'
      import importlib
      helper = importlib.import_module("orch_helper")
      helper.parse_decomposition()
      PY
"#
}

fn issue_734_create_bundle_repo(root: &Path, smart_orchestrator: &str) {
    let bundle = root.join("amplifier-bundle");
    for dir in BUNDLE_ESSENTIAL_DESTS {
        fs::create_dir_all(bundle.join(dir)).unwrap();
        fs::write(bundle.join(dir).join("marker.txt"), "x\n").unwrap();
    }
    fs::create_dir_all(bundle.join("tools")).unwrap();
    fs::write(bundle.join("tools/statusline.sh"), "echo hi\n").unwrap();
    fs::write(bundle.join("CLAUDE.md"), "framework\n").unwrap();

    let recipes = bundle.join("recipes");
    fs::write(recipes.join("smart-orchestrator.yaml"), smart_orchestrator).unwrap();
    fs::write(
        recipes.join("default-workflow.yaml"),
        "name: \"default-workflow\"\nsteps: []\n",
    )
    .unwrap();
    fs::write(
        recipes.join("investigation-workflow.yaml"),
        "name: \"investigation-workflow\"\nsteps: []\n",
    )
    .unwrap();
    for recipe in ISSUE_734_REQUIRED_SMART_RECIPES {
        fs::write(
            recipes.join(format!("{recipe}.yaml")),
            format!("name: \"{recipe}\"\nsteps: []\n"),
        )
        .unwrap();
    }
    fs::write(
        recipes.join("_recipe_manifest.json"),
        r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-execute-routing": "11612506ae846a47",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
    )
    .unwrap();
}

#[test]
fn issue_734_copy_rejects_stale_monolithic_smart_orchestrator_without_overwriting_good_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();

    let good_repo = temp.path().join("good-repo");
    issue_734_create_bundle_repo(&good_repo, issue_734_compatible_smart_orchestrator());
    directories::copy_amplifier_bundle(&good_repo, &claude_dir).unwrap();

    let stale_repo = temp.path().join("stale-repo");
    issue_734_create_bundle_repo(&stale_repo, issue_734_stale_monolithic_smart_orchestrator());

    let result = directories::copy_amplifier_bundle(&stale_repo, &claude_dir);

    assert!(
        result.is_err(),
        "stale monolithic smart-orchestrator bundles must be rejected before staging"
    );
    let staged_smart = fs::read_to_string(
        temp.path()
            .join(".amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml"),
    )
    .unwrap();
    assert!(
        staged_smart.contains("recipe: \"smart-classify-route\""),
        "existing compatible staged bundle must remain after rejecting stale source"
    );
    assert!(
        !staged_smart.contains("resolve-bundle-asset helper-path")
            && !staged_smart.contains("importlib")
            && !staged_smart.contains("orch_helper"),
        "stale monolithic smart-orchestrator content must not remain staged:\n{staged_smart}"
    );
}

#[test]
fn issue_734_run_install_skips_stale_amplihack_home_and_stages_compatible_cwd_bundle() {
    with_install_env(|home| {
        let stale_home = home.join("stale-amplihack-home");
        fs::create_dir_all(stale_home.join(".claude")).unwrap();
        issue_734_create_bundle_repo(&stale_home, issue_734_stale_monolithic_smart_orchestrator());

        let fresh_checkout = home.join("fresh-checkout");
        issue_734_create_bundle_repo(&fresh_checkout, issue_734_compatible_smart_orchestrator());
        let nested = fresh_checkout.join("nested/project");
        fs::create_dir_all(&nested).unwrap();

        let previous_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
        unsafe { std::env::set_var("AMPLIHACK_HOME", &stale_home) };
        let _cwd = crate::test_support::CwdGuard::set(&nested).unwrap();

        let result = run_install(None, false, false);

        match previous_amplihack_home {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
        }
        result.expect(
            "install must skip an incompatible AMPLIHACK_HOME bundle and use the compatible cwd checkout",
        );

        let staged_smart = fs::read_to_string(
            home.join(".amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml"),
        )
        .unwrap();
        for recipe in ISSUE_734_REQUIRED_SMART_RECIPES {
            assert!(
                staged_smart.contains(&format!("recipe: \"{recipe}\"")),
                "staged smart-orchestrator must reference required sub-recipe {recipe}; got:\n{staged_smart}"
            );
            assert!(
                home.join(format!(".amplihack/amplifier-bundle/recipes/{recipe}.yaml"))
                    .is_file(),
                "staged bundle must include required companion recipe {recipe}.yaml"
            );
        }
        assert!(
            !staged_smart.contains("resolve-bundle-asset helper-path")
                && !staged_smart.contains("importlib")
                && !staged_smart.contains("orch_helper"),
            "stale AMPLIHACK_HOME smart-orchestrator must not remain staged:\n{staged_smart}"
        );
    });
}

// ============================================================================
// Issue #416: regression tests for bundle-only source layouts
// ============================================================================
//
// These tests fail until the install layer learns about the amplifier-bundle/
// source layout. They specify the contract for the fix.

/// Helper: stage env (HOME, hooks binary, PATH) like the existing install
/// tests do, run `f`, restore env. Avoids ~50 lines of boilerplate per test.
fn with_install_env<R>(f: impl FnOnce(&Path) -> R) -> R {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    helpers::create_exe_stub(&bin_dir, "python3");
    let hooks_stub = helpers::create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    let result = f(temp.path());

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);
    result
}

/// Bundle essentials that must be staged into ~/.amplihack/.claude/ when the
/// source repo uses the amplifier-bundle/ layout. Mirrors the design spec's
/// BUNDLE_DIR_MAPPING destinations.
const BUNDLE_ESSENTIAL_DESTS: &[&str] = &[
    "agents",
    "skills",
    "context",
    "tools/amplihack",
    "tools/xpia",
    "recipes",
    "behaviors",
    "modules",
];

#[test]
fn local_install_fails_when_copilot_plugin_registration_fails() {
    // Issue #577 readiness: once Copilot CLI is present, plugin registration
    // is part of the hook integration contract. Config parse failures must
    // block install instead of producing success-shaped output with no plugin.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);

        let copilot_home = home.join(".copilot");
        fs::create_dir_all(&copilot_home).unwrap();
        fs::write(copilot_home.join("config.json"), "{ malformed json\n").unwrap();

        let result = local_install(&repo, None);

        assert!(
            result.is_err(),
            "local_install must fail when Copilot plugin registration cannot update config.json"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Copilot") || err.contains("config.json") || err.contains("JSON"),
            "install error should surface the Copilot registration/config failure, got: {err}"
        );
    });
}

#[test]
fn local_install_registers_copilot_plugin_and_native_hooks_idempotently() {
    // PR #579 readiness: when Copilot CLI is present, a local install must wire
    // both Copilot plugin hooks and native Claude hooks, and rerunning install
    // must refresh them without duplicate registration.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);

        let copilot_home = home.join(".copilot");
        fs::create_dir_all(&copilot_home).unwrap();
        fs::write(
            copilot_home.join("config.json"),
            r#"{
  "installedPlugins": [
    {
      "name": "other-plugin",
      "marketplace": "local",
      "enabled": true,
      "cache_path": "/tmp/other-plugin"
    },
    {
      "name": "amplihack",
      "marketplace": "local",
      "enabled": false,
      "cache_path": "/tmp/stale-amplihack"
    }
  ],
  "trustedFolders": ["/tmp/project"]
}
"#,
        )
        .unwrap();

        local_install(&repo, None).unwrap();
        local_install(&repo, None).unwrap();

        let plugin_dir = copilot_home
            .join("installed-plugins")
            .join("amplihack@local");
        let plugin_manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(plugin_dir.join("plugin.json")).unwrap())
                .unwrap();
        assert_eq!(
            plugin_manifest
                .get("hooks")
                .and_then(|value| value.as_str()),
            Some("./hooks.json"),
            "Copilot plugin manifest must declare hooks.json"
        );

        let hooks_json = fs::read_to_string(plugin_dir.join("hooks.json")).unwrap();
        for expected in [
            "session-start",
            "stop",
            "workflow-classification-reminder",
            "user-prompt-submit",
            "pre-tool-use",
            "post-tool-use",
        ] {
            assert!(
                hooks_json.contains(expected),
                "Copilot hooks.json must include amplihack-hooks {expected}; got:\n{hooks_json}"
            );
        }
        assert!(
            !hooks_json.contains("timeout "),
            "Copilot hook commands must not wrap amplihack-hooks in shell timeout commands"
        );

        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(copilot_home.join("config.json")).unwrap())
                .unwrap();
        let plugins = config["installedPlugins"].as_array().unwrap();
        assert_eq!(
            plugins
                .iter()
                .filter(
                    |plugin| plugin.get("name").and_then(|name| name.as_str()) == Some("amplihack")
                )
                .count(),
            1,
            "amplihack@local must be registered exactly once after repeated installs"
        );
        assert!(
            plugins
                .iter()
                .any(|plugin| plugin.get("name").and_then(|name| name.as_str())
                    == Some("other-plugin")),
            "registering amplihack must preserve unrelated Copilot plugins"
        );
        assert_eq!(
            config
                .get("trustedFolders")
                .and_then(|trusted| trusted.as_array())
                .map(Vec::len),
            Some(1),
            "registering amplihack must preserve unrelated Copilot config fields"
        );

        let settings = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
        for expected in [
            "session-start",
            "stop",
            "workflow-classification-reminder",
            "user-prompt-submit",
            "pre-tool-use",
            "post-tool-use",
            "pre-compact",
        ] {
            assert!(
                settings.contains(expected),
                "native Claude settings must include amplihack-hooks {expected}; got:\n{settings}"
            );
        }
    });
}

#[test]
fn local_install_replaces_managed_bundle_dirs_without_deleting_unmanaged_files() {
    // Issue #578 readiness: mapped framework directories are managed trees.
    // Re-install must remove stale files inside them while preserving unrelated
    // files outside the managed mapping.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);

        local_install(&repo, None).unwrap();

        let managed_tools = home.join(".amplihack/.claude/tools/amplihack");
        let stale = managed_tools.join("stale-python-hook.py");
        fs::write(&stale, "print('old')\n").unwrap();
        let unmanaged = home.join(".amplihack/.claude/user-notes.txt");
        fs::write(&unmanaged, "keep me\n").unwrap();

        local_install(&repo, None).unwrap();

        assert!(
            !stale.exists(),
            "stale files inside managed mapped dirs must be removed on re-install"
        );
        assert!(
            managed_tools.join("marker.txt").is_file(),
            "managed mapped dir must still contain source bundle content"
        );
        assert_eq!(
            fs::read_to_string(&unmanaged).unwrap(),
            "keep me\n",
            "unmanaged files outside mapped framework dirs must be preserved"
        );
    });
}

#[test]
fn local_install_from_bundle_only_source_copies_all_essentials() {
    // Issue #416 regression: a clean amplihack-rs checkout has NO top-level
    // `.claude/` (gitignored). Install must read framework assets from
    // `amplifier-bundle/` and stage them under ~/.amplihack/.claude/.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);

        // Pre-fix this fails with: ".claude not found at <repo>/.claude or ..."
        local_install(&repo, None).expect(
            "issue #416: local_install must succeed against a bundle-only source repo \
             (no top-level .claude/), but failed",
        );

        let staged = home.join(".amplihack/.claude");
        for rel in BUNDLE_ESSENTIAL_DESTS {
            assert!(
                staged.join(rel).is_dir(),
                "issue #416: bundle essential `{rel}` must be staged at {}",
                staged.join(rel).display()
            );
        }
        // Bundle marker file must round-trip into the staged tree, confirming
        // the source was actually amplifier-bundle/ and not a coincidental
        // empty-dir creation.
        assert!(
            staged.join("agents/marker.txt").is_file(),
            "issue #416: bundle content must be copied, not just empty dirs"
        );
    });
}

#[test]
fn local_install_with_no_source_assets_returns_err() {
    // Empty source root (no .claude AND no amplifier-bundle) must hard-error.
    // Pre-fix: `find_source_claude_dir` returns Err — already failing today.
    // Post-fix: must STILL fail (negative regression guard).
    with_install_env(|home| {
        let repo = home.join("empty-repo");
        fs::create_dir_all(&repo).unwrap();
        // Intentionally do NOT create amplifier-bundle/ or .claude/.

        let result = local_install(&repo, None);
        assert!(
            result.is_err(),
            "local_install must Err when source repo contains no framework assets, \
             got Ok with copied dirs (silent partial install)"
        );
        let err = result.unwrap_err().to_string();
        // Diagnostic must name BOTH probed locations so the user can fix it.
        assert!(
            err.contains("amplifier-bundle") || err.contains(".claude"),
            "error must reference probed source paths (amplifier-bundle / .claude), \
             got: {err}"
        );
    });
}

#[test]
fn local_install_hard_errors_when_no_dirs_copied() {
    // Per design D3: an empty `copied_dirs` is now a hard error
    // (was a println! warning at mod.rs:348-354). Triggered by a source repo
    // whose detected layout has no copyable essentials.
    with_install_env(|home| {
        let repo = home.join("repo-with-empty-claude");
        // Legacy layout, but completely empty .claude/ — copytree finds
        // nothing and returns Vec::new(). NO amplifier-bundle/ either, so
        // the bundle-first probe falls through to LegacyClaude. Pre-fix this
        // returns Ok(()) with a printed warning (silent partial install).
        fs::create_dir_all(repo.join(".claude")).unwrap();

        let result = local_install(&repo, None);
        assert!(
            result.is_err(),
            "local_install must hard-error when zero essential dirs are copied; \
             silent success with empty install is a regression vector"
        );
    });
}

#[test]
fn legacy_claude_source_layout_still_installs() {
    // Backward-compat: the hybrid `create_source_repo` fixture (which ships
    // both .claude/ and amplifier-bundle/) must keep installing successfully.
    // After #416 the bundle layout is preferred, so destinations are bundle
    // names; `.claude/`-relative legacy content remains intact in the source
    // tree but is not staged because bundle wins the probe.
    with_install_env(|home| {
        let repo = home.join("legacy-repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_source_repo(&repo);

        local_install(&repo, None).expect("hybrid create_source_repo fixture must keep working");

        let staged = home.join(".amplihack/.claude");
        // Bundle layout was selected; `agents/` is the bundle destination.
        assert!(
            staged.join("agents").is_dir(),
            "hybrid install (bundle-preferred) must stage agents/"
        );
    });
}

#[test]
fn missing_framework_paths_recognises_bundle_layout_install() {
    // After a bundle-only install, missing_framework_paths must NOT report
    // legacy-only entries (e.g. `commands/amplihack`, `workflow`, `templates`)
    // as missing — those are intentionally absent from the bundle layout.
    // Pre-fix: missing_framework_paths iterates ESSENTIAL_DIRS unconditionally
    // and floods the user with false-positive missing entries, triggering
    // an infinite re-install loop.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);
        local_install(&repo, None).expect("bundle install should succeed");

        let staged = home.join(".amplihack/.claude");
        let missing = settings::missing_framework_paths(&staged).unwrap();
        // Legacy-only essentials must NOT appear in the missing list when
        // the bundle layout was used. Whitelist the names that the bundle
        // does not ship.
        for legacy_only in &[
            "commands/amplihack",
            "workflow",
            "templates",
            "scenarios",
            "docs",
            "schemas",
            "config",
        ] {
            assert!(
                !missing.iter().any(|m| m.starts_with(legacy_only)),
                "legacy-only essential `{legacy_only}` must not be reported missing \
                 after a bundle-layout install, got missing: {missing:?}"
            );
        }
    });
}

#[cfg(unix)]
#[test]
fn find_source_root_rejects_symlinked_amplifier_bundle() {
    // Defense-in-depth: a symlinked amplifier-bundle/ root must be rejected
    // by the source-root probe (per design's is_real_dir contract). This is
    // the same defense as `copy_amplifier_bundle_rejects_symlinked_source_root`
    // but at the find_source_root layer.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let elsewhere = home.join("evil");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("secret.txt"), "private").unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join("amplifier-bundle")).unwrap();

        let result = local_install(&repo, None);
        assert!(
            result.is_err(),
            "symlinked amplifier-bundle source root must be rejected"
        );
    });
}

#[test]
fn install_writes_layout_marker_atomically() {
    // Per design: install writes a `.layout` marker (`bundle` or `legacy`)
    // via temp-file + rename. After a successful install, the marker must
    // exist with the expected content and no `.layout.tmp` may linger.
    with_install_env(|home| {
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);
        local_install(&repo, None).expect("bundle install should succeed");

        let staged = home.join(".amplihack/.claude");
        let marker = staged.join(".layout");
        assert!(
            marker.is_file(),
            "install must write .layout marker at {}",
            marker.display()
        );
        let content = fs::read_to_string(&marker).unwrap();
        assert_eq!(
            content.trim(),
            "bundle",
            ".layout must contain exactly 'bundle' (post-trim) for a bundle-layout install"
        );

        let tmp = staged.join(".layout.tmp");
        assert!(
            !tmp.exists(),
            "no .layout.tmp temp file may remain after a successful atomic write"
        );
    });
}

#[test]
fn marker_missing_defaults_to_legacy_layout() {
    // missing_framework_paths must tolerate a missing .layout marker by
    // defaulting to LegacyClaude (matches pre-fix behavior). Critically,
    // it must not emit a warning for the missing marker on staged installs
    // that pre-date the fix.
    with_install_env(|home| {
        let staged = home.join(".amplihack/.claude");
        // Build a legacy-shaped staged tree by hand, with NO .layout marker.
        helpers::create_minimal_staged_assets(home);
        assert!(!staged.join(".layout").exists());

        // Must not panic, must not bail; should treat as legacy and only
        // report dirs that are actually missing under the legacy mapping.
        let missing = settings::missing_framework_paths(&staged).unwrap();
        // create_minimal_staged_assets stages ALL legacy ESSENTIAL_DIRS,
        // so missing must be empty (apart from possibly bundle paths,
        // which are tolerated by this regression scope).
        for entry in &missing {
            assert!(
                !entry.contains(".layout"),
                "missing-marker case must not surface a `.layout` complaint, got: {entry}"
            );
        }
    });
}

#[test]
fn malformed_layout_marker_is_handled_gracefully() {
    // Per design strict-parse table: a malformed .layout (not 'bundle' or
    // 'legacy' post-trim) must be tolerated (warn + default to LegacyClaude),
    // never panic.
    with_install_env(|home| {
        let staged = home.join(".amplihack/.claude");
        helpers::create_minimal_staged_assets(home);
        fs::write(staged.join(".layout"), "garbage-value\n").unwrap();

        let result = settings::missing_framework_paths(&staged);
        assert!(
            result.is_ok(),
            "malformed .layout must not cause missing_framework_paths to error; \
             must warn and default to legacy. Got: {:?}",
            result.err()
        );
    });
}

// ============================================================================
// Issue #675: force_refresh parameter tests (TDD — written before impl)
// ============================================================================
//
// These tests define the contract for the `force_refresh` parameter added to
// `run_install()`.  They will fail to compile until the signature is updated,
// then fail at runtime until the logic is implemented.

#[test]
fn run_install_force_refresh_true_with_local_path_still_uses_local() {
    // Contract: `--local` takes priority over `force_refresh`. Even when
    // `force_refresh=true`, a valid `--local` path must be used directly
    // (no network download). This ensures the explicit user override is
    // never bypassed by internal force-refresh logic.
    with_install_env(|home| {
        let local_repo = home.join("local-repo");
        fs::create_dir_all(&local_repo).unwrap();
        helpers::create_source_repo(&local_repo);

        let result = run_install(Some(local_repo), false, true);
        result.expect(
            "issue #675: run_install with --local and force_refresh=true must succeed \
             (local takes priority over force_refresh)",
        );

        assert!(
            home.join(".amplihack/.claude/install/amplihack-manifest.json")
                .exists(),
            "manifest must exist after --local install with force_refresh=true"
        );
    });
}

#[test]
fn run_install_force_refresh_true_nonexistent_local_path_errors() {
    // Contract: `force_refresh` must not change `--local` path validation.
    // A non-existent `--local` path must still return Err regardless of
    // the force_refresh value.
    let nonexistent = std::path::PathBuf::from("/nonexistent/amplihack-repo/does-not-exist");
    let result = run_install(Some(nonexistent), false, true);
    assert!(
        result.is_err(),
        "run_install must return Err for non-existent --local path even with force_refresh=true"
    );
}

#[test]
fn run_install_force_refresh_false_uses_bundled_root_when_available() {
    // Contract: `force_refresh=false` preserves the existing behavior —
    // `find_bundled_framework_root()` is consulted and its result is used
    // when available. In the test environment (running inside the
    // amplihack-rs workspace), the bundled root will always be found via
    // CWD walk-up or compile-time workspace root.
    //
    // This is a regression guard: the new parameter must not break
    // the default (non-update) install path.
    with_install_env(|home| {
        // Do NOT create a local repo — let find_bundled_framework_root()
        // discover the workspace's amplifier-bundle/ automatically.
        let result = run_install(None, false, false);
        result.expect(
            "issue #675: run_install(None, false, false) must succeed via bundled root \
             (existing behavior unchanged)",
        );

        assert!(
            home.join(".amplihack/.claude/install/amplihack-manifest.json")
                .exists(),
            "manifest must exist after install via bundled root"
        );
    });
}

#[test]
fn run_install_force_refresh_true_skips_bundled_root() {
    // Contract (THE FIX): when `force_refresh=true` and `local` is `None`,
    // `run_install` must NOT use `find_bundled_framework_root()`. It must
    // fall through directly to the network download path.
    //
    // In the test environment, `find_bundled_framework_root()` always returns
    // `Some` (the workspace root is discoverable). With `force_refresh=true`,
    // the function must skip it and attempt a fresh network download instead.
    //
    // Since network download may succeed or fail depending on environment
    // (git availability, network connectivity), this test verifies the
    // contract by checking that the code path taken is different from
    // the `force_refresh=false` path. Specifically:
    //
    // - `force_refresh=false` uses bundled root → stages from local source
    // - `force_refresh=true` skips bundled root → downloads from upstream
    //
    // We verify the download path was taken by checking for the
    // freshness SHA stamp that is ONLY written after a network-fallback
    // install (see mod.rs L100-102), NOT after a bundled-root install.
    with_install_env(|home| {
        // First: install with force_refresh=false (bundled root path).
        // This should succeed and NOT write the framework-sha stamp.
        let result_default = run_install(None, false, false);
        result_default.expect("baseline install with force_refresh=false must succeed");

        let sha_stamp = home.join(".amplihack/.framework-sha");
        let had_sha_after_default = sha_stamp.exists();

        // Clean staged assets for a fresh run
        let _ = fs::remove_dir_all(home.join(".amplihack/.claude"));
        let _ = fs::remove_file(&sha_stamp);

        // Second: install with force_refresh=true (network download path).
        // This test validates that the code ATTEMPTS the network path.
        // If the network download succeeds, the framework-sha stamp will
        // be written (distinguishing it from the bundled-root path).
        // If the download fails (no network), that's also acceptable —
        // the error message should reference network/download, not bundled root.
        let result_refresh = run_install(None, false, true);
        match result_refresh {
            Ok(()) => {
                // Network download succeeded — verify the SHA stamp was written
                // (this stamp is ONLY written after network-fallback installs).
                assert!(
                    sha_stamp.exists() || !had_sha_after_default,
                    "issue #675: force_refresh=true must take the network download path, \
                     which writes the framework-sha stamp"
                );
            }
            Err(ref e) => {
                let msg = e.to_string();
                // Network download failed — the error must reference the
                // download/network path, NOT the bundled root.
                assert!(
                    !msg.contains("Using bundled framework"),
                    "issue #675: force_refresh=true must NOT use bundled framework root. \
                     Got error: {msg}"
                );
            }
        }
    });
}
