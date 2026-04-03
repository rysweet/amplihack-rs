use super::*;
use std::fs;
use tempfile::TempDir;

// ── get_shared_runtime_dir tests ────────────────────────────────────────

#[test]
fn returns_default_runtime_dir_for_non_git_directory() {
    let tmp = TempDir::new().unwrap();
    clear_cache();
    let result = get_shared_runtime_dir(tmp.path()).unwrap();
    let expected = tmp.path().join(".claude").join("runtime");
    assert_eq!(result, expected.to_string_lossy());
    assert!(expected.exists(), "runtime dir should be created");
}

#[test]
fn runtime_dir_has_owner_only_permissions() {
    let tmp = TempDir::new().unwrap();
    clear_cache();
    let result_path = get_shared_runtime_dir(tmp.path()).unwrap();
    let path = std::path::Path::new(&result_path);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "runtime dir should be chmod 0o700");
    }
}

#[test]
fn env_override_valid_path_under_home() {
    // We test the validation function directly to avoid unsafe set_var.
    if let Some(home) = dirs_home() {
        let test_path = home.join("some-runtime-dir");
        assert!(
            validate_env_runtime_dir(&test_path).is_ok(),
            "path under HOME should be accepted"
        );
    }
}

#[test]
fn env_override_rejects_path_outside_allowed_roots() {
    let result = validate_env_runtime_dir(Path::new("/etc/shadow-runtime"));
    assert!(
        result.is_err(),
        "should reject paths outside home and /tmp"
    );
}

#[test]
fn empty_project_root_still_works() {
    let tmp = TempDir::new().unwrap();
    clear_cache();
    let result = get_shared_runtime_dir(tmp.path());
    assert!(result.is_ok(), "should succeed for any valid directory");
}

#[test]
fn caching_returns_same_result() {
    let tmp = TempDir::new().unwrap();
    clear_cache();
    let first = get_shared_runtime_dir(tmp.path()).unwrap();
    let second = get_shared_runtime_dir(tmp.path()).unwrap();
    assert_eq!(first, second, "cached result should match");
}

#[test]
fn clear_cache_works() {
    let tmp = TempDir::new().unwrap();
    clear_cache();
    let _ = get_shared_runtime_dir(tmp.path()).unwrap();
    clear_cache();
    // After clearing, the cache lookup should miss.
    let canonical = fs::canonicalize(tmp.path())
        .unwrap_or_else(|_| tmp.path().to_path_buf());
    assert!(
        cache_get(&canonical).is_none(),
        "cache should be empty after clear"
    );
}

#[test]
fn git_repo_returns_local_runtime_dir() {
    let tmp = TempDir::new().unwrap();
    clear_cache();
    // Initialize a real git repo.
    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output();
    if init.is_err() {
        // git not available — skip.
        return;
    }
    let result = get_shared_runtime_dir(tmp.path()).unwrap();
    let expected = fs::canonicalize(tmp.path())
        .unwrap_or_else(|_| tmp.path().to_path_buf())
        .join(".claude")
        .join("runtime")
        .to_string_lossy()
        .to_string();
    assert_eq!(result, expected);
}

#[test]
fn validate_env_runtime_dir_accepts_home() {
    if let Some(home) = dirs_home() {
        let test_path = home.join("some-dir");
        assert!(validate_env_runtime_dir(&test_path).is_ok());
    }
}

#[test]
fn validate_env_runtime_dir_rejects_etc() {
    let result = validate_env_runtime_dir(Path::new("/etc/bad-dir"));
    assert!(result.is_err());
}

#[test]
fn resolve_runtime_path_falls_back_on_non_git() {
    let tmp = TempDir::new().unwrap();
    let default = tmp.path().join("default");
    let result = resolve_runtime_path(tmp.path(), &default);
    assert_eq!(result, default);
}
