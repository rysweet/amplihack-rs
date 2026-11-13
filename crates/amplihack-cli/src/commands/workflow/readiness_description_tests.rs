//! Pure composer contract for #2550; no subprocesses or platform credentials.
use super::readiness_description::compose_readiness_description;

const MARKER: &str = "<!-- merge-ready-report: v1 -->";
const REPORT: &str = "<!-- merge-ready-report: v1 -->\n## Merge readiness\nVerdict: MERGE_READY\n";

#[test]
fn appending_a_report_preserves_description_bytes_and_published_link_identity() {
    for description in [
        "",
        "Refs #2550\n",
        "Closes plan/tasks#2550\r\n\r\n",
        "Refs #2550\nLiteral $(false); `command` and 'quotes'.\n",
    ] {
        let body = compose_readiness_description(description, REPORT).unwrap();
        assert!(body.starts_with(description));
        assert!(body.ends_with(REPORT));
        assert_eq!(body.matches(MARKER).count(), 1);
    }
}

#[test]
fn replacing_a_report_retains_the_exact_prefix_and_is_idempotent() {
    let prefix = "Summary\r\n\r\nRefs owner/repo#2550\r\n\r\n";
    for boundary in [MARKER, "## Merge readiness", "## MERGE READINESS\r"] {
        let description = format!("{prefix}{boundary}\nOld evidence.\n");
        let body = compose_readiness_description(&description, REPORT).unwrap();
        assert_eq!(body, format!("{prefix}{REPORT}"));
        assert_eq!(compose_readiness_description(&body, REPORT).unwrap(), body);
    }
}

#[test]
fn inline_and_indented_marker_examples_remain_description_data() {
    let description = "Refs #2550\nInline <!-- merge-ready-report example -->\n  ## Merge readiness example\nKeep the tail.\n";
    assert!(
        compose_readiness_description(description, REPORT)
            .unwrap()
            .starts_with(description)
    );
}

#[test]
fn duplicate_or_reordered_boundaries_are_rejected() {
    for description in [
        format!("Refs #2550\n{MARKER}\nold\n{MARKER}\nsecond\n"),
        format!("Refs #2550\n## Merge readiness\nold\n{MARKER}\nsecond\n"),
    ] {
        assert!(compose_readiness_description(&description, REPORT).is_err());
    }
    assert!(
        compose_readiness_description("Refs #2550\n", &format!("{REPORT}{MARKER}\nsecond\n"))
            .is_err()
    );
}

#[test]
fn every_closing_keyword_and_reference_form_is_rejected_only_in_new_report() {
    for keyword in [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ] {
        for reference in [
            "#2550",
            "owner/repo#2550",
            "https://github.com/owner/repo/issues/2550",
        ] {
            for trailer in [
                format!("{keyword} {reference}"),
                format!(
                    "**{}** [{reference}](https://github.com/owner/repo/issues/2550)",
                    keyword.to_uppercase()
                ),
                format!("{}: {reference}, #2551", keyword.to_uppercase()),
            ] {
                assert!(
                    compose_readiness_description("Refs #2550\n", &format!("{REPORT}{trailer}\n"))
                        .is_err(),
                    "accepted {trailer:?}"
                );
                let published = format!("Published {trailer}\n\n");
                assert!(
                    compose_readiness_description(&published, REPORT)
                        .unwrap()
                        .starts_with(&published),
                    "published link changed: {trailer:?}"
                );
            }
        }
    }
}

#[test]
fn issue_identity_without_a_closing_keyword_is_safe_evidence() {
    for evidence in [
        "Issue: owner/repo#2550",
        "Refs #2550",
        "Issue https://github.com/owner/repo/issues/2550",
        "Fixed-width output is covered.",
        "This fixes pagination and closes the dialog.",
        "[Fixes](https://example.org) pagination; [issue](https://github.com/owner/repo/issues/2550)",
        "[Fixes pagination](https://github.com/owner/repo/issues/2550)",
        "[Notes](https://example.org/fixes) #2550",
        "[Notes](https://example.org/closes) <https://github.com/owner/repo/issues/2550>",
        "Refs [issue][evidence]\n\n[evidence]: https://github.com/owner/repo/issues/2550",
        "[Fixes][] pagination.\n\n[Fixes]: https://example.org",
        "[Fixes] pagination.\n\n[Fixes]: https://example.org",
    ] {
        let report = format!("{REPORT}{evidence}\n");
        assert!(
            compose_readiness_description("Refs #2550\n", &report)
                .unwrap()
                .ends_with(&report)
        );
    }
}

#[test]
fn linked_keywords_and_issue_labels_cannot_hide_closing_references() {
    for closing in [
        "~~Fixes~~ **#2550**",
        r"\*Fixes\* #2550",
        "[Fixes](https://example.org) #2550",
        "Fixes [the issue](https://github.com/owner/repo/issues/2550)",
        "**Resolved**: [the issue](https://github.com/owner/repo/issues/2550 \"title\")",
        "[Fixes](https://github.com/owner/repo/issues/17) #2550",
        "[Fixes #2550](https://github.com/owner/repo/issues/17)",
        "[Fixes][issue]\n\n[issue]: https://github.com/owner/repo/issues/2550",
        "[Fixes][]\n\n[Fixes]: https://github.com/owner/repo/issues/2550",
        "[Fixes]\n\n[fixes]: https://github.com/owner/repo/issues/2550",
        "Fixes [issue][evidence]\n\n[evidence]: <https://github.com/owner/repo/issues/2550> \"title\"",
        "Fixes [issue][]\n\n[issue]: https://github.com/owner/repo/issues/2550",
        "Fixes [issue]\n\n[issue]: https://github.com/owner/repo/issues/2550",
        "[**Fixes**][The Issue] #2550\n\n[the   issue]: https://example.org",
        "[Fi&#120;es](https://github.com/owner/repo/issues/2550)",
        "[Fixes](https://example.org) [issue][target]\n\n[target]: https://github.com/owner/repo/issues/2550",
    ] {
        assert!(
            compose_readiness_description("Refs #2550\n", &format!("{REPORT}{closing}\n")).is_err(),
            "accepted {closing}"
        );
        let published = format!("Published {closing}\n\n");
        assert!(
            compose_readiness_description(&published, REPORT)
                .unwrap()
                .starts_with(&published),
            "published Markdown must remain authoritative: {closing}"
        );
    }
}

#[test]
fn production_template_marker_is_a_removable_report_boundary() {
    let marker = "<!-- merge-ready-report: everything from this line down is the pre-merge readiness report and is left out of the squash commit -->";
    let report = format!("{marker}\n## Merge readiness\nEvidence: owner/repo#2550\n");
    let description = "Refs #2550\n\n";
    let body = compose_readiness_description(description, &report).unwrap();
    assert_eq!(body, format!("{description}\n{report}"));
    assert_eq!(compose_readiness_description(&body, &report).unwrap(), body);
}

#[test]
fn only_new_report_links_are_validated_against_published_reference_definitions() {
    let published =
        "Published [Fixes][issue]\n\n[issue]: https://github.com/owner/repo/issues/2550\n\n";
    assert!(
        compose_readiness_description(published, REPORT)
            .unwrap()
            .starts_with(published)
    );
    for content in [
        "[Fixes][issue]",
        "Fixes [issue][]",
        "Fixes [issue]",
        // A duplicate definition in the report cannot override the earlier one.
        "Fixes [issue]\n\n[issue]: https://example.org",
    ] {
        assert!(
            compose_readiness_description(published, &format!("{REPORT}{content}\n")).is_err(),
            "accepted {content}"
        );
    }
    assert!(compose_readiness_description(published, &format!("{REPORT}Refs [issue]\n")).is_ok());
}
