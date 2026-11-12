---
title: Quick-Change Recipe
description: Small changes on new or existing branches, pinned diff limits, publication and change-based titles.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Quick-Change Recipe

`quick-change` makes a small change in one agent session, runs the repository's own checks, then commits and pushes. A new branch gets a pull request. With `existing_branch` set, the recipe updates that exact branch and leaves its existing PR unchanged. It does not review, merge, drive CI or finalise.

When the change turns out to be larger than the limits allow, a bash step (not the agent) hands an ordinary new-branch run over to `default-workflow`, whose exact branch selector requires the branch to exist on origin. Existing-branch continuation stops visibly before delegation, preserving the checkout and a patch, because continuation must never create or modify a PR. See [Hand-over to default-workflow](#hand-over-to-default-workflow), including recovery when the initial setup push failed.

The recipe is `amplifier-bundle/recipes/quick-change.yaml`. It was added for issue #1740.

## Contents

- [When it runs](#when-it-runs)
- [Running it](#running-it)
- [Context variables](#context-variables)
- [Existing-branch continuation](#existing-branch-continuation)
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

smart-orchestrator selects `quick-change` for a `Development` task with exactly one workstream when the classifier recommends it, or when the caller forces it with `-c workflow=quick-change`. The classifier is asked to recommend it for five kinds of task:

| Category | Example task |
| --- | --- |
| Documentation only | "Fix the broken link to the recipe CLI reference in docs/index.md" |
| Dependency lock update | "Update Cargo.lock for the serde patch release" |
| Comment or typo fix | "Correct the misspelt word in the orch.rs doc comment" |
| Configuration or wording change | "Set `site_name` in mkdocs.yml to the new project name", "Reword the missing-token error message" |
| Single-file fix with no behaviour change to a public interface | "Close the file handle the private cache loader leaves open" |

The prompt tells the classifier to choose `default-workflow` when in doubt, and never to recommend `quick-change` for a feature, a behaviour change visible to users, or a task with more than one workstream.

A task that targets a branch it already has, through `existing_branch`, `pr_number` or a `Branch: <ref>` line in the task text, gets `default-workflow` through smart-orchestrator. To use quick-change continuation, call the recipe directly with `existing_branch`. See [Existing-branch continuation](#existing-branch-continuation) and [Routing from smart-orchestrator](#routing-from-smart-orchestrator).

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
| `issue_number` | `""` | Optional. A valid explicit number takes precedence; when empty, the first valid `#N` in the task text is used. Numbers must match `^[1-9][0-9]{0,8}$`. Publication preserves an explicit `Fixes #N` for the selected issue independently of request truncation; otherwise it appends `Refs #N`. See [Issue references](#issue-references). New-branch naming also uses the issue number; continuation never renames its branch. |
| `quick_change_max_files` | `"3"` | File limit for the diff. Must match `^[0-9]{1,6}$`. |
| `quick_change_max_lines` | `"150"` | Line limit (additions plus deletions). Must match `^[0-9]{1,6}$`. |
| `allow_no_op` | `"false"` | When exactly `"true"`, an empty diff finishes the run with verdict `empty` and no commit, push or PR, in place of failing it. smart-classify-route sets it for docs-only, audit, orchestration and meta tasks, and smart-execute-routing passes it on, as it does to `default-workflow`. See [Empty diff](#empty-diff). |
| `existing_branch` | `""` | Empty creates a new `quick/` branch. Otherwise, the exact branch must exist on `origin`; its checkout is reused or created without replacing local history or work. The `quick/` prefix is not required. On `proceed`, commits are pushed to this branch with no PR operations. An oversized continuation saves recovery files and stops before hand-over. See [Existing-branch continuation](#existing-branch-continuation). |
| `pr_number` | not declared | Not used. The worktree call clears it; continuing an existing PR requires `existing_branch`, with no PR lookup. |
| `base_branch`, `forbidden_base_branches`, `strict_delivery` | inherited | The [delivery inputs](workflow-delivery-inputs.md#where-the-inputs-apply) of `default-workflow`, not declared here. The worktree step clears `base_branch` and `forbidden_base_branches`. A new `quick/` branch starts from `origin/HEAD`, which the new PR targets. Continuation retains the selected branch's local history. `strict_delivery` is passed through: `"true"` refuses new-branch creation; it does not replace a supplied existing branch. |

No GitHub issue is created by `quick-change`. An ordinary new-branch hand-over can create one; see [Hand-over to default-workflow](#hand-over-to-default-workflow).

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

## Existing-branch continuation

Call `quick-change` directly to fix review findings on an existing PR branch. Run from any checkout of the repository, including the checkout already on that branch. Git, access to `origin` and a resolvable commit identity are required; `existing_branch` is the branch name without `origin/` or `refs/heads/`.

For example, for the existing branch from issue #2210:

```bash
amplihack recipe run quick-change \
  -c task_description="Fix the remaining review findings for #2210" \
  -c repo_path=. \
  -c existing_branch=quick/issue-2210-fix-issue-2210-amplihack \
  -c issue_number=2210
```

This command requires that branch to exist on `origin`. A successful run with verdict `proceed` adds its change commit to that exact branch and verifies the pushed SHA. The existing PR receives the new commits through Git; quick-change makes no `gh` calls, including PR discovery. It does not rename, edit, comment on or change the draft status of the PR. Continuation stops before unrestricted hand-over when its diff exceeds the limits.

`existing_branch` establishes the publication destination without looking up a PR. A remote branch without a PR follows the same contract: continuation does not create one. `pr_number` alone does not select a continuation branch.

### Remote validation and checkout selection

Before changing the checkout or starting the builder, the recipe validates the literal Git heads ref, refuses `main`, `master`, `develop`, `trunk` and origin's default branch, checks for the exact `refs/heads/<existing_branch>` on `origin` and fetches it. Checkout shorthand such as `@{-1}` is refused. The default branch is checked against origin even when local `origin/HEAD` is missing or stale. A local branch or stale remote-tracking ref alone does not satisfy the existence check. Similar remote names do not match. A missing branch is refused with a diagnostic naming it; an origin-access failure is reported separately.

Fetch updates only `refs/remotes/origin/<existing_branch>` through an explicit refspec. Configured fetch mappings cannot overwrite local heads during selection.

| Checkout state | Selection |
| --- | --- |
| The caller's checkout is on the requested branch | Reuse that checkout, including its existing work. |
| Another accessible registered worktree is on the branch | Reuse that worktree. |
| The branch exists locally but has no worktree | Attach the existing local branch without resetting it to origin. |
| Only origin has the branch | Create a local tracking branch with the exact requested name and check it out. |
| A registered checkout is inaccessible, or the destination conflicts | Stop with a diagnostic; do not replace the checkout or substitute another branch. |

Each selected checkout must belong to the caller's repository, verified through its canonical Git common directory, and be on the exact requested branch. An inaccessible or mismatched registration stops selection.

Selection preserves local commits and staged, unstaged and untracked work. It does not stash, discard, reset or force a checkout. No alias or replacement branch is created, even when the caller already has the requested branch checked out. Continuation selection runs before general orphan-worktree cleanup can affect the selected checkout and performs no setup push.

The quick-change worktree call forwards `existing_branch` unchanged and sets `worktree_policy: quick-change`. This policy selects exact continuation; other workflows retain their existing worktree-selection behaviour by default. The returned `worktree_setup` identifies the actual checkout path and exact branch name, without alias metadata. The recipe verifies branch identity before implementation, commit and push.

### Base, existing work and publication

The selected checkout's local `HEAD` is pinned before implementation. This SHA remains the base throughout the run, even when local history differs from origin. Earlier commits already present at that SHA do not count towards the limits. Applicable staged, unstaged and untracked work does count, alongside builder edits and hook repairs, and is included in the resulting change commit. Ignored files and existing publication exclusions still apply.

For example, if a branch already has ten commits and its checkout has two modified files and one untracked file, the base is the tenth commit. The three files count towards the default three-file limit; the ten earlier commits do not. A fourth applicable file exceeds that limit and stops continuation with recovery files.

Publication uses the explicit, non-force destination `refs/heads/<existing_branch>:refs/heads/<existing_branch>` on `origin`, with upstream tracking, artefact guards, all configured hooks, configuration checks and remote-SHA verification. The complete committed diff is measured against the pinned base before publication, including changes from checks and commit-hook repair.

A non-fast-forward rejection preserves local history. The recipe does not reset, rebase, force-push or switch to another branch to overcome rejection. Continuation remeasures immediately before pushing and stops on rejection without automatic push repair, preserving the checkout, local commits and state. See [Hook repair](#hook-repair).

Check failures and hook repairs remain visible as run warnings. Since continuation leaves the PR unchanged, it cannot mark that PR as a draft or add those warnings to its body. Successful continuation removes the run's state directory after verified publication. Empty changes retain the [existing no-op behaviour](#empty-diff); oversized changes save a patch and stop before [hand-over](#hand-over-to-default-workflow), leaving the exact branch and checkout available for further work.

## Steps

| # | Step ID | Type | What it does |
| --- | --- | --- | --- |
| 1 | `quick-change-worktree` | recipe | Runs `workflow-worktree` with `branch_prefix: quick`, `worktree_policy: quick-change` and `existing_branch` forwarded unchanged. Clears `pr_number`, `base_branch` and `forbidden_base_branches`. Validates and selects an existing branch before any setup publication, or creates a new `quick/` branch. Output `worktree_setup`. |
| 2 | `quick-change-record-base` | bash | Checks the branch (see [Branch name](#branch-name)), checks that a safe commit identity resolves, so a run that could never commit stops before the agent and the checks, removes other runs' state directories that have been idle for 14 days (see [State directory](#state-directory)), pins the base commit (`git rev-parse HEAD`) and snapshots the git configuration, hooks and push URL into the state directory. |
| 3 | `quick-change-implement` | agent | The only agent step. `amplihack:core:builder` makes the change and returns a structured change summary and proposed `commit_subject`, without committing or publishing. |
| 4 | `quick-change-diff-limits` | bash | Runs `quick_change_diff_limits.sh`. Output `quick_change_verdict`: `proceed`, `handover`, or `empty` when the diff is empty and `allow_no_op` is `"true"`. Has no condition. Fails the run on git configuration drift, an empty diff without `allow_no_op`, or a fault in the tool. |
| 5 | `quick-change-write-handover` | bash | On `handover`, writes the patch file and the hand-over note. Existing-branch continuation then fails visibly before delegation. |
| 6 | `quick-change-handover` | recipe | After step 5 succeeds on an ordinary run with `handover`, runs `default-workflow`. |
| 7 | `quick-change-run-checks` | bash | On `proceed`, runs `quick_change_checks.sh`. Output `quick_change_checks`. |
| 8 | `quick-change-commit` | bash | On `proceed`, runs the artifact guard, stages, runs the guard again, sets the commit identity, commits with `git commit -F` and measures the committed change again. Output `quick_change_commit_repaired`: `yes` when hook repair ran, else `no`. |
| 9 | `quick-change-push` | bash | On `proceed`, rechecks branch identity and the configuration snapshot. Continuation remeasures before one push and stops on rejection. Ordinary new-branch runs retain push repair. Both remeasure afterwards and verify the remote SHA. Output `quick_change_push_repaired`: `yes` when hook repair ran, else `no`; see [Hook repair](#hook-repair). |
| 10 | `quick-change-create-pr` | bash | On `proceed`, reports warnings and completes existing-branch continuation without any PR operation, or opens a new-branch PR. Removes successful-run state. Always the last step. |

The agent works under the leaf-step constraints of #1333: it does not start an orchestrator, recipe or workflow. It is also told not to commit, and not to change git configuration, hooks, remotes or refs. These instructions are guidance; the controls that enforce them are the bash steps listed under [Security limits](#security-limits).

### Branch name

Without `existing_branch`, the branch comes from `workflow-worktree` with the `quick` prefix:

```text
quick/issue-1740-route-small-tasks
quick/issue-0-fix-broken-link
```

`issue-0` is used when there is no issue number. Every branch must pass `git check-ref-format` and must not be the default branch or a protected name. New branches must start with `quick/`; continuation branches must exactly equal `existing_branch` and need no prefix. `quick-change-record-base` checks before the agent runs, and the commit and push steps check again. A branch that fails stops the recipe; this covers a `feat/` fallback name on an ordinary new-branch run.

Without `existing_branch`, task text that names its own branch (`Branch: <ref>` and the other directive forms of #1426) takes precedence over the `quick/` prefix in `workflow-worktree`. A resulting branch outside `quick/` fails the new-branch check. smart-orchestrator selects `default-workflow` for tasks naming a branch; see [Precedence](#precedence). Direct continuation requires an explicit `existing_branch`, which takes precedence over task-text directives.

### State directory

Each run keeps its files in `~/.amplihack/state/quick-change/<n>`, where `<n>` is the `cksum` of the worktree path. The directory is absolute, outside the worktree and created with `umask 077`; it is exported to the tools as `QUICK_CHANGE_STATE_DIR`. Files are written with `mktemp` followed by `mv`, so a symlink planted at the target path is replaced rather than followed. The directory holds the pinned base, the git configuration snapshot, the checks table and, on hand-over, the patch and note.

`quick-change-create-pr` removes the directory once the PR is open or existing-branch continuation has completed after verified publication. Nothing removes it at the end of a hand-over, a run with verdict `empty` or a failed run: a hand-over patch is the backup for the run that continues it, and a failed run's files are there to inspect. Instead, `quick-change-record-base` deletes every other run's directory that has not been modified for 14 days. So a hand-over leaves a full patch of the change under `$HOME` until a later run starts at least 14 days afterwards, readable only by the user.

## Diff limits

`quick-change-diff-limits` decides between `proceed` and `handover` from git output alone. It never reads agent text.

### Measurement

- The base is the selected checkout's local `HEAD`, pinned by `quick-change-record-base` before the agent ran. It is passed as `--base` and must be a 40-character hex SHA and an ancestor of `HEAD`. Missing or inconsistent base data stops the run; the base is never recomputed from main, origin or the final `HEAD`.
- Measurement includes all applicable changes against that SHA: pre-existing staged and unstaged work, untracked files, implementation edits and repair changes. Commits already present at the pinned `HEAD` are outside this run's diff limits, including local commits absent from origin.
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

**Shared configuration.** A linked worktree's local configuration is the repository's `.git/config`, which every worktree shares ([git-worktree, configuration file](https://git-scm.com/docs/git-worktree#_configuration_file)). Other runs write branch upstream entries there while this one is in progress: `workflow-worktree` runs `git branch --set-upstream-to`, `git worktree add -b` from `origin/main` sets an upstream, and `git push -u` writes one ([git-push](https://git-scm.com/docs/git-push)). This recipe's own push step does the same. Branch entries are therefore compared as follows:

| Snapshot line | What it covers |
| --- | --- |
| `config-local`, `config-worktree` | The local file and the per-worktree file (`config.worktree`), less every `branch.<name>.<key>` entry. Two-part keys such as `branch.autoSetupMerge` still count. |
| `branch-upstream` | `branch.<name>.remote` and `branch.<name>.merge` for the branch checked out in the run's worktree, at every configuration level |
| `branch-other` | Every other `branch.<name>.<key>` entry for that branch, `pushRemote` included, at every configuration level |

Another branch's entries apply only when that branch is checked out, so they are not compared. The run's own entries are, because `default-workflow` pushes with a bare `git push` after a hand-over (`workflow-publish.yaml`), and that push follows them. `push.default`, which decides what a bare push updates, is checked at every level for the same reason.

`--committed` does not compare `branch-upstream`. The recipe's own `git push -u origin <ref>:<ref>` writes it, and it differs from the snapshot whenever the best-effort push in `workflow-worktree` did not happen: the branch then still tracks `origin/main`, as `git worktree add -b` from `origin/main` set it. By the time `--committed` runs, the run can no longer hand over, and the push and PR steps name `origin` and the branch explicitly, so the upstream is not used. Step 4 compares every line.

The per-worktree file is read directly, since `git config --worktree` fails in a linked worktree unless `extensions.worktreeConfig` is set.

**Git configuration drift stops the run.** Any difference from the snapshot gives verdict `stop`. There is no hand-over and no push, because `default-workflow` would otherwise run its own commit and push steps in a worktree whose hooks, `insteadOf` rules or push URL the agent has changed. The snapshot is not restored automatically: a restore covers only the keys in the snapshot, and a change outside them would pass unnoticed. The run fails with:

```text
quick-change: git configuration or hooks differ from the snapshot taken before the agent ran; stopping without hand-over or push
```

The message does not say who made the change. Besides the agent, it can be a check, hook repair, or another process that writes the shared configuration with something other than the entries above, such as a new remote. The `changed` list in the JSON names the snapshot lines that differ.

The checkout and branch are left in place for inspection. Preserve local commits and staged, unstaged and untracked work, including work that predates the run. Before changing the checkout, keep a backup outside it that also includes any needed ignored files; the run's state directory is not a complete checkout backup.

Use the `changed` list to investigate the affected configuration, hooks and remote settings. Resolve unintended changes and verify the effective configuration and push destination before retrying with `existing_branch`. Local configuration and hooks can be shared by other worktrees, so coordinate corrections with other runs. Starting another run only takes a new snapshot; it does not establish that the drift was safe. Checkout removal and branch deletion are optional cleanup only after all needed work is preserved and configuration drift is resolved; neither action repairs shared configuration.

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

`--committed` measures the change after it has been committed, which the commit and push steps do because hook repair can add commits after step 4. The worktree must be clean, so the diff against the base equals `base..HEAD`; otherwise the tool exits 2. Rule 6 (`agent-commit`) is skipped, since the recipe's own commits are expected, and every other rule runs, rule 1 included, without the `branch-upstream` line (see [Shared configuration](#rules)). A `handover` verdict in this mode exits 5 and prints to stderr:

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

The recipe step fails on exit 2 or 4, and on exit 3 unless `allow_no_op` is `"true"`; neither a PR nor a hand-over happens. An empty diff stops the run with `quick-change: no changes produced`; drift stops it with the message above. Otherwise the step's output is the verdict word only: `proceed` or `handover` on exit 0, `empty` on exit 3.

### Empty diff

A task where the agent rightly finds nothing to change, such as a documentation audit, produces an empty diff. `default-workflow` treats that as success when `allow_no_op` is `"true"`, and `quick-change` does the same:

| `allow_no_op` | Empty diff |
| --- | --- |
| `"true"` | Verdict `empty`. Every later step runs only on `proceed` or `handover`, so none runs: no checks, commit, push, PR or hand-over. The run finishes successfully. |
| anything else, including `"True"` and `"1"` | The step exits 3 and the run fails. |

Only the exact value `"true"` counts, as in `default-workflow`'s own no-op check, so a value that happens to look true cannot turn an agent that did nothing into a successful run. The selected branch and checkout are left in place, whether newly created or reused. `allow_no_op` changes nothing else: a non-empty diff gets `proceed` or `handover` as usual, and git configuration drift still stops the run.

## Hand-over to default-workflow

When the verdict is `handover`:

1. `quick-change-write-handover` writes the full binary-capable diff against the pinned base, untracked files included, to `<state dir>/quick-change-handover.patch`, and writes the hand-over note described below. The recovery files are readable only by the user.
2. Nothing is committed or pushed by `quick-change`. The worktree and branch are left as they are, including any commit the agent made against its instructions.
3. For an ordinary new-branch run, `quick-change-handover` runs `default-workflow` with the original task, the note appended, and `existing_branch` set from `worktree_setup.branch_name`. Its exact selector requires that branch to exist on origin.

For an existing-branch continuation, `quick-change-write-handover` saves the patch and note, then fails visibly before delegating. The error explains that continuation cannot proceed within quick-change limits, that `default-workflow` may create or modify a PR, and gives the preserved checkout and patch paths. The checkout, local history, existing work and state directory remain available. The note and patch describe the complete change against the original pinned SHA. Continue that work explicitly in the preserved checkout. Continuation makes no `gh` calls.

For an ordinary new-branch run whose branch exists on origin, `default-workflow` reuses the `quick/` branch through its own steps:

- Step 04 (`workflow-worktree`) finds the worktree already registered for the `quick/` branch and reuses it as it is. It does not reset, clean or check out anything on that path, so the changes from `quick-change`, committed by the agent or not, are still in the worktree when the builder starts.
- `workflow-prep` creates a tracking issue, because `quick-change` did not. This is a second issue when the task already had one.
- The hand-over passes `worktree_policy: quick-change`, so step 04 uses exact checkout selection and performs no setup push. That selector fetches only into the remote-tracking ref, ignoring configured fetch mappings that might overwrite local branches. It requires the checkout's canonical Git common directory to match the caller's repository and separately checks its exact branch name. Publication happens later in `default-workflow`.

After a successful hand-over, smart-execute-routing runs `handover-reselect-workflow`, which sets `selected_workflow` to `default-workflow` and logs the change, so orchestrator reflection and outside-in validation run as for any full workflow. See [Log line](#log-line). This correction is for a run that went through `execute-single-round-1-quick-change`. Each workstream of a parallel run under `-c workflow=quick-change` is a separate process whose outputs do not reach the parent, so such a run still skips those steps after a workstream hands over.

### When the setup push failed

New-branch setup makes a best-effort push before implementation. If that push failed and the branch is absent from origin, saving the local branch does not make it eligible for exact selection: `default-workflow` cannot complete hand-over through this selector. A stale remote-tracking ref is insufficient. A failed setup push does not itself prevent hand-over if the exact branch already exists on origin and validation succeeds.

The patch and note are saved before ordinary new-branch hand-over. If origin validation then fails in `default-workflow` step 04, the checkout, local commits, uncommitted work and recovery files remain available. The workflow may already have created its tracking issue. There is no automatic setup-push retry or replacement branch through the exact selector. Existing-branch continuation always stops before delegation.

Recover in the preserved checkout: inspect the full diff against the pinned base, resolve the push failure, and explicitly publish the branch when ready before continuing with `default-workflow`. Starting a fresh quick-change continuation pins a new base; it does not retain the original run's measurement boundary.

### Hand-over note

The note is appended to the task description that `default-workflow` receives. It holds no patch content and no agent text, only fixed wording and values produced by the bash steps (`qc_handover_note` in `quick_change_common.sh`). The reason is read back from the state directory and must be one of the hand-over reasons in the [Rules](#rules) table, or the step stops. `<state dir>` is replaced by the absolute path of the run's state directory.

The second line depends on whether the branch has commits after the base, which the step reads with `git rev-list --count <base>..HEAD`. Without commits:

```text
Hand-over from quick-change: the change needs the full workflow (reason=public-interface).
The changes are uncommitted in this worktree on branch quick/issue-1740-route-small-tasks.
Base commit: 093edc996...
Patch of the whole change against the base commit: <state dir>/quick-change-handover.patch
Before you start, run `git diff 093edc996...` to see the whole change, committed and uncommitted; `git status` does not show committed changes.
Apply the patch only if that diff is empty: git apply --index <state dir>/quick-change-handover.patch
Continue from these changes; do not start again from the base commit.
```

When the agent committed:

```text
Hand-over from quick-change: the change needs the full workflow (reason=agent-commit).
The changes are on branch quick/issue-1740-route-small-tasks in this worktree: 1 commit(s) after the base commit, and possibly uncommitted changes as well.
...
```

The line is chosen from git, not from the reason. `agent-commit` always comes with commits, but rules 2, 4 and 5 are decided before rule 6, so an agent that committed a large change hands over with `files>N`, `lines>N` or `unparseable-path`, and its commits are still on the branch.

The instruction is the same in both cases. `git diff <base>` compares the worktree with the base, so it shows committed and uncommitted changes, and the files the diff-limits tool marked intent-to-add. The patch is the same diff, so it applies cleanly only to a tree at the base. When the agent committed, `git status` can show a clean tree while the change is in `HEAD`; an instruction keyed on `git status` would have the next agent apply the patch a second time.

The patch is the backup copy. It is needed only if the worktree was changed between the hand-over and the builder step, for example by a person resetting it.

## Checks

`quick-change-run-checks` runs the repository's own checks after the diff limits are measured, and only on `proceed`. An ordinary hand-over leaves checks to `default-workflow`; an oversized continuation stops before checks. The checks may change files (a pre-commit fixer, for example); the commit step measures the committed change again, so such changes are still held to the limits.

```text
amplifier-bundle/tools/quick_change_checks.sh <worktree> <base-sha> <table-out-file>
```

| Repository has | Checks run |
| --- | --- |
| `Cargo.toml` | `cargo fmt --all -- --check`, always. When the change can affect what is compiled (see below), `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo test -p <crate>` for each affected crate, or the same two once with `--workspace`. |
| `.pre-commit-config.yaml` | `pre-commit run --files <changed files>` |
| `package.json` | `npm run lint` and `npm test`, each only when the script exists |

These changes count, whatever the file's name says about Rust:

| Changed file | Effect |
| --- | --- |
| `.rs` file | Its crate |
| `Cargo.lock`, at any depth | The whole workspace, because the lockfile decides what every crate compiles against and `cargo fmt` never reads it ([Cargo resolver](https://doc.rust-lang.org/cargo/reference/resolver.html)) |
| Any other file inside a member crate's directory | That crate, because `include_str!` can compile it in: `crates/amplihack-hooks/src/routing_prompt.txt` is one |
| Any other file under `src/`, `tests/`, `benches/` or `examples/` at the repository root | The root package, or the whole workspace when the root is also the workspace root |

Other files, such as `docs/index.md` or a root `README.md`, run no clippy or test.

An affected crate is the nearest ancestor directory of the changed file whose `Cargo.toml` has a `[package]` section. When a counted file has no such directory, when that `Cargo.toml` is also the workspace root (it has a `[workspace]` section), or when the crate name does not match `^[A-Za-z0-9_-]+$`, clippy and test run once for the whole workspace, and no per-crate check runs:

```text
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

A file outside every package directory can be compiled only through a `path` key on a target ([Cargo targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#configuring-a-target)) or a `#[path]` attribute. In this repository, `bins/amplihack/Cargo.toml` compiles `tests/integration/main.rs` that way, so a change under `tests/integration/` runs the workspace checks. A root package is not trusted to cover its directory, because a member can compile files anywhere under it in the same way.

Checks run with `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` removed from the environment. Their output goes to stderr only, never into the PR, the commit or recipe outputs.

Stdout is one line:

```text
QUICK_CHANGE_CHECKS=failed ran=3 failed=1 skipped=2
```

| Status | Meaning |
| --- | --- |
| `passed` | At least one check ran and none failed |
| `failed` | At least one check failed |
| `none` | No check ran. The PR is a draft and its body says "No checks ran" |
| `unverified` | Checks ran and passed, but none could fail on the change. The PR is a draft and its body says "No build or test covered this change; the checks that passed do not read it" |

`unverified` applies when Rust code or `Cargo.lock` changed and no cargo clippy or test ran, when `package-lock.json`, `yarn.lock` or `pnpm-lock.yaml` changed and `npm test` did not run, and whenever `uv.lock` or `poetry.lock` changed, since this tool has no check that reads them. For example, a `pre-commit` pass with `cargo` missing from `PATH` does not make a lock update ready for review. `failed` takes priority over `unverified`, and `none` applies when nothing ran.

A skipped check is never counted as passed. Skip reasons come from a closed set:

| Reason | When |
| --- | --- |
| `cargo not on PATH` | `Cargo.toml` exists but `cargo` does not |
| `no .rs files changed` | clippy and test, when no changed file counts as Rust (see above) |
| `pre-commit not on PATH` | `.pre-commit-config.yaml` exists but `pre-commit` does not |
| `npm not on PATH` | `package.json` exists but `npm` does not |
| `jq not on PATH` | `package.json` scripts cannot be read |
| `no "lint" script` / `no "test" script` | The script is absent from `package.json` |

A failing check does not stop the recipe. A new PR is opened as a draft and its body says "Checks failed"; continuation reports the warning without modifying the existing PR. The final step takes the checks result from the runner's output of `quick-change-run-checks`, and stops if the copy in the state directory differs from it, so a rewritten summary file cannot turn a failed run into a ready-for-review PR. The tool exits 0 once the checks have run and 2 on a usage error.

The PR step also checks every row of the stored table against the form the tool writes (a code-span label, then `exit N` or one of the skip reasons above) and the row counts against the summary, so the table cannot carry other text into the PR body. These patterns in `quick_change_common.sh` repeat strings that `quick_change_checks.sh` and `quick_change_diff_limits.sh` produce. `issue_1740_state_gates` feeds `qc_checks_summary`, `qc_checks_table` and `qc_handover_reason` what the two tools print, covering every skip reason and every hand-over reason, and reads both tools' source: a new skip reason or hand-over rule fails that test until the gate accepts it, rather than failing a run after the branch is pushed or the agent's work is done.

## Pull request

This section describes ordinary new-branch publication with verdict `proceed`. Existing-branch continuation completes after verified publication and warning reporting, before any `gh` operation. Only an ordinary new-branch run can [hand over](#hand-over-to-default-workflow) to `default-workflow` and its normal PR handling.

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
| `cargo clippy --all-targets -- -D warnings` | skipped (no .rs files changed) |
| `cargo test` | skipped (no .rs files changed) |
| `pre-commit run --files docs/index.md` | exit 0 |
| `npm run lint` | skipped (no "lint" script) |
| `npm test` | exit 0 |
```

Discovery covers Cargo, pre-commit and npm. In any other repository, or when the tools are missing from `PATH`, no check runs, the table shows at most skipped rows, and the PR opens as a draft that starts with "No checks ran".

The Checks table above is what `quick_change_checks.sh` wrote for a one-line change to `docs/index.md` in this repository, whose root has `Cargo.toml`, `.pre-commit-config.yaml` and a `package.json` with a `test` script and no `lint` script.

| Item | Rule |
| --- | --- |
| Title | The most recent usable change commit subject after the pinned base, or a descriptive path-based fallback. See [Commit subjects and PR titles](#commit-subjects-and-pr-titles). |
| Request | The task text, cut to 4000 bytes on a UTF-8 boundary |
| Issue reference | Preserves an explicit `Fixes #N` for the selected issue independently of the title and request truncation; otherwise appends `Refs #N`. See [Issue references](#issue-references). |
| Draft | When `QUICK_CHANGE_CHECKS=failed`, `none` or `unverified`, or when hook repair ran in the commit or push step |

The body's first section is headed "Request" because it holds the task text as given. The commit message body is the same text. The independently generated subject describes the actual change; only a validated subject is eligible for the PR title.

When the checks failed, the body starts with "Checks failed"; when no check ran, it starts with "No checks ran"; when the status is `unverified`, it starts with "No build or test covered this change; the checks that passed do not read it". When hook repair ran, it also says "Hook repair changed the code after the checks ran; the checks table describes the pre-repair tree." The create-PR step reads `quick_change_commit_repaired` and `quick_change_push_repaired` from the runner's outputs, never from the state directory, and fails when either is missing.

The push uses an explicit refspec, `git push -u origin refs/heads/<branch>:refs/heads/<branch>`, only after the snapshot (which holds a checksum of the push URL) still matches, and is followed by `verify_pushed_ref.sh`. `gh pr create` runs once, without timed retries, interval probes or a wall-clock window. A failure is reported directly and leaves the local work available for recovery.

### Commit subjects and PR titles

The builder returns a structured change summary and proposed `commit_subject`; it does not commit or publish. The subject describes the change independently of the task's first line. The publication step validates the proposal before using it as a commit subject.

The validator rejects an empty proposal, a path alone, a leading `Brief:` (case-insensitive), multiple lines or control characters. Usable overlong subjects are shortened at a word boundary to at most 72 characters. Rejection selects a deterministic fallback describing the change category and naming the main applicable changed path, such as `docs: update docs/index.md`.

PR-title selection reads actual commit subjects in `base..HEAD`. Commits already reachable from the pinned base are excluded. Path-only subjects and subjects beginning `Brief:` are ineligible. If no usable subject exists, selection uses the descriptive fallback. Neither route truncates the task's first line to make a title.

Title selection reads newest first and chooses the most recent usable subject, including a later usable repair subject. Overlong subjects are shortened to at most 72 characters at a word boundary, without a cut-off word or trailing ellipsis. Invalid subjects are rejected before shortening.

The fallback chooses the path with the greatest additions-plus-deletions count in the measured diff. Stable Git path ordering breaks ties. Binary files are eligible with a line count of zero. The title preserves the complete path, including long paths; control characters are escaped so unusual filenames cannot make a multiline title. The fallback is never only a path.

For example, this task starts with a brief path:

```text
Brief: docs/reference/quick-change-recipe.md
Correct the recipe CLI link in docs/index.md for #2240.
```

A suitable subject is `docs: correct the recipe CLI link in docs/index.md`. With no other eligible change commit, that subject becomes the new PR's title. The body gets `Refs #2240`, independently of the brief line and title selection. The task above does not explicitly request a closing reference.

### Issue references

`issue_number` takes precedence over issue references found in task text. When it is empty, the first valid `#N` in the full task text selects the issue. An invalid nonempty `issue_number` produces no generated reference; it does not fall back to task text. Numbers must match `^[1-9][0-9]{0,8}$`. No issue is inferred from a generated title.

Publication resolves the reference from the full task before truncating the request. It preserves an explicit `Fixes #N` for the selected issue in a separate reference line in both the commit message and new PR body; otherwise it emits `Refs #N`. A closing reference to a different issue does not change the selected issue or turn its generated reference into `Fixes`. For example, `issue_number=2240` with `Fixes #2210` in the task yields `Refs #2240`; an explicit `Fixes #2240` beyond byte 4000 still yields `Fixes #2240`. Continuation uses the commit reference without editing the existing PR.

### Hook repair

Every commit and push honours the configured hooks. Commit-hook repair uses the loop described in [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md) for both new-branch and continuation runs. Its fixing agent can change files after step 4 measured the diff, so the commit step runs `quick_change_diff_limits.sh --committed` against the pinned base after the commit succeeds. An oversized result stops before publication and retains the local work and state.

Continuation remeasures the committed change against the original pinned base immediately before pushing. An oversized result or rejected push stops the run, retaining the checkout, commits and state. Oversized continuation also saves a private binary patch. Continuation does not enter automatic push repair, which could publish further commits before measurement. No PR lookup, creation or edit follows a rejected push.

Ordinary new-branch runs retain push-hook repair. That loop commits the fix and pushes it itself, so its additional measurement runs after the push. A failure leaves the branch on `origin`, but stops the run before a PR is opened.

When repaired work stays within the limits, an ordinary new-branch PR opens as a draft because the checks table describes the tree before repair. Successful continuation reports the same repair warning through the run log before completing, without changing the existing PR's body or draft status.

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

After the table, a `quick-change` result becomes `default-workflow` when the run already has a branch. This is the orchestrator's routing policy; direct quick-change runs support [existing-branch continuation](#existing-branch-continuation). The source is kept. The checks are made in the order `workflow-worktree` uses:

| Condition | `reason=` | Warning when the source is `override` |
| --- | --- | --- |
| `existing_branch` or `pr_number` is set | `existing_branch or pr_number set` | W3 |
| Neither is set, and the task text names a branch | `task names a branch` | W4 |

"Names a branch" uses the directive rules of `workflow_branch_name.sh explicit` (#1426), which `workflow-worktree` runs: a line starting `Branch:`, `Branch name:`, `Branch ref:` or `Branch =` (any case) with the value on the same line or the next non-blank one, or `Branch`, a dash (`-`, `–` or `—`), any text and a final `:`, with the value on the next non-blank line. The value must be a valid ref containing `/` or `-`, not a base branch name and not starting with `origin/`, `refs/` or `heads/`. Prose such as "Branch coverage is low in: src/foo-bar.rs" does not count. Whether the branch exists does not matter. The helper reads the task text from `RECIPE_VAR_task_description`; `crates/amplihack-cli/src/commands/orch/named_branch.rs` repeats the shell tool's patterns, a unit test there fails when the shell tool's patterns or bounds change, and `issue_1740_select_workflow` runs both on each directive form and fails when they disagree.

A valid override applies however many workstreams there are. With `-c workflow=quick-change` and a parallel decomposition, every Development workstream runs `quick-change` and orchestrator reflection is skipped. Without an override, every Development workstream of a parallel decomposition runs `default-workflow`, whatever recipe the model gave it, and a non-Development workstream that the model gave `quick-change` runs its classification's own recipe instead; see [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md#routing-invariant). The log line therefore names the recipe the workstreams run, with the one correction described under [Log line](#log-line). Non-Development routing (Q&A, Operations, Investigation) never changes.

Override values are trimmed and lower-cased, then matched against the exact list `quick-change`, `default-workflow`.

### Warnings

```text
[dev-orchestrator] WARNING: unknown workflow override 'quick' ignored
[dev-orchestrator] WARNING: workflow override 'quick-change' ignored for task_type Q&A
[dev-orchestrator] WARNING: workflow override 'quick-change' ignored: existing_branch or pr_number is set
[dev-orchestrator] WARNING: workflow override 'quick-change' ignored: the task names a branch
```

The first is W1, the second W2, the third W3, the fourth W4. The warning text, the `reason=` values (including `quick-change needs one workstream; got N`, `task names a branch` and `unspecified`) and the eight rows of the precedence table are fixed by this page. The unit tests in `select_workflow.rs` assert them exactly.

### Log line

Every orchestrated run writes one line to stderr after any warnings:

```text
[dev-orchestrator] Workflow: <quick-change|default-workflow|none> source=<override|classifier|default|handover> reason=<text>
```

Two cases write a second line. When `quick-change` was forced on a run that then falls back to `default-workflow`, because the recursion guard blocked the parallel launch or adaptive recovery took over, `blocked-fallback-reselect-workflow` or `adaptive-reselect-workflow` sets `selected_workflow` to `default-workflow` and logs the change. When a single-workstream `quick-change` hands over, `handover-reselect-workflow` does the same with `source=handover`, as the selection may have come from the classifier or an override. In each case the last line names what ran, and reflection and validation run as for any full workflow:

```text
[dev-orchestrator] Workflow: default-workflow source=override reason=parallel launch blocked; the single-session fallback runs default-workflow
[dev-orchestrator] Workflow: default-workflow source=handover reason=quick-change handed over; default-workflow ran
```

The one exception is a parallel run forced to `quick-change`, described under [Hand-over to default-workflow](#hand-over-to-default-workflow).

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
| Development | `quick-change`, with `existing_branch` or `pr_number` set, or a task text that names a branch | any | `default-workflow` |
| Development | empty or invalid, or the top-level `task_type` is not Development | any, `quick-change` included | `default-workflow` |
| Non-Development | any | `quick-change` | the classification's own recipe: `investigation-workflow`, `qa-workflow` or `consensus-workflow`, and `default-workflow` for Operations |
| Non-Development | any | anything else | unchanged |

The helper reads the override from `RECIPE_VAR_workflow`, the two targets from `RECIPE_VAR_existing_branch` and `RECIPE_VAR_pr_number`, and the task text from `RECIPE_VAR_task_description`: the same values `select-workflow` receives. The log line that `select-workflow` writes therefore names the recipe every Development workstream runs: `quick-change` only under a valid override, otherwise `default-workflow`, with `reason=quick-change needs one workstream; got N` when the classifier recommended `quick-change`. W1 to W4 are printed by `select-workflow`; the helper prints W1 again for an invalid override. The blocked-fallback and adaptive execution paths always use `default-workflow`, and correct `selected_workflow` when it said `quick-change`; see [Log line](#log-line).

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
| `RECIPE_VAR_task_description` (environment) | The task text, read by the helper itself so it never goes through argv. A `Branch: <ref>` line rules out `quick-change`. |
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

When `selected_workflow` is still `quick-change` after execution, these smart-orchestrator steps do not run:

| Recipe | Steps |
| --- | --- |
| `smart-reflect-loop` | `reflect-round-1`, `execute-round-2`, `reflect-round-2`, `execute-round-3`, `reflect-final` |
| `smart-validate-summarize` | `validate-outside-in-testing` |

`summarize` and `complete-session` still run, including after a run that finished with verdict `empty`.

Reflection rounds and outside-in validation exist to judge a full workflow run. Outside-in validation looks for Step 13 and qa-team evidence that `quick-change` never produces, so running it would start extra rounds and remove the saving the recipe is for. A run that hands over has run `default-workflow`, which does produce that evidence, so `handover-reselect-workflow` changes the selection and these steps run.

`selected_workflow` is declared in the context of `smart-orchestrator`, `smart-classify-route`, `smart-execute-routing`, `smart-reflect-loop` and `smart-validate-summarize`, so each condition can read it.

## Recursion limits

`quick-change.yaml` sets `max_depth: 6` and `max_total_steps: 140`. On the pinned runner, limits are counted from the top-level recipe, and a `recipe` step fails when the depth it runs at has reached `max_depth`. The longest path is smart-orchestrator, smart-execute-routing, `quick-change`, then a hand-over into `default-workflow` and its sub-recipes: about 146 of 200 steps, counting `handover-reselect-workflow` and the reflection and validation steps that run after a hand-over, with the deepest sub-recipe (`workflow-terminal-adjudication`) at depth 5 of 6. A change to the runner pin can change how these are counted.

`issue_1740_quick_change_recipe::handover_nesting_leaves_one_level_spare` walks the `recipe:` steps of the bundled YAML from smart-orchestrator, and from `quick-change` run on its own, through `quick-change` and its hand-over, and fails when any sub-recipe on those paths sits deeper than `max_depth - 1`. A new level of sub-recipe under `default-workflow` would use that spare level, and the test fails before it ships.

## Security limits

The agent's prompt instructions are not relied on. The controls are in the bash steps:

- Tools are resolved from `AMPLIHACK_HOME`, `RECIPE_VAR_repo_path` or the two `$HOME` install locations, never from the current directory. A tools path inside the agent's worktree is refused.
- The diff base is pinned before the agent runs. An agent commit triggers hand-over for an ordinary new-branch run; continuation preserves the work and stops before delegation. Git configuration drift and hook changes stop either run with no hand-over and no push.
- Hook-generated changes are measured again after commit. Continuation remeasures immediately before push and stops on rejection without push repair. Ordinary new-branch push repair is measured after publication; an oversized result fails the run. See [Hook repair](#hook-repair).
- The diff-limit step and the push step compare git configuration, hooks and the push URL with the snapshot taken before the agent ran, and refuse to continue when any of them differ. The snapshot covers remotes, URL rewrites, `http.*`, credential helpers, `core.hooksPath`, `core.sshCommand`, `core.fsmonitor`, `core.gitProxy`, `core.askPass`, `protocol.*`, includes, clean and smudge filters, `core.attributesFile`, commit signing and `push.default`, at every configuration level, plus the local and per-worktree configuration files and the hooks directory. The configuration files leave out `branch.<name>.<key>` entries, which other runs write; the entries of the run's own branch are compared at every level instead, as described under [Shared configuration](#rules).
- Every step after the agent reads the pinned base from the state directory and checks it against the value the runner holds in memory (`quick_change_base`), so rewriting the stored base stops the run.
- The push goes to `origin` with an explicit refspec and no force, mirror, tags or delete, and only after the snapshot comparison passes. No step merges.
- Existing-branch setup validates and fetches the exact origin ref before setup publication or implementation. Local-only and stale tracking refs do not establish remote existence. Branch identity must equal the supplied `existing_branch` before implementation, commit and push; protected-branch refusal applies to both continuation and new-branch runs.
- Untrusted text reaches bash only through `RECIPE_VAR_*` variables; no `command:` block contains `{{`. No step uses `eval`, `bash -c "$VAR"` or `set -x`.

Known limits:

- The public-interface rule covers Rust only. An interface change in another language that fits within the size limits proceeds and is left to PR review.
- The public-interface rule reads changed lines only. A change to a later line of a multi-line `pub` signature (a parameter on its own line, a where clause) leaves the `pub` line unchanged and proceeds when it fits within the size limits. `#[macro_export]` macros are also missed, as they are not declared with `pub`.
- Checks run code from the repository. Tokens are removed from the environment, but credential files on disk remain readable.
- The agent and the check commands run with the user's full privileges and are not sandboxed. The controls above stop changes inside the worktree and to git configuration from weakening the gate, but they cannot stop a process that edits files outside the worktree: the installed tools under `AMPLIHACK_HOME` or `repo_path`, the git configuration snapshot in the state directory, or `~/.ssh/config`. PR review is the control for those cases.
- An existing lockfile is excluded from the line limit. A large lockfile change, including one that points a package at a different download URL, proceeds when the other limits are met and is left to PR review.
- Changed file names are passed to `pre-commit run --files` as they are. An agent-created file whose name starts with `-` is read by pre-commit as an option.
- A new file created with the executable bit is counted like any other new file; only a mode change to an existing file, or a symlink, forces a hand-over.
- When ordinary new-branch push repair grows the change beyond the limits, the branch is already on `origin` and the run fails without opening a PR. Continuation stops on rejection without push repair; see [Hook repair](#hook-repair).
- The remote-existence check is point-in-time validation. Later remote changes remain subject to non-force push and remote-SHA verification; the recipe does not poll GitHub or reset local history to accommodate them.
- The repository configuration is shared by every worktree. Other branches' entries are left out of the drift check, but any other change another process makes there while a run is in progress, such as adding a remote, stops that run as drift.
- A file outside every package directory that a crate compiles in through a relative path, such as `include_str!("../../../docs/x.md")`, runs no clippy or test here, because the tool does not read `include_str!` calls. CI builds it.
- A changed `.rs` file is checked with the package of the nearest `Cargo.toml` that has a `[package]` section and is not the workspace root. A file that a different package compiles through a `path` key or a `#[path]` attribute, but that sits under another member's directory, is checked with that member, whose checks do not build it.
- `.gitignore` is the boundary for secrets. An ignored file is neither counted nor committed; a secret in a tracked or new unignored file is caught only by the artifact guard before the commit.

## Out of scope

- Review, CI repair, merge and finalisation. Use [Auto Drive To Merge](./auto-drive-to-merge.md) for those.
- Discovering or modifying the existing PR during continuation, including changing its title, body or draft status.
- Version bumps. `quick-change` does not use `workflow-publish`.
- Adaptive workflow selection (#1708).

## Related

- [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md) - The routing invariant and workstream normalisation
- [Verify Dev-Orchestrator Routing](../howto/verify-dev-orchestrator-routing.md) - Check the routing choice from the CLI
- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md) - What happens when a hook rejects the commit or push
- [Pass Large Recipe Context and Agent Text](../howto/pass-large-recipe-context.md) - Why bodies and messages go through files
