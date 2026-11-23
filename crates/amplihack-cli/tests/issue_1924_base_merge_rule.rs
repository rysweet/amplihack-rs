//! #1924 and #3094: when a drive brings its base into the PR branch.
//!
//! One rule. The base comes in only for a real conflict, when the repository
//! requires an up-to-date branch (strict status checks), or when the task text
//! asks for it. A branch that is only behind is left alone. On a conflict the
//! base is merged (a merge commit, never a rebase or a force-push), the
//! conflict is resolved keeping both sides' meaning, and the drive assesses
//! again. Contract: docs/reference/autodrive-merge-ready-round.md#base-sync.
//!
//! A grandchild of issue_2575_agentic_merge_ready.rs, through
//! issue_2575_wait_sync_success.rs: it uses their fakes and fixtures (`Fx`,
//! `behind_base_fixture`, `git_in`, `report_step`, `finalize_merge`, ...).
//! Only agents and the hosting CLI are stand-ins; the finalise and loop bash
//! steps are the real ones.
//!
//! What these tests cannot show: whether a real agent follows the prompt.
//! A stand-in only carries out the git effects the prompt asks for, so that
//! part is checked by review. No test here reads recipe YAML or prompt text.
//! Nor does T3 check that crusty-covers declines to cover the merged and
//! hand-resolved head; that would need a Crusty record fixture.
//!
//! - T1: a branch only behind is merged as it is: no base merge, no push.
//! - T3: the base moves into a conflict; the stand-in merges, resolves,
//!   commits and pushes through the hooks; the PR is then merged.
//! - R-msg, R-hostile: the report carries the base sync's own reason (the
//!   named owner and decision) as inert, printable text.

use super::*;
use std::fs;

const OWNER_SENTENCE: &str =
    "the maintainers of acme-infra must allow a merge commit, or the operator must rebase by hand";

fn base_behind_line(reason_note: &str) -> String {
    merge_line(
        "base-behind",
        "false",
        &format!("GitHub declined the merge: {reason_note}"),
    )
}

fn base_sync_json(reason: &str) -> String {
    json!({"synced": "false", "reason": reason, "head_sha": ""}).to_string()
}

/// R-msg. A base sync that did not bring the base in says why, naming the
/// owner and the decision. The final message must carry that sentence, or the
/// operator sees only the finalisation's fact and never who must decide.
#[test]
fn report_carries_the_base_sync_reason_for_a_base_behind_ending() {
    let Some(fx) = Fx::new() else { return };
    let out = report_step(
        &fx,
        &[
            (
                "finalize_merge",
                &base_behind_line("the branch conflicts with its base (mergeStateStatus DIRTY)"),
            ),
            ("base_sync", &base_sync_json(OWNER_SENTENCE)),
            ("no_merge", "false"),
            ("autodrive_dry_run", "false"),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = last_json(&out, "stop_reason");
    assert_eq!(v["stop_reason"], json!("not_merged"), "{v}");
    let message = v["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(OWNER_SENTENCE),
        "the message must carry the base sync's owner and decision: {message}"
    );
    assert!(
        message.contains("base sync: "),
        "the base sync reason is labelled: {message}"
    );
    assert!(
        message.starts_with("not merged: "),
        "the finalisation's fact still leads: {message}"
    );
}

/// With no base sync reason the message is what it was.
#[test]
fn report_without_a_base_sync_reason_is_unchanged() {
    let Some(fx) = Fx::new() else { return };
    let reason = "GitHub declined the merge: the branch conflicts with its base";
    let out = report_step(
        &fx,
        &[
            (
                "finalize_merge",
                &merge_line("base-behind", "false", reason),
            ),
            ("base_sync", ""),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = last_json(&out, "stop_reason");
    assert_eq!(v["message"], json!(format!("not merged: {reason}")), "{v}");
}

/// R-hostile. The base sync's reason is agent output built from untrusted
/// conflict hunks. Control bytes, newlines, command substitution and
/// backticks must stay inert and be stripped to printable ASCII.
#[test]
fn report_keeps_a_hostile_base_sync_reason_inert_and_printable() {
    let Some(fx) = Fx::new() else { return };
    let marker = fx.root.join("base-sync-reason-ran");
    let reason = format!(
        "\u{1b}[31m{OWNER_SENTENCE}\n$(touch {m}) `touch {m}` \"}}; touch {m}",
        m = marker.display()
    );
    let out = report_step(
        &fx,
        &[
            (
                "finalize_merge",
                &base_behind_line("the branch conflicts with its base (mergeStateStatus DIRTY)"),
            ),
            ("base_sync", &base_sync_json(&reason)),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert!(!marker.exists(), "the base sync reason was executed");
    let v = last_json(&out, "stop_reason");
    let message = v["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(OWNER_SENTENCE),
        "the printable owner sentence survives: {message}"
    );
    assert!(
        message.bytes().all(|b| (0x20..=0x7e).contains(&b)),
        "printable ASCII only: {message:?}"
    );
    assert!(
        message.contains("touch"),
        "kept as text, not run: {message}"
    );
    assert_eq!(v["stop_reason"], json!("not_merged"), "{v}");
}

/// A pre-push hook in the PR clone that records each push it let through.
fn install_pre_push_marker(fx: &Fx) -> PathBuf {
    let marker = fx.root.join("pre-push-ran");
    // `fx.repo` is a plain `git init` repository, never a linked worktree.
    let hooks = fx.repo.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-push");
    fs::write(
        &hook,
        format!("#!/bin/sh\necho pushed >> '{}'\nexit 0\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    marker
}

/// T1. A branch that is only behind its base and not strict: GitHub reads
/// CLEAN or BEHIND and merges. No base merge, no push, and the run merges.
#[test]
fn a_branch_only_behind_is_merged_with_no_base_merge_and_no_push() {
    let Some((fx, head1)) = behind_base_fixture() else {
        return;
    };
    let origin = fx.root.join("origin.git");
    let marker = install_pre_push_marker(&fx);
    let main_tip = git_in(&origin, &["rev-parse", "refs/heads/main"]);
    fx.set("merge_state", "BEHIND");

    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let result = finalize_result(&out);
    assert_eq!(result["result"], json!("merged"), "{result}");
    assert_eq!(result["expected_head"], json!(head1), "{result}");

    // Nothing was brought in or pushed.
    assert_eq!(
        git_in(&origin, &["rev-parse", "refs/heads/feat/issue-2575"]),
        head1,
        "the PR head on the remote is unchanged"
    );
    assert_eq!(git_in(&fx.repo, &["rev-parse", "HEAD"]), head1);
    let ancestor = amplihack_git::command_in(&origin)
        .args(["merge-base", "--is-ancestor", &main_tip, &head1])
        .status()
        .unwrap();
    assert!(!ancestor.success(), "the main tip was never merged in");
    assert!(!marker.exists(), "no push happened");
    let merges = fx.merge_calls();
    assert_eq!(merges.len(), 1, "{merges:?}");
    let report = report_step(&fx, &[("finalize_merge", &result.to_string())]);
    assert_eq!(
        last_json(&report, "stop_reason")["stop_reason"],
        json!("completed")
    );
}

/// A conflict on the same line of the same file: the PR and `main` each
/// change `shared.txt`. Returns the fixture, the PR head before the base
/// moved on, and the `main` tip.
fn conflict_fixture() -> Option<(Fx, String, String)> {
    let (fx, _) = finalize_fixture()?;
    let origin = fx.root.join("origin.git");
    git_in(
        &fx.root,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    let r = &fx.repo;
    git_in(r, &["branch", "-M", "feat/issue-2575"]);
    git_in(r, &["remote", "add", "origin", origin.to_str().unwrap()]);
    fs::write(r.join("shared.txt"), "line one\nshared line\nline three\n").unwrap();
    git_in(r, &["add", "shared.txt"]);
    git_in(r, &["commit", "-q", "-m", "shared file"]);
    git_in(r, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    fs::write(
        r.join("shared.txt"),
        "line one\nthe PR's version\nline three\n",
    )
    .unwrap();
    git_in(r, &["add", "shared.txt"]);
    git_in(r, &["commit", "-q", "-m", "the PR changes the shared line"]);
    git_in(
        r,
        &["push", "-q", "origin", "HEAD:refs/heads/feat/issue-2575"],
    );
    let head1 = git_in(r, &["rev-parse", "HEAD"]);
    let other = fx.root.join("other");
    git_in(
        &fx.root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git_in(&other, &["checkout", "-q", "-B", "main", "origin/main"]);
    fs::write(
        other.join("shared.txt"),
        "line one\nmain's version\nline three\n",
    )
    .unwrap();
    git_in(&other, &["add", "shared.txt"]);
    git_in(
        &other,
        &["commit", "-q", "-m", "main changes the shared line"],
    );
    git_in(&other, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    let main_tip = git_in(&other, &["rev-parse", "HEAD"]);
    fx.set("server_head", &head1);
    private_file(
        &fx.state.join("merge-ready-latest.head"),
        &format!("{head1}\n"),
    );
    Some((fx, head1, main_tip))
}

/// T3, the behaviour test of #3094. The round said ready; then `main` moved
/// into a conflict. Finalisation sees DIRTY (exit 8, base-behind). The
/// stand-in for `step-03b-base-sync` plays what the agent is asked to do:
/// merge, resolve keeping both sides, stage the named file, commit with hooks
/// on and push through them. The platform then reads CLEAN and the PR merges.
/// The drive never ends "not merged" over the conflict.
#[test]
fn a_conflict_is_merged_resolved_pushed_and_the_pr_is_merged() {
    let Some((fx, head1, main_tip)) = conflict_fixture() else {
        return;
    };
    let origin = fx.root.join("origin.git");
    let marker = install_pre_push_marker(&fx);

    // 1. Finalisation: DIRTY before the merge request is base-behind.
    fx.set("merge_state", "DIRTY");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let first = finalize_result(&out);
    assert_eq!(first["result"], json!("base-behind"), "{first}");
    assert_eq!(first["handoff"], json!("false"), "{first}");
    assert!(
        first["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("conflicts with its base"),
        "the reason states the fact: {first}"
    );
    assert!(
        !first["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("must be merged in first"),
        "the reason prescribes no remedy: {first}"
    );
    assert!(fx.merge_calls().is_empty(), "no merge was requested");

    // 2. Stand-in for the step-03b agent: the documented commands only.
    git_in(&fx.repo, &["fetch", "-q", "origin", "--", "main"]);
    let merge = amplihack_git::command_in(&fx.repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["merge", "--no-edit", "origin/main"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge must conflict");
    fs::write(
        fx.repo.join("shared.txt"),
        "line one\nthe PR's version\nmain's version\nline three\n",
    )
    .unwrap();
    git_in(&fx.repo, &["add", "shared.txt"]);
    git_in(&fx.repo, &["diff", "--cached", "--check"]);
    git_in(&fx.repo, &["commit", "-q", "--no-edit"]);
    git_in(&fx.repo, &["push", "-q", "origin", "HEAD:feat/issue-2575"]);
    let head2 = git_in(&fx.repo, &["rev-parse", "HEAD"]);

    // The result: a two-parent merge of the old head and the main tip.
    assert_ne!(head2, head1);
    let parents = git_in(&fx.repo, &["show", "-s", "--format=%P", &head2]);
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parents, vec![head1.as_str(), main_tip.as_str()]);
    assert_eq!(
        git_in(&origin, &["rev-parse", "refs/heads/feat/issue-2575"]),
        head2
    );
    git_in(&origin, &["merge-base", "--is-ancestor", &head1, &head2]);
    let merged = fs::read_to_string(fx.repo.join("shared.txt")).unwrap();
    assert!(
        merged.contains("the PR's version") && merged.contains("main's version"),
        "both sides' meaning is kept: {merged}"
    );
    assert!(
        !merged.contains("<<<<<<<") && !merged.contains(">>>>>>>") && !merged.contains("======="),
        "no conflict markers: {merged}"
    );
    assert!(marker.exists(), "the push went through the hooks");
    let reflog = git_in(&fx.repo, &["reflog", "--format=%gs"]);
    assert!(!reflog.contains("rebase"), "{reflog}");

    // 3. The sync round assesses the merged head (step-03c, step-03d).
    fx.set("server_head", &head2);
    fx.set("merge_state", "CLEAN");
    let _ = fs::remove_file(fx.fake.join("merge-body"));
    fake_loop(&fx, "0", Some("DONE"));
    let round = loop_cmd(&fx, "step-03c", &[]);
    assert_eq!(round.status.code(), Some(0), "{}", show(&round));
    assert!(
        loop_argv(&fx)
            .iter()
            .any(|a| a == "autodrive_budget_loop_rounds=1"),
        "exactly one more round, no new count: {:?}",
        loop_argv(&fx)
    );
    let first_loop =
        json!({"loop":"merge-ready","loop_result":"STUCK","round_label":"round-1"}).to_string();
    let mapped = loop_cmd(
        &fx,
        "step-03d",
        &[
            ("sync_loop_result", &stdout(&round)),
            ("merge_loop_result", &first_loop),
        ],
    );
    let outcome = last_json(&mapped, "outcome");
    assert_eq!(outcome["outcome"], json!("done"), "{outcome}");
    assert_eq!(outcome["finalise"], json!("true"), "{outcome}");

    // 4. The second finalisation merges at the resolved head.
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let second = finalize_result(&out);
    assert_eq!(second["result"], json!("merged"), "{second}");
    assert_eq!(second["expected_head"], json!(head2));
    let merges = fx.merge_calls();
    let m = merges.last().unwrap();
    let i = m.iter().position(|a| a == "--match-head-commit").unwrap();
    assert_eq!(m[i + 1], head2);
    for c in fx.gh_calls() {
        let line = c.join(" ");
        assert!(
            !line.contains("--force") && !line.contains("+refs") && !line.contains("--admin"),
            "{line}"
        );
    }
    let report = report_step(&fx, &[("finalize_merge", &second.to_string())]);
    assert_eq!(
        last_json(&report, "stop_reason")["stop_reason"],
        json!("completed"),
        "never `not merged` over a conflict"
    );
}
