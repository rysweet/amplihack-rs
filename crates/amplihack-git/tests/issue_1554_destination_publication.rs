//! Real destination histories; these are new diagnostics, not the lost nine-case packet.
#[path = "support/issue_1554.rs"]
mod fixture;

use amplihack_git::publication_validation;
use fixture::{Repo, git, isolated, source_authority, text, write};

#[test]
fn relative_push_destinations_are_resolved_from_the_source_checkout() {
    isolated(
        "relative_push_destinations_are_resolved_from_the_source_checkout",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            git(
                &repo.root,
                &["remote", "set-url", "origin", "../destination.git"],
            );
            let tip = repo.commit("source.txt", "authorised source\n");
            publication_validation::publish(&repo.root)
                .expect("a relative local remote is a valid push destination");
            assert_eq!(
                text(&repo.remote, &["rev-parse", "refs/heads/feat/task"]),
                tip
            );
        },
    );
}

#[test]
fn an_advertised_tag_is_a_valid_import_basis_for_a_new_task_ref() {
    isolated(
        "an_advertised_tag_is_a_valid_import_basis_for_a_new_task_ref",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            // Retain base history only through an annotated remote tag.
            git(
                &repo.root,
                &["tag", "--annotate", "-m", "remote base", "base-import"],
            );
            git(
                &repo.root,
                &["push", "--quiet", "origin", "refs/tags/base-import"],
            );
            git(&repo.remote, &["update-ref", "-d", "refs/heads/main"]);
            git(&repo.root, &["checkout", "--quiet", "-b", "feat/task-1554"]);
            let tip = repo.commit("source.txt", "task source\n");
            publication_validation::publish(&repo.root).unwrap();
            assert_eq!(
                text(&repo.remote, &["rev-parse", "refs/heads/feat/task-1554"]),
                tip
            );
        },
    );
}

#[test]
fn a_policy_failure_at_any_destination_prevents_all_pushes() {
    isolated(
        "a_policy_failure_at_any_destination_prevents_all_pushes",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            let older = text(&repo.remote, &["rev-parse", "refs/heads/main"]);
            let second = repo.temp.path().join("second.git");
            git(
                &repo.root,
                &[
                    "clone",
                    "--bare",
                    "--quiet",
                    repo.remote.to_str().unwrap(),
                    second.to_str().unwrap(),
                ],
            );
            // Only destination one advertises the unauthorised local commit.
            repo.commit("protected.txt", "advertised at first destination only\n");
            git(&repo.root, &["push", "--quiet", "origin", "HEAD:main"]);
            let first_before = text(&repo.remote, &["rev-parse", "refs/heads/main"]);
            repo.commit("source.txt", "authorised task source\n");
            for destination in [&repo.remote, &second] {
                git(
                    &repo.root,
                    &[
                        "config",
                        "--add",
                        "remote.origin.pushurl",
                        destination.to_str().unwrap(),
                    ],
                );
            }
            // These forgeries must not turn the second destination into the first.
            git(
                &repo.root,
                &["update-ref", "refs/remotes/origin/main", "HEAD"],
            );
            write(
                repo.root.join(".git/FETCH_HEAD"),
                format!(
                    "{}\t\tforged basis\n",
                    text(&repo.root, &["rev-parse", "HEAD"])
                ),
            );
            let error = publication_validation::publish(&repo.root).unwrap_err();
            assert!(error.to_string().contains("protected.txt"), "{error:#}");
            assert_eq!(
                text(&repo.remote, &["rev-parse", "refs/heads/main"]),
                first_before
            );
            assert_eq!(text(&second, &["rev-parse", "refs/heads/main"]), older);
        },
    );
}

#[test]
fn chained_push_rewrites_cannot_subtract_history_from_the_wrong_destination() {
    isolated(
        "chained_push_rewrites_cannot_subtract_history_from_the_wrong_destination",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            let advertised = repo.temp.path().join("advertised.git");
            let actual = repo.temp.path().join("actual.git");
            for remote in [&advertised, &actual] {
                git(
                    &repo.root,
                    &[
                        "clone",
                        "--bare",
                        "--quiet",
                        repo.remote.to_str().unwrap(),
                        remote.to_str().unwrap(),
                    ],
                );
            }
            let base = text(&actual, &["rev-parse", "refs/heads/main"]);
            repo.commit(
                "protected.txt",
                "only advertised destination owns this history\n",
            );
            git(
                &repo.root,
                &["push", "--quiet", advertised.to_str().unwrap(), "HEAD:main"],
            );
            let advertised_tip = text(&advertised, &["rev-parse", "refs/heads/main"]);
            repo.commit("source.txt", "authorised source\n");
            git(
                &repo.root,
                &[
                    "config",
                    &format!("url.{}.pushInsteadOf", advertised.display()),
                    repo.remote.to_str().unwrap(),
                ],
            );
            git(
                &repo.root,
                &[
                    "config",
                    &format!("url.{}.pushInsteadOf", actual.display()),
                    advertised.to_str().unwrap(),
                ],
            );
            let error = publication_validation::publish(&repo.root).unwrap_err();
            assert!(error.to_string().contains("rewritten"), "{error:#}");
            assert_eq!(
                text(&advertised, &["rev-parse", "refs/heads/main"]),
                advertised_tip
            );
            assert_eq!(text(&actual, &["rev-parse", "refs/heads/main"]), base);
        },
    );
}

#[test]
fn a_configured_refspec_with_a_newline_cannot_become_two_updates() {
    isolated(
        "a_configured_refspec_with_a_newline_cannot_become_two_updates",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            let before = text(&repo.remote, &["rev-parse", "refs/heads/main"]);
            repo.commit("source.txt", "authorised source\n");
            git(
                &repo.root,
                &[
                    "config",
                    "remote.origin.push",
                    "refs/heads/main:refs/heads/main\nrefs/heads/main:refs/heads/extra",
                ],
            );
            assert!(publication_validation::publish(&repo.root).is_err());
            assert_eq!(
                text(&repo.remote, &["rev-parse", "refs/heads/main"]),
                before
            );
            assert!(
                !amplihack_git::command_in(&repo.remote)
                    .args(["show-ref", "--verify", "refs/heads/extra"])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        },
    );
}

#[test]
fn empty_push_configuration_cannot_fall_back_to_unvalidated_default_staging() {
    isolated(
        "empty_push_configuration_cannot_fall_back_to_unvalidated_default_staging",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            let before = text(&repo.remote, &["rev-parse", "refs/heads/main"]);
            repo.commit("protected.txt", "unauthorised outgoing source\n");
            git(&repo.root, &["config", "remote.origin.push", ""]);
            assert!(publication_validation::publish(&repo.root).is_err());
            assert_eq!(
                text(&repo.remote, &["rev-parse", "refs/heads/main"]),
                before
            );
        },
    );
}

#[test]
fn explicit_default_branch_destination_is_refused_without_changing_tracking() {
    isolated(
        "explicit_default_branch_destination_is_refused_without_changing_tracking",
        || {
            let repo = Repo::new();
            source_authority(&["source.txt"]);
            let before = text(&repo.remote, &["rev-parse", "refs/heads/main"]);
            git(&repo.root, &["config", "branch.feat/task.remote", "origin"]);
            git(
                &repo.root,
                &["config", "branch.feat/task.merge", "refs/heads/main"],
            );
            repo.commit("source.txt", "task source\n");
            for destination in ["main", "master", "release"] {
                git(
                    &repo.root,
                    &[
                        "config",
                        "remote.origin.push",
                        &format!("refs/heads/feat/task:refs/heads/{destination}"),
                    ],
                );
                if destination == "release" {
                    git(&repo.remote, &["update-ref", "refs/heads/release", &before]);
                    git(
                        &repo.remote,
                        &["symbolic-ref", "HEAD", "refs/heads/release"],
                    );
                }
                assert!(publication_validation::publish(&repo.root).is_err());
                assert_eq!(
                    text(&repo.remote, &["rev-parse", "refs/heads/main"]),
                    before
                );
                assert_eq!(
                    text(&repo.root, &["config", "branch.feat/task.merge"]),
                    "refs/heads/main"
                );
            }
        },
    );
}
