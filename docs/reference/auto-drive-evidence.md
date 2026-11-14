---
title: Auto-drive Evidence and Outcomes
description: merge-ready criteria and interpretations, the PR write-up, and the outcomes an auto-drive run reports.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Evidence and Outcomes

Auto-drive agents collect observations, assess readiness and write the
assessment up in the PR for people. Nothing parses that write-up: the drive
reads only a one-word outcome, and a merge depends only on the captured
authorization, the assessed head and GitHub (#2575). Neither a successful
command nor a list of criterion names establishes readiness. Controlled tests
exercise the outcome word, the loop endings and the merge tool; they do not
demonstrate live agent recovery or cross-platform execution.

## Contents

- [Authority and criteria](#authority-and-criteria)
- [The write-up](#the-write-up)
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
auto-drive's repository-type and crusty interpretations. The runner selects a private launch bundle for recipes, skills, templates, evaluator and helpers. The retired shell gate and the retired parsed readiness record are replaced by an agentic assessment and GitHub's own merge conditions.


For **auto-drive criterion 3**, accepted crusty convergence substitutes for
a separate quality audit, with **no minimum round count**. For **standalone
`/merge-ready`**, the separate quality audit still requires at least three
SEEK → VALIDATE → FIX cycles, continued until clean. The template's cycle
1–3 rows describe that standalone audit. In auto-drive, the write-up replaces
those rows with actual accepted crusty-round evidence, including reviewed head, verdict,
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
evidence. The documentation-only exception narrows scenario obligations in
criterion 1; suite tests and all other criteria still apply.

## The write-up

Each merge-ready round posts one PR comment. It names the PR and the full
head SHA assessed, gives the observations and sources for each of the 13
criteria, names every blocker, and ends with one sentence stating the outcome:
ready; not ready, with the blockers named; or hand-off, when only a person can
clear the blocker. The bundled
[evidence contract](../../amplifier-bundle/skills/auto-drive-to-merge/evidence-contract.md)
gives the agent's guidance.

The write-up is for people. Its shape does not affect the drive: unknown
keys, a notice printed before a JSON block, `null` fields or plain prose all
lead to the same next step. A reader agent turns the stated outcome into one
of `ready`, `not-ready` or `hand-off`; any other answer counts as `not-ready`.
See [outcome word](autodrive-merge-ready-round.md#outcome-word).

Unknown observations stay unknown, and a criterion that cannot be established
is a blocker. A command failure and its recovery are reported in the write-up
even if later observations establish readiness.

## Validity and storage

An assessment applies only to the selected PR and the head it names. The
round captures that head before the assessment starts and writes it to the
`.head` round file, so a commit made during or after the assessment is never
recorded as assessed. A merge
happens only at the head the ready round assessed: the merge step compares it
with the PR worktree's head and passes it to `--match-head-commit`, so a later
push refuses the merge. Matching the head alone does not refresh approvals,
conversations or policy; GitHub enforces those when it accepts the merge.

Private directories are created with mode `0700` and files with `0600`,
whatever the umask; see [Auto-drive State Permissions](./auto-drive-state-permissions.md)
for when the check repairs an entry this user owns and when it refuses.
Round files are written to a temporary file in the same directory and moved
into place, never through an existing path; round labels are checked and `.`
and `..` are refused. Write-ups are cut at 256 KiB and never passed on a
command line. Collection limits and incomplete pagination produce an
incomplete assessment, never silently truncated success. PR comments,
including earlier write-ups, cannot grant authority. Same-user state provides
workflow integrity, not an OS security boundary.

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

Requested action, loop result, merge result and stop reason are separate.
The structured report takes the merge result and loop result only from
tool-written JSON; the finalization agent's statement is kept verbatim as the
summary and decides nothing. The report's fields are listed in
[run report](autodrive-merge-ready-round.md#run-report).

| Situation | Reported meaning |
| --- | --- |
| Ready round and evaluator `DONE`; merging disabled | Merge `not-authorised`, stop reason `completed`; PR stays open. |
| Not ready; next round repairs the named blockers | No report yet; the loop continues. |
| Required checks failing or pending at finalization | Merge `checks-not-green`, no merge request; PR handed off. |
| Head moved after assessment, or GitHub refuses the merge | Merge `refused`; PR handed off. |
| GitHub reads back `MERGED` after the request | Merge `merged`, stop reason `completed`. |
| PR was merged at entry | Already merged; this invocation did not perform the action. |
| Hand-off round, evaluator `STUCK`, no progress, budget exhausted, unreadable loop result | Stop reason `handoff`, `no_progress`, `budget_exhausted` or `unreadable`; PR drafted with a comment naming the blocker. |
| Usage limit, orphan or terminal refusal | Distinct stop reason; never substituted with success, hand-off or generic `STUCK`. |

A `no_merge=true` or dry-run result makes zero merge requests. Readiness
does not imply merging. Command success and evaluator `DONE` do not establish
platform confirmation; only GitHub's read-back of `MERGED` does.

## Validation coverage

Record coverage in PR descriptions or CI logs, separately for:

- Controlled tests: write-ups of any shape (unknown keys, a notice before the
  JSON, `null` fields) still continue or merge; unclear outcome words lead to
  another round; merge success; refusal on a moved head or failing checks;
  fallback when no checks are required; exit `79` and usage-limit pass-through;
  merge refused without captured authorization.
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
`no_merge=true` or dry-run, and that no agent output can cause a merge. Loop-health and crusty contract regressions
remain required.

See [workflow configuration](auto-drive-to-merge.md) and
[verify-only usage](../howto/verify-pr-with-auto-drive.md).
