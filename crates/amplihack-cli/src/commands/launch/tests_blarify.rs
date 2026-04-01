use super::*;
use crate::test_support::{home_env_lock, restore_home, set_home};
use std::fs;

#[test]
fn maybe_prompt_re_enable_power_steering_removes_disabled_file_on_yes_default() {
    let project = tempfile::tempdir().unwrap();
    let disabled_file = project
        .path()
        .join(".claude/runtime/power-steering/.disabled");
    fs::create_dir_all(disabled_file.parent().unwrap()).unwrap();
    fs::write(&disabled_file, "").unwrap();

    maybe_prompt_re_enable_power_steering_with(project.path(), |_prompt, _timeout| Ok(None))
        .unwrap();

    assert!(!disabled_file.exists());
}

#[test]
fn maybe_prompt_re_enable_power_steering_keeps_disabled_file_on_no() {
    let project = tempfile::tempdir().unwrap();
    let disabled_file = project
        .path()
        .join(".claude/runtime/power-steering/.disabled");
    fs::create_dir_all(disabled_file.parent().unwrap()).unwrap();
    fs::write(&disabled_file, "").unwrap();

    maybe_prompt_re_enable_power_steering_with(project.path(), |_prompt, _timeout| {
        Ok(Some("n".to_string()))
    })
    .unwrap();

    assert!(disabled_file.exists());
}

#[test]
fn parse_github_repo_uri_accepts_supported_formats() {
    assert_eq!(
        parse_github_repo_uri("owner/repo").unwrap(),
        ("owner".to_string(), "repo".to_string())
    );
    assert_eq!(
        parse_github_repo_uri("https://github.com/owner/repo.git").unwrap(),
        ("owner".to_string(), "repo".to_string())
    );
    assert_eq!(
        parse_github_repo_uri("git@github.com:owner/repo.git").unwrap(),
        ("owner".to_string(), "repo".to_string())
    );
    assert!(parse_github_repo_uri("https://example.com/owner/repo").is_err());
}

#[test]
#[cfg(unix)]
fn resolve_checkout_repo_in_uses_git_clone_stub() {
    use std::os::unix::fs::PermissionsExt;

    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path().join("bin");
    let base_dir = temp.path().join("checkouts");
    fs::create_dir_all(&bin_dir).unwrap();
    let git_path = bin_dir.join("git");
    fs::write(
        &git_path,
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  /bin/mkdir -p \"$3/.git\"\n  exit 0\nfi\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&git_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&git_path, permissions).unwrap();

    let previous_path = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", &bin_dir) };

    let checkout = resolve_checkout_repo_in("owner/repo", &base_dir).unwrap();

    match previous_path {
        Some(value) => unsafe { std::env::set_var("PATH", value) },
        None => unsafe { std::env::remove_var("PATH") },
    }

    assert_eq!(checkout, base_dir.join("owner-repo"));
    assert!(checkout.join(".git").is_dir());
}

#[test]
fn should_prompt_blarify_indexing_only_for_interactive_claude_opt_in() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    unsafe { std::env::set_var("AMPLIHACK_ENABLE_BLARIFY", "1") };
    assert!(should_prompt_blarify_indexing("claude", false));
    assert!(!should_prompt_blarify_indexing("copilot", false));
    assert!(!should_prompt_blarify_indexing("claude", true));
    unsafe { std::env::remove_var("AMPLIHACK_ENABLE_BLARIFY") };
    assert!(!should_prompt_blarify_indexing("claude", false));
}

#[test]
fn should_allow_noninteractive_blarify_when_mode_is_explicit() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    unsafe {
        std::env::set_var("AMPLIHACK_ENABLE_BLARIFY", "1");
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
    }
    assert!(should_prompt_blarify_indexing("claude", true));
    unsafe {
        std::env::remove_var("AMPLIHACK_ENABLE_BLARIFY");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }
}

#[test]
fn maybe_run_blarify_indexing_prompt_surfaces_prompt_failure() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = tempfile::tempdir().unwrap();

    unsafe {
        std::env::set_var("AMPLIHACK_ENABLE_BLARIFY", "1");
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
    }

    let result =
        maybe_run_blarify_indexing_prompt_with("claude", true, Some(project.path()), |_path| {
            Err(anyhow::anyhow!("synthetic prompt failure"))
        });

    unsafe {
        std::env::remove_var("AMPLIHACK_ENABLE_BLARIFY");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let error = result.expect_err("prompt failure should stop launch");
    let error_message = error.to_string();
    let error_chain = format!("{error:#}");
    assert!(
        error_message.contains("code graph indexing prompt failed for"),
        "expected launch-side prompt failure context, got: {error_message}"
    );
    assert!(
        error_chain.contains("synthetic prompt failure"),
        "expected root-cause prompt failure, got: {error_chain}"
    );
    assert!(
        error_chain.contains(&project.path().display().to_string()),
        "expected project path in error chain, got: {error_chain}"
    );
}

#[test]
fn parse_blarify_prompt_choice_matches_supported_inputs() {
    assert_eq!(
        parse_blarify_prompt_choice(Some("y")),
        BlarifyPromptChoice::Foreground
    );
    assert_eq!(
        parse_blarify_prompt_choice(Some("background")),
        BlarifyPromptChoice::Background
    );
    assert_eq!(
        parse_blarify_prompt_choice(Some("skip")),
        BlarifyPromptChoice::Never
    );
    assert_eq!(parse_blarify_prompt_choice(None), BlarifyPromptChoice::Skip);
}

#[test]
fn blarify_mode_parses_supported_values() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "sync") };
    assert_eq!(blarify_mode(), BlarifyMode::Sync);
    unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background") };
    assert_eq!(blarify_mode(), BlarifyMode::Background);
    unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip") };
    assert_eq!(blarify_mode(), BlarifyMode::Skip);
    unsafe { std::env::remove_var("AMPLIHACK_BLARIFY_MODE") };
    assert_eq!(blarify_mode(), BlarifyMode::Prompt);
}

#[test]
fn resolve_blarify_index_action_prefers_import_for_current_json() {
    let dir = tempfile::tempdir().unwrap();
    let json_path = dir.path().join(".amplihack").join("blarify.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(&json_path, "{}\n").unwrap();
    let status = crate::commands::memory::IndexStatus {
        needs_indexing: false,
        reason: "up-to-date".to_string(),
        estimated_files: 1,
        last_indexed: None,
    };

    assert_eq!(
        resolve_blarify_index_action(&status, &json_path),
        BlarifyIndexAction::ImportExistingJson
    );
}

#[test]
fn resolve_blarify_index_action_prefers_native_scip_for_stale_json() {
    let dir = tempfile::tempdir().unwrap();
    let json_path = dir.path().join(".amplihack").join("blarify.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(&json_path, "{}\n").unwrap();
    let status = crate::commands::memory::IndexStatus {
        needs_indexing: true,
        reason: "stale".to_string(),
        estimated_files: 3,
        last_indexed: None,
    };

    assert_eq!(
        resolve_blarify_index_action(&status, &json_path),
        BlarifyIndexAction::GenerateNativeScip
    );
}

#[test]
fn consent_cache_round_trip_persists_per_project() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let previous_home = set_home(home.path());

    assert!(!has_blarify_consent(project.path()).unwrap());
    save_blarify_consent(project.path()).unwrap();
    assert!(has_blarify_consent(project.path()).unwrap());

    let consent_path = consent_cache_path(project.path()).unwrap();
    assert!(consent_path.exists());

    restore_home(previous_home);
}
