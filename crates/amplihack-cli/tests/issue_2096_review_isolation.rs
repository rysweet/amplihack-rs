//! Preserve the existing guard's behavior on private detached checkouts.
//! Provisioning ownership and trusted agent launch need separate implementation tests.
use super::*;

fn guard(f: &Fixture, args: &[&str]) -> Output {
    let mut c = Command::new("bash");
    f.environment(&mut c);
    c.arg(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../amplifier-bundle/tools/autodrive_worktree_guard.sh"),
    )
    .args(args)
    .current_dir(&f.repo)
    .output()
    .unwrap()
}

#[test]
fn caller_staging_does_not_trip_private_review_but_private_staging_does() {
    let f = Fixture::new();
    let review = f.root.join("review");
    f.git(&[
        "worktree",
        "add",
        "--detach",
        review.to_str().unwrap(),
        "HEAD",
    ]);
    let stamped = guard(&f, &["index-stamp", review.to_str().unwrap()]);
    succeeds(&stamped);
    let stamp = String::from_utf8(stamped.stdout).unwrap();
    fs::write(f.repo.join("caller-only"), "unrelated").unwrap();
    f.git(&["add", "caller-only"]);
    succeeds(&guard(
        &f,
        &[
            "index-check",
            review.to_str().unwrap(),
            stamp.trim(),
            "judging",
        ],
    ));
    fs::write(review.join("private-only"), "mutation").unwrap();
    let mut c = Command::new(&f.git);
    f.environment(&mut c);
    assert!(
        c.current_dir(&review)
            .args(["add", "private-only"])
            .status()
            .unwrap()
            .success()
    );
    let changed = guard(
        &f,
        &[
            "index-check",
            review.to_str().unwrap(),
            stamp.trim(),
            "judging",
        ],
    );
    assert!(
        !changed.status.success(),
        "private review mutations must still stop review"
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&changed.stdout),
        String::from_utf8_lossy(&changed.stderr)
    );
    assert!(text.contains("REVIEW_DIR_CHANGED"), "{text}");
}
