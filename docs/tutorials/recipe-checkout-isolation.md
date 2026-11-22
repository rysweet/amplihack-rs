---
title: Run a Recipe in a Selected Worktree
description: Observe staged, unstaged, and untracked content and writes in a real linked Git worktree.
doc_type: tutorial
last_updated: 2026-11-18
---

# Run a recipe in a selected worktree

Create a local repository and a linked task worktree. Run a small recipe in
that worktree, then inspect its writes and the original checkout's index.
No agent login or network is needed. You need Git, `jq`, `amplihack`, and
`recipe-runner-rs` on `PATH`.

## Start a temporary shell

Find the binaries before changing home directories. Run the remaining steps
inside the shell this command opens:

```bash
demo=$(mktemp -d)
amplihack_bin=$(command -v amplihack)
runner_bin=$(command -v recipe-runner-rs)
mkdir -m 700 "$demo/home" "$demo/tmp"
env -i PATH="$PATH" HOME="$demo/home" DEMO="$demo" \
  AMPLIHACK_BIN="$amplihack_bin" RECIPE_RUNNER_RS_PATH="$runner_bin" \
  AMPLIHACK_HOME="$demo/home/.amplihack" \
  CLAUDE_CONFIG_DIR="$demo/home/.claude" \
  AMPLIHACK_CLAUDE_PROFILE_ROOT="$demo/home/profiles" \
  CODEX_HOME="$demo/home/.codex" XDG_CONFIG_HOME="$demo/home/config" \
  AMPLIHACK_WORK_ROOT="$demo/work" TMPDIR="$demo/tmp" \
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
  GIT_TERMINAL_PROMPT=0 AMPLIHACK_NONINTERACTIVE=1 \
  RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
  AMPLIHACK_NO_FRESHNESS_CHECK=1 AMPLIHACK_SKIP_AUTO_INSTALL=1 \
  AMPLIHACK_ROTATE_AUTOSTART=0 \
  bash --noprofile --norc
```

The temporary home has mode `0700`. The real `~/.claude`, `~/.amplihack`,
and profiles are untouched.

## Create the repository and task worktree

```bash
mkdir "$DEMO/repository"
cd "$DEMO/repository"
git init -q
git config user.name 'Worktree Tutorial'
git config user.email 'worktree@example.invalid'
printf 'committed version\n' > notes.txt
git add -- notes.txt
git commit -qm 'Create tutorial input'
git worktree add -q -b tutorial/named-checkout "$DEMO/task"
caller_head=$(git rev-parse HEAD)
caller_index=$(git hash-object .git/index)
cd "$DEMO/task"
printf 'staged version\n' > notes.txt
git add -- notes.txt
printf 'working version\n' > notes.txt
printf 'local recipe input\n' > local-notes.txt
git status --short
```

Output (the untracked line may appear first):

```text
MM notes.txt
?? local-notes.txt
```

## Write and run the recipe

The recipe stays outside the repository. Its step reads the index and working
content separately, then stages the untracked file in the selected worktree.

```bash
cat > "$DEMO/inspect-inputs.yaml" <<'YAML'
name: inspect-local-inputs
description: Read staged, working, and untracked content, then stage one file.
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
"$AMPLIHACK_BIN" recipe run "$DEMO/inspect-inputs.yaml" \
  -w "$DEMO/task" -c repo_path="$DEMO/task" \
  --no-detach --format json > "$DEMO/result.json"
jq -r '.success, .step_results[].output' "$DEMO/result.json"
```

Output ends with the physical path to your task worktree; its temporary
prefix varies:

```text
true
staged version
working version
local recipe input
```

The next line is the task path, which the following comparison verifies.
Progress messages go to stderr.

## Verify the execution directory and retained work

```bash
step_root=$(jq -r '.step_results[].output' "$DEMO/result.json" | tail -n 1)
task_root=$(cd "$DEMO/task" && pwd -P)
test "$step_root" = "$task_root" && echo 'step ran in the selected worktree'
git diff --cached --name-only -- local-notes.txt
test "$caller_head" = "$(git -C "$DEMO/repository" rev-parse HEAD)" \
  && echo 'original HEAD unchanged'
test "$caller_index" = "$(git -C "$DEMO/repository" hash-object .git/index)" \
  && echo 'original index unchanged'
git status --short
```

Output includes:

```text
step ran in the selected worktree
local-notes.txt
original HEAD unchanged
original index unchanged
A  local-notes.txt
MM notes.txt
```

The original checkout is unchanged because you selected a separate legitimate
worktree. The recipe's `git add` changed that worktree's index. Its staged,
unstaged, and newly staged work remains after the run ends. No private
execution repository is created or released.

## Finish the tutorial

Exit the temporary shell when finished. Keep the temporary directory while
you inspect the retained work; this tutorial does not run cleanup against it.

```bash
exit
```

For development, the default workflow creates or recovers its task worktree.
Auto-drive uses its PR worktree and follows review with agentic change
assessment. See [run a recipe](../howto/run-a-recipe.md) and the
[execution reference](../reference/recipe-checkout-isolation.md).
