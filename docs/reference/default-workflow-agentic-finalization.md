# Default Workflow Agentic Finalization

> [Home](../index.md) > Reference > Default Workflow Agentic Finalization

`default-workflow` finalization is a fail-closed terminal assessment for
development workstreams. It is modeled as a sequence of typed recipe steps.
Deterministic steps collect evidence, emit typed recipe state, validate the
terminal decision, and choose the process exit code. An agentic finalizer step
contributes a human-readable narrative only.

Since #2343 the first evidence is the run's real outcome: the branch pushed at
the local head, the PR open at that head, and its check state. A missing context
file, an unset empty object key or pending CI is not a failure. See
[The Run's Real Outcome](#the-runs-real-outcome-2343). Where this
page describes fail-closed handling of incomplete bookkeeping, that section
takes precedence.

**The terminal state is decided by one step, `assess-finalization`, on the
facts of what the run produced.** Since #2809 it is not a deterministic
function of typed evidence: bash collects the facts and reports the decision,
and it computes neither. What stays deterministic is everything around the
decision -- which facts are collected, which blockers precede them, which
tokens may be published, and how the result is built.

Finalization never parses agent-generated prose with `jq`, regex, fence
stripping, or JSON extraction to decide the outcome. `assess-finalization`
returns one typed JSON object through the recipe's own context transport; the
narrative the *finalizer* agent produces is a separate artifact for humans and
is never read. A fully successful run cannot be reported as failed merely
because the finalizer emitted human-readable text instead of parser-compatible
JSON.

This page is the target implementation contract for the feature. If recipe,
helper, or test behavior differs, update the implementation to match this
contract rather than reintroducing a brittle parser that translates
unconstrained agent prose into workflow control flow.

The finalizer does not mutate Git state, create or edit pull requests, merge, or
decide success from free-form prose. Terminal success is reported only for a
state `assess-finalization` named and the published vocabulary calls a success,
and only after the deterministic blockers below have been answered.

## Design Rule: No Prose Parsing

This feature exists because a prior finalization model asked the finalizer agent
to serialize exactly one JSON object, then parsed that blob to drive control
flow. Run `f1968919-2808-4e80-8272-615ae77388eb` reproduced the failure: every
durable step (`finalize-terminal-state`, the `step-20*`/`step-21`/`step-22`
gates, `collect-finalization-evidence`, and `agentic-finalizer`) completed, yet
`validate-agentic-finalization` exited `1` with
`jq: parse error: Invalid numeric literal at line 1, column 4` because the
finalizer emitted prose instead of a single JSON object.

The finished feature removes that boundary entirely:

- Deterministic steps own every typed value that crosses a step boundary.
- Agent output is treated as untrusted, opaque narrative. It is captured as an
  artifact and never evaluated, interpolated, or parsed for control flow.
- Control flow derives from typed recipe state (`RECIPE_VAR_*`) and step exit
  status, never from re-reading an agent's stdout.

Making the parser more tolerant is explicitly rejected. There is no schema
retry, fence stripping, regex fallback, or prompt-tightening that turns
unconstrained prose into a decision.

## When Finalization Runs

Agentic finalization runs at the end of `default-workflow` and any
`smart-orchestrator` development workstream routed through `default-workflow`.
It also runs when publish or PR handling short-circuits into an already-terminal
state such as merged, obsolete, no-diff, or blocked CI.

Use it through the normal workflow entry point:

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack recipe run smart-orchestrator \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  --format json > recipe-result.json
```

Standalone `default-workflow` runs use the same finalization contract:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  -c branch_name="$(git branch --show-current)" \
  --format json > workflow-result.json
```

Maintainers may invoke the finalization path directly for diagnosis when the
implementation, verification, publish, and PR context are already known:

```bash
amplihack recipe run workflow-finalize \
  -c repo_path=. \
  -c branch_name="$(git branch --show-current)" \
  -c base_ref=origin/main \
  -c pr_number=123 \
  --format json > finalize-result.json
```

## Finalization Pipeline

Finalization is a recipe of explicit steps. Each step has one responsibility,
typed inputs, and typed outputs. Control flow moves through recipe state and
step status, not through an agent's stdout.

| Step | Type | Responsibility | Emits |
| --- | --- | --- | --- |
| `collect-finalization-evidence` | Deterministic shell/JSON | Read Git status, branch/base diff, provider-safe change-request metadata, CI state, implementation and verification markers, publish result, prior recipe state, missing tooling, observed phases, and the facts of what the run produced (`outcome`). | `finalization_evidence` (typed JSON, `schema_version: 1`) |
| `agentic-finalizer` | Agentic | Produce a human-readable narrative explaining the run and the recommended next action for operators. | `agentic_finalizer_narrative` (opaque text artifact) |
| `finalizer-step-status` | Deterministic shell/JSON | Record whether the narrative step itself completed or failed, as typed state. | `finalizer_step_status` (`{status, reporting_failure}`) |
| `assess-finalization` | Agentic (sub-recipe) | Decide the run's terminal state from the collected facts, and close a gap with one of two additive actions when that is what is missing (#2809). | `finalization_assessment` (`{terminal_state, action_taken, pr_number, reason, next_action}`) |
| `validate-agentic-finalization` | Deterministic shell/JSON | Apply the blockers about the local checkout, confirm the facts were gathered, and report the terminal state the assessment decided. | `workflow_result` (normalized terminal result) |
| `workflow-complete` | Deterministic shell/JSON | Emit the human-readable summary and canonical result JSON, including the reporting-vs-implementation distinction. | `workflow_completion` |

`agentic-finalizer` is deliberately boxed out of control flow. Its narrative is
never scanned for tokens such as `terminal_state: MERGED`, and no step reads it.

`assess-finalization` is the opposite: it is in control flow, and that is the
point (#2809). It decides the terminal state, which the validator then reports.
What it cannot do is reach past the blockers the validator takes first -- a
dirty worktree, missing `git` or `jq`, a deterministic probe that already named
a blocker -- or supply the identity of a pull request: `pr_url` is rebuilt from
the remote the facts report, and `pr_number` is used only when it is a pull
request number. It may push the run branch and open a pull request, and nothing
else. See [The assessment decides](#the-assessment-decides).

### Typed State Across Step Boundaries

When a typed value must cross a step boundary, a **deterministic** step owns the
type. Deterministic bash steps declare `output: <name>` with `parse_json: true`
and print a single JSON object. The recipe runner exposes each field to later
steps as `RECIPE_VAR_<output>__<field>`. Later steps read those typed variables
and step exit status; they never re-read agent prose.

The `agentic-finalizer` step is the one step that does **not** declare
`parse_json`. Its stdout is captured verbatim as `agentic_finalizer_narrative`
and used only for human-facing reporting.

## Observed Failure Modes This Model Addresses

Recent workflow logs, tests, and history showed repeated brittle finalization
failures. The finished model is designed around these concrete cases:

| Failure mode | Observed cause | Required finalization behavior |
| --- | --- | --- |
| Prose parsed as control flow | The finalizer's stdout was parsed as one exact JSON object; human-readable text produced `FAILED_FINALIZER_OUTPUT` even after a fully successful run. | Classification is derived only from typed `finalization_evidence` and `RECIPE_VAR_*` markers. Narrative text is never parsed. |
| Reporting failure hid a successful implementation | A failure in the finalization/reporting step turned completed implementation into an undifferentiated recipe failure. | Reporting failures classify as `FAILED_REPORTING` and preserve durable evidence (`pr_url`, `pr_number`, `implementation_completed`, `verification_completed`). |
| Missing or stale change-request metadata | A stale provider ID, missing change-request URL, mismatched head branch, stale head SHA, or unavailable provider metadata caused the wrong state to be trusted. | Validate change-request identity against repo, branch, base, provider, and head SHA. If the needed metadata is unavailable or mismatched, fail closed. |
| Dirty worktree misclassification | Generated files, unstaged edits, or leftover workflow artifacts were treated as harmless no-diff states. | Dirty worktree evidence blocks terminal success unless the workflow explicitly commits, removes, or accounts for the changes before finalization. |
| Closed-unmerged PR handling | Closed PRs without merge evidence were sometimes treated like completed work even when branch diffs remained. | Return `CLOSED_OBSOLETE` only when local no-diff/obsolete proof exists; otherwise return a failing closed-unmerged state. |
| Remaining meaningful diff | Branch changes remain but no valid publish, merge, follow-up, or implementation-plus-verification path proves closure. | Return `FAILED_MEANINGFUL_DIFF` unless the diff is intentionally represented by a validated PR/follow-up or another success state with deterministic proof. |
| Missing tooling | `gh`, `jq`, Git metadata, or provider auth was absent on paths that required it. | Tooling absence is reported as a deterministic failure such as `FAILED_MISSING_TOOLING` or `FAILED_PR_METADATA_UNAVAILABLE`, not as no-op success. |
| Failed CI | Open PRs with failed or unavailable required checks reached final output that looked complete. | Return `BLOCKED_CI` with failing check evidence and `terminal_success=false`. |
| Hollow success | A recipe exited `0` after setup, planning, empty agent output, or inaccessible codebase messages without implementation, verification, publish, or valid no-op evidence. | The signal is a typed fact beside the published outcome; `HOLLOW_SUCCESS` when nothing published contradicts it (#2809). |
| Finished runs failed on bookkeeping | A run whose branch was pushed and whose pull request was open ended `FAILED_IMPLEMENTATION` because its context file was unreadable and its checks were pending (#2343, the #2784 run). | The terminal state is decided on what the run published, not on whether its own record of itself is complete. |
| Implementation failure | Implementation or verification evidence is absent or failed while meaningful work remains. | Return `FAILED_IMPLEMENTATION`, distinct from a reporting failure. |

## Input Evidence Document

`collect-finalization-evidence` assembles one normalized evidence document, and
`assess-finalization` decides the terminal state on it.
`validate-agentic-finalization` reads five fields of it for the blockers no
published outcome can answer, and reports that decision. Producers collect the
data from recipe context, step outputs, shell helpers, Git, and provider
metadata, but each reader sees a single JSON object.

| Field | Type | Description |
| --- | --- | --- |
| `schema_version` | integer | Evidence schema version. Current value is `1`. |
| `recipe_name` | string | Usually `default-workflow` or `workflow-finalize`. |
| `workflow_classification` | string | Classification such as `Development`, `Default`, `Feature`, `Bugfix`, or `Refactor`. |
| `repo_path` | string | Repository root or workflow worktree used for local evidence. |
| `branch_name` | string | Expected branch for the workflow-owned work. |
| `base_ref` | string | Intended comparison base. |
| `git` | object | Clean/dirty status, branch match, diff status, commits ahead, and base resolution details. |
| `change_request` | object | Provider, URL, ID, state, merge evidence, source branch, base branch, head SHA, and identity-match booleans. Empty when no change request exists. |
| `ci` | object | Required check state, failing checks, pending checks, and CI metadata availability. |
| `implementation` | object | Whether code/docs/config work was applied and where that evidence came from. |
| `verification` | object | Whether required tests, pre-commit, or validation completed and summaries of those checks. |
| `publish` | object | Publish result, follow-up PR details, no-diff state, or provider-specific skip reason. |
| `tooling` | object | Missing deterministic tooling and whether `gh` is required for the selected path. |
| `observed_phases` | array of strings | Recipe phases that produced evidence before finalization. |
| `task` | object | `description`: the run's own task text, as data, with any fence of three or more backticks defanged and the whole capped at 4000 characters; `description_read`: whether any reached the collector. |
| `agent_outputs` | object | `implementation_output`: whether the implementation step returned anything (`present`, `empty`, `unread`); `hollow_success_signals`: a typed flag another producer may set, `unknown` when nobody set it. |
| `prior_terminal_state` | object | Existing terminal markers from publish or terminal-state probes, when present. |
| `outcome` | object | The facts of what the run produced; see [The facts](#the-facts). |

`task.description` is there because two success states are judgements about
the task and not about the artifacts: `ALLOW_NO_OP` is "the task asked for no
change" and `NO_DIFF_SUCCESS` is "did what was asked". Without the task text
`assess-finalization` would have to fail every no-change task or approve them
blind; the prompt says so, and says to choose on the published artifacts and
record why when `description_read` is `false`. `agent_outputs` is the same
rule applied to `HOLLOW_SUCCESS`: `unknown` means the signal was not
collected, which is not `false`.

Evidence collectors must record absence explicitly. For example, a missing PR is
`"pr": {"present": false}`, not an omitted `pr` object.

`jq` is used only to build and read this **deterministic** evidence document and
the typed markers below. The prohibition on parsing applies solely to
**agent-generated narrative**.

## Typed Markers Collected Beside the Evidence

In addition to `finalization_evidence`, earlier deterministic steps produce
typed recipe markers. Since #2343 and #2809 nothing in bash classifies them:
they are collected, handed to `assess-finalization` and carried into the
result. The validator reads only the five that are blockers about the local
checkout, listed under [The Run's Real Outcome](#the-runs-real-outcome-2343).

| Marker | Source | Meaning |
| --- | --- | --- |
| `implementation_completed` | TDD/implementation evidence step | Requested repository changes were applied. |
| `verification_completed` | Pre-commit/test evidence step | Required checks for the selected path completed. |
| `publish_state_reached` | Publish/terminal-state step | PR, merge, no-diff, obsolete, or follow-up semantics reached. |
| `prior_terminal_state` | Deterministic terminal-state probe | Existing deterministic terminal classification, if any. |
| `pr_url` / `pr_number` | Publish/terminal-state step | Durable change-request identifiers. |
| `finalizer_step_status.status` | `finalizer-step-status` step | `ok` when the narrative/reporting step completed; `failed` when it did not. |
| `finalizer_step_status.reporting_failure` | `finalizer-step-status` step | `true` when a reporting/finalization step itself failed (derived from that step's own exit status). |

`finalizer-step-status` only observes the reporting step's exit status — it does
not and cannot know whether implementation succeeded. `reporting_failure=true`
means "a reporting step failed," nothing more. Since #2809 it is passed to
`assess-finalization` beside the run's published outcome, which is where
`FAILED_REPORTING` (the work is published, only the reporting failed) is told
apart from `FAILED_IMPLEMENTATION` (it is not). See
[Implementation Failure vs Reporting Failure](#implementation-failure-vs-reporting-failure).

`implementation_completed`, `verification_completed` and `publish_state_reached`
are no longer read to decide a terminal state at all (#2343). They are still
collected and still reported in the result, so an operator can see what the run
recorded about itself beside what it actually produced.

All markers are typed recipe state. None of them are derived by scanning the
finalizer narrative.

## The Run's Real Outcome (#2343)

Finalization reports what the run produced. It does not ask whether the run's
bookkeeping is complete. A finished run, one whose branch is pushed and whose
pull request is open at that head, ends as a success whatever state the context
file is in, whether `implementation_completed` or `verification_completed` were
recorded, and whether CI has reported yet.

The work is split in three, and the split is the point:

| Part | File | What it does |
| --- | --- | --- |
| Collect | [`workflow_finalization_outcome.sh`](../../amplifier-bundle/tools/workflow_finalization_outcome.sh) | Reports what `git` and `gh` printed. Reaches no verdict. |
| Decide | [`workflow-finalization-assessment.yaml`](../../amplifier-bundle/recipes/workflow-finalization-assessment.yaml) | One agent step reads those facts and names the terminal state. |
| Report | [`workflow_finalization_validate.sh`](../../amplifier-bundle/tools/workflow_finalization_validate.sh) | Emits the result for the state the assessment decided. Derives none of its own. |

Before #2809 the middle part was bash: a reader that issued `SUCCESS`,
`BLOCKED`, `GAP` and `UNCERTAIN` verdicts, a classifier that turned them into
terminal states, and a validator that re-read the outcome and classified on
that, discarding the agent's answer. All three are gone, and with them the
coercion rules that made a field outside its allowed set default to the
success-looking value.

### The facts

```text
finalization_outcome_facts <target_dir> <branch> [<pr_ref>]
```

| Argument | Meaning |
| --- | --- |
| `target_dir` | Worktree or repository path. |
| `branch` | The run branch. It is checked with `git check-ref-format` before any git call, and a rejected name is reported unread, not as an error. |
| `pr_ref` | Optional PR number or exact GitHub PR URL. Without it the PR is found by head branch. A PR reference the run recorded is never widened to discovery, so another branch's pull request cannot be adopted. |

It always exits `0` and always prints one compact JSON object. Every value but
`schema_version` is a String:

```json
{
  "schema_version": 3,
  "branch": "feat/issue-2343-fix-issue-2343-and-1726",
  "local_head": "3f9c2a7d5e1b4c60a8d2f1e9b7c3a5d4e6f80912",
  "remote": "rysweet/amplihack-rs",
  "github_origin": "true",
  "remote_head": "3f9c2a7d5e1b4c60a8d2f1e9b7c3a5d4e6f80912",
  "remote_head_descends_local_head": "true",
  "open_pr_count": "1",
  "pr_number": "2792",
  "pr_url": "https://github.com/rysweet/amplihack-rs/pull/2792",
  "pr_state": "OPEN",
  "pr_draft": "false",
  "pr_head": "3f9c2a7d5e1b4c60a8d2f1e9b7c3a5d4e6f80912",
  "pr_cross_repository": "false",
  "pr_head_descends_local_head": "true",
  "checks": "pending",
  "check_states": "build=COMPLETED/SUCCESS; e2e=IN_PROGRESS",
  "unread": ""
}
```

| Field | What it reports |
| --- | --- |
| `branch` | The run branch this read is about. Empty when the recorded name was unusable, or when the checkout is on another branch; `unread` then says which. |
| `local_head` | `git rev-parse HEAD^{commit}` in `target_dir`. |
| `remote` | `origin` as `owner/repo`, through `parse_github_repo_identity`. The origin URL itself never appears, here or on stderr. |
| `github_origin` | `true` when that parse succeeded. |
| `remote_head` | The commit `git ls-remote --heads origin refs/heads/<branch>` listed. Empty when it answered and listed the branch nowhere; empty with a sentence in `unread` when it did not answer. |
| `open_pr_count` | How many open pull requests `gh pr list` returned for the branch: `0`, `1`, or `2` for two or more. Empty when the run named its own pull request, so no list was asked for. |
| `pr_number`, `pr_url` | The pull request this read is about, and its canonical URL. |
| `pr_state`, `pr_draft`, `pr_head`, `pr_cross_repository` | `gh pr view`'s `.state`, `.isDraft`, `.headRefOid` and `.isCrossRepository`. A value outside its shape is reported unread, never coerced into another value. |
| `checks` | That view's `statusCheckRollup` through `_finalization_pr_rollup_class`: `green`, `failing`, `pending`, `none` or `unknown`. Empty when no view was read. A class that falls outside that set reads `unknown` and says so in `unread`. |
| `check_states` | The same rollup as the states gh reported, `name=STATUS/CONCLUSION` for a check run and `name=STATE` for a status context, separated by `; `, at most 40 entries. The class above is a reading; this is what it was read from, so a reader handed `failing` can still see which check failed. Empty when the view carried no rollup. |
| `remote_head_descends_local_head`, `pr_head_descends_local_head` | What `git merge-base --is-ancestor <local_head> <commit>` answered: `true`, `false`, or empty when the commit could not be obtained to ask about. |
| `unread` | One sentence per fact that could not be read, with the reason the tool gave. Empty when everything was read. |

There is no `verdict` field, no `gap` field, no `branch_pushed` and no
`pr_head_relation`. Whether the branch was pushed, whether the pull request is
this run's, and whether any of that is a success are questions about the facts,
not facts, and this file answers none of them. A fact that was read and is
negative -- the remote listing the branch nowhere, `gh pr list` returning none
-- is reported as that fact with `unread` empty, so a reader can tell it from
a fact that could not be read at all.

The reader reuses `_finalization_pr_rollup_class`, `_finalization_pr_head_descends`,
`_finalization_pr_number_valid` and `parse_github_repo_identity`, so the
fast-forward rule and the meaning of "green" are the ones described under
[Resumed Runs](#resumed-runs-open-pr-success-path-1641) and are defined once.
It adds no wait, poll, retry or timer: every state is read exactly once. The
remote is addressed by the name `origin`, never by a URL read from
configuration, and git runs with `GIT_TERMINAL_PROMPT=0` and the batch SSH
command `workflow_pr_head_fetch.sh` already uses, so it fails rather than
prompting. `git` and `gh` stderr is discarded.

### Where the facts are read

`collect-finalization-evidence` runs the reader once and embeds the object at
`finalization_evidence.outcome`. If the reader cannot run, the collector embeds
an empty object, which routes the validator to a live read rather than to a
failure. No field passes through a command line: the object is written to a
`0600` temporary file under a private directory and spliced into the evidence
with `jq --slurpfile`.

`validate-agentic-finalization` uses the embedded object. When it carries no
facts the validator runs the reader itself, once. It fails only when no facts
could be gathered at all, and then as a gap in finalization rather than a
verdict about the run; see [Terminal Classification](#terminal-classification).

### The context file is not a gate

A missing, unreadable, symlinked or directory file named by
`AMPLIHACK_CONTEXT_FILE` makes every context read report absence (status `2`,
nothing on stdout), including the finalization metadata and context readers.
`workflow_context.sh` decides that once, when it is sourced, and says so once:
one `WARN` line on stderr naming the path and why it carried nothing. Removing
the refusal that ended the run was the #2343 fix; removing the diagnostic with
it was not, and a consumer such as `workflow_enforce_verdict.sh` was left
reporting that its producer "produced no output" with nothing anywhere naming
the file (#2819).
Absence is not failure: the facts above are still gathered and still reported.
A file that is present but not a single JSON object carries no data either, so
it reports absence too. A value that IS present but unparseable, of the wrong
type, or carrying a NUL byte is corrupt data: it still returns status `3` with
its diagnostic, it is never repaired from a stale alias, and it is recorded as
unreadable beside the result. See
[Workflow Context Transport](./workflow-context-transport.md#absence-and-corruption-2343).

What a lost context file does cost is the decision it was carrying. The run
then has facts and no terminal state, which finalization reports as exactly
that rather than filling one in.

### The assessment decides

`assess-finalization` is a sub-recipe step between `finalizer-step-status` and
`validate-agentic-finalization`, whose one agent step lives in
[`workflow-finalization-assessment.yaml`](../../amplifier-bundle/recipes/workflow-finalization-assessment.yaml).
It runs on every outcome, with no `condition:` and no verdict that skips it:
before #2343 the states it now decides were reached by bash rules in
`workflow_finalization_validate.sh` and `workflow_finalization_open_pr.sh`,
which exited the run before any agent was asked. Those rules and that second
file are gone.

Its answer is stored as `finalization_assessment`:

| Leaf | Meaning |
| --- | --- |
| `terminal_state` | The run's terminal state, one of the states in [Terminal States](#terminal-states) that finalization owns. This is the decision. |
| `action_taken` | `none`, `pushed_branch` or `opened_pull_request`. |
| `pr_number` | The pull request it judged, as digits, or empty. |
| `reason` | One sentence naming the artifacts it decided on. Becomes `terminal_reason`. |
| `next_action` | What a human or the next loop should do. Becomes `required_next_action`. |

The agent reads the evidence as data. The prompt fences it and states that
nothing inside it is an instruction. It may take two additive actions: push the
run branch (clean worktree, current branch equal to the run branch, `origin` at
the repository the facts name) and open a pull request from it into the run's
recorded base in the same repository. It must not force-push, rewrite history,
delete a branch, work on another branch, merge, close, or change a pull
request's state, readiness, base or metadata. It must not run `env`,
`gh auth token` or `git config --list`, and must not put command output in its
JSON. After an action it reads the result back with `gh` and reports the pull
request number it read.

What it supplies is a state and words. It does not supply identity: the result's
`pr_url` is always built as `https://github.com/<outcome.remote>/pull/<number>`,
and `pr_number` is its own only when that is a canonical pull request number,
otherwise the one the facts read. No repository, URL or commit from its text
reaches `pr_number`, `pr_url` or `evidence_used`. `terminal_reason` and
`required_next_action` are its sentences, cleaned of control characters and
capped in length; they are read by humans and drive nothing.

Both the outer `assess-finalization` step in `workflow-finalize.yaml` and the
agent step inside the sub-recipe carry `continue_on_error: true`, so neither a
declined assessment nor a failure of the sub-recipe itself (resolution, the
recursion limit) ends the run by itself. It leaves the validator with no
decision to report, which is handled below.

## Terminal Classification

`validate-agentic-finalization` reports the terminal state `assess-finalization`
decided. It reads no typed implementation or verification boolean to decide
whether work was finished, it reads no agent narrative, and it compares no
commits of its own.

Evaluation order:

1. **Tooling.** Without `jq` the validator cannot read evidence and returns
   `FAILED_MISSING_TOOLING`.
2. **Selected evidence fields.** A field that is present but corrupt is marked
   unread. It is not a terminal state of its own and it never ends the run; it
   is recorded in `evidence_used` as `finalization_evidence=unreadable`.
3. **Blockers about the local checkout**, which no published outcome can
   answer. A dirty worktree is `FAILED_DIRTY_WORKTREE`; missing `git` or `jq`
   is `FAILED_MISSING_TOOLING`. A missing `gh` is not among them: without `gh`
   the outcome cannot be read, which the facts say in words. A deterministic
   terminal probe that already named a blocker is answered from its own record,
   without asking the remote again. These precede everything else, so no
   decision can mask them.

   The dirty-worktree blocker is the one of the three with two sources, so its
   precedence is explicit: the collected `git.dirty_worktree` is believed when
   it says exactly `true` or exactly `false`, and in every other case -- unread,
   `unknown`, `yes`, `1` -- the worktree is probed live, because none of those
   values answers the question and the worktree always can (#2819).
4. **The facts.** The recorded outcome, or one live read when none was
   recorded. Nothing here reads them as a success or a failure. When no facts
   could be gathered at all the result is `FAILED_MISSING_TERMINAL_EVIDENCE`,
   with the reason naming what could not be read.
5. **The decision.** `finalization_assessment.terminal_state` is the terminal
   state, if it is one finalization publishes. Whether that state is a success
   is a property of the published vocabulary
   (`_finalization_state_is_success` in
   [`workflow_finalization_result.sh`](../../amplifier-bundle/tools/workflow_finalization_result.sh)),
   not of this run. A non-success is routed through the hand-off brick, so a
   pull request reaches the auto-drive loops (#1714).
6. **No decision.** A missing assessment, or a state outside the published
   vocabulary, is `FAILED_MISSING_TERMINAL_EVIDENCE`. The reason quotes what was
   returned and names any fact that was unread, and the step exits `1`.

There is one decision the validator declines to publish, and it is not a
judgement about the run (#2819). `EXISTING_OPEN_PR` is *defined* as a pull
request open at the run's head, so a decision that reports it while naming no
pull request has shown nothing. The validator does not ask whether that pull
request should exist, or re-read the facts to find one, or substitute the
number the run's own bookkeeping recorded: it refuses to certify a decision
that contradicts its own definition, which is no decision, and routes it to
step 6 with a reason quoting what came back. The number has to be the
assessment's own, in canonical form, because the point of the state is that the
agent read the pull request back after deciding. `_finalization_state_names_pr`
in `workflow_finalization_result.sh` is the list, and `EXISTING_OPEN_PR` is the
only member: `IMPLEMENTED_VERIFIED`, `ALLOW_NO_OP` and `NO_DIFF_SUCCESS` all
assert the opposite and name none.

Step 6 is a gap in finalization, not a verdict about the run, and it is what
sends the step to its own recovery agent. `validate-agentic-finalization` runs
under `amplihack orch helper step-recovery` with `--essential
--stop-on-handoff --no-repair`, so the agent's move is `handoff`: it stops the
run with the reason and an owner.

`--no-repair` is there because a retry could not observe a repair (#2819).
Everything this step reads was recorded before it first ran -- the decision in
`finalization_assessment`, the facts and the blockers in
`finalization_evidence` -- so running the same body again reads the same bytes
and ends the same way. The recovery agent cannot change what they say either:
`step-failure-recovery.yaml` forbids it every GitHub write, so it cannot push
the branch or open the pull request a `FAILED_IMPLEMENTATION` is missing, and
forbids it starting a recipe, so it cannot produce an assessment where there
is none. Offering `retry_after_repair` would promise a convergence that cannot
happen. Repairing the run's outcome belongs one step earlier, to
`assess-finalization`, which is permitted exactly those two actions -- push
the run branch, open the pull request -- and then decides the terminal state
on what it did (see
[Step Failure Recovery](./step-failure-recovery.md#when-a-retry-cannot-observe-a-repair)).

`HOLLOW_SUCCESS` and the implementation-vs-reporting split are no longer exits
of their own. `agent_outputs` (`implementation_output` and the typed
`hollow_success_signals`) and the typed
`finalizer_step_status` are facts handed to the assessment and carried in the
result (`hollow_success_detected`, `reporting_failure`, and
`reporting_failure=` in `evidence_used`); the assessment returns `HOLLOW_SUCCESS`
or `FAILED_REPORTING` when they are what happened. This closes the limit the
first version of #2343 left: a run whose finalizer step failed is now judged on
its published outcome rather than ahead of it.

`FAILED_INVALID_EVIDENCE` is no longer produced. The evidence-completeness
checks, the `verification_completed`-gated pending branch and the typed-boolean
fallback were removed: the run's real outcome answers the question each of them
was a proxy for. The token stays in `terminal_vocabulary` and in
`_handoff_known_state`, because a reader must still recognise it from older
runs and other producers. There is no `FAILED_FINALIZER_OUTPUT` state; the
narrative is never parsed.

### What the record carries

`evidence_used` carries the facts the result was reported against, so an
operator can act on a failure without opening the log, and so a test can assert
that the decision was made against what the run produced rather than against
its bookkeeping:

```text
outcome.branch=feat/x,outcome.local_head=<sha>,outcome.remote_head=<sha|none>,
outcome.pr=<n|none>,outcome.pr_state=OPEN,outcome.pr_head=<sha|none>,
outcome.checks=pending,assessment_state=EXISTING_OPEN_PR,
assessment_action=none,reporting_failure=false
```

Tests assert on those SHAs and identifiers, never on wording.

| Situation | Expected `terminal_state` | The record names |
| --- | --- | --- |
| Local head is not on the remote branch | `FAILED_IMPLEMENTATION` | The branch and the local head, with `outcome.remote_head=none`. |
| Branch pushed, no open PR | `FAILED_IMPLEMENTATION` | The branch and `outcome.pr=none`. |
| PR head differs and does not descend from the local head | `FAILED_IMPLEMENTATION` | `outcome.pr`, `outcome.pr_head` and `outcome.local_head`. |
| A check is failing | `BLOCKED_CI` | `outcome.pr` and `outcome.checks=failing`. |
| PR open at the run's head, CI pending | `EXISTING_OPEN_PR` (a success) | `outcome.pr`, `outcome.pr_state=OPEN` and `outcome.checks=pending`. |
| `EXISTING_OPEN_PR` decided with no pull request named | `FAILED_MISSING_TERMINAL_EVIDENCE` (no decision) | The facts, and a reason quoting the `pr_number` that came back. |

A real failure ends the run as before and exits `1`. The change removes the
failures that were not real.


## Terminal States

| State | Success? | Meaning |
| --- | --- | --- |
| `MERGED` | Yes | Workflow-owned PR is merged or closed with merge evidence. |
| `CLOSED_OBSOLETE` | Yes | PR or branch is obsolete and local evidence proves no meaningful work remains. |
| `NO_DIFF_SUCCESS` | Yes | Worktree is clean with no meaningful diff or commits against base. |
| `FOLLOWUP_CREATED` | Yes | Meaningful remaining work is represented by a new workflow-owned follow-up PR or issue. |
| `SUPERSEDED` | Yes | A newer workflow-owned PR or issue explicitly supersedes this run and durable metadata links the old run to the replacement. |
| `IMPLEMENTED_VERIFIED` | Yes | Implementation and required verification completed on a path that does not require publish/merge evidence. |
| `ALLOW_NO_OP` | Yes | Explicit no-op path was allowed and includes evidence-backed reason text. |
| `EXISTING_OPEN_PR` | Yes | The branch is pushed at the local head and an open, same-repo PR sits at that head (or a descendant of it). Checks may be green, pending or not reported; when they are not green, `terminal_reason` names the pending checks and `required_next_action` names the PR to watch. A failing check is `BLOCKED_CI` instead. See [The Run's Real Outcome](#the-runs-real-outcome-2343). |
| `MANUAL_REQUIRED` | No | A provider action is intentionally manual and the final output names the required next action. |
| `BLOCKED_MANUAL_PROVIDER` | No | Provider tooling, credentials, permissions, or APIs block required automation. |
| `BLOCKED_CI` | No | Required checks are failing, pending beyond policy, or unavailable when required. On the resumed-run open-PR path, any failing check in the PR's `statusCheckRollup` (#1641). |
| `FAILED_DIRTY_WORKTREE` | No | Uncommitted or untracked work prevents terminal success. |
| `FAILED_MEANINGFUL_DIFF` | No | Meaningful branch changes remain but no validated publish, merge, follow-up, no-op, or implementation-plus-verification path proves closure. |
| `FAILED_CLOSED_UNMERGED` | No | PR is closed without merge evidence and meaningful branch diff remains. |
| `FAILED_PR_METADATA_UNAVAILABLE` | No | Provider change-request proof is required but metadata or auth is missing, stale, or ambiguous. |
| `FAILED_MISSING_TOOLING` | No | Required deterministic tooling such as `git`, `jq`, or provider CLI support is missing for the selected path. |
| `FAILED_INVALID_EVIDENCE` | No | Retired as an outcome of finalization (#2343). Still recognised in the vocabulary so older runs and other producers read correctly. |
| `FAILED_IMPLEMENTATION` | No | Durable implementation or verification evidence is absent or failed while meaningful work remains. |
| `FAILED_REPORTING` | No | The work is published but a reporting/finalization step failed. Durable evidence (`pr_url`, `pr_number`) is preserved and reported. Since #2809 `assess-finalization` returns it, judging the typed `finalizer_step_status` beside the published outcome rather than ahead of it. |
| `FAILED_MISSING_TERMINAL_EVIDENCE` | No | Finalization reached no decision: the facts of the run's outcome could not be gathered, or `assess-finalization` named no state finalization publishes. It is a gap in finalization rather than a verdict about the run, and it leaves the step failing for its recovery agent (#2343, #2809). |
| `HOLLOW_SUCCESS` | No | Empty or inaccessible agent output: the run may have produced nothing at all. Since #2809 the signal is a fact handed to `assess-finalization`, which returns this state when nothing published contradicts it, rather than an exit taken ahead of the outcome read. |
| `INCOMPLETE` | No | Work remains and no more specific terminal failure state applies. It is the default for a missing `workflow_result` in `workflow-complete`, with a plain reason. |

`terminal_failure=true` is derived for all non-success states. Failure states
override success-looking implementation, verification, publish, and no-op
markers.

`MANUAL_REQUIRED` and `BLOCKED_MANUAL_PROVIDER` are not states finalization
decides. They belong to the broader terminal-state vocabulary in
`workflow-terminal-state.yaml` and reach the finalization result only via
`prior_terminal_state` propagation from an upstream deterministic probe.
Finalization preserves them but never originates them: they are not in the
list `assess-finalization` may choose from, and the validator reaches no state
of its own.

> **Retired:** `FAILED_FINALIZER_OUTPUT` no longer exists. It described the
> brittle "agent output was not a single JSON object" failure that this feature
> removes. Reporting-step failures classify as `FAILED_REPORTING`. Malformed
> deterministic evidence no longer classifies at all: it is recorded as
> unreadable beside whatever the outcome says (#2343).

## Implementation Failure vs Reporting Failure

Two failure families that were once collapsed stay distinct:

| | `FAILED_IMPLEMENTATION` | `FAILED_REPORTING` |
| --- | --- | --- |
| Meaning | The work was not published: nothing on the remote, no pull request, or a pull request at a commit unrelated to the run's head. | The work is published, but a reporting/finalization step failed. |
| `reporting_failure` | May be either | `true` |
| Durable evidence (`pr_url`, `pr_number`) | Named when the decision was about a pull request | Preserved and reported |
| `terminal_success` | `false` | `false` |
| Operator action | Push the work and open the pull request, or explain why there is none. | Retry the reporting step; the implementation is intact. |

The split ensures that free-form explanatory output, or a transient failure in
a reporting step, can never erase proof that the work was published.

Since #2809 it is `assess-finalization` that makes the split, not the
validator. `finalizer-step-status` reports, deterministically and without
touching agent prose, whether the reporting step failed; that typed fact is
handed to the assessment beside the run's published outcome, and carried into
the result as `reporting_failure` and into `evidence_used`. The assessment
returns `FAILED_REPORTING` when the work is published and only the reporting
failed, and `FAILED_IMPLEMENTATION` when it is not. The validator reports
whichever it returns; it no longer combines the marker with typed
implementation and verification booleans, which it does not read.

## Deterministic Validation Rules

These are what `validate-agentic-finalization` checks for itself before it
reports the result. They are deliberately few: since #2343 the step is a
reporter, so every rule here is either a blocker about the local checkout or a
check that a decision names what it asserts. None of them re-derives a
terminal state.

1. The run's real outcome is read first (see
   [The Run's Real Outcome](#the-runs-real-outcome-2343)).
   Evidence with no usable `outcome` is not invalid: the outcome is read live.
2. A dirty worktree and missing `git` or `jq` still end the run. A missing `gh`
   does not; it routes to the assessment. A collected `dirty_worktree` that is
   neither `true` nor `false` is not a negative: the worktree is probed.
3. No terminal state is derived from empty or generic agent output.
4. Reporting failure (`finalizer_step_status.reporting_failure=true`) is a
   typed fact passed to `assess-finalization` and carried into the result; it
   is not an exit of the validator's own.
5. The terminal state is the one `assess-finalization` returned, if it is one
   finalization publishes. Otherwise the result is
   `FAILED_MISSING_TERMINAL_EVIDENCE` and the step is left failing for its
   recovery agent.
6. One decision is refused: `EXISTING_OPEN_PR` naming no pull request. That
   state's definition *is* an open pull request, so a decision reporting it
   without one has shown nothing, and the result is
   `FAILED_MISSING_TERMINAL_EVIDENCE` with the returned value quoted back.
   The number has to be the assessment's own, in canonical form; the validator
   does not read the remote to find one, substitute the number the run's
   bookkeeping recorded, or ask whether that pull request should exist. No
   other state carries a proof requirement here. In particular `ALLOW_NO_OP`
   is *not* checked for a no-op authorization and `BLOCKED_CI` is not checked
   against CI: `allow_no_op`, `terminal_no_op` and the check states are facts
   handed to `assess-finalization`, which weighs them against the run's task
   text, and its answer is what gets reported.
7. The narrative artifact is never read for any of these decisions.
8. The normalized result is persisted even for failure states so operators can
   diagnose the run from recipe JSON.

Finalization is still total: every input produces exactly one result. Inside
`validate_finalization` the fallthrough is step 6 of the evaluation order in
[Terminal Classification](#terminal-classification) -- no decision, carrying
the outcome's gap -- which is a failure to reach a terminal state rather than a
terminal state of its own. When validation produces no result at all, the
default is `INCOMPLETE` with a plain reason. The completion helper (`complete()`
in `complete_workflow` in `workflow_finalization_complete.sh`) initialises its
`terminal_state` default to `INCOMPLETE`. `terminal_vocabulary` and the hand-off helper's known-state list
are unchanged, so retired tokens stay recognised.

The proof requirements this section used to list -- `MERGED` needs merge
evidence, `CLOSED_OBSOLETE` needs obsolete proof, `SUPERSEDED` needs a
replacement identifier, and so on -- belong to the producers of those states,
not to finalization. `assess-finalization` cannot return them (they are not in
the list it chooses from), and the validator never reached them on its own.

## Resumed Runs: Open-PR Success Path (#1641)

A run that resumes an earlier attempt's work often has nothing left to
implement. The PR already exists and CI is already green, so this run's
implementation and verification steps never set `implementation_completed` or
`verification_completed`. Before #1641, `validate-agentic-finalization` then
fell through to the default `FAILED_IMPLEMENTATION`, even though step 22b
(`workflow_final_status.sh`, since #1623) had already reported the same run as a
success. PRs #1635 and #1639 hit this.

Since #2343 that case is decided on the run's real outcome, described under
[The Run's Real Outcome](#the-runs-real-outcome-2343), and `verification_completed`
is not consulted at all: a pending or unreported CI state with the pull request
open at the pushed head is `EXISTING_OPEN_PR` without it.

What survives of this section is its definitions, which the facts reader and
the terminal-state probe both still use: what counts as the same pull request
(the [PR head rule](#pr-head-rule-1714)), how the pull request is located, and
what ["green" means](#what-green-means). What does not survive is the
classifier those definitions once fed. There is no longer a list of conditions
that must all hold before `EXISTING_OPEN_PR` may be reached: the facts below
are collected and reported, and `assess-finalization` decides what they mean.

Read as facts rather than as conditions, they are:

| Fact | Where it comes from | Reported as |
| --- | --- | --- |
| The finalization target directory | `worktree_path`, else `repo_path`. The `.` fallback is not a recorded worktree, so no outcome is read for it. | Nothing is gathered; the result says so. |
| The origin | `remote.origin.url` of that directory, through `parse_github_repo_identity`. | `remote`, `github_origin` |
| The remote the run declared | `remote_host_type`. A run that recorded a non-GitHub remote is never asked about a GitHub pull request, whatever its origin says (#1641, D5). | `declared_remote_host`, and a sentence in `unread` |
| The run branch | `branch_name`, and `git branch --show-current` when both are set. Not `main`, `master` or `HEAD`, not leading-dash, and valid under `git check-ref-format`. | `branch`, or empty with a sentence in `unread` |
| The commit the remote carries | one `git ls-remote --heads origin refs/heads/<branch>` | `remote_head`, `remote_head_descends_local_head` |
| The pull request | `gh pr view --repo <owner>/<repo>`, for the number the run recorded or the single open one `gh pr list --head <branch>` returned | `pr_number`, `pr_url`, `pr_state`, `pr_draft`, `pr_head`, `pr_head_ref`, `pr_cross_repository`, `open_pr_count` |
| Its checks | `statusCheckRollup` through `_finalization_pr_rollup_class` | `checks` |
| Whether that head carries the run's work | `git merge-base --is-ancestor`, with the one bounded fetch of the [PR head rule](#pr-head-rule-1714) | `pr_head_descends_local_head` |

Metadata that is present but corrupt (wrong JSON type, NUL byte) still stops a
live read being directed by it, because it cannot say which branch or pull
request to read. Absence is not corruption: a missing context file, or an unset
empty object key, reads as empty and disables nothing (#1726, #2343). A pull
request number the run recorded is still never widened to discovery, so another
branch's pull request cannot be adopted.

The judgements those facts used to be fed into are now the assessment's, and
the prompt states them: a pull request open at the run's head is a success
whether its checks are green, pending or unreported; a failing check is
`BLOCKED_CI`; a pull request on another branch, from a fork, or at a commit the
local head is not an ancestor of does not carry this run's work, and a green
rollup on an older commit cannot vouch for code that was never pushed.

### PR head rule (#1714)

The PR head commit (`headRefOid`) qualifies when it is the local HEAD, or when
the local HEAD is its ancestor, which is the case when a merge of main was
pushed to the PR from another checkout. Every local commit is then in the PR,
so the PR's checks cover this run's work. Since #2343 the facts reader reports
the question's answer rather than a label for it: `pr_head` is the commit gh
named, and `pr_head_descends_local_head` is what `git merge-base --is-ancestor`
replied -- `true`, `false`, or empty when the commit could not be obtained to
ask about. `evidence_used` carries `outcome.local_head` and `outcome.pr_head`,
so a result shows both.

| Local HEAD vs PR head | `pr_head_descends_local_head` | `finalization_open_pr_signal`'s `head` |
| --- | --- | --- |
| Equal | `true` | `local` |
| Local HEAD is an ancestor of the PR head | `true` | `descendant` |
| Local HEAD is ahead (unpushed commits) | `false` | no signal |
| Diverged, or the PR head is a rewrite of the local commit | `false` | no signal |
| PR head commit not in the local object store and not fetchable | empty, with the reason in `unread` | no signal |

The test and its fetch are `_finalization_pr_head_descends` in
[`workflow_pr_head_fetch.sh`](../../amplifier-bundle/tools/workflow_pr_head_fetch.sh).
The terminal-state probe that `workflow-finalize` runs before this validator
uses the same function (see
[A PR ahead of local HEAD](./workflow-terminal-state.md#a-pr-ahead-of-local-head-is-the-same-pr-1714)),
so the probe does not report a fast-forward PR head as stale metadata
(`FAILED_INVALID_INPUT`, which this validator never hands off) before this
check can run.

When the PR head commit is not in the local object store, the helper runs one
`git fetch --no-tags --no-write-fetch-head --refmap= origin refs/heads/<branch>`.
It runs only after the view has passed every check that does not need the
commit: the PR is open, same-repo, on the run branch, and has the requested
number and canonical URL. A closed, fork or wrong-branch PR is rejected without
a network call. The fetch writes no ref and no `FETCH_HEAD`.

The fetch cannot prompt, and its wait is limited even where `timeout(1)` is
missing, as on stock macOS:

| Transport | What stops a prompt or a hang |
| --- | --- |
| HTTPS | stdin is `/dev/null`, `GIT_TERMINAL_PROMPT=0` stops git's credential prompt, and `http.lowSpeedLimit=1000` with `http.lowSpeedTime=30` aborts a transfer that stays below 1 KB/s for 30 seconds. |
| SSH (`git@github.com:…`, `ssh://git@github.com/…`) | `GIT_SSH_COMMAND` is set to the ssh command git would otherwise use (`GIT_SSH_COMMAND`, then `core.sshCommand`, then `GIT_SSH`, then `ssh`) followed by `-o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2`. ssh then fails instead of asking for a key passphrase, a password or an unknown host key, gives up on a connection after 15 seconds and drops a dead one after about 30. Options on the command line take precedence over `~/.ssh/config`. A non-OpenSSH `GIT_SSH` program such as plink rejects `-o`, so the fetch fails and there is no signal. |
| Both | Where `timeout` exists, the whole fetch is also capped at 60 seconds. |

The same setting keeps every other git that amplihack and its tests start
from waiting on a credential prompt; see
[Keep Tests from Prompting for Git Credentials](../howto/keep-tests-from-prompting-for-git.md).

A failed fetch is not an error. If the commit is still missing afterwards, or
`git merge-base --is-ancestor` fails for any reason, there is no signal: a
missing object is never read as an ancestor.

### How the PR is located

The repo `R` (`owner/repo`) always comes from the target directory's
`remote.origin.url`, and every `gh` call passes `--repo "$R"`. Without it, `gh`
resolves the repo from the checkout's remotes and prefers `upstream` or a
`gh repo set-default` choice, so a fork checkout would query the wrong repo and
fail falsely.

1. If the context carries a canonical `pr_number` (`^[1-9][0-9]{0,9}$`), it is
   used; otherwise a `pr_url` of the exact form
   `https://github.com/<owner>/<repo>/pull/<n>` whose repo equals `R` is used.
   The validator makes one signal call with that reference and runs:

   ```bash
   gh pr view 1641 --repo rysweet/amplihack-rs \
     --json number,url,state,headRefName,headRefOid,isCrossRepository,statusCheckRollup
   ```

   A `pr_url` that names any other repo, or is not in that exact form (trailing
   slash, `/files` suffix, different letter case), is rejected without calling
   `gh`. A valid `pr_number` takes precedence over the URL, so a malformed URL
   never hides it; the number is re-checked live like any other reference.
   A non-canonical `pr_number` (`abc`, `0`, `#7`) with no `pr_url` is rejected
   without calling `gh`. It never falls back to `gh pr list` from a rejected
   reference.

2. Otherwise, when condition 2 holds through a publish status alone (for
   example the GitHub publish path's lowercase `existing-open-pr`, which records
   no URL), it runs a read-only search:

   ```bash
   gh pr list --repo rysweet/amplihack-rs \
     --head feat/issue-1641-fix-issue-1641-read-it --state open \
     --json number,url,isCrossRepository --limit 2
   ```

   The result must contain exactly one PR in total, and that PR must have
   `isCrossRepository=false`. Zero results fail closed. Two results fail closed
   even when only one of them is same-repo (for example, one same-repo PR plus
   one fork PR with the same head branch name). The single match is then
   checked with `gh pr view` as in step 1.

3. With no publish signal, `gh` is never called.

`gh` runs read-only with a fixed argv and literal `--json` field lists. Each call
makes a single attempt with a 60-second timeout, does not retry, does not wait
out rate limits and has no REST fallback. A validation makes at most two calls
(`pr list` then `pr view`), so the worst case is about two minutes. A missing,
unauthenticated or failing `gh`, or malformed JSON, means "no signal". The
outcome is then `UNCERTAIN` and the
[assessment](#the-assessment-step) names the gap.

### What "green" means

`checks=green` requires a non-empty `statusCheckRollup` in which:

- every `CheckRun` has `status=COMPLETED` and `conclusion` of `SUCCESS`,
  `NEUTRAL` or `SKIPPED`;
- every `StatusContext` has `state=SUCCESS`.

A pending, failing or null entry, an unknown type, or an empty rollup is not
green. Every entry counts, not only required checks: `gh pr view` does not say
which checks are required, so the evidence is labelled `all_checks=green`, not
`required_checks=green`. CI that has not reported (`pending` or `none`)
does not stop the hand-off. A `failing` entry classifies `BLOCKED_CI`, and an
`unknown` one goes to the assessment.

### Result

On success the canonical result keeps its schema of 19 string keys and exits 0.
`terminal_reason` and `required_next_action` are `assess-finalization`'s own
sentences and are illustrative here; nothing reads them. `evidence_used`
carries the facts the decision was reported against, and is what a test asserts
on:

```text
terminal_success=true
terminal_state=EXISTING_OPEN_PR
terminal_reason=<the assessment's sentence>
required_next_action=No action required.
publish_state_reached=true
pr_number=1641
pr_url=https://github.com/rysweet/amplihack-rs/pull/1641
hollow_success_detected=false
reporting_failure=false
evidence_used=outcome.branch=feat/x,outcome.local_head=<sha>,outcome.remote_head=<sha>,outcome.pr=1641,outcome.pr_state=OPEN,outcome.pr_head=<sha>,outcome.checks=green,assessment_state=EXISTING_OPEN_PR,assessment_action=none,reporting_failure=false
```

When CI has not reported yet (`checks` is `pending` or `none`), the run still
succeeds. This is the "PR open, CI pending" state: `outcome.checks=pending` (or
`none`) in `evidence_used`, and a `required_next_action` that names the pull
request to watch.

When the pull request has a failing check, `assess-finalization` returns
`BLOCKED_CI` and the result exits nonzero. `pr_number` and `pr_url` are still
the confirmed values, so the next step knows which pull request to fix:

```text
terminal_success=false
terminal_state=BLOCKED_CI
required_next_action=<the assessment's sentence>
evidence_used=...,outcome.pr=1641,outcome.checks=failing,assessment_state=BLOCKED_CI,...
```

`pr_number` is the one the assessment judged, validated to be a pull request
number; `pr_url` is always rebuilt as
`https://github.com/<outcome.remote>/pull/<number>`, so neither is copied from
the input context or from the assessment's text.
`publish_state_reached=true` comes from an internal override that
`validate_finalization` clears on entry, so it cannot be set from the
environment; it is `true` when the facts report that pull request `OPEN`.

`workflow-complete` accepts `EXISTING_OPEN_PR` as success. It is listed in
`terminal_vocabulary`, and `workflow_final_status.sh` already treats it as
success.

### Fail-closed cases kept

The validator no longer maps a situation to a terminal state; it applies the
blockers about the local checkout, gathers the facts and reports the decision.
The first group below is still the validator's, taken before anything is read,
so no decision can mask it. The second is what the facts carry and what the
assessment prompt says the answer is; the assessment may be asked to read more
itself, and the result says plainly when it decided nothing.

Blockers the validator still owns, in order, before any outcome is gathered:

| Situation | Terminal state | `gh` called? |
| --- | --- | --- |
| Dirty worktree, with or without an open green PR | `FAILED_DIRTY_WORKTREE` | No |
| Collected evidence reports missing `git` or `jq` | `FAILED_MISSING_TOOLING` | No |
| `prior_terminal_state=BLOCKED_CI` (or another deterministic blocker), with or without an open green PR | that state | No |
| No recorded worktree or repository path, or corrupt metadata | `FAILED_MISSING_TERMINAL_EVIDENCE`, naming what could not be read | No |
| `assess-finalization` returned nothing, or a state finalization does not publish | `FAILED_MISSING_TERMINAL_EVIDENCE`, quoting what it returned | Facts were read |

What the facts report, and the state the prompt tells the assessment to return:

| Situation | The facts carry | Expected state | `gh` called? |
| --- | --- | --- | --- |
| Nothing pushed, no pull request | `remote_head=""`, `pr_number=""`, `open_pr_count=0` | `FAILED_IMPLEMENTATION` | Yes |
| Open PR at the pushed head, any check failing | `pr_state=OPEN`, `pr_head_descends_local_head=true`, `checks=failing` | `BLOCKED_CI` | Yes |
| Open PR at the pushed head, a check pending or none reported | the same with `checks=pending` or `none` | `EXISTING_OPEN_PR` (success) | Yes |
| Open PR, an unclassifiable check | `checks=unknown` | the assessment reads the checks itself, or fails naming them | Yes |
| PR `CLOSED` or `MERGED` | `pr_state=CLOSED` / `MERGED` | `FAILED_IMPLEMENTATION`, or `FAILED_CLOSED_UNMERGED` | Yes |
| PR on another branch, or cross-repository | `pr_head_ref`, `pr_cross_repository=true` | `FAILED_IMPLEMENTATION` | Yes |
| PR head diverged from, behind, or not a fast-forward of the local HEAD | `pr_head`, `pr_head_descends_local_head=false` | `FAILED_IMPLEMENTATION` | Yes |
| PR head not fetchable | `pr_head_descends_local_head=""` and a sentence in `unread` | not a success it could not check | Yes |
| `pr_url` names another repo and no valid `pr_number` is set | `pr_number=""` and a sentence in `unread`; no `pr list` fallback | `FAILED_IMPLEMENTATION` | No |
| `gh` missing, unauthenticated, non-zero exit or malformed JSON | the pull request fields empty, with the reason in `unread` | never an unexplained failure | Attempted |
| `pr list` returns zero, two, or one cross-repository PR | `open_pr_count` `0` or `2`, `pr_number=""` | `FAILED_IMPLEMENTATION` | Yes |
| Target directory is a `.git` directory or bare repo, or the branch is `main` | `branch=""` and a sentence in `unread` | not a success it could not check | No |
| The run recorded a non-GitHub remote | `declared_remote_host`, no pull request read | `IMPLEMENTED_VERIFIED` when the work was done | No |
| Hollow-success signals with an open green PR | `hollow_success_signals=true` beside the open PR | `EXISTING_OPEN_PR` (the PR is the evidence) | Yes |
| `reporting_failure=true` with a PR at the pushed head | `reporting_failure=true` beside the open PR | `FAILED_REPORTING` | Yes |
| `reporting_failure=true` with nothing published | `reporting_failure=true`, `pr_number=""` | `FAILED_IMPLEMENTATION` | Yes |

Neither the facts reader nor the validator reads
`agentic_finalizer_narrative` or any other prose, and neither adds a prose
field to the `workflow_finalization_field` allowlist.

### Helper: `finalization_open_pr_signal`

`amplifier-bundle/tools/workflow_finalization_pr_signal.sh` is sourced by
`workflow_agentic_finalization.sh` together with `workflow_gh_retry.sh`. It
reports facts and does not classify.

Since #2343 the validator does not call this entry point. It is kept as the one
definition of the rules the outcome reader reuses -- `_finalization_pr_checks`,
`_finalization_pr_list_number`, `_finalization_pr_number_valid` and the bounded
`gh` read -- so the "is this PR this run's PR" rule cannot drift between the
two. The brick that used to wrap it, `workflow_finalization_open_pr.sh`, is
deleted: its check asked whether the run's own bookkeeping permitted it to
look, and refused to look otherwise, which is the behaviour #2343 removes.

```text
finalization_open_pr_signal <target_dir> <branch> [<pr_ref>]
```

| Argument | Meaning |
| --- | --- |
| `target_dir` | Worktree or repo path. Its `remote.origin.url` sets the repo identity passed to every `gh` call as `--repo`. |
| `branch` | The run branch from metadata. Must match the live branch when both are set. |
| `pr_ref` | Optional PR number (`^[1-9][0-9]{0,9}$`) or exact GitHub PR URL. If omitted, the helper uses the `gh pr list --head` lookup. |

Exit 0 prints one compact JSON line:

```json
{"pr_number":"1641","pr_url":"https://github.com/rysweet/amplihack-rs/pull/1641","checks":"green","head":"local"}
```

`checks` is one of `green`, `failing`, `pending`, `none` or `unknown`. `head`
is `local` or `descendant`; see [PR head rule](#pr-head-rule-1714). Exit 1
prints nothing on stdout and means no usable open-PR signal; stderr gets one
fixed line, `WARNING: no open-PR signal: <reason>`, naming the check that
failed (for example `PR reference has an invalid PR number` or `local HEAD
could not be resolved`). Reasons are literal strings: no URL, branch name or
`gh` output is ever interpolated. The helper never prints
`gh` stderr or the origin URL. It uses a private `mktemp -d` directory for retry
scratch files and removes it on every return path. If `mktemp` fails, it
returns 1 rather than writing into the working directory.

### Known limitations

- **Every reported check, not just the required ones.** `gh pr view` cannot
  tell which checks are required, so the rollup it feeds cannot either. Since
  #2819 the facts carry `check_states` -- what gh reported, check by check --
  beside the rollup class, and `assess-finalization` is told to judge on that
  where the two could differ. It is still the conservative reading: a failing
  check nobody marked required is a reason to look, not to call a red build
  done.
- **Point-in-time.** The PR can change after validation. Finalization reports
  what it saw; it is not a merge gate.
- **AzDO.** Resumed AzDO runs are not checked against provider CI. Without the
  GitHub path they stay `FAILED_IMPLEMENTATION`.
- **Local origin is trusted.** Repo identity comes from the worktree's
  `remote.origin.url`.
- **Detached HEAD.** With no live branch, only the context `branch_name` is
  checked against the PR's `headRefName`. The PR must still be open, from the
  same repo, have `headRefOid` equal to the detached HEAD or a fast-forward of it, and pass
  verification.
- **Empty `remote_host_type`.** Treated as GitHub. This cannot widen the check:
  the origin URL must still parse as a `github.com` repository, and userinfo
  containing `/`, `?`, `#` or `\` is rejected so a foreign host cannot hide
  behind `@github.com/`.

## Auto-Drive Finalization Hand-Off (#1714)

`BLOCKED_CI`, `FAILED_IMPLEMENTATION` and `FAILED_REPORTING` normally exit non-zero, which ends any
recipe that called `default-workflow`. Under
[auto-drive](auto-drive-to-merge.md#finalization-hand-off-from-phase-1-1714)
a published, open PR is exactly what the later crusty and merge-ready loops
review and fix, so `autodrive-build` asks finalization to hand the PR on
instead of ending the run. The classification does not change: the state is
unchanged and `terminal_success` is still
`"false"`. Only the exit code and one field differ.

### Conditions

The validator hands off only when all of these hold. Otherwise it fails closed
exactly as before.

| # | Condition | Source |
| --- | --- | --- |
| 1 | The state is `BLOCKED_CI`, `FAILED_IMPLEMENTATION` or `FAILED_REPORTING`, matched exactly by `finalization_handoff_allowed`. | `workflow_finalization_handoff.sh` |
| 2 | `RECIPE_VAR_finalization_handoff` is exactly `true`. Only `autodrive-build` step-02 sets it; `default-workflow.yaml` declares it `""`. | `finalization_handoff_flag` |
| 3 | `pr_number` passes `_finalization_pr_number_valid`. On the assessment path this is the number `assess-finalization` returned; on the prior-state path it is the run's own PR evidence. | Validator state |
| 4 | For `FAILED_IMPLEMENTATION` and `FAILED_REPORTING` only: `assess-finalization` named a pull request, in canonical form, with the failure it decided (`open_pr_confirmed_number`). Since #2343 there is no live open-PR check: the agent read the request back after deciding, and that number is what the loops are handed. It is checked again before any hand-off is acted on -- `autodrive_build_handoff` refuses a pull request that is not `OPEN`. | `finalization_try_handoff_open_pr` |

Both `BLOCKED_CI` exits use it: the prior-state probe and the assessment's own
decision. The default `FAILED_IMPLEMENTATION` exit and the reporting-path
`FAILED_IMPLEMENTATION` exit use it through
`finalization_fail_or_handoff_open_pr`; the `FAILED_REPORTING` exit calls
`finalization_try_handoff_open_pr` and otherwise emits its own result, which
keeps `finalizer_output_valid=true`. The five other prior-state blockers
(`FAILED_MEANINGFUL_DIFF`, `FAILED_CLOSED_UNMERGED`,
`FAILED_PR_METADATA_UNAVAILABLE`, `FAILED_INVALID_INPUT`,
`FAILED_WRONG_BRANCH`), `FAILED_DIRTY_WORKTREE`, and every other failure keep
`exit 1` with the flag on. So does a `FAILED_IMPLEMENTATION` or
`FAILED_REPORTING` that named no pull request.

### Result

The result is the normal `BLOCKED_CI` result with `handoff` added, and the
validator exits `0`:

```text
terminal_success=false
terminal_failure=true
terminal_state=BLOCKED_CI
required_next_action=Fix the failing CI checks on PR #134 before merge.
pr_number=134
finalizer_output_valid=false
handoff=merge_ready_loop
```

stderr gets `INFO: #1714: BLOCKED_CI handed to the merge-ready loop (PR #134)`
(or `FAILED_IMPLEMENTATION` in place of `BLOCKED_CI`).
That line is for people; no step decides anything from it.
`workflow_finalization_complete.sh` nests the whole object, so
`.workflow_result.handoff` reaches the caller without further changes.

The validator writes the result through `emit_result | jq -c '. + {handoff:"merge_ready_loop"}'`
under `pipefail`. If that pipeline fails, it exits `1`, so exit `0` always
means a complete JSON object was written.

### Helper: `finalization_fail_or_handoff`

```text
finalization_fail_or_handoff <state> <reason> <next_action> <evidence>
```

A drop-in for `fail_result` with the same four arguments. It is defined in
`amplifier-bundle/tools/workflow_finalization_handoff.sh`, which
`workflow_agentic_finalization.sh` sources after
`workflow_finalization_pr_signal.sh`. It calls `fail_result` and
`emit_result`, which are nested functions of `validate_finalization`, so it
may be called only from inside `validate_finalization`. When the conditions
above do not hold it calls `fail_result "$@"` and never returns.

`finalization_fail_or_handoff_open_pr` takes the same arguments. It hands off
only when the validator recorded a pull request the assessment named
(`open_pr_confirmed_number`, `open_pr_confirmed_url` and
`open_pr_confirmed_facts`, set only from a canonical number the assessment
itself returned). Otherwise, or when the hand-off is refused, it calls `fail_result` with
the run's own PR evidence restored.

### Standalone runs

A standalone `default-workflow`, and every other caller of it, never hands
off. `default-workflow.yaml` declares `finalization_handoff: ""`. An exported
`RECIPE_VAR_finalization_handoff=true` does not switch the hand-off on, provided
the recipe runner keeps giving a recipe's declared context priority over an
inherited `RECIPE_VAR_` value; see
[`finalization_handoff`](./workflow-terminal-state.md#finalization_handoff).

The hand-off also depends on the opposite case: the value `"true"` that
`autodrive-build` step-02 passes to its `default-workflow` sub-recipe must
override `default-workflow.yaml`'s declared `""`. If the runner does not
apply it, the flag is never on, the hand-off never happens, and an auto-drive
build that ends `BLOCKED_CI` or `FAILED_IMPLEMENTATION` fails closed exactly
as it did before #1714.

A standalone run with red CI still ends:

```text
terminal_success=false
terminal_state=BLOCKED_CI
=> exit 1, no handoff field
```

## Canonical Result Schema

Successful and failing finalization results use the same normalized result
object. In full recipe JSON this object is stored as `workflow_result`. Shell
helpers and individual recipe steps may also expose the same fields as flattened
key/value outputs, but the field names and meanings are identical.

| Output | Meaning |
| --- | --- |
| `terminal_success` | Boolean string or JSON boolean indicating whether finalization proved success. |
| `terminal_state` | Stable state from the terminal-state vocabulary. |
| `terminal_reason` | Deterministic classification reason. |
| `required_next_action` | Actionable next step. |
| `hollow_success_detected` | Whether hollow success was detected. |
| `evidence_used` | Typed evidence keys used for classification. |
| `reporting_failure` | `true` when a reporting/finalization step itself failed (from that step's exit status). Since #2809 it is one of the facts `assess-finalization` weighs against the published outcome to choose between `FAILED_REPORTING` and `FAILED_IMPLEMENTATION`; the validator only carries it. |
| `implementation_completed` | Normalized implementation evidence. |
| `verification_completed` | Normalized verification evidence. |
| `publish_state_reached` | Normalized publish/PR/follow-up evidence. |
| `terminal_no_op` | Whether the final state is an explicit no-op success. |
| `terminal_failure` | `true` for all non-success terminal states. |
| `change_request_url` | Provider change-request URL when a validated change request or follow-up exists. |
| `change_request_id` | Provider change-request identifier when available. |
| `pr_url` | GitHub compatibility URL when a validated GitHub PR or follow-up exists. Preserved on `FAILED_REPORTING`. |
| `pr_number` | GitHub compatibility number when a validated GitHub PR exists. Preserved on `FAILED_REPORTING`. |
| `handoff` | Present only on an [auto-drive hand-off](#auto-drive-finalization-hand-off-1714), with the value `merge_ready_loop`. Absent, never `""`, on every other result. It never changes `terminal_success`. |

### Retained Legacy Compatibility Fields

To keep the `workflow_result` object shape stable for existing consumers and
`workflow-complete`/terminal-state contract tests, three fields that previously
described the parsed agent blob are **retained but no longer agent-derived**.
They are emitted as deterministic constants (or evidence-derived values) and are
never read back for control flow. New code should classify from the typed fields
above; these exist only for backward compatibility.

| Output | Value under the finished contract | Meaning |
| --- | --- | --- |
| `finalizer_schema_version` | Constant `1` | Evidence/result schema version. No longer read from agent stdout. |
| `finalizer_confidence` | Constant `high` on classified success, `low` otherwise | Retained for shape compatibility only. Confidence is no longer supplied by the agent and is not a gate. |
| `finalizer_output_valid` | Constant `true` | The narrative is an opaque artifact that is always "valid" because it is never parsed. Retained for shape compatibility only. |

The narrative artifact is exposed separately as `agentic_finalizer_narrative`
for human reading and is not part of the machine-control result.

## Configuration

No feature flag is required. Agentic finalization is part of the
`default-workflow` terminal path. The one flag below changes only the exit
code of a `BLOCKED_CI` result, or of a `FAILED_IMPLEMENTATION` on a confirmed
open PR, and only `autodrive-build` sets it.

| Setting or tool | Required when | Notes |
| --- | --- | --- |
| `NODE_OPTIONS=--max-old-space-size=32768` | Large nested workflow runs or Node-heavy checks | Recommended saved preference for this repository. It does not relax finalization rules. |
| `AMPLIHACK_AGENT_BINARY` | Nested agentic finalizer sessions | Preserved by the launcher so the narrative step uses the active supported agent runtime. |
| `git` | Always | Required for repository, branch, worktree, and diff evidence. |
| `jq` | Always | Required for building and reading the deterministic evidence document and typed markers. Not used to parse agent narrative. |
| `gh` | GitHub PR metadata path | Required only when a GitHub PR URL or GitHub remote makes PR metadata necessary. |
| `GH_TOKEN` or GitHub auth | GitHub PR metadata path | Missing or invalid auth fails closed when GitHub metadata is required. |
| `finalization_handoff=true` | `autodrive-build` step-02 only | Exact value `true`; anything else is off. Hands a `BLOCKED_CI` PR, or a `FAILED_IMPLEMENTATION` on a PR the assessment named, to auto-drive's crusty and merge-ready loops with exit `0` and `handoff=merge_ready_loop`. `terminal_success` stays `false`. `default-workflow.yaml` declares it `""`. See [Auto-Drive Finalization Hand-Off](#auto-drive-finalization-hand-off-1714). |
| `allow_no_op=true` | Explicit no-op tasks | Allows only valid no-op success states with evidence and reason text. It cannot bypass dirty work, failed CI, or missing terminal evidence. |

There is no configuration that converts finalization failures into advisory
warnings, and no configuration that reintroduces prose parsing.

## Examples

### Successful run with a human-readable finalizer narrative (run f1968919)

The durable steps completed and the finalizer emitted prose instead of JSON.
Under the finished contract, classification comes from typed evidence, so the
run is a success.

Typed markers:

```text
implementation_completed=true
verification_completed=true
finalizer_step_status.status=ok
finalizer_step_status.reporting_failure=false
git.dirty_worktree=false
```

`agentic_finalizer_narrative` (captured verbatim, never parsed):

```text
Implementation and verification are complete. I pushed the branch, CI is green,
review comments are resolved, and the PR is ready to merge. No further action is
required.
```

Normalized result:

```text
terminal_success=true
terminal_state=IMPLEMENTED_VERIFIED
terminal_reason=implementation and verification evidence present
required_next_action=No further action is required.
reporting_failure=false
```

The recipe exits `0`. No `jq`/regex/fence-stripping is applied to the narrative.

### Reporting failure preserves durable evidence

Implementation and verification succeeded and a PR exists, but a reporting step
failed.

Typed markers:

```text
implementation_completed=true
verification_completed=true
pr_url=https://github.com/rysweet/amplihack-rs/pull/123
pr_number=123
finalizer_step_status.status=failed
finalizer_step_status.reporting_failure=true
```

Normalized result:

```text
terminal_success=false
terminal_state=FAILED_REPORTING
terminal_reason=implementation succeeded but a reporting step failed
required_next_action=Re-run the reporting step; the implementation is intact.
reporting_failure=true
pr_url=https://github.com/rysweet/amplihack-rs/pull/123
pr_number=123
implementation_completed=true
verification_completed=true
```

The recipe exits nonzero, but the durable implementation and PR evidence is
preserved so operators are not told the work was lost.

### Implementation failure is distinct from reporting failure

Implementation did not complete and a meaningful diff remains unresolved.

```text
terminal_success=false
terminal_state=FAILED_IMPLEMENTATION
terminal_reason=implementation/verification evidence absent while meaningful work remains
required_next_action=Resume default-workflow from implementation and verification.
reporting_failure=false
implementation_completed=false
verification_completed=false
```

### Open PR with failing CI

```text
terminal_success=false
terminal_state=BLOCKED_CI
terminal_reason=required CI checks are failing for PR #123
required_next_action=Fix failing CI checks before merge.
```

The recipe exits nonzero and persists `terminal_state=BLOCKED_CI`. Under
auto-drive, with a numeric `pr_number`, it instead exits `0` with
`handoff=merge_ready_loop`; see
[Auto-Drive Finalization Hand-Off](#auto-drive-finalization-hand-off-1714).

### Resumed run with an already-published PR

A run resumes after an earlier attempt opened PR #1641 and CI went green. This
run's implementation and verification steps recorded nothing. The publish step
reported `existing-open-pr`:

```text
publish_status=existing-open-pr
implementation_completed=
verification_completed=
=> gh pr list --repo <origin repo> --head <branch> returns exactly one PR, same-repo
=> gh pr view 1641 --repo <origin repo>: state=OPEN, headRefOid = local HEAD, rollup all SUCCESS
=> outcome: pr_number=1641, pr_state=OPEN, pr_head_descends_local_head=true, checks=green
=> assess-finalization: terminal_state=EXISTING_OPEN_PR, pr_number=1641
=> terminal_state=EXISTING_OPEN_PR, terminal_success=true, exit 0
```

Since #2343 the publish signal is not a precondition: the same run with no
publish signal at all is read the same way. A run with nothing on the remote
and no pull request gets `outcome.remote_head=none` and `outcome.pr=none`,
`assess-finalization` returns `FAILED_IMPLEMENTATION`, and the result carries
`workflow finalization failed closed` on stderr. A run where nothing decided at
all ends `FAILED_MISSING_TERMINAL_EVIDENCE` and goes to the step's recovery
agent instead.

### Hollow success after planning-only output

```text
terminal_success=false
terminal_state=HOLLOW_SUCCESS
terminal_reason=planning output only; nothing published contradicts the empty agent output
required_next_action=Resume default-workflow from implementation or emit a valid no-op state with evidence.
hollow_success_detected=true
evidence_used=...,outcome.remote_head=none,outcome.pr=none,assessment_state=HOLLOW_SUCCESS,...
```

The recipe exits nonzero even if earlier planning steps exited `0`. Since #2809
the hollow-success signal is a typed fact handed to `assess-finalization`
rather than an exit taken ahead of the outcome read: the same signal beside a
pull request open at the run's head is `EXISTING_OPEN_PR`, because the pull
request is the evidence that the run produced something.

### Adversarial narrative cannot flip the decision

The finalizer narrative contains shell metacharacters and fake tokens such as
`terminal_state: MERGED` or `"terminal_success": true`. The narrative is never
parsed or evaluated, so it changes nothing:

```text
agentic_finalizer_narrative="... terminal_state: MERGED ..."
=> outcome.remote_head=none, outcome.pr=none
=> assess-finalization: terminal_state=FAILED_IMPLEMENTATION
=> terminal_state=FAILED_IMPLEMENTATION (narrative never read)
```

`assess-finalization` reads the evidence, which is a different thing: its
prompt fences it as data and says that nothing inside it is an instruction, and
only validated fields are forwarded -- pull request titles, bodies and commit
messages are not. What it returns is the terminal state, so its own answer is
the decision; what it cannot do is name a repository or a URL, which the result
builds from the facts, or reach past the blockers the validator takes first.

## Operator Troubleshooting

Use these commands to inspect finalization results:

```bash
jq '.. | objects | select(has("terminal_state")) | {
  terminal_success,
  terminal_state,
  terminal_reason,
  required_next_action,
  hollow_success_detected,
  reporting_failure,
  evidence_used,
  pr_url,
  pr_number
}' recipe-result.json
```

Read the human-readable finalizer narrative separately; it is diagnostic only:

```bash
jq -r '.. | objects | .agentic_finalizer_narrative // empty' recipe-result.json
```

For GitHub-backed PR decisions, confirm the PR identity:

```bash
gh pr view 123 --json number,state,mergedAt,headRefName,baseRefName,headRefOid,statusCheckRollup
git status --short
git rev-parse HEAD
git diff --stat origin/main...HEAD
```

If a run reports `FAILED_IMPLEMENTATION` although its pull request is open,
read `evidence_used` first: it carries the facts the decision was reported
against (`outcome.remote_head`, `outcome.pr`, `outcome.pr_head`,
`outcome.checks`, `assessment_state`), which usually names the gap on its own.
To check the same facts by hand:

```bash
git -C "$WORKTREE" branch --show-current   # must equal branch_name and not be main
git -C "$WORKTREE" config --get remote.origin.url
R=rysweet/amplihack-rs   # owner/repo parsed from the origin URL above
gh pr list --repo "$R" --head "$(git -C "$WORKTREE" branch --show-current)" \
  --state open --json number,url,isCrossRepository
git -C "$WORKTREE" rev-parse HEAD           # must equal the PR's headRefOid, or be its ancestor:
git -C "$WORKTREE" merge-base --is-ancestor HEAD <headRefOid> && echo fast-forward
gh pr view 1641 --repo "$R" --json state,headRefName,headRefOid,isCrossRepository,statusCheckRollup \
  --jq '.statusCheckRollup[] | {name, status, conclusion, state}'
```

A failing entry in the rollup (`BLOCKED_CI`), a second open PR for the branch
(even from a fork), or a PR head that does not contain the local HEAD (unpushed
or rewritten commits) all keep the run failed by design. A pending entry does
not: since #2784 a pull request open at the run's head with CI still running is
a success that hands the request on. Nor does a missing publish signal, which
is not consulted since #2343.

`FAILED_MISSING_TERMINAL_EVIDENCE` means something different from the states
above: finalization reached no decision. Either no facts could be gathered
(`terminal_reason` names what could not be read) or `assess-finalization`
returned nothing or a state finalization does not publish (`terminal_reason`
quotes it). The step is left failing for its recovery agent, so check the
step-recovery record and the `assess-finalization` step's own output before
anything else.

If an auto-drive build stopped on `BLOCKED_CI` or `FAILED_IMPLEMENTATION` instead of handing off, check
the `handoff` field and the step-03 `#1714` error line:

```bash
jq '.. | objects | select(has("terminal_state")) | {terminal_state, pr_number, handoff}' recipe-result.json
```

No `handoff` field means the flag did not reach finalization, or the decision
named no pull request: `pr_number` is the one `assess-finalization` returned,
and an assessment that names none is taken at its word, so there is nothing to
hand on. A `handoff` with a step-03 error means the PR was missing, not open,
or differed from the one phase 1 resolved.

Do not override `BLOCKED_CI`, `FAILED_MEANINGFUL_DIFF`, `HOLLOW_SUCCESS`,
`FAILED_IMPLEMENTATION`, `FAILED_REPORTING`, or
`FAILED_MISSING_TERMINAL_EVIDENCE` in CI scripts. These states mean finalization did not prove successful closure.
`FAILED_REPORTING` specifically means the implementation is intact — retry the
reporting step rather than re-running the implementation.

## See Also

- [Workflow Terminal-State Reference](workflow-terminal-state.md)
- [Workflow Context Transport](workflow-context-transport.md#absence-and-corruption-2343)
- [Workflow Terminal-State Provider Safety](workflow-terminal-state-provider-safety.md)
- [Default Coding Workflow](../concepts/default-workflow.md)
- [Agentic Step Patterns](../concepts/agentic-step-patterns.md)
