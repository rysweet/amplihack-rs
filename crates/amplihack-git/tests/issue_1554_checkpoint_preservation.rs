//! Checkpoint validation must cover the complete index and preserve metadata.
#[path = "support/issue_1554.rs"]
mod fixture;

use amplihack_git::{checkpoint_transaction, workspace_validation::Workspace};
use fixture::{Repo, git, isolated, source_authority, text, write};
use serde_json::json;
use std::fs;

#[test]
fn unchanged_tracked_runtime_is_rejected_by_private_index_validation() {
    isolated(
        "unchanged_tracked_runtime_is_rejected_by_private_index_validation",
        || {
            let repo = Repo::new();
            repo.commit(".amplihack/graph_db", "already tracked runtime\n");
            let workspace = Workspace::open(&repo.root).unwrap();
            let scratch = tempfile::Builder::new()
                .prefix("checkpoint-")
                .tempdir_in(workspace.git_dir())
                .unwrap();
            let index = scratch.path().join("index");
            fs::copy(workspace.git_dir().join("index"), &index).unwrap();
            let authority = scratch.path().join("scope.json");
            write(
                &authority,
                serde_json::to_vec(&json!({
                    "version": 1, "task": "1554", "checkout": workspace.root(),
                    "project": workspace.common(), "paths": ["source.txt"]
                }))
                .unwrap(),
            );
            let before = fs::read(workspace.git_dir().join("index")).unwrap();
            assert!(
                checkpoint_transaction::validate_index(&repo.root, &index, &authority).is_err(),
                "unchanged runtime in the proposed commit escaped validation"
            );
            assert_eq!(fs::read(workspace.git_dir().join("index")).unwrap(), before);
        },
    );
}

#[test]
fn staging_preserves_protected_intent_to_add_and_entry_flags() {
    isolated(
        "staging_preserves_protected_intent_to_add_and_entry_flags",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            write(repo.root.join("intent.txt"), "uncommitted intent\n");
            git(&repo.root, &["add", "--intent-to-add", "--", "intent.txt"]);
            git(
                &repo.root,
                &["update-index", "--assume-unchanged", "--", "protected.txt"],
            );
            let protected = git(
                &repo.root,
                &[
                    "ls-files",
                    "--stage",
                    "--debug",
                    "--",
                    "intent.txt",
                    "protected.txt",
                ],
            );
            let head = text(&repo.root, &["rev-parse", "HEAD"]);
            write(repo.root.join("source.txt"), "authorised source\n");
            checkpoint_transaction::checkpoint(&repo.root, "stage authorised source", true)
                .unwrap();
            assert_eq!(
                git(
                    &repo.root,
                    &[
                        "ls-files",
                        "--stage",
                        "--debug",
                        "--",
                        "intent.txt",
                        "protected.txt"
                    ]
                ),
                protected,
                "protected Git entries changed while staging another path"
            );
            assert_eq!(text(&repo.root, &["rev-parse", "HEAD"]), head);
            assert_eq!(
                git(&repo.root, &["show", ":source.txt"]),
                b"authorised source\n"
            );
        },
    );
}

#[test]
fn unsupported_authority_fails_before_staging_or_index_replacement() {
    isolated(
        "unsupported_authority_fails_before_staging_or_index_replacement",
        || {
            let repo = Repo::new();
            write(repo.root.join("protected.txt"), "protected staged bytes\n");
            git(&repo.root, &["add", "--", "protected.txt"]);
            write(
                repo.root.join("protected.txt"),
                "different unstaged bytes\n",
            );
            write(repo.root.join("source.txt"), "authorised-looking source\n");
            fixture::authority(
                json!({"version": 99, "task": "1554", "permitted_source_paths": ["source.txt"]}),
                json!({"version": 99, "task": "1554", "files_to_change": ["source.txt"]}),
            );
            let index = repo.root.join(".git/index");
            let before = fs::read(&index).unwrap();
            let result = checkpoint_transaction::checkpoint(&repo.root, "must refuse", true);
            assert!(result.is_err(), "unsupported authority reached staging");
            assert_eq!(
                fs::read(index).unwrap(),
                before,
                "refusal changed the original index"
            );
        },
    );
}
