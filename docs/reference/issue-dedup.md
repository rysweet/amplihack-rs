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

The `default-workflow.yaml` recipe (step `step-03-create-issue`) uses a
two-guard idempotency pattern implemented entirely in Bash. Both guards
run before any `gh issue create` call.

```
task_description
       │
       ▼
┌──────────────┐    match    ┌──────────────┐   exists   ┌──────────┐
│ Extract #NNN │───────────▶│ gh issue view │──────────▶│ Reuse    │
│ from text    │            │ $REF_ISSUE    │           │ (exit 0) │
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

Extracts an issue number from the `task_description` context variable using
a Bash regex match on `#([0-9]+)`.

```bash
# From default-workflow.yaml, step-03-create-issue
if [[ "$TASK_DESC" =~ \#([0-9]+) ]]; then
    REF_ISSUE_NUM="${BASH_REMATCH[1]}"
fi
```

If a number is extracted, the guard verifies the issue exists via
`gh issue view "$REF_ISSUE_NUM"`. When the issue exists, its URL is printed
and the step exits with code 0, skipping creation entirely.

**Defense-in-depth**: A secondary check rejects non-numeric values before
interpolating into the `gh` command.

## Guard 2: Title search

When Guard 1 does not match (no `#NNN` in the task description) or the
referenced issue does not exist, Guard 2 searches open issues by title:

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
