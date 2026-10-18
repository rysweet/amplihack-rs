# Issue Deduplication — Reference

How amplihack recipes avoid creating duplicate GitHub issues when the same
task runs more than once.

## Contents

- [Current implementation](#current-implementation)
- [Guard 1: Reference extraction](#guard-1-reference-extraction)
- [Guard 2: Title search](#guard-2-title-search)
- [smart-orchestrator gap](#smart-orchestrator-gap)
- [Proposed design: Rust-side dedup](#proposed-design-rust-side-dedup)
- [Configuration](#configuration)

---

## Current implementation

The `workflow-prep.yaml` sub-recipe of `default-workflow` (step
`step-03-create-issue`) uses a two-guard idempotency pattern implemented
entirely in Bash. Both guards run before any `gh issue create` call. On
GitHub, Guard 1 is `select_github_tracking_issue` in
`amplifier-bundle/tools/workflow_issue_tracking.sh`; the
[Tracking Issue Selection Reference](workflow-tracking-issue-selection.md)
(#1595) describes it in full.

```
task_description
       │
       ▼
┌──────────────┐    match    ┌──────────────┐   exists   ┌──────────┐
│ issue_number │───────────▶│ gh issue view │──────────▶│ Reuse    │
│ or task #NNN │            │ / gh api      │  (open)   │ (exit 0) │
└──────────────┘            └──────────────┘           └──────────┘
       │ no match                   │ not found
       ▼                            ▼
┌──────────────┐    found    ┌──────────┐
│ gh issue list│───────────▶│ Reuse    │
│ --search     │            │ (exit 0) │
└──────────────┘            └──────────┘
       │ not found
       ▼
┌──────────────┐
│ gh issue     │
│ create       │
└──────────────┘
```

## Guard 1: Reference extraction

On a GitHub host, step-03 calls `select_github_tracking_issue`:

1. An explicit `issue_number` context value wins. It is checked with
   `gh issue view N --json url`; if that lookup fails, step-03 uses local
   tracking for that number instead of searching or creating.
2. Otherwise up to five `#N` references in the task text are tried, marked
   ones (`Issue #N`, `Fixes #N`, ...) first. `PR #N`, `owner/repo#N` and
   `AB#N` are never tried. Each is checked with
   `gh api repos/{owner}/{repo}/issues/N` and reused only if it is an open
   issue, not a pull request, in this repository.

When an issue is accepted, its URL is printed and the step exits with
code 0, skipping creation entirely. Every number is checked against
`^[0-9]+$` before it reaches `gh`.

On Azure DevOps, step-03 still takes the first `AB#N`, otherwise the first
`#N`, into `REF_ISSUE_NUM` and reuses that work item if
`az boards work-item show` finds it.

## Guard 2: Title search

When Guard 1 selects nothing (no explicit number and no task-text
reference accepted as an open issue), Guard 2 searches open issues by title:

```bash
SEARCH_QUERY="${ISSUE_TITLE:0:100}"
FOUND_URL=$(timeout 60 gh issue list --state open \
    --search "$SEARCH_QUERY" --json url --jq '.[0].url // ""' 2>/dev/null || echo '')
```

This matches the first 100 characters of the issue title. If a matching
open issue is found, its URL is reused.

**Limitation**: Title search uses GitHub's search API, which is fuzzy.
Unrelated issues with similar titles can cause false positives.

## smart-orchestrator gap

The `smart-orchestrator.yaml` recipe creates issues in its
`file-routing-bug` step (line ~627) with **no dedup guards at all**:

```bash
ISSUE_URL=$(gh issue create \
    --title "smart-orchestrator routing gap: ..." \
    --body "$BODY" \
    --label "bug" 2>/dev/null) || true
```

This is the primary source of duplicate issue noise. Each routing failure
files a new issue regardless of whether an identical one already exists.

## Proposed design: Rust-side dedup

The following types are **design specifications for future implementation**,
not existing code.

### `IssueClient` trait

```rust
// Proposed — not yet implemented
pub trait IssueClient: Send + Sync {
    /// Search for an existing issue by content fingerprint.
    fn find_by_fingerprint(&self, fingerprint: &str) -> Result<Option<IssueRef>>;

    /// Add a comment to an existing issue instead of creating a new one.
    fn add_comment(&self, issue: u64, body: &str) -> Result<()>;

    /// Create a new issue. Returns the issue URL.
    fn create(&self, title: &str, body: &str, labels: &[&str]) -> Result<String>;
}
```

### Fingerprint algorithm

The proposed fingerprint hashes the normalized issue title and the `bug`
label to produce a deterministic key. Same-day duplicates append a comment;
next-day duplicates create a rollup issue referencing the original.

### `GhCliClient` and `MockIssueClient`

- `GhCliClient`: Production implementation that shells out to `gh`.
- `MockIssueClient`: Test double that records calls in a `Vec<Call>`.

### Decision tree (proposed)

| Condition | Action |
|---|---|
| Fingerprint matches open issue, same calendar day | Append comment |
| Fingerprint matches open issue, different day | Create rollup referencing original |
| No fingerprint match | Create new issue |

## Configuration

### Environment variables (existing)

| Variable | Default | Description |
|---|---|---|
| `GH_TOKEN` / `GITHUB_TOKEN` | (required) | GitHub CLI authentication |

### Environment variables (proposed)

| Variable | Default | Description |
|---|---|---|
| `AMPLIHACK_ISSUE_DEDUP` | `1` | Enable (`1`) or disable (`0`) fingerprint dedup |
| `AMPLIHACK_ISSUE_DEDUP_WINDOW` | `86400` | Seconds within which duplicates are commented, not created |

## Related

- [amplihack recipe](./recipe-command.md) — CLI reference for recipe execution
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — How recipe steps run
- [Smart-Orchestrator Recovery](../concepts/smart-orchestrator-recovery.md) — Failure handling that triggers issue creation
