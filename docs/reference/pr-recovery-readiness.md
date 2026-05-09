# PR Recovery Readiness Reference

> [Home](../index.md) > Reference > PR Recovery Readiness

Field-level contract for recovering an existing pull request through
`default-workflow`, proving Copilot hook readiness, proving additive-copy
readiness, publishing only to the existing PR branch, and finalizing without a
manual merge.

## Contents

- [Workflow Context Inputs](#workflow-context-inputs)
- [Environment Inputs](#environment-inputs)
- [Recovery Output Contract](#recovery-output-contract)
- [Copilot Plugin Contract](#copilot-plugin-contract)
- [Native Hook Contract](#native-hook-contract)
- [Additive-Copy Contract](#additive-copy-contract)
- [Install Verification Contract](#install-verification-contract)
- [Validation Evidence](#validation-evidence)
- [Publish Contract](#publish-contract)
- [Finalization Contract](#finalization-contract)
- [Failure Semantics](#failure-semantics)

## Workflow Context Inputs

`default-workflow` accepts these recovery fields:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `task_description` | string | Yes | Human-readable recovery task and bounded scope. |
| `repo_path` | path string | Yes | Repository root or worktree root for the target PR. |
| `pr_number` | integer or numeric string | Yes | Existing pull request number to recover. |
| `existing_branch` | string | Yes | Existing PR head branch that must be reused. |
| `issue_requirements` | string | Yes | Acceptance criteria for the readiness surfaces under recovery. |
| `expected_gh_account` | string | Required for GitHub mutation | Exact GitHub login allowed to publish or finalize. |

Example using PR 579:

```bash
amplihack recipe run default-workflow \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "pr_number=579" \
  -c "existing_branch=fix/issues-577-578-copilot-hooks-and-additive-copy" \
  -c "task_description=Recover PR #579 after interrupted workflow; resolve Copilot hook readiness and additive-copy readiness only; do not manually merge" \
  -c "issue_requirements=#577: Copilot plugin and native hooks are staged, registered, idempotent, and verified. #578: mapped framework directories replace stale amplihack-owned trees safely, preserve rollback, and guard source/destination aliasing."
```

`pr_number` and `existing_branch` are identity fields. A mismatch is a blocker
before publish or finalization.

## Environment Inputs

| Variable | Required | Meaning |
| --- | --- | --- |
| `NODE_OPTIONS` | No | V8 heap setting inherited by Node-based agent tooling. Use `--max-old-space-size=32768` for PR 579-sized recovery runs. |
| `AMPLIHACK_HOME` | No | Overrides the default `~/.amplihack` staging root. |
| `AMPLIHACK_AGENT_BINARY` | No | Propagates the active agent binary to nested workflow agents. |

The active process environment is authoritative for the current run. A saved
preference in `~/.amplihack/config` supplies the same value to nested
workflow-owned agents when configured.

## Recovery Output Contract

The workflow records PR identity under `pr_recovery`:

```json
{
  "pr_recovery": {
    "repository": "rysweet/amplihack-rs",
    "pr_number": 579,
    "existing_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "branch_reused": true,
    "replacement_branch_created": false,
    "manual_merge_performed": false,
    "issue_numbers": [577, 578]
  }
}
```

Rules:

- `branch_reused` is `true` for normal recovery.
- `replacement_branch_created` is `false` unless the workflow emits a concrete
  reason that the original branch is unusable.
- The workflow does not publish to a branch that is not attached to `pr_number`.
- `manual_merge_performed` is always `false` for workflow-owned recovery.

## Copilot Plugin Contract

`crates/amplihack-cli/src/commands/install/copilot_plugin.rs` registers
amplihack as a local Copilot CLI plugin when Copilot CLI is present.

### Install Module Entry Points

```rust
fn register_copilot_plugin(repo_root: &Path, hooks_bin: &Path) -> Result<bool>;

fn register_copilot_plugin_in(
    copilot_home: &Path,
    repo_root: &Path,
    hooks_bin: &Path,
) -> Result<bool>;
```

Return values:

| Return | Meaning |
| --- | --- |
| `Ok(true)` | `~/.copilot/` exists and plugin files were created or refreshed. Current code can also return this after logging a warning and skipping `installedPlugins` update for malformed JSON. |
| `Ok(false)` | `copilot_home` does not exist, so Copilot CLI is not installed on this host. |
| `Err(_)` | Plugin staging failed, or config mutation failed for conditions such as a non-object config root, non-array `installedPlugins`, or write failure. `amplihack install` currently surfaces this as a warning and continues after Claude hook wiring succeeds. |

### Plugin Directory

When `~/.copilot/` exists, install writes:

```text
~/.copilot/installed-plugins/amplihack@local/
|-- plugin.json
|-- hooks.json
`-- commands/
```

`commands/` is staged only when command markdown files exist under one of the
supported source locations:

1. `<repo>/docs/claude/commands/amplihack/`
2. `<repo>/.claude/commands/amplihack/`
3. `<repo>/../.claude/commands/amplihack/`

Only `*.md` command files are staged.

### `plugin.json`

Required fields:

```json
{
  "name": "amplihack",
  "description": "amplihack framework - structured agentic development workflows, hooks, and commands",
  "version": "<current crate::VERSION>",
  "author": { "name": "amplihack" },
  "license": "MIT",
  "hooks": "./hooks.json"
}
```

When commands are staged, the manifest also contains:

```json
{
  "commands": "./commands"
}
```

### `hooks.json`

Required shape:

```json
{
  "version": 1,
  "hooks": {
    "sessionStart": [
      { "type": "command", "bash": "\"/home/dev/.local/bin/amplihack-hooks\" session-start", "timeoutSec": 10 }
    ],
    "sessionEnd": [
      { "type": "command", "bash": "\"/home/dev/.local/bin/amplihack-hooks\" stop", "timeoutSec": 120 }
    ],
    "userPromptSubmitted": [
      { "type": "command", "bash": "\"/home/dev/.local/bin/amplihack-hooks\" workflow-classification-reminder", "timeoutSec": 5 },
      { "type": "command", "bash": "\"/home/dev/.local/bin/amplihack-hooks\" user-prompt-submit", "timeoutSec": 10 }
    ],
    "preToolUse": [
      { "type": "command", "bash": "\"/home/dev/.local/bin/amplihack-hooks\" pre-tool-use", "timeoutSec": 30 }
    ],
    "postToolUse": [
      { "type": "command", "bash": "\"/home/dev/.local/bin/amplihack-hooks\" post-tool-use", "timeoutSec": 30 }
    ]
  }
}
```

The binary path is quoted so home directories with spaces remain valid shell
commands.

### `~/.copilot/config.json`

The plugin registration updates `installedPlugins` idempotently:

```json
{
  "installedPlugins": [
    {
      "name": "amplihack",
      "marketplace": "local",
      "version": "<current crate::VERSION>",
      "enabled": true,
      "cache_path": "/home/dev/.copilot/installed-plugins/amplihack@local",
      "source": "local",
      "installed_at": "2026-05-09T10:25:08Z"
    }
  ]
}
```

Rules:

- Existing non-amplihack config fields are preserved.
- Existing `installedPlugins` entries for other plugins are preserved.
- Existing `installedPlugins` entries named `amplihack` are replaced with one
  current entry.
- Leading `//` JSONC comment lines are tolerated on read, then removed when the
  file is rewritten as strict JSON.
- Block comments and trailing comments are not supported by this install path.
- Malformed JSON logs a warning and skips the `installedPlugins` update.
- A non-object root or non-array `installedPlugins` returns an error to the
  registration caller, but `amplihack install` reports that error as a warning
  and continues.
- PR recovery treats any unproven required registration as `blocked`, even when
  the install command itself completed with warnings.

### Copilot Config Schema Notes

There are two Copilot config surfaces in this codebase:

| Path | Config key | JSONC behavior |
| --- | --- | --- |
| `commands/install/copilot_plugin.rs` | `installedPlugins` | Strips leading `//` lines, rewrites strict JSON, and does not preserve comments. |
| `copilot_setup/staging.rs` and `copilot_setup/hooks.rs` | `plugins` and user-level hooks | Uses shared JSONC helpers that preserve leading comment prefixes and tolerate broader JSONC comments. |

Use `installedPlugins` when validating `amplihack install` readiness.

## Native Hook Contract

`~/.claude/settings.json` registers native hook subcommands in this order:

| Event | Matcher | Command | Timeout |
| --- | --- | --- | --- |
| `SessionStart` | none | `amplihack-hooks session-start` | 10 |
| `Stop` | none | `amplihack-hooks stop` | 120 |
| `PreToolUse` | `*` | `amplihack-hooks pre-tool-use` | host default |
| `PostToolUse` | `*` | `amplihack-hooks post-tool-use` | host default |
| `UserPromptSubmit` | none | `amplihack-hooks workflow-classification-reminder` | 5 |
| `UserPromptSubmit` | none | `amplihack-hooks user-prompt-submit` | 10 |
| `PreCompact` | none | `amplihack-hooks pre-compact` | 30 |

Readiness requires:

- user-owned hook entries are preserved
- amplihack-owned entries are replaced in place
- rerunning install does not duplicate amplihack entries
- native hook contract drift is visible in install output and workflow evidence
- the install manifest records the registered event names

## Additive-Copy Contract

`crates/amplihack-cli/src/commands/install/directories.rs` owns framework asset
directory staging.

### Source Resolution

```rust
fn find_source_root(repo_root: &Path) -> Result<(PathBuf, SourceLayout)>;
```

Resolution order:

1. `<repo>/amplifier-bundle/` as `SourceLayout::Bundle`
2. `<repo>/.claude/` as `SourceLayout::LegacyClaude`
3. `<repo>/../.claude/` as `SourceLayout::LegacyClaude`

Symlinked source roots fail closed.

### Mapped Directory Copy

```rust
fn copytree_manifest(
    source_root: &Path,
    layout: SourceLayout,
    claude_dir: &Path,
) -> Result<Vec<String>>;
```

For each mapped directory, install calls the mapped-directory replacement
contract:

```text
source_dir -> target_dir.staging -> target_dir
                         target_dir -> target_dir.old
```

Ready behavior:

| Condition | Behavior |
| --- | --- |
| Destination absent | Copy to `.staging`, then rename into destination. |
| Destination present | Rename destination to `.old`, rename `.staging` to destination, remove `.old` after success. |
| Final rename fails | Restore `.old` to destination and remove `.staging`. |
| Stale `.staging` exists | Remove stale staging directory before the new copy. |
| Source and destination canonicalize to the same path | Skip destructive replacement and delegate to same-path-safe copy behavior. |
| Source is missing, a file, or a symlink | Fail before modifying the destination. |
| Destination mapping contains `..` | Fail before copying. |

This contract intentionally removes stale files from amplihack-owned mapped
destinations. Files that are not part of mapped framework directories, such as
user settings and runtime state, are handled by their own install phases.

### Layout Marker

`local_install` writes `~/.amplihack/.claude/.layout` atomically with the
selected source layout:

| Marker value | Meaning |
| --- | --- |
| `bundle` | Source assets came from `<repo>/amplifier-bundle/`. |
| `legacy` | Source assets came from a legacy `.claude/` tree. |

`missing_framework_paths` uses this marker to avoid checking legacy-only
destinations after a bundle-layout install. A missing marker defaults to legacy
for pre-marker installs. A malformed marker is warned about and also treated as
legacy.

### Bundle Staging

`copy_amplifier_bundle(repo_root, claude_dir)` stages
`<repo>/amplifier-bundle/` to `~/.amplihack/amplifier-bundle/` with the same
staging-and-rename rollback pattern. This keeps recipe execution available for
`smart-orchestrator`, `default-workflow`, and `investigation-workflow`.

## Install Verification Contract

`verify_install_completeness(source_root, layout, claude_dir)` fails when source
and destination evidence disagree.

Checks:

| Check | Failure condition |
| --- | --- |
| Source mapping | Required source directory is missing, not a directory, or a symlink. |
| Destination mapping | Required destination directory is absent. |
| Nested directories | Any real source subdirectory is missing in the staged destination. |
| Skill count | Staged skill directories are fewer than source skill directories. |
| Bundle staging | Bundle layout did not stage `~/.amplihack/amplifier-bundle/`. |
| Manifest paths | Manifest entries are absolute, contain `..`, or contain root/prefix components. |

Successful verification prints source-derived framework manifest evidence and
allows manifest generation to proceed.

## Validation Evidence

`default-workflow` records command evidence under `validation_evidence`.

```json
{
  "validation_evidence": [
    {
      "surface": "copilot-plugin",
      "command": "cargo test -p amplihack-cli writes_plugin_manifest_with_hooks_field --lib",
      "status": "passed",
      "classification": "fixed"
    },
    {
      "surface": "additive-copy",
      "command": "cargo test -p amplihack-cli copy_amplifier_bundle_replaces_existing_atomically --lib",
      "status": "passed",
      "classification": "fixed"
    }
  ]
}
```

Fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `surface` | string | Readiness surface under validation. |
| `command` | string | Exact command or explicit workflow check. |
| `status` | `passed`, `failed`, `skipped`, or `blocked` | Raw outcome. |
| `classification` | `fixed`, `pre_existing`, `environmental`, or `blocked` | Workflow classification for non-trivial outcomes. |
| `notes` | string | Optional safe diagnostic text. |

Classification rules:

| Classification | Required evidence |
| --- | --- |
| `fixed` | Workflow-owned changes remediated an in-scope failure and the check passed after rerun. |
| `pre_existing` | The failure is outside issues 577/578 and reproduces without PR 579 recovery changes. |
| `environmental` | Credentials, rate limits, missing local tools, or unavailable services prevented the check. |
| `blocked` | The failure is in scope and unresolved. |

## Publish Contract

`workflow-publish` owns commits and pushes for recovery changes.

```json
{
  "workflow_publish": {
    "target_pr": 579,
    "target_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "changes_required": true,
    "pushed": true,
    "replacement_branch_created": false,
    "commits": ["abc1234"]
  }
}
```

Rules:

- Publish targets the existing PR branch.
- Dirty readiness docs/evidence are either committed by the workflow or
  explicitly classified outside final readiness.
- If files changed but cannot be pushed, final status is `blocked`.
- Manual commits or pushes outside the workflow are not readiness evidence.

## Finalization Contract

`workflow-finalize` emits the final PR state.

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "final_status": "ready",
    "hook_readiness": "ready",
    "additive_copy_readiness": "ready",
    "manual_merge_performed": false,
    "branch_reused": true,
    "remaining_blockers": [],
    "evidence": [
      "branch reused",
      "Copilot plugin readiness ready",
      "additive-copy readiness ready",
      "publish complete or not required"
    ]
  }
}
```

| `final_status` | Meaning |
| --- | --- |
| `ready` | The PR is ready for normal workflow-managed completion. |
| `blocked` | The workflow stopped on a named blocker. |
| `finalized` | The workflow completed its permitted finalization path. |

The final output includes enough evidence for a reviewer to understand why the
PR is ready, blocked, or finalized.

## Failure Semantics

These conditions fail closed for workflow-owned PR recovery:

- The target PR is missing, closed unexpectedly, or not in the expected repository.
- The target PR head branch does not match `existing_branch`.
- The workflow cannot check out or reuse the existing branch.
- Copilot plugin files are missing or invalid when Copilot CLI is installed.
- Required Copilot plugin registration cannot be proven after install warnings
  or config update errors.
- Native Claude hook contract drift remains after install.
- Mapped directory replacement leaves stale files in amplihack-owned
  destinations.
- Same-path source/destination aliasing would cause destructive replacement.
- A failed staged swap cannot restore the previous destination tree.
- Install verification reports missing source-derived assets.
- The install manifest is incomplete or contains unsafe path entries.
- GitHub mutation is required but authentication or identity verification fails.
- Publish or finalization is attempted outside the workflow-owned path.
- A manual merge is detected.

Failure output names the failed surface and records the target PR as `blocked`.

## See Also

- [Workflow-owned PR recovery readiness](../features/pr-recovery-readiness.md)
- [How to recover an existing PR with `default-workflow`](../howto/recover-existing-pr-with-default-workflow.md)
- [Tutorial: recover PR 579 readiness](../tutorials/pr-recovery-readiness.md)
- [Install command reference](install-command.md)
- [Install manifest reference](install-manifest.md)
