use std::fs;

use super::fixture::{BRANCH, Fixture, stderr, success};

#[test]
fn caller_checkout_is_reused_with_exact_identity_and_all_dirty_work() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.write(&f.caller, "notes.md", "staged review notes\n");
    f.git(&f.caller, &["add", "notes.md"]);
    f.write(
        &f.caller,
        "notes.md",
        "staged review notes\nunstaged review notes\n",
    );
    f.write(&f.caller, "untracked.md", "untracked review notes\n");
    let before = f.git(&f.caller, &["status", "--porcelain=v1"]);
    let index = f.git(&f.caller, &["diff", "--cached"]);
    let head = f.git(&f.caller, &["rev-parse", "HEAD"]);
    let refs = f.git(
        &f.caller,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ],
    );
    success(&f.select());
    assert_eq!(f.selected(), f.caller.canonicalize().unwrap());
    assert_eq!(f.ctx.get("worktree_setup.branch_name").unwrap(), BRANCH);
    assert_eq!(f.git(&f.selected(), &["status", "--porcelain=v1"]), before);
    assert_eq!(f.git(&f.selected(), &["diff", "--cached"]), index);
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        f.git(
            &f.caller,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads"
            ]
        ),
        refs,
        "setup must create no alias"
    );
    assert_eq!(success(&f.qc("record-base")), head);
}

#[test]
fn registered_alternate_checkout_with_spaces_is_reused_without_sweeping_its_work() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    let other = f.tmp.path().join("review checkout with spaces");
    f.git(
        &f.caller,
        &["worktree", "add", other.to_str().unwrap(), BRANCH],
    );
    f.write(&other, "notes.md", "keep this review work\n");
    f.write(&other, "untracked.md", "keep this too\n");
    let before = f.git(&other, &["status", "--porcelain=v1"]);
    success(&f.select_direct());
    assert_eq!(f.selected(), other.canonicalize().unwrap());
    assert_eq!(f.git(&other, &["status", "--porcelain=v1"]), before);
    assert_eq!(f.git(&f.caller, &["branch", "--show-current"]), "main");
}

#[test]
fn unregistered_local_branch_keeps_commits_ahead_of_origin() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.write(&f.caller, "local.md", "local commit absent from origin\n");
    f.git(&f.caller, &["add", "local.md"]);
    f.git(&f.caller, &["commit", "-qm", "local-only commit"]);
    let head = f.git(&f.caller, &["rev-parse", "HEAD"]);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    success(&f.select_direct());
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), head);
    assert_eq!(f.git(&f.selected(), &["branch", "--show-current"]), BRANCH);
    assert_eq!(success(&f.qc("record-base")), head);
}

#[test]
fn remote_only_branch_is_checked_out_under_its_exact_name() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    let remote_head = f.git(&f.caller, &["rev-parse", "HEAD"]);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    f.git(&f.caller, &["branch", "-D", BRANCH]);
    f.git(
        &f.caller,
        &["update-ref", "-d", &format!("refs/remotes/origin/{BRANCH}")],
    );
    success(&f.select_direct());
    assert_eq!(f.git(&f.selected(), &["branch", "--show-current"]), BRANCH);
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), remote_head);
    assert_eq!(
        f.git(&f.selected(), &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        format!("origin/{BRANCH}")
    );
    let trace = f.read_home("git-trace");
    let lookups: Vec<_> = trace
        .lines()
        .filter(|line| line.contains("built-in: git ls-remote"))
        .collect();
    assert_eq!(lookups.len(), 1, "setup must make one exact origin lookup");
    assert!(
        lookups[0].contains("--exit-code") && lookups[0].contains(&format!("refs/heads/{BRANCH}")),
        "{}",
        lookups[0]
    );
    assert!(
        trace.find("built-in: git ls-remote").unwrap() < trace.find("built-in: git fetch").unwrap(),
        "remote validation must precede fetch and checkout"
    );
    assert_eq!(success(&f.qc("record-base")), remote_head);
}

fn assert_remote_refusal(f: &mut Fixture) {
    let before = f.git(&f.origin, &["show-ref"]);
    let out = f.select_direct();
    assert!(
        !out.status.success(),
        "a branch absent from origin was accepted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
    assert_eq!(
        f.git(&f.origin, &["show-ref"]),
        before,
        "remote validation must precede any setup push"
    );
    assert!(f.ctx.get("worktree_setup.worktree_path").is_none());
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn missing_remote_branch_is_refused_by_name_before_publication() {
    assert_remote_refusal(&mut Fixture::new(BRANCH));
}

#[test]
fn local_only_branch_is_refused_without_publishing_it() {
    let mut f = Fixture::new(BRANCH);
    f.branch(false);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    assert_remote_refusal(&mut f);
}

#[test]
fn stale_tracking_ref_and_similar_remote_name_do_not_prove_remote_existence() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    f.git(
        &f.origin,
        &["update-ref", "-d", &format!("refs/heads/{BRANCH}")],
    );
    f.git(
        &f.caller,
        &["push", "-q", "origin", &format!("{BRANCH}:team/{BRANCH}")],
    );
    assert_remote_refusal(&mut f);
}

#[test]
fn inaccessible_registered_checkout_is_refused_without_replacement() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    let old = f.tmp.path().join("registered");
    let moved = f.tmp.path().join("moved");
    f.git(
        &f.caller,
        &["worktree", "add", old.to_str().unwrap(), BRANCH],
    );
    fs::rename(&old, &moved).unwrap();
    let refs = f.git(&f.caller, &["show-ref"]);
    let out = f.select_direct();
    assert!(
        !out.status.success(),
        "must not prune and replace an inaccessible registration"
    );
    assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
    assert!(moved.join("old-work.md").exists());
    assert!(!old.exists());
    assert_eq!(f.git(&f.caller, &["show-ref"]), refs);
}

#[test]
fn conflicting_destination_preserves_files_instead_of_removing_them() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    let base = success(
        &f.command("bash")
            .arg(super::fixture::root().join("amplifier-bundle/tools/workflow_worktree_root.sh"))
            .args(["base", f.caller.to_str().unwrap()])
            .output()
            .unwrap(),
    );
    let conflict = std::path::Path::new(&base).join(BRANCH);
    f.write(&conflict, "precious.md", "do not discard\n");
    let out = f.select_direct();
    assert!(
        !out.status.success(),
        "must refuse a non-worktree destination"
    );
    assert_eq!(
        fs::read_to_string(conflict.join("precious.md")).unwrap(),
        "do not discard\n"
    );
}

#[test]
fn origin_access_failure_is_distinct_from_a_missing_branch_and_has_no_retries() {
    let mut f = Fixture::new(BRANCH);
    f.branch(false);
    f.git(
        &f.caller,
        &[
            "remote",
            "set-url",
            "origin",
            f.tmp.path().join("absent.git").to_str().unwrap(),
        ],
    );
    let out = f.select_direct();
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains(BRANCH) && err.contains("origin"), "{err}");
    assert!(
        ["could not", "cannot", "unable", "failed", "access"]
            .iter()
            .any(|word| err.to_lowercase().contains(word)),
        "origin-access failure lacks its own diagnostic: {err}"
    );
    assert!(
        !err.contains("does not exist on origin"),
        "access failure misreported as absence: {err}"
    );
    let trace = f.read_home("git-trace");
    assert_eq!(
        trace
            .lines()
            .filter(|l| l.contains("built-in: git ls-remote"))
            .count(),
        1,
        "one exact lookup, without retries: {trace}"
    );
}
