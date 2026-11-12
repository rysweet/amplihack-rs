---
title: Observe workflow runtime isolation
description: Run a shell-only recipe with private runtime storage and inspect guarded disposal.
doc_type: tutorial
last_updated: 2026-11-12
---

# Tutorial: Workflow Runtime Isolation

Run a small recipe that records its runtime paths, then observe which files
remain after completion. The recipe requires no agent or provider calls.

## Contents

- [Before you start](#before-you-start)
- [Create private storage](#1-create-private-storage)
- [Write the recipe](#2-write-a-recipe-that-records-its-paths)
- [Run with an external parent](#3-run-with-an-external-parent)
- [Inspect placement and disposal](#4-inspect-placement-and-disposal)
- [Preview stale cleanup](#5-preview-stale-cleanup-in-the-same-private-environment)

## Before you start

Use Linux with the filesystem guarantees described in the
[cleanup reference](../reference/owned-run-directory-cleanup.md). Install
`amplihack`, `recipe-runner-rs`, and `jq`, and run these commands in one Bash
shell from an amplihack checkout. Use the checkout only as the working directory;
the example writes its recipe and evidence into a private directory.

## 1. Create private storage

```bash
RUNTIME_GUIDE_DIR="$(mktemp -d /var/tmp/ah-runtime.XXXXXX)" || exit 1
install -d -m 700 "$RUNTIME_GUIDE_DIR"/{work,runtime,tmp,evidence}
printf 'keep this sibling\n' > "$RUNTIME_GUIDE_DIR/runtime/sentinel"
```

Keep the resolved `runtime/` parent at most 59 bytes long. The path above
normally fits; the runner reports a fallback if its resolved path is too long.
The work root, temporary root, and external runtime parent are separate private
directories. The sentinel is outside any disposable run directory.

## 2. Write a recipe that records its paths

```bash
cat > "$RUNTIME_GUIDE_DIR/inspect-runtime.yaml" <<'YAML'
name: inspect-runtime
description: Record this run's disposable directory and temporary child.
version: "1.0.0"
context:
  evidence_dir: ""
steps:
  - id: record-paths
    type: bash
    command: |
      set -eu
      printf '%s\n' "$AMPLIHACK_WORKFLOW_RUNTIME_DIR" > "{{evidence_dir}}/runtime-path"
      printf '%s\n' "$TMPDIR" > "{{evidence_dir}}/temporary-path"
      printf '%s\n' "$AMPLIHACK_RUN_DIR" > "{{evidence_dir}}/run-path"
      printf 'disposable scratch\n' > "$TMPDIR/scratch"
      printf 'paths recorded\n'
YAML
```

The step writes its observation outside the runtime directory so that evidence
survives successful cleanup.

## 3. Run with an external parent

```bash
env \
  TMPDIR="$RUNTIME_GUIDE_DIR/tmp" \
  AMPLIHACK_WORK_ROOT="$RUNTIME_GUIDE_DIR/work" \
  AMPLIHACK_RUN_TMP_ROOT="$RUNTIME_GUIDE_DIR/runtime" \
  AMPLIHACK_CLEANUP_ON_EXIT=0 \
  amplihack recipe run "$RUNTIME_GUIDE_DIR/inspect-runtime.yaml" \
    -c evidence_dir="$RUNTIME_GUIDE_DIR/evidence" \
    --format json > "$RUNTIME_GUIDE_DIR/evidence/result.json" \
    2> "$RUNTIME_GUIDE_DIR/evidence/progress.log"
```

The recipe succeeds and captures `paths recorded` as step output.
`AMPLIHACK_CLEANUP_ON_EXIT=0` prevents the sweep of earlier runs; this run's
own directory still goes through guarded disposal. The explicit external
parent wins over the caller's private `TMPDIR`.

## 4. Inspect placement and disposal

```bash
GUIDE_RUNTIME="$(cat "$RUNTIME_GUIDE_DIR/evidence/runtime-path")"
GUIDE_TMP="$(cat "$RUNTIME_GUIDE_DIR/evidence/temporary-path")"
GUIDE_RUN="$(cat "$RUNTIME_GUIDE_DIR/evidence/run-path")"
printf 'Runtime: %s\nTemporary: %s\n' "$GUIDE_RUNTIME" "$GUIDE_TMP"
jq '{runtime_dir, runtime_root, outcome}' "$GUIDE_RUN/run.json"
```

The runtime path is the resolved private parent's `ahr-<12 hex>` child.
The step's temporary path is that child's `tmp/` directory. `run.json` records
the external path as `runtime_dir`, its independently selected parent as
`runtime_root`, and `success` as the outcome. The suffix varies per run.

On a supported Linux filesystem, verify removal and preservation:

```bash
test ! -e "$GUIDE_RUNTIME" &&
test -d "$RUNTIME_GUIDE_DIR/runtime" &&
test -f "$RUNTIME_GUIDE_DIR/runtime/sentinel" &&
test -f "$GUIDE_RUN/run.json" &&
printf 'Runtime removed; parent, sibling, and report remain.\n'
```

Expected output:

```text
Runtime removed; parent, sibling, and report remain.
```

The configured parent can still contain ownership metadata. An empty parent
is not a cleanup success criterion. If the runtime directory survives, read
`evidence/progress.log` for the preservation reason. Missing filesystem
guarantees, changed identities, or invalid evidence cause preservation rather
than a weaker deletion attempt. Do not add markers or recursively delete the
parent to force this checkpoint to pass.

## 5. Preview stale cleanup in the same private environment

```bash
env \
  TMPDIR="$RUNTIME_GUIDE_DIR/tmp" \
  AMPLIHACK_WORK_ROOT="$RUNTIME_GUIDE_DIR/work" \
  amplihack cleanup --only run-tmp --format json
```

The command reports a dry run. A successfully removed runtime directory has
no remaining `run-tmp` candidate. A retained directory requires the same
ownership, parent, protected-path, and identity checks during stale cleanup;
its run report alone does not grant deletion authority. The sentinel remains.

Keep the private evidence directory for inspection. This tutorial does not
perform a broad cleanup of its parent storage or change source files.

## Continue with configuration and worktree guards

- [Configure Workflow Runtime Isolation](../howto/configure-workflow-runtime-isolation.md)
  covers placement overrides and preservation diagnostics.
- [Owned Run Directory Cleanup](../reference/owned-run-directory-cleanup.md)
  defines ownership, claims, lifecycle outcomes, and supported guarantees.
- [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
  covers the separate worktree preflight helper and its narrow artifact list.
- [Artifact Guard](../artifact-guard.md) describes checks before staging generated
  output. Runtime placement does not exempt generated artifacts in a worktree.
