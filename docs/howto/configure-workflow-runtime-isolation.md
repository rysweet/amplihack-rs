---
title: Configure workflow runtime isolation
description: Launch in an existing checkout and resolve runtime, index and publication conflicts.
doc_type: howto
last_updated: 2026-11-12
---

# Configure workflow runtime isolation

**Acceptance remains incomplete.** The candidate implementation retains the
[runner and historical-evidence gates](../reference/workflow-checkpoint-publication.md#regression-and-runner-acceptance).
See the [current restrictions](../reference/workflow-checkpoint-publication.md#implementation-gaps).

Use an installed Amplihack bundle, a configured agent and an existing non-bare
checkout with committed history and the remote required by your workflow.
No `.gitignore` or source-manifest amendment is required.

## Check the checkout and launch

Run these commands from the intended repository root:

```bash
git rev-parse --show-toplevel
git rev-parse --is-bare-repository
git remote get-url origin
git rev-parse --verify 'HEAD^{commit}'
```

Expect the checkout root, `false`, the remote URL and a commit ID. Launch the
workflow with a concrete source task:

```bash
amplihack recipe run default-workflow --detach   -c task_description="Clarify the installation prerequisites in README.md"   -c repo_path=.
```

The command returns the detached session and log location. Preparation checks
prerequisites before runtime writes, registers clone-local exclusions and keeps
launcher state external. Checkpoints use exact paths established by task review.
Review feedback and cleanup use the same boundaries.

If preparation refuses the workspace, select the correct checkout or create a
repository explicitly outside the workflow. A parent folder containing several
checkouts is not itself a suitable repository. `auto_init_repo=true` does not
change this requirement.

## Choose external runtime storage

For recipe runs, use the [work-root settings](clean-up-after-recipe-runs.md).
To create a private work root for this shell:

```bash
WORKFLOW_WORK_ROOT="$(mktemp -d /var/tmp/amplihack-work.XXXXXX)"
chmod 700 "$WORKFLOW_WORK_ROOT"
export AMPLIHACK_WORK_ROOT="$WORKFLOW_WORK_ROOT"
```

The runner supplies its runtime directory to child steps. Standalone launchers
use suitable external `AMPLIHACK_HOME` state, otherwise `$HOME/.amplihack`.
The [runtime reference](../reference/workflow-runtime-artifacts.md#external-state-resolution)
describes resolution and identity scopes.

Keep effective runtime paths outside every registered checkout, including linked
worktrees. Paths resolving inside a checkout through a symlink are refused.
An `AMPLIHACK_HOME` used for framework source lookup is not a state directory.
Without a usable external root, dependent writes fail visibly.

## Inspect automatic exclusions

After preparation, inspect the common exclusion file and source inventory:

```bash
COMMON_GIT_DIR="$(git rev-parse --path-format=absolute --git-common-dir)"
cat "$COMMON_GIT_DIR/info/exclude"
env -u HOME -u XDG_CONFIG_HOME git ls-files --others --exclude-standard
```

Existing comments, patterns and permissions remain intact. Effective owned
runtime exclusions appear once and apply to linked worktrees. Runtime creation
leaves the source inventory unchanged. See the
[owned-path catalogue](../reference/workflow-runtime-artifacts.md#checkout-local-exclusions).

If registration fails, correct the reported permissions or path conflict. The
dependent runtime write stops. A global ignore entry does not repair this failure.

## Resolve an index refusal

Inspect without changing the index:

```bash
git diff --cached --name-status
amplihack hygiene artifact-guard --repo . --mode pre-publish
```

The guard returns `1` for prohibited runtime content and leaves all entries
unchanged. This includes unchanged tracked runtime files, force-staged files,
deletions and rename endpoints. No allowlist or baseline exemption permits them.

Review the reported paths before taking a separate, deliberate index action.
The workflow never automatically unstages protected work. If a source candidate
overlaps an existing user-staged entry, resolve that overlap explicitly. If Git
entry metadata cannot be preserved exactly, the checkpoint refuses before
creating a commit.

A reconciliation failure after commit creation includes the new commit ID.
Inspect that commit and the preserved original index before another checkpoint;
do not repeat the commit blindly.

## Resolve a publication refusal

In the intended interface, status `78` identifies unauthorised outgoing history.
The current dispatcher returns `78` for every workflow-operation error, so read
the diagnostics to identify the failing operation; see
[exit-status contract](../reference/workflow-checkpoint-publication.md#implementation-gaps).
For an authority refusal, review the reported ref,
commit and path. Establish legitimate scope extensions through requirements and
design review, or prepare history containing only authorised work. Staging a
path or editing tracking refs and `FETCH_HEAD` does not authorise it.

Advertisement, fetch, history and hook failures stop dependent publication.
Correct the reported problem before a new explicit attempt. Pushes across
several destinations can partly succeed; inspect each destination outcome
before proceeding. Use ordinary hooks and non-forced updates throughout.

See [structured publication results](../reference/workflow-checkpoint-publication.md#structured-results)
for the PR-creation condition and failure reporting.

## Related documentation

- [Workflow runtime isolation](../features/workflow-runtime-isolation.md)
- [Runtime isolation tutorial](../tutorials/workflow-runtime-isolation.md)
- [Checkpoint and publication reference](../reference/workflow-checkpoint-publication.md)
- [Artifact Guard](../artifact-guard.md)
