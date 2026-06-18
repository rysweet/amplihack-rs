//! TDD contract tests for the merge-ready skill platform-neutral contract.
//!
//! These tests fail against the previous GitHub-only merge-ready wording and
//! pass once the skill gives explicit GitHub and Azure DevOps/AzDO guidance for
//! each required merge-readiness aspect.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dir = crate_dir.clone();
    while !dir.join("amplifier-bundle").exists() {
        if !dir.pop() {
            panic!("could not find amplifier-bundle from {crate_dir:?}");
        }
    }
    dir
}

fn merge_ready_skill() -> String {
    let path = repo_root().join("amplifier-bundle/skills/merge-ready/SKILL.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn merge_ready_template() -> String {
    let path = repo_root().join("amplifier-bundle/skills/merge-ready/pr-description-template.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn assert_contains_all(haystack: &str, needles: &[&str], context: &str) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "{context} must contain `{needle}`"
        );
    }
}

fn markdown_table_row<'a>(document: &'a str, aspect: &str) -> Vec<&'a str> {
    let prefix = format!("| {aspect} |");
    let row = document
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("merge-readiness matrix must include row `{aspect}`"));

    row.trim_matches('|').split('|').map(str::trim).collect()
}

fn markdown_section<'a>(document: &'a str, heading: &str) -> &'a str {
    let start = document
        .find(heading)
        .unwrap_or_else(|| panic!("document must include heading `{heading}`"));
    let remainder = &document[start..];
    let next_heading = remainder
        .lines()
        .skip(1)
        .scan(heading.len(), |offset, line| {
            let current = *offset;
            *offset += line.len() + 1;
            Some((current, line))
        })
        .find_map(|(offset, line)| line.starts_with("### ").then_some(offset));

    match next_heading {
        Some(end) => &remainder[..end],
        None => remainder,
    }
}

#[test]
fn skill_top_level_contract_uses_platform_neutral_merge_readiness_terms() {
    let skill = merge_ready_skill();
    let neutral_intro = skill
        .split("## Platform scope")
        .next()
        .expect("skill must have content before Platform scope");
    let required_outcome = skill
        .split("## Required outcome")
        .nth(1)
        .and_then(|section| section.split("## Non-negotiable guardrails").next())
        .expect("skill must have a Required outcome section");

    assert_contains_all(
        &skill,
        &[
            "PR/pull request",
            "checks/build validation",
            "reviews/approvals",
            "policies/protection",
            "linked work items/issues",
            "comments/threads",
        ],
        "merge-ready skill",
    );

    for neutral_section in [neutral_intro, required_outcome] {
        for github_specific in [
            "GitHub Actions",
            "`gh`",
            "branch protection",
            "GitHub issues",
        ] {
            assert!(
                !neutral_section.contains(github_specific),
                "top-level neutral merge-readiness sections must not use GitHub-specific term `{github_specific}`"
            );
        }
    }

    assert!(
        !skill.contains("CI status"),
        "merge-ready skill must use checks/build validation wording instead of GitHub-flavored CI status"
    );
}

#[test]
fn skill_declares_github_and_azure_devops_platform_paths() {
    let skill = merge_ready_skill();

    assert_contains_all(
        &skill,
        &[
            "The top-level merge-readiness contract is platform-neutral.",
            "**GitHub**: GitHub pull requests, GitHub Actions, `gh`, GitHub issues, review conversations, and branch protection.",
            "**Azure DevOps/AzDO**: Azure DevOps, AzDO, Azure Repos pull requests, Azure Pipelines, `az repos`, `az pipelines`, Azure Boards work items, PR threads, and branch policies.",
            "report `NOT_MERGE_READY` with a platform-access blocker instead of assuming GitHub behavior",
            "Platform detection signals:",
            "`github.com` remotes or PR URLs",
            "`dev.azure.com` or `*.visualstudio.com` remotes or PR URLs",
            "Azure Repos remote paths",
        ],
        "platform scope",
    );
}

#[test]
fn merge_readiness_matrix_covers_every_required_aspect_for_both_platforms() {
    let skill = merge_ready_skill();

    let aspects = [
        (
            "Checks/build validation",
            &["GitHub Actions", "`gh pr checks \"$PR\"`"][..],
            &[
                "Azure Pipelines",
                "`az repos pr policy list --id \"$PR_ID\"`",
            ][..],
        ),
        (
            "PR/pull request metadata",
            &["`gh pr view`", "GitHub UI"][..],
            &["`az repos pr show`", "Azure Repos PR"][..],
        ),
        (
            "Reviews/approvals",
            &["reviewDecision", "requested changes"][..],
            &[
                "`az repos pr reviewer list --id \"$PR_ID\"`",
                "branch policy status",
            ][..],
        ),
        (
            "Merge conflicts",
            &["mergeable", "mergeStateStatus"][..],
            &["mergeStatus", "AzDO UI"][..],
        ),
        (
            "Policies/protection",
            &["branch protection", "`gh api`"][..],
            &[
                "branch policy evidence",
                "`az repos pr policy list --id \"$PR_ID\"`",
            ][..],
        ),
        (
            "Linked work items/issues",
            &["closingIssuesReferences", "issue links"][..],
            &[
                "`az repos pr work-item list --id \"$PR_ID\"`",
                "Azure Boards work items",
            ][..],
        ),
        (
            "Comments/threads",
            &["review conversation", "`gh api`"][..],
            &["PR thread evidence", "authorized REST API"][..],
        ),
        (
            "Final merge/close verification",
            &["`gh pr merge`", "mergedAt"][..],
            &[
                "platform-approved completion command",
                "`az repos pr show --id \"$PR_ID\"",
                "`az repos pr update --id \"$PR_ID\" --status completed` is an example only, not the default path",
            ][..],
        ),
    ];

    for (aspect, github_requirements, azdo_requirements) in aspects {
        let cells = markdown_table_row(&skill, aspect);
        assert_eq!(
            cells.len(),
            3,
            "matrix row `{aspect}` must have Aspect, GitHub path, and Azure DevOps/AzDO path cells"
        );
        assert_eq!(cells[0], aspect);
        assert_contains_all(
            cells[1],
            github_requirements,
            &format!("GitHub guidance for {aspect}"),
        );
        assert_contains_all(
            cells[2],
            azdo_requirements,
            &format!("Azure DevOps/AzDO guidance for {aspect}"),
        );
    }
}

#[test]
fn skill_requires_evidence_or_not_merge_ready_for_external_blockers() {
    let skill = merge_ready_skill();

    assert_contains_all(
        &skill,
        &[
            "Collect explicit evidence for every aspect below before declaring `MERGE_READY`.",
            "If any criterion is missing, stale, inaccessible, or blocked by external approval, check, or policy state, the PR/pull request is **not merge-ready**.",
            "`MERGE_READY`: every criterion passed, required evidence was added to the PR/pull request description, and no platform blocker remains",
            "`NOT_MERGE_READY`: one or more blockers remain",
            "platform access or authentication cannot prove required metadata, reviews/approvals, mergeability, policies/protection, linked work items/issues, comments/threads, or final state",
        ],
        "evidence and blocker contract",
    );
}

#[test]
fn evidence_template_has_platform_neutral_fields_and_examples_for_all_aspects() {
    let template = merge_ready_template();

    assert_contains_all(
        &template,
        &[
            "# PR/pull request description evidence template",
            "- Platform: `<GitHub / Azure DevOps/AzDO>`",
            "- PR/pull request: `<number or ID and URL>`",
            "- Verdict: `<MERGE_READY / NOT_MERGE_READY>`",
        ],
        "template header and verdict",
    );

    for heading in [
        "### Checks/build validation",
        "### PR/pull request metadata",
        "### Reviews/approvals",
        "### Merge conflicts",
        "### Policies/protection",
        "### Linked work items/issues",
        "### Comments/threads",
        "### Final merge/close verification",
    ] {
        let section = markdown_section(&template, heading);
        assert!(
            section.contains("GitHub example:"),
            "{heading} must include clearly labeled GitHub guidance"
        );
        assert!(
            section.contains("Azure DevOps/AzDO example:"),
            "{heading} must include clearly labeled Azure DevOps/AzDO guidance"
        );
    }

    assert_contains_all(
        &template,
        &[
            "- Required linked work items/issues: `<satisfied / not applicable / missing>`",
            "- Required comments/threads resolved: `<yes / no / not applicable>`",
            "Evidence source: az repos pr work-item list --id 456",
            "Azure DevOps/AzDO example: PR thread evidence from the Azure DevOps UI or authorized REST API.",
        ],
        "linked work item and thread evidence",
    );
}
