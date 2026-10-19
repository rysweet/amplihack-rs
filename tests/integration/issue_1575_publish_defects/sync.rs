//! Defect 1: `workflow_sync_pr_head.sh` reasons and exit codes. Every
//! `reason` in the documented step15 table is checked with its exit code, and
//! pre-probe mode exits 0 in every case. The probe-driven tests live in
//! `sync_probe.rs`.

use std::fs;
use std::path::Path;

use super::{Sandbox, assert_line, dump, step15_line, text, write_file};

const SYNC: &str = "workflow_sync_pr_head.sh";

pub(super) fn sync(
    sb: &Sandbox,
    dir: &Path,
    mode: &str,
    env: &[(&str, &str)],
) -> std::process::Output {
    sb.run_helper(dir, SYNC, &["--mode", mode], env)
}

pub(super) fn commit_ahead(sb: &Sandbox, repo: &Path, line: &str) -> String {
    sb.commit_file(repo, "feature.txt", &format!("f1\n{line}\n"), line);
    sb.head(repo)
}

/// Push a conflicting commit to `origin/feature` from a second clone, so the
/// local branch is both ahead and behind. Returns the remote commit.
pub(super) fn diverge_remote(sb: &Sandbox, origin: &Path) -> String {
    let other = sb.root().join("other");
    sb.git(
        sb.root(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    sb.git(&other, &["switch", "-q", "feature"]);
    sb.commit_file(&other, "feature.txt", "f1\nUPSTREAM\n", "upstream amend");
    sb.git(&other, &["push", "-q", "origin", "feature"]);
    sb.head(&other)
}

#[test]
fn step15_mode_fails_on_divergence_with_structured_reason() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    // The last commit both sides share; the local clone never fetched `remote`.
    let merge_base = sb.bare_ref(&fx.origin, "feature").expect("feature");
    let remote = diverge_remote(&sb, &fx.origin);
    commit_ahead(&sb, &fx.repo, "amended locally");

    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(1), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        format!(
            "pushed=false sha= reason=diverged-upstream: ahead=1 behind=1 merge_base={merge_base} upstream=origin/feature"
        )
    );
    let stderr = text(&output.stderr);
    assert_line(
        &stderr,
        "ERROR: step-15 refuses to rebase divergent history onto upstream 'origin/feature' (issue #978).",
        "step15 divergence header",
    );
    assert!(
        stderr.contains("amend"),
        "the amended-commit hint must be printed\n{stderr}"
    );
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(remote.as_str())
    );
}

// ---------------------------------------------------------------------------
// step15 mode: every documented reason with its exit code.
// ---------------------------------------------------------------------------

#[test]
fn step15_mode_pushes_and_verifies_a_fast_forward() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let head = commit_ahead(&sb, &fx.repo, "new");
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        format!("pushed=true sha={head} reason=pushed")
    );
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(head.as_str())
    );
}

#[test]
fn step15_mode_reports_already_pushed_when_not_ahead() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let head = sb.head(&fx.repo);
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        format!("pushed=true sha={head} reason=already-pushed")
    );
}

#[test]
fn detached_head_is_a_no_op_in_both_modes() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    sb.git(&fx.repo, &["switch", "-q", "--detach", "HEAD"]);
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        "pushed=false sha= reason=detached-head"
    );
    let pre = sync(&sb, &fx.repo, "pre-probe", &[]);
    assert_eq!(pre.status.code(), Some(0), "{}", dump(&pre));
    assert!(pre.stdout.is_empty());
}

#[test]
fn local_tracking_branch_is_a_no_op() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let before = sb.bare_ref(&fx.origin, "feature");
    sb.git(&fx.repo, &["config", "branch.feature.remote", "."]);
    commit_ahead(&sb, &fx.repo, "local");
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        "pushed=false sha= reason=local-tracking"
    );
    assert_eq!(sb.bare_ref(&fx.origin, "feature"), before);
}

#[test]
fn branch_without_a_remote_counterpart_is_a_no_op_and_creates_nothing() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    sb.git(&fx.repo, &["switch", "-q", "-c", "fresh"]);
    sb.commit_file(&fx.repo, "fresh.txt", "x\n", "fresh");
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        "pushed=false sha= reason=no-remote-branch"
    );
    assert_eq!(sb.bare_ref(&fx.origin, "fresh"), None);
    let pre = sync(&sb, &fx.repo, "pre-probe", &[]);
    assert_eq!(pre.status.code(), Some(0), "{}", dump(&pre));
    assert_eq!(sb.bare_ref(&fx.origin, "fresh"), None);
}

fn assert_protected(sb: &Sandbox, fx: &super::RemoteFixture, branch: &str, env: &[(&str, &str)]) {
    let before = sb.bare_ref(&fx.origin, branch);
    let output = sync(sb, &fx.repo, "step15", env);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{branch} must be refused\n{}",
        dump(&output)
    );
    assert_eq!(
        step15_line(&output),
        "pushed=false sha= reason=protected-branch"
    );
    let pre = sync(sb, &fx.repo, "pre-probe", env);
    assert_eq!(pre.status.code(), Some(0), "{}", dump(&pre));
    assert_eq!(
        sb.bare_ref(&fx.origin, branch),
        before,
        "{branch} must not move"
    );
}

#[test]
fn protected_main_and_master_are_never_pushed() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    sb.git(&fx.repo, &["switch", "-q", "main"]);
    sb.commit_file(&fx.repo, "README.md", "base\nlocal\n", "local on main");
    assert_protected(&sb, &fx, "main", &[]);

    sb.git(&fx.repo, &["switch", "-q", "-c", "master"]);
    sb.git(&fx.repo, &["push", "-q", "-u", "origin", "master"]);
    sb.commit_file(
        &fx.repo,
        "README.md",
        "base\nlocal\nmore\n",
        "local on master",
    );
    assert_protected(&sb, &fx, "master", &[]);
}

#[test]
fn configured_base_branch_is_protected_under_both_names() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    sb.git(&fx.repo, &["switch", "-q", "-c", "develop"]);
    sb.git(&fx.repo, &["push", "-q", "-u", "origin", "develop"]);
    sb.commit_file(&fx.repo, "dev.txt", "d\n", "local on develop");
    assert_protected(
        &sb,
        &fx,
        "develop",
        &[("RECIPE_VAR_base_branch", "develop")],
    );
    assert_protected(&sb, &fx, "develop", &[("BASE_BRANCH", "develop")]);
}

#[test]
fn remote_head_target_is_protected() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    sb.git(&fx.repo, &["switch", "-q", "-c", "trunk"]);
    sb.git(&fx.repo, &["push", "-q", "-u", "origin", "trunk"]);
    sb.git(&fx.repo, &["remote", "set-head", "origin", "trunk"]);
    sb.commit_file(&fx.repo, "t.txt", "t\n", "local on trunk");
    assert_protected(&sb, &fx, "trunk", &[]);
}

#[test]
fn invalid_or_unknown_remote_names_are_refused() {
    for remote in ["bad remote", "ghost", "-oops"] {
        let sb = Sandbox::new();
        let fx = sb.remote_fixture();
        sb.git(&fx.repo, &["config", "branch.feature.remote", remote]);
        commit_ahead(&sb, &fx.repo, "local");
        let output = sync(&sb, &fx.repo, "step15", &[]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "remote `{remote}`\n{}",
            dump(&output)
        );
        assert_eq!(step15_line(&output), "pushed=false sha= reason=invalid-ref");
    }
}

#[test]
fn bad_arguments_exit_2() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    for args in [
        vec![],
        vec!["--mode"],
        vec!["--mode", "bogus"],
        vec!["--force"],
    ] {
        let output = sb.run_helper(&fx.repo, SYNC, &args, &[]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "args {args:?}\n{}",
            dump(&output)
        );
        assert!(output.stdout.is_empty(), "{}", dump(&output));
    }
}

#[test]
fn rejected_push_reports_push_failed_and_redacts_secrets() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let hook = fx.origin.join("hooks/pre-receive");
    write_file(
        &hook,
        "#!/usr/bin/env bash\necho 'denied for https://bob:hunter2secret@example.com token ghp_ABCDEFGHIJKLMNOP1234 Bearer abcdefghijkl1234' >&2\nexit 1\n",
    );
    super::make_executable(&hook);
    let before = sb.bare_ref(&fx.origin, "feature");
    commit_ahead(&sb, &fx.repo, "local");

    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert!(!output.status.success(), "{}", dump(&output));
    assert!(step15_line(&output).starts_with("pushed=false sha= reason=push-failed: "));
    let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
    for secret in [
        "hunter2secret",
        "ghp_ABCDEFGHIJKLMNOP1234",
        "abcdefghijkl1234",
    ] {
        assert!(!all.contains(secret), "`{secret}` must be redacted\n{all}");
    }
    assert_eq!(sb.bare_ref(&fx.origin, "feature"), before);
    let pre = sync(&sb, &fx.repo, "pre-probe", &[]);
    assert_eq!(
        pre.status.code(),
        Some(0),
        "pre-probe exits 0 on push failure\n{}",
        dump(&pre)
    );
}

#[test]
fn remote_ref_moved_after_push_reports_push_verify_failed() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let decoy = sb.bare_ref(&fx.origin, "main").expect("main");
    let hook = fx.origin.join("hooks/post-receive");
    write_file(
        &hook,
        &format!("#!/usr/bin/env bash\ngit update-ref refs/heads/feature {decoy}\n"),
    );
    super::make_executable(&hook);
    commit_ahead(&sb, &fx.repo, "local");
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(1), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        "pushed=false sha= reason=push-verify-failed"
    );
}

#[test]
fn pre_probe_warns_and_exits_0_when_the_fetch_fails() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    commit_ahead(&sb, &fx.repo, "local");
    let gone = sb.root().join("gone.git");
    sb.git(
        &fx.repo,
        &["remote", "set-url", "origin", gone.to_str().unwrap()],
    );
    let output = sync(&sb, &fx.repo, "pre-probe", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("WARNING"),
        "{}",
        dump(&output)
    );
}

#[test]
fn reason_never_contains_newlines_or_the_remote_url() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    commit_ahead(&sb, &fx.repo, "local");
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    let line = step15_line(&output);
    assert!(line.starts_with("pushed=true sha="), "{line}");
    let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
    assert!(
        !all.contains(fx.origin.to_str().unwrap()),
        "only the remote name may be printed, never its URL\n{all}"
    );
}

/// Delete `feature` on the bare remote while the clone keeps its unpruned
/// `refs/remotes/origin/feature`, then commit locally so there is work to push.
fn delete_remote_branch_keep_tracking_ref(sb: &Sandbox, fx: &super::RemoteFixture) {
    sb.git(&fx.origin, &["update-ref", "-d", "refs/heads/feature"]);
    commit_ahead(sb, &fx.repo, "work after the remote branch was deleted");
    sb.git(
        &fx.repo,
        &["rev-parse", "--verify", "refs/remotes/origin/feature"],
    );
}

#[test]
fn step15_mode_fails_loudly_when_the_remote_branch_was_deleted() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    delete_remote_branch_keep_tracking_ref(&sb, &fx);
    let output = sync(&sb, &fx.repo, "step15", &[]);
    assert_eq!(output.status.code(), Some(1), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        "pushed=false sha= reason=remote-branch-deleted"
    );
    let stderr = text(&output.stderr);
    assert_line(
        &stderr,
        "ERROR: step-15: 'origin/feature' was deleted on origin (possibly auto-deleted after a merge) but refs/remotes/origin/feature remains; committed work was not pushed",
        "deleted remote branch",
    );
    for cmd in [
        "git push -u -- origin HEAD:refs/heads/feature",
        "git fetch --prune origin",
    ] {
        assert!(
            stderr.contains(cmd),
            "recovery command `{cmd}` missing\n{stderr}"
        );
    }
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature"),
        None,
        "the helper must never recreate a deleted remote branch"
    );
}

#[test]
fn pre_probe_warns_and_exits_0_when_the_remote_branch_was_deleted() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    delete_remote_branch_keep_tracking_ref(&sb, &fx);
    let output = sync(&sb, &fx.repo, "pre-probe", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert!(output.stdout.is_empty(), "{}", dump(&output));
    assert_line(
        &text(&output.stderr),
        "WARNING: publish-sync-pr-head: 'origin/feature' was deleted on origin but refs/remotes/origin/feature remains; not recreating it",
        "pre-probe deleted remote branch",
    );
    assert_eq!(sb.bare_ref(&fx.origin, "feature"), None);
}

#[test]
fn verify_helper_stdout_never_replaces_the_result_line() {
    // A verify helper found through the lookup list must not be able to print
    // its own `pushed=…` line ahead of the real one.
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let bundle = sb.root().join("bundle");
    let tools = bundle.join("amplifier-bundle/tools");
    fs::create_dir_all(&tools).expect("tools dir");
    fs::copy(super::helper_path(SYNC), tools.join(SYNC)).expect("copy sync helper");
    write_file(
        &tools.join("verify_pushed_ref.sh"),
        "#!/usr/bin/env bash\necho 'pushed=true sha=deadbeef reason=forged'\nexit 0\n",
    );
    let head = commit_ahead(&sb, &fx.repo, "new");
    let output = sb
        .command("bash", &fx.repo)
        .arg(tools.join(SYNC))
        .args(["--mode", "step15"])
        .output()
        .expect("run copied sync helper");
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        step15_line(&output),
        format!("pushed=true sha={head} reason=pushed")
    );
}
