//! Preserve the established post-crusty allowlist using sanitized local Git.
use super::{records::CandidateRecord, trusted};
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub fn verify(repo: &Path, candidate: &CandidateRecord) -> Result<()> {
    let reviewed = &candidate.reviewed_head_sha;
    let head = &candidate.binding.head_sha;
    let git = |args: &[&str]| trusted::git(repo, args);
    ensure!(
        git(&["rev-parse", "--is-shallow-repository"])? == "false",
        "crusty-range-unreadable: shallow repository"
    );
    if reviewed == head {
        return Ok(());
    }
    git(&["merge-base", "--is-ancestor", reviewed, head])
        .context("crusty-review-required: reviewed head is not an ancestor")?;
    let base = &candidate.binding.base_sha;
    let commits = git(&[
        "rev-list",
        "--reverse",
        "--topo-order",
        &format!("{reviewed}..{head}"),
        &format!("^{base}"),
    ])
    .context("crusty-range-unreadable")?;
    for commit in commits.lines() {
        let parent_text = git(&["show", "-s", "--format=%P", commit])?;
        let parents: Vec<_> = parent_text.split_whitespace().collect();
        match parents.as_slice() {
            [first, second] => {
                git(&["merge-base", "--is-ancestor", second, base])
                    .context("crusty-review-required: merge is not from base")?;
                let expected = git(&["merge-tree", "--write-tree", first, second])
                    .context("crusty-review-required: base merge is not clean")?;
                let actual = git(&["rev-parse", "--verify", &format!("{commit}^{{tree}}")])?;
                ensure!(
                    expected.lines().next() == Some(actual.as_str()),
                    "crusty-review-required: base merge contains additional changes"
                );
            }
            [first] => {
                let changes = git(&[
                    "diff-tree",
                    "-r",
                    "-z",
                    "--no-renames",
                    "--raw",
                    first,
                    commit,
                ])?;
                let parts: Vec<_> = changes.split_terminator('\0').collect();
                ensure!(
                    !parts.is_empty() && parts.len().is_multiple_of(2),
                    "crusty-review-required: empty or unreadable commit"
                );
                for change in parts.chunks_exact(2) {
                    let mode = change[0]
                        .split_whitespace()
                        .nth(1)
                        .context("crusty-range-unreadable: missing mode")?;
                    let path = change[1];
                    ensure!(
                        matches!(mode, "100644" | "100755" | "000000")
                            && (matches!(
                                path,
                                "PR_DESCRIPTION.md" | ".github/pull_request_template.md"
                            ) || path.starts_with(".autodrive/evidence/")),
                        "crusty-review-required:{commit}: substantive path or mode"
                    );
                }
            }
            _ => anyhow::bail!("crusty-review-required:{commit}: root or octopus commit"),
        }
    }
    Ok(())
}
