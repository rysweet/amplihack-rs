---
title: Quick-Change Recipe
description: The single-session recipe for small changes, the diff limits that hand work over to default-workflow, and how smart-orchestrator selects it.
last_updated: 2026-10-29
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Quick-Change Recipe

`quick-change` makes a small change in one agent session, runs the repository's own checks, then commits, pushes and opens a pull request. It stops once the PR is open. It does not review, merge, drive CI or finalise.

When the change turns out to be larger than the limits allow, a bash step (not the agent) hands the work over to `default-workflow`.

The recipe is `amplifier-bundle/recipes/quick-change.yaml`. It was added for issue #1740.

## Contents

- [When it runs](#when-it-runs)
- [Running it](#running-it)
- [Context variables](#context-variables)
- [Steps](#steps)
- [Diff limits](#diff-limits)
- [Hand-over to default-workflow](#hand-over-to-default-workflow)
- [Checks](#checks)
- [Pull request](#pull-request)
- [Routing from smart-orchestrator](#routing-from-smart-orchestrator)
- [The select-workflow helper](#the-select-workflow-helper)
- [Steps skipped after a quick change](#steps-skipped-after-a-quick-change)
- [Recursion limits](#recursion-limits)
- [Security limits](#security-limits)
- [Out of scope](#out-of-scope)
- [Related](#related)

## When it runs

smart-orchestrator selects `quick-change` for a `Development` task with exactly one workstream when the classifier recommends it, or when the caller forces it with `-c workflow=quick-change`. The classifier is asked to recommend it for four kinds of task:

| Category | Example task |
| --- | --- |
| Documentation only | "Fix the broken link to the recipe CLI reference in docs/index.md" |
| Dependency lock update | "Update Cargo.lock for the serde patch release" |
| Comment or typo fix | "Correct the misspelt word in the orch.rs doc comment" |
| Small contained fix | "Change the default heartbeat interval constant from 30 to 60" |

The prompt tells the classifier to choose `default-workflow` when in doubt, and never to recommend `quick-change` for a feature, a behaviour change visible to users, or a task with more than one workstream.

Everything else, including every non-Development task, routes as before. See [Routing from smart-orchestrator](#routing-from-smart-orchestrator).

## Running it

Through the orchestrator, which is the normal route:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Fix the broken link to the recipe CLI reference in docs/index.md" \
  -c repo_path=.
```

Forcing the recipe:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Update Cargo.lock for the serde patch release" \
  -c repo_path=. \
  -c workflow=quick-change
```

Forcing the full workflow for a task the classifier would treat as small:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Fix the typo in the routing prompt" \
  -c repo_path=. \
  -c workflow=default-workflow
```

Directly, without classification:

```bash
amplihack recipe run quick-change \
  -c task_description="Fix the broken link to the recipe CLI reference in docs/index.md" \
  -c repo_path=.
```

The run log shows the routing choice on stderr:

```text
[dev-orchestrator] Workflow: quick-change source=override reason=-c workflow=quick-change
```

## Context variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `task_description` | `""` | The change to make. Required. |
| `repo_path` | `"."` | Repository to work in. |
| `issue_number` | `""` | Optional. When it matches `^[1-9][0-9]{0,8}$`, the PR body gets `Refs #N` and the branch name uses it. A `#N` in the task text is used when this is empty. |
| `quick_change_max_files` | `"3"` | File limit for the diff. Must match `^[0-9]{1,6}$`. |
| `quick_change_max_lines` | `"150"` | Line limit (additions plus deletions). Must match `^[0-9]{1,6}$`. |
| `existing_branch`, `pr_number` | `""` | Not used by `quick-change`, which always creates a new `quick/` branch. Its worktree step passes both as empty, so values inherited from the orchestrator are ignored; the orchestrator does not choose `quick-change` when either is set (see [Precedence](#precedence)). The recipe sets `existing_branch` itself when it invokes `default-workflow` on hand-over. |

No GitHub issue is created by `quick-change`. A hand-over can create one; see [Hand-over to default-workflow](#hand-over-to-default-workflow).

### Tuning the limits

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Update the three install guides for the new flag name" \
  -c repo_path=. \
  -c workflow=quick-change \
  -c quick_change_max_files=5 \
  -c quick_change_max_lines=300
```

A value that does not match `^[0-9]{1,6}$` fails the `quick-change-diff-limits` step with exit code 2. There is no option that skips the diff-limit step.

## Steps

| # | Step ID | Type | What it does |
| --- | --- | --- | --- |
| 1 | `quick-change-worktree` | recipe | Runs `workflow-worktree` with `branch_prefix: quick` and with `existing_branch` and `pr_number` set to empty. Output `worktree_setup`. |
| 2 | `quick-change-record-base` | bash | Checks the branch (see [Branch name](#branch-name)), pins the base commit (`git rev-parse HEAD`) and snapshots the git configuration, hooks and push URL into the state directory. |
| 3 | `quick-change-implement` | agent | The only agent step. `amplihack:core:builder` makes the change in the worktree. |
| 4 | `quick-change-diff-limits` | bash | Runs `quick_change_diff_limits.sh`. Output `quick_change_verdict`: `proceed` or `handover`. Has no condition. Fails the run on git configuration drift, an empty diff or a fault in the tool. |
| 5 | `quick-change-write-handover` | bash | On `handover`, writes the patch file and the hand-over note. |
| 6 | `quick-change-handover` | recipe | On `handover`, runs `default-workflow`. |
| 7 | `quick-change-run-checks` | bash | On `proceed`, runs `quick_change_checks.sh`. Output `quick_change_checks`. |
| 8 | `quick-change-commit` | bash | On `proceed`, runs the artifact guard, stages, runs the guard again, sets the commit identity, commits with `git commit -F` and measures the committed change again. Output `quick_change_commit_repaired`: `yes` when hook repair ran, else `no`. |
| 9 | `quick-change-push` | bash | On `proceed`, pushes the branch, measures the committed change again and verifies the remote ref. Output `quick_change_push_repaired`: `yes` when hook repair ran, else `no`. |
| 10 | `quick-change-create-pr` | bash | On `proceed`, opens the PR. Always the last step. |

The agent works under the leaf-step constraints of #1333: it does not start an orchestrator, recipe or workflow. It is also told not to commit, and not to change git configuration, hooks, remotes or refs. These instructions are guidance; the controls that enforce them are the bash steps listed under [Security limits](#security-limits).

### Branch name

The branch comes from `workflow-worktree`, so it follows that recipe's pattern with the `quick` prefix:

```text
quick/issue-1740-route-small-tasks
quick/issue-0-fix-broken-link
```

`issue-0` is used when there is no issue number. The branch must pass `git check-ref-format`, must start with `quick/`, and must not be the default branch or a protected name. `quick-change-record-base` checks the branch before the agent runs, and the commit and push steps check it again. A branch that fails stops the recipe; this covers a task text that names its own branch with `Branch: <ref>` and the worktree step's `feat/` fallback name.

### State directory

Each run keeps its files in `~/.amplihack/state/quick-change/<n>`, where `<n>` is the `cksum` of the worktree path. The directory is absolute, outside the worktree and created with `umask 077`; it is exported to the tools as `QUICK_CHANGE_STATE_DIR`. Files are written with `mktemp` followed by `mv`, so a symlink planted at the target path is replaced rather than followed. The directory holds the pinned base, the git configuration snapshot, the checks table and, on hand-over, the patch and note.

## Diff limits

`quick-change-diff-limits` decides between `proceed` and `handover` from git output alone. It never reads agent text.

### Measurement

- The base is the commit pinned by `quick-change-record-base` before the agent ran. It is passed as `--base` and must be a 40-character hex SHA and an ancestor of `HEAD`.
- Untracked files are included: the tool runs `git add -N -A` first. Files matched by `.gitignore` are not counted.
- The diff is read with `git -c core.quotepath=off diff --no-renames --numstat -z`, so paths containing spaces, tabs or newlines parse correctly. A rename counts as two files.
- Binary files count as one file and zero lines.

### Rules

The tool parses the whole `--numstat -z` output first. The rules are then applied in this order, and the first that matches sets the verdict and reason. The two checks that fail closed come before any file or line count, so a large diff cannot hide them.

| # | Rule | Verdict | `reason=` |
| --- | --- | --- | --- |
| 1 | Git configuration or hooks differ from the snapshot | `stop` (exit 4) | `git-config-drift` |
| 2 | A path could not be parsed | `handover` | `unparseable-path` |
| 3 | No changes at all | `empty` (exit 3) | `empty-diff` |
| 4 | More files changed than `quick_change_max_files` | `handover` | `files>N` |
| 5 | More lines changed than `quick_change_max_lines`, lockfiles excluded | `handover` | `lines>N` |
| 6 | The agent made a commit (`git rev-list --count base..HEAD` > 0) | `handover` | `agent-commit` |
| 7 | A public Rust item was added, removed or changed | `handover` | `public-interface` |
| 8 | A CLI attribute line changed in a `.rs` file | `handover` | `cli-definition` |
| 9 | A manifest changed | `handover` | `manifest` |
| 10 | A sensitive path changed | `handover` | `sensitive-path` |
| 11 | A file mode changed or a symlink was added or changed | `handover` | `mode-or-symlink` |
| | None of the above | `proceed` | `within-limits` |

A path counts as unparseable when a `-z` record is truncated, has a missing NUL terminator, or has a count field that is neither a number nor `-`. The tool does not try to recover the remaining records.

**Lockfiles.** `Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `uv.lock` and `poetry.lock` count towards the file limit but not the line limit, so a lock update stays small. A lockfile that is new in the diff gets no exemption.

**Public interface.** An added or removed line in a `.rs` file counts when it matches:

```text
^[[:space:]]*pub([[:space:]]|$)
```

Any line that starts with `pub` and whitespace counts: items of every kind (`fn`, `struct`, `enum`, `trait`, `type`, `const`, `static`, `mod`, `use`, `extern`, `union`, `macro`), named struct fields such as `    pub name: u32,`, and tuple fields written on their own line. `pub` must be followed by whitespace or the end of the line, so `pub(crate)`, `pub(super)` and `pub(in ...)` do not count. Changing a private function to `pub(crate)` proceeds.

**CLI definitions.** Any changed line in a `.rs` file containing `#[arg`, `#[command` or `#[clap`. The pattern is written `#[[](arg|command|clap)`, with the `[` inside a bracket expression rather than escaped, and is passed to `awk` through the environment rather than `awk -v`. `awk -v` processes escape sequences, and gawk and busybox turn `\[` into `[`, which does not compile; the form used here means the same in mawk, gawk, busybox and BWK awk.

**Matcher faults.** The public-interface and CLI rules read the `-U0` diff of each changed `.rs` file with `awk`. A failed `git diff`, an `awk` that exits non-zero or cannot compile the pattern, and an `awk` that prints no verdict are faults (exit 2). The verdict is read from what `awk` prints, not from its exit status, so an awk that exits 1 on a bad pattern is not taken as "no match". A fault never yields `proceed`.

**Manifests.** Any changed line in `Cargo.toml`, `package.json` or `pyproject.toml`, at any depth.

**Sensitive paths.**

| Path | Reason |
| --- | --- |
| `.github/` | CI workflows run with repository secrets |
| `build.rs` | Runs at build time |
| `.cargo/config*`, `rust-toolchain`, `rust-toolchain.toml` | Change how the build runs |
| `.pre-commit-config.yaml`, `.husky/` | Change which hooks run |
| `amplifier-bundle/tools/`, `amplifier-bundle/recipes/` | Change the recipe that is running |
| `.gitmodules`, `.gitattributes` | Change how git fetches and filters content |

**Git configuration snapshot.** Taken by `quick-change-record-base` before the agent runs. The keys it covers are listed under [Security limits](#security-limits); it also covers the contents of the hooks directory. A configuration read that fails is a fault (exit 2), so two empty snapshots can never compare equal.

**Git configuration drift stops the run.** Any difference from the snapshot gives verdict `stop`. There is no hand-over and no push, because `default-workflow` would otherwise run its own commit and push steps in a worktree whose hooks, `insteadOf` rules or push URL the agent has changed. The snapshot is not restored automatically: a restore covers only the keys in the snapshot, and a change outside them would pass unnoticed. The run fails with:

```text
quick-change: git configuration or hooks changed during the agent step; stopping without hand-over or push
```

The worktree is left in place for inspection. Do not pass its branch to a later run as `existing_branch`. Remove it once inspected:

```bash
git worktree remove --force worktrees/quick/issue-0-fix-broken-link
git branch -D quick/issue-0-fix-broken-link
```

### The diff-limits tool

```text
amplifier-bundle/tools/quick_change_diff_limits.sh <worktree> [--base <sha>] [--committed] [--max-files N] [--max-lines N]
amplifier-bundle/tools/quick_change_diff_limits.sh --snapshot <worktree>
```

`--snapshot` prints the git configuration and hooks snapshot. `quick-change-record-base` stores it as `$QUICK_CHANGE_STATE_DIR/git-config.snapshot`, and `quick-change-push` takes it again and refuses to push when it differs. Each line is a name and a checksum, so the snapshot itself holds no configuration values. A measuring run reads the stored snapshot and exits 2 when `QUICK_CHANGE_STATE_DIR` is set and the file is missing; when the variable is unset, it warns and skips rule 1.

Line 1 of stdout is fixed; line 2 is JSON built with `jq -n`:

```text
QUICK_CHANGE_VERDICT=handover reason=public-interface
{"verdict":"handover","reason":"public-interface","files":2,"lines":41,"lockfile_lines":0,"max_files":3,"max_lines":150,"base":"093edc996..."}
```

`--committed` measures the change after it has been committed, which the commit and push steps do because hook repair can add commits after step 4. The worktree must be clean, so the diff against the base equals `base..HEAD`; otherwise the tool exits 2. Rule 6 (`agent-commit`) is skipped, since the recipe's own commits are expected, and every other rule runs, rule 1 included. A `handover` verdict in this mode exits 5 and prints to stderr:

```text
quick-change: hook repair grew the change beyond the quick-change limits; not publishing
```

A hand-over is not possible at that point, so the step fails instead.

On drift, line 1 is `QUICK_CHANGE_VERDICT=stop reason=git-config-drift`, and the JSON carries `"verdict":"stop"` with the names of the keys or hook files that differ. Their values are not printed.

| Exit code | Meaning |
| --- | --- |
| 0 | Verdict is `proceed`, or `handover` without `--committed` |
| 2 | Fault: bad arguments, a `--base` that is not an ancestor of `HEAD`, a git failure (a failed configuration read included), an `awk` failure while matching Rust lines, `jq` missing, or an unclean worktree with `--committed` |
| 3 | Empty diff |
| 4 | Git configuration drift |
| 5 | Verdict is `handover` with `--committed` |

The recipe step fails on exit 2, 3 or 4, and neither a PR nor a hand-over happens. An empty diff stops the run with `quick-change: no changes produced`; drift stops it with the message above. On exit 0 the step's output is the verdict word only.

## Hand-over to default-workflow

When the verdict is `handover`:

1. `quick-change-write-handover` writes the full diff against the pinned base, untracked files included, to `<state dir>/quick-change-handover.patch`, and writes the hand-over note described below.
2. Nothing is committed or pushed by `quick-change`. The worktree and branch are left as they are.
3. `quick-change-handover` runs `default-workflow` with the original task, the note appended, and `existing_branch` set from `worktree_setup.branch_name`.

`default-workflow` then reuses the `quick/` branch through its own steps:

- Step 04 (`workflow-worktree`) finds the worktree already registered for the `quick/` branch and reuses it as it is. It does not reset, clean or check out anything on that path, so the uncommitted changes from `quick-change` are still in the worktree when the builder starts.
- `workflow-prep` creates a tracking issue, because `quick-change` did not. This is a second issue when the task already had one.
- The branch ref is pushed before any commit exists on it, so the remote briefly has an empty `quick/` branch.

A run that hands over skips orchestrator reflection; `default-workflow` has its own review steps.

### Hand-over note

The note is appended to the task description that `default-workflow` receives. It holds no patch content and no agent text, only fixed wording and values produced by the bash steps. The reason is read back from the state directory and must be one of the hand-over reasons in the [Rules](#rules) table, or the step stops. `<state dir>` is replaced by the absolute path of the run's state directory:

```text
Hand-over from quick-change: the diff exceeded its limits (reason=public-interface).
The uncommitted changes are already in this worktree on branch quick/issue-1740-route-small-tasks.
Base commit: 093edc996...
Patch: <state dir>/quick-change-handover.patch
Before you start, run `git status`. If the changes listed in the patch are missing,
apply them with: git apply --index <state dir>/quick-change-handover.patch
Continue from these changes; do not start again from the base commit.
```

The patch is the backup copy. It is needed only if the worktree was changed between the hand-over and the builder step, for example by a person working in it.

## Checks

`quick-change-run-checks` runs the repository's own checks after the diff limits are measured, and only on `proceed`. A hand-over does not run them, because `default-workflow` runs its own. The checks may change files (a pre-commit fixer, for example); the commit step measures the committed change again, so such changes are still held to the limits.

```text
amplifier-bundle/tools/quick_change_checks.sh <worktree> <base-sha> <table-out-file>
```

| Repository has | Checks run |
| --- | --- |
| `Cargo.toml` | `cargo fmt --all -- --check`, always. When `.rs` files changed, `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo test -p <crate>` for each affected crate. |
| `.pre-commit-config.yaml` | `pre-commit run --files <changed files>` |
| `package.json` | `npm run lint` and `npm test`, each only when the script exists |

An affected crate is the nearest ancestor directory of a changed `.rs` file whose `Cargo.toml` has a `[package]` section. Crate names must match `^[A-Za-z0-9_-]+$`.

Checks run with `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` removed from the environment. Their output goes to stderr only, never into the PR, the commit or recipe outputs.

Stdout is one line:

```text
QUICK_CHANGE_CHECKS=failed ran=3 failed=1 skipped=2
```

| Status | Meaning |
| --- | --- |
| `passed` | At least one check ran and none failed |
| `failed` | At least one check failed |
| `none` | No check ran |

A skipped check is never counted as passed. Skip reasons come from a closed set:

| Reason | When |
| --- | --- |
| `cargo not on PATH` | `Cargo.toml` exists but `cargo` does not |
| `no .rs files changed` | clippy and test, when only non-Rust files changed |
| `pre-commit not on PATH` | `.pre-commit-config.yaml` exists but `pre-commit` does not |
| `npm not on PATH` | `package.json` exists but `npm` does not |
| `jq not on PATH` | `package.json` scripts cannot be read |
| `no "lint" script` / `no "test" script` | The script is absent from `package.json` |

A failing check does not stop the recipe. The PR is opened as a draft and its body says "Checks failed". The PR step takes the checks result from the runner's output of `quick-change-run-checks`, and stops if the copy in the state directory differs from it, so a rewritten summary file cannot turn a failed run into a ready-for-review PR. The tool exits 0 once the checks have run and 2 on a usage error.

## Pull request

The PR body is built in a file and passed with `--body-file`. The commit message is passed with `git commit -F`. Neither goes through argv (#1570).

```markdown
## Request

Fix the broken link to the recipe CLI reference in docs/index.md

Refs #1740

## Diff

 docs/index.md | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

## Checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy -p amplihack-cli --all-targets -- -D warnings` | skipped (no .rs files changed) |
| `cargo test -p amplihack-cli` | skipped (no .rs files changed) |
| `pre-commit run --files docs/index.md` | exit 0 |
```

| Item | Rule |
| --- | --- |
| Title | Task text with control characters and backticks removed and leading markdown markers (`#`, `>`, `*`, `_`, `-`) stripped, cut to 72 characters; `quick change` when nothing is left |
| Request | The task text, cut to 4000 bytes on a UTF-8 boundary |
| `Refs #N` | Only when the issue number matches `^[1-9][0-9]{0,8}$` |
| Draft | When `QUICK_CHANGE_CHECKS=failed`, or when hook repair ran in the commit or push step |

The body's first section is headed "Request" because it holds the task text as given, not a summary of the change: no agent-written text reaches the PR (S8). The commit message body is the same text.

When the checks failed, the body starts with "Checks failed". When hook repair ran, it also says "Hook repair changed the code after the checks ran; the checks table describes the pre-repair tree." The create-PR step reads `quick_change_commit_repaired` and `quick_change_push_repaired` from the runner's outputs, never from the state directory, and fails when either is missing.

The push uses an explicit refspec, `git push -u origin refs/heads/<branch>:refs/heads/<branch>`, only after the snapshot (which holds a checksum of the push URL) still matches, and is followed by `verify_pushed_ref.sh`. `gh pr create` runs through `workflow_gh_retry.sh` under an outer timeout.

### Hook repair

When a `pre-commit` or `pre-push` hook rejects the commit or push, the step uses the hook-repair loop described in [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md). Its fixing agent can change files after step 4 measured the diff, so both steps run `quick_change_diff_limits.sh --committed` against the pinned base once the commit or push has succeeded:

- In the commit step, a change that now exceeds the limits fails the step (exit 5), and nothing is pushed.
- In the push step, the loop commits the fix and pushes it itself, so the check runs after the push. A failure leaves the branch on `origin`, but the run stops before a PR is opened.
- When the change is still within the limits, the PR is opened as a draft, because the checks table describes the tree before the repair.

## Routing from smart-orchestrator

The routing rule, as stated in the dev-orchestrator skill, the routing prompt and the recipes:

> Development classification routes to `default-workflow`, or to `quick-change` for a small change in a single workstream or when `-c workflow=quick-change` forces it; any recipe value the model gives a workstream is coerced to `default-workflow`.

### Where the decision is made

1. `smart-classify-route` asks the classifier for two extra fields, `recommended_workflow` and `workflow_reason`, alongside the existing decomposition.
2. Its `select-workflow` step, after `activate-workflow`, runs `amplihack orch helper select-workflow` and stores the result as `selected_workflow`.
3. `smart-execute-routing` runs `execute-single-round-1-quick-change` when `selected_workflow == 'quick-change'`, and `execute-single-round-1-development` otherwise.

Classifier output with the new fields:

```json
{
  "task_type": "Development",
  "goal": "Fix the broken recipe CLI link",
  "workstreams": [
    {"name": "fix-link", "classification": "Development", "description": "Fix the broken link in docs/index.md"}
  ],
  "recommended_workflow": "quick-change",
  "workflow_reason": "documentation-only edit to one file"
}
```

### Precedence

| # | Condition | `selected_workflow` | `source` | Warning |
| --- | --- | --- | --- | --- |
| 1 | `task_type` is not Development and an override is set | `none` | `override` | W2 |
| 2 | `task_type` is not Development | `none` | `default` | |
| 3 | Override is `quick-change` or `default-workflow` | the override | `override` | |
| 4 | Override is set to any other value | continue at 5 | | W1 |
| 5 | Classifier recommends `quick-change` and there is exactly one workstream | `quick-change` | `classifier` | |
| 6 | Classifier recommends `quick-change` and there is more than one workstream | `default-workflow` | `classifier` | |
| 7 | Classifier recommends `default-workflow` | `default-workflow` | `classifier` | |
| 8 | No recommendation, or any other value | `default-workflow` | `default` | |

After the table, a `quick-change` result becomes `default-workflow` with `reason=existing_branch or pr_number set` when the run sets `existing_branch` or `pr_number`, because `quick-change` cannot work on an existing branch. The source is kept. When the source is `override`, W3 is printed.

A valid override applies however many workstreams there are. With `-c workflow=quick-change` and a parallel decomposition, every Development workstream runs `quick-change` and orchestrator reflection is skipped. Without an override, every Development workstream of a parallel decomposition runs `default-workflow`, whatever recipe the model gave it, and a non-Development workstream that the model gave `quick-change` runs its classification's own recipe instead; see [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md#routing-invariant). The log line therefore names the recipe the workstreams run. Non-Development routing (Q&A, Operations, Investigation) never changes.

Override values are trimmed and lower-cased, then matched against the exact list `quick-change`, `default-workflow`.

### Warnings

```text
[dev-orchestrator] WARNING: unknown workflow override 'quick' ignored
[dev-orchestrator] WARNING: workflow override 'quick-change' ignored for task_type Q&A
[dev-orchestrator] WARNING: workflow override 'quick-change' ignored: existing_branch or pr_number is set
```

The first is W1, the second W2, the third W3. The warning text, the `reason=` values (including `quick-change needs one workstream; got N` and `unspecified`) and the eight rows of the precedence table are fixed by this page. The unit tests in `select_workflow.rs` assert them exactly.

### Log line

Every orchestrated run writes one line to stderr after any warnings:

```text
[dev-orchestrator] Workflow: <quick-change|default-workflow|none> source=<override|classifier|default> reason=<text>
```

Examples:

```text
[dev-orchestrator] Workflow: quick-change source=classifier reason=documentation-only edit to one file
[dev-orchestrator] Workflow: default-workflow source=classifier reason=quick-change needs one workstream; got 2
[dev-orchestrator] Workflow: default-workflow source=default reason=unspecified
```

The reason is sanitised before it is logged: control characters (including ESC and newlines) and the bidirectional and invisible formatting characters U+200B to U+200F, U+202A to U+202E, U+2066 to U+2069 and U+FEFF are replaced with spaces, and the result is cut to 200 characters. An empty reason is written as `reason=unspecified`. The override value and `task_type` quoted in warnings are sanitised the same way and cut to 64 characters.

### Parallel decompositions

`amplihack orch helper build-workstreams-config` normalises each workstream's `recipe`. It runs only when there is more than one workstream, so it applies rows 3 to 6 of the [precedence table](#precedence): a valid override is used, and the model never selects `quick-change`, whether through `recommended_workflow` or a workstream's own `recipe` field.

| Workstream classification | Override (`-c workflow=`) | Model `recipe` | Result |
| --- | --- | --- | --- |
| Development | `quick-change` or `default-workflow`, and the top-level `task_type` is Development | any | the override |
| Development | `quick-change`, with `existing_branch` or `pr_number` set | any | `default-workflow` |
| Development | empty or invalid, or the top-level `task_type` is not Development | any, `quick-change` included | `default-workflow` |
| Non-Development | any | `quick-change` | the classification's own recipe: `investigation-workflow`, `qa-workflow` or `consensus-workflow`, and `default-workflow` for Operations |
| Non-Development | any | anything else | unchanged |

The helper reads the override from `RECIPE_VAR_workflow` and the two targets from `RECIPE_VAR_existing_branch` and `RECIPE_VAR_pr_number`, the same values `select-workflow` receives. The log line that `select-workflow` writes therefore names the recipe every Development workstream runs: `quick-change` only under a valid override, otherwise `default-workflow`, with `reason=quick-change needs one workstream; got N` when the classifier recommended `quick-change`. W1, W2 and W3 are printed by `select-workflow`; the helper prints W1 again for an invalid override. The blocked-fallback and adaptive execution paths always use `default-workflow`.

## The select-workflow helper

```text
amplihack orch helper select-workflow [--override <value>] [--task-type <type>] [--workstream-count <n>] [--existing-branch <ref>] [--pr-number <n>] < classifier.json
```

| Input | Source in `select-workflow` step |
| --- | --- |
| `--override` | `RECIPE_VAR_workflow` |
| `--task-type` | Normalised `task_type` |
| `--workstream-count` | `workstream_count` |
| `--existing-branch` | `RECIPE_VAR_existing_branch`. Any non-blank value rules out `quick-change`. |
| `--pr-number` | `RECIPE_VAR_pr_number`. Any non-blank value rules out `quick-change`. |
| stdin | `decomposition_json`, written to a `mktemp` file with `printf` |

| Output | Content |
| --- | --- |
| stdout | The bare value `quick-change`, `default-workflow` or `none`, with no trailing newline |
| stderr | Warnings, then the log line |
| exit code | Always 0 |

stdin is read up to 1 MiB. Larger or unreadable input is treated as empty, which gives rule 8. Invalid JSON is treated the same way.

```bash
printf '%s' '{"recommended_workflow":"quick-change","workflow_reason":"comment typo"}' \
  | amplihack orch helper select-workflow --task-type Development --workstream-count 1
```

stdout:

```text
quick-change
```

stderr:

```text
[dev-orchestrator] Workflow: quick-change source=classifier reason=comment typo
```

## Steps skipped after a quick change

When `selected_workflow` is `quick-change`, these smart-orchestrator steps do not run:

| Recipe | Steps |
| --- | --- |
| `smart-reflect-loop` | `reflect-round-1`, `execute-round-2`, `reflect-round-2`, `execute-round-3`, `reflect-final` |
| `smart-validate-summarize` | `validate-outside-in-testing` |

`summarize` and `complete-session` still run.

Reflection rounds and outside-in validation exist to judge a full workflow run. Outside-in validation looks for Step 13 and qa-team evidence that `quick-change` never produces, so running it would start extra rounds and remove the saving the recipe is for. A run that hands over also skips these steps; `default-workflow` reviews the work itself.

`selected_workflow` is declared in the context of `smart-orchestrator`, `smart-classify-route`, `smart-execute-routing`, `smart-reflect-loop` and `smart-validate-summarize`, so each condition can read it.

## Recursion limits

`quick-change.yaml` sets `max_depth: 6` and `max_total_steps: 140`. On the pinned runner, limits are counted from the top-level recipe, and a `recipe` step fails when the depth it runs at has reached `max_depth`. The longest path is smart-orchestrator, smart-execute-routing, `quick-change`, then a hand-over into `default-workflow` and its sub-recipes: about 139 of 200 steps, with the deepest sub-recipe (`workflow-terminal-adjudication`) at depth 5 of 6. A change to the runner pin can change how these are counted.

`issue_1740_quick_change_recipe::handover_nesting_leaves_one_level_spare` walks the `recipe:` steps of the bundled YAML from smart-orchestrator, and from `quick-change` run on its own, through `quick-change` and its hand-over, and fails when any sub-recipe on those paths sits deeper than `max_depth - 1`. A new level of sub-recipe under `default-workflow` would use that spare level, and the test fails before it ships.

## Security limits

The agent's prompt instructions are not relied on. The controls are in the bash steps:

- Tools are resolved from `AMPLIHACK_HOME`, `RECIPE_VAR_repo_path` or the two `$HOME` install locations, never from the current directory. A tools path inside the agent's worktree is refused.
- The diff base is pinned before the agent runs. An agent commit forces a hand-over. Git configuration drift and hook changes stop the run with no hand-over and no push.
- Changes made by hook repair are measured again after the commit and after the push; a change that no longer fits the limits is not opened as a PR.
- The diff-limit step and the push step compare git configuration, hooks and the push URL with the snapshot taken before the agent ran, and refuse to continue when any of them differ. The snapshot covers remotes, URL rewrites, `http.*`, credential helpers, `core.hooksPath`, `core.sshCommand`, `core.fsmonitor`, `core.gitProxy`, `core.askPass`, `protocol.*`, includes, clean and smudge filters, `core.attributesFile` and commit signing, at every configuration level, plus the whole local and worktree configuration files and the hooks directory.
- Every step after the agent reads the pinned base from the state directory and checks it against the value the runner holds in memory (`quick_change_base`), so rewriting the stored base stops the run.
- The push goes to `origin` with an explicit refspec and no force, mirror, tags or delete, and only after the snapshot comparison passes. No step merges.
- Untrusted text reaches bash only through `RECIPE_VAR_*` variables; no `command:` block contains `{{`. No step uses `eval`, `bash -c "$VAR"` or `set -x`.

Known limits:

- The public-interface rule covers Rust only. An interface change in another language that fits within the size limits proceeds and is left to PR review.
- The public-interface rule reads changed lines only. A change to a later line of a multi-line `pub` signature (a parameter on its own line, a where clause) leaves the `pub` line unchanged and proceeds when it fits within the size limits. `#[macro_export]` macros are also missed, as they are not declared with `pub`.
- Checks run code from the repository. Tokens are removed from the environment, but credential files on disk remain readable.
- The agent and the check commands run with the user's full privileges and are not sandboxed. The controls above stop changes inside the worktree and to git configuration from weakening the gate, but they cannot stop a process that edits files outside the worktree: the installed tools under `AMPLIHACK_HOME` or `repo_path`, the git configuration snapshot in the state directory, or `~/.ssh/config`. PR review is the control for those cases.
- An existing lockfile is excluded from the line limit. A large lockfile change, including one that points a package at a different download URL, proceeds when the other limits are met and is left to PR review.
- Changed file names are passed to `pre-commit run --files` as they are. An agent-created file whose name starts with `-` is read by pre-commit as an option.
- A new file created with the executable bit is counted like any other new file; only a mode change to an existing file, or a symlink, forces a hand-over.
- When hook repair during the push grows the change beyond the limits, the branch is already on `origin` and no PR is opened. Delete the remote branch by hand if it is not wanted.
- `.gitignore` is the boundary for secrets. An ignored file is neither counted nor committed; a secret in a tracked or new unignored file is caught only by the artifact guard before the commit.

## Out of scope

- Review, CI repair, merge and finalisation. Use [Auto Drive To Merge](./auto-drive-to-merge.md) for those.
- Version bumps. `quick-change` does not use `workflow-publish`.
- Adaptive workflow selection (#1708).

## Related

- [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md) - The routing invariant and workstream normalisation
- [Verify Dev-Orchestrator Routing](../howto/verify-dev-orchestrator-routing.md) - Check the routing choice from the CLI
- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md) - What happens when a hook rejects the commit or push
- [Pass Large Recipe Context and Agent Text](../howto/pass-large-recipe-context.md) - Why bodies and messages go through files
