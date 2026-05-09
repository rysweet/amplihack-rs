# Workflow-Owned PR Recovery Readiness

**Existing pull requests are recovered by `default-workflow` on their original
branch. Readiness is proven by workflow evidence, not by local assertion or a
manual merge.**

> [Home](../index.md) > [Features](README.md) > Workflow-Owned PR Recovery Readiness

## Quick Navigation

- [How to recover an existing PR with `default-workflow`](../howto/recover-existing-pr-with-default-workflow.md)
- [Tutorial: recover PR 579 readiness](../tutorials/pr-recovery-readiness.md)
- [PR recovery readiness reference](../reference/pr-recovery-readiness.md)

## What This Feature Does

Workflow-owned PR recovery resumes an interrupted pull request and produces a
durable readiness decision from the workflow. It is used when a PR already
exists, the branch is still usable, and recovery must not bypass the normal
workflow gates.

The feature has four guarantees:

| Guarantee | Behavior |
| --- | --- |
| PR identity is explicit | `default-workflow` receives `pr_number` and verifies the pull request belongs to the repository being recovered. |
| Branch reuse is explicit | `existing_branch` is the only normal recovery branch. A replacement branch requires workflow evidence that the original branch is unusable. |
| Readiness scope is bounded | Recovery validates only the requested readiness surfaces. A recovery task names the issues and acceptance criteria under review. |
| Finalization is workflow-owned | `workflow-finalize` emits `ready`, `blocked`, or `finalized`. Operators do not manually merge around that decision. |

Workflow-ready is not the same as merge-ready. A recovered PR can be
workflow-ready when all in-scope recovery surfaces are proven, even while a
long-running required check is still in progress and the GitHub merge state is
blocked by branch protection. The workflow reports that state instead of
bypassing it.

PR 579 is the concrete recovery example used by the companion tutorial. Its
recovery branch is:

```text
fix/issues-577-578-copilot-hooks-and-additive-copy
```

For exact-head no-op recovery, the head gate fails closed unless all three
values match:

```text
local HEAD == PR headRefOid == expected_head_sha
```

and the recovered pull request remains `rysweet/amplihack-rs#579`.

## Quick Start

Run `default-workflow` from the recovery worktree with the target PR and branch
named explicitly:

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack recipe run default-workflow \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "pr_number=<PR_NUMBER>" \
  -c "existing_branch=<EXISTING_PR_BRANCH>" \
  -c "expected_head_sha=<EXPECTED_HEAD_SHA>" \
  -c "task_description=<bounded recovery task; do not manually merge>" \
  -c "issue_requirements=<issue acceptance criteria for the recovery scope>"
```

`NODE_OPTIONS=--max-old-space-size=32768` is the supported heap setting for
large nested agent runs. Persist it in `~/.amplihack/config` when the local
environment needs nested workflow agents to inherit it automatically.

## Recovery Contract

The workflow records a recovery identity block before it mutates or publishes:

```json
{
  "pr_recovery": {
    "repository": "rysweet/amplihack-rs",
    "pr_number": 579,
    "existing_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "branch_reused": true,
    "replacement_branch_created": false,
    "manual_merge_performed": false
  }
}
```

If the PR cannot be loaded, belongs to a different repository, or has a
different head branch, recovery stops as `blocked` before publishing.

## Copilot Hook Readiness

Copilot hook readiness means `amplihack install` stages a local Copilot CLI
plugin that invokes the native `amplihack-hooks` binary for Copilot sessions,
while preserving unrelated Copilot configuration.

When `~/.copilot/` exists, install refreshes:

```text
~/.copilot/installed-plugins/amplihack@local/
|-- plugin.json
|-- hooks.json
`-- commands/
```

`plugin.json` identifies the local plugin and declares the hooks file:

```json
{
  "name": "amplihack",
  "description": "amplihack framework - structured agentic development workflows, hooks, and commands",
  "version": "<current crate::VERSION>",
  "author": { "name": "amplihack" },
  "license": "MIT",
  "hooks": "./hooks.json",
  "commands": "./commands"
}
```

The `commands` field is present only when command markdown files were staged.
`hooks.json` maps Copilot CLI events to native hook subcommands:

| Copilot event | Native command |
| --- | --- |
| `sessionStart` | `amplihack-hooks session-start` |
| `sessionEnd` | `amplihack-hooks stop` |
| `userPromptSubmitted` | `amplihack-hooks workflow-classification-reminder`, then `amplihack-hooks user-prompt-submit` |
| `preToolUse` | `amplihack-hooks pre-tool-use` |
| `postToolUse` | `amplihack-hooks post-tool-use` |

`~/.copilot/config.json` keeps existing fields such as `trustedFolders` and
`lastLoggedInUser`. The install path writes the current `crate::VERSION` to the
`installedPlugins` entry whose `name` is `amplihack`, so rerunning install is
idempotent and does not duplicate the plugin.

This path uses the current Copilot CLI `installedPlugins` schema. Older
`copilot_setup` helpers still write a separate `plugins` array for the legacy
setup flow, so readers should not treat the two config shapes as the same API.

### Missing or Malformed Copilot Configuration

If `~/.copilot/` is absent, Copilot CLI is not installed on that host. The
plugin step is a visible no-op and returns success for the overall install:

```text
Copilot CLI not detected (~/.copilot missing) - skipping
```

If `~/.copilot/config.json` exists but cannot be parsed, current install
behavior logs a warning, refreshes the plugin files, and skips the
`installedPlugins` update rather than failing the whole install. If the config
root is not an object or `installedPlugins` is not an array, the registration
function returns an error, but `amplihack install` still surfaces that error as a
non-fatal warning after Claude hook wiring succeeds.

PR recovery must not treat those warnings as proof of readiness. If the recovery
scope requires Copilot CLI registration, the workflow records the warning or
missing `installedPlugins` entry as readiness evidence and reports hook
readiness as blocked until registration can be proven.

The install path only tolerates leading `//` line comments in Copilot config.
It rewrites the file as strict JSON and does not preserve those comments. Block
comments and trailing comments are handled by the older shared JSONC utilities,
not by this install registration path.

## Claude Native Hook Readiness

Claude Code remains wired through `~/.claude/settings.json`. Install registers
the same native `amplihack-hooks` binary using canonical event order:

| Claude event | Native command | Timeout |
| --- | --- | --- |
| `SessionStart` | `amplihack-hooks session-start` | 10 seconds |
| `Stop` | `amplihack-hooks stop` | 120 seconds |
| `PreToolUse` | `amplihack-hooks pre-tool-use` | host default |
| `PostToolUse` | `amplihack-hooks post-tool-use` | host default |
| `UserPromptSubmit` | `amplihack-hooks workflow-classification-reminder` | 5 seconds |
| `UserPromptSubmit` | `amplihack-hooks user-prompt-submit` | 10 seconds |
| `PreCompact` | `amplihack-hooks pre-compact` | 30 seconds |

The workflow readiness check verifies that user-owned settings entries are
preserved, amplihack-owned native hook entries are updated in place, and hook
contract drift is reported instead of treated as readiness.

## Additive-Copy Readiness

Additive-copy readiness means framework assets are installed from the resolved
source layout without leaving stale amplihack-owned files behind and without
destroying the only valid source tree.

The installer resolves one source layout:

| Source layout | Source root | Destination root |
| --- | --- | --- |
| Bundle | `<repo>/amplifier-bundle/` | `~/.amplihack/.claude/` |
| Legacy | `<repo>/.claude/` or `<repo>/../.claude/` | `~/.amplihack/.claude/` |

Install writes `~/.amplihack/.claude/.layout` atomically with `bundle` or
`legacy` so later verification checks the right destination mapping. Missing
markers default to the legacy layout for older installs; malformed markers are
warned about and treated as legacy.

Mapped directories are replaced by a staged swap:

1. Copy the source directory into a sibling `.staging` directory.
2. Rename the existing destination to a sibling `.old` backup.
3. Rename `.staging` into the final destination.
4. Restore `.old` if the swap fails.
5. Remove `.old` only after the replacement succeeds.

This removes stale files that no longer exist in the source bundle. It also
preserves rollback when a copy or rename fails.

When the source and destination canonicalize to the same path, install treats
the operation as a same-path no-op. It does not remove the destination first,
because that would delete the source as well.

## Install Verification

Install readiness is verified from source-derived evidence:

| Surface | Evidence |
| --- | --- |
| Required source directories | Bundle and legacy source roots are real directories, not symlinks. |
| Mapped destination directories | Every mapped destination exists after copy. |
| Nested framework directories | Every real source subdirectory exists at the matching staged destination. |
| Skills | The staged skill count is at least the source skill count. |
| Staged bundle | Bundle installs also stage `~/.amplihack/amplifier-bundle/` for recipe execution. |
| Manifest | `install/amplihack-manifest.json` records files, directories, binaries, and hook registrations. |

Missing source assets, symlinked source roots, incomplete destination trees,
manifest path traversal, hook contract drift, or unproven required Copilot
registration are readiness blockers when they affect the PR recovery scope.

User-owned files are handled separately from mapped framework directories.
`~/.claude/settings.json` and `~/.copilot/config.json` preserve unrelated user
entries while amplihack-owned hook/plugin entries are updated. `context/PROJECT.md`
is initialized by install from a template. Mapped framework directories under
`~/.amplihack/.claude/` are amplihack-owned and can be replaced to remove stale
files that no longer exist in the source bundle.

## Publish and Finalization

`workflow-publish` pushes workflow-owned readiness commits back to the existing
PR branch. It does not create a replacement PR while the original branch is
usable.

```json
{
  "workflow_publish": {
    "target_pr": 579,
    "target_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "changes_required": true,
    "pushed": true,
    "replacement_branch_created": false
  }
}
```

`workflow-finalize` emits the final recovery decision:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "final_status": "ready",
    "hook_readiness": "ready",
    "additive_copy_readiness": "ready",
    "manual_merge_performed": false,
    "evidence": [
      "PR #579 branch reused",
      "Copilot plugin hook registration verified",
      "mapped directory replacement verified",
      "publish complete or not required"
    ]
  }
}
```

For a recovery that only needs to prove readiness at an already-validated head,
the workflow can emit an accepted no-op decision. The no-op is valid only when
the exact PR head was checked and the requested surfaces have no remaining
workflow-owned changes:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "head_sha": "4041d4b650a245501d8e381b1dfed95a94b65fca",
    "final_status": "ready",
    "changes_required": false,
    "files_modified": [],
    "hook_readiness": "ready",
    "additive_copy_readiness": "ready",
    "check_state": {
      "lint_format": "green",
      "builds": "green",
      "test": "in_progress",
      "merge_state": "blocked"
    },
    "no_op_justification": "No workflow-owned hook or additive-copy readiness changes are required at head 4041d4b650a245501d8e381b1dfed95a94b65fca. Lint/Format and build checks are green; Test is still naturally in progress and branch protection keeps the merge state blocked, so the PR is workflow-ready but not merge-ready.",
    "manual_merge_performed": false,
    "merge_bypass_performed": false,
    "nested_default_workflow_launched": false
  }
}
```

The accepted no-op does not mark the PR as merged, does not override branch
protection, and does not treat a pending Test check as a recovery defect.

Valid final statuses are:

| Status | Meaning |
| --- | --- |
| `ready` | Recovery completed and PR 579 is ready for normal workflow-managed completion. |
| `blocked` | Recovery stopped on a concrete remaining blocker. |
| `finalized` | The workflow completed its permitted finalization path. |

Manual merge is never part of this feature's recovery contract.

## Related Documentation

- [How to recover an existing PR with `default-workflow`](../howto/recover-existing-pr-with-default-workflow.md)
- [Tutorial: recover PR 579 readiness](../tutorials/pr-recovery-readiness.md)
- [PR recovery readiness reference](../reference/pr-recovery-readiness.md)
- [amplihack install reference](../reference/install-command.md)
