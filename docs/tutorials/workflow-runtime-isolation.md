---
title: Check workflow runtime isolation
description: Prepare clone-local exclusions and check runtime rejection in a disposable repository.
doc_type: tutorial
last_updated: 2026-11-12
---

# Check workflow runtime isolation

This exercise creates a repository without `.gitignore`, prepares its runtime
exclusions and checks that deliberately staged runtime files are refused without
changing protected source bytes or the index. A repository-owned inventory hook
runs during a normal source commit with its original manifest.

You need Git, Bash and an Amplihack installation providing
`amplihack hygiene workflow prepare`. No agent login or network access is needed.
All files and profile roots stay beneath a temporary directory. `HOME` and
`XDG_CONFIG_HOME` remain unset throughout the child environment.

## 1. Create an isolated checkout

Run the complete block below in Bash. The child environment discards inherited
Git, agent and profile settings before invoking Amplihack.

```bash
TUTORIAL_ROOT="$(mktemp -d /tmp/amplihack-isolation.XXXXXX)"
AMPLIHACK_BIN="$(command -v amplihack)"
env -i PATH="$PATH" TUTORIAL_ROOT="$TUTORIAL_ROOT" \
  AMPLIHACK_BIN="$AMPLIHACK_BIN" bash --noprofile --norc <<'BASH'
set -eu
umask 077
export AMPLIHACK_HOME="$TUTORIAL_ROOT/state"
export AMPLIHACK_WORK_ROOT="$TUTORIAL_ROOT/work"
export CLAUDE_CONFIG_DIR="$TUTORIAL_ROOT/claude"
export CODEX_HOME="$TUTORIAL_ROOT/codex"
export XDG_CACHE_HOME="$TUTORIAL_ROOT/cache"
export XDG_STATE_HOME="$TUTORIAL_ROOT/xdg-state"
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
mkdir -p "$AMPLIHACK_HOME" "$CLAUDE_CONFIG_DIR" "$CODEX_HOME"

git init -q -b main "$TUTORIAL_ROOT/source"
git init -q --bare "$TUTORIAL_ROOT/remote.git"
cd "$TUTORIAL_ROOT/source"
git config user.name 'Runtime Isolation Tutorial'
git config user.email 'runtime-isolation@example.invalid'
git remote add origin "$TUTORIAL_ROOT/remote.git"
printf '# Runtime isolation exercise\n' > README.md
git add -- README.md
git commit -q -m 'Add exercise source'
printf 'Installation notes\n' > notes.md

# Record both the untracked inventory and the repository-owned source manifest.
git ls-files --others --exclude-standard > "$TUTORIAL_ROOT/untracked-before"
git ls-files --cached --others --exclude-standard | LC_ALL=C sort > "$TUTORIAL_ROOT/manifest"
COMMON_GIT_DIR="$(git rev-parse --path-format=absolute --git-common-dir)"
cat > "$COMMON_GIT_DIR/hooks/pre-commit" <<'HOOK'
#!/bin/sh
set -eu
git ls-files --cached --others --exclude-standard | LC_ALL=C sort > "$TUTORIAL_ROOT/hook-inventory"
cmp "$TUTORIAL_ROOT/manifest" "$TUTORIAL_ROOT/hook-inventory"
printf 'Source inventory unchanged\n'
HOOK
chmod 700 "$COMMON_GIT_DIR/hooks/pre-commit"

# 2. Prepare automatically before creating checkout-local runtime fixtures.
"$AMPLIHACK_BIN" hygiene workflow prepare --repo .
cp "$COMMON_GIT_DIR/info/exclude" "$TUTORIAL_ROOT/exclude-first"
"$AMPLIHACK_BIN" hygiene workflow prepare --repo .
cmp "$TUTORIAL_ROOT/exclude-first" "$COMMON_GIT_DIR/info/exclude"
mkdir -p .amplihack .claude/runtime worktrees
printf 'graph fixture\n' > .amplihack/graph_db
printf '123\n' > .amplihack/indexing.pid
printf '{}\n' > .claude/runtime/launcher_context.json
printf '{}\n' > .claude/runtime/sessions.jsonl
: > .claude/runtime/sessions.jsonl.lock
printf 'scratch fixture\n' > worktrees/runtime.txt
git ls-files --others --exclude-standard > "$TUTORIAL_ROOT/untracked-after"
cmp "$TUTORIAL_ROOT/untracked-before" "$TUTORIAL_ROOT/untracked-after"
cat "$TUTORIAL_ROOT/untracked-after"

# 3. Keep staged source different from the working file; test each runtime path.
git add -- notes.md
git show :notes.md > "$TUTORIAL_ROOT/staged-notes"
printf 'Installation notes with unstaged edits\n' > notes.md
for RUNTIME_PATH in .amplihack/graph_db .amplihack/indexing.pid \
  .claude/runtime/launcher_context.json .claude/runtime/sessions.jsonl \
  .claude/runtime/sessions.jsonl.lock; do
  git add -f -- "$RUNTIME_PATH"
  cp "$COMMON_GIT_DIR/index" "$TUTORIAL_ROOT/index-before"
  if "$AMPLIHACK_BIN" hygiene artifact-guard --repo . --mode pre-publish; then
    printf 'Guard accepted prohibited runtime: %s\n' "$RUNTIME_PATH" >&2
    exit 1
  else
    GUARD_STATUS=$?
    test "$GUARD_STATUS" -eq 1 || exit "$GUARD_STATUS"
  fi
  cmp "$TUTORIAL_ROOT/index-before" "$COMMON_GIT_DIR/index"
  git show :notes.md > "$TUTORIAL_ROOT/staged-notes-after"
  cmp "$TUTORIAL_ROOT/staged-notes" "$TUTORIAL_ROOT/staged-notes-after"

  # Deliberate tutorial action: remove only the runtime entry just force-staged.
  git rm -q --cached -- "$RUNTIME_PATH"
done

# 4. Commit the original staged source through the inventory hook.
git diff --cached --name-only
git commit -q -m 'Record reviewed installation notes'
git show HEAD:notes.md > "$TUTORIAL_ROOT/committed-notes"
cmp "$TUTORIAL_ROOT/staged-notes" "$TUTORIAL_ROOT/committed-notes"
printf 'Runtime checks passed\n'
BASH
```

## 2. Read the results

Both filename listings contain only `notes.md`. Each guard invocation names its
runtime violation and returns `1`; that expected refusal lets the exercise
continue. Every `cmp` succeeds silently. The hook prints
`Source inventory unchanged`, then the exercise prints `Runtime checks passed`.
The committed source contains the original staged notes; the later working-file
edit remains uncommitted.

Both source manifests are sorted in the C locale. Git lists untracked files
before cached files, so staging `notes.md` changes the listing order without
changing the inventory. Sorting keeps the hook comparison independent of that
order.

The graph and PID fixtures demonstrate automatic exclusions. The launcher files
are deliberately misplaced fixtures: real launchers put their context, session
records and locks outside registered checkouts. Checking runtime paths separately
prevents one violation from masking an exemption for another.

The forced adds and exact removals are deliberate actions inside this disposable
repository. Workflows never automatically unstage protected entries.

## 3. Continue with a real workflow

The disposable repository remains at `$TUTORIAL_ROOT/source`. This exercise
covers preparation, index rejection and a source-inventory hook. Full launch,
linked-worktree sharing, bounded checkpoint transactions and destination-history
validation have separate acceptance controls in the
[checkpoint reference](../reference/workflow-checkpoint-publication.md#regression-and-runner-acceptance).
It does not replace the original native runner controls or lift a dependency hold.
The [shared-state and exclusion contract](../reference/workflow-runtime-artifacts.md#implementation-gaps)
also need separate controls; this exercise does not cover them.

Use [Configure workflow runtime isolation](../howto/configure-workflow-runtime-isolation.md)
to launch with a configured agent and remote. See the
[runtime catalogue](../reference/workflow-runtime-artifacts.md#checkout-local-exclusions)
for the complete set of owned paths.
