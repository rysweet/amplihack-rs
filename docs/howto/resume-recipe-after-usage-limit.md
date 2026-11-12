---
title: Resume a recipe after a usage limit
description: Relaunch identified work, inspect the restart decision, or deliberately start fresh.
doc_type: howto
last_updated: 2026-11-11
---

# Resume a recipe after a usage limit

Use a current Amplihack and the supported runner with `durable_recovery_v1`.
Keep the original work root available. Recovery starts only after the previous
owner has stopped and its latest matching attempt records a usage limit.

Relaunch the same recipe with the same repository and issue identity.
Amplihack finds the latest matching run; the runner restores compatible
completed steps when that run durably stopped on a usage limit.

## Contents

- [Relaunch the same work](#relaunch-the-same-work)
- [Check where execution resumes](#check-where-execution-resumes)
- [Start fresh deliberately](#start-fresh-deliberately)
- [Resolve a fresh fallback](#resolve-a-fresh-fallback)
- [Keep recovery state available](#keep-recovery-state-available)

## Relaunch the same work

Use an installed Amplihack and runner with
`durable_recovery_v1`. Keep the original work root available and wait until
the earlier run has stopped; an active process is not taken over. Check
that the provider has capacity before relaunching.

From the `amplihack-rs` checkout, this command identifies issue 1741
explicitly and keeps execution in the foreground:

```sh
amplihack recipe run smart-orchestrator --no-detach \
  --working-dir . \
  -c repo_path=. \
  -c issue_number=1741 \
  -c task_description="Fix usage-limit relaunch recovery for amplihack-rs issue 1741"
```

The result is live progress on stderr followed by the recipe
result. When the latest matching run is eligible, the progress identifies
the source run and the first step that must execute. Without eligible
history, it explains why this is a fresh execution.

Repeat your original command when recovering actual work; changing the
task description, branch selection, or other effective inputs can invalidate
saved results. The command above starts development work for that issue;
it is not a read-only probe.

Use `--detach` in place of `--no-detach` to launch in tmux. The returned
session and private log path identify where to inspect progress. Recovery
admission happens once inside the executing pane. A driver can relaunch
with the same identity after the pane exits; no prior run ID is required.

For URL-based identification, supply
`-c issue_url=https://github.com/rysweet/amplihack-rs/issues/1741` with the
matching checkout. A URL for another repository does not establish a
matching identity. See [identity rules](../reference/recipe-recovery.md#identity-and-admission).

## Check where execution resumes

Read the recovery decision in stderr or the detached log. It reports the
source run and actual execution location. For example,
compatible classification and preparation can be restored while an
incomplete design invocation resumes at `step-05b-api-design`.

The invocation path distinguishes the specific development route,
`default-workflow` call, and `workflow-design` child. Seeing the parent
routing step entered again does not mean its completed children rerun.

Add `--format json` to the launch command to retain machine-readable
results. Inspect the additive `recovery` object described in
[RecipeResult](../reference/recipe-result.md#recovery-metadata). Restored
steps retain their original results; the separate recovery metadata tells
you which work executed in this launch.

Do not set `resume_checkpoint` just to find the previous run. That context
key controls existing workflow checkpoint behavior and does not restore
the historical context needed for durable replay.

## Start fresh deliberately

Add `--fresh` to bypass historical recovery:

```sh
amplihack recipe run smart-orchestrator --fresh --no-detach \
  --working-dir . \
  -c repo_path=. \
  -c issue_number=1741 \
  -c task_description="Fix usage-limit relaunch recovery for amplihack-rs issue 1741"
```

The recovery decision is `fresh_requested`, with no restored
steps. The flag propagates to nested and detached execution. It preserves
older records and records the new run normally. If this new run stops on
a usage limit, a later launch without `--fresh` can recover it.

`--fresh` does not undo filesystem changes, clear an explicit checkpoint,
or repeat the old run in its old process. Existing conditions and
idempotency guards still determine what the new execution does.

Use `--dry-run` to inspect a plan without admitting a recovery attempt or
changing the latest marker. A dry run does not prove that saved outputs
will pass execution-time compatibility checks.

## Resolve a fresh fallback

| Reported reason | Action |
| --- | --- |
| No matching identity or history | Use the same recipe, canonical repository, issue, and work root; supply the issue explicitly |
| Latest run succeeded, failed without a limit, or has unknown outcome | Allow fresh execution; an older usage-limit run is intentionally excluded |
| Previous owner still active | Inspect or stop that run through normal session controls before relaunching |
| Historical state missing, corrupt, or unsupported | Allow the logged fresh fallback; do not edit records to force eligibility |
| Input, revision, recipe, or relevant file changed | Review the change; affected work and its ordered suffix in that invocation execute again |
| Required artifact missing or unsafe | Regenerate it through the workflow; an old path in saved output is insufficient |
| Live admission or authorization unavailable | Resolve the current session, governor, or authorization refusal; historical receipts do not grant access |
| Current recovery storage unsafe or publication failed | Fix ownership, permissions, disk capacity, or the configured work root, then relaunch; this error stops execution |

Do not reset Git or delete local edits merely to match a saved record.
Workflow-produced branch changes are checked against recorded cumulative
state. Unexplained changes require affected work to run again.

## Keep recovery state available

Keep the run's private `recovery/` directory and identity-index metadata
alongside the durable work root. `runtime/` remains disposable. Required
runtime artifacts are copied into durable storage before their completion
is published; missing unrecorded artifacts cannot be reconstructed from a
path alone.

Treat recovery state as sensitive: permitted context, prompts, outputs,
and artifact bytes may be stored there. Read
[cleanup and retention guidance](clean-up-after-recipe-runs.md#preserve-or-retire-recovery-state)
for the intended storage requirements. The retirement command and its
concurrency controls remain unresolved; existing cleanup categories do not
provide that operation. Bypassing history
with `--fresh` flag will not erase sensitive durable copies.

For the behavior model, read
[Recovery after a recipe usage limit](../concepts/recipe-usage-limit-recovery.md).
