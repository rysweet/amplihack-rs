//! Issue #2096: an auto-drive's review dir is a checkout of its own.
//!
//! Compatibility-boundary contracts for exact PR-head provisioning and
//! the #1528 guards. These do not certify actual agent instruction discovery.
use super::*;

#[path = "issue_2096_instruction_trust.rs"]
mod instruction_trust;

fn guard(f: &Fixture, args: &[&str]) -> Output {
    let mut c = Command::new("bash");
    f.environment(&mut c);
    let binary = crate::support::amplihack_bin::amplihack_bin();
    c.env(
        "PATH",
        std::env::join_paths(
            std::iter::once(binary.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap(),
    );
    c.arg(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../amplifier-bundle/tools/autodrive_worktree_guard.sh"),
    )
    .args(args)
    .current_dir(&f.repo)
    .output()
    .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A PR worktree on a branch, as auto-drive's preflight answers it.
fn pr_worktree(f: &Fixture) -> PathBuf {
    let pr = f.root.join("pr");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature",
        pr.to_str().unwrap(),
    ]);
    pr
}

fn private_review(f: &Fixture, pr: &Path) -> PathBuf {
    let made = guard(
        f,
        &[
            "private-review-dir",
            pr.to_str().unwrap(),
            f.root.join("review").to_str().unwrap(),
        ],
    );
    succeeds(&made);
    PathBuf::from(String::from_utf8(made.stdout).unwrap().trim())
}

#[test]
fn independently_allocated_reviews_in_one_state_directory_release_only_their_own_resources() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = f.root.join("shared-review-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let mut checkouts = Vec::new();
    for _ in 0..2 {
        let mut command = Command::new(crate::support::amplihack_bin::amplihack_bin());
        f.environment(&mut command);
        let output = command
            .args(["internal", "review-allocate"])
            .arg(&pr)
            .arg(&state)
            .output()
            .unwrap();
        succeeds(&output);
        checkouts.push(PathBuf::from(
            String::from_utf8(output.stdout).unwrap().trim(),
        ));
    }
    assert_ne!(checkouts[0], checkouts[1]);
    succeeds(&guard(
        &f,
        &["release-review-dir", checkouts[0].to_str().unwrap()],
    ));
    assert!(!checkouts[0].exists());
    assert!(checkouts[1].is_dir());
    let mut command = Command::new(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    succeeds(
        &command
            .args(["internal", "review-validate"])
            .arg(&pr)
            .arg(&checkouts[1])
            .output()
            .unwrap(),
    );
    succeeds(&guard(
        &f,
        &["release-review-dir", checkouts[1].to_str().unwrap()],
    ));
}

#[test]
fn legacy_main_review_input_becomes_an_owned_pr_generation_and_releases_from_state() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    fs::write(pr.join("pr-tip"), b"review this tip\n").unwrap();
    f.git_in(&pr, &["add", "pr-tip"]);
    f.git_in(&pr, &["commit", "-qm", "PR tip"]);
    let state = f.root.join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let before = f.snapshot();
    let mut command = Command::new(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    let made = command
        .args(["internal", "review-next"])
        .arg(&pr)
        .arg(&f.repo)
        .arg(&state)
        .output()
        .unwrap();
    succeeds(&made);
    let checkout = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    assert_ne!(checkout, f.repo);
    assert_eq!(
        f.git_in(&checkout, &["rev-parse", "HEAD"]),
        f.git_in(&pr, &["rev-parse", "HEAD"])
    );
    let mut command = Command::new(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    succeeds(
        &command
            .args(["internal", "review-release-state"])
            .arg(state)
            .output()
            .unwrap(),
    );
    assert!(!checkout.exists());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn round_publication_binds_the_recorded_commit_and_generation() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    let record = f.root.join("round.json");
    fs::write(
        &record,
        b"{\"blocker_count\":2,\"merge_ready_verdict\":\"NOT_MERGE_READY\"}",
    )
    .unwrap();
    let mut command = Command::new(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    let output = command
        .args(["internal", "review-bind-record"])
        .arg(&review)
        .arg(&record)
        .output()
        .unwrap();
    succeeds(&output);
    let record = fs::read(record).unwrap();
    assert!(
        record.starts_with(b"{\"merge_ready_verdict\":"),
        "preserve the trust reader's first-key contract"
    );
    let value: serde_json::Value = serde_json::from_slice(&record).unwrap();
    assert_eq!(
        value["reviewed_head_sha"],
        String::from_utf8(f.git_in(&pr, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
    );
    assert!(
        value["review_generation"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(value["merge_ready_verdict"], "NOT_MERGE_READY");
}

#[test]
fn conflicting_review_commit_is_not_rewritten_during_publication() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    let record = f.root.join("round.json");
    let bytes = b"{\"reviewed_head_sha\":\"0000000000000000000000000000000000000000\"}";
    fs::write(&record, bytes).unwrap();
    let mut command = Command::new(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    let output = command
        .args(["internal", "review-bind-record"])
        .arg(&review)
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "mismatched review attempt was accepted: {output:?}"
    );
    assert_eq!(fs::read(record).unwrap(), bytes);
}

#[test]
fn private_review_dir_is_a_detached_checkout_at_the_recorded_pr_head() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    fs::write(pr.join("pr-only"), b"review this revision\n").unwrap();
    f.git_in(&pr, &["add", "pr-only"]);
    f.git_in(&pr, &["commit", "-qm", "PR changes"]);
    let recorded = f.git_in(&pr, &["rev-parse", "HEAD"]);
    assert_ne!(
        recorded,
        f.git(&["rev-parse", "HEAD"]),
        "fixture must distinguish PR from main"
    );
    let review = private_review(&f, &pr);
    assert_ne!(review, f.repo);
    assert_eq!(
        f.git_in(&review, &["rev-parse", "HEAD"]),
        recorded,
        "main HEAD is not a substitute for the recorded PR head"
    );
    let mut c = amplihack_git::command();
    f.environment(&mut c);
    let branch = c
        .current_dir(&review)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .unwrap();
    assert!(!branch.status.success(), "the review checkout is detached");
}

#[test]
fn pr_advance_creates_a_new_review_generation_without_rewriting_the_old_one() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let first = private_review(&f, &pr);
    let first_head = f.git_in(&first, &["rev-parse", "HEAD"]);
    fs::write(pr.join("advance"), b"new PR tip\n").unwrap();
    f.git_in(&pr, &["add", "advance"]);
    f.git_in(&pr, &["commit", "-qm", "advance"]);
    let made = guard(
        &f,
        &[
            "private-review-dir",
            pr.to_str().unwrap(),
            f.root.join("review-next").to_str().unwrap(),
        ],
    );
    succeeds(&made);
    let second = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    assert_ne!(first, second);
    assert_eq!(f.git_in(&first, &["rev-parse", "HEAD"]), first_head);
    assert_eq!(
        f.git_in(&second, &["rev-parse", "HEAD"]),
        f.git_in(&pr, &["rev-parse", "HEAD"])
    );
}

#[test]
fn an_explicit_recorded_sha_is_not_replaced_by_a_later_pr_tip_or_main() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    fs::write(pr.join("recorded"), b"judge this version\n").unwrap();
    f.git_in(&pr, &["add", "recorded"]);
    f.git_in(&pr, &["commit", "-qm", "recorded attempt"]);
    let recorded = f.git_in(&pr, &["rev-parse", "HEAD"]);
    fs::write(pr.join("later"), b"not this later version\n").unwrap();
    f.git_in(&pr, &["add", "later"]);
    f.git_in(&pr, &["commit", "-qm", "PR advanced after recording"]);
    // Thin compatibility entry point: the final argument binds preparation
    // to the attempt's recorded SHA. Provisioning itself belongs in Rust.
    let made = guard(
        &f,
        &[
            "private-review-dir",
            pr.to_str().unwrap(),
            f.root.join("recorded-review").to_str().unwrap(),
            std::str::from_utf8(&recorded).unwrap().trim(),
        ],
    );
    succeeds(&made);
    let review = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    assert_eq!(f.git_in(&review, &["rev-parse", "HEAD"]), recorded);
    assert!(!review.join("later").exists());
}

#[test]
fn release_refuses_an_unowned_detached_worktree_even_in_the_same_repository() {
    let f = Fixture::new();
    let unowned = f.root.join("someone-elses-checkout");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        unowned.to_str().unwrap(),
        "HEAD",
    ]);
    fs::write(unowned.join("keep"), b"another owner\n").unwrap();
    let output = guard(&f, &["release-review-dir", unowned.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "detached state is not cleanup authorization: {output:?}"
    );
    assert_eq!(fs::read(unowned.join("keep")).unwrap(), b"another owner\n");
}

#[test]
fn failed_review_inspection_does_not_claim_a_mutation_was_compared() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    let stamped = guard(&f, &["index-stamp", review.to_str().unwrap()]);
    succeeds(&stamped);
    let index = f.git_in(
        &review,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    let index = PathBuf::from(String::from_utf8(index).unwrap().trim());
    fs::write(index, b"invalid index\n").unwrap();
    let output = guard(
        &f,
        &[
            "index-check",
            review.to_str().unwrap(),
            String::from_utf8(stamped.stdout).unwrap().trim(),
            "judging",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "failed inspection is distinct from confirmed mutation: {output:?}"
    );
    assert!(!text(&output).contains("changed the index or HEAD"));
}

#[test]
fn caller_staging_does_not_trip_the_private_review_dir_but_staging_there_does() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    let review_arg = review.to_str().unwrap();
    let stamped = guard(&f, &["index-stamp", review_arg]);
    succeeds(&stamped);
    let stamp = String::from_utf8(stamped.stdout).unwrap();

    fs::write(f.repo.join("caller-only"), "unrelated").unwrap();
    f.git(&["add", "caller-only"]);
    succeeds(&guard(
        &f,
        &["index-check", review_arg, stamp.trim(), "judging"],
    ));

    fs::write(review.join("private-only"), "mutation").unwrap();
    f.git_in(&review, &["add", "private-only"]);
    let changed = guard(&f, &["index-check", review_arg, stamp.trim(), "judging"]);
    assert!(
        !changed.status.success(),
        "a change in the review checkout must still stop the drive"
    );
    assert!(
        text(&changed).contains("changed the index or HEAD"),
        "{}",
        text(&changed)
    );
}

#[test]
fn loop_preflight_accepts_the_private_review_dir() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    let out = guard(
        &f,
        &[
            "loop-preflight",
            "crusty",
            pr.to_str().unwrap(),
            review.to_str().unwrap(),
            "autodrive-crusty-round",
        ],
    );
    succeeds(&out);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().lines().next(),
        Some(review.to_str().unwrap())
    );
}

#[test]
fn loop_preflight_refuses_a_detached_worktree_at_a_commit_main_does_not_have() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    fs::write(review.join("untrusted"), "x").unwrap();
    f.git_in(&review, &["add", "untrusted"]);
    f.git_in(
        &review,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "untrusted",
        ],
    );
    let out = guard(
        &f,
        &[
            "loop-preflight",
            "crusty",
            pr.to_str().unwrap(),
            review.to_str().unwrap(),
            "autodrive-crusty-round",
        ],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("UNTRUSTED_REVIEW_DIR"),
        "{}",
        text(&out)
    );
}

#[test]
fn release_review_dir_removes_the_checkout_and_refuses_the_main_checkout() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    succeeds(&guard(
        &f,
        &["release-review-dir", review.to_str().unwrap()],
    ));
    assert!(!review.exists());
    let listed = String::from_utf8(f.git(&["worktree", "list", "--porcelain"])).unwrap();
    assert!(!listed.contains(review.to_str().unwrap()), "{listed}");

    let refused = guard(&f, &["release-review-dir", f.repo.to_str().unwrap()]);
    assert!(
        !refused.status.success(),
        "the main checkout is never removed"
    );
    assert!(f.repo.join(".git").exists());
    let refused = guard(&f, &["release-review-dir", pr.to_str().unwrap()]);
    assert!(
        !refused.status.success(),
        "a branch's worktree is never removed"
    );
    assert!(pr.exists());
}

fn preflight(f: &Fixture, pr: &Path, review: &Path) -> Output {
    guard(
        f,
        &[
            "loop-preflight",
            "crusty",
            pr.to_str().unwrap(),
            review.to_str().unwrap(),
            "autodrive-crusty-round",
        ],
    )
}

#[test]
fn loop_preflight_refuses_a_review_dir_in_another_repository() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let other = f.root.join("other-repo");
    fs::create_dir(&other).unwrap();
    f.git_in(&other, &["init", "-q"]);
    f.git_in(
        &other,
        &[
            "-c",
            "user.name=o",
            "-c",
            "user.email=o@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "other",
        ],
    );
    let out = preflight(&f, &pr, &other);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("another repository"), "{}", text(&out));
    assert!(
        other.join(".git").exists(),
        "the refused directory is left alone"
    );
}

#[test]
fn loop_preflight_refuses_a_review_dir_that_is_not_a_checkout() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let plain = f.root.join("plain-dir");
    fs::create_dir(&plain).unwrap();
    fs::write(plain.join("keep"), "x").unwrap();
    let out = preflight(&f, &pr, &plain);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        plain.join("keep").exists(),
        "the refused directory is left alone"
    );
}

#[test]
fn loop_preflight_refuses_a_branch_worktree_as_review_dir() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let out = preflight(&f, &pr, &pr);
    assert!(
        !out.status.success(),
        "the PR's own branch worktree is not a review dir"
    );
    assert!(pr.exists());
}
