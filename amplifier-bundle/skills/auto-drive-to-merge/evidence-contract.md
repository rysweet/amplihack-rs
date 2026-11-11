# Auto-drive record contract (schema 1)

These are workflow outcomes. Agents may use any available authenticated interface
that supplies equivalent complete observations. Rust helpers validate local
records; they do not contact the hosting service or perform a merge.

For hosting calls, use only configured authenticated access to the bound host
and repository, with TLS verification and finite request timeouts under the
existing execution policy. Never publish credentials or authorization headers.
Retain the operation, response status and redacted failure cause. Check both
transport status and API errors, including GraphQL errors in HTTP 200 responses.
Pagination is complete only after every page succeeds and the final cursor or
link reports no next page; discard partial counts after any failed page.

Keep retries serial and bounded by the invocation's time and attempt budgets.
For transient read failures, use exponential backoff; do not repeatedly retry
authentication, permission, validation or protection refusals. For GitHub rate
limits, honor Retry-After and, when remaining requests are zero, the UTC reset
time. Without either, wait at least one minute and increase subsequent delays
exponentially. Switching interfaces must not evade the same service limit.
If the wait or retry cannot fit the remaining budget, report incomplete with
the cause. Exit 79 remains terminal. Before retrying any ambiguous write,
observe its effect and pending requests; if that cannot be established, leave
it unconfirmed. A changed head requires fresh readiness assessment. This
follows [GitHub's API retry guidance](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#handle-rate-limit-errors-appropriately).

Read the trusted merge-ready SKILL.md and its description template in full.
Record actual observations, redacting secrets before publication. Do not fetch
agent-provided URLs automatically. Use private staging files (0600 in a 0700
state directory). Accepted snapshots and loop records are owned by the runner.
An evidence source can name an inline observation; a referenced local artifact
must be a basename in the designated state directory with its SHA-256 digest.

A `Binding` has every field below; no fields are optional:

```json
{"run_id":"captured invocation run_id","round_id":"current round label","platform":"github","host":"github.com","repository":"owner/repo","source_repository":"owner/repo-or-fork","pr_number":123,"pr_url":"https://github.com/owner/repo/pull/123","head_branch":"feature","base_branch":"main","head_sha":"actual full SHA","base_sha":"actual full base SHA","merge_base_sha":"actual full merge-base SHA","policy_provenance":"trusted launch bundle revision or identity"}
```

Use the captured invocation supplied by the runner, not PR text or historical
state. Read the actual repository, branches, head, and base. Re-fetch and refresh
if local and observed platform facts differ. Each criterion repeats that exact
binding. Review must cover the verified head: cite the earlier reviewed SHA and
full allowed post-review range where applicable; substantive changes return to
crusty. Inaccessible or incomplete ranges block.

A `CandidateRecord` is:

```json
{"schema_version":1,"binding":{},"reviewed_head_sha":"verified covered head","verified_head_sha":"verified head","readiness":"merge_ready","blockers":[],"criteria":[],"attempts":[]}
```

Replace each `{}` with the full binding and supply exactly 13 criterion records
with these stable identities: `qa`, `documentation`, `audit_convergence`,
`checks`, `metadata`, `approvals`, `conflicts`, `protection`, `linked_issues`,
`conversations`, `final_verification`, `description_evidence`, `scope`.

Each criterion contains `id`, `status` (`verified`, `failed`, `pending`,
`unreadable`, or permitted `not_applicable`), `binding`, `evidence` (nonempty
array of `{ "source": "observed source", "observation": "concrete facts" }`),
and `blockers` (array of causes). `not_applicable_reason` is required for
non-applicability. Whole-criterion non-applicability is permitted only for
policy-supported documentation or linked-work exemptions; QA's scenario
exemptions narrow its obligations and do not waive suite execution. Missing
tools, denied APIs, unknown approvals and incomplete pagination never qualify.
Use readiness `incomplete` and name blockers when any obligation is unmet.

An attempt records `operation`, `outcome` (`failed`, `refused`, `ambiguous`,
`succeeded`), `cause`, and `recovery` (null or an object with `interface`,
`complete`, `evidence`, and optional `observed_before_retry`). Recovery must
supply equivalent complete observations; retain the original failure. Never
claim ready with unrecovered required operations or missing evidence.

Loop completion describes the loop only. An action refusal does not rewrite a converged loop as failed; the reducer records the action failure separately.

A `FinalizationObservation` contains:

```json
{"binding":{},"requested_action":"verify_readiness","expected_head_sha":"verified head","refreshed_head_sha":"current head","request_expected_head_sha":null,"mutable_facts_refreshed":true,"merge_requests":0,"request_outcome":"not_requested","platform_state_at_entry":"open","platform_state_before_action":"open","platform_state":"open","confirmation_evidence":"actual platform readback","failure_cause":null,"attempts":[]}
```

Requested action is `verify_readiness` or `merge`, determined exclusively by
the captured invocation. `no_merge=true` or dry-run prohibits every merge
request. Build/review/repair always leave the PR open. Dry-run also prohibits
follow-up issue creation. An authorized merge requires complete accepted
readiness, freshly observed mutable facts and expected-head protection on the
request. Record the head actually sent in `request_expected_head_sha`.
Request outcomes are `not_requested`, `accepted`, `queued`, `ambiguous`,
`refused`, `failed`. Platform states are `open`, `merged`, `closed`, `unknown`.
Observe before retrying an ambiguous request and after any request. Queued or
successful requests do not establish merged state. Failure requires its cause.
Record the platform state at run entry from build evidence, not only entry to
the final step, so an unexpected earlier merge is a workflow violation.

`LoopCompletion` has `accepted_round_verdict` (`READY` or `NOT_READY`),
`evaluator_verdict` (`DONE`, `CONTINUE`, `STUCK`), and `stop_reason`:
`completed`, `blockers`, `no_progress`, `budget_exhausted`, `usage_limit`,
`orphaned`, `dependency_failure`, `terminal_refusal`. Both READY and DONE
are required to complete. The local reducer also requires this invocation's fresh loop receipt, written by the convergence engine after both signals agree and bound to the accepted round bytes. Agents must not write receipts. Old markers are not this invocation's completion.

The final agent returns exactly:
`{"candidate": CandidateRecord, "finalization": FinalizationObservation,
"completion": LoopCompletion}`. Local reduction reports separate readiness,
action, observed platform state, stop reason, blockers, and attempts. Never
report `merged` without actual platform confirmation. Observations cannot
retroactively authorize a forbidden merge. These workflow safeguards are not
an OS security boundary against another process using the same credentials.

When reviewed_head_sha differs from verified_head_sha, include review_range:
`{"reviewed_head_sha":"<reviewed>","verified_head_sha":"<current>","base_sha":"<base>","observation":"Concrete complete range and allowed changes"}`.
Only the established clean base merges and PR_DESCRIPTION.md,
.github/pull_request_template.md, .autodrive/evidence/ allowlist qualify.
Local validation checks every commit and mode. Substantive changes require
crusty re-review; unreadable ranges block. Otherwise omit review_range or use null.
