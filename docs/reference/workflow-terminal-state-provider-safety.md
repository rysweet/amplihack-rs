# Workflow Terminal-State Provider Safety

`workflow-terminal-state` is provider-neutral. It uses provider adapters for
change-request metadata and falls back to local Git evidence only when that is
safe for the requested terminal state. GitHub pull-request metadata is used only
by the GitHub adapter. Azure Repos, `visualstudio.com`, `ssh.dev.azure.com`, and
unknown remotes never trigger GitHub CLI calls.

> [Home](../index.md) > [Workflows](../claude/workflow/DEFAULT_WORKFLOW.md) > Workflow Terminal-State Provider Safety

## What This Feature Does

The terminal-state probe decides whether `default-workflow` can safely stop
before creating, updating, waiting on, or merging a pull request. It preserves
the meaningful-diff safety gate across all Git providers while avoiding
GitHub-only assumptions on non-GitHub repositories.

The feature has three guarantees:

| Guarantee | Behavior |
| --- | --- |
| Provider-aware metadata | Provider metadata commands run only through the detected provider adapter. A bare provider-specific number never enables a different provider by itself. |
| Local safety everywhere | Clean worktree, resolvable base ref, branch/base diff, and commits-ahead checks run for every provider. |
| Provider fail-closed semantics | Provider remotes and provider change-request URLs require trustworthy matching metadata when metadata is needed to prove a change-request state or to decide whether meaningful local work is safe to close. |

`gh` is optional for Azure Repos and unknown remotes. Missing GitHub PR
metadata is not a terminal-state failure merely because a probe is running in a
GitHub repository. A clean no-diff GitHub branch can still return
`NO_DIFF_SUCCESS` from local Git evidence. Metadata absence fails closed only
when GitHub PR evidence is required: for example, proving `MERGED`, validating
a supplied GitHub PR URL, or deciding that a meaningful local diff is safe to
treat as terminal.

## Provider Detection

Provider detection starts with the most explicit signal and falls back to
`origin`:

1. If `pr_url` is present, classify that URL.
2. Otherwise classify `git config --get remote.origin.url`.
3. Enable provider metadata only after a positive provider match.
4. Treat every unsupported remote as local/unknown.
5. Use provider-specific IDs only after provider metadata is enabled; a bare
   number is not provider evidence.

| Input example | Provider classification | GitHub PR metadata |
| --- | --- | --- |
| `https://github.com/OWNER/REPO/pull/123` | GitHub explicit PR URL | Enabled |
| `https://github.com/OWNER/REPO.git` | GitHub remote | Enabled |
| `git@github.com:OWNER/REPO.git` | GitHub remote | Enabled |
| `ssh://git@github.com/OWNER/REPO.git` | GitHub remote | Enabled |
| `https://dev.azure.com/ORG/PROJECT/_git/REPO` | Azure Repos | Disabled |
| `https://ORG.visualstudio.com/PROJECT/_git/REPO` | Azure Repos legacy host | Disabled |
| `git@ssh.dev.azure.com:v3/ORG/PROJECT/REPO` | Azure Repos SSH | Disabled |
| `ssh://git@ssh.dev.azure.com/v3/ORG/PROJECT/REPO` | Azure Repos SSH | Disabled |
| `https://gitlab.com/OWNER/REPO.git` | Unknown/non-GitHub | Disabled |
| No `origin` remote | Unknown/non-GitHub | Disabled |

Non-GitHub change-request URLs are recognized as non-GitHub provider signals and
never trigger `gh`.

## Safety Model

### Provider-independent local Git gates

These checks always run:

1. The target path is a Git worktree.
2. `branch_name` is valid and matches the checked-out branch.
3. `base_ref` resolves to a commit, either from input or from the default-base
   search order.
4. The worktree is clean before terminal success can be claimed.
5. The branch/base diff is inspected.
6. Commits ahead of base are counted.

Terminal success is allowed only when local Git evidence proves one of the safe
states:

| State | Local proof |
| --- | --- |
| `NO_DIFF_SUCCESS` | Clean branch, no meaningful diff, and no commits ahead of base. |
| `CLOSED_OBSOLETE` | Clean branch with no meaningful diff remaining, even if historical local commits exist. |

Meaningful local diffs are never converted into terminal success. On
non-GitHub remotes, a meaningful diff keeps the workflow active or blocked
until the diff is published by the applicable workflow path, removed, merged
upstream, or explicitly superseded.

### GitHub metadata gates

When GitHub metadata is enabled, `workflow-terminal-state` uses GitHub CLI
metadata to prove PR-specific states:

| State | Required metadata |
| --- | --- |
| `MERGED` | PR state is merged, or closed with merge evidence such as `mergedAt`. |
| `CLOSED_OBSOLETE` | PR is closed without merge evidence and local Git proves no meaningful diff remains. |
| `BLOCKED_CI` | GitHub status-check metadata contains real failing checks. |

GitHub metadata must match the local repository and branch:

- PR head branch equals `branch_name`.
- PR base branch equals normalized `base_ref`.
- PR head SHA equals local `HEAD`.
- PR head and base repositories match the current GitHub remote.
- Cross-repository PRs are rejected by the terminal-state proof.

If a GitHub metadata command fails, returns ambiguous data, or cannot prove the
requested GitHub PR identity, terminal-state validation fails closed when the
current decision depends on that metadata. Clean no-diff proof can still return
`NO_DIFF_SUCCESS` from local Git evidence without PR metadata.

### Non-GitHub fallback

For Azure Repos and unknown remotes without configured change-request automation,
terminal-state validation intentionally does not inspect provider PR metadata. It
does not call:

```bash
gh pr list
gh pr view
gh pr status
```

The local Git gates remain authoritative:

| Local condition | Terminal-state behavior |
| --- | --- |
| Dirty worktree | Terminal success is denied so pending local work can be handled. |
| Missing or unresolved base ref | Terminal-state validation fails with an actionable base-ref error. |
| Clean no-diff branch | `NO_DIFF_SUCCESS` is allowed without GitHub metadata. |
| Clean branch with no meaningful diff but commits ahead | `CLOSED_OBSOLETE` is allowed as superseded/no-diff evidence. |
| Meaningful branch/base diff | Terminal success is denied; the workflow must publish, remove, merge, supersede the diff, or return `ManualRequired` with a next action. |

## Usage

`workflow-terminal-state` normally runs inside `default-workflow`. Direct
invocation is useful for diagnosing terminal-state decisions:

```bash
amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c branch_name="$(git branch --show-current)" \
  -c base_ref=origin/main
```

For large nested workflow runs, keep the supported Node heap setting in the
environment or in `~/.amplihack/config`:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

## Recipe API

### Inputs

| Input | Required | Description |
| --- | --- | --- |
| `worktree_setup.worktree_path` | No | Preferred workflow checkout. When present, terminal-state checks run here instead of `repo_path`. |
| `repo_path` | Usually | Repository path used when no worktree path is supplied. Defaults to `.`. |
| `branch_name` | No | Expected current branch. Defaults to the checked-out branch. |
| `base_ref` | No | Intended comparison base. Defaults to `origin/HEAD`, `origin/main`, `origin/master`, `origin/develop`, `main`, `master`, or `develop`, whichever resolves first. |
| `pr_number` | No | GitHub PR number. Used only after the PR URL or remote has enabled GitHub metadata; a bare number does not imply GitHub support. |
| `pr_url` | No | PR URL. GitHub URLs enable GitHub metadata; non-GitHub URLs classify the provider and otherwise remain opaque. |
| `goal_already_met` | No | Legacy design signal. It can support a no-diff proof but never overrides dirty work, failing checks, or meaningful diffs. |

### Outputs

| Output | Meaning |
| --- | --- |
| `terminal_success` | `true` only when the workflow can safely stop without publishing or merging more work. |
| `terminal_state` | Stable state such as `MERGED`, `CLOSED_OBSOLETE`, `NO_DIFF_SUCCESS`, `FOLLOWUP_CREATED`, `SUPERSEDED`, `FAILED_MEANINGFUL_DIFF`, `FAILED_FINALIZER_OUTPUT`, or `BLOCKED_CI`. |
| `terminal_reason` | Human-readable evidence for the decision. |
| `publish_status` | Publish-facing status using the same vocabulary as `terminal_state`. |
| `should_publish` | `true` only when the workflow should continue through a publish path for meaningful work. |
| `should_finalize` | `true` when finalization should emit a non-mutating final decision. |
| `should_run_ci_wait` | `true` only when CI waiting remains valid for the active provider path. |
| `should_merge` | `true` only when merge remains valid for green, active work. |
| `change_request_url` | Provider change-request URL when metadata identified one; empty for local-only probes. |
| `change_request_id` | Provider change-request identifier when metadata identified one. |
| `pr_url` | GitHub compatibility URL when GitHub metadata identified one; empty for non-GitHub local-only probes. |
| `pr_number` | GitHub compatibility number when GitHub metadata identified one; empty for non-GitHub local-only probes. |
| `branch_diff_status` | `no-diff`, `has-diff`, or `unknown`. |
| `commits_ahead` | Number of commits ahead of `base_ref`, when countable. |

## Configuration

No provider configuration is required.

| Environment or tool | Required when | Notes |
| --- | --- | --- |
| `git` | Always | Used for all provider-independent safety checks. |
| `jq` | Always | Used by the recipe to emit structured output. |
| `gh` | GitHub remotes or explicit GitHub PR URLs | Required only for GitHub PR metadata. Not required for Azure Repos or unknown remotes. |
| `GH_TOKEN` or GitHub auth | GitHub metadata path | Missing or invalid auth fails closed for GitHub-backed terminal-state proofs. |
| `NODE_OPTIONS=--max-old-space-size=32768` | Large nested workflow runs | Recommended saved preference for this project; unrelated to provider detection. |

If a non-GitHub repository has no resolvable default base, pass `base_ref`
explicitly:

```bash
amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c base_ref=main
```

## Examples

### Azure Repos HTTPS remote, no GitHub CLI installed

```bash
git remote set-url origin https://dev.azure.com/acme/platform/_git/service

amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c branch_name=fix/provider-safe-terminal-state \
  -c base_ref=origin/main
```

Expected behavior:

- No `gh pr list` or `gh pr view` command is run.
- A clean no-diff branch can return `NO_DIFF_SUCCESS`.
- A meaningful diff returns a non-terminal decision and cannot be treated as
  workflow closure.

### Azure Repos SSH remote

```bash
git remote set-url origin git@ssh.dev.azure.com:v3/acme/platform/service

amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c base_ref=origin/main
```

Expected behavior is the same as Azure HTTPS: local Git evidence is sufficient
for no-diff closure and sufficient to block unsafe meaningful-diff closure.

### Unknown remote with a meaningful diff

```bash
git remote set-url origin https://git.example.invalid/acme/service.git
echo "change" >> README.md
git add README.md
git commit -m "Document provider-safe terminal state"

amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c base_ref=origin/main
```

Expected behavior:

- No GitHub metadata command is run.
- `branch_diff_status` reports `has-diff`.
- `terminal_success` is `false`.
- The workflow does not claim terminal closure until the diff is resolved,
  published by an applicable provider path, merged upstream, or superseded.

### GitHub remote with same-branch PR discovery

```bash
git remote set-url origin git@github.com:rysweet/amplihack-rs.git

amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c branch_name=fix/provider-safe-terminal-state \
  -c base_ref=origin/main
```

Expected behavior:

- `gh pr list --head fix/provider-safe-terminal-state --state all ...` is
  allowed.
- PR metadata failures fail closed when a meaningful diff remains.
- A merged matching PR can return `MERGED`.

### Explicit GitHub PR URL

```bash
amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c pr_url=https://github.com/rysweet/amplihack-rs/pull/751 \
  -c base_ref=origin/main
```

Expected behavior:

- GitHub metadata is enabled even if the current remote is missing or ambiguous.
- The PR URL must identify the same repository and branch as the local checkout.
- Stale head SHA, mismatched base, cross-repository identity, or failing checks
  block terminal success.

### Explicit Azure Repos PR URL

```bash
amplihack recipe run workflow-terminal-state \
  -c repo_path=. \
  -c pr_url=https://dev.azure.com/acme/platform/_git/service/pullrequest/42 \
  -c base_ref=origin/main
```

Expected behavior:

- The URL classifies the provider as non-GitHub.
- The URL is not parsed as a GitHub PR.
- `gh` is not required.
- Local Git safety checks decide whether terminal closure is safe.

## Troubleshooting

| Symptom | Meaning | Fix |
| --- | --- | --- |
| `gh is required` on an Azure Repos remote | Provider detection regressed or failed to classify the remote as non-GitHub. | Treat this as a bug: capture the remote URL form and add or fix the provider-detection regression coverage. |
| `base_ref does not resolve` | Local Git cannot compare branch to base. | Fetch the base branch or pass a resolvable `base_ref`. |
| `terminal_success=false` with `branch_diff_status=has-diff` | Meaningful work remains. | Continue the workflow, publish through the applicable provider path, or explicitly supersede/remove the diff. |
| GitHub PR metadata mismatch | GitHub PR identity does not match local branch/base/HEAD. | Fetch, checkout the correct branch, or pass the correct PR URL/number. |

## Regression Expectations

Provider-safety tests cover these scenarios:

1. Azure HTTPS remotes do not invoke `gh pr list`.
2. Azure SSH remotes do not invoke `gh pr list`.
3. Unknown non-GitHub remotes do not invoke `gh pr list`.
4. Non-GitHub meaningful diffs are detected and cannot produce terminal success.
5. GitHub remotes still invoke GitHub PR discovery when no explicit PR target is
   supplied.
6. Explicit GitHub PR URLs preserve fail-closed metadata validation.
7. A bare `pr_number` on Azure Repos or unknown remotes does not invoke `gh`.
8. A clean no-diff GitHub branch can return `NO_DIFF_SUCCESS` even when PR
   metadata is unavailable.
