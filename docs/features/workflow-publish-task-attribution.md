# Publish and Finalize: Pre-Probe Push, Task-Attributed Commits and gh Auth Backoff

**`default-workflow` pushes outstanding branch commits before each terminal-state probe, commits only changes attributable to the task under a subject derived from the diff, and tolerates a transient `gh` authentication drop at step 21.**

> [Home](../index.md) > [Features](README.md) > Publish and Finalize Task Attribution

## Contents

- [Scope](#scope)
- [Pre-probe push](#pre-probe-push)
- [Step-15 staging filter](#step-15-staging-filter)
- [Step-15 commit message](#step-15-commit-message)
- [Step-21 authentication backoff and push-permission check](#step-21-authentication-backoff-and-push-permission-check)
- [Configuration](#configuration)
- [Known limitations](#known-limitations)
- [Verification](#verification)
- [Related docs](#related-docs)

---

## Scope

This page describes three behaviours introduced for issue
[#1575](https://github.com/rysweet/amplihack-rs/issues/1575):

| Area | Recipe step | Helper |
|---|---|---|
| Pre-probe push | `publish-prepush-before-probe`, `finalize-prepush-before-probe` | `amplifier-bundle/tools/workflow_prepush_before_probe.sh` |
| Staging filter and commit message | `step-15-commit-push` | `amplifier-bundle/tools/workflow_commit_filter.sh` |
| Authentication backoff and permission check | `step-21-pr-ready` | `amplifier-bundle/tools/workflow_pr_ready.sh` |

The exact command-line contracts, exit codes and output strings are in the
[Workflow Publish Helpers Reference](../reference/workflow-publish-helpers.md).

---

## Pre-probe push

### Problem addressed

`probe-terminal-state` compares the PR's `headRefOid` with the local `HEAD`
and refuses with `stale PR metadata` when they differ. Runs that continue an
existing PR (`pr_number`, `existing_branch` or `resume_checkpoint`) can make
new commits before reaching the probe, and nothing pushed them first. The
probe was right to refuse; the commits needed to be pushed.

### Behaviour

A bash step runs immediately before each terminal-state probe:

| Step id | Runs before | Recipe |
|---|---|---|
| `publish-prepush-before-probe` | `publish-terminal-state` | `workflow-publish.yaml` |
| `finalize-prepush-before-probe` | `finalize-terminal-state` | `workflow-finalize.yaml` |

Both steps run unconditionally. The helper does nothing when there is nothing
to push, and it never commits; uncommitted work remains the job of step 15.

The helper:

1. Refuses to run on a protected branch (`main`, `master`, the remote's `HEAD`
   branch, or the value of `RECIPE_VAR_base_branch`) before any network call.
2. Chooses the remote from the branch's upstream configuration, falling back
   to `origin` only when `branch.<b>.remote` is unset. If no upstream is
   configured but `refs/remotes/origin/<branch>` exists after a fetch, that
   ref is used. Fresh branches with neither are a no-op. A branch whose
   `branch.<b>.remote` is `.` tracks a local branch (the local-tracking
   fallback from #815 and #804); the helper prints one `INFO` line and does
   nothing, and never falls back to `origin` in that case.
3. Always pushes to `refs/heads/<current branch>` on that remote. The
   upstream's merge ref is never used as the push target, so a branch created
   from `origin/main` with `branch.<b>.merge = refs/heads/main` cannot push
   to `main`.
4. Fetches the remote branch and counts `ahead` and `behind`.
5. Pushes with a plain fast-forward `git push`, then confirms the remote ref
   with `verify_pushed_ref.sh`.

| Situation | Result | Exit |
|---|---|---|
| Detached `HEAD`, no remote, no upstream and no `origin/<branch>`, or `ahead == 0` | One `INFO` line, no push | `0` |
| `branch.<b>.remote` is `.` (local tracking, #815 and #804) | One `INFO` line, no fetch or push; `origin` is not tried | `0` |
| `git fetch` fails | `WARNING`, no push; the probe reports any mismatch itself | `0` |
| `ahead > 0`, `behind == 0` | Push, then verify | `0` |
| `ahead > 0`, `behind > 0` (diverged) | Ancestry evidence on stderr, no push | `1` |
| `ahead == 0`, `behind > 0` (behind only) | Ancestry evidence on stderr, no push | `1` |
| Push rejected, or `verify_pushed_ref.sh` reports a mismatch | Error on stderr | `1` |
| Protected branch, or an invalid branch or remote name | Error on stderr, no fetch or push | `1` |
| `verify_pushed_ref.sh` not found | `WARNING`; the push result stands (as in #1268) | `0` |

Divergence evidence uses the same format as step 15's #978 block:

```text
ERROR: pre-probe push refuses divergent history on 'feat/issue-1575-implement-that-issue-on' (issue #1575).
  ancestry: ahead=2 behind=1 merge_base=3f1c0a9… head=7cd9cd6… upstream=origin/feat/issue-1575-implement-that-issue-on
```

### Effect of a non-zero exit

Both steps fail closed. A branch that has diverged from its remote, or whose
push was rejected, cannot reach a trustworthy terminal state.

- **`workflow-publish.yaml`.** `publish-prepush-before-probe` is the first
  step, ahead of `publish-terminal-state` and therefore ahead of steps 14 and
  15. Exit `1` stops the publish recipe there: no version bump, no staging,
  no commit, no push and no PR step runs. Uncommitted work stays in the
  worktree, and `default-workflow` reports the publish recipe as failed.
  After the divergence is resolved by hand, a resumed run starts publish
  again from this step.
- **`workflow-finalize.yaml`.** Exit `1` from
  `finalize-prepush-before-probe` ends the run before agentic adjudication.

The comment above each step in the recipe states this intent.

### Missing helper

If the helper cannot be found through the ladder described in
[Helper lookup](../reference/workflow-publish-helpers.md#helper-lookup), the
step prints a `WARNING` and exits `0`. The probe that follows remains strict.

---

## Step-15 staging filter

### Problem addressed

`step-15-commit-push` staged everything with `git add -A`. On one branch this
committed a crate version bump (`Cargo.toml` and `Cargo.lock`, 0.5.0 to 0.6.0)
in a repository whose releases are cut by automated release PRs, and it swept
generated `.github/hooks/*` stubs into commits.

### Behaviour

After `git add -A` and before the second artifact-guard pass, step 15 runs
`workflow_commit_filter.sh filter`. The filter:

- **Unstages generated hook stubs.** Untracked paths under `.github/hooks/`
  are unstaged and left on disk. Each path is appended once to
  `$(git rev-parse --git-common-dir)/info/exclude` under the marker
  `# amplihack #1575: generated hook stubs`, so later `git status` and
  `git add -A` calls ignore it. Tracked modifications under `.github/hooks/`
  are unstaged with a `WARNING` and left in the worktree. This happens
  whatever the value of `allow_version_bump`.
- **Removes version-only hunks.** When `allow_version_bump` is false (the
  default), each hunk whose changed lines all match a version rule (see the
  table below) is removed from both the index and the worktree. The removed
  diff is saved first to
  `$(git rev-parse --git-dir)/amplihack-excluded-version-bump.patch`.
  Dependency additions and other edits in the same files stay staged.
- **Prints one warning per exclusion**, for example:

  ```text
  WARNING: step-15 excluded .github/hooks/pre-commit (generated hook stub; added to .git/info/exclude)
  WARNING: step-15 excluded Cargo.toml (version-only hunk; recover with: git apply <git-dir>/amplihack-excluded-version-bump.patch)
  ```

  `<git-dir>` stands for the absolute output of `git rev-parse --git-dir`,
  which the real message prints in full.

Version rules:

| File | A changed line counts as a version line when |
|---|---|
| `Cargo.toml` | it is `version =` inside `[package]` or `[workspace.package]` |
| `package.json` | it is the top-level `"version":` key (two-space indentation) |
| `package-lock.json` | it is the top-level `"version"` or `packages[""].version` |
| `pyproject.toml` | it is `version =` inside `[project]` or `[tool.poetry]` |
| `Cargo.lock` | it is `version = "…"` and the preceding `name = "…"` context line names a package declared in a tracked `Cargo.toml` `[package]` section |

The section rules for `Cargo.toml` and `pyproject.toml` are worked out from
the staged file (`git show :<path>`), using the new-file line numbers in each
hunk header to find the enclosing table. A `version =` line under
`[dependencies.foo]`, `[workspace.dependencies]` or any other table stays
staged. Third-party package versions in `Cargo.lock` are not excluded.

After filtering, `git status --porcelain` is clean if nothing else is pending,
so `probe-terminal-state` does not see leftover files and step 20b's
`git add -A` has nothing to sweep.

If filtering leaves nothing staged, step 15 takes its existing
"Nothing staged" branch.

### Safety of the reverse apply

The filter saves the patch (mode `0600`) before touching anything, then runs
`git apply -R --check` against both the index and the worktree. It applies
the reverse patch only if both checks pass. If either check fails, for example
because of CRLF line endings, the filter exits `2`, step 15 reports
`reason: "commit-filter-failed"`, and the index and worktree are unchanged.

### Missing helper

Step 15 resolves the filter helper before staging. If it cannot be found,
step 15 exits `2` before `git add`, with this result record:

```json
{"pushed":"false","sha":"","branch":"feat/issue-1575-implement-that-issue-on","reason":"commit-filter-missing"}
```

---

## Step-15 commit message

The subject is `<prefix><summary>`. The whole subject, prefix included, is
cut to 72 characters; the prefix is at most six characters, so it is never
cut. The task description is never used as subject text.

- **Prefix.** Taken from the step-14 classification (`fix:`, `feat:` or
  `feat!:`). Only when that classification is missing does step 15 fall back
  to its `TASK_DESC` keyword heuristic, which chooses the prefix and nothing
  else.
- **Summary.** Produced by `workflow_commit_filter.sh summary [issue]` from
  the staged diff:
  - `update <N> file(s) in <top-level paths> (#<issue>)`, listing at most
    three top-level paths separated by commas, followed by `, …` when there
    are more. The issue suffix appears only when an issue number is known.
  - `update workflow changes`, when git is unavailable.

The body is produced by `workflow_commit_filter.sh body`. It lists the staged
files from `git diff --cached --name-status -M`, at most 50 lines followed by
`… and N more`, and then the existing host-aware issue reference paragraph
(`Fixes #N`, `Refs #N`, `AB#N` or `Ref #N`, see #684 and #1361) as the last
paragraph. The former boilerplate (`Implementation as per design
specification`, and so on) is gone.

Example for a run on issue #1575 classified as `PATCH`:

```text
fix: update 4 file(s) in amplifier-bundle, docs, tests (#1575)

Files changed:
- A `amplifier-bundle/tools/workflow_commit_filter.sh`
- M `amplifier-bundle/recipes/workflow-publish.yaml`
- A `docs/features/workflow-publish-task-attribution.md`
- A `tests/integration/issue_1575_publish_defects_test.rs`

Fixes #1575
```

A rename or copy keeps the similarity score from `--name-status` and lists
both paths, each in backticks, old path first:

```text
- R100 `docs/old-name.md` `docs/new-name.md`
```

Paths are sanitised before they reach the message: control characters are
removed, body paths are wrapped in backticks, and a zero-width space is
inserted between `#` and a following digit. A file named `Fixes #1` therefore
cannot add a closing reference.

---

## Step-21 authentication backoff and push-permission check

### Problem addressed

The host shares `~/.config/gh/hosts.yml` across concurrent workstreams, so the
active `gh` account can briefly change or appear unauthenticated. Step 21 ran
`gh auth status` once and failed the whole run at its last step.

### Behaviour

`workflow_pr_ready.sh` now:

1. Exits `127` immediately if `gh` is not installed (unchanged).
2. Retries `gh auth status` with exponential backoff. With the defaults
   (4 attempts, 15-second base delay) it waits 15, 30 and 60 seconds,
   105 seconds in total. Each failed attempt prints one line:

   ```text
   WARNING: gh auth status failed (attempt 2/4); retrying in 30s
   ```

   After the last attempt it exits `1` with the existing message, unchanged:

   ```text
   ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness
   ```

3. Resolves `repo_identity` from `origin` with the existing parser, which
   recognises only `github.com` URLs. If the identity is empty, because
   `origin` is missing or is not a GitHub remote, checks 3 and 4 are skipped
   and the script continues to its existing checks, which handle that case
   as before. Only a GitHub identity that fails
   `^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$` exits `1`, before any API call:

   ```text
   ERROR: origin repo identity 'rysweet/amplihack rs' is not a valid GitHub owner/repo; refusing to check push permission
   ```

4. When a valid identity was found, and before any `gh pr ready` or
   `gh pr comment`, runs `gh api repos/<owner>/<repo> --jq .permissions.push`
   with the same backoff:

   | Result | Outcome |
   |---|---|
   | `true` | Continue |
   | `false` | Exit `1` |
   | API error on every attempt | Exit `1` |
   | Field missing or `null` (GitHub App and Actions tokens) | `WARNING`, continue; GitHub still authorises the mutation itself |

   The failure message names the active login, taken from
   `gh api user --jq .login` and sanitised, or `unknown`:

   ```text
   ERROR: active gh account 'ryan-mistt' lacks push permission on rysweet/amplihack-rs
   ```

The helper never runs `gh auth switch`, `gh auth login`, `gh auth refresh` or
`gh auth setup-git`, and it never prints `gh auth status` output or token
material.

---

## Configuration

| Name | Kind | Default | Effect |
|---|---|---|---|
| `allow_version_bump` | Recipe context (`workflow-publish`, `default-workflow`) | `"false"` | `true`, `1` or `yes` (case-insensitive) keeps version-only hunks in the step-15 commit. `.github/hooks/` paths are excluded regardless. |
| `WORKFLOW_GH_AUTH_ATTEMPTS` | Environment | `4` | Attempts for each step-21 auth and permission check. Values outside 1 to 10 fall back to `4`. |
| `WORKFLOW_GH_AUTH_DELAY` | Environment | `15` | Base delay in seconds, doubled after each failed attempt. Values outside 0 to 300 fall back to `15`. Tests set `0`. |
| `WORKFLOW_PREPUSH_HELPER` | Environment | unset | Overrides the path of `workflow_prepush_before_probe.sh`. |
| `WORKFLOW_COMMIT_FILTER_HELPER` | Environment | unset | Overrides the path of `workflow_commit_filter.sh`. |

Allow a version bump for one run, for example when a repository does not use
automated release PRs:

```bash
amplihack recipe run default-workflow \
  -c task_description="Release 0.7.0" \
  -c repo_path="$PWD" \
  -c allow_version_bump=true
```

Shorten the step-21 backoff on a host where the account does not change:

```bash
export WORKFLOW_GH_AUTH_ATTEMPTS=2 WORKFLOW_GH_AUTH_DELAY=5
```

To recover a version change that step 15 removed:

```bash
git apply "$(git rev-parse --git-dir)/amplihack-excluded-version-bump.patch"
```

---

## Known limitations

- The `package.json` rule recognises only two-space indentation.
- Step 20b still runs `git pull --rebase` and `git add -A`. With the filter
  and `info/exclude` in place it has nothing extra to sweep, but the blind
  rebase is inconsistent with #978 and is tracked as follow-up work.
- `step-14-bump-version` still proposes version bumps for `MINOR` and `MAJOR`
  changes. With `allow_version_bump` false, step 15 removes them from the
  commit; see [Publish Workflow Versioning](../WORKFLOW_PUBLISH_VERSIONING.md).

---

## Verification

```bash
# New regression suite for issue #1575
cargo test -p amplihack --test integration -- issue_1575

# Existing suites affected by the change
cargo test -p amplihack --test integration -- \
  workflow_publish workflow_finalize issue_978 default_workflow issue_684 issue_815

# Shell tests that exercise the changed recipes and helpers
for t in test-issue-944-bugfix-no-version-bump \
         test-issue-962-runtime-artifact-ladders \
         test-pr-always-opens \
         test-issue-1060-existing-pr-is-idempotent \
         test-issue-955-git-identity-fallback \
         test-default-workflow-reliability; do
  bash "amplifier-bundle/recipes/tests/$t.sh" || echo "FAILED: $t"
done

# Repository hooks (formatting, shellcheck, YAML and Markdown checks)
pre-commit run --all-files

# Brick budgets
wc -l amplifier-bundle/recipes/workflow-publish.yaml \
      amplifier-bundle/recipes/workflow-finalize.yaml \
      amplifier-bundle/tools/workflow_pr_ready.sh \
      amplifier-bundle/tools/workflow_prepush_before_probe.sh \
      amplifier-bundle/tools/workflow_commit_filter.sh
```

Expected line counts: both recipes and `workflow_pr_ready.sh` at 400 or
fewer, `workflow_prepush_before_probe.sh` at 150 or fewer and
`workflow_commit_filter.sh` at 250 or fewer.

The tests run against temporary repositories with local bare remotes, a fake
`gh` on `PATH`, and an isolated `HOME`, `XDG_CONFIG_HOME` and
`GIT_CONFIG_GLOBAL`. They never read real `gh` or git configuration.

---

## Related docs

- [Workflow Publish Helpers Reference](../reference/workflow-publish-helpers.md) - command-line contracts for the helpers on this page
- [Ancestry-Aware Step 15 Publish](workflow-publish-ancestry-aware-step15.md) - the #978 ancestry check the pre-probe push reuses
- [Publish Workflow Versioning](../WORKFLOW_PUBLISH_VERSIONING.md) - how step 14 classifies changes and chooses the commit prefix
- [GitHub API Rate-Limit Tolerance](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md) - the shared `gh` retry library, which treats auth errors as permanent
- [Workflow Terminal-State Reference](../reference/workflow-terminal-state.md) - the probe that checks `headRefOid` against `HEAD`
- [How to Recover an Existing PR with `default-workflow`](../howto/recover-existing-pr-with-default-workflow.md) - runs that continue an existing PR branch
