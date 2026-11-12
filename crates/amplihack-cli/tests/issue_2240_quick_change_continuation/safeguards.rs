use std::fs;

use serde_json::json;

use super::fixture::{BRANCH, Fixture, SUBJECT, stderr, success};
use super::publication::{commit, prepare};

#[test]
fn changed_git_configuration_stops_push_before_hooks_or_remote_mutation() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    let branch = f.git(&f.selected(), &["branch", "--show-current"]);
    let remote = f.git(&f.origin, &["rev-parse", &branch]);
    fs::write(f.home.join("hook-calls"), "").unwrap();
    f.git(&f.selected(), &["config", "push.default", "matching"]);
    let out = f.qc("push");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("changed"), "{}", stderr(&out));
    assert!(!f.read_home("hook-calls").contains("pre-push"));
    assert_eq!(f.git(&f.origin, &["rev-parse", &branch]), remote);
}

#[test]
fn continuation_rechecks_exact_branch_before_commit_and_before_push() {
    for operation in ["commit", "push"] {
        let mut f = Fixture::new(BRANCH);
        f.branch(true);
        prepare(&mut f, SUBJECT);
        if operation == "push" {
            commit(&mut f);
        }
        f.git(&f.selected(), &["checkout", "-q", "-b", "quick/substitute"]);
        let head = f.git(&f.selected(), &["rev-parse", "HEAD"]);
        let remote = f.git(&f.origin, &["show-ref"]);
        fs::write(f.home.join("hook-calls"), "").unwrap();
        let out = f.qc(operation);
        assert!(!out.status.success(), "{operation} accepted another branch");
        assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
        assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), head);
        assert_eq!(f.git(&f.origin, &["show-ref"]), remote);
        assert!(f.read_home("hook-calls").is_empty());
    }
}

#[test]
fn matching_but_nonexistent_pinned_sha_is_not_accepted_as_a_base() {
    let mut f = Fixture::new("");
    success(&f.select());
    success(&f.qc("record-base"));
    let missing = "0".repeat(40);
    fs::write(f.state().join("base"), &missing).unwrap();
    f.ctx.set("quick_change_base", json!(missing));
    let out = f.common("qc_base");
    assert!(
        !out.status.success(),
        "unknown SHA accepted as immutable base"
    );
}
