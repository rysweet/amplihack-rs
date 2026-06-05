---
title: Existing Branch Finalization Runbook
description: Resume, validate, publish, and merge an already-implemented improvement branch without rewriting completed work.
last_updated: 2026-06-05
review_schedule: as-needed
doc_type: howto
---

# Existing Branch Finalization Runbook

> [Home](../index.md) > [Features](README.md) > Existing Branch Finalization

Use this runbook when a scoped improvement is already implemented on one or
more branches and the remaining work is to preserve it, validate it, publish one
pull request, and merge only after repository policy allows it.

The post-v0.9.77 issue-658 branch is the concrete example in this document. Do
not treat the issue number as a reusable feature name; it is an input to this
runbook.

## Contents

- [Quick start](#quick-start)
- [Inputs](#inputs)
- [Configuration](#configuration)
- [Preserve existing work](#preserve-existing-work)
- [Validate the changed surfaces](#validate-the-changed-surfaces)
- [Publish or update the PR](#publish-or-update-the-pr)
- [Merge gate](#merge-gate)
- [Examples](#examples)
- [Troubleshooting](#troubleshooting)

## Quick start

Run from the worktree that contains the completed implementation:

```bash
export NODE_OPTIONS=--max-old-space-size=32768

git --no-pager branch --show-current
git --no-pager status --short --branch
git --no-pager diff --stat
git --no-pager diff --cached --stat
```

Switch to the deliverable branch only after confirming it already contains the
implementation or after preserving the current branch's uncommitted and
committed work. A plain `git switch` changes branches; it does not move commits.

```bash
DELIVERABLE_BRANCH="feat/issue-658-continue-implementing-the-remaining-post-v0977-amp"
git log --oneline "$DELIVERABLE_BRANCH"..HEAD
git switch "$DELIVERABLE_BRANCH"
```

Run focused validations for the changed surfaces, then the broader repository
checks:

```bash
cargo fmt --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
npm test
```

Commit and push only the focused branch:

```bash
git add <scoped-files>
git commit \
  -m "Finalize post-v0.9.77 improvements" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
git push origin "$DELIVERABLE_BRANCH"
```

Create or update the pull request from that branch to the repository default
base branch. Wait for GitHub Actions and branch protection before merging.

## Inputs

| Input | Example |
| --- | --- |
| Deliverable branch | `feat/issue-658-continue-implementing-the-remaining-post-v0977-amp` |
| Base branch | Repository default branch from `gh repo view --json defaultBranchRef` |
| Current worktree | Any worktree that contains the already-completed implementation |
| Validation scope | Existing tests and checks for surfaces touched by the branch |

## Configuration

### `NODE_OPTIONS`

Large Rust-plus-Node validation runs use the supported V8 heap setting:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

Persist this value in `~/.amplihack/config` when nested workflow agents need to
inherit it automatically. If a different `NODE_OPTIONS` value is already
present, preserve it unless validation proves the heap setting is the cause of a
failure.

### Release-version contract

Release builds set `AMPLIHACK_RELEASE_VERSION` at build time so released
binaries report the tag version instead of a stale Cargo workspace version:

```yaml
env:
  AMPLIHACK_RELEASE_VERSION: ${{ needs.version-bump.outputs.version }}
```

The Rust CLI reads this through `option_env!` and falls back to
`CARGO_PKG_VERSION` for local developer builds. See
[Environment Variables Reference](../reference/environment-variables.md#amplihack_release_version).

### npm wrapper controls

The npm wrapper resolves and caches the latest GitHub release before downloading
native binaries. These variables control that behavior:

| Variable | Use |
| --- | --- |
| `AMPLIHACK_NPM_VERSION` | Pin npm bootstrap to an explicit release version. A leading `v` is accepted and stripped. |
| `AMPLIHACK_NPM_NO_LATEST=1` | Skip the latest-release lookup and use the package fallback version. |
| `AMPLIHACK_NPM_LATEST_TTL_MS` | Override the latest-tag cache TTL. `0` disables the cache. |
| `AMPLIHACK_NPM_WRAPPER_CACHE` | Override the npm wrapper cache root, primarily for isolated automation. |
| `AMPLIHACK_NPM_WRAPPER_FORCE_SOURCE=1` | Build native binaries from the local Cargo workspace instead of trying release artifacts first. |

## Preserve existing work

### Inspect before changing branches

Always inspect the branch, staged changes, unstaged changes, and base
relationship before editing:

```bash
git --no-pager branch --show-current
git --no-pager status --short --branch
git --no-pager diff --cached --stat
git --no-pager diff --stat
git --no-pager merge-base HEAD origin/HEAD
```

Do not merge obsolete intermediate branches into the deliverable branch just to
collect work. Preserve the implementation with the smallest safe operation for
its current state.

### If the work is uncommitted

Stash staged, unstaged, and untracked files before switching branches, then
apply the stash on the deliverable branch:

```bash
git stash push --include-untracked -m "finalization-transfer"
git switch "$DELIVERABLE_BRANCH"
git stash pop
```

Resolve conflicts only in files that are part of the scoped implementation.

### If the work is already committed on an intermediate branch

List commits that are not on the deliverable branch, switch to the deliverable
branch, then cherry-pick the required commits:

```bash
git log --oneline "$DELIVERABLE_BRANCH"..HEAD
git switch "$DELIVERABLE_BRANCH"
git cherry-pick <commit-sha>
```

Use multiple `git cherry-pick` arguments when the implementation spans several
ordered commits. Do not cherry-pick unrelated planning, debugging, or status
commits.

### If the deliverable branch already contains the work

Switch directly and continue validation:

```bash
git switch "$DELIVERABLE_BRANCH"
git --no-pager status --short --branch
```

## Validate the changed surfaces

Focused validation runs before broader validation so failures point at the
changed surface rather than the entire workspace.

| Surface | Validation target |
| --- | --- |
| CLI doctor/version reporting | Rust tests covering `amplihack doctor`, `amplihack --version`, and the release-version contract. |
| Copilot Node remediation | Rust tests covering Node.js version detection, managed Node install decisions, and non-interactive failure behavior. |
| Workflow git recovery | Integration tests covering existing branch reuse, PR-number resolution, worktree reattachment, fetch retries, and base-ref fallback. |
| Workflow publish / PR metadata | Recipe tests covering PR creation retries, existing PR reuse, optional design-spec metadata, and hollow-success detection. |
| npm bootstrap | `npm test`, including release target mapping, checksum parsing, URL allowlisting, latest-tag cache behavior, and version override handling. |

Then run the repository's existing CI-style commands:

```bash
cargo fmt --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
npm test
```

Do not add new tools to satisfy this runbook. If a command fails because a
required local dependency is missing, install only the repository's documented
dependency and rerun the same command.

## Publish or update the PR

Use `gh` to create the focused pull request:

```bash
BASE_BRANCH="$(gh repo view --json defaultBranchRef -q .defaultBranchRef.name)"

gh pr create \
  --base "$BASE_BRANCH" \
  --head "$DELIVERABLE_BRANCH" \
  --title "Finalize post-v0.9.77 improvements" \
  --body "Finalizes the scoped post-v0.9.77 improvements with focused and CI-equivalent validation."
```

If a PR already exists for the branch, update that PR instead of opening a
second one:

```bash
PR_NUMBER="$(gh pr list \
  --head "$DELIVERABLE_BRANCH" \
  --json number \
  -q '.[0].number')"

gh pr edit "$PR_NUMBER" \
  --title "Finalize post-v0.9.77 improvements" \
  --body "Finalizes the scoped post-v0.9.77 improvements with focused and CI-equivalent validation."
```

The PR body should document the scoped improvements and validation commands. Do
not include session logs, temporary notes, or obsolete intermediate-branch
history.

## Merge gate

Before merging, verify all required GitHub state from the focused PR:

```bash
gh pr view "$PR_NUMBER" \
  --json state,mergeable,reviewDecision,isDraft,statusCheckRollup,headRefName,baseRefName
```

The PR is merge-ready only when all of these are true:

| Gate | Required state |
| --- | --- |
| PR state | `OPEN` |
| Draft state | Not draft |
| Mergeability | Mergeable and not conflicted |
| Checks | Required checks completed successfully |
| Skips | No required check is skipped or missing |
| Reviews | Required review policy satisfied |
| Scope | `headRefName` is the deliverable branch |

To distinguish expected and unexpected skipped checks, compare the PR's
`statusCheckRollup` with branch protection:

```bash
BASE_BRANCH="$(gh pr view "$PR_NUMBER" --json baseRefName -q .baseRefName)"
gh api "repos/{owner}/{repo}/branches/$BASE_BRANCH/protection/required_status_checks"
```

Any required check context from branch protection that is missing, pending,
failing, cancelled, timed out, or skipped is blocking. Optional checks may be
skipped only when their workflow `if:` or path filter intentionally excludes the
pull request.

Merge with the repository-required method only after the gates pass. Replace
`--squash` with `--merge` or `--rebase` when repository policy requires a
different method:

```bash
gh pr merge "$PR_NUMBER" --squash --delete-branch
```

If branch deletion would delete the wrong branch, omit `--delete-branch` and
clean up manually after verifying the branch name.

## Examples

### Finalize the issue-658 branch

```bash
export NODE_OPTIONS=--max-old-space-size=32768
DELIVERABLE_BRANCH="feat/issue-658-continue-implementing-the-remaining-post-v0977-amp"

git --no-pager status --short --branch
git switch "$DELIVERABLE_BRANCH"

cargo fmt --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
npm test

git add <scoped-files>
git commit \
  -m "Finalize post-v0.9.77 improvements" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
git push origin "$DELIVERABLE_BRANCH"
```

### Pin npm bootstrap to a release

```bash
AMPLIHACK_NPM_VERSION=v0.9.78 npx @rysweet/amplihack-rs --version
```

### Build a release binary with an explicit version

```bash
AMPLIHACK_RELEASE_VERSION=0.9.78 cargo build --release --locked --bin amplihack
./target/release/amplihack --version
```

### Launch Copilot with managed Node remediation

```bash
amplihack copilot --print "summarize the repository"
```

If system Node.js is missing or below v24 on a supported interactive host,
`amplihack` installs the managed runtime before launching Copilot.

## Troubleshooting

### The active branch is an intermediate branch

Do not open the PR from the intermediate branch. Preserve the already-completed
implementation with stash/apply for uncommitted work or cherry-pick for
committed work, then continue on the deliverable branch.

### `cargo clippy --workspace --locked -- -D warnings` fails

Fix only the warning or error reported by clippy. Do not use the failure as a
reason to rewrite unrelated implementation surfaces.

### `npm test` uses a stale latest release

Clear or bypass the latest-tag cache:

```bash
AMPLIHACK_NPM_LATEST_TTL_MS=0 npm test
```

For deterministic bootstrap behavior, pin the version:

```bash
AMPLIHACK_NPM_VERSION=v0.9.78 npm test
```

### GitHub checks are pending

Wait. Local validation is not a substitute for required GitHub Actions. A PR
with pending required checks is not merge-ready.

### GitHub reports required checks skipped

Treat skipped required jobs as blocked unless branch protection no longer
requires them. Fix the workflow trigger or branch state, rerun the checks, and
merge only after the required jobs complete successfully.

## See also

- [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md)
- [Node.js Version Checking](../reference/node-version-checking.md)
- [Post-Update Install: Re-exec New Binary](update-reexec-new-binary.md)
- [Self-Heal: Auto-Restage Framework Assets](self-heal-asset-restage.md)
- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md)
- [Environment Variables Reference](../reference/environment-variables.md)
