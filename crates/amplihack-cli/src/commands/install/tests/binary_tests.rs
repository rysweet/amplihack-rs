use super::helpers::create_exe_stub;
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

// ─── TDD: Group 5 — find_hooks_binary resolution ─────────────────────────

#[test]
fn find_hooks_binary_uses_env_var_override_when_set() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let fake_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let prev = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &fake_bin);
    }

    let result = binary::find_hooks_binary();

    if let Some(v) = prev {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }

    let resolved = result.expect("find_hooks_binary must resolve via env-var override");
    assert_eq!(
        resolved, fake_bin,
        "must return the exact path from env var"
    );
}

#[test]
fn find_hooks_binary_errors_when_env_var_path_nonexistent() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let nonexistent = temp.path().join("does-not-exist");

    let prev = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &nonexistent);
    }

    let result = binary::find_hooks_binary();

    if let Some(v) = prev {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }

    assert!(
        result.is_err(),
        "find_hooks_binary must return an error when env var path does not exist"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH"),
        "error message must mention the env var name; got: {msg}"
    );
}

// ─── TDD: Group 6 — validate_hook_command_string ─────────────────────────

#[test]
fn validate_hook_command_string_rejects_pipe() {
    assert!(
        binary::validate_hook_command_string("/home/user/amplihack-hooks | evil").is_err(),
        "must reject pipe metacharacter"
    );
}

#[test]
fn validate_hook_command_string_rejects_semicolon() {
    assert!(
        binary::validate_hook_command_string("/home/user/amplihack-hooks; rm -rf /").is_err(),
        "must reject semicolon"
    );
}

#[test]
fn validate_hook_command_string_rejects_dollar_sign() {
    assert!(
        binary::validate_hook_command_string("/home/user/amplihack-hooks $HOME").is_err(),
        "must reject dollar-sign variable expansion"
    );
}

#[test]
fn validate_hook_command_string_rejects_backtick() {
    assert!(
        binary::validate_hook_command_string("/home/user/amplihack-hooks `id`").is_err(),
        "must reject backtick"
    );
}

#[test]
fn validate_hook_command_string_accepts_valid_binary_subcmd() {
    assert!(
        binary::validate_hook_command_string("/home/user/.local/bin/amplihack-hooks session-start")
            .is_ok(),
        "must accept plain binary + subcommand"
    );
}

#[test]
fn validate_hook_command_string_accepts_valid_python_path() {
    assert!(
        binary::validate_hook_command_string(
            "/home/user/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py"
        )
        .is_ok(),
        "must accept absolute Python file path"
    );
}

// ─── TDD: Group 7 — deploy_binaries ──────────────────────────────────────

#[test]
fn deploy_binaries_copies_hooks_binary_to_local_bin_with_755_perms() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let hooks_stub = create_exe_stub(temp.path(), "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    }

    let result = binary::deploy_binaries();

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    crate::test_support::restore_home(previous);

    let deployed = result.expect("deploy_binaries must succeed");
    assert!(!deployed.is_empty(), "must return deployed paths");

    let dst = temp.path().join(".local/bin/amplihack-hooks");
    assert!(dst.exists(), "amplihack-hooks must be at ~/.local/bin");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dst).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "deployed binary must have 0o755 perms, got {:03o}",
            mode & 0o777
        );
    }
}

#[test]
fn deploy_binaries_succeeds_when_local_bin_not_in_path() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let hooks_stub = create_exe_stub(temp.path(), "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        std::env::set_var("PATH", "/usr/bin:/bin"); // ~/.local/bin intentionally absent
    }

    let result = binary::deploy_binaries();

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    assert!(
        result.is_ok(),
        "deploy_binaries must exit 0 (warning only) even when ~/.local/bin absent from PATH"
    );
}

// ─── Issue #885 — self-source resolution & tolerant self-deploy ───────────

#[test]
fn strip_deleted_suffix_strips_linux_marker() {
    // Linux reports a swapped-out running binary as "<path> (deleted)".
    let stripped =
        binary::strip_deleted_suffix(Path::new("/home/u/.cargo/bin/amplihack (deleted)"));
    assert_eq!(
        stripped,
        Some(PathBuf::from("/home/u/.cargo/bin/amplihack")),
        "must strip the trailing ' (deleted)' marker"
    );
}

#[test]
fn strip_deleted_suffix_returns_none_without_marker() {
    assert!(
        binary::strip_deleted_suffix(Path::new("/home/u/.cargo/bin/amplihack")).is_none(),
        "a normal path has no ' (deleted)' marker to strip"
    );
}

#[test]
fn resolve_self_source_from_returns_existing_path_directly() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = create_exe_stub(tmp.path(), "amplihack");

    let resolved = binary::resolve_self_source_from(&exe);
    assert_eq!(
        resolved,
        Some(exe),
        "an existing current_exe() must be used as-is"
    );
}

#[test]
fn resolve_self_source_from_recovers_deleted_suffix_binary() {
    // Issue #885: after `amplihack update` swaps the binary in place, the
    // running process reports "<path> (deleted)" while the freshly-written
    // replacement binary is at "<path>". Resolution must recover the real file.
    let tmp = tempfile::tempdir().unwrap();
    let real = create_exe_stub(tmp.path(), "amplihack");
    let deleted = PathBuf::from(format!("{} (deleted)", real.display()));

    let resolved = binary::resolve_self_source_from(&deleted);
    assert_eq!(
        resolved,
        Some(real),
        "must strip ' (deleted)' and resolve the freshly-written binary"
    );
}

#[test]
fn resolve_self_source_from_falls_back_to_path_when_unrecoverable() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    // amplihack is on PATH but NOT at the (deleted) location.
    let path_bin = tmp.path().join("path_bin");
    let path_stub = create_exe_stub(&path_bin, "amplihack");
    let unrecoverable = tmp.path().join("gone").join("amplihack (deleted)");

    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("PATH", &path_bin);
    }

    let resolved = binary::resolve_self_source_from(&unrecoverable);

    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }

    assert_eq!(
        resolved,
        Some(path_stub),
        "must fall back to the amplihack binary found on $PATH"
    );
}

#[test]
fn resolve_self_source_from_returns_none_when_nothing_found() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let unrecoverable = tmp.path().join("gone").join("amplihack (deleted)");

    let prev_path = std::env::var_os("PATH");
    unsafe {
        // Point PATH at an empty dir so find_binary("amplihack") misses.
        std::env::set_var("PATH", tmp.path().join("empty_bin"));
    }

    let resolved = binary::resolve_self_source_from(&unrecoverable);

    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }

    assert!(
        resolved.is_none(),
        "no existing file, no recoverable (deleted) path, and no PATH copy → None"
    );
}

#[test]
fn deploy_self_and_resolver_skips_when_source_none() {
    // When no real amplihack source can be located, the self/resolver deploy
    // is a no-op — the running binary is already installed on PATH. Returning
    // an empty vec (not an error) lets framework-asset staging proceed (#885).
    let tmp = tempfile::tempdir().unwrap();
    let local_bin = tmp.path().join(".local/bin");
    fs::create_dir_all(&local_bin).unwrap();

    let deployed = binary::deploy_self_and_resolver(&local_bin, None);

    assert!(
        deployed.is_empty(),
        "None source must skip the self-copy entirely"
    );
    assert!(
        !local_bin.join("amplihack").exists(),
        "no amplihack binary should be written when the source is unresolvable"
    );
}

#[test]
fn deploy_self_and_resolver_tolerates_missing_source_without_erroring() {
    // A source that does not exist (and whose destination is also absent) makes
    // the underlying copy fail. `deploy_self_and_resolver` must swallow that
    // failure and return an empty vec so the caller (`deploy_binaries` →
    // `run_install`) continues on to stage framework assets (#885).
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let local_bin = tmp.path().join(".local/bin");
    fs::create_dir_all(&local_bin).unwrap();
    let bogus_source = tmp.path().join("nonexistent-amplihack");

    // Point PATH at an empty dir so the resolver PATH-fallback also misses —
    // otherwise a host-installed amplihack-asset-resolver would be deployed.
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("PATH", tmp.path().join("empty_bin"));
    }

    // No panic, no error — just a skipped copy.
    let deployed = binary::deploy_self_and_resolver(&local_bin, Some(bogus_source));

    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }

    assert!(
        deployed.is_empty(),
        "a failed self-copy must be skipped, not recorded as deployed"
    );
    assert!(
        !local_bin.join("amplihack").exists(),
        "the failed copy must not leave a partial destination binary"
    );
}

#[test]
fn deploy_self_and_resolver_copies_valid_source_to_local_bin() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let src_dir = tmp.path().join("cargo_bin");
    let source = create_exe_stub(&src_dir, "amplihack");
    let local_bin = tmp.path().join(".local/bin");
    fs::create_dir_all(&local_bin).unwrap();

    let deployed = binary::deploy_self_and_resolver(&local_bin, Some(source));

    let dst = local_bin.join("amplihack");
    assert!(dst.exists(), "amplihack must be copied into ~/.local/bin");
    assert!(
        deployed.contains(&dst),
        "the deployed amplihack path must be reported for the manifest, got {deployed:?}"
    );
}

// ─── TDD: Group 18 — find_hooks_binary lookup order ──────────────────────

#[test]
fn find_hooks_binary_path_lookup_wins_over_local_bin() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local").join("bin");
    fs::create_dir_all(&local_bin).unwrap();
    create_exe_stub(&local_bin, "amplihack-hooks");

    let path_bin = temp.path().join("path_bin");
    fs::create_dir_all(&path_bin).unwrap();
    let path_stub = create_exe_stub(&path_bin, "amplihack-hooks");

    let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        std::env::set_var("PATH", &path_bin);
    }

    let result = binary::find_hooks_binary();

    if let Some(v) = prev_env {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let resolved = result.expect("find_hooks_binary must find the binary");
    assert_eq!(
        resolved, path_stub,
        "PATH lookup (Step 3) must win — find_hooks_binary returned {resolved:?} instead of {path_stub:?}"
    );
}

#[test]
fn find_hooks_binary_reinstall_after_uninstall_removes_local_bin() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local").join("bin");
    fs::create_dir_all(&local_bin).unwrap();

    let usr_local_bin = temp.path().join("usr_local_bin");
    fs::create_dir_all(&usr_local_bin).unwrap();
    let system_stub = create_exe_stub(&usr_local_bin, "amplihack-hooks");

    let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        std::env::set_var("PATH", &usr_local_bin);
    }

    let result = binary::find_hooks_binary();

    if let Some(v) = prev_env {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let resolved = result.expect(
        "find_hooks_binary must find the binary via PATH even when ~/.local/bin copy was removed by uninstall",
    );
    assert_eq!(
        resolved, system_stub,
        "reinstall must find system binary via PATH — got {resolved:?} instead of {system_stub:?}"
    );
}

#[test]
fn find_hooks_binary_falls_through_to_cargo_bin_when_local_bin_absent() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let local_bin = temp.path().join(".local").join("bin");
    fs::create_dir_all(&local_bin).unwrap();

    let cargo_bin = temp.path().join(".cargo").join("bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    let cargo_stub = create_exe_stub(&cargo_bin, "amplihack-hooks");

    let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        std::env::set_var("PATH", temp.path().join("empty_bin"));
    }

    let result = binary::find_hooks_binary();

    if let Some(v) = prev_env {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let resolved = result.expect("find_hooks_binary must fall through to ~/.cargo/bin");
    assert_eq!(
        resolved, cargo_stub,
        "~/.cargo/bin must be used when ~/.local/bin has no binary"
    );
}

#[test]
fn find_hooks_binary_returns_err_with_helpful_message_when_not_found() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        std::env::set_var("PATH", temp.path().join("empty_bin"));
    }

    let result = binary::find_hooks_binary();

    if let Some(v) = prev_env {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let err = result.expect_err("find_hooks_binary must return Err when binary is absent");
    let msg = format!("{err}");
    assert!(
        msg.contains("amplihack-hooks"),
        "error message must mention 'amplihack-hooks' to guide the user, got: {msg}"
    );
}
