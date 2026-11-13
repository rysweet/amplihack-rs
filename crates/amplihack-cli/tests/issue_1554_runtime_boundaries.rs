//! #1554: collection prevention and publication prohibition are independent.
//! All environment changes are confined to a quiet child and disposable roots.

use crate::issue_1554_fixture::{RUNTIME, Repo, describe, executable, source_root, success, write};
use crate::support::quiet_process::{
    child_remove_var, child_set_current_dir, child_set_var, in_quiet_process,
};
use amplihack_utils::artifact_guard::{ArtifactGuardConfig, ArtifactGuardMode, scan_artifacts};
use amplihack_utils::launcher_context::{
    LauncherKind, read_launcher_context, write_launcher_context,
};
use std::collections::BTreeMap;
use std::fs;

fn in_profile(name: &str, body: impl FnOnce(&Repo)) {
    in_quiet_process(module_path!(), name, || {
        let r = Repo::new();
        for key in [
            "AMPLIHACK_RUNTIME_DIR",
            "AMPLIHACK_SESSION_ID",
            "AMPLIHACK_AGENT_BINARY",
            "AMPLIHACK_AGENT_BINARY_SOURCE",
            "CLAUDECODE",
            "CODEX_THREAD_ID",
            "TMUX",
        ] {
            child_remove_var(key);
        }
        for (key, path) in [
            ("HOME", r.home.clone()),
            ("XDG_CONFIG_HOME", r.home.join("xdg")),
            ("AMPLIHACK_HOME", r.home.join("amplihack")),
            ("CODEX_HOME", r.home.join("codex")),
            ("CLAUDE_CONFIG_DIR", r.home.join("claude")),
            ("AMPLIHACK_CLAUDE_PROFILE_ROOT", r.home.join("profiles")),
        ] {
            child_set_var(key, path);
        }
        body(&r);
    });
}

#[test]
fn launcher_context_round_trip_is_external_and_keeps_source_inventory_exact() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "launcher_context_round_trip_is_external_and_keeps_source_inventory_exact",
        || {
            in_profile(
                "launcher_context_round_trip_is_external_and_keeps_source_inventory_exact",
                |r| {
                    write(
                        r.work.join("source-manifest.txt"),
                        "source awaiting a commit\n",
                    );
                    let before = r.inventory(&r.work);
                    assert_eq!(before, b"source-manifest.txt\0");
                    let path = write_launcher_context(
                        &r.work,
                        LauncherKind::Codex,
                        "codex",
                        BTreeMap::new(),
                    )
                    .unwrap();
                    let loaded =
                        read_launcher_context(&r.work).expect("external writer and reader agree");
                    assert_eq!(loaded.launcher, LauncherKind::Codex);
                    assert!(
                        !path.canonicalize().unwrap().starts_with(&r.work),
                        "launcher context must be physically outside the checkout: {}",
                        path.display()
                    );
                    assert_eq!(
                        r.inventory(&r.work),
                        before,
                        "launch changed the source manifest"
                    );
                    assert!(!r.work.join(".claude/runtime").exists());
                },
            );
        },
    );
}

#[test]
fn both_session_trackers_and_the_cli_log_lock_are_external() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "both_session_trackers_and_the_cli_log_lock_are_external",
        || {
            in_profile(
                "both_session_trackers_and_the_cli_log_lock_are_external",
                |r| {
                    let before = r.inventory(&r.work);
                    let cli = amplihack_cli::session_tracker::SessionTracker::new(&r.work).unwrap();
                    let id = cli
                        .start_session(
                            std::process::id(),
                            &r.work,
                            &[],
                            false,
                            &amplihack_cli::nesting::NestingResult::NotNested,
                        )
                        .unwrap();
                    cli.complete_session(&id).unwrap();
                    let other =
                        amplihack_launcher::session_tracker::SessionTracker::new(&r.work).unwrap();
                    let id = other
                        .start_session(
                            std::process::id(),
                            r.work.to_str().unwrap(),
                            &[],
                            false,
                            false,
                            None,
                        )
                        .unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        for path in [
                            other.log_path().to_owned(),
                            other.log_path().with_extension("jsonl.lock"),
                        ] {
                            fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
                        }
                    }
                    other.complete_session(&id).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        for path in [
                            other.log_path().to_owned(),
                            other.log_path().with_extension("jsonl.lock"),
                        ] {
                            assert_eq!(
                                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                                0o600,
                                "session state did not become private: {}",
                                path.display()
                            );
                            fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
                        }
                    }
                    cli.complete_session(&id).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        for path in [
                            other.log_path().to_owned(),
                            other.log_path().with_extension("jsonl.lock"),
                        ] {
                            assert_eq!(
                                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                                0o600
                            );
                        }
                    }
                    assert!(
                        other
                            .read_entries()
                            .unwrap()
                            .iter()
                            .any(|entry| entry.session_id == id)
                    );
                    assert!(
                        !other
                            .log_path()
                            .canonicalize()
                            .unwrap()
                            .starts_with(&r.work)
                    );
                    assert_eq!(
                        r.inventory(&r.work),
                        before,
                        "sessions.jsonl or its lock leaked"
                    );
                    assert!(!r.work.join(".claude/runtime").exists());
                },
            );
        },
    );
}

#[test]
fn same_named_repositories_and_linked_checkouts_do_not_share_launcher_context() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "same_named_repositories_and_linked_checkouts_do_not_share_launcher_context",
        || {
            in_profile(
                "same_named_repositories_and_linked_checkouts_do_not_share_launcher_context",
                |r| {
                    let other = r.root.join("elsewhere/task");
                    fs::create_dir_all(other.parent().unwrap()).unwrap();
                    r.git(
                        &r.root,
                        &[
                            "clone",
                            "-q",
                            r.remote.to_str().unwrap(),
                            other.to_str().unwrap(),
                        ],
                    );
                    let mut paths = Vec::new();
                    for (repo, launcher) in [
                        (&r.main, LauncherKind::Claude),
                        (&r.work, LauncherKind::Codex),
                        (&other, LauncherKind::Copilot),
                    ] {
                        paths.push(
                            write_launcher_context(
                                repo,
                                launcher,
                                launcher.as_str(),
                                BTreeMap::new(),
                            )
                            .unwrap(),
                        );
                    }
                    assert_eq!(
                        read_launcher_context(&r.main).unwrap().launcher,
                        LauncherKind::Claude
                    );
                    assert_eq!(
                        read_launcher_context(&r.work).unwrap().launcher,
                        LauncherKind::Codex
                    );
                    assert_eq!(
                        read_launcher_context(&other).unwrap().launcher,
                        LauncherKind::Copilot
                    );
                    paths.sort();
                    paths.dedup();
                    assert_eq!(
                        paths.len(),
                        3,
                        "project and canonical checkout identity are both required"
                    );
                    assert!(paths.iter().all(|p| !p.starts_with(&r.main)
                        && !p.starts_with(&r.work)
                        && !p.starts_with(&other)));
                },
            );
        },
    );
}

#[test]
fn copied_context_from_a_different_checkout_is_not_routing_authority() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "copied_context_from_a_different_checkout_is_not_routing_authority",
        || {
            in_profile(
                "copied_context_from_a_different_checkout_is_not_routing_authority",
                |r| {
                    let task = write_launcher_context(
                        &r.work,
                        LauncherKind::Codex,
                        "codex",
                        BTreeMap::new(),
                    )
                    .unwrap();
                    let main = write_launcher_context(
                        &r.main,
                        LauncherKind::Copilot,
                        "copilot",
                        BTreeMap::new(),
                    )
                    .unwrap();
                    assert_eq!(
                        read_launcher_context(&r.work).unwrap().launcher,
                        LauncherKind::Codex
                    );
                    fs::copy(main, task).unwrap();
                    assert!(
                        read_launcher_context(&r.work).is_none(),
                        "identity metadata from another checkout was accepted"
                    );
                },
            );
        },
    );
}

#[test]
fn read_only_resolution_does_not_create_state_or_honour_legacy_checkout_context() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "read_only_resolution_does_not_create_state_or_honour_legacy_checkout_context",
        || {
            in_profile(
                "read_only_resolution_does_not_create_state_or_honour_legacy_checkout_context",
                |r| {
                    write(
                        r.work.join(".claude/runtime/launcher_context.json"),
                        r#"{"launcher":"codex","command":"codex","timestamp":"2099-01-01T00:00:00Z","environment":{}}"#,
                    );
                    let before = r.inventory(&r.work);
                    assert!(
                        read_launcher_context(&r.work).is_none(),
                        "legacy checkout context is not routing authority"
                    );
                    let resolved = amplihack_utils::agent_binary::resolve_detailed_configured(
                        &r.work,
                        amplihack_utils::agent_binary::ConfiguredBinary::Unset,
                        &|_| false,
                    )
                    .unwrap();
                    assert_ne!(
                        resolved.binary, "codex",
                        "agent-binary still trusts checkout-local context"
                    );
                    assert_eq!(r.inventory(&r.work), before);
                    assert!(
                        !r.home.join("amplihack").exists(),
                        "a reader created external state"
                    );
                },
            );
        },
    );
}

#[test]
fn checkout_contained_runtime_override_and_symlink_alias_are_rejected() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "checkout_contained_runtime_override_and_symlink_alias_are_rejected",
        || {
            in_profile(
                "checkout_contained_runtime_override_and_symlink_alias_are_rejected",
                |r| {
                    // Put the checkout beneath this disposable HOME so that the old
                    // home-containment check alone cannot make the regression pass.
                    child_set_var("HOME", &r.root);
                    for override_path in [
                        r.work.join("runtime-override"),
                        r.root.join("runtime-alias"),
                    ] {
                        if override_path.file_name().unwrap() == "runtime-alias" {
                            std::os::unix::fs::symlink(&r.work, &override_path).unwrap();
                        }
                        child_set_var("AMPLIHACK_RUNTIME_DIR", &override_path);
                        let before = r.inventory(&r.work);
                        let result = amplihack_utils::worktree::get_shared_runtime_dir(&r.work);
                        assert!(
                            result.is_err(),
                            "checkout-contained override accepted: {result:?}"
                        );
                        assert_eq!(
                            r.inventory(&r.work),
                            before,
                            "rejection must precede directory creation"
                        );
                    }
                },
            );
        },
    );
}

#[test]
fn both_nesting_detectors_follow_the_external_session_log() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "both_nesting_detectors_follow_the_external_session_log",
        || {
            in_profile(
                "both_nesting_detectors_follow_the_external_session_log",
                |r| {
                    child_set_current_dir(&r.work);
                    let tracker =
                        amplihack_cli::session_tracker::SessionTracker::new(&r.work).unwrap();
                    let id = tracker
                        .start_session(
                            std::process::id(),
                            &r.work,
                            &[],
                            false,
                            &amplihack_cli::nesting::NestingResult::NotNested,
                        )
                        .unwrap();
                    assert!(matches!(amplihack_cli::nesting::NestingDetector::detect(),
            amplihack_cli::nesting::NestingResult::Nested { session_id, .. } if session_id == id));
                    let nesting =
                        amplihack_launcher::NestingDetector::new().detect_nesting(&r.work, &[]);
                    assert_eq!(nesting.parent_session_id.as_deref(), Some(id.as_str()));
                    assert!(!r.work.join(".claude/runtime/sessions.jsonl").exists());
                    tracker.complete_session(&id).unwrap();
                    assert_eq!(
                        amplihack_cli::nesting::NestingDetector::detect(),
                        amplihack_cli::nesting::NestingResult::NotNested
                    );
                    assert!(
                        !amplihack_launcher::NestingDetector::new()
                            .detect_nesting(&r.work, &[])
                            .is_nested
                    );
                },
            );
        },
    );
}

#[test]
fn real_launch_with_a_local_agent_preserves_the_source_manifest() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "real_launch_with_a_local_agent_preserves_the_source_manifest",
        || {
            let r = Repo::new();
            r.install_binary();
            write(r.work.join("source-manifest.txt"), "intended source\n");
            let before = r.inventory(&r.work);
            executable(
                r.bin.join("claude"),
                "#!/bin/sh\ncase \"$1\" in --version) echo '2.1.319 (Claude Code)';; *) printf 'agent completed\\n';; esac\n",
            );
            let out = r
                .command(r.bin.join("amplihack"))
                .args([
                    "claude",
                    "--subprocess-safe",
                    "--",
                    "-p",
                    "fixture source task",
                ])
                .output()
                .unwrap();
            success(&out);
            assert!(
                String::from_utf8_lossy(&out.stdout).contains("agent completed"),
                "the launch must reach the local agent: {}",
                describe(&out)
            );
            assert_eq!(
                r.inventory(&r.work),
                before,
                "real launch changed the manifest"
            );
            assert!(
                !r.work
                    .join(".claude/runtime/launcher_context.json")
                    .exists()
            );
            assert!(!r.work.join(".claude/runtime/sessions.jsonl").exists());
            assert!(!r.work.join(".claude/runtime/sessions.jsonl.lock").exists());
        },
    );
}

#[test]
fn missing_remote_and_bare_workspaces_are_rejected_before_preparation() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "missing_remote_and_bare_workspaces_are_rejected_before_preparation",
        || {
            let r = Repo::new();
            r.git(&r.main, &["remote", "remove", "origin"]);
            for workspace in [&r.main, &r.remote] {
                let out = r
                    .step_command("workflow-prep", "step-01-prepare-workspace")
                    .env("REPO_PATH", workspace)
                    .env("RECIPE_VAR_repo_path", workspace)
                    .output()
                    .unwrap();
                assert!(
                    !out.status.success(),
                    "unsuitable workspace accepted: {}",
                    describe(&out)
                );
                assert!(!workspace.join(".claude").exists());
                assert!(!workspace.join(".amplihack").exists());
            }
        },
    );
}

#[test]
fn direct_runtime_resolution_cannot_mutate_a_non_repository() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "direct_runtime_resolution_cannot_mutate_a_non_repository",
        || {
            in_profile(
                "direct_runtime_resolution_cannot_mutate_a_non_repository",
                |r| {
                    let plain = r.root.join("non-repository");
                    fs::create_dir(&plain).unwrap();
                    write(plain.join("source.txt"), "source manifest\n");
                    let result = amplihack_utils::worktree::get_shared_runtime_dir(&plain);
                    assert!(
                        result.is_err(),
                        "non-repository runtime write accepted: {result:?}"
                    );
                    assert_eq!(fs::read_dir(&plain).unwrap().count(), 1);
                    assert_eq!(
                        fs::read(plain.join("source.txt")).unwrap(),
                        b"source manifest\n"
                    );
                },
            );
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guard_rejects_a_runtime_filename_that_apfs_cannot_represent() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "guard_rejects_a_runtime_filename_that_apfs_cannot_represent",
        || {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            let r = Repo::new();
            let relative = std::path::PathBuf::from(OsString::from_vec(
                b".claude/runtime/non-utf8-\xff".to_vec(),
            ));
            write(r.work.join(&relative), "runtime\n");
            let out = r
                .command("git")
                .args(["add", "-f", "--"])
                .arg(&relative)
                .output()
                .unwrap();
            success(&out);
            let before = r.staged();
            match scan_artifacts(
                &ArtifactGuardConfig::new(&r.work).with_mode(ArtifactGuardMode::PrePublish),
            ) {
                Ok(report) => assert!(
                    !report.blocking_violations().is_empty(),
                    "unsupported runtime filename accepted"
                ),
                Err(_) => {} // Explicit inspection failure also blocks publication.
            }
            assert_eq!(r.staged(), before);
        },
    );
}

#[test]
fn common_exclusions_cover_owned_runtime_in_main_and_linked_worktrees_only() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "common_exclusions_cover_owned_runtime_in_main_and_linked_worktrees_only",
        || {
            let r = Repo::new();
            let exclude = r.main.join(".git/info/exclude");
            let original =
                b"# keep this exact comment\n/local-only\n/worktrees/\n/custom-without-newline";
            write(&exclude, original);
            success(&r.prepare(&r.main));
            let once = fs::read(&exclude).unwrap();
            assert!(
                once.starts_with(original),
                "existing bytes and unterminated line must survive"
            );
            success(&r.prepare(&r.work));
            success(&r.prepare(&r.main));
            assert_eq!(
                fs::read(&exclude).unwrap(),
                once,
                "common exclusions must be idempotent"
            );
            for repo in [&r.main, &r.work] {
                for path in RUNTIME {
                    write(repo.join(path), "owned runtime\n");
                }
                for path in [
                    ".amplihack/project.toml",
                    ".claude/settings.json",
                    "src/worktrees/source.rs",
                ] {
                    write(repo.join(path), "legitimate project source\n");
                }
                assert_eq!(
                    r.inventory(repo),
                    b".amplihack/project.toml\0.claude/settings.json\0src/worktrees/source.rs\0",
                    "owned runtime must be excluded without hiding configuration or nested source"
                );
            }
        },
    );
}

#[test]
fn equivalent_existing_exclusions_are_not_duplicated_and_permissions_survive() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "equivalent_existing_exclusions_are_not_duplicated_and_permissions_survive",
        || {
            use std::os::unix::fs::PermissionsExt;
            let r = Repo::new();
            let exclude = r.main.join(".git/info/exclude");
            write(
                &exclude,
                "# existing owned paths\n/worktrees/**\n/.claude/runtime/**\n/.amplihack/graph_db\n",
            );
            fs::set_permissions(&exclude, fs::Permissions::from_mode(0o640)).unwrap();
            success(&r.prepare(&r.work));
            let content = fs::read_to_string(&exclude).unwrap();
            assert_eq!(
                fs::metadata(&exclude).unwrap().permissions().mode() & 0o777,
                0o640
            );
            for pattern in ["/worktrees/", "/.claude/runtime/", "/.amplihack/graph_db"] {
                let equivalents = content
                    .lines()
                    .filter(|line| *line == pattern || *line == format!("{pattern}**"))
                    .count();
                assert_eq!(
                    equivalents, 1,
                    "duplicate equivalent rule: {pattern}\n{content}"
                );
            }
            write(r.work.join(".amplihack/indexing.pid"), "runtime\n");
            assert!(
                r.inventory(&r.work).is_empty(),
                "preparation did not add the other owned exclusions"
            );
        },
    );
}

#[test]
fn simultaneous_exclusion_registration_preserves_content_without_duplicates() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "simultaneous_exclusion_registration_preserves_content_without_duplicates",
        || {
            use std::io::Write;
            use std::process::Stdio;
            let r = Repo::new();
            let exclude = r.main.join(".git/info/exclude");
            write(&exclude, "# pre-existing content\n/user-rule\n");
            let mut children = Vec::new();
            for cwd in [&r.main, &r.work, &r.main, &r.work] {
                children.push(
            r.shell("set -eu; IFS= read -r ready; . \"$1\"; preflight_known_workflow_runtime_artifacts .")
                .arg(source_root().join("amplifier-bundle/tools/workflow_runtime_artifacts.sh"))
                .current_dir(cwd)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
            }
            // Release every child through its input pipe after all have started.
            // There is no timer, readiness polling or assumed scheduling window.
            for child in &mut children {
                child.stdin.take().unwrap().write_all(b"ready\n").unwrap();
            }
            for child in children {
                success(&child.wait_with_output().unwrap());
            }
            let bytes = fs::read(&exclude).unwrap();
            success(&r.prepare(&r.work));
            assert_eq!(
                fs::read(&exclude).unwrap(),
                bytes,
                "concurrent registration duplicated rules"
            );
            assert!(bytes.starts_with(b"# pre-existing content\n/user-rule\n"));
            write(r.work.join(".amplihack/indexing.pid"), "123\n");
            assert!(
                r.inventory(&r.work).is_empty(),
                "concurrent preparation did not register runtime"
            );
        },
    );
}

#[test]
fn exclusion_failure_is_reported_before_any_dependent_runtime_write() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "exclusion_failure_is_reported_before_any_dependent_runtime_write",
        || {
            let r = Repo::new();
            let exclude = r.main.join(".git/info/exclude");
            let before = r.inventory(&r.work);
            let original = fs::read(&exclude).unwrap();
            fs::remove_file(&exclude).unwrap();
            fs::create_dir(&exclude).unwrap(); // deterministic failure even when tests run as root
            let out = r.prepare(&r.work);
            assert!(
                !out.status.success(),
                "registration failure was swallowed: {}",
                describe(&out)
            );
            assert!(!out.stderr.is_empty(), "explain the failed registration");
            fs::remove_dir(&exclude).unwrap();
            write(&exclude, original);
            assert_eq!(r.inventory(&r.work), before);
        },
    );
}

#[test]
fn ordinary_non_git_workspace_with_nested_checkout_is_rejected_without_mutation() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "ordinary_non_git_workspace_with_nested_checkout_is_rejected_without_mutation",
        || {
            let r = Repo::new();
            let workspace = r.root.join("workspace");
            fs::create_dir(&workspace).unwrap();
            r.git(
                &workspace,
                &["clone", "-q", r.remote.to_str().unwrap(), "nested"],
            );
            write(workspace.join("notes.txt"), "user workspace\n");
            let nested_head = r.oid(&workspace.join("nested"), "HEAD");
            let out = r
                .step_command("workflow-prep", "step-01-prepare-workspace")
                .env("REPO_PATH", &workspace)
                .env("RECIPE_VAR_repo_path", &workspace)
                .env("AUTO_INIT_REPO", "true")
                .env("RECIPE_VAR_auto_init_repo", "true")
                .output()
                .unwrap();
            assert!(
                !workspace.join(".git").exists(),
                "ordinary launch initialised a parent repository"
            );
            assert!(
                !out.status.success(),
                "unsuitable workspace accepted: {}",
                describe(&out)
            );
            assert_eq!(r.oid(&workspace.join("nested"), "HEAD"), nested_head);
            assert_eq!(
                fs::read(workspace.join("notes.txt")).unwrap(),
                b"user workspace\n"
            );
            assert!(!workspace.join(".amplihack").exists());
            assert!(!workspace.join(".claude").exists());
        },
    );
}

fn assert_guard_rejects(r: &Repo, path: &str) {
    let entries = r.staged();
    let patch = r.staged_patch();
    let report =
        scan_artifacts(&ArtifactGuardConfig::new(&r.work).with_mode(ArtifactGuardMode::PrePublish))
            .unwrap();
    assert!(
        report.blocking_violations().iter().any(|v| v.path == path),
        "pre-publish accepted staged runtime {path}: {report:?}"
    );
    assert_eq!(r.staged(), entries, "guard rewrote staged entries");
    assert_eq!(r.staged_patch(), patch, "guard changed staged contents");
}

macro_rules! guard_case {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            crate::issue_1554_fixture::sandbox(module_path!(), stringify!($name), || {
                let r = Repo::new();
                r.protect_user_blob();
                write(r.work.join($path), "runtime\n");
                r.git(&r.work, &["add", "-f", "--", $path]);
                assert_guard_rejects(&r, $path);
            });
        }
    };
}
guard_case!(guard_rejects_graph_db, ".amplihack/graph_db");
guard_case!(guard_rejects_indexing_pid, ".amplihack/indexing.pid");
guard_case!(
    guard_rejects_launcher_context,
    ".claude/runtime/launcher_context.json"
);
guard_case!(guard_rejects_session_log, ".claude/runtime/sessions.jsonl");
guard_case!(
    guard_rejects_session_lock,
    ".claude/runtime/sessions.jsonl.lock"
);
guard_case!(
    guard_rejects_unknown_amplihack_configuration_when_staged,
    ".amplihack/project.toml"
);

#[test]
fn guard_rejects_tracked_runtime_deletion_even_with_an_allowlist() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "guard_rejects_tracked_runtime_deletion_even_with_an_allowlist",
        || {
            let r = Repo::new();
            let path = ".claude/runtime/launcher_context.json";
            r.commit_file(&r.work, path, "legacy committed runtime\n");
            write(
                r.work.join(".amplihack-artifact-allowlist"),
                format!("{path}\n"),
            );
            r.git(&r.work, &["rm", "--", path]);
            r.protect_user_blob();
            assert_guard_rejects(&r, path);
        },
    );
}

#[test]
fn guard_rejects_both_rename_endpoints_of_runtime_boundary() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "guard_rejects_both_rename_endpoints_of_runtime_boundary",
        || {
            for (old, new) in [
                (".amplihack/graph_db", "source.db"),
                ("source.db", ".claude/runtime/sessions.jsonl"),
            ] {
                let r = Repo::new();
                r.commit_file(&r.work, old, "rename content\n");
                fs::create_dir_all(r.work.join(new).parent().unwrap()).unwrap();
                r.git(&r.work, &["mv", "--", old, new]);
                r.protect_user_blob();
                assert_guard_rejects(&r, if old.starts_with('.') { old } else { new });
            }
        },
    );
}

#[test]
fn failed_and_partial_fetch_stop_workspace_preparation() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "failed_and_partial_fetch_stop_workspace_preparation",
        || {
            for partial in [false, true] {
                let r = Repo::new();
                let real_git = r.shell("command -v git").output().unwrap();
                success(&real_git);
                let real_git = String::from_utf8(real_git.stdout)
                    .unwrap()
                    .trim()
                    .to_owned();
                executable(
                    r.bin.join("git"),
                    r#"#!/bin/bash
for arg in "$@"; do
  if [ "$arg" = fetch ]; then
    printf 'fetch\n' >> "$FETCH_CALLS"
    if [ "$PARTIAL" = true ]; then "$REAL_GIT" update-ref refs/remotes/origin/partial HEAD || exit 96; fi
    echo 'injected incomplete fetch' >&2
    exit 23
  fi
done
exec "$REAL_GIT" "$@"
"#,
                );
                let calls = r.root.join("fetch-calls");
                let out = r
                    .step_command("workflow-prep", "step-01-prepare-workspace")
                    .env("REAL_GIT", real_git)
                    .env("FETCH_CALLS", &calls)
                    .env("PARTIAL", partial.to_string())
                    .output()
                    .unwrap();
                assert!(
                    !out.status.success(),
                    "failed fetch reported successful preparation: {}",
                    describe(&out)
                );
                assert!(!String::from_utf8_lossy(&out.stdout).contains("Workspace Prepared"));
                assert_eq!(
                    fs::read_to_string(calls).unwrap(),
                    "fetch\n",
                    "fetch must not retry"
                );
            }
        },
    );
}
