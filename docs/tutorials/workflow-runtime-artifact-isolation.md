---
title: "Tutorial: Workflow Runtime Artifact Isolation"
description: "Practice the lifecycle that removes workflow-generated runtime artifacts before strict Artifact Guard gates."
last_updated: 2026-06-18
review_schedule: quarterly
owner: amplihack
doc_type: tutorial
---

# Tutorial: Workflow Runtime Artifact Isolation

> [Home](../index.md) > Tutorials > Workflow Runtime Artifact Isolation

This tutorial walks through the runtime artifact lifecycle used by bundled
workflows. You will create a temporary repository, add workflow-generated
artifacts, preflight them, and confirm Artifact Guard still blocks unexpected
artifacts.

## What you will learn

- How `.claude/runtime/` is cleaned before guard-sensitive phases.
- How workflow-owned nested worktrees are identified by marker files.
- Why preflight does not replace Artifact Guard.

## Prerequisites

- `git`
- `amplihack`
- a shell with access to this checkout

## 1. Create a temporary repository

```bash
tmprepo="$(mktemp -d)"
git -C "$tmprepo" init
git -C "$tmprepo" config user.email "dev@example.com"
git -C "$tmprepo" config user.name "Amplihack Dev"
printf 'hello\n' >"$tmprepo/README.md"
git -C "$tmprepo" add README.md
git -C "$tmprepo" commit -m "initial"
```

You now have a real git worktree that the helper can validate.

## 2. Add workflow-generated runtime output

```bash
mkdir -p "$tmprepo/.claude/runtime/logs"
printf 'generated\n' >"$tmprepo/.claude/runtime/logs/session.log"
mkdir -p "$tmprepo/.claude"
printf '{"permissions":{}}\n' >"$tmprepo/.claude/settings.json"
```

The helper owns `.claude/runtime/`, but it does not own `.claude/settings.json`.

## 3. Add a marked nested workflow worktree

```bash
mkdir -p "$tmprepo/worktrees/feat-issue-780"
touch "$tmprepo/worktrees/feat-issue-780/.amplihack-workflow-worktree"
printf 'generated worktree output\n' >"$tmprepo/worktrees/feat-issue-780/output.txt"
```

The marker tells cleanup that this nested worktree was created by the workflow
and may be removed when it contains no tracked files.

## 4. Run preflight

From the amplihack checkout, source the helper and preflight the temporary
repository:

```bash
. ./amplifier-bundle/tools/workflow_runtime_artifacts.sh
preflight_known_workflow_runtime_artifacts "$tmprepo"
```

After preflight, the generated runtime and marked nested worktree are gone:

```bash
test ! -e "$tmprepo/.claude/runtime"
test ! -e "$tmprepo/worktrees/feat-issue-780"
test -f "$tmprepo/.claude/settings.json"
```

The settings file remains because cleanup never removes `.claude/` itself.

## 5. Confirm Artifact Guard passes after cleanup

```bash
amplihack hygiene artifact-guard --repo "$tmprepo" --mode all
```

Artifact Guard sees no workflow runtime leakage because preflight already
handled known workflow-owned artifacts.

## 6. Confirm unknown artifacts still fail

Add an unrelated prohibited artifact:

```bash
mkdir -p "$tmprepo/node_modules/example"
printf 'generated dependency tree\n' >"$tmprepo/node_modules/example/file.txt"
```

Run preflight and Artifact Guard:

```bash
preflight_known_workflow_runtime_artifacts "$tmprepo"
if amplihack hygiene artifact-guard --repo "$tmprepo" --mode all; then
  echo "ERROR: Artifact Guard unexpectedly allowed node_modules" >&2
  exit 1
else
  echo "blocked as expected"
fi
```

Preflight does not delete `node_modules/`. Artifact Guard blocks it because the
strict guard remains responsible for unknown prohibited artifacts.

## 7. Clean up

```bash
rm -rf "$tmprepo"
```

## What happened

The workflow lifecycle has two boundaries:

1. `preflight_known_workflow_runtime_artifacts` removes only known
   workflow-owned `.claude/runtime/` and marked nested `worktrees/` output.
2. `amplihack hygiene artifact-guard` blocks any prohibited artifact that still
   exists.

This keeps normal workflow execution from leaving generated runtime or nested
worktree artifacts inside commit worktrees without weakening Artifact Guard.

## Related documentation

- [Preflight Workflow Runtime Artifacts](../howto/preflight-workflow-runtime-artifacts.md)
- [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
- [Artifact Guard](../artifact-guard.md)
