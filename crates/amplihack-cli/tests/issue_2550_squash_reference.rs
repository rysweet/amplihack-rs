//! The actual guarded merge argv and stdin, using the production message helper.
#![cfg(unix)]

use crate::issue_2550_reference_preservation::squash;
use crate::support::issue_2550::{Fixture, succeeds};

#[test]
fn existing_guarded_squash_helper_passes_published_refs_on_stdin() {
    let f = Fixture::new();
    f.write("body", "Implemented the feature\nRefs #42\n<!-- merge-ready-report: evidence -->\nVerdict: ready\n");
    f.write("expected-head", f.git(&["rev-parse", "HEAD"]));
    succeeds(&squash(&f));
    assert_eq!(f.read("merge-body"), "Implemented the feature\nRefs #42");
    assert_eq!(f.read("merge-subject"), "fix: select reference (#99)");
}

#[test]
fn substantive_refs_publication_survives_evidence_edits_into_actual_squash_stdin() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    succeeds(&f.commit(Some("refs")));
    succeeds(&f.publication(Some("refs")));
    let baseline = f.read("body");
    assert!(
        baseline.lines().any(|s| s == "Refs #42"),
        "publication failed to apply refs: {baseline}"
    );
    succeeds(&f.edit(&baseline, "Implemented the feature\nCloses #42\n<!-- merge-ready-report: evidence -->\nVerdict: ready\n"));
    f.write("expected-head", f.git(&["rev-parse", "HEAD"]));
    succeeds(&squash(&f));
    assert!(
        f.read("merge-body").lines().any(|s| s == "Refs #42"),
        "{}",
        f.read("merge-body")
    );
    assert!(!f.read("merge-body").contains("Closes #42"));
    assert!(
        !f.read("merge-body").contains("Verdict:")
            && !f.read("merge-body").contains("merge-ready-report")
    );
    assert_eq!(f.read("merge-subject"), "fix: select reference (#99)");
    assert_eq!(
        f.read("events")
            .lines()
            .filter(|s| *s == "pr merge")
            .count(),
        1
    );
}

#[test]
fn existing_refs_below_readiness_boundary_are_restored_into_squash_description() {
    let f = Fixture::new();
    succeeds(&f.edit(
        "Description\n<!-- merge-ready-report: old -->\nRefs #42\n",
        "Updated description\n## Merge readiness\nNew evidence\n",
    ));
    f.write("expected-head", f.git(&["rev-parse", "HEAD"]));
    succeeds(&squash(&f));
    assert!(
        f.read("merge-body").contains("Refs #42"),
        "{}",
        f.read("merge-body")
    );
    assert!(!f.read("merge-body").contains("New evidence"));
}
