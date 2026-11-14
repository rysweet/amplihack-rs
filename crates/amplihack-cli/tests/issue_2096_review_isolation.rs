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
        let mut command =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
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
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
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
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
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
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
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
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
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
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
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

/// `amplihack internal review-validate <pr> <review>`.
fn review_validate(f: &Fixture, pr: &Path, review: &Path) -> Output {
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    command
        .args(["internal", "review-validate"])
        .arg(pr)
        .arg(review)
        .output()
        .unwrap()
}

/// Security finding 1 (S-AZ2): a verdict names the recorded commit, so the
/// review checkout must still hold that commit's files. An unstaged edit to
/// a tracked file and a new untracked file are each refused; a clean
/// checkout passes, and so does one with only ignored files (build output).
#[test]
fn review_validate_refuses_a_checkout_whose_files_differ_from_its_commit() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    succeeds(&review_validate(&f, &pr, &review));

    let exclude = f.git_in(
        &review,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    );
    let exclude = PathBuf::from(String::from_utf8(exclude).unwrap().trim());
    fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    fs::write(&exclude, "build-output/\n").unwrap();
    fs::create_dir(review.join("build-output")).unwrap();
    fs::write(review.join("build-output/artifact"), "ignored").unwrap();
    succeeds(&review_validate(&f, &pr, &review));

    fs::write(review.join("tracked"), "edited after the commit\n").unwrap();
    let edited = review_validate(&f, &pr, &review);
    assert!(!edited.status.success(), "an unstaged edit must be refused");
    assert!(
        text(&edited).contains("changes that are not in its recorded commit"),
        "{}",
        text(&edited)
    );
    f.git_in(&review, &["checkout", "--", "tracked"]);
    succeeds(&review_validate(&f, &pr, &review));

    fs::write(review.join("new-file"), "untracked\n").unwrap();
    let untracked = review_validate(&f, &pr, &review);
    assert!(
        !untracked.status.success(),
        "an untracked file must be refused"
    );
    assert!(
        text(&untracked).contains("new-file"),
        "{}",
        text(&untracked)
    );
}

/// `index-check` applies the same refusal after a judging step: an edit that
/// is never staged leaves the stamp unchanged, and is still caught.
#[test]
fn index_check_refuses_an_unstaged_edit_in_the_review_checkout() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let review = private_review(&f, &pr);
    let review_arg = review.to_str().unwrap();
    let stamped = guard(&f, &["index-stamp", review_arg]);
    succeeds(&stamped);
    let stamp = String::from_utf8(stamped.stdout).unwrap();
    succeeds(&guard(
        &f,
        &["index-check", review_arg, stamp.trim(), "judging"],
    ));

    fs::write(review.join("tracked"), "edited by the judge\n").unwrap();
    let edited = guard(&f, &["index-check", review_arg, stamp.trim(), "judging"]);
    assert!(
        !edited.status.success(),
        "an unstaged edit must stop the drive"
    );
    assert!(
        text(&edited).contains("left changes in the working tree"),
        "{}",
        text(&edited)
    );
}

// ---------------------------------------------------------------------------
// Crusty finding 9: `review-next` retires the generation it supersedes.
//
// Once the next generation is provisioned and recorded, `advance` removes the
// superseded checkout and its storage and marks its manifest `retired`. The
// manifest (beside the checkout, inside the allocation directory) stays, so
// `review-release` of the root still walks the whole chain. A retired
// generation is never trusted again: validate, launch, head and next refuse
// it. Release removes storage only when it is the same directory, never
// through a symlink or a substitute.
// ---------------------------------------------------------------------------

/// `amplihack internal <args>` in the fixture's private environment.
fn internal(f: &Fixture, args: &[&std::ffi::OsStr]) -> Output {
    let mut command =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut command);
    command.arg("internal").args(args).output().unwrap()
}

fn os(path: &Path) -> &std::ffi::OsStr {
    path.as_os_str()
}

fn private_state(f: &Fixture) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let state = f.root.join("retire-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    state
}

fn allocate(f: &Fixture, pr: &Path, state: &Path) -> PathBuf {
    let out = internal(f, &["review-allocate".as_ref(), os(pr), os(state)]);
    succeeds(&out);
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

fn next(f: &Fixture, pr: &Path, checkout: &Path, state: &Path) -> Output {
    internal(
        f,
        &["review-next".as_ref(), os(pr), os(checkout), os(state)],
    )
}

fn advance_pr(f: &Fixture, pr: &Path, name: &str) {
    fs::write(pr.join(name), format!("{name}\n")).unwrap();
    f.git_in(pr, &["add", name]);
    f.git_in(pr, &["commit", "-qm", name]);
}

fn manifest_of(checkout: &Path) -> PathBuf {
    let mut name = checkout.file_name().unwrap().to_os_string();
    name.push(".review-owner.json");
    checkout.with_file_name(name)
}

fn manifest(checkout: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(manifest_of(checkout)).unwrap()).unwrap()
}

/// The generation's storage directory, from its manifest.
fn storage_of(checkout: &Path) -> PathBuf {
    PathBuf::from(manifest(checkout)["repository"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn advance_retires_superseded_generation() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let storage = storage_of(&first);
    assert!(storage.is_dir());
    assert_ne!(
        manifest(&first)["retired"],
        true,
        "a live generation is not retired"
    );
    advance_pr(&f, &pr, "advance");
    let made = next(&f, &pr, &first, &state);
    succeeds(&made);
    let second = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    assert_ne!(second, first);
    assert_eq!(
        f.git_in(&second, &["rev-parse", "HEAD"]),
        f.git_in(&pr, &["rev-parse", "HEAD"])
    );
    assert!(!first.exists(), "the superseded checkout is removed");
    assert!(!storage.exists(), "the superseded storage is removed");
    let record = manifest(&first);
    assert_eq!(
        record["retired"], true,
        "the superseded record is marked retired"
    );
    assert_eq!(
        record["children"],
        serde_json::json!([second]),
        "the retired record still names its successor"
    );
    assert!(storage_of(&second).is_dir());
    assert_ne!(
        manifest(&second)["retired"],
        true,
        "the new generation is live"
    );
}

#[test]
fn advance_at_the_same_head_retires_nothing() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let made = next(&f, &pr, &first, &state);
    succeeds(&made);
    assert_eq!(
        PathBuf::from(String::from_utf8(made.stdout).unwrap().trim()),
        first
    );
    assert!(first.is_dir() && storage_of(&first).is_dir());
    assert_ne!(manifest(&first)["retired"], true);
}

#[test]
fn retired_generation_is_refused() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    advance_pr(&f, &pr, "advance");
    succeeds(&next(&f, &pr, &first, &state));
    for args in [
        vec!["review-validate".as_ref(), os(&pr), os(&first)],
        vec!["review-launch".as_ref(), os(&first)],
        vec!["review-head".as_ref(), os(&first)],
        vec!["review-clean".as_ref(), os(&first)],
    ] {
        let out = internal(&f, &args);
        assert!(
            !out.status.success(),
            "{args:?} must refuse a retired generation"
        );
        assert!(
            text(&out).contains("superseded and retired"),
            "{args:?}: {}",
            text(&out)
        );
    }
    let again = next(&f, &pr, &first, &state);
    assert!(
        !again.status.success(),
        "review-next must not advance a retired generation"
    );
    assert!(
        text(&again).contains("superseded and retired"),
        "{}",
        text(&again)
    );
}

#[test]
fn release_root_after_retirement_removes_everything() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let allocation = first.parent().unwrap().to_path_buf();
    advance_pr(&f, &pr, "second");
    let made = next(&f, &pr, &first, &state);
    succeeds(&made);
    let second = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    advance_pr(&f, &pr, "third");
    let made = next(&f, &pr, &second, &state);
    succeeds(&made);
    let third = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    assert!(
        !second.exists(),
        "a chain retires every superseded generation"
    );
    assert_eq!(manifest(&second)["retired"], true);
    let third_storage = storage_of(&third);
    let before = f.snapshot();
    succeeds(&internal(&f, &["review-release".as_ref(), os(&first)]));
    for gone in [&first, &second, &third, &third_storage, &allocation] {
        assert!(!gone.exists(), "{} must be released", gone.display());
    }
    for checkout in [&first, &second, &third] {
        assert!(!manifest_of(checkout).exists());
    }
    assert_eq!(f.snapshot(), before, "the caller's checkout is untouched");
}

#[test]
fn release_frees_storage_left_by_failed_retirement() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let storage = storage_of(&first);
    let repository = storage.join("repository.git");
    // A locked worktree refuses a single --force removal, so retirement fails.
    f.git_in(&repository, &["worktree", "lock", first.to_str().unwrap()]);
    advance_pr(&f, &pr, "advance");
    let made = next(&f, &pr, &first, &state);
    succeeds(&made);
    let second = PathBuf::from(String::from_utf8(made.stdout).unwrap().trim());
    assert!(
        second.is_dir(),
        "a failed retirement still returns the new generation"
    );
    assert!(
        String::from_utf8_lossy(&made.stderr)
            .contains("amplihack: warning: could not retire superseded review generation"),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    assert!(
        first.is_dir() && storage.is_dir(),
        "nothing is removed on failure"
    );
    assert_ne!(
        manifest(&first)["retired"],
        true,
        "a failed retirement is not recorded"
    );
    f.git_in(
        &repository,
        &["worktree", "unlock", first.to_str().unwrap()],
    );
    succeeds(&internal(&f, &["review-release".as_ref(), os(&first)]));
    assert!(!first.exists() && !storage.exists() && !second.exists());
    assert!(
        !first.parent().unwrap().exists(),
        "the allocation directory is removed"
    );
}

#[test]
fn retired_release_refuses_symlinked_storage() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let storage = storage_of(&first);
    advance_pr(&f, &pr, "advance");
    succeeds(&next(&f, &pr, &first, &state));
    assert!(!storage.exists());
    let victim = f.root.join("victim");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("canary"), b"keep me\n").unwrap();
    std::os::unix::fs::symlink(&victim, &storage).unwrap();
    let out = internal(&f, &["review-release".as_ref(), os(&first)]);
    assert!(
        !out.status.success(),
        "a symlink at the storage path is not the storage"
    );
    assert!(
        text(&out).contains("review storage identity changed; retained"),
        "{}",
        text(&out)
    );
    assert_eq!(fs::read(victim.join("canary")).unwrap(), b"keep me\n");
    assert!(
        manifest_of(&first).exists(),
        "the record is retained for recovery"
    );
}

#[test]
fn retired_release_refuses_substituted_storage() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let storage = storage_of(&first);
    advance_pr(&f, &pr, "advance");
    succeeds(&next(&f, &pr, &first, &state));
    fs::create_dir(&storage).unwrap();
    fs::write(storage.join("canary"), b"another owner\n").unwrap();
    let out = internal(&f, &["review-release".as_ref(), os(&first)]);
    assert!(
        !out.status.success(),
        "a new directory at the storage path is not the storage"
    );
    assert!(
        text(&out).contains("review storage identity changed; retained"),
        "{}",
        text(&out)
    );
    assert_eq!(
        fs::read(storage.join("canary")).unwrap(),
        b"another owner\n"
    );
}

/// Rewrites a live generation's manifest so its storage is `storage`, with
/// identities that match, as a forger with write access to the manifest
/// could. Only the confinement rule tells it apart.
fn forge_storage(checkout: &Path, storage: &Path) {
    use std::os::unix::fs::MetadataExt;
    let repository = storage.join("repository.git");
    let launch = storage.join("launch");
    fs::create_dir_all(&repository).unwrap();
    fs::create_dir_all(&launch).unwrap();
    let id = |p: &Path| {
        let m = fs::symlink_metadata(p).unwrap();
        serde_json::json!([m.dev(), m.ino()])
    };
    let mut record = manifest(checkout);
    record["repository"] = serde_json::json!(repository);
    record["launch"] = serde_json::json!(launch);
    record["identities"][0] = id(storage);
    record["identities"][1] = id(&repository);
    record["identities"][3] = id(&launch);
    let path = manifest_of(checkout);
    fs::remove_file(&path).unwrap();
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
}

#[test]
fn storage_outside_the_allocation_directory_is_refused() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let elsewhere = f.root.join("elsewhere").join("review-generation-forged");
    forge_storage(&first, &elsewhere);
    let out = internal(&f, &["review-launch".as_ref(), os(&first)]);
    assert!(
        !out.status.success(),
        "storage outside the checkout's directory must be refused: {}",
        text(&out)
    );
    let out = internal(&f, &["review-release".as_ref(), os(&first)]);
    assert!(
        !out.status.success(),
        "release must not trust forged storage"
    );
    assert!(
        elsewhere.join("repository.git").is_dir(),
        "forged storage is not removed"
    );
}

#[test]
fn storage_not_named_as_a_review_generation_is_refused() {
    let f = Fixture::new();
    let pr = pr_worktree(&f);
    let state = private_state(&f);
    let first = allocate(&f, &pr, &state);
    let misnamed = first.parent().unwrap().join("someone-elses-directory");
    forge_storage(&first, &misnamed);
    let out = internal(&f, &["review-launch".as_ref(), os(&first)]);
    assert!(
        !out.status.success(),
        "storage not named review-generation-* must be refused: {}",
        text(&out)
    );
    let out = internal(&f, &["review-release".as_ref(), os(&first)]);
    assert!(!out.status.success());
    assert!(
        misnamed.join("repository.git").is_dir(),
        "misnamed storage is not removed"
    );
}
