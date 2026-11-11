//! Independent ownership, metadata isolation, and destructive-boundary tests.
use super::{contract_support as boundary, *};

#[test]
fn runtime_replacement_is_retained_and_prevents_success() {
    let f = Fixture::new();
    let output = boundary::command(&f, "runtime-replaced")
        .env("ISSUE_2096_MODE", "runtime-replacement")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "runtime ownership failure was ignored: {output:?}"
    );
    let runtime = boundary::path(&f, "runtime-replaced", "replacement-runtime");
    assert_eq!(
        fs::read(runtime.join("unowned-sentinel")).unwrap(),
        b"keep\n"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(boundary::run_dir(&f).join("run.json")).unwrap()).unwrap();
    assert_eq!(report["outcome"], "failure");
}

#[test]
fn an_independently_launched_nested_cli_does_not_inherit_the_outer_lease() {
    let f = Fixture::new();
    let before = f.snapshot();
    succeeds(
        &boundary::command(&f, "outer")
            .env("ISSUE_2096_MODE", "nested")
            .env(
                "ISSUE_2096_AMPLIHACK",
                crate::support::amplihack_bin::amplihack_bin(),
            )
            .env("ISSUE_2096_RECIPE", f.root.join("recipe.yaml"))
            .output()
            .unwrap(),
    );
    assert_eq!(f.observed("outer", "outer-still-active"), b"yes");
    assert_ne!(
        f.observed("outer", "cwd"),
        f.observed("outer/nested", "cwd"),
        "process nesting does not establish logical-run ownership"
    );
    assert_ne!(
        f.observed("outer", "index"),
        f.observed("outer/nested", "index")
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn execution_uses_private_repository_metadata_and_object_storage() {
    let f = Fixture::new();
    let run = boundary::Running::start(&f, "metadata");
    let common = boundary::path(&f, "metadata", "common");
    let objects = boundary::path(&f, "metadata", "objects");
    assert_ne!(
        common,
        f.repo.join(".git"),
        "linked metadata is shared writable state"
    );
    assert!(
        !objects.join("info/alternates").exists(),
        "caller object dependencies are forbidden"
    );
    let object = f.git(&["rev-parse", "HEAD"]);
    let object = std::str::from_utf8(&object).unwrap().trim();
    let relative = format!("{}/{}", &object[..2], &object[2..]);
    // For loose objects, identity demonstrates that no caller hardlink was
    // used. Packed transports are allowed and do not need a loose object.
    let source = f.repo.join(".git/objects").join(&relative);
    let copy = objects.join(relative);
    if copy.exists() {
        use std::os::unix::fs::MetadataExt;
        let (source, copy) = (fs::metadata(source).unwrap(), fs::metadata(copy).unwrap());
        assert_ne!((source.dev(), source.ino()), (copy.dev(), copy.ino()));
    }
    succeeds(&run.finish());
}

#[test]
fn finishing_one_live_run_cannot_remove_another_runs_checkout_or_index() {
    let f = Fixture::new();
    let one = boundary::Running::start(&f, "owner-one");
    let two = boundary::Running::start(&f, "owner-two");
    let second = boundary::path(&f, "owner-two", "cwd");
    let index = boundary::path(&f, "owner-two", "index");
    let before = fs::read(&index).unwrap();
    succeeds(&one.finish());
    assert!(second.is_dir(), "another active lease was removed");
    assert_eq!(fs::read(index).unwrap(), before);
    succeeds(&two.finish());
    assert!(!second.exists());
}

#[test]
fn replaced_checkout_directory_is_preserved_and_release_failure_fails_the_run() {
    let f = Fixture::new();
    let run = boundary::Running::start(&f, "replacement");
    let checkout = boundary::path(&f, "replacement", "cwd");
    let parked = f.root.join("original-checkout");
    fs::rename(&checkout, &parked).unwrap();
    fs::create_dir(&checkout).unwrap();
    fs::write(checkout.join("unowned-sentinel"), b"keep\n").unwrap();
    let output = run.finish();
    assert!(
        !output.status.success(),
        "release identity mismatch cannot report success: {output:?}"
    );
    assert_eq!(
        fs::read(checkout.join("unowned-sentinel")).unwrap(),
        b"keep\n",
        "a replacement directory has no deletion authority"
    );
    assert!(
        parked.join("tracked").exists(),
        "preserve the original as recovery evidence"
    );
}

#[test]
fn a_durable_worktree_keeps_its_repository_after_execution_checkout_release() {
    let f = Fixture::new();
    let run = boundary::Running::start(&f, "durable");
    let execution = boundary::path(&f, "durable", "cwd");
    let durable = f.root.join("durable-task");
    f.git_in(
        &execution,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "durable-task",
            durable.to_str().unwrap(),
        ],
    );
    // Exercise the existing ownership registration used by workflow-created
    // worktrees, rather than inventing a new cleanup API for the test.
    // The current registration boundary is a bundled helper. Keep the
    // test at that boundary until its Rust compatibility adapter replaces it.
    let mut register = Command::new("bash");
    f.environment(&mut register);
    register
        .env("AMPLIHACK_RUN_DIR", boundary::run_dir(&f))
        .current_dir(&execution)
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../amplifier-bundle/tools/amplihack_run_owned.sh"),
        )
        .args(["record", "worktree"])
        .arg(&durable)
        .arg("durable-task");
    succeeds(&register.output().unwrap());
    succeeds(&run.finish());
    assert!(!execution.exists());
    assert_eq!(
        f.git_in(&durable, &["rev-parse", "HEAD"]),
        f.git(&["rev-parse", "HEAD"])
    );
    assert_ne!(
        f.git_in(
            &durable,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"]
        ),
        f.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
    );
}
