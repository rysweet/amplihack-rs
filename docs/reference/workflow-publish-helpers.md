# Workflow Publish Helpers Reference

Arguments, inputs, exit codes and output of the four helpers added for issue
[#1575](https://github.com/rysweet/amplihack-rs/issues/1575), and the changes
to `workflow_pr_ready.sh`. For what they do and why, see
[Publish: PR Head Sync and Task-Scoped Commits](../features/workflow-publish-task-attribution.md).

> [Home](../index.md) > Reference > Workflow Publish Helpers

## Contents

- [workflow_sync_pr_head.sh](#workflow_sync_pr_headsh)
- [workflow_stage_task_changes.sh](#workflow_stage_task_changessh)
- [workflow_commit_message.sh](#workflow_commit_messagesh)
- [workflow_gh_auth_gate.sh](#workflow_gh_auth_gatesh)
- [workflow_pr_ready.sh changes](#workflow_pr_readysh-changes)
- [Recipe changes](#recipe-changes)
- [Helper lookup](#helper-lookup)
- [Security properties](#security-properties)
- [Contract tests](#contract-tests)

All four helpers live in `amplifier-bundle/tools/` and stay within the
400-line limit.

---

## workflow_sync_pr_head.sh

Pushes the current branch to its remote counterpart when that is a
fast-forward, and refuses divergent history. One ancestry policy for
`publish-sync-pr-head` and step 15 (issue #978).

### Synopsis

```bash
bash workflow_sync_pr_head.sh --mode pre-probe
bash workflow_sync_pr_head.sh --mode step15
```

Runs on the repository in the current directory. Any other argument exits
`2`.

### Inputs

| Variable | Meaning |
|---|---|
| `RECIPE_VAR_base_branch`, `BASE_BRANCH` | An additional protected branch name |

### Order of operations

1. Read the branch with `git symbolic-ref --short HEAD` and validate it with
   `git check-ref-format --branch`. A name starting with `-` is rejected. A
   detached `HEAD` is a no-op.
2. Refuse a protected branch: `main`, `master`, the target of
   `refs/remotes/<remote>/HEAD`, and the base branch from the inputs. No
   network call is made first.
3. Choose the remote: `branch.<branch>.remote`, or `origin` when unset. `.` is
   a no-op. Any other value must match the remote-name pattern and appear
   exactly in `git remote`.
4. `git fetch <remote> <branch>` with `GIT_TERMINAL_PROMPT=0`,
   `GCM_INTERACTIVE=never`, an empty `GIT_ASKPASS` and `timeout 120`. On
   success the remote commit is `FETCH_HEAD^{commit}`. If the fetch fails,
   pre-probe mode stops with a `WARNING`; step15 mode uses
   `refs/remotes/<remote>/<branch>` instead. A fetch that fails because the
   remote branch does not exist yet is the `no-remote-branch` no-op in both
   modes.
5. Count `ahead` and `behind`. Every commit ID and count is checked against a
   pattern before it is used.
6. When `ahead > 0` and `behind == 0`, run
   `git push <remote> HEAD:refs/heads/<branch>` with `timeout 120`, then
   `verify_pushed_ref.sh <remote> <branch> <sha>`.

`verify_pushed_ref.sh` is looked up next to the helper first, then through the
[five-tier ladder](#helper-lookup). If it is missing, the helper prints a
`WARNING` and the push result stands (#1268).

### Exit codes and output

| Mode | stdout | Exit |
|---|---|---|
| `pre-probe` | nothing | always `0` |
| `step15` | one line: `pushed=<true\|false> sha=<oid or empty> reason=<text>` | see below |

Step15 mode results:

| Situation | Line | Exit |
|---|---|---|
| Pushed and verified | `pushed=true sha=<oid> reason=pushed` | `0` |
| `ahead == 0` | `pushed=true sha=<oid> reason=already-pushed` | `0` |
| Detached `HEAD` | `pushed=false sha= reason=detached-head` | `0` |
| `branch.<branch>.remote` is `.` | `pushed=false sha= reason=local-tracking` | `0` |
| No remote branch and no tracking ref | `pushed=false sha= reason=no-remote-branch` | `0` |
| Diverged | `pushed=false sha= reason=diverged-upstream: ahead=<n> behind=<n> merge_base=<oid> upstream=<remote>/<branch>` | `1` |
| Push failed | `pushed=false sha= reason=push-failed: <redacted git stderr>` | the push's exit code |
| Remote ref differs from `HEAD` after push | `pushed=false sha= reason=push-verify-failed` | `1` |
| Protected branch | `pushed=false sha= reason=protected-branch` | `1` |
| Invalid branch or remote name | `pushed=false sha= reason=invalid-ref` | `1` |
| Bad argument | none | `2` |

`reason` is the rest of the line after `reason=`. It is at most 500 bytes and
contains no newlines.

All diagnostics go to stderr. No-op cases print one `INFO` line. Divergence
prints the block shown in
[Step-15 push](../features/workflow-publish-task-attribution.md#step-15-push),
which always contains `issue #978`, `refuses to rebase divergent history`,
`ahead=`, `behind=` and `merge_base=`, the amended-commit hint, and these
recovery commands:

```text
git fetch origin
git log --oneline @{u}..HEAD
git log --oneline HEAD..@{u}
git rebase @{u}
git push --force-with-lease
```

The helper prints the last two as choices for the operator. It never runs a
rebase or any form of force-push.

The first line of the block names the mode:

```text
ERROR: step-15 refuses to rebase divergent history onto upstream '<remote>/<branch>' (issue #978).
ERROR: publish-sync-pr-head refuses to rebase divergent history onto upstream '<remote>/<branch>' (issue #978).
```

### Example

```bash
$ git log --oneline origin/feat/issue-1575-implement-that-issue-on..HEAD
7cd9cd6 fix: update 2 files in amplifier-bundle
5a02be1 test: update tests/integration/issue_1575_publish_defects_test.rs
$ bash amplifier-bundle/tools/workflow_sync_pr_head.sh --mode step15
pushed=true sha=7cd9cd6e0b6d0d3f4c2b8f6a1a2e5c9d4b3a7f10 reason=pushed
$ echo $?
0
```

---

## workflow_stage_task_changes.sh

Stages the worktree, then removes generated paths and version churn from the
index.

### Synopsis

```bash
ALLOW_VERSION_BUMP=false bash workflow_stage_task_changes.sh
```

Runs on the repository in the current directory. Takes no arguments.

### Inputs

| Variable | Meaning |
|---|---|
| `ALLOW_VERSION_BUMP` | `true`, `1` or `yes` permits version churn. Anything else, or unset, does not. Step 15 sets it from the `allow_version_bump` context variable. |
| `VERSION_BUMP_NEW_VERSION`, or `RECIPE_VAR_version_bump__new_version` | Step 14's `version_bump.new_version`. The first name wins when both are set. |
| `VERSION_BUMP_CURRENT_VERSION`, or `RECIPE_VAR_version_bump__current_version` | Step 14's `version_bump.current_version`. The first name wins when both are set. |

Version churn is kept only when `ALLOW_VERSION_BUMP` permits it **and** both
versions are non-empty and differ. Unset or empty step-14 values mean no bump
was requested, so churn is excluded whatever `ALLOW_VERSION_BUMP` says. The
helper reads all three only from the environment; step 15 sets them as:

```bash
ALLOW_VERSION_BUMP="${RECIPE_VAR_allow_version_bump:-false}" bash "$STAGE"
```

and the recipe runner already exports the `VERSION_BUMP_*` and
`RECIPE_VAR_version_bump__*` names from step 14's output.

### Order of operations

1. Save the index with `git write-tree`. Exit `2` if that fails.
2. `git add -A`.
3. Unstage every path under `.github/hooks/`, read NUL-delimited, with a
   `WARNING` per path. Append untracked paths that pass the path rule to
   `$(git rev-parse --git-path info/exclude)`.
4. Classify each staged `Cargo.toml`, `Cargo.lock` and `package.json` (by
   basename, at any depth) from
   `git diff --cached --no-ext-diff --no-textconv --no-color -U0`. A changed
   line is an own-version line only when it matches the pattern and its
   location qualifies:

   | File | Pattern | Location |
   |---|---|---|
   | `Cargo.toml` | `^[+-]\s*version\s*=` | Under `[package]` or `[workspace.package]` |
   | `Cargo.lock` | `^[+-]\s*version\s*=` | In a `[[package]]` entry with no `source =` line |
   | `package.json` | `^[+-]\s*"version"\s*:` | Brace depth 1, counting braces outside strings |

   The location comes from `awk` over `git show :<path>` for `+` lines and
   `git show HEAD:<path>` for `-` lines, using the line numbers in the hunk
   headers. A file with no `HEAD` blob is never churn. A file is churn when
   every changed line is an own-version line, and mixed when it has at least
   one own-version line and any other change.
5. If all are churn and churn is not permitted, write the patch, check it,
   and restore the files.

On any failure after step 1, the helper runs `git read-tree <saved tree>` and
exits `2`.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Staging finished. The index may be empty. |
| `2` | Staging could not be completed safely. The index is back to its state before the helper ran. |

### Files written

| Path | Content |
|---|---|
| `$(git rev-parse --git-path info/exclude)` | One `/<path>` line per untracked generated path, under `# amplihack #1575: generated paths`. Append-only, no duplicates. Only paths matching `^\.github/hooks/[A-Za-z0-9._/-]+$` with no `..` are written. Shared by all linked worktrees. |
| `$(git rev-parse --git-path amplihack/excluded-version-churn.patch)` | The excluded churn, mode `0600`, written to a temporary file and renamed into place. Per worktree, overwritten on each exclusion. |

### Output

stderr only:

```text
WARNING: step-15 unstaged generated path <path> (added to info/exclude)
WARNING: step-15 unstaged generated path <path> (tracked; modification left in the working tree)
WARNING: step-15 unstaged generated path <path> (not added to info/exclude: path contains unsupported characters)
WARNING: step-15 excluded version-only change in <file> (allow_version_bump=<value>); saved to <patch path>
WARNING: step-15 excluded version-only change in <file> (no version bump requested by step 14); saved to <patch path>
WARNING: step-15 kept version files staged: <file> mixes a version change with other edits
INFO: step-15 keeping version change <current> -> <new> (allow_version_bump=<value>)
```

---

## workflow_commit_message.sh

Builds the step-15 commit subject and body from the staged index. Never reads
`TASK_DESCRIPTION`.

### Synopsis

```bash
bash workflow_commit_message.sh subject "fix: "
bash workflow_commit_message.sh body
```

Always exits `0`, so it is safe under `set -euo pipefail`.

### `subject <prefix>`

| Staged index | stdout |
|---|---|
| One file | `<prefix>update <path>` |
| Several files | `<prefix>update <N> files in <d1>[, <d2>[, <d3>]]`, adding `, …` when there are more than three top-level directories. Root files appear as `.`. |
| Empty index, or git unavailable | nothing; step 15 falls back to `<prefix>update files` |

Control characters are removed and the result is cut to 72 characters.

### `body`

Prints `git -c core.quotePath=true diff --cached --stat=72 --no-color --no-ext-diff`,
at most 30 lines, then `… (stat truncated)` if the stat was longer.

### Use in step 15

```bash
COMMIT_TITLE=$(bash "$MSG" subject "$COMMIT_PREFIX") || COMMIT_TITLE=""
[ -n "$COMMIT_TITLE" ] || COMMIT_TITLE=$(printf '%supdate files' "$COMMIT_PREFIX")
COMMIT_MSG=$(printf '%s\n\n%s\n\n%s' "$COMMIT_TITLE" "$(bash "$MSG" body)" "$ISSUE_REF")
```

---

## workflow_gh_auth_gate.sh

A library sourced by `workflow_pr_ready.sh`. It defines two functions, uses
only local variables, and does not change the caller's shell options or traps.

### Functions

| Function | Returns `0` | Returns `1` |
|---|---|---|
| `gh_auth_gate` | `timeout 30 gh auth status` succeeded on some attempt | every attempt failed; prints the contract `ERROR` |
| `gh_push_permission_gate <owner/repo>` | `.permissions.push` is `true`, or the result is unknown, or the identity is invalid | `.permissions.push` was `false` on every attempt; prints the `lacks push permission` `ERROR` |

### Settings

| Variable | Default | Valid range |
|---|---|---|
| `WORKFLOW_GH_AUTH_ATTEMPTS` | `5` | 1 to 10 |
| `WORKFLOW_GH_AUTH_BASE_DELAY` | `4` | 0 to 300 seconds |
| `WORKFLOW_GH_AUTH_MAX_DELAY` | `60` | 0 to 600 seconds |

Each value must match `^[0-9]{1,3}$` before it is used in arithmetic.
Otherwise, or when out of range, the gate prints one `WARNING` and uses the
default. The wait before retry `i` is
`min(BASE_DELAY × 2^(i-1), MAX_DELAY)`.

### Output

stderr only:

```text
WARNING: gh auth status failed (attempt 2/5); retrying in 8s
ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness
WARNING: gh reports push=false on rysweet/amplihack-rs (attempt 1/5); retrying in 4s
ERROR: active gh account 'ryan-mistt' lacks push permission on rysweet/amplihack-rs
WARNING: could not determine push permission on rysweet/amplihack-rs; continuing (GitHub enforces permissions on the mutation)
WARNING: repo identity is not a valid GitHub owner/repo; skipping push-permission check
```

### Validation

- The repository identity must match `^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$`, and
  the repository part must not be `.`, `..` or start with `.`.
- `.permissions.push` counts only when it is exactly `true` or `false`.
- The login from `gh api user --jq .login` must match `^[A-Za-z0-9-]{1,39}$`,
  otherwise `unknown` is printed.

---

## workflow_pr_ready.sh changes

| Location | Change |
|---|---|
| After the `gh` presence check | Resolves `${WORKFLOW_GH_AUTH_GATE_HELPER:-$SCRIPT_DIR/workflow_gh_auth_gate.sh}`. If missing: `ERROR`, exit `2`. Otherwise sources it and runs `gh_auth_gate \|\| exit 1`. |
| After `repo_identity` is resolved and its empty-identity check | `gh_push_permission_gate "$repo_identity" \|\| exit 1` |

Checks in order:

| Order | Check | Failure exit |
|---|---|---|
| 1 | `command -v gh` | `127` |
| 2 | Gate helper present | `2` |
| 3 | `gh_auth_gate` | `1` |
| 4 | `command -v jq` | `127` |
| 5 | Repository identity from `origin` (existing check) | `1` |
| 6 | `gh_push_permission_gate` | `1` |

### Stub `gh` for tests

Tests that run `workflow_pr_ready.sh` with a stub `gh` must answer the two new
read-only calls and set `WORKFLOW_GH_AUTH_BASE_DELAY=0`:

```bash
case "$1 $2" in
  "api repos/"*) echo true ;;
  "api user")    echo tester ;;
esac
```

---

## Recipe changes

| Recipe | Change |
|---|---|
| `workflow-publish.yaml` | New first step `publish-sync-pr-head`, before `publish-terminal-state`. Step 15 rewritten to use the three helpers. Context variable `allow_version_bump: "false"`. |
| `default-workflow.yaml` | Context variable `allow_version_bump: "false"` |

`publish-sync-pr-head` is listed in `EXPECTED_STEP_INVENTORY` in
`tests/integration/default_workflow_decomposition_test.rs`.

Step 15 passes the context variable only through the environment:

```bash
ALLOW_VERSION_BUMP="${RECIPE_VAR_allow_version_bump:-false}" bash "$STAGE"
```

---

## Helper lookup

The recipe locates `workflow_sync_pr_head.sh`, `workflow_stage_task_changes.sh`
and `workflow_commit_message.sh` with the same five-tier loop as the other
publish helpers, checking `$c/amplifier-bundle/tools/<helper>` for each `c`, in
order:

1. `${AMPLIHACK_HOME:-}`
2. `${REPO_PATH:-}`
3. `$(pwd)`
4. `${HOME:-/root}/.copilot`
5. `${HOME:-/root}/.amplihack`

The first match wins. Helpers are run with `bash`, never sourced.
`amplifier-bundle/recipes/tests/test-issue-962-runtime-artifact-ladders.sh`
checks the tier order. Because tier 3 is the task worktree, a repository can
supply its own copy of a helper; this trust model is the same as for the
existing publish helpers.

| Helper not found | Result |
|---|---|
| `workflow_sync_pr_head.sh` in `publish-sync-pr-head` | `WARNING`; the step exits `0` and the probe stays strict |
| Any of the three in step 15 | `ERROR` naming the helper and the searched locations; `reason: "helper-missing:<name>"`; exit `2` before staging |

`workflow_gh_auth_gate.sh` is not found through the ladder. It comes from
`WORKFLOW_GH_AUTH_GATE_HELPER` or the directory of `workflow_pr_ready.sh`.

---

## Security properties

Static checks in `tests/integration/issue_1575_publish_defects_test.rs`
(module `issue_1575_publish_defects` in `tests/integration/main.rs`) enforce
these:

- No `--force`, `-f`, `+` refspec, `--force-with-lease`,
  `--force-if-includes`, `--mirror`, `--all`, `--tags`, `--delete` or
  `--no-verify` passed to `git push`. The recovery hint text appears only in
  `echo` or `printf` lines.
- No `eval`, and no `gh auth switch`, `login`, `refresh`, `setup-git` or
  `--show-token`.
- No `git add -A` in step 15, and no `{{…}}` template expansion in any new
  shell line of the recipe.
- Every number read from the environment or git is matched against a pattern
  before arithmetic.
- Staged paths are read NUL-delimited and passed after `--`.
- git stderr is redacted before printing: URL credentials, `gh[pousr]_` and
  `github_pat_` tokens, and `Bearer` tokens. Only remote names are printed,
  never URLs.
- `commit_result` is written with `jq --arg`, never by string concatenation.

---

## Contract tests

The reasons, exit codes and output strings on this page are a contract.
`tests/integration/issue_1575_publish_defects_test.rs` runs each helper in a
temporary repository with a local bare remote and checks the exact text, not
a substring of it:

| Helper | Checked |
|---|---|
| `workflow_sync_pr_head.sh` | Every `reason` in the step15 table, with its exit code. Pre-probe mode exits `0` in every case, and its divergence `ERROR` names `publish-sync-pr-head` where step15 mode names `step-15`. |
| `workflow_stage_task_changes.sh` | Every `WARNING` and `INFO` line above. Churn kept only when `ALLOW_VERSION_BUMP=true` and the two versions differ; excluded when they are equal, empty or unset, using both the `VERSION_BUMP_*` and the `RECIPE_VAR_version_bump__*` names. A `[dependencies.foo]` `version` change in `Cargo.toml`, a registry entry's `version` change in `Cargo.lock`, and a nested `"version"` in `package.json` stay staged. Exit `2` restores the index. |
| `workflow_commit_message.sh` | The subject and body for each documented example. |
| `workflow_gh_auth_gate.sh` | Each permission-gate and auth-gate `WARNING` and `ERROR`, with `WORKFLOW_GH_AUTH_BASE_DELAY=0`. |

Changing any of these strings means changing this page and the test together.
