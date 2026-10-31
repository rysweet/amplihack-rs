# Workstream token usage

`amplihack usage report --manifest usage.json --format json` collects native Claude
Code and Codex JSONL accounting records without model calls or network access.
Omit `--format json` for a readable Markdown summary. Source files are opened
read-only. Only files explicitly listed in the manifest are read; relative paths
resolve against the manifest's directory. There is no automatic home-directory
scan, quota governor, daemon, account switching, or pricing download. Usage
commands also skip generic startup asset repair; reporting needs no installed
framework assets or external tools.

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
never proof of zero consumption. Unknown native session IDs are diagnosed with `unmapped_native_session`.
Reliably deduplicated tokens remain in the unattributed subset; ownership is
never assigned to another manifest session. Records without reliable deduplication
identity remain in `unresolved` as non-additive evidence, excluded from totals.
Conflicting native owners for the same response leave that response counted once
in the unattributed bucket, regardless of manifest order. Parse diagnostics retain
the source session/file/line; `incomplete_usage_observation` marks the accounting
owner separately, and incompleteness propagates through its ancestors.
Copied records with null or absent usage retain their native owner and response
metadata so missing observations also mark that owner and its ancestors incomplete.
No valid observed token counters (including empty usage objects) means `total`,
session `direct` and `inclusive` are `null`. An explicitly observed zero counter
produces numeric zero; absent or invalid counters never establish zero. Partial
numeric observations are lower bounds, not actual consumption claims.

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
- Current Codex `token_usage_record` records deduplicate globally by
  `payload.response_id`, using response `usage` and native `thread_id` (or
  `session_id`) for ownership. `thread_token_usage` and `event_msg/token_count`
  cumulative snapshots are coverage evidence, never added to those responses.
  A cumulative prefix missing from identified responses is retained once as an
  explicitly unknown-model remainder, with unknown attempts and incompleteness.
  `turn_token_usage` is not added to either response or thread totals.
  Missing or invalid `session_meta.payload.id` emits
  `missing_or_invalid_session_identity` and clears both session and model context.
  Later records use only their own native identity until valid session metadata
  returns. Responses with stable IDs and unknown owners remain unattributed;
  cumulative snapshots without native identity remain non-additive `unresolved`
  evidence. Restoring session metadata does not restore the previous model context.
- Cumulative-only Codex counters are ordered across all explicitly supplied
  resume files. Per-native-session high-water deltas are taken before subtracting
  cache-read/cache-write and reasoning subsets, and before assigning a delta to
  its model. Model changes do not reset the baseline. Repeated total snapshots
  and `last_token_usage` alongside cumulative snapshots add nothing. Standalone
  last-only usage needs a stable request/response ID; otherwise known fields
  remain non-additive unresolved evidence. Counter regressions remain incomplete
  lower bounds; no reset epoch is guessed. Delayed cache/reasoning subsets across
  model changes collapse to unknown model attribution while preserving token
  conservation; incompatible cumulative remainders stay non-additive evidence. Cache writes are retained with unknown
  duration when native records do not supply a duration. Older snapshots without
  a cache-write counter retain an explicit `cache_write_counter_unreported`
  diagnostic. Codex input includes
  cached/read/write subsets, so these are subtracted from new input; reasoning is
  subtracted from output. Unknown subsets stay diagnosed.
- Direct session totals count owned observations only. Inclusive totals include
  descendants; they overlap and must not be summed. The workstream total sums
  direct totals once. Stream/copy deduplication is global for provider IDs;
  cumulative counters are scoped to native session identity. Copied histories
  that erase original IDs cannot be reliably reconstructed.
- `observed_attempts` means unique provider responses/messages where available,
  or positive cumulative-only Codex transitions. Cumulative remainders have unknown
  attempts; zero observed attempts is not zero actual provider calls. Exact provider invocation counts,
  including unlogged failed calls, are unknown. Models and token buckets remain
  separate in JSON; `reasoning_in_output` makes Claude's unsplit reasoning explicit. Timestamp bounds cover observed usage, not wall-clock work.
- Unknown models are retained, and API list-price estimates and price references
  remain `null`. There is no hardcoded price table. Counts are observations,
  distinct from subscription quota percentages, invoices and paid API charges.
  Codex turn context applies only within its source file and native session;
  a copied response without applicable context keeps unknown model attribution.
  Conflicting model names for one response permanently place its deduplicated
  tokens under `unknown`, even if later copies repeat one of those names.

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
The actual binary regression in `bins/amplihack/tests/issue_1810_usage_startup.rs`
checks JSON and summary reporting against empty and stale installations, with
no external tools on PATH and unchanged input/home state. Run it with
`cargo test -p amplihack --test amplihack_bin issue_1810_usage_startup::`.
Tests make no model or network calls. Fixtures include native stable response
records, cumulative-only fallbacks, cache writes, missing prefixes and unknown
ownership, and distinguish unobserved/null from non-additive partial evidence.

The usage collector maintainers own additional provider/schema adapters,
provider-exact invocation counts, reset-epoch attribution, numerical shared-PR
allocation and optional versioned price tables. These are follow-ups; partial
historical inputs are not represented as complete accounting. Workstream owners
remain responsible for reliable manifests and completion publication.
