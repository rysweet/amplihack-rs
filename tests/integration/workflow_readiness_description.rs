//! #2550 composer, fetch-compose-edit and finalisation/squash contracts.
//! These tests submit actual fake-service edits and inspect platform readback.
use super::workflow_issue_link_fixture::{Fixture, MARKER, PR, command, describe, root};
use std::fs;

fn compose(f: &Fixture, description: &str, report: &str) -> std::process::Output {
    f.stdin(
        &["workflow", "compose-readiness-description"],
        &serde_json::json!({"description": description, "report": report}).to_string(),
    )
}

fn report(verdict: &str) -> String {
    format!("{MARKER}\n## Merge readiness\nVerdict: {verdict}\nIssue evidence: owner/repo#2550\n")
}

#[test]
fn composer_appends_one_report_preserving_the_published_description_verbatim() {
    let f = Fixture::new();
    for description in [
        "",
        "Refs #2550",
        "## Change\r\n\r\nRefs owner/repo#2550\r\n\r\n",
        "Closes #2550\n\nA published closing link is authoritative.\n",
        "Refs #2550\nLiteral $(touch should-not-exist); `false` and 'quotes'\n",
    ] {
        let report = report("MERGE_READY");
        let out = compose(&f, description, &report);
        assert!(out.status.success(), "{}", describe(&out));
        let body = String::from_utf8(out.stdout).unwrap();
        assert!(
            body.starts_with(description),
            "published bytes must survive: {body:?}"
        );
        assert!(
            body.ends_with(&report),
            "replacement report is data: {body:?}"
        );
        assert_eq!(body.matches(MARKER).count(), 1, "one removable report");
    }
    assert!(!f.repo.join("should-not-exist").exists());
}

#[test]
fn composer_replaces_existing_marker_and_legacy_reports_without_changing_prefix() {
    let f = Fixture::new();
    let prefix = "## Change\r\nKeep this exact whitespace.  \r\n\r\nRefs plan/tasks#2550\r\n\r\n";
    for old in [
        format!("{MARKER}\n## Merge readiness\nVerdict: OLD\n"),
        "## Merge readiness\nVerdict: OLD\n".to_owned(),
        "## MERGE READINESS\r\nVerdict: OLD\r\n".to_owned(),
    ] {
        let replacement = report("MERGE_READY");
        let out = compose(&f, &format!("{prefix}{old}"), &replacement);
        assert!(out.status.success(), "{}", describe(&out));
        let body = String::from_utf8(out.stdout).unwrap();
        assert_eq!(body, format!("{prefix}{replacement}"));
        let second = compose(&f, &body, &replacement);
        assert!(second.status.success(), "{}", describe(&second));
        assert_eq!(
            second.stdout,
            body.as_bytes(),
            "replacing an identical report is idempotent"
        );
    }
}

#[test]
fn inline_or_indented_boundary_text_does_not_truncate_the_description() {
    let f = Fixture::new();
    let description = "Refs #2550\nUse `<!-- merge-ready-report` as a marker.\n  ## Merge readiness example\nTail must survive.\n";
    let out = compose(&f, description, &report("MERGE_READY"));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with(description)
    );
}

#[test]
fn conflicting_boundaries_are_rejected_without_partial_output() {
    let f = Fixture::new();
    for description in [
        format!("Refs #2550\n{MARKER}\n## Merge readiness\nold\n{MARKER}\nsecond\n"),
        format!("Refs #2550\n## Merge readiness\nold\n{MARKER}\nsecond\n"),
    ] {
        let out = compose(&f, &description, &report("MERGE_READY"));
        assert!(
            !out.status.success(),
            "conflicting boundary accepted: {}",
            describe(&out)
        );
        assert!(
            out.stdout.is_empty(),
            "failed composition must emit no body"
        );
    }
    let out = compose(
        &f,
        "Refs #2550\n",
        &format!("{}\n{MARKER}\nsecond report\n", report("MERGE_READY")),
    );
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn composer_rejects_all_closing_keyword_families_and_reference_forms() {
    let f = Fixture::new();
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
                let out = compose(
                    &f,
                    "Refs #2550\n",
                    &format!("{}\n{trailer}\n", report("MERGE_READY")),
                );
                assert!(
                    !out.status.success(),
                    "unsafe report accepted {trailer:?}: {}",
                    describe(&out)
                );
                assert!(out.stdout.is_empty(), "no partial output for {trailer:?}");
            }
        }
    }
}

#[test]
fn malformed_requests_and_non_string_fields_never_produce_a_body() {
    let f = Fixture::new();
    for input in [
        "",
        "not json",
        "[]",
        "{}",
        r#"{"description":"Refs #2550"}"#,
        r#"{"report":"safe"}"#,
        r#"{"description":null,"report":"safe"}"#,
        r#"{"description":42,"report":"safe"}"#,
        r#"{"description":"safe","report":{}}"#,
        r#"{"description":"safe","report":null}"#,
        "{\"description\":\"safe\",\"report\":\"safe\"} trailing data",
    ] {
        let out = f.stdin(&["workflow", "compose-readiness-description"], input);
        assert!(!out.status.success(), "{input:?}: {}", describe(&out));
        assert!(
            out.stdout.is_empty(),
            "malformed request emitted a body: {input:?}"
        );
    }
}

/// Use the executable GitHub example maintained in the production skill. The
/// example's inputs are PR (trusted identity) and REPORT_FILE (private data).
/// Execute the fetch-compose-edit example; preservation prose alone cannot
/// establish behavioural coverage.
fn editing_script() -> String {
    let skill =
        fs::read_to_string(root().join("amplifier-bundle/skills/merge-ready/SKILL.md")).unwrap();
    skill.split("```bash\n").skip(1)
        .filter_map(|part| part.split("```").next())
        .find(|script| script.contains("compose-readiness-description") && script.contains("gh pr edit"))
        .expect("merge-ready must supply an executable fetch-compose-edit example using PR and REPORT_FILE")
        .to_owned()
}

fn edit(f: &Fixture) -> std::process::Output {
    f.bash(&editing_script())
        .env("PR", PR)
        .env("REPORT_FILE", f.path().join("report.md"))
        .output()
        .unwrap()
}

fn seed_published_refs(f: &Fixture) {
    f.commit_change("src/feature.rs");
    // Seed a concise published description for composition failure tests.
    let out = f.publish(Some("refs"));
    assert!(out.status.success(), "{}", describe(&out));
    let mut pr: serde_json::Value = serde_json::from_str(&f.read("pr.json")).unwrap();
    pr["body"] = "## Change\nA substantive fix.\n\nRefs #2550\n".into();
    f.write("pr.json", &pr.to_string());
    f.write("report.md", &report("MERGE_READY"));
    f.write("operations", "");
}

#[test]
fn production_edit_path_preserves_refs_with_empty_closing_references() {
    let f = Fixture::new();
    seed_published_refs(&f);
    let mut initial: serde_json::Value = serde_json::from_str(&f.read("pr.json")).unwrap();
    let description = "## Change\nA substantive fix.\n\nRefs #2550\nLiteral $(touch description-executed) and 'quotes'.\n";
    initial["body"] = description.into();
    f.write("pr.json", &initial.to_string());
    f.write(
        "report.md",
        &format!(
            "{}\nLiteral $(touch report-executed); `touch backticks-executed`.\n",
            report("MERGE_READY")
        ),
    );
    let out = edit(&f);
    assert!(out.status.success(), "{}", describe(&out));
    let readback = f
        .child("gh")
        .args(["pr", "view", PR, "--json", "body,closingIssuesReferences"])
        .output()
        .unwrap();
    assert!(readback.status.success());
    let pr: serde_json::Value = serde_json::from_slice(&readback.stdout).unwrap();
    assert_eq!(pr["closingIssuesReferences"], serde_json::json!([]));
    assert!(pr["body"].as_str().unwrap().starts_with(description));
    assert!(
        pr["body"]
            .as_str()
            .unwrap()
            .contains("$(touch report-executed)")
    );
    for name in [
        "description-executed",
        "report-executed",
        "backticks-executed",
    ] {
        assert!(
            !f.repo.join(name).exists(),
            "platform text was executed: {name}"
        );
    }
    assert!(
        pr["body"]
            .as_str()
            .unwrap()
            .contains("Verdict: MERGE_READY")
    );
    assert!(
        f.read("operations")
            .lines()
            .any(|line| line.starts_with("pr edit "))
    );
}

#[test]
fn failed_fetch_parse_or_validation_leaves_platform_description_untouched() {
    for failure in ["fetch", "parse", "closing-reference", "boundary"] {
        let f = Fixture::new();
        seed_published_refs(&f);
        match failure {
            "fetch" => f.write("fetch-fails", ""),
            "parse" => f.write("pr.json", "not-json"),
            "closing-reference" => f.write(
                "report.md",
                &format!("{}\nFixes owner/repo#2550\n", report("MERGE_READY")),
            ),
            _ => f.write(
                "report.md",
                &format!("{}\n{MARKER}\nconflicting report\n", report("MERGE_READY")),
            ),
        }
        let before = f.read("pr.json");
        let out = edit(&f);
        assert!(!out.status.success(), "{failure}: {}", describe(&out));
        assert_eq!(
            f.read("pr.json"),
            before,
            "{failure}: platform state changed"
        );
        assert!(
            !f.read("operations")
                .lines()
                .any(|line| line.starts_with("pr edit ")),
            "{failure}: edit submitted after failure"
        );
    }
}

#[test]
fn existing_finalisation_and_squash_paths_preserve_an_already_published_refs_link() {
    let f = Fixture::new();
    seed_published_refs(&f);
    let before: serde_json::Value = serde_json::from_str(&f.read("pr.json")).unwrap();
    let out = f
        .bash(&command("workflow-finalize", "step-21-pr-ready"))
        .env("RECIPE_VAR_pr_url", PR)
        .env("RECIPE_VAR_pr_number", "99")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let after: serde_json::Value = serde_json::from_str(&f.read("pr.json")).unwrap();
    assert_eq!(after["isDraft"], false);
    assert_eq!(after["body"], before["body"]);
    let mut with_report = after;
    with_report["body"] = format!(
        "{}\n{}",
        with_report["body"].as_str().unwrap(),
        report("NOT_MERGE_READY")
    )
    .into();
    f.write("pr.json", &with_report.to_string());
    let out = f
        .bash(
            r#"
set -euo pipefail
. "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_message.sh"
pr_json=$(gh pr view "$PR" --json title,body)
autodrive_merge_body "$pr_json"
printf '%s' "$MERGE_BODY"
"#,
        )
        .env("PR", PR)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        before["body"].as_str().unwrap().trim_end()
    );
}

#[test]
fn published_refs_survive_actual_finalisation_readiness_edits_and_squash_body() {
    let f = Fixture::new();
    f.commit_change("src/feature.rs");
    f.git(&["push"]);
    let published = f.assert_published(&f.publish(Some("refs")), "Refs #2550");
    let out = f
        .bash(&command("workflow-finalize", "step-21-pr-ready"))
        .env("RECIPE_VAR_pr_url", PR)
        .env("RECIPE_VAR_pr_number", "99")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let pr: serde_json::Value = serde_json::from_str(&f.read("pr.json")).unwrap();
    assert_eq!(pr["isDraft"], false, "the actual ready operation must run");
    assert_eq!(
        pr["body"], published,
        "finalisation must not reconstruct the description"
    );
    assert!(
        f.read("operations")
            .lines()
            .any(|line| line.starts_with("pr ready "))
    );
    f.write("report.md", &report("NOT_MERGE_READY"));
    let out = edit(&f);
    assert!(out.status.success(), "{}", describe(&out));
    f.write("report.md", &report("MERGE_READY"));
    let out = edit(&f);
    assert!(out.status.success(), "{}", describe(&out));
    // Exercise the existing production squash helper against platform readback,
    // not a separate reimplementation of its boundary or trailer policy.
    let out = f
        .bash(
            r#"
set -euo pipefail
. "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_message.sh"
pr_json=$(gh pr view "$PR" --json title,body,closingIssuesReferences)
autodrive_merge_body "$pr_json"
printf '%s' "$MERGE_BODY"
"#,
        )
        .env("PR", PR)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let body = String::from_utf8(out.stdout).unwrap();
    assert_eq!(body, published.trim_end());
    assert_eq!(body.lines().filter(|line| *line == "Refs #2550").count(), 1);
    for unwanted in [
        "Closes #2550",
        "Fixes #17",
        "Task: unrelated-trailer",
        "Verdict:",
        "merge-ready-report",
        "## Merge readiness",
    ] {
        assert!(
            !body.contains(unwanted),
            "squash body leaked {unwanted:?}: {body}"
        );
    }
}
