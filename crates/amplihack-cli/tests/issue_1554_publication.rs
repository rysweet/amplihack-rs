//! #1554: real Git, actual recipe adapters and local destination advertisements.
//! The nine-case matrix is reconstructed from the issue's three phases and
//! three advertised-sibling states. It does not claim recovery of cc's packet.

use crate::issue_1554_fixture::{Repo, describe, executable, success, write};
use std::fs;

const PHASES: &[(&str, &str)] = &[
    ("workflow-publish", "step-15-commit-push"),
    ("workflow-pr-review", "step-18c-push-feedback-changes"),
    ("workflow-finalize", "step-20b-push-cleanup"),
];

fn sibling(r: &Repo, path: &str) {
    let sibling = r.root.join("sibling");
    r.git(
        &r.root,
        &[
            "clone",
            "-q",
            r.remote.to_str().unwrap(),
            sibling.to_str().unwrap(),
        ],
    );
    r.git(&sibling, &["config", "user.name", "Sibling"]);
    r.git(
        &sibling,
        &["config", "user.email", "sibling@example.invalid"],
    );
    r.git(
        &sibling,
        &["checkout", "-q", "-b", "feat/1554", "origin/feat/1554"],
    );
    r.commit_file(&sibling, path, "already advertised sibling change\n");
    r.git(&sibling, &["push", "-q", "origin", "HEAD"]);
}

fn matrix_case(phase: usize, sibling_path: Option<&str>) {
    let r = Repo::new();
    r.install_binary();
    if let Some(path) = sibling_path {
        sibling(&r, path);
    }
    r.commit_file(&r.work, "src/new.rs", "authorised outgoing task source\n");
    let (recipe, step) = PHASES[phase];
    let out = r.step_command(recipe, step).output().unwrap();
    success(&out);
    assert_eq!(
        r.oid(&r.remote, "refs/heads/feat/1554"),
        r.oid(&r.work, "HEAD"),
        "{recipe} reported success without publishing the task tip"
    );
    assert_eq!(
        r.git(&r.remote, &["show", "refs/heads/feat/1554:src/new.rs"]),
        b"authorised outgoing task source\n"
    );
    if let Some(path) = sibling_path {
        assert_eq!(
            r.git(
                &r.remote,
                &["show", &format!("refs/heads/feat/1554:{path}")]
            ),
            b"already advertised sibling change\n",
            "advertised imports must survive"
        );
    }
}

macro_rules! matrix {
    ($name:ident, $phase:expr, $sibling:expr) => {
        #[test]
        fn $name() {
            crate::issue_1554_fixture::sandbox(module_path!(), stringify!($name), || {
                matrix_case($phase, $sibling)
            });
        }
    };
}
matrix!(publish_without_sibling, 0, None);
matrix!(publish_with_allowed_path_sibling, 0, Some("src/allowed.rs"));
matrix!(
    publish_with_unrelated_path_sibling,
    0,
    Some("unrelated.txt")
);
matrix!(feedback_without_sibling, 1, None);
matrix!(
    feedback_with_allowed_path_sibling,
    1,
    Some("src/allowed.rs")
);
matrix!(
    feedback_with_unrelated_path_sibling,
    1,
    Some("unrelated.txt")
);
matrix!(cleanup_without_sibling, 2, None);
matrix!(cleanup_with_allowed_path_sibling, 2, Some("src/allowed.rs"));
matrix!(
    cleanup_with_unrelated_path_sibling,
    2,
    Some("unrelated.txt")
);

fn assert_scope_refusal(r: &Repo, phase: usize) {
    let before = r.oid(&r.remote, "refs/heads/feat/1554");
    let entries = r.staged();
    let (recipe, step) = PHASES[phase];
    let out = r.step_command(recipe, step).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(78),
        "{recipe} must propagate scope refusal: {}",
        describe(&out)
    );
    assert_eq!(
        r.oid(&r.remote, "refs/heads/feat/1554"),
        before,
        "refused publication changed the destination"
    );
    assert_eq!(
        r.staged(),
        entries,
        "publication changed unrelated staged entries"
    );
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        diagnostics.contains("unrelated.txt"),
        "scope refusal must identify the path: {diagnostics}"
    );
}

#[test]
fn every_phase_returns_78_for_an_unauthorised_outgoing_commit() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "every_phase_returns_78_for_an_unauthorised_outgoing_commit",
        || {
            for phase in 0..PHASES.len() {
                let r = Repo::new();
                r.install_binary();
                r.commit_file(&r.work, "unrelated.txt", "unauthorised outgoing change\n");
                assert_scope_refusal(&r, phase);
            }
        },
    );
}

#[test]
fn intermediate_unauthorised_change_then_revert_is_still_outgoing_history() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "intermediate_unauthorised_change_then_revert_is_still_outgoing_history",
        || {
            let r = Repo::new();
            r.install_binary();
            let bad = r.commit_file(&r.work, "unrelated.txt", "must not be published\n");
            r.git(&r.work, &["revert", "--no-edit", &bad]);
            assert!(
                r.git(&r.work, &["diff", &r.base, "HEAD"]).is_empty(),
                "fixture final diff must be empty"
            );
            assert_scope_refusal(&r, 0);
        },
    );
}

#[test]
fn forged_tracking_refs_and_fetch_head_cannot_hide_outgoing_commits() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "forged_tracking_refs_and_fetch_head_cannot_hide_outgoing_commits",
        || {
            for forge in ["tracking", "fetch-head", "both"] {
                let r = Repo::new();
                r.install_binary();
                let bad = r.commit_file(&r.work, "unrelated.txt", "unauthorised outgoing change\n");
                if forge != "fetch-head" {
                    r.git(
                        &r.work,
                        &["update-ref", "refs/remotes/origin/feat/1554", &bad],
                    );
                }
                if forge != "tracking" {
                    let path = r.git(
                        &r.work,
                        &[
                            "rev-parse",
                            "--path-format=absolute",
                            "--git-path",
                            "FETCH_HEAD",
                        ],
                    );
                    write(
                        String::from_utf8(path).unwrap().trim(),
                        format!("{bad}\t\tbranch 'feat/1554' of forged-local-reference\n"),
                    );
                }
                assert_scope_refusal(&r, 0);
            }
        },
    );
}

#[test]
fn every_configured_pushed_ref_is_validated_before_any_update() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "every_configured_pushed_ref_is_validated_before_any_update",
        || {
            let r = Repo::new();
            r.install_binary();
            r.git(&r.work, &["checkout", "-q", "-b", "extra"]);
            r.commit_file(&r.work, "unrelated.txt", "unauthorised second ref\n");
            r.git(&r.work, &["checkout", "-q", "feat/1554"]);
            r.commit_file(&r.work, "src/new.rs", "authorised first ref\n");
            r.git(
                &r.work,
                &[
                    "config",
                    "--add",
                    "remote.origin.push",
                    "refs/heads/feat/1554:refs/heads/feat/1554",
                ],
            );
            r.git(
                &r.work,
                &[
                    "config",
                    "--add",
                    "remote.origin.push",
                    "refs/heads/extra:refs/heads/extra",
                ],
            );
            assert_scope_refusal(&r, 1);
            assert!(
                !r.git_output(&r.remote, &["rev-parse", "--verify", "refs/heads/extra"])
                    .status
                    .success()
            );
        },
    );
}

#[test]
fn new_remote_branch_subtracts_all_advertised_tips_including_other_branches() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "new_remote_branch_subtracts_all_advertised_tips_including_other_branches",
        || {
            let r = Repo::new();
            r.install_binary();
            // This commit is unrelated to the task but already belongs to the remote
            // under another ref. It must not become outgoing merely because the task
            // destination is new.
            r.git(&r.main, &["checkout", "-q", "-b", "imported"]);
            let imported = r.commit_file(
                &r.main,
                "unrelated.txt",
                "advertised on a different branch\n",
            );
            r.git(&r.main, &["push", "-q", "origin", "HEAD"]);
            r.git(&r.work, &["merge", "--ff-only", &imported]);
            r.commit_file(&r.work, "src/new.rs", "authorised new branch source\n");
            r.git(&r.work, &["push", "origin", ":refs/heads/feat/1554"]);
            r.git(&r.work, &["branch", "--unset-upstream"]);
            let out = r.step_command(PHASES[0].0, PHASES[0].1).output().unwrap();
            success(&out);
            assert_eq!(
                r.oid(&r.remote, "refs/heads/feat/1554"),
                r.oid(&r.work, "HEAD")
            );
        },
    );
}

fn imported_merge(r: &Repo, unauthorised_resolution: bool) {
    r.git(&r.main, &["checkout", "-q", "-b", "imported"]);
    let imported = r.commit_file(&r.main, "unrelated.txt", "remote import\n");
    r.git(&r.main, &["push", "-q", "origin", "HEAD"]);
    r.commit_file(&r.work, "src/new.rs", "authorised outgoing source\n");
    r.git(&r.work, &["merge", "--no-ff", "--no-commit", &imported]);
    if unauthorised_resolution {
        write(
            r.work.join("unrelated.txt"),
            "unauthorised merge resolution\n",
        );
        r.git(&r.work, &["add", "--", "unrelated.txt"]);
    }
    r.git(
        &r.work,
        &["commit", "-qm", "merge advertised remote history"],
    );
}

#[test]
fn merging_advertised_unrelated_commits_is_an_allowed_import() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "merging_advertised_unrelated_commits_is_an_allowed_import",
        || {
            let r = Repo::new();
            r.install_binary();
            imported_merge(&r, false);
            success(&r.step_command(PHASES[0].0, PHASES[0].1).output().unwrap());
            assert_eq!(
                r.oid(&r.remote, "refs/heads/feat/1554"),
                r.oid(&r.work, "HEAD")
            );
        },
    );
}

#[test]
fn advertised_merge_parent_does_not_authorise_resolution_changes() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "advertised_merge_parent_does_not_authorise_resolution_changes",
        || {
            let r = Repo::new();
            r.install_binary();
            imported_merge(&r, true);
            assert_scope_refusal(&r, 0);
        },
    );
}

#[test]
fn push_url_advertisement_is_authoritative_when_fetch_url_is_ahead() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "push_url_advertisement_is_authoritative_when_fetch_url_is_ahead",
        || {
            let r = Repo::new();
            r.install_binary();
            let destination = r.root.join("destination.git");
            r.git(
                &r.root,
                &[
                    "clone",
                    "-q",
                    "--bare",
                    r.remote.to_str().unwrap(),
                    destination.to_str().unwrap(),
                ],
            );
            let bad = r.commit_file(&r.work, "unrelated.txt", "present at fetch remote only\n");
            r.git(&r.work, &["push", "origin", "HEAD"]);
            r.git(
                &r.work,
                &[
                    "config",
                    "remote.origin.pushurl",
                    destination.to_str().unwrap(),
                ],
            );
            // Local tracking ref truthfully says this is pushed to the FETCH remote.
            // The actual destination has never advertised it.
            assert_eq!(r.oid(&r.work, "refs/remotes/origin/feat/1554"), bad);
            let before = r.oid(&destination, "refs/heads/feat/1554");
            let out = r.step_command(PHASES[0].0, PHASES[0].1).output().unwrap();
            assert_eq!(
                out.status.code(),
                Some(78),
                "fetch remote was mistaken for push destination: {}",
                describe(&out)
            );
            assert_eq!(r.oid(&destination, "refs/heads/feat/1554"), before);
        },
    );
}

#[test]
fn local_replace_refs_cannot_remove_unauthorised_history_from_validation() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "local_replace_refs_cannot_remove_unauthorised_history_from_validation",
        || {
            let r = Repo::new();
            r.install_binary();
            let bad = r.commit_file(&r.work, "unrelated.txt", "hidden by a local replace ref\n");
            r.git(&r.work, &["replace", &bad, &r.base]);
            assert_scope_refusal(&r, 0);
        },
    );
}

#[test]
fn failed_advertisement_or_partial_fetch_never_permits_publication() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "failed_advertisement_or_partial_fetch_never_permits_publication",
        || {
            for failure in ["advertisement", "fetch", "partial-fetch"] {
                let r = Repo::new();
                r.install_binary();
                r.commit_file(&r.work, "src/new.rs", "authorised outgoing change\n");
                let before = r.oid(&r.remote, "refs/heads/feat/1554");
                let real_git = r.shell("command -v git").output().unwrap();
                success(&real_git);
                let real_git = String::from_utf8(real_git.stdout)
                    .unwrap()
                    .trim()
                    .to_owned();
                let calls = r.root.join("transport-calls");
                executable(
                    r.bin.join("git"),
                    r#"#!/bin/bash
for arg in "$@"; do
  if { [ "$FAILURE" = advertisement ] && [ "$arg" = ls-remote ]; } ||
     { [ "$FAILURE" != advertisement ] && [ "$arg" = fetch ]; }; then
    printf '%s\n' "$arg" >> "$TRANSPORT_CALLS"
    if [ "$FAILURE" = partial-fetch ]; then "$REAL_GIT" update-ref refs/remotes/origin/partial HEAD || exit 96; fi
    echo 'injected incomplete transport result' >&2
    exit 23
  fi
  if [ "$arg" = push ]; then printf 'PUSH\n' >> "$TRANSPORT_CALLS"; fi
done
exec "$REAL_GIT" "$@"
"#,
                );
                let out = r
                    .step_command(PHASES[0].0, PHASES[0].1)
                    .env("REAL_GIT", real_git)
                    .env("FAILURE", failure)
                    .env("TRANSPORT_CALLS", &calls)
                    .output()
                    .unwrap();
                assert!(
                    !out.status.success(),
                    "{failure} reported success: {}",
                    describe(&out)
                );
                assert_eq!(r.oid(&r.remote, "refs/heads/feat/1554"), before);
                let calls = fs::read_to_string(calls)
                    .expect("the production path must obtain destination history");
                assert!(
                    !calls.contains("PUSH"),
                    "publication continued after {failure}: {calls}"
                );
                assert_eq!(
                    calls.lines().count(),
                    1,
                    "transport failures must not trigger retries"
                );
            }
        },
    );
}

#[test]
fn ordinary_push_hooks_run_and_a_rejection_never_reports_success() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "ordinary_push_hooks_run_and_a_rejection_never_reports_success",
        || {
            let r = Repo::new();
            r.install_binary();
            r.commit_file(&r.work, "src/new.rs", "authorised source\n");
            let before = r.oid(&r.remote, "refs/heads/feat/1554");
            r.hook(
                "pre-push",
                "#!/bin/sh\ncat > \"$PUSH_INPUT\"\nprintf 'hook-rejected\\n' >&2\nexit 1\n",
            );
            let input = r.root.join("push-input");
            // Feedback uses a direct push adapter and cannot invoke a fixing model.
            let out = r
                .step_command(PHASES[1].0, PHASES[1].1)
                .env("PUSH_INPUT", &input)
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "pre-push rejection swallowed: {}",
                describe(&out)
            );
            assert!(
                !fs::read(input).unwrap().is_empty(),
                "normal pre-push hook was bypassed"
            );
            assert_eq!(r.oid(&r.remote, "refs/heads/feat/1554"), before);
        },
    );
}

#[test]
fn implicit_tag_following_cannot_add_unvalidated_ref_updates() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "implicit_tag_following_cannot_add_unvalidated_ref_updates",
        || {
            let r = Repo::new();
            r.install_binary();
            r.commit_file(&r.work, "src/new.rs", "authorised outgoing source\n");
            r.git(
                &r.work,
                &[
                    "tag",
                    "-a",
                    "unrequested-tag",
                    "-m",
                    "must not be implicitly published",
                ],
            );
            r.git(&r.work, &["config", "push.followTags", "true"]);
            success(&r.step_command(PHASES[1].0, PHASES[1].1).output().unwrap());
            assert_eq!(
                r.oid(&r.remote, "refs/heads/feat/1554"),
                r.oid(&r.work, "HEAD")
            );
            assert!(
                !r.git_output(
                    &r.remote,
                    &["rev-parse", "--verify", "refs/tags/unrequested-tag"]
                )
                .status
                .success(),
                "push.followTags published an unvalidated tag"
            );
        },
    );
}

#[test]
fn a_later_destination_failure_reports_partial_completion_truthfully() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "a_later_destination_failure_reports_partial_completion_truthfully",
        || {
            let r = Repo::new();
            r.install_binary();
            let tip = r.commit_file(&r.work, "src/new.rs", "authorised outgoing source\n");
            r.git(
                &r.work,
                &[
                    "config",
                    "--add",
                    "remote.origin.pushurl",
                    r.remote.to_str().unwrap(),
                ],
            );
            let missing = r.root.join("missing-destination.git");
            r.git(
                &r.work,
                &[
                    "config",
                    "--add",
                    "remote.origin.pushurl",
                    missing.to_str().unwrap(),
                ],
            );
            let out = r.step_command(PHASES[0].0, PHASES[0].1).output().unwrap();
            assert!(
                !out.status.success(),
                "partial multi-destination push reported success: {}",
                describe(&out)
            );
            let first_destination = r.oid(&r.remote, "refs/heads/feat/1554");
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            // Validating every destination before pushing either is also safe. If a
            // push has already succeeded, however, the result must say so explicitly.
            if first_destination == tip {
                assert!(
                    text.to_lowercase().contains("partial"),
                    "successful first destination was not reported: {text}"
                );
                assert!(
                    text.contains(r.remote.to_str().unwrap()),
                    "name the destination that succeeded: {text}"
                );
            } else {
                assert_eq!(
                    first_destination, r.base,
                    "an unexpected object reached the first destination"
                );
            }
        },
    );
}
