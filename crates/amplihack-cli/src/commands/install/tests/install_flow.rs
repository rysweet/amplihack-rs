use super::*;
use super::helpers::*;
use std::fs;

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
    local_install(temp.path()).unwrap();

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
    local_install(temp.path()).unwrap();

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

    let result = run_install(Some(local_repo));

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
    let result = run_install(Some(nonexistent));
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
