//! The guard's policy is independent of ignore rules, authority and baselines.
#[path = "../../amplihack-git/tests/support/issue_1554.rs"]
mod fixture;

use amplihack_git::checkpoint_transaction::ValidatedIndex;
use amplihack_utils::artifact_guard::{
    ArtifactGuardConfig, ArtifactGuardMode, scan_artifacts, scan_validated_index,
};
use fixture::{Repo, git, isolated, write};
use std::fs;

#[test]
fn unchanged_runtime_cannot_be_exempted_by_a_baseline_or_exact_allowlist() {
    isolated(
        "unchanged_runtime_cannot_be_exempted_by_a_baseline_or_exact_allowlist",
        || {
            let mut escaped = Vec::new();
            for path in [
                ".amplihack/project.toml",
                ".claude/runtime/launcher_context.json",
            ] {
                let repo = Repo::new();
                let baseline = repo.commit(path, "pre-existing runtime\n");
                let allowlist = repo.root.join(".amplihack-artifact-allowlist");
                write(&allowlist, format!("{path}\n"));
                write(repo.root.join("protected.txt"), "user-staged bytes\n");
                git(&repo.root, &["add", "--", "protected.txt"]);
                let original_index = fs::read(repo.root.join(".git/index")).unwrap();
                for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::PrePublish] {
                    let config = ArtifactGuardConfig::new(&repo.root)
                        .with_mode(mode)
                        .with_baseline(&baseline)
                        .with_allowlist(allowlist.clone());
                    let report = scan_artifacts(&config).unwrap();
                    if !report.blocks()
                        || !report.blocking_violations().iter().any(|v| v.path == path)
                    {
                        escaped.push(format!("{mode:?}: {path}"));
                    }
                }
                assert_eq!(
                    fs::read(repo.root.join(".git/index")).unwrap(),
                    original_index,
                    "guard changed an unrelated staged entry"
                );
            }
            assert!(
                escaped.is_empty(),
                "prohibited tracked runtime was exempted: {escaped:?}"
            );
        },
    );
}

#[test]
fn private_index_scan_checks_unchanged_tracked_runtime_too() {
    isolated(
        "private_index_scan_checks_unchanged_tracked_runtime_too",
        || {
            let repo = Repo::new();
            repo.commit(".amplihack/graph_db", "already committed graph\n");
            let original = fs::read(repo.root.join(".git/index")).unwrap();
            let private = tempfile::Builder::new()
                .prefix("checkpoint-")
                .tempdir_in(repo.root.join(".git"))
                .unwrap();
            let index = private.path().join("index");
            write(&index, &original);
            let validated = ValidatedIndex::open(&repo.root, &index).unwrap();
            let config =
                ArtifactGuardConfig::new(&repo.root).with_mode(ArtifactGuardMode::PrePublish);
            let report = scan_validated_index(&config, &validated).unwrap();
            assert!(
                report.blocks(),
                "private scan inspected only differences and missed tracked runtime"
            );
            assert!(
                report
                    .blocking_violations()
                    .iter()
                    .any(|v| v.path == ".amplihack/graph_db")
            );
            assert_eq!(fs::read(repo.root.join(".git/index")).unwrap(), original);
        },
    );
}

#[test]
fn runtime_deletion_endpoints_are_prohibited_at_the_commit_gate() {
    isolated(
        "runtime_deletion_endpoints_are_prohibited_at_the_commit_gate",
        || {
            let repo = Repo::new();
            repo.commit(".claude/runtime/sessions.jsonl", "tracked runtime\n");
            git(
                &repo.root,
                &["rm", "--quiet", "--", ".claude/runtime/sessions.jsonl"],
            );
            let before = fs::read(repo.root.join(".git/index")).unwrap();
            let config =
                ArtifactGuardConfig::new(&repo.root).with_mode(ArtifactGuardMode::PreCommit);
            let report = scan_artifacts(&config).unwrap();
            assert!(
                report.blocks(),
                "deletion endpoint escaped the commit guard"
            );
            assert_eq!(fs::read(repo.root.join(".git/index")).unwrap(), before);
        },
    );
}

#[test]
fn private_index_scan_preserves_registered_worktree_gitlink_protection() {
    isolated(
        "private_index_scan_preserves_registered_worktree_gitlink_protection",
        || {
            let repo = Repo::new();
            let linked = repo.temp.path().join("source/worktrees/task");
            git(
                &repo.root,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "-b",
                    "task",
                    linked.to_str().unwrap(),
                ],
            );
            let oid = fixture::text(&linked, &["rev-parse", "HEAD"]);
            git(
                &repo.root,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("160000,{oid},worktrees/task"),
                ],
            );
            let original = fs::read(repo.root.join(".git/index")).unwrap();
            let private = tempfile::Builder::new()
                .prefix("checkpoint-")
                .tempdir_in(repo.root.join(".git"))
                .unwrap();
            let index = private.path().join("index");
            write(&index, &original);
            let validated = ValidatedIndex::open(&repo.root, &index).unwrap();
            let config =
                ArtifactGuardConfig::new(&repo.root).with_mode(ArtifactGuardMode::PrePublish);
            let report = scan_validated_index(&config, &validated).unwrap();
            assert!(
                report
                    .blocking_violations()
                    .iter()
                    .any(|v| v.rule_id == "run-worktree-gitlink"),
                "private-index scan lost the independent gitlink rule: {:?}",
                report.violations
            );
            assert_eq!(fs::read(repo.root.join(".git/index")).unwrap(), original);
        },
    );
}
