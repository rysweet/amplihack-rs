# Workstream token usage

`amplihack usage report --manifest usage.json --format json` collects native Claude
Code and Codex JSONL accounting records without model calls or network access.
Omit `--format json` for a readable Markdown summary. Source files are opened
read-only. Only files explicitly listed in the manifest are read; relative paths
resolve against the manifest's directory. There is no automatic home-directory
scan, daemon, account switching, or pricing download. Reports do not limit
work; the [capacity governor](./capacity-governor.md) does that, and reads
a manifest only to show each workstream's token share. Usage
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
  the original owner. Claude Code subagent transcripts
  (`<session>/subagents/agent-*.jsonl`) mark their records `isSidechain: true`
  but carry the parent's `sessionId`, so their usage is direct usage of the
  parent session wherever the file is listed; list them in the parent's entry.
  A child entry holding only such a file owns nothing. A separate `claude`
  process has its own `sessionId` and is a child in its own right. Cache
  creation totals include 5-minute/1-hour partitions;
  the partitions are displayed without adding them again. An unpartitioned
  remainder has unknown duration. Claude reasoning remains inside output when
  the native record does not expose a separate count.
- Claude Code writes its own `assistant` records for API errors and interrupts:
  `message.model` is `<synthetic>`, API errors also carry `isApiErrorMessage:
  true`, `message.id` is a client UUID rather than a provider message ID, and
  every counter is an explicit zero. These records are not provider responses,
  so they are not observations: they add no response, no attempt, no tokens and
  no diagnostic. A failed or interrupted call therefore stays an unrecorded
  call, unknown rather than zero. A record with either marker that carries a
  nonzero or non-numeric counter is diagnosed `usage_on_client_generated_record`
  and makes its session incomplete. A session holding only such records reports
  the same as a session with no records.
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
  and `last_token_usage` alongside cumulative snapshots add nothing; the latter
  only confirms whether a transition is one request (see context per request).
  Standalone
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
  or positive cumulative-only Codex transitions. A transition that its record's
  `last_token_usage` does not confirm still counts here as one transition, while
  the session is diagnosed incomplete and its context attempts are unknown.
  Cumulative remainders have unknown attempts; zero observed attempts is not zero actual provider calls. Exact provider invocation counts,
  including unlogged failed calls, are unknown. Models and token buckets remain
  separate in JSON; `reasoning_in_output` makes Claude's unsplit reasoning explicit. Timestamp bounds cover observed usage, not wall-clock work.
- Unknown models are retained, and API list-price estimates and price references
  remain `null`. There is no hardcoded price table. Counts are observations,
  distinct from subscription quota percentages, invoices and paid API charges.
  Codex turn context applies only within its source file and native session;
  a copied response without applicable context keeps unknown model attribution.
  Conflicting model names for one response permanently place its deduplicated
  tokens under `unknown`, even if later copies repeat one of those names.

## Response counts and context per request

Report version 1 adds `identifier_backed_response_count` globally and on every
session. Count provider-scoped stable response IDs once, including records with
missing usage. Unidentified observations and cumulative-only transitions do not
contribute, and neither do Claude Code's client-generated API-error and interrupt
records (see the counting contract), whose IDs are not provider IDs. Conflicting native ownership counts once globally, remains unattributed,
and contributes to neither session's count. Direct counts can sum to less than the
global count. These fields differ from model `observed_attempts`.

Every session has a `context_per_request` object:

```json
{
  "input_tokens": 130,
  "observed_attempts": 1,
  "tokens_per_observed_attempt": 130.0,
  "unit": "tokens per observed attempt",
  "attempt_basis": "identifier_backed_responses",
  "unknown_reasons": []
}
```

Context is direct session input divided by compatible observed attempts, in
**tokens per observed attempt**. It is an observational estimate of context per
request, not a measurement of context-window occupancy. Claude input adds disjoint
new, cache-read and cache-write buckets after response deduplication. Codex input
already includes cache subsets and is counted once. Output, reasoning, duration
partitions and inclusive child totals are excluded. Claude Code subagent
requests are not excluded: they share the parent's `sessionId` (see the
counting contract), so once their transcripts are listed they count as direct
requests of the parent, and the parent's estimate mixes the subagents' own,
usually smaller, contexts with the main thread's. `isSidechain` is not used to
separate them.

Operands and the ratio are nullable; an explicit observed zero input remains
numeric with a compatible positive denominator. A zero or unknown denominator
never becomes one. Checked arithmetic refuses overflow. Unknown contributors
invalidate the ratio while known token lower bounds remain in the usage fields.
Unknown reasons are sorted and unique:

- `zero_observed_attempts`: known denominator is zero.
- `no_usage_observed`: no usage record was attributed to the session, as with
  an empty log, a log of client-generated records only, or records whose
  native session is not in the manifest. Input and attempts are null. No field
  was absent, so this is not `missing_input_fields`.
- `missing_input_fields`: a required input bucket is absent. Claude requires
  `input_tokens`, `cache_read_input_tokens` and `cache_creation_input_tokens`.
  Codex requires `input_tokens` and `cached_input_tokens`; Codex CLI
  `token_count` records omit `cache_write_input_tokens`, so an absent value
  counts as zero, and a present value is validated and joins the subset check.
- `invalid_input_fields`: an input bucket is not an unsigned integer.
- `overlapping_input_subsets`: cache subsets cannot be disjoint within input.
- `unknown_attempts_for_input`: retained input has no corresponding known
  attempts, as for a cumulative remainder beside identified responses or an
  unconfirmed cumulative-only transition.
- `unidentified_response_input`: a response-level usage record has no stable
  response identity, so its input can be neither deduplicated nor counted;
  `input_tokens` and `observed_attempts` are both null rather than a partial sum.
- `incompatible_attempt_basis`: response and cumulative observations disagree.
- `unsupported_provider`: validated provider label has no adapter.
- `unsupported_schema`: accounting semantics cannot be established.
- `ambiguous_session_ownership`: conflicting native owners prevent assignment.
- `arithmetic_overflow`: exact input or attempt accumulation exceeds `u64`.

The attempt basis is `identifier_backed_responses`, `cumulative_only_transitions`,
`unknown`, or `incompatible`. Corroborating cumulative snapshots add no attempts.
Beside identified responses, a missing cumulative prefix is the unknown
remainder described in the counting contract: its input is retained with
unknown attempts.

Without identified responses, each positive high-water transition of a native
session can supply one attempt. Codex CLI `event_msg/token_count` records carry
`last_token_usage`, the request that moved `total_token_usage`, and a
transition counts as one attempt only when its delta equals that value. When
the start of the thread is missing, or a resume file between two listed files
is missing, the first transition after the gap covers several requests, so its
delta is larger than the record's last usage. Whenever the two differ, the
input is retained, attempts are unknown with `unknown_attempts_for_input`, and
the session is diagnosed
`unconfirmed_cumulative_transition` and incomplete. A present
`last_token_usage` without usable counters cannot confirm a transition either.
`thread_token_usage` snapshots and records without `last_token_usage` carry no
such evidence: each positive transition counts as one attempt, and a missing
prefix there is not detected. Replayed snapshots add nothing. Missing,
malformed and incompatible observations are never silently dropped from
context validity.

Provider labels are validated, preserving spelling for unsupported providers.
Only exact `claude` and `codex` select adapters. An unsupported provider retains
its session, unknown usage and context, and incomplete coverage. Because its
tokens are unknown, `governor status` shows no `token_share` for a manifest that
names one, as it did when such a manifest was rejected. The manifest,
report and publication marker remain version 1; no manifest fields were added.
The canonical Markdown renderer used by both report and publish lists every
session's operands, count, estimate or specific unknown reasons under its own
"Session context per request" heading, separate from the direct / inclusive
token totals, so each heading carries one line per session. It retains
coverage, unattributed usage and non-additive evidence.

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
