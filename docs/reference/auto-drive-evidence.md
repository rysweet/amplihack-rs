---
title: Auto-drive Evidence and Outcomes
description: evidence validity, merge-ready interpretations, and separate readiness, action, and platform outcomes.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Evidence and Outcomes

This reference describes the implemented record contract. Controlled tests exercise validation and outcome reduction; they do not demonstrate live agent recovery or cross-platform execution.

Auto-drive agents collect observations and assess readiness. Rust helpers
validate records, preserve accepted snapshots, and reduce outcomes. Neither
a successful command nor a list of criterion names establishes readiness.

## Contents

- [Authority and criteria](#authority-and-criteria)
- [Record contract](#record-contract)
- [Validity and storage](#validity-and-storage)
- [Crusty review evidence](#crusty-review-evidence)
- [Approval and protection interpretation](#approval-and-protection-interpretation)
- [Recovery evidence](#recovery-evidence)
- [Result contract](#result-contract)
- [Validation coverage](#validation-coverage)

## Authority and criteria

The trusted [merge-ready skill](../../amplifier-bundle/skills/merge-ready/SKILL.md)
and its [description template](../../amplifier-bundle/skills/merge-ready/pr-description-template.md)
define the criteria. This reference preserves those definitions with
auto-drive's repository-type and crusty interpretations. The runner selects a private launch bundle for recipes, skills, templates, evaluator and helpers. The retired shell gate is replaced by agent finalization with deterministic local record validation.


For **auto-drive criterion 3**, accepted crusty convergence substitutes for
a separate quality audit, with **no minimum round count**. For **standalone
`/merge-ready`**, the separate quality audit still requires at least three
SEEK → VALIDATE → FIX cycles, continued until clean. The template's cycle
1–3 rows describe that standalone audit. In auto-drive, replace those rows
with actual accepted crusty-round evidence, including reviewed head, verdict,
accepted completion dispatch, and required follow-ups; do not claim unperformed cycles.

| ID | Criterion | Supporting evidence |
| --- | --- | --- |
| 1 | QA | Selected suite, source and directory, validated and executed scenarios where required, commands and results, tested head, and log references. Follow [QA selection](auto-drive-qa.md). Scenario files alone are insufficient. |
| 2 | Documentation | Changed external surfaces, corresponding documentation updates and discoverable links, or a supported explanation of why no update is needed. |
| 3 | Audit convergence | Auto-drive substitutes accepted completed crusty convergence: `CLEAN` or permitted `MINOR_ONLY`, required follow-ups, reviewed-head binding, and accepted completion dispatch (no evaluator call for a completed objective; see [Crusty completion](autodrive-crusty-completion.md)). No minimum round count applies. Standalone `/merge-ready` retains its separate audit of at least three SEEK → VALIDATE → FIX cycles, continued until clean. |
| 4 | Checks/build validation | Complete current-head check rollup, required checks, conclusions and policy interpretation. Pending, failing, cancelled, stale, unavailable, or zero reported checks do not pass. |
| 5 | Metadata | Read-back PR title, description, source and target, draft/reviewable state, and other required metadata. |
| 6 | Reviews/approvals | Current decision and approval/policy observations under the interpretation below; no outstanding requested changes. |
| 7 | Conflicts | Observed current mergeability and evidence that conflicts are absent. |
| 8 | Protection/policies | Current rules and platform enforcement state; every required rule satisfied, without bypass. |
| 9 | Linked work | Required issue/work-item links, or an explicit policy-supported non-applicability explanation. |
| 10 | Conversations | Complete paginated review/comment thread observations, including outdated unresolved threads, and required resolutions or permitted non-blocking acknowledgments. |
| 11 | Final verification | Intended outcome, refreshed facts and observed platform state. A verify-only run documents readiness with an open PR; a performed merge/close requires confirmation. |
| 12 | Description evidence | Read-back description containing concrete evidence for all other criteria, including scope. A local draft or successful update command is insufficient. |
| 13 | Scope | Complete changed-file and diff assessment showing no unrelated changes. An unreadable or truncated diff cannot establish scope. |

Non-applicability requires an explanation permitted by the authoritative
criterion. It cannot excuse missing tooling, access, approvals, policy, or
evidence. Criterion 1 is a recorded judgement, not an exemption: a
documentation-only PR is recorded as Verified with a `not_user_facing`
judgement that cites its files. See [Auto-drive QA](auto-drive-qa.md).

## Record contract

Versioned records describe outcomes and observations, independently of the
command or interface used. The logical contract below does not prescribe an
API client, gate executable, or serialized command protocol.

| Record | Required information |
| --- | --- |
| Invocation | Schema version, fresh run identity, captured action, strictly parsed `no_merge` and dry-run restrictions, trusted bundle/policy provenance. Only the runner grants authorization. |
| PR identity | Platform and host, target and source repository identities, PR number and URL, branches, verified head SHA, relevant base revision and merge base, linked worktree and observed platform state. |
| Criterion evidence | Run and round identity, criterion identity, verification status, concrete observations, evidence references, PR/head/base binding, policy provenance, blockers, and any permitted non-applicability explanation. |
| Failed attempt/recovery | Operation attempted, redacted cause and result, alternative chosen, observations recovered, completeness limits, and remaining blockers. |
| Readiness assessment | All 13 criterion records, accepted evidence references, reviewed and verified heads, readiness verdict, and blockers. |
| Finalization | Captured requested action, refreshed observations, expected head, request outcome, subsequent platform observations, and any ambiguous response or workflow violation. |
| Completion/report | Accepted round outcomes, completion dispatch or evaluator outcome, readiness, action, final platform state, stop reason, failures/recovery, follow-ups, and consumed budget. |

The closed value sets distinguish these meanings:

| Dimension | Values |
| --- | --- |
| Requested action | Verify readiness; merge. |
| Criterion verification | Verified; failed; pending; unreadable; not applicable with a permitted explanation. |
| Readiness | Merge-ready; not merge-ready; incomplete. |
| Action outcome | Not requested; pending; refused; failed; unconfirmed; merged; already merged; workflow violation. |
| Observed PR state | Open; closed without merge; merged; unknown. |
| Stop reason | Completed; blockers; no progress; budget exhausted; usage limit; orphaned; dependency failure; terminal refusal. |

Unknown observations remain unknown. A command failure and its recovery are
recorded even if later observations establish readiness.

Statuses and outcomes use closed enums. Unknown schema versions, duplicate
fields, unsupported statuses, missing/duplicate criteria, and contradictory
claims are rejected. Missing or unparseable verdicts never default to success.

## Validity and storage

Evidence is accepted only for the selected PR, run/round, head, relevant base,
and trusted policy. Missing, unreadable, stale, altered, mismatched,
unsupported, or contradictory records cannot establish readiness. Matching
the head alone does not refresh approvals, conversations, checks, or policy.

Private directories are created with mode `0700` and files with `0600`.
Storage validates ownership and permissions, rejects traversal, symlinks and
special files, bounds input sizes, and atomically snapshots the same opened
bytes that it validates and hashes. Evidence references stay within designated
storage; a record cannot cause arbitrary secret files or URLs to be read.
Collection limits and incomplete pagination produce incomplete evidence,
never silently truncated success.

Candidate staging is separate from accepted snapshots and loop-completion
records. Historical entries are validated, rejected, or quarantined; permission
repair and hashes alone cannot establish provenance. PR comments cannot seed
accepted state. Same-user snapshots provide workflow integrity, not an OS
security boundary.

Local Git observations reject inherited directory, index, object-store,
replacement-ref, and graft overrides. Arguments are passed as data, with
option terminators where supported. PR text, paths, refs, URLs, errors, test
output, and comments cannot alter authorization, criteria, QA selection,
budgets, or trusted instructions.

Published evidence redacts credentials, tokens, authentication headers,
cookies, and credential-bearing URLs. Evidence is reviewed before publication.
Recovery preserves TLS verification and uses configured authenticated access.

## Crusty review evidence

Keep the opt-in structured reviewer contract: `crusty_verdict` is `CLEAN`
or `CONCERNS`; concerns carry stable IDs, severity, summary, and evidence.
Standalone prose use of crusty is unchanged. Parsing selects the last JSON
object carrying the required verdict field and checks exact tokens.
Missing or unreadable verdicts default to `CONCERNS`.

Only the round may derive `MINOR_ONLY`. It requires exactly one non-empty
concern array, every element an object with severity exactly `minor`, every
ID matching `^[A-Za-z0-9._-]{1,64}$`, no previously major ID, and no security
finding under the existing ID/text keyword rules. Empty, unknown, downgraded
major, and security findings remain `CONCERNS`. Recurring valid minor findings
do not alone force another repair round.

Severity history includes earlier major findings. Security text matching uses
whole-word stems `secur` (including `insecure`), `authenticat`,
`authoris`/`authoriz`, `authn`, `authz`, `secret`, `token`,
`credential`, `inject`, `xss`, `csrf`, `cve`, `vuln`, `unsafe`,
`privilege`, `permission`, `sanitis`/`sanitiz`, `escape`, `traversal`,
`ssrf`, `rce`, and `password`. ID matching lowercases and treats `-`,
`_`, and `.` as spaces; it adds the whole-word forms `sec`, `auth`,
`perm`/`perms`, `cred`/`creds`, `priv`/`privs`, `privesc`, and
`xsrf`. These rules are shared with light mode.

Accepted review evidence includes reviewed head, stable findings, severity
history, resolved concerns, and follow-up issue identities. Each minor finding
needs one filed issue linked to the task; an issue task also needs its recorded
parent comment. Dry-run creates no follow-ups and cannot claim missing ones
exist. Previously resolved concerns need new evidence to be re-raised.

Substantive commits after review return to crusty before another readiness
round. The established exception permits clean base merges and changes limited
to `PR_DESCRIPTION.md`, `.github/pull_request_template.md`, and
`.autodrive/evidence/`. Inspect the full range; an unreadable range blocks.
Re-review runs at the original crusty-loop depth, not nested inside a round.

## Approval and protection interpretation

For GitHub, apply these rows top-down. Other platforms use their authoritative
policy equivalents; platform access must be proven rather than assumed.

| Review decision | Other facts | Approval status/source |
| --- | --- | --- |
| `APPROVED` | Any | `MET`, review decision. |
| `CHANGES_REQUESTED` or `REVIEW_REQUIRED` | Any | `NOT_MET`, review decision. |
| Empty | Readable required count `0`, no rule making it unknown | `MET`, required count. |
| Empty | Readable required count above `0` | `NOT_MET`, required count. |
| Empty | Count unknown; merge state `CLEAN`, `HAS_HOOKS`, or `UNSTABLE` | `MET`, merge state; count stays unknown. |
| Empty | Count unknown; `DIRTY`, `BEHIND`, conflicts, or `BLOCKED` with non-passing/no checks or unresolved threads | `PENDING`, other blockers. |
| Empty | Count unknown; `BLOCKED`, no agent-clearable blockers, named review rule | `NOT_MET`, review rule. |
| Empty | Count unknown; `UNKNOWN` | `UNREADABLE`, facts unreadable. |
| Empty | Remaining state, including `BLOCKED` without a named review rule or `DRAFT` | `UNREADABLE`, state unreadable. |
| Unsupported decision or failed observation | Any | `UNREADABLE`, facts unreadable. |

Use the higher readable count from classic protection and applicable rulesets;
a lone readable count settles the question only above zero. A nominal zero
with code-owner, last-push, or required-reviewer obligations is unknown.
A protection endpoint 404 never proves zero required reviews. A complete
authoritative merge-state fallback may still establish the relevant status;
an access failure with no equivalent observations remains a blocker.

For the fallback above, completed check conclusions/statuses `SUCCESS`,
`NEUTRAL`, and `SKIPPED` count as passed. An unfinished run does not.
Unreadable thread counts are not measured unresolved threads; outdated
unresolved threads count. `PENDING` means clear the other blockers and measure
again, not immediately ask for approval.

Criterion 6 passes only for `MET`. Preserve distinct blockers:
`changes-requested`, `required-approval-missing`,
`approval-pending-other-blockers`, `approval-facts-unreadable`, and
`approval-state-unreadable`. An agent cannot grant a human approval.

Criterion 8 uses the established `CLEAN`/`HAS_HOOKS`/`UNSTABLE` merge-state
interpretation where complete and authoritative. This does not waive criterion
4 or any known policy requirement. Unknown enforcement stays blocked.

## Recovery evidence

A GraphQL refusal can be recovered through another authenticated interface
only if it supplies equivalent complete facts, including every page of
conversations. A missing `gh` can be recovered through another available
client. Neither authorizes incomplete evidence or removes a failed criterion.
Required QA tooling remains required.

Record what failed, the cause, the alternative, and what it proved. If no
usable alternative exists, retain the named blocker and incomplete outcome.
An alternative merge interface must support expected-head protection.
Observe state before retrying ambiguous requests and after performing actions.

## Result contract

Readiness, requested action, request result, platform state, and stop reason
are separate. Human-readable and structured reports derive from accepted
records, not a final sentence of agent prose.

| Situation | Reported meaning |
| --- | --- |
| All criteria valid; merging disabled; PR observed open | Merge-ready, no merge requested, open. |
| Missing, stale, contradictory, or failed evidence | Not merge-ready or incomplete, with criterion blockers. |
| Required checks/approvals still pending | Pending work, not completed readiness. |
| Authorized request explicitly refused | Refused action with cause and observed state; not merged. |
| Request accepted or queued; merge not confirmed | Pending or unconfirmed action; not merged. |
| Platform confirms merge after authorized finalization | `merged`, with confirmation evidence. |
| Platform confirms PR was merged at entry | Already merged; this invocation did not perform the action. |
| PR merges unexpectedly before finalization | Observed merged state plus workflow violation; not successful authorized completion. |
| Budget, usage limit, orphan, dependency failure, or terminal refusal | Distinct incomplete stop reason; never substituted with success or generic `STUCK`. |

A `no_merge=true` or dry-run result records zero merge requests. Readiness
does not imply merging. Command success, queue admission, evaluator `DONE`,
and an old completion marker do not establish platform confirmation.

## Validation coverage

Record coverage in PR descriptions or CI logs, separately for:

- Controlled tests: merge success/refusal, GraphQL refusal, missing tools,
  ambiguous responses, complete alternatives, unavailable alternatives,
  incomplete pagination, required-tool failure, and moved heads.
- Actual agent execution against isolated fixtures: demonstrate that an
  agent adapts, or reports why recovery cannot satisfy a requirement.
- Live verify-only execution: after invocation-wide authorization is implemented
  and tested, readiness work against an open PR with `no_merge=true`, without
  a real merge. The current build-phase flag is insufficient protection.
- Compatibility: the [six-cell target matrix](auto-drive-to-merge.md#target-compatibility-matrix)
  for unchanged YAML across Linux/macOS and Claude/Codex/Copilot. Record actual
  execution evidence per cell; mark cells without it untested, or unavailable
  with a reason when the host/runtime cannot be used.

Stubs demonstrate controlled behavior, not live agent recovery or execution
on an unavailable host/runtime. Tests assert no merge requests under
`no_merge=true` or dry-run, and that interface switching cannot bypass failed
criteria or missing evidence. Loop-health and crusty contract regressions
remain required.

See [workflow configuration](auto-drive-to-merge.md) and
[verify-only usage](../howto/verify-pr-with-auto-drive.md).
