# Non-Fatal Documentation Review Checkpoint Reference

> [Home](../index.md) > Reference > Non-Fatal Documentation Review Checkpoint

Field-level contract for the non-fatal documentation-review behavior introduced
by issue [#834](https://github.com/rysweet/amplihack-rs/issues/834) in
`amplifier-bundle/recipes/workflow-design.yaml`.

## Contents

- [Affected Steps](#affected-steps)
- [step-06b-documentation-review](#step-06b-documentation-review)
- [Documentation-Review Checkpoint Step](#documentation-review-checkpoint-step)
- [Checkpoint Output Contract](#checkpoint-output-contract)
- [Durable Artifact References](#durable-artifact-references)
- [Summary Markers](#summary-markers)
- [Propagation Through default-workflow and smart-orchestrator](#propagation-through-default-workflow-and-smart-orchestrator)
- [Security Invariants](#security-invariants)
- [Scope and Non-Goals](#scope-and-non-goals)

---

## Affected Steps

The feature touches the documentation phase of `workflow-design.yaml` only.
Steps are listed in execution order:

| Order | Step | Role | Change |
| --- | --- | --- | --- |
| 1 | `step-06a-documentation` | Writes retcon documentation. | Unchanged. |
| 2 | `step-06b-documentation-review` | Reviews the documentation. | `continue_on_error: true` added so a non-zero exit no longer aborts the workflow. |
| 3 | `step-06c-documentation-refinement` | Refines documentation. | Unchanged; already `continue_on_error: true`. |
| 4 | `step-06b-checkpoint-doc-review` | Records partial success and follow-up. | New `type: bash` step, inserted **after** `step-06c` and before `step-06d`. Always exits `0`. |
| 5 | `step-06d-goal-already-met-probe` | Pre-flight goal probe. | Unchanged. |

**Note on the `06b` prefix.** The checkpoint id is
`step-06b-checkpoint-doc-review` but it executes *after* `step-06c`. The `06b`
prefix denotes its *semantic association* with the `step-06b` documentation
review whose feedback it inspects — not its execution position. It is placed
after `step-06c` so refinement runs first, and the checkpoint observes the
final state of the documentation phase before the goal probe.

`default-workflow.yaml` and `smart-orchestrator.yaml` are unchanged. They
inherit the behavior because they compose `workflow-design`.

---

## step-06b-documentation-review

```yaml
- id: "step-06b-documentation-review"
  agent: "amplihack:architect"
  working_dir: "{{worktree_setup.worktree_path}}"
  continue_on_error: true
  prompt: |
    # Step 6b: Documentation Review
    ...
  output: "doc_review_feedback"
```

| Field | Value | Meaning |
| --- | --- | --- |
| `continue_on_error` | `true` | A non-zero exit is recorded but does not abort the parent workflow. |
| `output` | `doc_review_feedback` | Review feedback string consumed by the checkpoint and by `step-06c`. May be empty when the agent step failed. |

This mirrors the canonical non-fatal pattern already used by
`step-06c-documentation-refinement`.

---

## Documentation-Review Checkpoint Step

A `type: bash` checkpoint step runs after `step-06c` and is independent of it.
Its contract:

| Property | Value |
| --- | --- |
| `id` | `step-06b-checkpoint-doc-review` |
| `type` | `bash` |
| `output` | `doc_review_checkpoint` |
| Exit status | Always `0` (structural — the step never aborts the workflow). |
| Shell mode | `set -uo pipefail` (no `-e`). |
| stderr | Human-readable `WARNING:` diagnostic when review failed. |
| stdout | Machine-consumable summary consumed as the `output`. |

### Decision Logic

The checkpoint classifies `doc_review_feedback` by matching fixed keyword
patterns. The matches are evaluated in this order:

| # | Condition on `doc_review_feedback` | stdout summary | stderr |
| --- | --- | --- | --- |
| 1 | Empty (or whitespace only) | `NEEDS_ATTENTION: doc-review` + refs | `WARNING: ... feedback unavailable` |
| 2 | Matches a failure pattern (`fail`, `error`, `reject`, `block`, `must fix`) | `NEEDS_ATTENTION: doc-review` + refs | `WARNING: ... review reported failure` |
| 3 | Matches a success pattern (`approve`, `pass`, `lgtm`, `looks good`, `no changes needed`) | `OK: doc-review` | none |
| 4 | Present but matches neither (ambiguous / neutral) | `OK: doc-review` | none |

**Default for ambiguous feedback (row 4) is `OK`.** Rationale: the step only
exists to convert *review failure that lands after durable side effects* into a
visible follow-up. Empty feedback (row 1) is treated as a failure-to-produce
signal because the review step is known to have exited non-zero with no usable
output. Non-empty feedback that names no failure keyword is treated as benign
(`OK`) rather than escalated, so the checkpoint never manufactures a
`NEEDS_ATTENTION` item from neutral prose. Failure detection (row 2) is
evaluated before success (row 3) so mixed feedback containing an explicit
failure keyword is correctly flagged.

The feedback is consumed strictly as data via `printf '%s' "$DOC_FEEDBACK" |
grep -qiE ...`. Verbatim feedback is never interpolated into the summary; only
fixed markers are emitted.

---

## Checkpoint Output Contract

`doc_review_checkpoint` is a single string propagated to the parent context and
the run summary.

### OK shape

```text
OK: doc-review
```

### NEEDS_ATTENTION shape

```text
NEEDS_ATTENTION: doc-review
  branch:        <branch or omitted>
  pr:            <owner/repo#number (url) or omitted>
  commit:        <sha or omitted>
  review_thread: <thread/comment id or omitted>
  follow_up:     Re-run documentation review for the listed PR before close.
```

Each artifact line is emitted only when its source reference is present in
context. Absent references are omitted entirely; the checkpoint never prints a
placeholder, never errors on an unset variable, and never fabricates a value.

---

## Durable Artifact References

The checkpoint surfaces an allow-listed set of non-sensitive metadata
references. Each is read through a guarded expansion (`${VAR:-}` with fallback
chains) so missing references are skipped, not fatal. Context outputs reach the
bash step as flattened, upper-cased environment variables, following the same
convention the runner already uses for `worktree_setup.worktree_path` →
`WORKTREE_SETUP_WORKTREE_PATH` (see `default-workflow.yaml`).

| Reference | Source variable (with fallbacks) | Example | Notes |
| --- | --- | --- | --- |
| `branch` | `${BRANCH_NAME:-${WORKTREE_SETUP_BRANCH:-}}` | `feat/issue-834-non-fatal-doc-review` | Recovery/working branch name. `branch_name` is a declared `default-workflow` output; `worktree_setup.branch` is the step-04 fallback. |
| `pr` | `${PR_URL:-}` / `${PR_NUMBER:-}` | `rysweet/amplihack-rs#841` + URL | PR url and/or number when a PR was opened or merged in this or a prior round. |
| `commit` | `${COMMIT_SHA:-${HEAD_SHA:-}}` | `8fb46865fb4412038b9313a62c02cc5aa0693132` | Pushed commit sha. |
| `review_thread` | `${REVIEW_THREAD_ID:-${REVIEW_COMMENT_ID:-}}` | `1987654321` | Posted review thread or comment id. |

The review feedback itself is read from `${DOC_REVIEW_FEEDBACK:-}` (the
`output: doc_review_feedback` from `step-06b`).

Every reference is optional. A fresh round typically has only `branch` (from
`worktree_setup`) populated at this point; `pr`, `commit`, and `review_thread`
are present mainly on a resumed round where a prior round already pushed and
published. Each guarded expansion defaults to empty, so any reference not yet in
context is omitted rather than printed blank or treated as an error.

Only this allow list is surfaced. Arbitrary `RECIPE_VAR_*` values are not echoed
wholesale. No tokens, `GITHUB_TOKEN`, or environment/`set` dumps are printed.

---

## Summary Markers

Two fixed markers express run state:

| Marker | Stream | Meaning |
| --- | --- | --- |
| `WARNING:` | stderr | Human-facing diagnostic naming the failed step and reason. |
| `NEEDS_ATTENTION:` | stdout (summary) | Machine-consumable follow-up flag with the artifact references. |
| `OK:` | stdout (summary) | Documentation review succeeded; no follow-up required. |

A degraded-success run is identified by the presence of `NEEDS_ATTENTION:
doc-review` in the summary alongside the normal evidence of completed
implementation, verification, and PR work.

---

## Propagation Through default-workflow and smart-orchestrator

```text
smart-orchestrator
  └─ default-workflow
       └─ workflow-design
            ├─ step-06b-documentation-review     (continue_on_error: true)
            ├─ step-06c-documentation-refinement (continue_on_error: true)
            ├─ step-06b-checkpoint-doc-review    (output: doc_review_checkpoint)
            └─ step-06d-goal-already-met-probe
```

The checkpoint runs after refinement (`step-06c`) and before the goal probe
(`step-06d`), even though its id carries the `06b` semantic prefix.

Because `continue_on_error: true` stops the failure from propagating, the parent
recipes reach their reconciliation/summary phase. The `doc_review_checkpoint`
output flows up so the top-level summary lists the degraded-success detail. No
change to `default-workflow.yaml` or `smart-orchestrator.yaml` is needed.

---

## Security Invariants

The checkpoint step enforces these invariants:

- **SR1** — No `eval`, `source`, or dynamic command construction. `${!var}`,
  `${var@P}`, and chained-substitution constructs are rejected.
- **SR2** — Every expansion is double-quoted (`"$DOC_FEEDBACK"`, `"$PR_URL"`).
- **SR3** — Untrusted feedback and refs are printed with `printf '%s' "$X"`,
  never `printf "$X"` or `echo $X`, to prevent format-string injection.
- **SR4** — `set -uo pipefail` without `-e`; the step exits `0` structurally.
- **SR5** — Feedback is consumed only as data via `printf '%s' ... | grep`.
  Verbatim feedback is never interpolated into the summary, preventing marker
  spoofing.
- **SR6** — No secret leakage. Tokens, `GITHUB_TOKEN`, and env/`set` dumps are
  never printed. Surfaced refs are non-sensitive metadata only.
- **SR7** — Allow-list refs only; arbitrary `RECIPE_VAR_*` values are not echoed.
- **SR8** — stderr (the `WARNING` diagnostic) and stdout (the machine-consumed
  `output`) are kept separate.

---

## Scope and Non-Goals

| In scope | Out of scope |
| --- | --- |
| Softening the post-side-effect doc-review failure path. | Changing documentation review logic. |
| Surfacing durable artifact refs in the summary. | Relaxing terminal-state / finalization gates (stay fail-closed). |
| Emitting `WARNING` + `NEEDS_ATTENTION` markers. | Softening pre-side-effect failures (no completed work to protect). |
| YAML-only change to `workflow-design.yaml`. | Any Rust, dependency, or `Cargo.toml` version change. |

The workspace version stays `0.11.1`; `--locked` CI is unaffected.

---

## See Also

- [Non-fatal documentation review checkpoint overview](../features/doc-review-non-fatal-checkpoint.md)
- [How to configure the documentation-review checkpoint](../howto/configure-doc-review-checkpoint.md)
- [Tutorial: degraded-success after a failed doc review](../tutorials/doc-review-non-fatal-checkpoint.md)
