//! Issue #1740: the git configuration drift check in
//! `quick_change_diff_limits.sh` and the repository configuration that every
//! worktree shares.
//!
//! A linked worktree's local configuration is the repository's `.git/config`.
//! Other runs write branch upstream entries there while a quick-change run is
//! in progress (`workflow-worktree`'s `git branch --set-upstream-to`,
//! `git worktree add -b` from a remote branch, `git push -u`), and the
//! recipe's own push step runs `git push -u origin <ref>:<ref>`. None of
//! those may stop the run. A change to this run's own branch entries, which
//! a bare `git push` follows (`default-workflow` pushes that way after a
//! hand-over), still must, at every configuration level; only `--committed`,
//! measured after the hand-over point, leaves the upstream pair out.
//!
//! The run's worktree here is a linked worktree on a `quick/` branch created
//! from `origin/main`, as `workflow-worktree` creates it, so its upstream is
//! `origin/main` until a push sets it to its own branch.

use std::path::PathBuf;

use crate::support::quick_change_repo::{Repo, git, stderr, verdict};

const BRANCH: &str = "quick/fix-typo";
const PROCEED: &str = "QUICK_CHANGE_VERDICT=proceed reason=within-limits";
const DRIFT: &str = "QUICK_CHANGE_VERDICT=stop reason=git-config-drift";

/// Whether `workflow-worktree`'s best-effort push and `--set-upstream-to`
/// took effect before the snapshot was taken.
#[derive(Clone, Copy, Debug)]
enum Upstream {
    /// The branch is on `origin` and tracks `origin/quick/fix-typo`.
    OwnBranch,
    /// The push did not happen; the branch tracks `origin/main`, as
    /// `git worktree add -b` from `origin/main` set it.
    BaseBranch,
    /// The branch was created with no upstream at all.
    None,
}

/// A repository whose run worktree is a linked worktree on [`BRANCH`], with
/// the snapshot taken of that worktree. Returns the repository (its `wt` is
/// the linked worktree) and the main checkout.
fn linked_run(upstream: Upstream) -> (Repo, PathBuf) {
    let mut r = Repo::new();
    let main = r.wt.clone();
    let linked = r.tmp.path().join("quick-wt");
    let linked_s = linked.to_str().unwrap();
    match upstream {
        Upstream::None => git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--no-track",
                "-b",
                BRANCH,
                linked_s,
                "origin/main",
            ],
        ),
        _ => git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                BRANCH,
                linked_s,
                "origin/main",
            ],
        ),
    };
    if let Upstream::OwnBranch = upstream {
        git(&linked, &["push", "-q", "origin", BRANCH]);
        git(
            &linked,
            &[
                "branch",
                &format!("--set-upstream-to=origin/{BRANCH}"),
                BRANCH,
            ],
        );
    }
    r.wt = linked;
    let snap = r.run(&["--snapshot", r.wt.to_str().unwrap()], true);
    assert!(snap.status.success(), "--snapshot: {}", stderr(&snap));
    std::fs::write(r.state.join("git-config.snapshot"), &snap.stdout).unwrap();
    (r, main)
}

fn shared_config(main: &std::path::Path) -> String {
    git(main, &["config", "--local", "--list"])
}

#[test]
fn another_runs_branch_upstream_in_the_shared_config_is_not_drift() {
    for upstream in [Upstream::OwnBranch, Upstream::BaseBranch, Upstream::None] {
        let (r, main) = linked_run(upstream);
        let before = shared_config(&main);
        // Another run: a worktree created as workflow-worktree creates one,
        // its best-effort upstream, and a later `git push -u`.
        let other = r.tmp.path().join("other-wt");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "quick/other",
                other.to_str().unwrap(),
                "origin/main",
            ],
        );
        git(&other, &["push", "-q", "-u", "origin", "quick/other"]);
        // And an upstream set on a branch with no worktree.
        git(&main, &["branch", "-q", "feat/elsewhere", "origin/main"]);
        assert_ne!(
            shared_config(&main),
            before,
            "{upstream:?}: the other run must have written the shared configuration"
        );

        r.write("README.md", "# demo, edited\n");
        verdict(&r.measure(&[]), 0, PROCEED);
    }
}

#[test]
fn the_recipes_own_push_u_does_not_stop_the_post_push_measure() {
    for upstream in [Upstream::OwnBranch, Upstream::BaseBranch, Upstream::None] {
        let (r, main) = linked_run(upstream);
        r.write("README.md", "# demo, edited\n");
        verdict(&r.measure(&[]), 0, PROCEED);
        // quick-change-commit, then quick-change-push's own push.
        r.commit_all("fix typo");
        verdict(&r.measure(&["--committed"]), 0, PROCEED);
        let before = shared_config(&main);
        git(
            &r.wt,
            &[
                "push",
                "-q",
                "-u",
                "origin",
                &format!("refs/heads/{BRANCH}:refs/heads/{BRANCH}"),
            ],
        );
        if !matches!(upstream, Upstream::OwnBranch) {
            assert_ne!(
                shared_config(&main),
                before,
                "{upstream:?}: push -u must have written the upstream"
            );
        }
        verdict(&r.measure(&["--committed"]), 0, PROCEED);
    }
}

fn changed(j: &serde_json::Value) -> Vec<String> {
    j["changed"]
        .as_array()
        .unwrap_or_else(|| panic!("no changed list: {j}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_change_to_this_branchs_entries_or_other_local_keys_is_drift() {
    let own = |key: &str| format!("branch.{BRANCH}.{key}");
    let cases: Vec<(String, &str, &[&str])> = vec![
        (own("pushRemote"), "/tmp/elsewhere.git", &["branch-other"]),
        (own("remote"), "/tmp/elsewhere.git", &["branch-upstream"]),
        (own("merge"), "refs/heads/main", &["branch-upstream"]),
        (own("rebase"), "true", &["branch-other"]),
        ("branch.autoSetupMerge".into(), "always", &["config-local"]),
        ("push.followTags".into(), "true", &["config-local"]),
        (
            "remote.pushDefault".into(),
            "elsewhere",
            &["config-local", "remote-other"],
        ),
    ];
    for (key, value, names) in cases {
        let (r, _) = linked_run(Upstream::OwnBranch);
        r.write("README.md", "# demo, edited\n");
        git(&r.wt, &["config", &key, value]);
        let j = verdict(&r.measure(&[]), 4, DRIFT);
        assert_eq!(changed(&j), names, "{key}");
        assert!(
            !j.to_string().contains("elsewhere"),
            "{key}: values must not be printed: {j}"
        );
    }
}

#[test]
fn this_branchs_entries_count_at_the_global_level_too() {
    let (r, _) = linked_run(Upstream::OwnBranch);
    r.write("README.md", "# demo, edited\n");
    let global = r.tmp.path().join(".gitconfig");
    git(
        &r.wt,
        &[
            "config",
            "--file",
            global.to_str().unwrap(),
            &format!("branch.{BRANCH}.pushRemote"),
            "/tmp/elsewhere.git",
        ],
    );
    let j = verdict(&r.measure(&[]), 4, DRIFT);
    assert_eq!(changed(&j), ["branch-other"]);
}

#[test]
fn committed_mode_leaves_out_only_this_branchs_upstream() {
    // After step 4 the run cannot hand over, and quick-change pushes with
    // `origin` and an explicit refspec, so --committed does not compare the
    // upstream that its own `git push -u` writes. pushRemote and the rest
    // are still compared.
    let (r, _) = linked_run(Upstream::OwnBranch);
    r.write("README.md", "# demo, edited\n");
    r.commit_all("fix typo");
    git(
        &r.wt,
        &[
            "config",
            &format!("branch.{BRANCH}.merge"),
            "refs/heads/other",
        ],
    );
    verdict(&r.measure(&["--committed"]), 0, PROCEED);
    git(
        &r.wt,
        &[
            "config",
            &format!("branch.{BRANCH}.pushRemote"),
            "/tmp/elsewhere.git",
        ],
    );
    let j = verdict(&r.measure(&["--committed"]), 4, DRIFT);
    assert_eq!(changed(&j), ["branch-other"]);
}

#[test]
fn push_default_at_any_level_is_drift() {
    // push.default decides what a bare `git push` updates (`matching` pushes
    // every branch with a namesake on the remote), and default-workflow
    // pushes that way after a hand-over.
    let (r, _) = linked_run(Upstream::OwnBranch);
    r.write("README.md", "# demo, edited\n");
    let global = r.tmp.path().join(".gitconfig");
    git(
        &r.wt,
        &[
            "config",
            "--file",
            global.to_str().unwrap(),
            "push.default",
            "matching",
        ],
    );
    let j = verdict(&r.measure(&[]), 4, DRIFT);
    assert_eq!(changed(&j), ["push-default"]);
}

#[test]
fn a_new_worktree_does_not_change_the_worktree_configuration_line() {
    // `git config --worktree` fails once a second worktree exists unless
    // extensions.worktreeConfig is set, so the tool reads the per-worktree
    // file itself. Adding a worktree to a single-worktree repository must
    // not change the snapshot.
    let r = Repo::new();
    let snap = std::fs::read_to_string(r.state.join("git-config.snapshot")).unwrap();
    let other = r.tmp.path().join("other-wt");
    git(
        &r.wt,
        &[
            "worktree",
            "add",
            "-q",
            "--no-track",
            "-b",
            "quick/other",
            other.to_str().unwrap(),
            "main",
        ],
    );
    let again = r.run(&["--snapshot", r.wt.to_str().unwrap()], true);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(String::from_utf8_lossy(&again.stdout), snap);

    // A per-worktree file that appears after the snapshot is drift.
    let (r, _) = linked_run(Upstream::OwnBranch);
    r.write("README.md", "# demo, edited\n");
    let file = git(
        &r.wt,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "config.worktree",
        ],
    );
    std::fs::write(file, "[core]\n\tsshCommand = ssh -o ProxyCommand=evil\n").unwrap();
    let j = verdict(&r.measure(&[]), 4, DRIFT);
    assert_eq!(changed(&j), ["config-worktree"]);
}
