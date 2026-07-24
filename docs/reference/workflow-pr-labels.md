# Best-Effort PR Labeling Reference

This reference documents the best-effort PR labeling helper that stamps
caller-configured labels onto the GitHub pull request produced by the publish
flow. It is implemented by `apply_pr_labels_best_effort()` in
`amplifier-bundle/tools/workflow_publish_pr.sh` and driven entirely by the
`WORKFLOW_PR_LABELS` environment variable.

Updated: 2026-07-23

## Contents

- [Overview](#overview)
- [Environment variable contract](#environment-variable-contract)
- [Shell helper API](#shell-helper-api)
- [Guards](#guards)
- [Best-effort semantics](#best-effort-semantics)
- [Label splitting](#label-splitting)
- [Lifecycle integration](#lifecycle-integration)
- [Configuration examples](#configuration-examples)
- [Security considerations](#security-considerations)
- [Non-goals](#non-goals)
- [Testing](#testing)

## Overview

After the publish flow resolves a GitHub pull request — either one it just
created, or an existing open PR it matched for the current branch — it applies
the labels named in `WORKFLOW_PR_LABELS` to that PR. This gives an autonomous
caller (for example an agent that needs a durable "eligible for gated
self-merge" marker) a deterministic way to stamp its PR without embedding any
label policy in the generic bundle.

Labeling is a **best-effort side effect**. It never changes the publish
outcome: the emitted JSON result, the terminal status, and the process exit
code are identical whether or not the labels were applied.

The helper delegates comma-separated label splitting to `gh` itself. It passes
the raw `WORKFLOW_PR_LABELS` value through to a single
`gh pr edit --add-label` call rather than parsing the list in shell. This is
the canonical, simplified behavior — there is no hand-rolled comma-splitting,
whitespace-trimming, or per-label loop.

## Environment variable contract

| Variable | Direction | Meaning |
| --- | --- | --- |
| `WORKFLOW_PR_LABELS` | Read | Comma-separated list of labels to apply to the published PR. Empty or unset means "apply no labels" (a no-op). The value is passed to `gh pr edit --add-label` verbatim. |

Contract details:

- The value is a comma-separated string, e.g. `simard-autonomous` or
  `simard-autonomous,needs-review`.
- The caller owns the label policy. The bundle stays policy-free: it applies
  exactly what the caller configured and nothing more.
- The variable name is stable. Callers such as Simard set it directly; it is
  never renamed or repurposed.
- Values are passed to `gh` as argument data (via `execve`), never evaluated as
  shell, so no shell metacharacter in a label value is interpreted.

## Shell helper API

The helper lives in:

```text
amplifier-bundle/tools/workflow_publish_pr.sh
```

### `apply_pr_labels_best_effort`

```bash
apply_pr_labels_best_effort
```

Applies the labels in `WORKFLOW_PR_LABELS` to the resolved PR
(`PR_NUMBER_RESULT`) with a single best-effort `gh` call. The full helper is:

```bash
apply_pr_labels_best_effort() {
  [ -n "${WORKFLOW_PR_LABELS:-}" ] || return 0
  [ "${HOST_TYPE:-}" = "github" ] || return 0
  case "${PR_NUMBER_RESULT:-}" in '' | *[!0-9]*) return 0 ;; esac
  case "${PR_STATE:-}" in MERGED | CLOSED) return 0 ;; esac
  command -v gh >/dev/null 2>&1 || return 0
  if ! timeout 60 gh pr edit "$PR_NUMBER_RESULT" \
      --add-label "$WORKFLOW_PR_LABELS" >/dev/null 2>&1; then
    echo "WARNING: workflow_publish_pr.sh: best-effort labels '${WORKFLOW_PR_LABELS}' not applied to PR #${PR_NUMBER_RESULT} (a label may not exist in this repo, GitHub API unavailable, or timed out)" >&2
  fi
  return 0
}
```

`WORKFLOW_PR_LABELS` is passed straight to `gh` — the helper introduces no
intermediate local copy or rewrite of the value, so the CSV that a caller sets
is exactly the argv `gh pr edit --add-label` receives.

Reads (as environment/shell state set by the enclosing publish flow):

| Name | Meaning |
| --- | --- |
| `WORKFLOW_PR_LABELS` | Comma-separated labels to apply. |
| `HOST_TYPE` | Must equal `github` for any labeling to occur. |
| `PR_NUMBER_RESULT` | Numeric PR number resolved by the publish flow. |
| `PR_STATE` | PR state; `MERGED` and `CLOSED` are skipped. Empty on the create-new path. |

Behavior:

- On success, the labels are added to the PR and the function returns `0`.
- On failure (label missing in the repo, GitHub API unavailable, or the call
  timing out), it writes a single `WARNING:` line to stderr naming the label
  CSV and PR number, then returns `0`.
- The function **always returns `0`** and produces no stdout.

## Guards

The helper is a no-op — returning `0` without invoking `gh` — unless **all** of
the following hold, checked in this order:

1. `WORKFLOW_PR_LABELS` is non-empty.
2. `HOST_TYPE` equals `github`.
3. `PR_NUMBER_RESULT` is a non-empty, all-numeric value
   (`case … in '' | *[!0-9]*) return 0`). This is the primary
   argument-injection guard: only a bare number ever reaches `gh`.
4. `PR_STATE` is not `MERGED` or `CLOSED`. A merged/closed terminal is a no-op
   success — there is nothing to gate for the merge queue, and labeling a
   closed PR would be pointless churn. `PR_STATE` is empty on the create-new
   path, so freshly created PRs are still labeled.
5. `gh` is present on `PATH` (`command -v gh`).

Only when every guard passes does the single `gh pr edit --add-label` call run.

## Best-effort semantics

- **Never fails the publish.** Every failure mode is warn-and-continue; the
  function always returns `0`. Labeling cannot change the publish JSON, terminal
  status, or exit code.
- **Timeout bounded.** The `gh` call is wrapped in `timeout 60`, mirroring every
  other `gh` invocation in the script, so a hung `gh pr edit` can never block
  `finish_publish` from emitting its result. Worst case is a single 60-second
  wait, regardless of how many labels are in the CSV.
- **Quiet on success.** `gh` stdout and stderr are suppressed
  (`>/dev/null 2>&1`). Only a failure emits a single `WARNING:` line to stderr,
  containing the label CSV and PR number — no tokens or secrets.

## Label splitting

`gh` splits comma-separated labels natively. Its own documented example is:

```bash
gh pr edit 23 --add-label "bug,help wanted"
```

Because of this, the helper passes `WORKFLOW_PR_LABELS` straight through to one
`gh pr edit --add-label "$WORKFLOW_PR_LABELS"` call and lets `gh` split the
list. There is no shell-side `tr`/`while read`/`sed` parsing, no per-label loop,
and no whitespace trimming — the CSV reaches `gh` verbatim, and a multi-label
value results in exactly one `gh` invocation.

> **Behavior change from the previous implementation.** The earlier helper
> hand-split the CSV and trimmed surrounding whitespace around each
> comma-separated entry before applying it. The simplified helper does not trim:
> the CSV is handed to `gh` verbatim, so any padding inside the value (for
> example `"a, b"`) is passed on as-is rather than being cleaned up by the
> bundle. Callers must therefore supply a clean, un-padded CSV such as `"a,b"`.

## Lifecycle integration

`apply_pr_labels_best_effort` is invoked from `finish_publish()` and only when
the publish terminal status is `success`:

```text
finish_publish "$state" "$legacy_state" "success" "$message"
  └─ apply_pr_labels_best_effort   # best-effort, only on success
  └─ emit_publish_result           # JSON result — unaffected by labeling
  └─ exit "$exit_code"
```

For non-success terminals the helper is not called at all. On success it runs
before the result JSON is emitted, but because it always returns `0` and writes
nothing to stdout, it cannot affect that JSON.

### Source-only test seam

`workflow_publish_pr.sh` supports the `WORKFLOW_PUBLISH_PR_LIB_ONLY` seam
(guarded by `[ "${BASH_SOURCE[0]}" != "${0}" ]`). Sourcing the script with this
seam defines `apply_pr_labels_best_effort` (and the other helpers) without
running the wider publish machinery, so the helper can be unit-tested without a
git remote. This seam is unchanged by the labeling simplification.

## Configuration examples

Apply a single durable self-merge marker:

```bash
export WORKFLOW_PR_LABELS="simard-autonomous"
# publish flow runs; on success the PR is labeled `simard-autonomous`
```

Apply multiple labels in one call (split by `gh`):

```bash
export WORKFLOW_PR_LABELS="simard-autonomous,needs-review"
# one `gh pr edit <n> --add-label "simard-autonomous,needs-review"` invocation
```

Disable labeling (no-op):

```bash
unset WORKFLOW_PR_LABELS      # or: export WORKFLOW_PR_LABELS=""
# publish runs normally; no `gh pr edit` is issued
```

## Security considerations

- **Deterministic stamp, never agentic.** Labeling is a deterministic stamp with
  no judgment. It is never an LLM/agentic step. The merge-queue discriminator
  that consumes this label must stay deterministic so that an agent can never
  fuzzily decide a stranger's PR is "mine" and self-merge it.
- **Numeric PR-number guard.** `PR_NUMBER_RESULT` is validated to be all-numeric
  before it reaches `gh`, preventing argument injection through the PR number.
- **No shell evaluation of labels.** `WORKFLOW_PR_LABELS` and
  `PR_NUMBER_RESULT` are always double-quoted and passed as `gh` argument data,
  not evaluated as shell.
- **No privilege escalation.** The helper uses the ambient `gh` authentication
  and never passes `--admin`. It does not bypass merge gates.
- **No secret leakage.** No `set -x`, no token echoing, no temp files. The only
  emitted line is the failure `WARNING`, which contains just the label CSV and
  PR number.

## Non-goals

The following are explicitly out of scope for this helper:

- **No `gh pr create --label`.** The publish flow also labels an existing open
  PR matched for the branch (`PR_STATE=OPEN`), which a create-time flag would
  miss; and a missing label at create time would abort PR creation instead of
  being best-effort. Labeling is therefore always a post-resolution
  `gh pr edit`.
- **No agentic labeling.** See [Security considerations](#security-considerations).
- **No renaming or repurposing** of `WORKFLOW_PR_LABELS`.
- **No caller changes.** Callers such as Simard continue to set
  `WORKFLOW_PR_LABELS` exactly as before.

## Testing

The helper has a dedicated unit test:

```text
amplifier-bundle/recipes/tests/test-pr-labels-best-effort.sh
```

It sources `workflow_publish_pr.sh` through the `WORKFLOW_PUBLISH_PR_LIB_ONLY`
seam and drives `apply_pr_labels_best_effort` against a fake `gh` shim that
records every invocation's argv. The shim fails an `--add-label` call for the
sentinel label `does-not-exist` (to exercise the best-effort path) and succeeds
otherwise, without contacting the network.

Contract verified by the test:

1. **Happy path.** With `WORKFLOW_PR_LABELS` set, host `github`, a numeric PR
   number, and `gh` present, the helper issues exactly **one**
   `gh pr edit <number> --add-label <value>` call passing the configured value
   verbatim, and returns `0`.
2. **Empty/unset labels.** No `gh pr edit` runs.
3. **Non-GitHub host.** No `gh pr edit` runs.
4. **Non-numeric or empty PR number.** No `gh pr edit` runs.
5. **`MERGED` / `CLOSED` PR state.** No `gh pr edit` runs (nothing to gate).
6. **`OPEN` PR state.** The PR is labeled (one call).
7. **Failing label edit.** A failing `gh pr edit` is best-effort — the function
   still returns `0` and never aborts the publish.

The obsolete cases that exercised the removed shell-side parser — multi-label
whitespace-trimming and per-label fan-out call counts — are gone, because there
is no longer a shell parser to test; `gh` owns the splitting.

Run it locally:

```bash
bash amplifier-bundle/recipes/tests/test-pr-labels-best-effort.sh
```

The test is wired into `.github/workflows/ci.yml` and must stay green.
`shellcheck` must remain clean on both `workflow_publish_pr.sh` and the test.
