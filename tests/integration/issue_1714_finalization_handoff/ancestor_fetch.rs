//! The one bounded fetch behind the fast-forward answer (#1714, case 2).
//!
//! When the pull request head is not in the local object store, the shared
//! `_finalization_pr_head_descends` fetches it once, writing no ref and no
//! `FETCH_HEAD`. A fetch that fails is not an error: the ancestry is then
//! unread, and the facts say which commit could not be obtained. The cases
//! where the commit is present are in `ancestor.rs`.

use super::ancestor::{
    assert_descendant_signal, assert_no_signal, descends, drop_pushed_branch, has_object,
    merge_main_on_pushed, origin_remote, prune, view_at,
};
use super::fixture::*;

fn assert_no_fetch_side_effects(fx: &Fixture) {
    assert_eq!(
        fx.git(&["for-each-ref", "refs/remotes"]).trim(),
        "",
        "the fetch must not write a remote-tracking ref"
    );
    assert!(
        !fx.repo.join(".git/FETCH_HEAD").exists(),
        "the fetch must not write FETCH_HEAD"
    );
}

// M is not in the local object store: one fetch brings it in, and nothing
// else in the repository changes.
#[test]
fn a_pr_head_missing_locally_is_fetched_without_writing_refs() {
    let fx = Fixture::new();
    let (local, pr_head) = merge_main_on_pushed(&fx);
    origin_remote(&fx);
    fx.git(&[
        "push",
        "-q",
        "origin",
        &format!("pushed:refs/heads/{BRANCH}"),
    ]);
    drop_pushed_branch(&fx);
    for r in fx
        .git(&["for-each-ref", "--format=%(refname)", "refs/remotes"])
        .lines()
    {
        fx.git(&["update-ref", "-d", r]);
    }
    prune(&fx, &pr_head);
    view_at(&fx, &pr_head, green_rollup());

    assert_descendant_signal(&fx.signal(BRANCH, Some("7")), "green", "after fetch");
    assert!(has_object(&fx, &pr_head), "the fetch brought M in");
    assert_eq!(fx.head(), local);
    assert_no_fetch_side_effects(&fx);

    // The outcome reader makes the same one bounded fetch and reports the
    // answer as a fact, leaving the repository exactly as it found it.
    prune(&fx, &pr_head);
    assert_eq!(
        descends(&fx),
        "true",
        "the reader fetched M and merge-base answered yes"
    );
    assert!(has_object(&fx, &pr_head), "the reader's fetch brought M in");
    assert_eq!(fx.head(), local);
    assert_no_fetch_side_effects(&fx);
}

// A missing object is never read as "ancestor": with no remote, or a remote
// whose branch is elsewhere, the check fails closed.
#[test]
fn an_unfetchable_pr_head_fails_closed() {
    let fx = Fixture::new();
    let (_, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    prune(&fx, &pr_head);
    view_at(&fx, &pr_head, green_rollup());
    assert_no_signal(&fx.signal(BRANCH, Some("7")), "no remote");
    // A commit that cannot be obtained is reported unread, never as "false":
    // a missing object must never read as "the local head is not an ancestor".
    let facts = fx.facts(BRANCH, Some("7")).0;
    assert_eq!(
        facts["pr_head_descends_local_head"], "",
        "an unfetchable PR head leaves the answer unknown: {facts}"
    );
    assert!(
        facts["unread"]
            .as_str()
            .expect("unread is a String")
            .contains(&pr_head),
        "the unread fact names the commit git could not obtain: {facts}"
    );

    origin_remote(&fx);
    fx.git(&[
        "push",
        "-q",
        "origin",
        &format!("{BRANCH}:refs/heads/{BRANCH}"),
    ]);
    assert_no_signal(&fx.signal(BRANCH, Some("7")), "remote branch elsewhere");
    assert!(!has_object(&fx, &pr_head));
}
