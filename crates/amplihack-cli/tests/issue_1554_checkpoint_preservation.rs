//! #1554: checkpoints must not consume, overwrite or authorise the user's index.
//! These tests execute the shipped recipe/helper, with real commit hooks.

use crate::issue_1554_fixture::{Repo, describe, source_root, success, write};
use std::fs;

fn checkpoint(r: &Repo) -> std::process::Command {
    r.step_command(
        "workflow-refactor-review",
        "checkpoint-after-review-feedback",
    )
}

#[test]
fn checkpoint_commits_only_intended_source_and_preserves_protected_staged_bytes() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "checkpoint_commits_only_intended_source_and_preserves_protected_staged_bytes",
        || {
            let r = Repo::new();
            r.install_binary();
            let protected = r.protect_user_blob();
            let before = r.staged_patch();
            write(r.work.join("src/allowed.rs"), "authorised modification\n");
            write(r.work.join("src/new.rs"), "authorised new source\n");
            fs::remove_file(r.work.join("src/deleted.rs")).unwrap();
            r.hook(
                "pre-commit",
                "#!/bin/sh\ngit diff --cached --name-only -z > \"$HOOK_RECORD\"\n",
            );
            r.hook(
                "commit-msg",
                "#!/bin/sh\nprintf 'commit-msg\\n' >> \"$HOOK_EVENTS\"\n",
            );
            let record = r.root.join("hook-paths");
            let events = r.root.join("hook-events");
            let out = checkpoint(&r)
                .env("HOOK_RECORD", &record)
                .env("HOOK_EVENTS", &events)
                .output()
                .unwrap();
            success(&out);
            assert_eq!(
                r.git(&r.work, &["show", ":user.txt"]),
                protected,
                "staged blob was replaced"
            );
            assert_eq!(
                r.staged_patch(),
                before,
                "protected staged diff was consumed or rewritten"
            );
            assert_eq!(
                r.git(&r.work, &["show", "HEAD:user.txt"]),
                b"initial user.txt\n"
            );
            assert_eq!(
                r.git(
                    &r.work,
                    &[
                        "diff-tree",
                        "--no-commit-id",
                        "--name-only",
                        "-r",
                        "HEAD",
                        "-z"
                    ]
                ),
                b"src/allowed.rs\0src/deleted.rs\0src/new.rs\0"
            );
            assert_eq!(
                fs::read(record).unwrap(),
                b"src/allowed.rs\0src/deleted.rs\0src/new.rs\0",
                "the normal pre-commit hook must see the bounded private index"
            );
            assert_eq!(fs::read(events).unwrap(), b"commit-msg\n");
        },
    );
}

#[test]
fn checkpoint_preserves_staged_deletions_and_executable_modes() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "checkpoint_preserves_staged_deletions_and_executable_modes",
        || {
            use std::os::unix::fs::PermissionsExt;
            let r = Repo::new();
            r.install_binary();
            r.git(&r.work, &["rm", "--", "user.txt"]);
            fs::set_permissions(
                r.work.join("unrelated.txt"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            r.git(&r.work, &["add", "--", "unrelated.txt"]);
            // The working tree no longer agrees with either protected staged entry.
            write(r.work.join("user.txt"), "recreated but unstaged\n");
            fs::set_permissions(
                r.work.join("unrelated.txt"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            let before = r.staged_patch();
            write(r.work.join("src/new.rs"), "intended source\n");
            success(&checkpoint(&r).output().unwrap());
            assert_eq!(r.staged_patch(), before, "staged deletion or mode changed");
            assert_eq!(
                r.git(&r.work, &["show", "HEAD:user.txt"]),
                b"initial user.txt\n"
            );
        },
    );
}

#[test]
fn candidate_overlap_with_protected_staged_work_is_rejected_before_commit() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "candidate_overlap_with_protected_staged_work_is_rejected_before_commit",
        || {
            let r = Repo::new();
            r.install_binary();
            write(r.work.join("src/allowed.rs"), "protected staged source\n");
            r.git(&r.work, &["add", "--", "src/allowed.rs"]);
            write(r.work.join("src/allowed.rs"), "different proposed bytes\n");
            let before = r.staged();
            let head = r.oid(&r.work, "HEAD");
            let out = checkpoint(&r).output().unwrap();
            assert!(
                !out.status.success(),
                "protected candidate overlap accepted: {}",
                describe(&out)
            );
            assert_eq!(r.oid(&r.work, "HEAD"), head);
            assert_eq!(r.staged(), before);
            assert!(
                String::from_utf8_lossy(&out.stderr)
                    .to_lowercase()
                    .contains("stag"),
                "explain the staged conflict"
            );
        },
    );
}

#[test]
fn a_prohibited_pre_existing_staged_entry_is_rejected_without_unstaging_it() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "a_prohibited_pre_existing_staged_entry_is_rejected_without_unstaging_it",
        || {
            let r = Repo::new();
            r.install_binary();
            r.protect_user_blob();
            write(
                r.work.join(".amplihack/graph_db"),
                "force-staged database\n",
            );
            r.git(&r.work, &["add", "-f", "--", ".amplihack/graph_db"]);
            write(r.work.join("src/new.rs"), "intended source\n");
            let entries = r.staged();
            let patch = r.staged_patch();
            let head = r.oid(&r.work, "HEAD");
            let out = checkpoint(&r).output().unwrap();
            assert!(
                !out.status.success(),
                "prohibited protected entry accepted: {}",
                describe(&out)
            );
            assert_eq!(r.staged(), entries);
            assert_eq!(r.staged_patch(), patch);
            assert_eq!(r.oid(&r.work, "HEAD"), head);
            assert!(r.work.join(".amplihack/graph_db").is_file());
        },
    );
}

#[test]
fn index_mutating_hooks_cannot_create_an_unauthorised_commit() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "index_mutating_hooks_cannot_create_an_unauthorised_commit",
        || {
            for hook in [
                "pre-commit",
                "prepare-commit-msg",
                "commit-msg",
                "reference-transaction",
            ] {
                let r = Repo::new();
                r.install_binary();
                r.protect_user_blob();
                let before = r.staged();
                let head = r.oid(&r.work, "HEAD");
                write(r.work.join("src/new.rs"), "intended source\n");
                r.hook(hook, "#!/bin/sh\ncase \"$1\" in committed|aborted) exit 0 ;; esac\nprintf 'hook-added unauthorised source\\n' > unrelated.txt\ngit add -- unrelated.txt\nprintf 'ran\\n' > \"$HOOK_RAN\"\n");
                let marker = r.root.join("hook-ran");
                let out = checkpoint(&r).env("HOOK_RAN", &marker).output().unwrap();
                assert!(
                    marker.is_file(),
                    "normal {hook} was bypassed: {}",
                    describe(&out)
                );
                assert_eq!(
                    r.oid(&r.work, "HEAD"),
                    head,
                    "{hook} created an unauthorised commit; post-commit rejection is too late"
                );
                assert!(
                    !out.status.success(),
                    "index mutation must fail before commit creation"
                );
                assert_eq!(
                    r.staged(),
                    before,
                    "hook repair modified the protected real index"
                );
            }
        },
    );
}

#[test]
fn missing_established_scope_cannot_be_inferred_from_the_index_or_candidates() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "missing_established_scope_cannot_be_inferred_from_the_index_or_candidates",
        || {
            let r = Repo::new();
            r.install_binary();
            r.protect_user_blob();
            write(r.work.join("src/new.rs"), "candidate without authority\n");
            let before = r.staged();
            let head = r.oid(&r.work, "HEAD");
            let out = checkpoint(&r)
                .env_remove("RECIPE_VAR_final_requirements")
                .env_remove("RECIPE_VAR_requirements")
                .env_remove("RECIPE_VAR_design_spec")
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "missing authority accepted: {}",
                describe(&out)
            );
            assert_eq!(r.staged(), before);
            assert_eq!(r.oid(&r.work, "HEAD"), head);
        },
    );
}

#[test]
fn candidate_inventory_cannot_extend_reviewed_task_authority() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "candidate_inventory_cannot_extend_reviewed_task_authority",
        || {
            let r = Repo::new();
            r.install_binary();
            write(r.work.join("unrelated.txt"), "outside reviewed scope\n");
            let before = r.staged();
            let head = r.oid(&r.work, "HEAD");
            let out = checkpoint(&r)
                .env("RECIPE_VAR_ops_modified_files", r#"["unrelated.txt"]"#)
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "candidate list granted authority: {}",
                describe(&out)
            );
            assert_eq!(r.oid(&r.work, "HEAD"), head);
            assert_eq!(r.staged(), before);
        },
    );
}

#[test]
fn candidate_paths_reject_traversal_absolute_names_and_empty_values_before_mutation() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "candidate_paths_reject_traversal_absolute_names_and_empty_values_before_mutation",
        || {
            for candidate in [
                "../unrelated.txt",
                "/tmp/unrelated.txt",
                "src/../unrelated.txt",
                "",
            ] {
                let r = Repo::new();
                r.install_binary();
                r.protect_user_blob();
                write(r.work.join("src/new.rs"), "intended source\n");
                let before = r.staged();
                let head = r.oid(&r.work, "HEAD");
                let out = checkpoint(&r)
                    .env(
                        "RECIPE_VAR_ops_modified_files",
                        serde_json::json!([candidate]).to_string(),
                    )
                    .output()
                    .unwrap();
                assert!(
                    !out.status.success(),
                    "invalid candidate {candidate:?} accepted: {}",
                    describe(&out)
                );
                assert_eq!(r.staged(), before);
                assert_eq!(r.oid(&r.work, "HEAD"), head);
            }
        },
    );
}

#[test]
fn a_task_identity_mismatch_cannot_establish_checkpoint_authority() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "a_task_identity_mismatch_cannot_establish_checkpoint_authority",
        || {
            let r = Repo::new();
            r.install_binary();
            r.protect_user_blob();
            write(r.work.join("src/new.rs"), "intended source\n");
            let before = r.staged();
            let head = r.oid(&r.work, "HEAD");
            let wrong_task = serde_json::json!({"version":1,"task": "a-different-task", "files_to_change": ["src/new.rs"]});
            let out = checkpoint(&r)
                .env("RECIPE_VAR_design_spec", wrong_task.to_string())
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "task identity mismatch accepted: {}",
                describe(&out)
            );
            assert_eq!(r.staged(), before);
            assert_eq!(r.oid(&r.work, "HEAD"), head);
        },
    );
}

#[test]
fn legacy_staging_adapter_requires_explicit_candidates_and_scope() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "legacy_staging_adapter_requires_explicit_candidates_and_scope",
        || {
            let r = Repo::new();
            r.protect_user_blob();
            write(r.work.join("unrelated.txt"), "must not be collected\n");
            write(r.work.join("src/new.rs"), "not authority by itself\n");
            let before = r.staged();
            let out = r
                .shell("set -euo pipefail; . \"$1\"; stage_workflow_changes")
                .arg(source_root().join("amplifier-bundle/tools/workflow_stage_changes.sh"))
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "unbounded staging adapter succeeded: {}",
                describe(&out)
            );
            assert_eq!(r.staged(), before);
        },
    );
}

#[test]
fn inherited_index_redirection_cannot_select_the_checkpoint_index() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "inherited_index_redirection_cannot_select_the_checkpoint_index",
        || {
            let r = Repo::new();
            r.install_binary();
            let protected = r.protect_user_blob();
            // Obtain the linked index path from Git rather than assume its worktree id.
            let index = r.git(
                &r.work,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            );
            let index = std::path::PathBuf::from(String::from_utf8(index).unwrap().trim());
            let decoy = r.root.join("decoy-index");
            fs::copy(index, &decoy).unwrap();
            let decoy_before = fs::read(&decoy).unwrap();
            write(r.work.join("src/new.rs"), "intended source\n");
            let out = checkpoint(&r)
                .env("GIT_INDEX_FILE", &decoy)
                .output()
                .unwrap();
            success(&out);
            assert_eq!(
                fs::read(decoy).unwrap(),
                decoy_before,
                "inherited index was used"
            );
            assert_eq!(r.git(&r.work, &["show", ":user.txt"]), protected);
            assert_eq!(
                r.git(&r.work, &["show", "HEAD:src/new.rs"]),
                b"intended source\n"
            );
        },
    );
}

#[test]
fn an_existing_index_lock_rejects_without_mutating_any_staged_entry() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "an_existing_index_lock_rejects_without_mutating_any_staged_entry",
        || {
            let r = Repo::new();
            r.install_binary();
            r.protect_user_blob();
            write(r.work.join("src/new.rs"), "intended source\n");
            let before = r.staged();
            let head = r.oid(&r.work, "HEAD");
            let path = r.git(
                &r.work,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "index.lock",
                ],
            );
            let lock = std::path::PathBuf::from(String::from_utf8(path).unwrap().trim());
            write(&lock, "owned by another operation\n");
            let out = checkpoint(&r).output().unwrap();
            assert!(
                !out.status.success(),
                "concurrent index writer accepted: {}",
                describe(&out)
            );
            assert_eq!(r.staged(), before);
            assert_eq!(r.oid(&r.work, "HEAD"), head);
            assert_eq!(fs::read(lock).unwrap(), b"owned by another operation\n");
        },
    );
}

#[test]
fn an_unmerged_index_is_rejected_without_resolving_or_committing_it() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "an_unmerged_index_is_rejected_without_resolving_or_committing_it",
        || {
            let r = Repo::new();
            r.install_binary();
            let incoming =
                r.commit_file(&r.main, "src/allowed.rs", "incoming conflicting source\n");
            r.commit_file(&r.work, "src/allowed.rs", "task conflicting source\n");
            let merge = r.git_output(&r.work, &["merge", "--no-commit", &incoming]);
            assert!(
                !merge.status.success(),
                "fixture must have a real merge conflict"
            );
            assert!(!r.git(&r.work, &["ls-files", "--unmerged", "-z"]).is_empty());
            let before = r.staged();
            let head = r.oid(&r.work, "HEAD");
            let out = checkpoint(&r).output().unwrap();
            assert!(
                !out.status.success(),
                "unmerged index accepted: {}",
                describe(&out)
            );
            assert_eq!(
                r.staged(),
                before,
                "automatic staging resolved an unmerged entry"
            );
            assert_eq!(r.oid(&r.work, "HEAD"), head);
        },
    );
}

#[test]
fn exact_source_names_are_literal_and_do_not_collect_sibling_paths() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "exact_source_names_are_literal_and_do_not_collect_sibling_paths",
        || {
            let r = Repo::new();
            r.install_binary();
            let literal = "src/*.rs";
            write(r.work.join(literal), "authorised literal filename\n");
            write(
                r.work.join("src/sibling.rs"),
                "unrelated source must stay untracked\n",
            );
            let requirements = serde_json::json!({"version":1,"task": "1554-fixture", "permitted_source_paths": [literal]});
            let design = serde_json::json!({"version":1,"task": "1554-fixture", "files_to_change": [literal]});
            let out = checkpoint(&r)
                .env("RECIPE_VAR_final_requirements", requirements.to_string())
                .env("RECIPE_VAR_requirements", requirements.to_string())
                .env("RECIPE_VAR_design_spec", design.to_string())
                .env(
                    "RECIPE_VAR_ops_modified_files",
                    serde_json::json!([literal]).to_string(),
                )
                .output()
                .unwrap();
            success(&out);
            assert_eq!(
                r.git(
                    &r.work,
                    &[
                        "diff-tree",
                        "--no-commit-id",
                        "--name-only",
                        "-r",
                        "HEAD",
                        "-z"
                    ]
                ),
                b"src/*.rs\0"
            );
            assert_eq!(r.inventory(&r.work), b"src/sibling.rs\0");
        },
    );
}

#[test]
fn normal_post_index_change_hooks_are_preserved_inside_the_commit() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "normal_post_index_change_hooks_are_preserved_inside_the_commit",
        || {
            let r = Repo::new();
            let marker = r.root.join("post-index-hook-ran");
            r.hook("post-index-change", "#!/bin/sh\ncase \"$(ps -p \"$PPID\" -o command=)\" in *' commit '*) printf 'post-index\\n' >> \"$HOOK_MARKER\" ;; esac\n");
            write(r.work.join("src/allowed.rs"), "normal Git control\n");
            r.git(&r.work, &["add", "--", "src/allowed.rs"]);
            let control = r
                .command("git")
                .env("HOOK_MARKER", &marker)
                .args(["commit", "--quiet", "-m", "normal hook control"])
                .output()
                .unwrap();
            success(&control);
            assert!(
                marker.exists(),
                "normal Git must call post-index-change within the commit fixture"
            );
            fs::remove_file(&marker).unwrap();
            let protected = r.protect_user_blob();
            write(r.work.join("src/new.rs"), "authorised checkpoint source\n");
            let out = checkpoint(&r).env("HOOK_MARKER", &marker).output().unwrap();
            success(&out);
            assert!(
                marker.exists(),
                "checkpoint suppressed a normal post-index-change hook: {}",
                describe(&out)
            );
            assert_eq!(r.git(&r.work, &["show", ":user.txt"]), protected);
        },
    );
}

#[test]
fn nested_reference_transactions_preserve_the_normal_hooks_input() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "nested_reference_transactions_preserve_the_normal_hooks_input",
        || {
            let r = Repo::new();
            r.install_binary();
            let record = r.root.join("reference-hook-input");
            r.hook("reference-transaction", "#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\nfirst=1\nwhile read -r old new reference; do\n  case \"$reference\" in refs/heads/*|HEAD)\n    printf '%s\\n' \"$reference\" >> \"$REF_RECORD\"\n    if [ \"$first\" = 1 ]; then\n      first=0\n      git update-ref refs/notes/normal-hook \"$old\" || exit $?\n    fi ;;\n  esac\ndone\n");
            write(r.work.join("src/allowed.rs"), "ordinary Git control\n");
            r.git(&r.work, &["add", "--", "src/allowed.rs"]);
            let control = r
                .command("git")
                .env("REF_RECORD", &record)
                .args(["commit", "--quiet", "-m", "normal reference-hook control"])
                .output()
                .unwrap();
            success(&control);
            let expected = fs::read(&record).unwrap();
            assert!(
                !expected.is_empty(),
                "ordinary Git must run the reference hook"
            );
            fs::remove_file(&record).unwrap();
            let protected = r.protect_user_blob();
            write(r.work.join("src/new.rs"), "authorised checkpoint source\n");
            let out = checkpoint(&r).env("REF_RECORD", &record).output().unwrap();
            success(&out);
            assert_eq!(
                fs::read(record).unwrap(),
                expected,
                "nested reference callbacks changed the normal hook's input"
            );
            assert_eq!(r.git(&r.work, &["show", ":user.txt"]), protected);
        },
    );
}
