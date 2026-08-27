use super::*;
use crate::binary_finder::BinaryInfo;
use crate::env_builder::EnvBuilder;
use crate::launcher_context::{LauncherKind, read_launcher_context};
use crate::test_support::{home_env_lock, restore_cwd, restore_home, set_cwd, set_home};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn build_command_injects_uvx_plugin_and_project_args_for_claude() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let execution_dir = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    let previous_original_cwd = std::env::var_os("AMPLIHACK_ORIGINAL_CWD");
    unsafe {
        std::env::set_var("UV_PYTHON", "1");
        std::env::remove_var("AMPLIHACK_ORIGINAL_CWD");
    }

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    let cmd = build_command_for_dir(
        &binary,
        false,
        false,
        false,
        &[],
        Some(execution_dir.path()),
        false,
    );
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }
    match previous_original_cwd {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ORIGINAL_CWD", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ORIGINAL_CWD") },
    }

    assert_eq!(args[0], "--plugin-dir");
    assert_eq!(
        args[1],
        home.path()
            .join(".amplihack")
            .join(".claude")
            .display()
            .to_string()
    );
    assert_eq!(args[2], "--add-dir");
    assert_eq!(args[3], execution_dir.path().display().to_string());
    assert_eq!(args[4], "--model");
}

#[test]
fn build_command_prefers_original_cwd_for_staged_uvx_launches() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let execution_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    let previous_original_cwd = std::env::var_os("AMPLIHACK_ORIGINAL_CWD");
    let previous_is_staged = std::env::var_os("AMPLIHACK_IS_STAGED");
    unsafe {
        std::env::set_var("UV_PYTHON", "1");
        std::env::set_var("AMPLIHACK_ORIGINAL_CWD", project_dir.path());
        std::env::set_var("AMPLIHACK_IS_STAGED", "1");
    }

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    let cmd = build_command_for_dir(
        &binary,
        false,
        false,
        false,
        &[],
        Some(execution_dir.path()),
        false,
    );
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }
    match previous_original_cwd {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ORIGINAL_CWD", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ORIGINAL_CWD") },
    }
    match previous_is_staged {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_IS_STAGED", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_IS_STAGED") },
    }

    assert_eq!(args[0], "--plugin-dir");
    assert_eq!(args[2], "--add-dir");
    assert_eq!(args[3], project_dir.path().display().to_string());
}

#[test]
fn build_command_does_not_duplicate_uvx_plugin_or_add_dir_args() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    unsafe { std::env::set_var("UV_PYTHON", "1") };

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    let extra = vec![
        "--plugin-dir".to_string(),
        "/custom/plugin".to_string(),
        "--add-dir".to_string(),
        "/custom/project".to_string(),
    ];
    let cmd = build_command(&binary, false, false, false, &extra);
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }

    // The system-prompt fragment (issue #1265) is compiled into the binary, so
    // it is injected on every claude launch regardless of what is on disk —
    // there is no `$HOME` state that can suppress it. Assert around it rather
    // than against it: this test is about `--plugin-dir` / `--add-dir` not
    // being duplicated, and pinning the fragment's bytes here would make an
    // unrelated wording change fail this test.
    let append = args
        .iter()
        .position(|a| a == "--append-system-prompt")
        .expect("the compiled-in fragment is always injected for claude");
    let mut without_fragment = args.clone();
    without_fragment.drain(append..=append + 1);

    assert_eq!(
        without_fragment,
        vec![
            "--model",
            "opus[1m]",
            "--plugin-dir",
            "/custom/plugin",
            "--add-dir",
            "/custom/project",
        ]
    );
    assert_eq!(
        args.iter().filter(|a| *a == "--plugin-dir").count(),
        1,
        "the user's own --plugin-dir must not be duplicated: {args:?}"
    );
    assert_eq!(
        args.iter().filter(|a| *a == "--add-dir").count(),
        1,
        "the user's own --add-dir must not be duplicated: {args:?}"
    );
}

#[test]
fn augment_claude_launch_env_sets_directory_copy_plugin_root_and_npm_bin() {
    // Issue #1266, Defect 4: the npm-prefix prepend is no longer
    // unconditional, so this test now resolves a target that genuinely lives
    // in the npm prefix. The assertion it makes — that the resolved target's
    // directory leads the child's PATH — is the same one it always made; the
    // old version just could not tell the difference between "amplihack's
    // install is what we are launching" and "amplihack has an install".
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());
    let previous_plugin_installed = std::env::var_os("AMPLIHACK_PLUGIN_INSTALLED");
    unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") };

    let resolved = home.path().join(".npm-global").join("bin").join("claude");
    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", Some(&resolved)).build();

    restore_home(original_home);
    match previous_plugin_installed {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") },
    }

    let expected_plugin_root = home.path().join(".amplihack").join(".claude");
    let expected_plugin_root = expected_plugin_root.display().to_string();
    assert_eq!(
        env.get("CLAUDE_PLUGIN_ROOT").map(String::as_str),
        Some(expected_plugin_root.as_str())
    );
    let path = env.get("PATH").expect("PATH should be populated");
    assert!(
        path.split(':')
            .next()
            .unwrap_or_default()
            .ends_with(".npm-global/bin"),
        "expected ~/.npm-global/bin to be prepended to PATH, got {path}"
    );
}

#[test]
fn augment_claude_launch_env_prefers_installed_plugin_cache_path() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let previous_plugin_installed = std::env::var_os("AMPLIHACK_PLUGIN_INSTALLED");
    unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", "true") };

    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", None).build();

    restore_home(original_home);
    match previous_plugin_installed {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") },
    }

    let expected_plugin_root = home
        .path()
        .join(".claude")
        .join("plugins")
        .join("cache")
        .join("amplihack")
        .join("amplihack")
        .join("0.9.0");
    let expected_plugin_root = expected_plugin_root.display().to_string();
    assert_eq!(
        env.get("CLAUDE_PLUGIN_ROOT").map(String::as_str),
        Some(expected_plugin_root.as_str())
    );
}

#[test]
fn persist_launcher_context_writes_copilot_context_file() {
    let dir = tempfile::tempdir().unwrap();
    let args = vec!["--model".to_string(), "opus".to_string()];

    persist_launcher_context("copilot", Some(dir.path()), &args).unwrap();

    let context = read_launcher_context(dir.path()).unwrap();
    assert_eq!(context.launcher, LauncherKind::Copilot);
    assert_eq!(context.command, "amplihack copilot --model opus");
    assert_eq!(
        context
            .environment
            .get("AMPLIHACK_LAUNCHER")
            .map(String::as_str),
        Some("copilot")
    );
}

/// Issue #506 regression: when launching with `--tool copilot`, the
/// persisted `launcher_context.json` must include
/// `AMPLIHACK_AGENT_BINARY=copilot` in its environment map so nested
/// re-launches (recipe runner sub-recipes, agent tasks) inherit the
/// correct active binary instead of silently falling back to claude.
///
/// The value comes from the launcher being invoked, never from
/// `std::env::var` — the parent may have no preference set even when we
/// know exactly which binary we are launching. The test is intentionally
/// tight: it asserts the exact key and value, so any future "lost in
/// translation" change at this chokepoint surfaces immediately.
///
/// Issue #1335 note: persistence is no longer copilot-only. It records
/// whichever launcher actually ran, so the persisted layer can answer for
/// claude as well. See `persist_launcher_context_records_the_launcher_that_ran`.
#[test]
fn persist_launcher_context_writes_agent_binary_for_copilot() {
    let dir = tempfile::tempdir().unwrap();

    persist_launcher_context("copilot", Some(dir.path()), &[]).unwrap();

    let context = read_launcher_context(dir.path()).unwrap();
    assert_eq!(context.launcher, LauncherKind::Copilot);
    assert_eq!(
        context
            .environment
            .get("AMPLIHACK_AGENT_BINARY")
            .map(String::as_str),
        Some("copilot"),
        "issue #506: persisted launcher context must propagate \
         AMPLIHACK_AGENT_BINARY=copilot so nested launches stay on the \
         copilot binary; got environment={:?}",
        context.environment
    );
    // Defense-in-depth: AMPLIHACK_LAUNCHER must remain alongside the new
    // AMPLIHACK_AGENT_BINARY entry — they are independent contracts and
    // the fix must add, not replace.
    assert_eq!(
        context
            .environment
            .get("AMPLIHACK_LAUNCHER")
            .map(String::as_str),
        Some("copilot"),
        "AMPLIHACK_LAUNCHER must still be present after #506 fix"
    );
}

/// Superseded by issue #1335. This used to assert that a context is written
/// only for copilot, on the reasoning that "sub-launches under
/// claude/codex/amplifier rely on the absence of a stale context file".
///
/// The absence is not what they relied on. With no file, resolution falls
/// through to `DEFAULT_BINARY`, which is *copilot* — so a claude session that
/// lost `AMPLIHACK_AGENT_BINARY` landed on copilot either way. The persisted
/// layer was a one-way voter: it could only ever say copilot, and its absence
/// also meant copilot. There was no path back to claude.
///
/// Downstream was always ready for the symmetric behaviour:
/// `pre_tool_use::launcher::detect_launcher_for_dirs` already maps
/// `LauncherKind::Claude` to `LauncherType::ClaudeCode`. That arm was simply
/// unreachable because nothing wrote it.
///
/// What must still hold is the narrower thing #506 actually needed: a
/// non-launcher subcommand writes nothing.
#[test]
fn persist_launcher_context_writes_nothing_for_non_launcher_subcommands() {
    let dir = tempfile::tempdir().unwrap();

    persist_launcher_context("install", Some(dir.path()), &[]).unwrap();
    persist_launcher_context("doctor", Some(dir.path()), &[]).unwrap();
    persist_launcher_context("recipe", Some(dir.path()), &[]).unwrap();

    assert!(
        read_launcher_context(dir.path()).is_none(),
        "only agent launchers describe a session; a maintenance subcommand \
         must not stamp the repository with one"
    );
}

#[test]
fn build_command_skips_dangerous_flag_for_copilot() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    unsafe { std::env::set_var("UV_PYTHON", "1") };

    let binary = BinaryInfo {
        name: "copilot".to_string(),
        path: PathBuf::from("/usr/bin/copilot"),
        version: None,
    };
    // skip_permissions = true, but tool is copilot → no --dangerously-skip-permissions
    let cmd = build_command(&binary, false, false, true, &[]);
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }

    assert!(
        !args.contains(&"--dangerously-skip-permissions".to_string()),
        "copilot must not receive --dangerously-skip-permissions, got: {args:?}"
    );
    // Copilot should also not get the Claude default model
    assert!(
        !args.iter().any(|a| a == "opus[1m]"),
        "copilot must not get Claude's default model, got: {args:?}"
    );
}

#[test]
fn build_command_injects_dangerous_flag_for_claude() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    unsafe { std::env::set_var("UV_PYTHON", "1") };

    let binary = BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    };
    // skip_permissions = true + tool is claude → should inject
    let cmd = build_command(&binary, false, false, true, &[]);
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    restore_cwd(&original_cwd).unwrap();
    restore_home(original_home);
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }

    assert!(
        args.contains(&"--dangerously-skip-permissions".to_string()),
        "claude should receive --dangerously-skip-permissions, got: {args:?}"
    );
}

// ---------------------------------------------------------------------------
// Defect 4 (issue #1266): the child's PATH must follow the resolved target.
//
// `augment_claude_launch_env` used to prepend `~/.npm-global/bin`
// unconditionally — an amplihack-writable directory placed ahead of the system
// directories for the child session and every subagent and shell-out inside
// it. So even after amplihack selects a healthy `/usr/bin/claude` by absolute
// path, a bare `claude` inside that session re-resolves to whatever is in the
// npm prefix, which is where the stub lives. On a host where
// `~/.npm-global/bin` is already the first PATH entry, that stub shadows the
// working install for every other shell on the machine too.
// ---------------------------------------------------------------------------

fn path_entries(env: &std::collections::HashMap<String, String>) -> Vec<PathBuf> {
    env.get("PATH")
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default()
}

#[test]
fn child_path_leads_with_the_resolved_targets_directory() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());

    let resolved = PathBuf::from("/usr/bin/claude");
    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", Some(&resolved)).build();

    restore_home(original_home);

    let entries = path_entries(&env);
    assert_eq!(
        entries.first().map(PathBuf::as_path),
        Some(Path::new("/usr/bin")),
        "the child's PATH must lead with the directory of the binary amplihack \
         actually resolved, got: {entries:?}"
    );
}

#[test]
fn child_path_does_not_prepend_the_npm_prefix_for_a_non_npm_target() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());

    let resolved = PathBuf::from("/usr/bin/claude");
    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", Some(&resolved)).build();

    restore_home(original_home);

    let npm_bin = home.path().join(".npm-global").join("bin");
    assert!(
        !path_entries(&env).contains(&npm_bin),
        "amplihack must not put its own writable prefix ahead of the system \
         directories when the binary it resolved does not live there"
    );
}

#[test]
fn child_path_prepends_the_npm_prefix_when_that_is_where_the_target_lives() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let npm_bin = home.path().join(".npm-global").join("bin");
    fs::create_dir_all(&npm_bin).unwrap();
    let original_home = set_home(home.path());

    let resolved = npm_bin.join("claude");
    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", Some(&resolved)).build();

    restore_home(original_home);

    assert_eq!(
        path_entries(&env).first(),
        Some(&npm_bin),
        "when the resolved target IS amplihack's own install, its directory \
         leads — that is the one case the old unconditional prepend got right"
    );
}

#[test]
fn child_path_is_untouched_when_nothing_healthy_resolved() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());

    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", None).build();

    restore_home(original_home);

    let npm_bin = home.path().join(".npm-global").join("bin");
    assert!(
        !path_entries(&env).contains(&npm_bin),
        "resolution found nothing healthy, so there is nothing to prefer — \
         prepending the prefix that holds the stub is the worst possible guess"
    );
}

// ---------------------------------------------------------------------------
// C1 / Issue 4 — one owner for "amplihack's npm prefix"
//
// `is_already_reachable` used to hardcode `home.join(".npm-global").join("bin")`
// to identify the directory `launch_target` already classifies authoritatively
// as `TargetSource::AmplihackPrefix`. Four independent spellings, and moving the
// prefix would have broken none of them at compile time — `claude` would just
// quietly stop being reachable in the child. They now share one function; this
// pins that they still do.
// ---------------------------------------------------------------------------

#[test]
fn the_prefix_exemption_tracks_the_one_function_that_owns_the_spelling() {
    let home = tempfile::tempdir().unwrap();
    let owned = amplihack_utils::launch_target::amplihack_prefix_bin(home.path());

    assert!(
        command::is_already_reachable(&owned, home.path()),
        "the directory amplihack installs into must be exempt from the \
         reachability test, whatever it is spelled as"
    );
    assert!(
        !command::is_already_reachable(&home.path().join(".npm-global"), home.path()),
        "the exemption is the bin directory, not the prefix above it"
    );
}

// ---------------------------------------------------------------------------
// F-S2, second half — an empty directory must never reach `prepend_path`
//
// `candidate_paths` filtering relative $PATH entries closes the front door.
// This is the back one: `resolved.and_then(Path::parent)` on a relative
// candidate yields the EMPTY path, `is_already_reachable("")` matches the
// empty $PATH element that produced it, and `prepend_path("")` writes a
// leading colon — putting the current directory at the front of the child's
// $PATH for the agent, every subagent and every shell-out.
//
// Both halves are asserted because they fail independently: a resolved path
// can also arrive from `CLAUDE_BINARY_PATH`, which never passes through
// `candidate_paths` filtering at all.
// ---------------------------------------------------------------------------

#[test]
fn a_relative_resolved_target_does_not_put_the_current_directory_on_the_child_path() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());

    // The hazard has to be IN the environment or this test asserts nothing.
    //
    // `is_already_reachable("")` is true exactly when the ambient `$PATH`
    // carries an empty element, and this host's does not — so before this line
    // the test passed over unfixed code and would only have gone red on a CI
    // runner whose `$PATH` happened to end in a colon. A green test over a live
    // exploit is worse than no test.
    //
    // The real `$PATH` is APPENDED to rather than replaced: several unrelated
    // tests in this crate spawn `git` and `node` by bare name on sibling
    // threads, and they are readers that never take `env_lock`. Adding a
    // trailing colon reproduces the defect without taking `/usr/bin` away from
    // them.
    let original_path = std::env::var_os("PATH");
    let poisoned_path = match &original_path {
        Some(value) => format!("{}:", value.to_string_lossy()),
        None => ":".to_string(),
    };
    // SAFETY: edition 2024 requires unsafe; serialised by `home_env_lock()`.
    unsafe { std::env::set_var("PATH", &poisoned_path) };

    // The bare candidate a stray colon in $PATH produces. Its parent is "".
    let resolved = PathBuf::from("claude");
    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", Some(&resolved)).build();

    // SAFETY: as above.
    unsafe {
        match &original_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }
    restore_home(original_home);

    let entries = path_entries(&env);
    assert!(
        !entries.iter().any(|e| e.as_os_str().is_empty()),
        "an empty PATH entry is the current directory; it must not be \
         prepended to the child's PATH, got: {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e.is_absolute()),
        "every entry amplihack adds to the child's PATH must be absolute, \
         got: {entries:?}"
    );
    let raw = env.get("PATH").cloned().unwrap_or_default();
    assert!(
        !raw.starts_with(':'),
        "a leading colon is cwd-first resolution for git, node and sh, got: {raw:?}"
    );
}

#[test]
fn a_dot_relative_resolved_target_does_not_reach_the_child_path() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
    let original_home = set_home(home.path());

    // `PATH=.:...` — the explicit spelling of the same hazard.
    let resolved = PathBuf::from("./claude");
    let env = augment_claude_launch_env(EnvBuilder::new(), "claude", Some(&resolved)).build();

    restore_home(original_home);

    let entries = path_entries(&env);
    assert!(
        entries.iter().all(|e| e.is_absolute()),
        "a `.`-relative resolved target must not contribute a PATH entry, \
         got: {entries:?}"
    );
}

/// Issue #1335: persistence used to begin `if tool != "copilot" { return }`.
/// That made the persisted layer a one-way voter — it could only ever say
/// "copilot" — and `DEFAULT_BINARY` is copilot too, so a claude session that
/// lost `AMPLIHACK_AGENT_BINARY` could not resolve back to claude by any path.
#[test]
fn persist_launcher_context_records_the_launcher_that_ran() {
    for (tool, expected) in [
        ("claude", LauncherKind::Claude),
        ("copilot", LauncherKind::Copilot),
        ("codex", LauncherKind::Codex),
        ("amplifier", LauncherKind::Amplifier),
    ] {
        let dir = tempfile::tempdir().unwrap();
        persist_launcher_context(tool, Some(dir.path()), &[]).unwrap();

        let context = read_launcher_context(dir.path())
            .unwrap_or_else(|| panic!("{tool}: expected a persisted launcher context"));
        assert_eq!(context.launcher, expected, "{tool}: wrong launcher kind");
        assert_eq!(
            context
                .environment
                .get("AMPLIHACK_AGENT_BINARY")
                .map(String::as_str),
            Some(tool),
            "{tool}: persisted context must name the launcher that ran"
        );
        assert_eq!(
            context
                .environment
                .get("AMPLIHACK_LAUNCHER")
                .map(String::as_str),
            Some(tool)
        );
    }
}

/// A non-launcher subcommand must not stamp the repository at all.
#[test]
fn persist_launcher_context_ignores_non_launcher_subcommands() {
    let dir = tempfile::tempdir().unwrap();
    persist_launcher_context("install", Some(dir.path()), &[]).unwrap();
    assert!(
        read_launcher_context(dir.path()).is_none(),
        "only agent launchers describe a session"
    );
}

/// Issue #1342, the actual field failure: the file that hijacked every
/// workflow under /tmp recorded `"command": "amplihack copilot --version"`. A
/// question-and-exit invocation is not a session and must not claim to be one.
///
/// Only three tokens are listed, and the omissions are deliberate. `--help`
/// and `-h` are intercepted by clap at the subcommand and never reach
/// `extra_args`; `--dry-run` is not a flag any amplihack launcher subcommand
/// defines. Asserting them would advertise coverage for input the CLI cannot
/// produce.
#[test]
fn persist_launcher_context_skips_non_session_invocations() {
    for flag in ["--version", "-V", "help"] {
        let dir = tempfile::tempdir().unwrap();
        persist_launcher_context("copilot", Some(dir.path()), &[flag.to_string()]).unwrap();
        assert!(
            read_launcher_context(dir.path()).is_none(),
            "{flag}: must not persist a session identity"
        );
    }
}

/// A real session with ordinary arguments still persists.
#[test]
fn persist_launcher_context_still_persists_a_real_session() {
    let dir = tempfile::tempdir().unwrap();
    persist_launcher_context(
        "claude",
        Some(dir.path()),
        &["--model".into(), "opus".into()],
    )
    .unwrap();
    let context = read_launcher_context(dir.path()).expect("a real session must persist");
    assert_eq!(context.launcher, LauncherKind::Claude);
}

/// Issue #1342 / crusty B3. `persist_launcher_context` lives in amplihack-cli
/// and `agent_binary::resolve` lives in amplihack-utils, and until now they had
/// never met in a test. That gap is exactly how a fixture emitting `created_at`
/// where the writer emits `timestamp` survived long enough to matter.
///
/// This is the claim the PR title actually makes: a session's own file is
/// honoured by the resolver that reads it.
#[test]
fn a_persisted_context_round_trips_through_the_resolver() {
    let _guard = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for tool in ["claude", "copilot", "codex", "amplifier"] {
        let dir = tempfile::tempdir().unwrap();
        persist_launcher_context(tool, Some(dir.path()), &[]).unwrap();

        // Layers above the file must be silent so the file is what answers.
        let prev = std::env::var_os("AMPLIHACK_AGENT_BINARY");
        let markers = [
            "CLAUDECODE",
            "CLAUDE_CODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_PROJECT_DIR",
            "COPILOT_CLI",
            "GITHUB_COPILOT",
            "GITHUB_COPILOT_AGENT",
            "COPILOT_AGENT",
        ];
        let saved: Vec<_> = markers.iter().map(|k| (*k, std::env::var_os(k))).collect();
        unsafe {
            std::env::remove_var("AMPLIHACK_AGENT_BINARY");
            for (k, _) in &saved {
                std::env::remove_var(k);
            }
        }

        let resolved = amplihack_utils::agent_binary::resolve(dir.path()).unwrap();

        unsafe {
            for (k, v) in saved {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
            match prev {
                Some(v) => std::env::set_var("AMPLIHACK_AGENT_BINARY", v),
                None => std::env::remove_var("AMPLIHACK_AGENT_BINARY"),
            }
        }

        assert_eq!(
            resolved, tool,
            "{tool}: the resolver must read back what the writer wrote"
        );
    }
}

/// `extra_args` carries prompt text, so scanning all of it would make an
/// ordinary request persist nothing and fall through to the built-in default --
/// the failure this guard exists to prevent, re-entered through another door.
#[test]
fn prompt_text_that_merely_mentions_a_flag_still_persists() {
    for args in [
        vec![
            "help".to_string(),
            "me".into(),
            "fix".into(),
            "the".into(),
            "build".into(),
        ],
        vec!["-p".to_string(), "help".into()],
        vec!["--model".to_string(), "opus".into(), "--version".into()],
    ] {
        let dir = tempfile::tempdir().unwrap();
        // `help` in position zero is a real question-and-exit; deeper in the
        // list it is just a word.
        let is_query = matches!(args.first().map(String::as_str), Some("help"));
        persist_launcher_context("claude", Some(dir.path()), &args).unwrap();
        assert_eq!(
            read_launcher_context(dir.path()).is_none(),
            is_query,
            "args {args:?}: only a leading question-and-exit token may suppress persistence"
        );
    }
}
