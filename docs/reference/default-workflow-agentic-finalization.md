# Default Workflow Agentic Finalization

> [Home](../index.md) > Reference > Default Workflow Agentic Finalization

`default-workflow` finalization is a fail-closed terminal assessment for
development workstreams. It is modeled as a sequence of typed recipe steps.
Deterministic steps collect evidence, emit typed recipe state, validate the
terminal decision, and choose the process exit code. An agentic finalizer step
contributes a human-readable narrative only.

**Terminal classification is a deterministic function of typed evidence.**
Finalization never parses agent-generated prose with `jq`, regex, fence
stripping, or JSON extraction to decide the outcome. The narrative the finalizer
agent produces is an output artifact for humans, not the machine-control
protocol. A fully successful run cannot be reported as failed merely because the
finalizer emitted human-readable text instead of parser-compatible JSON.

This page is the target implementation contract for the feature. If recipe,
helper, or test behavior differs, update the implementation to match this
contract rather than reintroducing a brittle parser that translates
unconstrained agent prose into workflow control flow.

The finalizer does not mutate Git state, create or edit pull requests, merge, or
decide success from free-form prose. Terminal success is reported only after the
deterministic finalization gate classifies a known terminal state from typed
evidence.

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
| `collect-finalization-evidence` | Deterministic shell/JSON | Read Git status, branch/base diff, provider-safe change-request metadata, CI state, implementation and verification markers, publish result, prior recipe state, missing tooling, and observed phases. | `finalization_evidence` (typed JSON, `schema_version: 1`) |
| `agentic-finalizer` | Agentic | Produce a human-readable narrative explaining the run and the recommended next action for operators. | `agentic_finalizer_narrative` (opaque text artifact) |
| `finalizer-step-status` | Deterministic shell/JSON | Record whether the narrative step itself completed or failed, as typed state. | `finalizer_step_status` (`{status, reporting_failure}`) |
| `validate-agentic-finalization` | Deterministic shell/JSON | Classify one terminal state from typed evidence and typed markers, applying guards before any success state. | `workflow_result` (normalized terminal result) |
| `workflow-complete` | Deterministic shell/JSON | Emit the human-readable summary and canonical result JSON, including the reporting-vs-implementation distinction. | `workflow_completion` |

The agentic finalizer is deliberately boxed out of control flow. It cannot make
an unsupported state successful, ignore dirty work, override failed CI, repair
invalid PR metadata, or flip a decision by emitting text such as
`terminal_state: MERGED`. Its narrative is never scanned for those tokens.

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
| Hollow success | A recipe exited `0` after setup, planning, empty agent output, or inaccessible codebase messages without implementation, verification, publish, or valid no-op evidence. | Return `HOLLOW_SUCCESS` or `FAILED_MISSING_TERMINAL_EVIDENCE`. |
| Implementation failure | Implementation or verification evidence is absent or failed while meaningful work remains. | Return `FAILED_IMPLEMENTATION`, distinct from a reporting failure. |

## Input Evidence Document

`validate-agentic-finalization` classifies the terminal state from one
normalized evidence document plus typed recipe markers. Producers collect the
data from recipe context, step outputs, shell helpers, Git, and provider
metadata, but the classifier sees a single JSON object.

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
| `agent_outputs` | object | Summaries needed to detect empty, inaccessible, or generic agent responses. |
| `prior_terminal_state` | object | Existing terminal markers from publish or terminal-state probes, when present. |

Evidence collectors must record absence explicitly. For example, a missing PR is
`"pr": {"present": false}`, not an omitted `pr` object.

`jq` is used only to build and read this **deterministic** evidence document and
the typed markers below. The prohibition on parsing applies solely to
**agent-generated narrative**.

## Typed Markers Consumed by the Classifier

In addition to `finalization_evidence`, the classifier reads typed recipe
markers produced by earlier deterministic steps.

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
means "a reporting step failed," nothing more. The **classifier**
(`validate-agentic-finalization`) is the sole owner of the reporting-vs-implementation
decision: it combines this marker with `implementation_completed` /
`verification_completed` evidence to choose `FAILED_REPORTING` (implementation
proven, reporting failed) versus `FAILED_IMPLEMENTATION` (implementation absent
or unproven). See [Implementation Failure vs Reporting Failure](#implementation-failure-vs-reporting-failure).

All markers are typed recipe state. None of them are derived by scanning the
finalizer narrative.

## Terminal Classification

`validate-agentic-finalization` computes `terminal_state` deterministically. It
applies guards before any success state, so a reporting failure or a dirty
worktree can never be reported as success.

Evaluation order:

1. **Guards (fail closed).** Dirty worktree, missing required tooling
   (`git`/`jq`, and `gh` when required), and deterministic prior blockers
   (`BLOCKED_CI`, `FAILED_MEANINGFUL_DIFF`, `FAILED_CLOSED_UNMERGED`,
   `FAILED_PR_METADATA_UNAVAILABLE`, `FAILED_INVALID_INPUT`,
   `FAILED_WRONG_BRANCH`) override any success-looking evidence.
2. **Hollow-success downgrade.** Empty, generic, or inaccessible agent output
   without implementation/verification/publish/no-op evidence resolves to
   `HOLLOW_SUCCESS`.
3. **Reporting vs implementation split.** If durable implementation and
   verification evidence is present but `finalizer_step_status.reporting_failure`
   is `true`, classify `FAILED_REPORTING` and preserve the durable evidence. If
   implementation/verification evidence is absent or failed while meaningful
   work remains, classify `FAILED_IMPLEMENTATION`.
4. **Success states.** With guards clear, classify the proven terminal success
   state (`MERGED`, `CLOSED_OBSOLETE`, `NO_DIFF_SUCCESS`, `FOLLOWUP_CREATED`,
   `SUPERSEDED`, `IMPLEMENTED_VERIFIED`, `ALLOW_NO_OP`) using the required
   deterministic proof for that state.
5. **Resumed-run open-PR path.** If steps 1–4 produced no result, a run whose
   PR is already published can still succeed as `EXISTING_OPEN_PR`. It needs an
   open same-repo PR on the run's branch, plus either all-green checks or, while
   CI has not reported (pending or no checks), this run's typed verification
   evidence. A failing check on that PR fails closed as `BLOCKED_CI`. See
   [Resumed Runs: Open-PR Success Path](#resumed-runs-open-pr-success-path-1641).
   This step only replaces the step 6 default. It never overrides steps 1–4.
6. **Fallthrough.** If no more specific state applies, the validator keeps its
   initial default, `FAILED_IMPLEMENTATION`. The validator never sets
   `INCOMPLETE`; that state reaches the result only through
   `prior_terminal_state`.

Malformed or contradictory `finalization_evidence` (for example, an object that
fails schema validation) fails closed as `FAILED_INVALID_EVIDENCE`. There is no
`FAILED_FINALIZER_OUTPUT` state; the narrative can no longer be malformed in a
way that matters because it is never parsed.

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
| `EXISTING_OPEN_PR` | Yes | The run did not record implementation evidence itself (usually because it resumed an earlier attempt), but its branch has a published, open, same-repo PR whose checks are all green, or whose CI has not reported yet (pending or no checks) while this run's typed verification evidence passed. A failing check is `BLOCKED_CI` instead. See [Resumed Runs](#resumed-runs-open-pr-success-path-1641). |
| `MANUAL_REQUIRED` | No | A provider action is intentionally manual and the final output names the required next action. |
| `BLOCKED_MANUAL_PROVIDER` | No | Provider tooling, credentials, permissions, or APIs block required automation. |
| `BLOCKED_CI` | No | Required checks are failing, pending beyond policy, or unavailable when required. On the resumed-run open-PR path, any failing check in the PR's `statusCheckRollup` (#1641). |
| `FAILED_DIRTY_WORKTREE` | No | Uncommitted or untracked work prevents terminal success. |
| `FAILED_MEANINGFUL_DIFF` | No | Meaningful branch changes remain but no validated publish, merge, follow-up, no-op, or implementation-plus-verification path proves closure. |
| `FAILED_CLOSED_UNMERGED` | No | PR is closed without merge evidence and meaningful branch diff remains. |
| `FAILED_PR_METADATA_UNAVAILABLE` | No | Provider change-request proof is required but metadata or auth is missing, stale, or ambiguous. |
| `FAILED_MISSING_TOOLING` | No | Required deterministic tooling such as `git`, `jq`, or provider CLI support is missing for the selected path. |
| `FAILED_INVALID_EVIDENCE` | No | Typed evidence is malformed, contradictory, unknown, or incomplete. |
| `FAILED_IMPLEMENTATION` | No | Durable implementation or verification evidence is absent or failed while meaningful work remains. |
| `FAILED_REPORTING` | No | Implementation succeeded but a reporting/finalization step failed. Durable evidence (`pr_url`, `pr_number`, implementation/verification markers) is preserved and reported. |
| `FAILED_MISSING_TERMINAL_EVIDENCE` | No | Development workflow stopped before implementation, verification, publish, or valid no-op evidence. |
| `HOLLOW_SUCCESS` | No | The recipe appeared successful but agents produced empty/generic output or could not access the codebase. |
| `INCOMPLETE` | No | Work remains and no more specific terminal failure state applies. Reaches the result only via `prior_terminal_state`; the validator never originates it. |

`terminal_failure=true` is derived for all non-success states. Failure states
override success-looking implementation, verification, publish, and no-op
markers.

`MANUAL_REQUIRED` and `BLOCKED_MANUAL_PROVIDER` are not emitted by the agentic
finalizer's own classifier. They belong to the broader terminal-state vocabulary
in `workflow-terminal-state.yaml` and reach the finalization result only via
`prior_terminal_state` propagation from an upstream deterministic probe. The
agentic classifier preserves them but never originates them.

> **Retired:** `FAILED_FINALIZER_OUTPUT` no longer exists. It described the
> brittle "agent output was not a single JSON object" failure that this feature
> removes. Reporting-step failures now classify as `FAILED_REPORTING`, and
> malformed deterministic evidence classifies as `FAILED_INVALID_EVIDENCE`.

## Implementation Failure vs Reporting Failure

The finished feature distinguishes two failure families that were previously
collapsed:

| | `FAILED_IMPLEMENTATION` | `FAILED_REPORTING` |
| --- | --- | --- |
| Meaning | The code change / verification did not complete. | The code change completed, but a reporting/finalization step failed. |
| `implementation_completed` | `false` or unproven | `true` |
| `verification_completed` | `false` or unproven | `true` |
| Durable evidence (`pr_url`, `pr_number`) | Not required | Preserved and reported when present |
| `terminal_success` | `false` | `false` |
| Operator action | Resume implementation/verification. | Retry the reporting step; the implementation is intact. |

This split ensures that free-form explanatory output — or a transient failure in
a reporting step — can never erase proof that the implementation succeeded.

The distinction is computed by the classifier, not by the reporting step. The
`finalizer-step-status` marker only reports whether a reporting step failed;
`validate-agentic-finalization` then combines that marker with typed
implementation and verification evidence. `reporting_failure=true` resolves to
`FAILED_REPORTING` only when implementation and verification are independently
proven; otherwise it resolves to `FAILED_IMPLEMENTATION`.

## Deterministic Validation Rules

After evidence is collected, the deterministic classifier validates typed state
before persisting or reporting the result.

1. `finalization_evidence` must be a single JSON object using schema version
   `1`. Malformed or unknown-version evidence fails closed as
   `FAILED_INVALID_EVIDENCE`.
2. Guards run first: dirty worktree, missing required tooling, and deterministic
   prior blockers override success-looking evidence.
3. Hollow success downgrades success-looking runs that lack meaningful
   completion evidence.
4. Reporting failure (`finalizer_step_status.reporting_failure=true`) with
   present implementation/verification evidence classifies `FAILED_REPORTING`
   and preserves durable identifiers.
5. Implementation/verification absence with remaining meaningful work classifies
   `FAILED_IMPLEMENTATION`.
6. Success states require their deterministic proof: `MERGED` needs merge
   evidence, `NO_DIFF_SUCCESS` needs clean no-diff proof, `CLOSED_OBSOLETE`
   needs local no-diff or obsolete proof, `FOLLOWUP_CREATED` needs a durable
   follow-up identifier, `SUPERSEDED` needs a durable replacement identifier plus
   a supersession reason, `IMPLEMENTED_VERIFIED` needs implementation and
   verification evidence, `ALLOW_NO_OP` needs explicit no-op authorization
   plus reason text, and `EXISTING_OPEN_PR` needs a publish signal, a live
   GitHub check that the PR is open, same-repo and on the run's branch, and
   either all-green checks or `verification_completed=true`.
7. The narrative artifact is never read for any of these decisions.
8. The normalized result is persisted even for failure states so operators can
   diagnose the run from recipe JSON.

The classifier is total: every input maps to exactly one terminal state. Inside
`validate_finalization` the fallthrough is `FAILED_IMPLEMENTATION` (evaluation
order step 6). When validation produces no result at all, the default is
`FAILED_INVALID_EVIDENCE`, not the retired `FAILED_FINALIZER_OUTPUT`. The
completion helper (`complete()` in `workflow_agentic_finalization.sh`) must
initialize its `terminal_state` default to `FAILED_INVALID_EVIDENCE` so that an
unclassifiable or empty-evidence run fails closed on a live state rather than a
retired one.

## Resumed Runs: Open-PR Success Path (#1641)

A run that resumes an earlier attempt's work often has nothing left to
implement. The PR already exists and CI is already green, so this run's
implementation and verification steps never set `implementation_completed` or
`verification_completed`. Before #1641, `validate-agentic-finalization` then
fell through to the default `FAILED_IMPLEMENTATION`, even though step 22b
(`workflow_final_status.sh`, since #1623) had already reported the same run as a
success. PRs #1635 and #1639 hit this.

Step 5 of the [evaluation order](#terminal-classification) closes that gap. It classifies
`EXISTING_OPEN_PR` only when every condition below holds. If any one fails, the
run stays `FAILED_IMPLEMENTATION`, exactly as before, with one exception: when
conditions 1–6 hold and the confirmed PR has a failing check, the result is
`BLOCKED_CI`, because a red check is a negative signal and the run must not
look like it only lacked evidence.

### Conditions

| # | Condition | Source |
| --- | --- | --- |
| 1 | Evaluation-order steps 1–4 returned no result: no invalid evidence, dirty worktree, missing tooling, prior blocker, hollow success, reporting-failure split, `IMPLEMENTED_VERIFIED` or `ALLOW_NO_OP`. | Classifier order |
| 2 | **Publish signal.** A non-empty `pr_url` or `pr_number`, `publish_state_reached=true`, or a `publish_status` or `prior_terminal_state` of `EXISTING_OPEN_PR`, `existing-open-pr`, `FOLLOWUP_CREATED`, `MERGED` or `closed-after-merge`. | Typed context (`workflow_finalization_context.sh`) |
| 2a | **Valid metadata.** None of `worktree_path`, `repo_path`, `pr_url`, `pr_number`, `publish_state_reached`, `prior_terminal_state`, `publish_status`, `branch_name` or `remote_host_type` is invalid authoritative context (wrong JSON type, NUL byte). An invalid value would otherwise read as empty, so any one disables this path. | `workflow_finalization_metadata` status 3 |
| 3 | **Explicit target directory.** `worktree_path` or `repo_path` resolved. The `.` fallback never qualifies. | Recipe metadata |
| 4 | **GitHub remote.** `remote_host_type` is empty or `github` (any of `github`, `GitHub`, `GITHUB`), and `remote.origin.url` of the target directory parses to `owner/repo`. | Local git config |
| 5 | **Run branch.** `branch_name` and `git branch --show-current` agree when both are set. The branch is not `main`, `master` or `HEAD`, does not start with `-`, and is a valid branch name under `git check-ref-format "refs/heads/<branch>"`. | Metadata + live git |
| 6 | **Live PR state.** `gh pr view --repo <owner>/<repo>` reports `state=OPEN`, `headRefName` equal to the run branch, `headRefOid` equal to the local `git rev-parse HEAD` (fails closed if HEAD cannot be resolved), `isCrossRepository=false`, the requested number, and `url` exactly `https://github.com/<owner>/<repo>/pull/<n>`. | `gh` read-only + live git |
| 7 | **Verification signal.** Either `checks=green` (every rollup entry passed, see below), or `checks` is `pending` or `none` (CI has not reported) and `verification_completed=true` from this run's typed `verification_terminal_evidence`. `checks=failing` never qualifies, whatever `verification_completed` says: it is `BLOCKED_CI`. `checks=unknown` never qualifies either. | `statusCheckRollup` or typed evidence |

Each kind of signal is insufficient on its own. A publish status without a live
open PR fails. An open PR with no green checks and no verification evidence
fails. An open PR with a failing check is `BLOCKED_CI` even with verification
evidence: this run's local pre-commit run does not override CI. A green rollup on another branch's PR fails. A green rollup on an older
commit fails too: if the run committed locally but never pushed, the PR's
`headRefOid` differs from local HEAD, so checks (and this run's precommit
evidence) that never covered the PR's code cannot vouch for it.

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
unauthenticated or failing `gh`, or malformed JSON, means "no signal": the run
stays `FAILED_IMPLEMENTATION` and no new failure state is introduced.

### What "green" means

`checks=green` requires a non-empty `statusCheckRollup` in which:

- every `CheckRun` has `status=COMPLETED` and `conclusion` of `SUCCESS`,
  `NEUTRAL` or `SKIPPED`;
- every `StatusContext` has `state=SUCCESS`.

A pending, failing or null entry, an unknown type, or an empty rollup is not
green. Every entry counts, not only required checks: `gh pr view` does not say
which checks are required, so the evidence is labelled `all_checks=green`, not
`required_checks=green`. The typed `verification_completed=true` signal can
stand in for CI only while CI has not reported (`pending` or `none`). It never
overrides a `failing` entry, which classifies `BLOCKED_CI`, and never an
`unknown` one, which stays `FAILED_IMPLEMENTATION`.

### Result

On success the canonical result keeps its schema of 19 string keys and exits 0:

```text
terminal_success=true
terminal_state=EXISTING_OPEN_PR
terminal_reason=<illustrative: final wording is set by the implementation>
required_next_action=No action required.
publish_state_reached=true
pr_number=1641
pr_url=https://github.com/rysweet/amplihack-rs/pull/1641
hollow_success_detected=false
reporting_failure=false
evidence_used=publish_state_reached=true,pr_state=OPEN,pr_head_matches_branch=true,pr_head_sha_matches=true,pr_number=1641,all_checks=green
```

The `terminal_reason` line above is a placeholder; its exact text is defined
by the code, not by this page. Every other line is the specified output.

When CI has not reported yet (`checks` is `pending` or `none`) and this run's
precommit verification passed, the run still succeeds, but the tail of
`evidence_used` is `precommit_passed=true,all_checks=pending` (or
`all_checks=none`) instead of `all_checks=green`, and `required_next_action` is
not "No action required.". It tells the next step to watch CI on the PR and to
treat any failing check as `BLOCKED_CI`:

```text
required_next_action=Watch CI on PR #1641: checks are pending and only this run's pre-commit verification vouches for it; a failing check is BLOCKED_CI.
evidence_used=...,pr_number=1641,precommit_passed=true,all_checks=pending
```

Those three tails (`all_checks=green`, `precommit_passed=true,all_checks=pending`
and `precommit_passed=true,all_checks=none`) are the only ones a success can
carry.

When the confirmed PR has a failing check, the result is `BLOCKED_CI` and exits
nonzero. `pr_number` and `pr_url` are still the confirmed values, so the next
step knows which PR to fix:

```text
terminal_success=false
terminal_state=BLOCKED_CI
required_next_action=Fix the failing CI checks on PR #1641 before merge.
evidence_used=...,pr_number=1641,all_checks=failing,verification_completed=<true|false>
```

`pr_number` and `pr_url` in the result are the values `gh` confirmed. They are
not copied from the input context. `publish_state_reached=true` comes from an
internal override that `validate_finalization` clears on entry, so it cannot be
set from the environment.

`workflow-complete` accepts `EXISTING_OPEN_PR` as success. It is listed in
`terminal_vocabulary`, and `workflow_final_status.sh` already treats it as
success.

### Fail-closed cases kept

| Situation | Terminal state | `gh` called? |
| --- | --- | --- |
| No PR, no publish state, no evidence | `FAILED_IMPLEMENTATION` | No |
| Open PR, any check failing (with or without `verification_completed=true`) | `BLOCKED_CI` | Yes |
| Open PR, a check pending, `verification_completed` unset | `FAILED_IMPLEMENTATION` | Yes |
| Open PR, an unclassifiable check (unknown type or conclusion), with or without `verification_completed=true` | `FAILED_IMPLEMENTATION` | Yes |
| PR `CLOSED` or `MERGED` | `FAILED_IMPLEMENTATION` | Yes |
| PR head ≠ run branch, or cross-repository PR | `FAILED_IMPLEMENTATION` | Yes |
| `pr_url` names a different repo than `origin` and no valid `pr_number` is set (no `pr list` fallback) | `FAILED_IMPLEMENTATION` | No |
| `gh` missing, unauthenticated, non-zero exit or malformed JSON | `FAILED_IMPLEMENTATION` | Attempted |
| `pr list` returns zero PRs, two PRs, or one cross-repository PR | `FAILED_IMPLEMENTATION` | Yes |
| Target directory fell back to `.` or is a `.git` directory / bare repo, branch is `main`, or remote is AzDO | `FAILED_IMPLEMENTATION` | No |
| Dirty worktree with an open green PR | `FAILED_DIRTY_WORKTREE` | No |
| Hollow-success signals with an open green PR | `HOLLOW_SUCCESS` | No |
| `prior_terminal_state=BLOCKED_CI` with an open green PR | `BLOCKED_CI` | No |
| `reporting_failure=true` | `FAILED_REPORTING` / `FAILED_IMPLEMENTATION` (unchanged) | No |

The path never reads `agentic_finalizer_narrative` or any other prose, and it
does not add fields to the `workflow_finalization_field` allowlist.

### Helper: `finalization_open_pr_signal`

`amplifier-bundle/tools/workflow_finalization_pr_signal.sh` is sourced by
`workflow_agentic_finalization.sh` together with `workflow_gh_retry.sh`. It
reports facts and does not classify.

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
{"pr_number":"1641","pr_url":"https://github.com/rysweet/amplihack-rs/pull/1641","checks":"green"}
```

`checks` is one of `green`, `failing`, `pending`, `none` or `unknown`. Exit 1
prints nothing on stdout and means no usable open-PR signal; stderr gets one
fixed line, `WARNING: no open-PR signal: <reason>`, naming the check that
failed (for example `PR reference has an invalid PR number` or `local HEAD
could not be resolved`). Reasons are literal strings: no URL, branch name or
`gh` output is ever interpolated. The helper never prints
`gh` stderr or the origin URL. It uses a private `mktemp -d` directory for retry
scratch files and removes it on every return path. If `mktemp` fails, it
returns 1 rather than writing into the working directory.

### Known limitations

- **Every reported check, not just the required ones.** A run whose PR has an
  irrelevant (non-required) failing check is `BLOCKED_CI`, even with
  `verification_completed=true`. That is deliberate: `gh pr view` cannot tell
  which checks are required, and a classifier that cannot tell must not call a
  red build done. Fix or remove the failing check, then re-run finalization.
- **Point-in-time.** The PR can change after validation. Finalization reports
  what it saw; it is not a merge gate.
- **AzDO.** Resumed AzDO runs are not checked against provider CI. Without the
  GitHub path they stay `FAILED_IMPLEMENTATION`.
- **Local origin is trusted.** Repo identity comes from the worktree's
  `remote.origin.url`.
- **Detached HEAD.** With no live branch, only the context `branch_name` is
  checked against the PR's `headRefName`. The PR must still be open, from the
  same repo, have `headRefOid` equal to the detached HEAD, and pass
  verification.
- **Empty `remote_host_type`.** Treated as GitHub. This cannot widen the check:
  the origin URL must still parse as a `github.com` repository, and userinfo
  containing `/`, `?`, `#` or `\` is rejected so a foreign host cannot hide
  behind `@github.com/`.

## Auto-Drive Hand-Off of BLOCKED_CI (#1714)

`BLOCKED_CI` normally exits non-zero, which ends any recipe that called
`default-workflow`. Under [auto-drive](auto-drive-to-merge.md#blocked_ci-hand-off-from-phase-1-1714)
the failing checks are exactly what the later merge-ready loop fixes, so
`autodrive-build` asks finalization to hand the PR on instead of ending the
run. The classification does not change: the state is still `BLOCKED_CI` and
`terminal_success` is still `"false"`. Only the exit code and one field differ.

### Conditions

The validator hands off only when all three hold. Otherwise it fails closed
exactly as before.

| # | Condition | Source |
| --- | --- | --- |
| 1 | The state is `BLOCKED_CI`, matched exactly by `blocked_ci_handoff_allowed`. | `workflow_blocked_ci_handoff.sh` |
| 2 | `RECIPE_VAR_blocked_ci_handoff` is exactly `true`. Only `autodrive-build` step-02 sets it; `default-workflow.yaml` declares it `""`. | `blocked_ci_handoff_flag` |
| 3 | `pr_number` passes `_finalization_pr_number_valid`. On the open-PR path this is the number `gh` confirmed; on the prior-state path it is the run's own PR evidence. | Validator state |

Both `BLOCKED_CI` exits use it: the prior-state probe and the open-PR check
with a failing check. The five other prior-state blockers
(`FAILED_MEANINGFUL_DIFF`, `FAILED_CLOSED_UNMERGED`,
`FAILED_PR_METADATA_UNAVAILABLE`, `FAILED_INVALID_INPUT`,
`FAILED_WRONG_BRANCH`) and every other failure keep `exit 1` with the flag on.

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

stderr gets `INFO: #1714: BLOCKED_CI handed to the merge-ready loop (PR #134)`.
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
`amplifier-bundle/tools/workflow_blocked_ci_handoff.sh`, which
`workflow_agentic_finalization.sh` sources after
`workflow_finalization_pr_signal.sh`. It calls `fail_result` and
`emit_result`, which are nested functions of `validate_finalization`, so it
may be called only from inside `validate_finalization`. When the conditions
above do not hold it calls `fail_result "$@"` and never returns.

### Standalone runs

A standalone `default-workflow`, and every other caller of it, never hands
off. `default-workflow.yaml` declares `blocked_ci_handoff: ""`. An exported
`RECIPE_VAR_blocked_ci_handoff=true` does not switch the hand-off on, provided
the recipe runner keeps giving a recipe's declared context priority over an
inherited `RECIPE_VAR_` value; see
[`blocked_ci_handoff`](./workflow-terminal-state.md#blocked_ci_handoff).

The hand-off also depends on the opposite case: the value `"true"` that
`autodrive-build` step-02 passes to its `default-workflow` sub-recipe must
override `default-workflow.yaml`'s declared `""`. If the runner does not
apply it, the flag is never on, the hand-off never happens, and an auto-drive
build that ends `BLOCKED_CI` fails closed exactly as it did before #1714.

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
| `reporting_failure` | `true` when a reporting/finalization step itself failed (from that step's exit status); combined with implementation evidence by the classifier to select `FAILED_REPORTING`. |
| `implementation_completed` | Normalized implementation evidence. |
| `verification_completed` | Normalized verification evidence. |
| `publish_state_reached` | Normalized publish/PR/follow-up evidence. |
| `terminal_no_op` | Whether the final state is an explicit no-op success. |
| `terminal_failure` | `true` for all non-success terminal states. |
| `change_request_url` | Provider change-request URL when a validated change request or follow-up exists. |
| `change_request_id` | Provider change-request identifier when available. |
| `pr_url` | GitHub compatibility URL when a validated GitHub PR or follow-up exists. Preserved on `FAILED_REPORTING`. |
| `pr_number` | GitHub compatibility number when a validated GitHub PR exists. Preserved on `FAILED_REPORTING`. |
| `handoff` | Present only on an [auto-drive hand-off](#auto-drive-hand-off-of-blocked_ci-1714), with the value `merge_ready_loop`. Absent, never `""`, on every other result. It never changes `terminal_success`. |

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
code of a `BLOCKED_CI` result, and only `autodrive-build` sets it.

| Setting or tool | Required when | Notes |
| --- | --- | --- |
| `NODE_OPTIONS=--max-old-space-size=32768` | Large nested workflow runs or Node-heavy checks | Recommended saved preference for this repository. It does not relax finalization rules. |
| `AMPLIHACK_AGENT_BINARY` | Nested agentic finalizer sessions | Preserved by the launcher so the narrative step uses the active supported agent runtime. |
| `git` | Always | Required for repository, branch, worktree, and diff evidence. |
| `jq` | Always | Required for building and reading the deterministic evidence document and typed markers. Not used to parse agent narrative. |
| `gh` | GitHub PR metadata path | Required only when a GitHub PR URL or GitHub remote makes PR metadata necessary. |
| `GH_TOKEN` or GitHub auth | GitHub PR metadata path | Missing or invalid auth fails closed when GitHub metadata is required. |
| `blocked_ci_handoff=true` | `autodrive-build` step-02 only | Exact value `true`; anything else is off. Hands a `BLOCKED_CI` PR to auto-drive's merge-ready loop with exit `0` and `handoff=merge_ready_loop`. `terminal_success` stays `false`. `default-workflow.yaml` declares it `""`. See [Auto-Drive Hand-Off of BLOCKED_CI](#auto-drive-hand-off-of-blocked_ci-1714). |
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
[Auto-Drive Hand-Off of BLOCKED_CI](#auto-drive-hand-off-of-blocked_ci-1714).

### Resumed run with an already-published PR

A run resumes after an earlier attempt opened PR #1641 and CI went green. This
run's implementation and verification steps recorded nothing. The publish step
reported `existing-open-pr`:

```text
publish_status=existing-open-pr
implementation_completed=
verification_completed=
=> gh pr list --repo <origin repo> --head <branch> returns exactly one PR, same-repo
=> gh pr view 1641 --repo <origin repo>: state=OPEN, head matches, all checks SUCCESS
=> terminal_state=EXISTING_OPEN_PR, terminal_success=true, exit 0
```

If the same run had no publish signal at all, `gh` is never called and the
result is `terminal_state=FAILED_IMPLEMENTATION` with
`workflow finalization failed closed` on stderr.

### Hollow success after planning-only output

```text
terminal_success=false
terminal_state=HOLLOW_SUCCESS
terminal_reason=planning output only; no implementation, verification, publish, or valid no-op evidence
required_next_action=Resume default-workflow from implementation or emit a valid no-op state with evidence.
hollow_success_detected=true
```

The recipe exits nonzero even if earlier planning steps exited `0`.

### Adversarial narrative cannot flip the decision

The finalizer narrative contains shell metacharacters and fake tokens such as
`terminal_state: MERGED` or `"terminal_success": true`. Because the narrative is
never parsed or evaluated, the classification is unchanged and derives only from
typed evidence.

```text
implementation_completed=false
verification_completed=false
=> terminal_state=FAILED_IMPLEMENTATION (narrative tokens ignored)
```

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

If a resumed run reports `FAILED_IMPLEMENTATION` although its PR is open, check
each [open-PR condition](#conditions) in turn:

```bash
git -C "$WORKTREE" branch --show-current   # must equal branch_name and not be main
git -C "$WORKTREE" config --get remote.origin.url
R=rysweet/amplihack-rs   # owner/repo parsed from the origin URL above
gh pr list --repo "$R" --head "$(git -C "$WORKTREE" branch --show-current)" \
  --state open --json number,url,isCrossRepository
git -C "$WORKTREE" rev-parse HEAD           # must equal the PR's headRefOid
gh pr view 1641 --repo "$R" --json state,headRefName,headRefOid,isCrossRepository,statusCheckRollup \
  --jq '.statusCheckRollup[] | {name, status, conclusion, state}'
```

A failing entry in the rollup (reported as `BLOCKED_CI`), a pending entry
without this run's verification evidence, a second open PR for the branch (even
from a fork), a PR head that is not the local HEAD (unpushed commits), a
`pr_url` for another repo, or a missing publish signal all keep
the run failed by design.

If an auto-drive build stopped on `BLOCKED_CI` instead of handing off, check
the `handoff` field and the step-03 `#1714` error line:

```bash
jq '.. | objects | select(has("terminal_state")) | {terminal_state, pr_number, handoff}' recipe-result.json
```

No `handoff` field means the flag did not reach finalization or `pr_number`
was not numeric. A `handoff` with a step-03 error means the PR was missing,
not open, or differed from the one phase 1 resolved.

Do not override `BLOCKED_CI`, `FAILED_MEANINGFUL_DIFF`, `HOLLOW_SUCCESS`,
`FAILED_IMPLEMENTATION`, `FAILED_REPORTING`, or `FAILED_INVALID_EVIDENCE` in CI
scripts. These states mean finalization did not prove successful closure.
`FAILED_REPORTING` specifically means the implementation is intact — retry the
reporting step rather than re-running the implementation.

## See Also

- [Workflow Terminal-State Reference](workflow-terminal-state.md)
- [Workflow Terminal-State Provider Safety](workflow-terminal-state-provider-safety.md)
- [Default Coding Workflow](../concepts/default-workflow.md)
- [Agentic Step Patterns](../concepts/agentic-step-patterns.md)
