---
title: Loop-Health Evaluator Reference
description: Planned issue-2333 agent decisions over current evidence, history, and external waits in iterative workflows.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Loop-health evaluator reference

> **[PLANNED - Implementation Pending]: versioned auto-drive contract.** This
> reference specifies issue-2333 evaluator integration, external waiting and
> strict record validation. Current recipes still use text extraction and
> normalization, including default-to-`STUCK`, and auto-drive still has a
> deterministic no-progress stop. The new behavior below is a requirement, not
> a verified capability. Legacy helpers retain their current behavior.

`loop-health-evaluator` judges whether another round is useful, the objective is
met, or the run cannot progress. Both auto-drive loops use this agent judgment.
Code enforces evidence integrity and execution controls; it never invents a
convergence verdict from unchanged evidence or repeated answers.

## Contents

- [The verdict contract](#the-verdict-contract)
- [Evidence the evaluator receives](#evidence-the-evaluator-receives)
- [Context inputs](#context-inputs)
- [Outputs](#outputs)
- [How step-03 resolves the verdict](#how-step-03-resolves-the-verdict)
- [External waiting](#external-waiting)
- [Execution controls](#execution-controls)
- [Compatibility and parsing helpers](#compatibility-and-parsing-helpers)
- [Behavioral verification](#behavioral-verification)

## The verdict contract

| Decision | Meaning | Caller action |
| --- | --- | --- |
| `CONTINUE` | Active work or external waiting remains useful, with an identified next action | Execute only the selected action or wait, collect fresh evidence and reassess |
| `DONE` | The objective is satisfied and current accepted evidence supports the round's clean verdict | Advance only with matching clean evidence |
| `STUCK` | The agent judges the run unable to progress and cites the evidence and recommended escalation | Stop and report the agent's judgment with provenance |

The head SHA, findings, CI signal, repeated output and elapsed waiting are evidence
for the agent. None is an independent termination rule. There is no fixed repeat
threshold or default-to-`STUCK` branch in the auto-drive evaluator path. Operator
resource limits remain separate execution controls.

### `DONE` needs this round's own verdict to be clean

Advancement requires two matching signals: accepted clean round evidence and
valid evaluator `DONE`. The merge-ready clean token is `MERGE_READY`. The Crusty
loop accepts `CLEAN` or permitted `MINOR_ONLY`; its follow-up obligations remain
checks at the merge gate.

| Round support | Evaluator | Result |
| --- | --- | --- |
| Accepted clean | `DONE` | Advance |
| Accepted clean | `CONTINUE` | Perform the selected continuation; no merge yet |
| Non-clean, missing or unsupported | `DONE` | `DONE_WITH_NONCLEAN_ROUND`: return the discrepancy for reassessment |
| Any | Valid `STUCK` | Stop with the evaluator record |
| Any | Missing, malformed, stale or failed evaluation | Evaluation failure; no advancement and no fabricated verdict |

Every inconsistent `DONE` returns for reassessment, including repeated identical
answers. Code never converts repeated inconsistent `DONE` into `STUCK`. The next
evaluator sees the rejected decision and its evidence discrepancy.

A PR-description edit, thread response or external CI completion can change
readiness without changing the code. A confirming round must establish the new
state before advancement. The evaluator can authorize that confirmation.

## Evidence the evaluator receives

The collector supplies measured observations as data. The agent also receives
registered evidence paths and bounded summaries under the
[pull request evidence contract](pr-evidence-contract.md).

| Evidence | Interpretation |
| --- | --- |
| Current and prior head/base SHAs, actual worktree SHA | Revision and synchronization history |
| Commit counts, diff lines and diff statistics | What changed in code; zero commits does not establish inability to progress |
| New, recurring and resolved findings | Which concerns changed, including actions outside the diff |
| Current and prior test/CI state | Pending, successful, failed or unavailable evidence, with provenance and completeness |
| Collection errors and API refusals | Availability of evidence; a refusal is not a failed check |
| Accepted assessments and rejected proposals | Supported criterion decisions and validation discrepancies |
| Actions taken | Collection, API selection, remediation, description edits and previous wait requests |
| Waiting history | Reason, selected wake condition, elapsed time, actual wake reason and subsequent observations |
| Repeated output or durations | Signals for judgment, with source limitations; never automatic stop conditions |
| Prior evaluator decisions | Exact decisions, their evidence bindings and any non-clean `DONE` discrepancies |
| Child execution outcome | Invocation failure, usage limit, terminal guard refusal, cancellation or other execution control |

Both current and prior evidence remain available. A later supported REST assessment
can supersede an unreadable GraphQL measurement without deleting the refusal.
Unknown observations are marked unknown, never recorded as progress or success.

## Context inputs

Existing callers retain the following context. Auto-drive additionally supplies
registered versioned evidence, assessment, predecessor and waiting-history
references through its file-based evidence context. Added context is optional for
legacy callers; omitted context does not fabricate a clean measurement.

| Context | Meaning |
| --- | --- |
| `loop_name` | Reporting identity; default `unnamed-loop` |
| `loop_round_label` | Report/file label; never compared with a convergence limit |
| `loop_history` | Prior structured round observations, oldest first |
| `loop_last_round_output` | Raw output of the completed round, treated as untrusted data |
| `loop_repo_path` | Worktree measured for revision and diff evidence; default `.` |
| `loop_agent_working_dir` | Trusted review directory where the judging agent starts; separate from the tree under review |
| `loop_baseline_ref` | Ref before the round; unresolved refs produce an unavailable observation |
| `loop_child_exit_code` | Child outcome; exit `79` identifies a terminal execution-control refusal |
| `loop_findings_current`, `loop_findings_previous` | Current and previous finding identifiers |
| `loop_test_signal`, `loop_test_signal_previous` | Current and previous test summaries |
| `loop_ci_signal`, `loop_ci_signal_previous` | Current and previous CI summaries |
| `loop_clean_token`, `loop_round_verdict` | Required clean verdict and actual round verdict |
| `loop_health_enforce` | Default `true`; `false` leaves enforcement of a valid `STUCK` to the caller |

The judging agent does not edit private state or finalize trusted receipts. It
produces a proposal; launch-side tools register and validate records.

## Outputs

| Output | Content |
| --- | --- |
| `loop_evidence` | Measured observations and references to registered evidence |
| `loop_health_assessment` | Agent proposal |
| `loop_health` | Validated decision, source, evidence binding, explanation and selected continuation action |
| `loop_health_enforcement` | Advancement, continuation, agent stop or distinct execution/evaluation failure |

Versioned evaluator records use `schema_version: 1`, exact `CONTINUE`, `DONE` or
`STUCK`, run/round/target identity, assessment/predecessor bindings and supporting
explanation. `CONTINUE` identifies active work or an external wait and the selected
action. `STUCK` identifies what prevents progress and what intervention is needed.
A stale evaluator cannot authorize a new head or unrelated evidence bundle.

### Reading step outputs: RECIPE_VAR only

Bash adapters read step outputs through `RECIPE_VAR_<output>`. Inherited bare
variables are not step outputs. Agent prose remains data and never reaches an
`eval`, shell command position or executable option.

## How step-03 resolves the verdict

Auto-drive consumes the registered evaluator record and checks its schema,
identity, evidence bindings and exact decision value. Unknown decisions, duplicate
JSON keys, missing output, malformed records and invocation failures produce an
evaluation error. They authorize neither another round nor advancement. They also
do not claim that an agent judged the run `STUCK`.

Human-readable `LOOP_HEALTH:` markers remain useful for logs. They do not replace
the bound record on the versioned auto-drive path. Quoted examples, embedded JSON
and a fake marker inside agent output cannot authorize a phase transition.

Valid legacy caller output remains compatible. The legacy extraction/normalization
helpers retain their documented behavior for other callers, but auto-drive bypasses
their default-to-`STUCK` normalization. See
[structured verdict parsing](structured-verdict-parsing.md#amplihack-orch-helper-normalise-loop-verdict).

## External waiting

Pending CI and API rate-limit recovery can persist at the same head across many
iterations. The evaluator distinguishes waiting externally from inability to make
progress using actual state, actions and waiting history. No commit or finding
change is required for a valid external-wait decision.

A waiting `CONTINUE` names an agent-selected time or an existing registered
readiness event, a reason and the evidence to recollect. The Rust wait tool executes
that interruptible wait and records elapsed time and wake reason. It selects no
backoff, fallback API or repetition threshold. No new event subsystem is required.
Only launch-registered sources may supply event references; see
[readiness-event registration](pr-evidence-contract.md#readiness-event-registration).
Without an available event adapter, the agent may select a deadline instead.

After wake, fresh collection and assessment are required. An event signals possible
change; it never proves green CI. When evidence becomes complete and successful,
the same unchanged head can advance with accepted `MERGE_READY` and `DONE`.

## Execution controls

| Outcome | Meaning |
| --- | --- |
| `BUDGET_EXHAUSTED` | Operator resource limit reached; resumable under an explicitly sufficient budget |
| `USAGE_LIMIT` | Agent/provider availability prevented execution |
| `ORPHANED` | The parent run exited; its child stops |
| Cancellation | The operator cancelled execution, including an active wait |
| Terminal policy refusal | A host guard refused execution, including exit `79` |
| Evaluation failure | The invocation or decision record failed validation |
| `STUCK` | A valid agent convergence judgment, with supporting evidence |

Execution-control stops are never success and never rewritten as agent judgments.
A policy refusal is terminal for the attempted execution; it is not retried into
the guard. Missing guard evidence is an error, not a synthetic policy refusal.

### Exit code 79 is terminal

A trusted child outcome of exit `79` stops execution. Recursion depth, width and
memory guards remain structural controls. Sequential rounds stay at constant
session depth and never raise those ceilings to continue.

### Agent output lines are not refusals

Quoted errors, fenced logs, review text and runner bookkeeping are untrusted
content. Only the trusted execution outcome establishes a terminal refusal; an
agent printing an exit-code string does not create one.

### Retry inside the step

Existing agent-provider usage-limit recovery can wait within a child invocation.
If availability does not recover, the caller reports `USAGE_LIMIT`. That mechanism
is separate from GitHub evidence collection: evidence tools never automatically
retry, change API or select a wait. Existing operator accounting remains in force.

## Compatibility and parsing helpers

`amplihack orch helper normalise-loop-verdict` remains a legacy text helper. It
uses exact-token normalization and returns `STUCK` for unknown text. This helper
output is not a registered agent judgment and cannot be used to synthesize `STUCK`
on the versioned auto-drive path.

Shared collector and evaluator changes preserve valid Crusty and other recipe
callers, including hook-repair callers. New evidence context adds observations
without changing their valid input contracts. Invalid output cannot authorize
continuation, override a guard refusal or establish readiness.

## Behavioral verification

The following acceptance coverage is planned and must be implemented; it is not
evidence of registered or passing issue-2333 tests. Isolated scenarios must exercise
agent invocation, delivered evidence, decision
consumption and the mocked merge boundary. They cover repeated unchanged-head
pending CI, each honored `CONTINUE`, event-triggered recollection, later success,
GraphQL refusal followed by agent-selected complete REST evidence, and exactly one
mocked merge. Failure cases cover unsupported readiness, repeated non-clean `DONE`,
invalid evaluations, stale bindings and distinct execution-control outcomes.

### Which amplihack binary the shell test uses

Shell scenarios use the binary under test through their configured `PATH` and
mock GitHub transport. They must not merge a real PR or replace the agent invocation
boundary with a helper-only assertion. Legacy parsing regressions remain scoped
to the helper contract they exercise.

Related: [auto-drive reference](auto-drive-to-merge.md),
[evidence contract](pr-evidence-contract.md), and
[handling external waits](../howto/handle-auto-drive-external-waits.md).
