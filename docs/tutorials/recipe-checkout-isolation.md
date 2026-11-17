---
title: Inspect a Local-Input Snapshot
description: Run a recipe against staged, unstaged and untracked changes and confirm the caller's checkout is untouched.
doc_type: tutorial
last_updated: 2026-11-16
---

# Inspect a local-input snapshot

In this tutorial you make a repository with a staged version and a different
working version of one file, plus an untracked file. You run a recipe that
reads all three and stages a file. Then you check that the recipe saw each
version and that your own index and `HEAD` did not change.

It takes about five minutes. You need Linux, Git, `jq`, `amplihack` and
`recipe-runner-rs`. No agent login or network is used. The rules behind each
result are in [checkout isolation](../reference/recipe-checkout-isolation.md).

## Start an isolated shell

Find the binaries first, then start a shell whose home, profile and work
directories are all temporary:

```bash
demo=$(mktemp -d)
amplihack_bin=$(command -v amplihack)
runner_bin=$(command -v recipe-runner-rs)
mkdir -p "$demo/home" "$demo/tmp"
env -i PATH="$PATH" HOME="$demo/home" DEMO="$demo" \
  AMPLIHACK_BIN="$amplihack_bin" RECIPE_RUNNER_RS_PATH="$runner_bin" \
  AMPLIHACK_HOME="$demo/home/.amplihack" \
  CLAUDE_CONFIG_DIR="$demo/home/.claude" \
  AMPLIHACK_CLAUDE_PROFILE_ROOT="$demo/home/profiles" \
  CODEX_HOME="$demo/home/.codex" XDG_CONFIG_HOME="$demo/home/config" \
  AMPLIHACK_WORK_ROOT="$demo/work" TMPDIR="$demo/tmp" \
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
  GIT_TERMINAL_PROMPT=0 AMPLIHACK_NONINTERACTIVE=1 \
  AMPLIHACK_NO_FRESHNESS_CHECK=1 AMPLIHACK_SKIP_AUTO_INSTALL=1 \
  AMPLIHACK_ROTATE_AUTOSTART=0 \
  bash --noprofile --norc
```

Run everything below inside this shell. Nothing touches your real
`~/.claude`, `~/.amplihack` or profiles.

## Create two local versions

```bash
mkdir "$DEMO/caller"
cd "$DEMO/caller"
git init -q
git config user.name 'Isolation Tutorial'
git config user.email 'isolation@example.invalid'
printf 'committed version\n' > notes.txt
git add -- notes.txt
git commit -qm 'Create tutorial input'
printf 'staged version\n' > notes.txt
git add -- notes.txt
printf 'working version\n' > notes.txt
printf 'local recipe input\n' > local-notes.txt
git status --short
```

Output:

```text
MM notes.txt
?? local-notes.txt
```

Save the index digest and commit so you can compare them later:

```bash
before_index=$(git hash-object .git/index)
before_head=$(git rev-parse HEAD)
```

## Write the recipe

Put the recipe outside the repository. `local-notes.txt` is an ordinary
untracked file, so the run copies it without any declaration.

```bash
cat > "$DEMO/inspect-inputs.yaml" <<'YAML'
name: inspect-local-inputs
description: Read the staged, working and untracked inputs, then stage one file.
steps:
  - id: inspect-inputs
    type: bash
    command: |
      set -eu
      git show :notes.txt
      cat notes.txt
      cat local-notes.txt
      git add -- local-notes.txt
      git rev-parse --show-toplevel
YAML
"$AMPLIHACK_BIN" recipe validate "$DEMO/inspect-inputs.yaml"
```

## Try a dry run

```bash
"$AMPLIHACK_BIN" recipe run "$DEMO/inspect-inputs.yaml" \
  -w "$DEMO/caller" --dry-run
ls "$DEMO/work/runs" 2>/dev/null | wc -l
```

The last line prints `0`. A dry run prepares no checkout and creates no run
folder.

## Run it

```bash
"$AMPLIHACK_BIN" recipe run "$DEMO/inspect-inputs.yaml" \
  -w "$DEMO/caller" --no-detach --format json > "$DEMO/result.json"
jq -r '.success, .step_results[].output' "$DEMO/result.json"
```

Output:

```text
true
staged version
working version
local recipe input
/tmp/tmp.XXXXXXXX/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/execution-Xq3TzA/checkout
```

The first two lines differ: the step read the staged version from the index
and the working version from the file. The third line shows the untracked
file was copied. The fourth is the repository the step actually worked in,
which is not `$DEMO/caller`. Your identifiers will differ. Progress messages
go to stderr.

## Check your checkout

```bash
test "$before_index" = "$(git hash-object .git/index)" && echo index unchanged
test "$before_head" = "$(git rev-parse HEAD)" && echo HEAD unchanged
git status --short
```

Output:

```text
index unchanged
HEAD unchanged
MM notes.txt
?? local-notes.txt
```

The step's `git add` changed only the execution checkout's index. In real
workflows, commits you want to keep go to a durable task or PR worktree.

## Confirm the step worked in a private clone

The step's own `git rev-parse --show-toplevel` named the repository it was
given. Compare it with the caller's:

`git rev-parse --show-toplevel` resolves symbolic links, so compare against a
resolved copy of your own paths rather than the literal `$DEMO`:

```bash
step_root=$(jq -r '.step_results[].output' "$DEMO/result.json" | tail -n 1)
caller_root=$(cd "$DEMO/caller" && pwd -P)
runs_root=$(cd "$DEMO/work/runs" && pwd -P)
test "$step_root" != "$caller_root" && echo 'step ran in a private clone'
case "$step_root" in "$runs_root/"*) echo 'under the run folder';; esac
```

Output:

```text
step ran in a private clone
under the run folder
```

That clone is removed when the run ends, so nothing of it is left:

```bash
find "$DEMO/work/runs" -maxdepth 2 -name 'execution-*' | wc -l
```

prints `0`.

The run keeps no account of the Git commands it made. There is no
`git-commands.jsonl`, no `git-trace2/` directory and no `git-recording.json`
in the run folder, and nothing traced the step while it ran:

```bash
find "$DEMO/work/runs" \
  \( -name git-commands.jsonl -o -name git-recording.json -o -name git-trace2 \) \
  | wc -l
```

prints `0`.

## Clean up

```bash
cd "$DEMO"
"$AMPLIHACK_BIN" cleanup
exit
```

`exit` returns you to your normal shell. Delete `$demo` when you are done.

## Next steps

- What is copied and what is refused:
  [checkout isolation reference](../reference/recipe-checkout-isolation.md#what-the-run-copies)
- What a run does and does not log:
  [recipe runner logging](../reference/recipe-runner-logging.md#git-commands-a-run-makes)
- How auto-drive reviews in its own checkout:
  [private review generations](../reference/recipe-checkout-isolation.md#private-review-generations)
