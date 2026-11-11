---
title: Future Acceptance Exercise for a Local-Input Snapshot
description: Planned acceptance exercise for preserving local input layers and the caller's index.
doc_type: tutorial
last_updated: 2026-11-10
---

# Future acceptance exercise: inspect a local-input snapshot

**[PLANNED - Implementation Pending]** This is a future acceptance exercise,
not a working tutorial for the current implementation. `RunCheckout` currently
copies only committed `HEAD` into a linked worktree, shares repository
metadata, and may reuse an ancestor checkout. `GitRecord` is cooperative
Trace2 and recording failures only warn. The expected output below is not a
runtime validation receipt.

Use this exercise only after the [input declaration proposal](../reference/recipe-checkout-isolation.md#required-local-input-declarations)
has an implemented schema and production-boundary validation demonstrates the
[required guarantees](../reference/recipe-checkout-isolation.md#requirement-and-review-disposition-ledger).
The YAML includes that proposed field and is not accepted as a supported
interface today. Shell syntax, disposable fixture setup, and YAML parsing do
not demonstrate snapshot preservation, observation, or failure supervision.

The future exercise creates a disposable repository with different staged
and working versions of one file. A shell-only recipe reads both versions,
reads a declared untracked input, and stages it in the execution checkout.
The assertions specify the intended caller-index and `HEAD` preservation.

Its prerequisites will be Linux with validated descendant observation, Git,
`jq`, the updated `amplihack`, and the supported pinned `recipe-runner-rs`.
No agent login or network service is needed. Under the target contract,
unsupported observation must stop execution before the runner starts.

## Start an isolated shell

Resolve the installed binaries before changing home directories, then enter
a shell with disposable homes and profile roots:

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

Run the remaining commands inside this shell. Its profile and framework
paths point into the temporary directory, not your normal configuration.

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

Expected status:

```text
MM notes.txt
?? local-notes.txt
```

Record the caller's index digest and commit before the run:

```bash
before_index=$(git hash-object .git/index)
before_head=$(git rev-parse HEAD)
```

## Create and validate the recipe

Keep the recipe outside the caller repository. Under the target contract its
explicit invocation path must remain usable after moving execution. Declare
`local-notes.txt` as a required input; reading it from a command literal alone
does not select an untracked file. The proposed declaration is not implemented.

```bash
cat > "$DEMO/inspect-inputs.yaml" <<'YAML'
name: inspect-local-inputs
# [PLANNED] Requires the proposed CLI/runner input schema.
execution_inputs:
  version: 1
  required:
    - local-notes.txt
  context_paths: {}
  opaque_path_fields: []
description: Read separate local input layers and stage in the execution checkout.
steps:
  - id: inspect-inputs
    type: bash
    command: |
      set -eu
      git show :notes.txt
      cat notes.txt
      cat local-notes.txt
      git add -- local-notes.txt
YAML
"$AMPLIHACK_BIN" recipe validate "$DEMO/inspect-inputs.yaml"
```

After schema implementation, validation must accept `inspect-local-inputs`
and reject unsupported input declarations. Current recipe validation is not
evidence that the proposed schema is honored. Neither YAML parsing nor recipe
validation establishes observer capability or runs the step.

## Run and inspect the result

```bash
"$AMPLIHACK_BIN" recipe run "$DEMO/inspect-inputs.yaml" \
  -w "$DEMO/caller" --no-detach --format json > "$DEMO/result.json"
jq -r '.success, .step_results[].output' "$DEMO/result.json"
```

**Planned acceptance output:** values must include `true` and this step output:

```text
staged version
working version
local recipe input
```

A production run producing these values must still be checked against the
caller assertions below. Distinct first and second lines are required for
independent index/working-tree preservation; the third requires the declared
untracked input. These values have not been established by syntax checks.
Live progress belongs on stderr.

Check the caller:

```bash
test "$before_index" = "$(git hash-object .git/index)"
test "$before_head" = "$(git rev-parse HEAD)"
git status --short
```

For acceptance, both checks must exit zero and status must still show
`MM notes.txt` and
`?? local-notes.txt`: the recipe staged the untracked file only in its own
checkout. Useful commits from real workflows belong in durable task or PR
worktrees, not the disposable execution checkout.

## Inspect the retained journal

The disposable work root contains only this exercise's run folders. Read
their command journals:

```bash
find "$DEMO/work/runs" -name git-commands.jsonl -type f \
  -exec jq -c '{time, argv, cwd, git_dir, index_file}' {} +
```

For acceptance, the journal must include the step's `git add -- local-notes.txt`,
attributed to the private execution checkout and effective index. Provisioning
and release may add other records. Check the implemented recording schema and
coverage/finalization outcome as well as these projected fields. Command
presence alone does not prove an index mutation; a passing caller digest
comparison supplies evidence for this exercise only. An empty journal here is
not expected because the step ran `git add`.

Under the target failure contract, if recording or finalization fails, retain
`result.json`, the failure report, and partial evidence. A step's successful
output does not override that failure.

## Leave the exercise

```bash
cd "$DEMO"
"$AMPLIHACK_BIN" cleanup
exit
```

Cleanup previews eligible resources. Keep the temporary directory until you
have inspected any failures; exiting returns to your original shell.

Passing this exercise alone will not validate concurrency, independent nested
runs, trusted review launch, real-project hooks, or injected recorder failures.
Those require the separate production tests in the
[validation contract](../reference/recipe-checkout-isolation.md#validation-contract).

See [checkout isolation](../reference/recipe-checkout-isolation.md) for required
ignored inputs, nested-run ownership, review generations, and security limits.
