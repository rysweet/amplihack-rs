# Publish: PR Head Sync, Task-Scoped Step-15 Commits and gh Auth Backoff

**`default-workflow` pushes a branch's unpushed commits before the publish terminal-state probe, commits only changes attributable to the task under a message built from the staged diff, and retries a transient `gh` authentication drop at step 21.**

> [Home](../index.md) > [Features](README.md) > Publish: PR Head Sync and Task-Scoped Commits

## Contents

- [Scope](#scope)
- [PR head sync before the probe](#pr-head-sync-before-the-probe)
- [Step-15 staging](#step-15-staging)
- [Step-15 commit message](#step-15-commit-message)
- [Step-15 push](#step-15-push)
- [Step-21 authentication and push-permission gates](#step-21-authentication-and-push-permission-gates)
- [Configuration](#configuration)
- [Recovering excluded changes](#recovering-excluded-changes)
- [Known limitations](#known-limitations)
- [Verification](#verification)
- [Related docs](#related-docs)

---

## Scope

Issue [#1575](https://github.com/rysweet/amplihack-rs/issues/1575) fixed three
defects in two phases: steps 15 and the new `publish-sync-pr-head` in
`workflow-publish.yaml`, and step 21 (`step-21-pr-ready`) in
`workflow-finalize.yaml`:

| Defect | Recipe step | Helper in `amplifier-bundle/tools/` |
|---|---|---|
| A run on an existing PR branch reaches the terminal-state probe with unpushed commits | `publish-sync-pr-head` (new), `step-15-commit-push` | `workflow_sync_pr_head.sh` |
| Step 15 commits everything, including version churn and generated hook stubs | `step-15-commit-push` | `workflow_stage_task_changes.sh` |
| Step 15 uses the task description as the commit subject | `step-15-commit-push` | `workflow_commit_message.sh` |
| Step 21 fails the run on a brief `gh` authentication drop | `step-21-pr-ready` | `workflow_gh_auth_gate.sh`, sourced by `workflow_pr_ready.sh` |

Command-line contracts, exit codes and exact output strings are in the
[Workflow Publish Helpers Reference](../reference/workflow-publish-helpers.md).

---

## PR head sync before the probe

### What went wrong

`probe-terminal-state` compares the PR's `headRefOid` with the local `HEAD`
and refuses with:

```text
FAILED_INVALID_INPUT: stale PR metadata: headRefOid '63e9b0d…' does not match local HEAD '7cd9cd6…'
```

Any run that continues an existing PR (`pr_number`, `existing_branch` or
`resume_checkpoint`) can make commits before it reaches publishing. Nothing
pushed them, so the probe refused, and the commits had to be pushed by hand.

### What happens now

`workflow-publish.yaml` runs a new bash step, `publish-sync-pr-head`, as its
first step, immediately before `publish-terminal-state`. It runs
`workflow_sync_pr_head.sh --mode pre-probe` in the task worktree.

The step runs on every publish. It acts only when the branch's remote
counterpart is a strict ancestor of `HEAD`, so the same rule covers
`resume_checkpoint`, `existing_branch`, `pr_number` and any other run whose
branch is ahead of its remote. It never stages or commits; uncommitted work is
still step 15's job.

The helper:

1. Reads the current branch with `git symbolic-ref`. A detached `HEAD` is a
   no-op.
2. Refuses a protected branch before any network call: `main`, `master`, the
   branch named by `refs/remotes/<remote>/HEAD`, and the value of
   `base_branch` (`RECIPE_VAR_base_branch` or `BASE_BRANCH`).
3. Chooses the remote from `branch.<branch>.remote`, or `origin` when that is
   unset. A value of `.` (a branch tracking a local branch) is a no-op.
4. Runs `git fetch <remote> <branch>` and takes the remote commit from
   `FETCH_HEAD`, not from the tracking ref, so a narrowed fetch refspec cannot
   produce a stale comparison.
5. Counts `ahead` and `behind`. When `ahead > 0` and `behind == 0` it pushes
   with `git push <remote> HEAD:refs/heads/<branch>` and confirms the result
   with `verify_pushed_ref.sh`.

The push target is always `refs/heads/<current branch>`. The upstream's merge
ref is never used, so a branch created from `origin/main` with
`branch.<branch>.merge = refs/heads/main` cannot push to `main`.

### Outcomes

The pre-probe step always exits `0`. The terminal-state probe that follows is
the gate: if the PR head still does not match `HEAD`, the probe fails closed
with `stale PR metadata`, and the helper's message has already explained why.

| Situation | Helper action | Probe result |
|---|---|---|
| Remote branch is an ancestor of `HEAD` (`ahead > 0`, `behind == 0`) | Push, then verify | Passes |
| Nothing to push (`ahead == 0`) | One `INFO` line | Passes if the PR head is current |
| No remote branch yet (fresh run) | One `INFO` line | Unchanged; step 15 publishes the branch |
| Detached `HEAD`, or `branch.<branch>.remote` is `.` | One `INFO` line | Unchanged |
| `git fetch` fails | `WARNING`, no push | Fails closed if the head is stale |
| Diverged (`ahead > 0`, `behind > 0`) | `ERROR` block with recovery commands, no push | Fails closed with `stale PR metadata` |
| Push rejected or verification mismatch | `ERROR` with redacted git output | Fails closed |
| Protected branch | `ERROR`, no fetch or push | Unchanged |
| Helper not found | The step prints a `WARNING` | Unchanged |

On divergence the helper prints the same evidence as step 15 (see
[Step-15 push](#step-15-push)), so the operator sees both commit IDs, the merge
base and the commands to resolve it before the probe's message.

`workflow-finalize.yaml` has no PR head sync step before its own probe. Steps
18c and 20b already push their own commits before `finalize-terminal-state`
runs. Its only #1575 change is the step-21 gate in
[Step-21 authentication gates](#step-21-authentication-and-push-permission-gates).

---

## Step-15 staging

### What went wrong

Step 15 ran `git add -A`. On one branch it committed a crate version bump
(`Cargo.toml` and `Cargo.lock`, 0.5.0 to 0.6.0) in a repository that cuts
releases through automated release PRs. It also swept generated
`.github/hooks/*` stubs into commits.

### What happens now

Step 15 no longer contains `git add -A`. The order is:

1. The existing guards and the first `artifact-guard --mode pre-publish` pass.
2. `workflow_stage_task_changes.sh`, which stages the worktree and then
   removes generated paths and version churn from the index.
3. A second `artifact-guard --mode pre-publish` pass over what is now staged.

If step 15 cannot find `workflow_stage_task_changes.sh`,
`workflow_commit_message.sh` or `workflow_sync_pr_head.sh`, it stops before
staging. It does not fall back to `git add -A`.

#### Generated paths

Every staged path under `.github/hooks/` is unstaged, whatever the value of
`allow_version_bump`, and each one is named in a `WARNING`:

```text
WARNING: step-15 unstaged generated path .github/hooks/pre-commit (added to info/exclude)
WARNING: step-15 unstaged generated path .github/hooks/post-checkout (tracked; modification left in the working tree)
```

Untracked hook paths are also appended to
`$(git rev-parse --git-path info/exclude)` under this header, one anchored line
each, never duplicated:

```text
# amplihack #1575: generated paths
/.github/hooks/pre-commit
```

That keeps later `git add -A` calls in steps 18c and 20b from picking them up.
Only paths matching `^\.github/hooks/[A-Za-z0-9._/-]+$` and containing no `..`
are written; any other hook path is unstaged but not added to the file.
`.gitignore` is never modified.

#### Version churn

The helper checks every staged `Cargo.toml`, `Cargo.lock` and `package.json`,
at any depth. A file is **version churn** when every changed line in its staged
diff is a version line for the project's **own** version. A changed line
counts only when it matches the pattern **and** sits in the right place in the
file:

| File | Pattern | Where the line must be |
|---|---|---|
| `Cargo.toml` | `^[+-]\s*version\s*=` | Under a `[package]` or `[workspace.package]` header. `version` keys under `[dependencies.foo]`, `[workspace.dependencies.foo]` or any other table do not count. |
| `Cargo.lock` | `^[+-]\s*version\s*=` | In a `[[package]]` entry with no `source =` line, which is how Cargo records workspace and path packages. Registry and git packages always have `source`. |
| `package.json` | `^[+-]\s*"version"\s*:` | The top-level `"version"` key: depth 1, counting `{` and `}` outside JSON strings. Nested keys such as `"dependencies": {"x": {"version": …}}` do not count. |

To find where a line sits, the helper reads the hunk headers of
`git diff --cached -U0`, then walks the staged blob (`git show :<path>`) for
`+` lines and the `HEAD` blob (`git show HEAD:<path>`) for `-` lines with
`awk`, tracking the current table header or brace depth up to that line
number. A file added in this commit has no `HEAD` blob and is never churn.

A version line in any other place is an ordinary change. A file with an
own-version line and any other change is **mixed**. So a dependency upgrade in
`Cargo.toml` (`[dependencies.serde]` `version = "1.0.210"` → `"1.0.215"`)
always stays staged, with or without a version bump beside it.

| Staged version files | `allow_version_bump` and step 14 | Result |
|---|---|---|
| None | any | Nothing to do |
| All churn | bump allowed (see below) | Kept staged; one `INFO` line |
| All churn | bump not allowed | Saved to a patch, unstaged and restored in the working tree; one `WARNING` per file |
| At least one mixed | any | Every version file stays staged; one `WARNING` per mixed file |

A bump is allowed only when **both** of these hold:

- `allow_version_bump` is `true`, `1` or `yes` (the default is `"false"`).
- Step 14 asked for a bump: `new_version` and `current_version` from its
  `version_bump` output are both non-empty and differ. The helper reads them
  from `VERSION_BUMP_NEW_VERSION` and `VERSION_BUMP_CURRENT_VERSION`, falling
  back to `RECIPE_VAR_version_bump__new_version` and
  `RECIPE_VAR_version_bump__current_version`, the same names the recipe
  runner uses for `VERSION_BUMP_CHANGE_CLASSIFICATION`. If step 14 left no
  output, or either value is empty, there was no bump request and churn is
  excluded even when `allow_version_bump` is `true`.

Version files are handled together, never one by one, so `Cargo.toml` and
`Cargo.lock` stay consistent for `--locked` builds. Mixed files stay staged
because a dependency change and a version change in the same file cannot be
separated safely without splitting hunks.

When churn is excluded, the helper:

1. Writes the staged diff of those files to
   `$(git rev-parse --git-path amplihack/excluded-version-churn.patch)` with
   mode `0600` (written to a temporary file, then renamed into place).
2. Checks the patch is non-empty and passes `git apply --check`.
3. Runs `git restore --staged --worktree` on those files, so steps 18c and 20b
   cannot commit the churn later.
4. Prints, for each file:

   ```text
   WARNING: step-15 excluded version-only change in Cargo.toml (allow_version_bump=false); saved to /home/ryan/src/amplihack-rs/.git/worktrees/issue-1575/amplihack/excluded-version-churn.patch
   ```

   When `allow_version_bump` is `true` but step 14 asked for no bump, the
   parenthesis reads `(no version bump requested by step 14)` instead.

If any of these steps fails, the helper restores the index it saved at the
start (`git write-tree`, then `git read-tree`) and exits `2`; step 15 stops
with `reason: "stage-failed"`.

If nothing is left staged, step 15 takes its existing nothing-to-commit path
and still runs the push in [Step-15 push](#step-15-push), so earlier unpushed
commits reach the remote.

The exclusion lists are not a secret scanner. The artifact guard passes before
and after staging remain the check for unwanted files.

---

## Step-15 commit message

The task description is no longer used anywhere in the commit message. It
still chooses the prefix when step 14's classification is missing, as
described in [Publish Workflow Versioning](../WORKFLOW_PUBLISH_VERSIONING.md#how-the-prefix-is-resolved).

### Subject

`workflow_commit_message.sh subject "<prefix>"` builds the subject from the
staged index:

| Staged files | Subject |
|---|---|
| One | `fix: update amplifier-bundle/tools/workflow_pr_ready.sh` |
| Several, in up to three top-level directories | `fix: update 7 files in amplifier-bundle, docs, tests` |
| Several, in more than three | `feat: update 12 files in amplifier-bundle, crates, docs, …` |
| Files at the repository root | the root is listed as `.`, for example `fix: update 2 files in .` |
| git unavailable | `fix: update files` |

Control characters are removed, and the subject is cut to 72 characters (not
bytes), prefix included.

### Body

`workflow_commit_message.sh body` prints
`git diff --cached --stat=72`, at most 30 lines, followed by
`… (stat truncated)` when the stat is longer. Step 15 then adds a blank line
and the issue reference line, unchanged from before: `Closes #N` or `Refs #N`
on GitHub (see #1361), `AB#N` on Azure DevOps, `Ref #N` elsewhere.

The previous `Implements issue #N` line and the three boilerplate bullets are
gone.

Example for a `PATCH` run on issue #1575:

```text
fix: update 4 files in amplifier-bundle, docs, tests

 amplifier-bundle/recipes/workflow-publish.yaml     | 92 +++------
 amplifier-bundle/tools/workflow_sync_pr_head.sh    | 171 ++++++++++++++++
 docs/features/workflow-publish-task-attribution.md | 40 ++--
 tests/integration/issue_1575_publish_defects_test.rs | 388 +++++++++++
 4 files changed, 617 insertions(+), 74 deletions(-)

Closes #1575
```

---

## Step-15 push

Step 15's ancestry check from
[#978](workflow-publish-ancestry-aware-step15.md) now lives in
`workflow_sync_pr_head.sh`, so step 15 and `publish-sync-pr-head` apply one
policy. Step 15 keeps its existing no-upstream check (REL-002) and then calls
the helper with `--mode step15`. The helper prints one result line, which
step 15 copies into `commit_result`:

```text
pushed=true sha=7cd9cd6e0b6d0d3f4c2b8f6a1a2e5c9d4b3a7f10 reason=pushed
```

| Situation | `commit_result.reason` | Step 15 exit |
|---|---|---|
| Fast-forward push verified | `pushed` | `0` |
| Nothing to push | `already-pushed` | `0` |
| No upstream configured (REL-002, checked before the helper) | `no-upstream` | `0` |
| Diverged | `diverged-upstream: ahead=… behind=… merge_base=… upstream=…` | `1` |
| Push rejected | `push-failed: <redacted git output>` | the push's exit code |
| Remote ref does not match `HEAD` after the push | `push-verify-failed` | `1` |
| Protected branch | `protected-branch` | `1` |
| A helper was not found | `helper-missing:<helper name>` | `2` |
| `workflow_stage_task_changes.sh` failed and rolled back the index | `stage-failed` | `2` |
| `HEAD` is detached | `detached-head` | `0` |
| `branch.<branch>.remote` is `.` | `local-tracking` | `0` |
| No remote branch and no tracking ref | `no-remote-branch` | `0` |
| Branch or remote name fails validation | `invalid-ref` | `1` |

In step15 mode a failed fetch falls back to the tracking ref
`refs/remotes/<remote>/<branch>`, as step 15 did before.

On divergence both modes print the #978 evidence, a hint about amended
commits, and the recovery commands:

```text
ERROR: step-15 refuses to rebase divergent history onto upstream 'origin/feat/issue-1575-implement-that-issue-on' (issue #978).
  ancestry: ahead=1 behind=1 merge_base=3f1c0a9… head=7cd9cd6… upstream=origin/feat/issue-1575-implement-that-issue-on remote_head=63e9b0d…
  Auto-rebasing would replay already-integrated commits and create add/add conflicts.
  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision.
  Hint: ahead and behind together often mean a commit that was already pushed was amended locally.
  Recovery, run by hand (this helper never rebases or force-pushes):
    git fetch origin
    git log --oneline @{u}..HEAD
    git log --oneline HEAD..@{u}
    then either: git rebase @{u}
    or, if your local commits replace the remote ones: git push --force-with-lease
```

In pre-probe mode the first line names `publish-sync-pr-head` instead of
`step-15`. The helper prints these commands; it never runs them.

---

## Step-21 authentication and push-permission gates

### What went wrong

This host shares `~/.config/gh/hosts.yml` across concurrent workstreams, so
the active `gh` account can briefly change or appear unauthenticated.
`step-21-pr-ready` ran `gh auth status` once and failed the whole run at its
last step.

### What happens now

`workflow_pr_ready.sh` sources `workflow_gh_auth_gate.sh` and runs two gates.

**1. Authentication gate.** Runs `timeout 30 gh auth status` up to
`WORKFLOW_GH_AUTH_ATTEMPTS` times. Before retry `i` it waits
`min(WORKFLOW_GH_AUTH_BASE_DELAY × 2^(i-1), WORKFLOW_GH_AUTH_MAX_DELAY)`
seconds and prints:

```text
WARNING: gh auth status failed (attempt 1/5); retrying in 4s
```

With the defaults the waits are 4, 8, 16 and 32 seconds: 60 seconds of sleep,
and about 3.5 minutes in the worst case once the 30-second timeouts are
counted. When every attempt fails, the script exits `1` with the existing
message, unchanged:

```text
ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness
```

**2. Push-permission gate.** After the repository identity is read from
`origin`, and before any `gh pr ready` or `gh pr comment`, the script runs
`gh api repos/<owner>/<repo> --jq .permissions.push` with the same retry
settings:

| Result | Outcome |
|---|---|
| `true` | Continue |
| `false` | Retry; if every attempt returns `false`, exit `1` |
| Non-zero exit, empty output or any other value | One `WARNING`, continue |
| Repository identity fails validation | One `WARNING`, continue; the script's existing identity check still applies |

An explicit `false` is the only fatal result, because it means the active
account cannot push. Anything else is treated as unknown: the check is
advisory, and GitHub still enforces permissions on the mutation itself. The
failure message names the active login from `gh api user --jq .login`, or
`unknown` if that call fails or returns something that is not a valid GitHub
login:

```text
ERROR: active gh account 'ryan-mistt' lacks push permission on rysweet/amplihack-rs
```

The worst case for this gate is about the same as the authentication gate,
plus 30 seconds for the login lookup.

Neither gate runs `gh auth switch`, `login`, `refresh` or `setup-git`, prints
`gh auth status` output, or reads a token. Retries are limited to step 21;
other `gh` calls keep their existing behaviour.

---

## Configuration

| Name | Kind | Default | Effect |
|---|---|---|---|
| `allow_version_bump` | Context variable in `default-workflow` and `workflow-publish` | `"false"` | `true`, `1` or `yes` keeps version churn in the step-15 commit when step 14 asked for a bump. `.github/hooks/` paths are excluded either way. |
| `WORKFLOW_GH_AUTH_ATTEMPTS` | Environment | `5` | Attempts per step-21 gate. Valid range 1 to 10. |
| `WORKFLOW_GH_AUTH_BASE_DELAY` | Environment | `4` | Seconds before the first retry, doubled after each failure. Valid range 0 to 300. |
| `WORKFLOW_GH_AUTH_MAX_DELAY` | Environment | `60` | Upper limit on one wait, in seconds. Valid range 0 to 600. |
| `WORKFLOW_GH_AUTH_GATE_HELPER` | Environment | unset | Path of `workflow_gh_auth_gate.sh`. When unset, the copy next to `workflow_pr_ready.sh` is used. |

A value that is not a one-to-three-digit number, or is out of range, produces
one `WARNING` and the default is used.

Shorten the step-21 backoff on a host where only one `gh` account is used:

```bash
export WORKFLOW_GH_AUTH_ATTEMPTS=2 WORKFLOW_GH_AUTH_BASE_DELAY=2
```

---

## Recovering excluded changes

Re-apply version churn that step 15 excluded, from the task worktree:

```bash
git apply "$(git rev-parse --git-path amplihack/excluded-version-churn.patch)"
```

The patch file belongs to the worktree and is overwritten each time step 15
excludes churn, so apply it before the next publish run.

Stop ignoring a generated hook path by deleting its line from the exclude
file:

```bash
"${EDITOR:-vi}" "$(git rev-parse --git-path info/exclude)"
```

`info/exclude` is shared by every linked worktree of the repository, so an
entry added in one task worktree applies to all of them.

---

## Known limitations

- Tracked modifications under `.github/hooks/` are unstaged but stay in the
  working tree. A later `git add -A` in step 18c or 20b can still commit them;
  the step-15 `WARNING` names each one.
- Version churn detection tracks table headers and brace depth, not a full
  TOML or JSON parse. A `[package]` table written as an inline table
  (`package = { version = "…" }`) or a `package.json` whose top-level keys
  share a line with nested objects is not recognised, so its version change
  stays staged as an ordinary change. That errs toward committing, never
  toward dropping a real change.
- In `Cargo.lock`, a path dependency outside the workspace also has no
  `source` line, so a version-only change to it counts as churn.
- With the default `allow_version_bump`, a `MINOR` or `MAJOR` bump made by
  step 14 is excluded from the commit. The commit prefix still follows the
  classification.
- An installed copy of amplihack older than these helpers makes step 15 stop
  with `helper-missing:<name>` instead of committing everything. The error
  names the paths that were searched.
- When `publish-sync-pr-head` finds divergence, the run still fails at the
  probe. Resolving the divergence is a manual step.

---

## Verification

```bash
# Regression suite for issue #1575
# (module issue_1575_publish_defects in tests/integration/main.rs)
cargo test -p amplihack --test integration issue_1575

# Existing suites that cover the changed steps
cargo test -p amplihack --test integration issue_978
cargo test -p amplihack --test integration workflow_finalize_resilience
cargo test -p amplihack --test integration issue_815_804
cargo test -p amplihack --test integration default_workflow_decomposition

# Recipe shell tests
bash amplifier-bundle/recipes/tests/test-issue-944-bugfix-no-version-bump.sh
bash amplifier-bundle/recipes/tests/test-issue-962-runtime-artifact-ladders.sh

# Line budgets: the recipe must be 399 or fewer, each helper 400 or fewer
wc -l amplifier-bundle/recipes/workflow-publish.yaml \
      amplifier-bundle/tools/workflow_sync_pr_head.sh \
      amplifier-bundle/tools/workflow_stage_task_changes.sh \
      amplifier-bundle/tools/workflow_commit_message.sh \
      amplifier-bundle/tools/workflow_gh_auth_gate.sh \
      amplifier-bundle/tools/workflow_pr_ready.sh
```

The tests use temporary repositories with local bare remotes, a stub `gh` on
`PATH`, a temporary `HOME` and `GIT_CONFIG_GLOBAL`, and retry delays of `0`.
They never read real `gh` logins or git configuration.

---

## Related docs

- [Workflow Publish Helpers Reference](../reference/workflow-publish-helpers.md): arguments, exit codes and output of the four helpers
- [Ancestry-Aware Step 15 Publish](workflow-publish-ancestry-aware-step15.md): the #978 ancestry policy the sync helper applies
- [Publish Workflow Versioning](../WORKFLOW_PUBLISH_VERSIONING.md): step 14's classification and the commit prefix
- [GitHub API Rate-Limit Tolerance](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md): the shared `gh` retry library, which treats auth errors as permanent
- [Workflow Terminal-State Reference](../reference/workflow-terminal-state.md): the probe that compares `headRefOid` with `HEAD`
- [How to Recover an Existing PR with `default-workflow`](../howto/recover-existing-pr-with-default-workflow.md): runs that continue an existing PR branch
