# Issue Deduplication — Reference

How amplihack recipes avoid creating duplicate GitHub issues when the same
task runs more than once.

## Contents

- [Current implementation](#current-implementation)
- [Guard 1: Reference extraction](#guard-1-reference-extraction)
- [Opt-in gate](#opt-in-gate)
- [Guard 2: Exact-title search](#guard-2-exact-title-search)
- [smart-orchestrator gap](#smart-orchestrator-gap)
- [Proposed design: Rust-side dedup](#proposed-design-rust-side-dedup)
- [Configuration](#configuration)

---

## Current implementation

The `workflow-prep.yaml` sub-recipe of `default-workflow` (step
`step-03-create-issue`) uses a two-guard idempotency pattern implemented
entirely in Bash, with an opt-in gate between the guards (#1881). Both guards
run before any `gh issue create` call. On GitHub, Guard 1 is
`select_github_tracking_issue` in
`amplifier-bundle/tools/workflow_issue_tracking.sh`, and Guard 2 and the
create are `find_open_issue_before_create` and
`create_github_issue_bounded` in `amplifier-bundle/tools/workflow_issue_create.sh`.
The [Tracking Issue Selection Reference](workflow-tracking-issue-selection.md)
describes all three in full.

```
task_description
       │
       ▼
┌──────────────┐  match   ┌──────────────┐  exists  ┌──────────┐
│ issue_number │─────────▶│ gh issue view│─────────▶│ Reuse    │
│ or task #NNN │          │ / gh api     │  (open)  │ (exit 0) │
└──────────────┘          └──────────────┘          └──────────┘
       │ no match                │ not found
       ▼                         ▼
┌──────────────┐   no     ┌────────────────┐
│ create_issue │─────────▶│ Local tracking │
│ = true?      │          │ (exit 0)       │
└──────────────┘          └────────────────┘
       │ yes
       ▼
┌──────────────┐ identical┌──────────┐
│ gh issue list│─────────▶│ Reuse    │
│ exact title  │  title   │ (exit 0) │
└──────────────┘          └──────────┘
       │ lookup failed (search, then recent listing) ──▶ local tracking
       │ no match
       ▼
┌──────────────┐  secondary limit: wait 60 s + jitter, look up again,
│ gh issue     │  give up to local tracking after
│ create       │  AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS
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

## Opt-in gate

When Guard 1 selects nothing, step-03 creates nothing unless the run passed
`create_issue=true` (also `1` or `yes`, any case). Without it, step-03 writes
local tracking metadata and makes no GitHub write, so repeated runs of the
same task cannot add issues at all. See
[Local tracking by default](workflow-tracking-issue-selection.md#local-tracking-by-default).

## Guard 2: Exact-title search

With the opt-in on, Guard 2 lists up to 100 open issues whose title contains
the task title as a phrase, then keeps only the one whose title is exactly
equal to the normalised title (control characters removed, cut to 200
characters, trimmed):

```bash
GH_PROMPT_DISABLED=1 timeout 30 gh issue list --state open --limit 100 \
  --search='"Add a --json flag to recipe list" in:title' \
  --json number,title,url </dev/null
```

A `jq` program then checks the title, the URL shape and the repository of
every result, and prints the URL of the first one that passes all three. A
result for another repository is skipped, not reused. Who opened the issue is
not checked, so a run under a different gh account reuses the same issue.
If the search fails, Guard 2 tries a plain listing of the newest open issues
instead, which uses the core API quota. If that fails too, step-03 uses local
tracking and creates nothing, because it cannot tell whether another run of
the task has already made the issue. The full program is in
[Exact-title reuse](workflow-tracking-issue-selection.md#lookup). Before every
retry of a rate-limited create, and once after a create that timed out, the
lookup runs again as a plain listing of the newest open issues without
`--search`, so a create that GitHub accepted but reported as failed is reused
instead of duplicated. See
[Exact-title reuse](workflow-tracking-issue-selection.md#exact-title-reuse).

This replaces the earlier fuzzy match, which reused the first search hit for
the first 100 characters of the title and could attach a run to an unrelated
issue.

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
