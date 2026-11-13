//! Pure issue-link policy contracts for #2550.
use super::issue_keyword::{Classification, IssueKeyword, IssueLinkMode, select_issue_keyword};

#[test]
fn substantive_classification_closes_unless_the_caller_requests_refs() {
    for (mode, expected) in [
        (IssueLinkMode::Auto, IssueKeyword::Closes),
        (IssueLinkMode::Refs, IssueKeyword::Refs),
        (IssueLinkMode::Closes, IssueKeyword::Closes),
    ] {
        assert_eq!(
            select_issue_keyword(mode, Some(Classification::Substantive)),
            expected,
        );
    }
}

#[test]
fn non_substantive_classification_never_closes_even_when_requested() {
    for mode in [
        IssueLinkMode::Auto,
        IssueLinkMode::Refs,
        IssueLinkMode::Closes,
    ] {
        assert_eq!(
            select_issue_keyword(mode, Some(Classification::NonSubstantive)),
            IssueKeyword::Refs,
        );
    }
}

#[test]
fn unavailable_classification_is_reference_only_for_every_mode() {
    for mode in [
        IssueLinkMode::Auto,
        IssueLinkMode::Refs,
        IssueLinkMode::Closes,
    ] {
        assert_eq!(select_issue_keyword(mode, None), IssueKeyword::Refs);
    }
}
