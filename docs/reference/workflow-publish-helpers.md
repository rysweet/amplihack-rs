# Workflow Publish Helpers Reference

Command-line contracts for the helpers added for issue
[#1575](https://github.com/rysweet/amplihack-rs/issues/1575), and the
changes to `workflow_pr_ready.sh`. For the behaviour they implement and why,
see [Publish and Finalize Task Attribution](../features/workflow-publish-task-attribution.md).

> [Home](../index.md) > Reference > Workflow Publish Helpers

## Contents

- [workflow_prepush_before_probe.sh](#workflow_prepush_before_probesh)
- [workflow_commit_filter.sh](#workflow_commit_filtersh)
- [workflow_pr_ready.sh changes](#workflow_pr_readysh-changes)
- [Recipe steps](#recipe-steps)
- [Helper lookup](#helper-lookup)
- [Security properties](#security-properties)

---

## workflow_prepush_before_probe.sh

Path: `amplifier-bundle/tools/workflow_prepush_before_probe.sh` (150 lines or fewer).

### Synopsis

```bash
bash workflow_prepush_before_probe.sh
```

The helper takes no arguments and operates on the git repository in the
current directory. Any argument is a usage error.

### Inputs

| Variable | Required | Meaning |
|---|---|---|
| `RECIPE_VAR_base_branch` | No | An additional protected branch name. The helper refuses to run on it. |

The helper calls `verify_pushed_ref.sh` from its own directory, with the
arguments `<remote> <branch> <sha>`. It sets `GIT_TERMINAL_PROMPT=0` for its own git calls and wraps
`git fetch` and `git push` in `timeout 120`.

### Target selection

| Item | Source |
|---|---|
| Branch | `git symbolic-ref --short HEAD`, validated with `git check-ref-format --branch` |
| Remote | `branch.<branch>.remote` if set and an exact match for a configured remote; `origin` only when `branch.<branch>.remote` is unset. A value of `.` (local tracking, #815 and #804) is a no-op, never a fallback to `origin`. Any other value that names no configured remote is an invalid remote name. |
| Comparison ref | `refs/remotes/<remote>/<branch>` after `git fetch <remote> <branch>` |
| Push refspec | `HEAD:refs/heads/<branch>` |

`branch.<branch>.merge` is never used as a push target.

Protected names, refused before any fetch or push: `main`, `master`, the
branch named by `refs/remotes/<remote>/HEAD`, and `RECIPE_VAR_base_branch`.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Pushed and verified, nothing to push, or a no-op case (detached `HEAD`, local tracking with `branch.<branch>.remote = .`, no remote, no remote branch, fetch failure, `verify_pushed_ref.sh` missing) |
| `1` | Diverged, behind only, push rejected, verification mismatch, protected branch, or invalid branch or remote name |
| `2` | Usage error |

### Output

All diagnostics go to stderr. Each no-op prints exactly one line beginning
`INFO: pre-probe push:`. The local-tracking case prints:

```text
INFO: pre-probe push: branch 'feat/issue-1575-implement-that-issue-on' tracks a local branch (remote '.'); nothing to push
```

Ancestry failures print:

```text
  ancestry: ahead=<n> behind=<n> merge_base=<sha> head=<sha> upstream=<remote>/<branch>
```

Remote URLs and credentials in git stderr are redacted before they are
printed.

### Effect of exit 1 in each recipe

| Recipe | Step | What exit `1` stops |
|---|---|---|
| `workflow-publish.yaml` | `publish-prepush-before-probe` (first step) | The rest of the publish recipe: `publish-terminal-state`, steps 14 to 16 and every later publish step. Nothing is committed or pushed. |
| `workflow-finalize.yaml` | `finalize-prepush-before-probe` | `finalize-terminal-state` and agentic adjudication. |

### Example

```bash
$ git log --oneline origin/feat/issue-1575-implement-that-issue-on..HEAD
7cd9cd6 fix: update 2 file(s) in amplifier-bundle (#1575)
5a02be1 test: cover pre-probe push
$ bash amplifier-bundle/tools/workflow_prepush_before_probe.sh
INFO: pre-probe push: pushing 2 commit(s) to origin/feat/issue-1575-implement-that-issue-on
INFO: pre-probe push: origin/feat/issue-1575-implement-that-issue-on verified at 7cd9cd6…
$ echo $?
0
```

---

## workflow_commit_filter.sh

Path: `amplifier-bundle/tools/workflow_commit_filter.sh` (250 lines or fewer).

### Synopsis

```bash
bash workflow_commit_filter.sh filter
bash workflow_commit_filter.sh summary [issue-number]
bash workflow_commit_filter.sh body
```

All subcommands operate on the index of the git repository in the current
directory.

### `filter`

Removes generated hook stubs and version-only hunks from the index, as
described in the [feature page](../features/workflow-publish-task-attribution.md#step-15-staging-filter).

| Input | Meaning |
|---|---|
| `ALLOW_VERSION_BUMP` | `true`, `1` or `yes` (case-insensitive) keeps version-only hunks. Any other value, or unset, removes them. Step 15 sets it from the `allow_version_bump` context variable. |

| Exit | Meaning |
|---|---|
| `0` | Filtering finished. The index may now be empty. |
| `2` | Filtering could not be applied safely (for example, a `git apply -R --check` failure). The index and worktree are unchanged. |

Side effects:

| Path | Content |
|---|---|
| `$(git rev-parse --git-common-dir)/info/exclude` | One anchored line per excluded untracked hook path, under `# amplihack #1575: generated hook stubs`. Only paths matching `^\.github/hooks/[A-Za-z0-9._/-]+$` are written; other hook paths are unstaged but not written. Existing lines are not duplicated. |
| `$(git rev-parse --git-dir)/amplihack-excluded-version-bump.patch` | The removed version-only hunks, mode `0600`, written before any change is made. Overwritten on each run that removes hunks. |

Stderr receives one line per excluded path:

```text
WARNING: step-15 excluded <path> (<reason>)
```

### `summary [issue-number]`

Prints a one-line summary of the staged diff to stdout, without a prefix.

| Staged state | Output |
|---|---|
| Files staged, issue `1575` | `update 4 file(s) in amplifier-bundle, docs, tests (#1575)` |
| More than three top-level paths | `update 9 file(s) in amplifier-bundle, crates, docs, … (#1575)` |
| No issue argument | `update 4 file(s) in amplifier-bundle, docs, tests` |
| git unavailable or not a repository | `update workflow changes` |

The issue argument must match `^[1-9][0-9]*$`; any other value is ignored.

### `body`

Prints the file list for the commit body to stdout:

```text
Files changed:
- M `amplifier-bundle/recipes/workflow-publish.yaml`
- A `amplifier-bundle/tools/workflow_commit_filter.sh`
- R100 `docs/old-name.md` `docs/new-name.md`
… and 3 more
```

At most 50 file lines are printed, followed by `… and N more` when the list
is longer. Status codes come from `git diff --cached --name-status -M -z`.
Renames (`R<score>`) and copies (`C<score>`) keep the score and list the old
path then the new path, each in its own backticks.
Control characters are removed from paths, and a zero-width space is inserted
between `#` and a following digit.

### Step-15 usage

```bash
FILTER="$(resolve helper)" || { emit reason=commit-filter-missing; exit 2; }
git add -A
ALLOW_VERSION_BUMP="${ALLOW_VERSION_BUMP}" bash "$FILTER" filter || { emit reason=commit-filter-failed; exit 2; }
# second artifact-guard pass, then CLOSE_KW from the filtered index
SUMMARY=$(bash "$FILTER" summary "$ISSUE_NUMBER" | tr '\n\r' '  ' | head -c 200)
COMMIT_TITLE="${COMMIT_PREFIX}${SUMMARY}"
COMMIT_TITLE="${COMMIT_TITLE:0:72}"   # whole subject, prefix included
COMMIT_MSG=$(printf '%s\n\n%s\n\n%s' "$COMMIT_TITLE" "$(bash "$FILTER" body)" "$ISSUE_REF")
```

This is a sketch of the order of operations, not the literal recipe text.
The 72-character limit applies to the whole subject. Prefixes are at most
six characters (`feat!:`), so truncation only ever shortens the summary.

---

## workflow_pr_ready.sh changes

Path: `amplifier-bundle/tools/workflow_pr_ready.sh` (400 lines or fewer).

### Environment

| Variable | Default | Valid range | Out-of-range value |
|---|---|---|---|
| `WORKFLOW_GH_AUTH_ATTEMPTS` | `4` | 1 to 10 | treated as `4` |
| `WORKFLOW_GH_AUTH_DELAY` | `15` | 0 to 300 seconds | treated as `15` |

Wait before attempt `i` (for `i` from 2) is `DELAY * 2^(i-2)` seconds.

### Checks, in order

| Order | Check | Retried | Failure |
|---|---|---|---|
| 1 | `command -v gh` | No | exit `127`, `ERROR: gh CLI not found — cannot verify or mutate GitHub PR readiness` |
| 2 | `gh auth status` | Yes | exit `1`, `ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness` |
| 3 | `repo_identity` from `origin`, using the existing `parse_github_repo_identity`, matches `^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$` | No | Empty identity (no `origin`, or a non-GitHub URL, which the parser does not recognise): checks 3 and 4 are skipped. Non-empty but malformed: exit `1`, `ERROR: origin repo identity '<repo_identity>' is not a valid GitHub owner/repo; refusing to check push permission` |
| 4 | `gh api repos/<repo_identity> --jq .permissions.push`, only when check 3 found a valid identity | Yes, unless the result is `false` or missing | `false` or persistent error: exit `1`, `ERROR: active gh account '<login>' lacks push permission on <repo_identity>`. Missing or `null`: `WARNING`, continue. |

Skipping checks 3 and 4 does not change what happens next. The script's
existing check later in the file still exits `1` with `ERROR: unable to
determine current GitHub repo identity from origin remote` when the identity
is empty, as it does today.

Each failed retried attempt writes one stderr line containing
`attempt <i>/<n>`. `gh` probe calls send their own output to `/dev/null`.

### Test shims

Fake `gh` scripts used by tests that run `workflow_pr_ready.sh` must answer:

```bash
case "$1 $2" in
  "api repos/"*) echo true ;;
  "api user")    echo tester ;;
esac
```

Tests in which `gh auth status` always fails set `WORKFLOW_GH_AUTH_DELAY=0`.

---

## Recipe steps

| Recipe | Step id | Position | Condition |
|---|---|---|---|
| `workflow-publish.yaml` | `publish-prepush-before-probe` | immediately before `publish-terminal-state` | none |
| `workflow-finalize.yaml` | `finalize-prepush-before-probe` | immediately before `finalize-terminal-state` | none |

Both ids appear in `EXPECTED_STEP_INVENTORY` in
`tests/integration/default_workflow_decomposition_test.rs`.

Context variable:

| Recipe | Variable | Default |
|---|---|---|
| `workflow-publish.yaml`, `default-workflow.yaml` | `allow_version_bump` | `"false"` |

---

## Helper lookup

Recipes locate both new helpers in two stages, first match wins.

1. **Override.** If `WORKFLOW_PREPUSH_HELPER` or
   `WORKFLOW_COMMIT_FILTER_HELPER` is set and names an existing file, that
   file is used and the ladder is not searched.
2. **Existing five-tier ladder.** Otherwise the recipe runs the same
   `for c in` loop the other publish helpers use, checking
   `$c/amplifier-bundle/tools/<helper>` for each `c` in this order:
   1. `${AMPLIHACK_HOME:-}`
   2. `${REPO_PATH:-}`
   3. `$(pwd)`
   4. `${HOME:-/root}/.copilot`
   5. `${HOME:-/root}/.amplihack`

The override sits in front of the loop and is not a tier of it, so the loop
text stays identical to the existing ladders.
`amplifier-bundle/recipes/tests/test-issue-962-runtime-artifact-ladders.sh`
checks the five tiers and their order for each recipe and helper pair.
Helpers are run with `bash` and never sourced.

| Helper | Not found |
|---|---|
| `workflow_prepush_before_probe.sh` | `WARNING`, step exits `0` |
| `workflow_commit_filter.sh` | step 15 exits `2` before `git add`, `reason: "commit-filter-missing"` |

---

## Security properties

The following are enforced by static checks in
`tests/integration/issue_1575_publish_defects_test.rs`:

- No `--force`, `--force-with-lease`, `-f`, `+` refspecs, `--no-verify`,
  `--mirror`, `--all` or `--tags` in any push.
- No `eval` or `sh -c`, no `git config` writes and no `core.hooksPath`
  changes.
- No `gh auth switch`, `login`, `refresh` or `setup-git`.
- Every path argument follows `--`. Path enumeration uses `-z`. Diffs use
  `--no-ext-diff --no-textconv --no-color` with fixed prefixes and
  `core.quotepath=false`.
- The hunk parser is an `awk` program with no `system()` or `getline`.
- Commit messages contain no task-description text, and the issue reference
  is the last paragraph.
