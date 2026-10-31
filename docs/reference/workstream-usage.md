# Workstream token usage

`amplihack usage report --manifest usage.json --format json` collects native Claude
Code and Codex JSONL accounting records without model calls or network access.
Omit `--format json` for a readable Markdown summary. Source files are opened
read-only. Only files explicitly listed in the manifest are read; relative paths
resolve against the manifest's directory. There is no automatic home-directory
scan, quota governor, daemon, account switching, or pricing download.

## Manifest version 1

```json
{
  "version": 1,
  "workstream": "issue-1810",
  "repository": "owner/repository",
  "branch": "feat/usage",
  "prs": [42],
  "completed": false,
  "sessions": [
    {
      "id": "native-session-id",
      "provider": "codex",
      "run_id": "recipe-run-uuid",
      "parent": null,
      "complete": true,
      "files": ["logs/start.jsonl", "logs/resume.jsonl"]
    },
    {
      "id": "native-child-id",
      "provider": "claude",
      "run_id": "review-run-uuid",
      "parent": "native-session-id",
      "complete": false,
      "files": ["logs/review.jsonl"]
    }
  ]
}
```

Use the native `session_meta.payload.id` (Codex) or `sessionId` (Claude), not a
filename-derived guess. Copy recipe `run_id` from `amplihack.recipe.log_pointer`
or `AMPLIHACK_RECIPE_RUN_ID`; copy the workstream and PR identities from the
workstream owner. There is no inferred association from cwd, prompts, branch
names, or time proximity. Keep the manifest private because it contains local
paths. Report JSON deliberately omits input paths, record contents, prompts,
tool arguments, account data, and credentials. Diagnostics use zero-based
manifest session/file ordinals and one-based line numbers, with fixed codes.

List every implementation, child, review, retry, and resumed session. A resume
adds files to the same session entry. A retry with a new native session gets a
new entry and its actual run ID. `parent` identifies a native session, even
across providers. Missing ancestors are diagnosed; cycles and duplicate session
IDs are rejected. An absent workstream/repository/branch/PR or run ID leaves
usage visibly unattributed. Missing, partial, malformed, unidentifiable, or
unsupported accounting data is diagnosed; absent token fields are lower bounds,
never proof of zero consumption. Unknown native session IDs are omitted with
`unmapped_native_session`, rather than assigned to another workstream.

`complete: true` is an explicit input-coverage assertion by the manifest owner.
It must only be set when the files cover the whole session. A clean parse cannot
prove that a log is exhaustive. Report completeness means complete **for those
declared inputs**, not that every provider request or billed attempt was logged.
Missing logs, unrecorded failed requests and future adapter schemas cannot be
reconstructed. Keep `complete: false` while late reviews or retries are expected.
`completed` authorizes publication of the finished workstream; it does not change
or certify accounting completeness.

## Counting contract

- Claude messages deduplicate globally by provider message ID, merging streamed
  usage by component maxima. Copied history uses its native `sessionId` to retain
  the original owner. Cache creation totals include 5-minute/1-hour partitions;
  the partitions are displayed without adding them again. An unpartitioned
  remainder has unknown duration. Claude reasoning remains inside output when
  the native record does not expose a separate count.
- Codex cumulative counters are ordered across all explicitly supplied resume
  files. Per-session high-water deltas are taken before subtracting cache-read
  and reasoning subsets, and before assigning the delta to the current model.
  Model changes do not reset the cumulative baseline. Repeated total snapshots
  and repeated `last_token_usage` beside a cumulative snapshot add nothing.
  Counter regressions remain incomplete lower bounds; no reset epoch is guessed.
  Last-only observations require a stable request ID; without it the collector
  omits ambiguous usage and diagnoses it. Prefer complete cumulative rollouts.
- Direct session totals count owned observations only. Inclusive totals include
  descendants; they overlap and must not be summed. The workstream total sums
  direct totals once. Stream/copy deduplication is global for provider IDs;
  cumulative counters are scoped to native session identity. Copied histories
  that erase original IDs cannot be reliably reconstructed.
- `observed_attempts` means unique Claude messages or positive Codex cumulative
  transitions / unique last-only request IDs. Exact provider invocation counts,
  including unlogged failed calls, are unknown. Models and token buckets remain
  separate in JSON. Timestamp bounds cover observed usage, not wall-clock work.
- Unknown models are retained, and API list-price estimates and price references
  remain `null`. There is no hardcoded price table. Counts are observations,
  distinct from subscription quota percentages, invoices and paid API charges.

## Explicit completion publication

After the workstream is complete, set `completed: true`, then run:

```sh
amplihack usage publish --manifest usage.json --pr 42
```

This is the only command that writes externally. It re-collects the current
inputs and uses the existing authenticated `gh` runner, with no credential-file
access. It verifies the PR's manifest branch, refuses drafts and closed unmerged
PRs, and creates or updates the authenticated publisher's stable marker comment.
The marker is derived from workstream identity and PR number, not usage contents;
rerunning after late reviews/retries updates the same comment. Identical reports
are a no-op. Other authors' matching markers and duplicate markers fail visibly.
Serialize publication per workstream/PR: GitHub's comment API has no atomic
create-if-absent operation. Pagination is supported. Oversized summaries fail
instead of dropping completeness information.

For shared work, list every PR in `prs`. Each comment explicitly presents the
same **shared non-additive total**; do not sum comments or imply a per-PR split.
The first implementation does not support numerical allocation weights.

Usage summaries are informational. They are never acceptance evidence, a
workflow completion ledger, or input to a merge gate. The authorized completion
workflow can invoke the explicit command after its own independent completion
checks; this feature does not rewrite recipe acceptance rules.

## Offline validation and remaining ownership

Unit tests in `commands/usage/tests.rs` use synthetic native fixtures and an
in-memory hosting runner. Run `cargo test -p amplihack-cli --lib commands::usage::`.
They make no model or network calls.

The usage collector maintainers own additional provider/schema adapters,
provider-exact invocation counts, reset-epoch attribution, numerical shared-PR
allocation and optional versioned price tables. These are follow-ups; partial
historical inputs are not represented as complete accounting. Workstream owners
remain responsible for reliable manifests and completion publication.
