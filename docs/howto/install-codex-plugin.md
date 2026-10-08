---
title: Use Amplihack with Codex
description: Install native Codex assets, preserve configuration and run recipes.
type: howto
updated: 2026-10-08
---

# Use Amplihack with Codex

Amplihack supports interactive Codex sessions and unattended Codex recipe steps alongside Claude and Copilot. The integration uses native Codex plugins, command hooks, and provider-neutral skills. The CLI contract targets codex-cli 0.160.0; no model is pinned.

## Contents

- [Install and launch](#install-and-launch)
- [Run an unattended recipe](#run-an-unattended-recipe)
- [Preserve configuration](#preserve-configuration)
- [Skills, commands, and personas](#skills-commands-and-personas)
- [Hook behavior and limitations](#hook-behavior-and-limitations)
- [Runner delivery and removal](#runner-delivery-and-removal)
- [Migration and troubleshooting](#migration-and-troubleshooting)

## Install and launch

Install Codex using the [official CLI instructions](https://developers.openai.com/codex/cli), authenticate with Codex, then run:

```bash
codex --version
amplihack install
codex plugin list --json
amplihack codex -- "Explain the structure of this repository"
```

Installation stages the Amplihack package from canonical assets and registers `amplihack@amplihack-local` through Codex's native local marketplace commands. Registration uses the same `CODEX_HOME` as launch. Optional registration is skipped when Codex is absent. On `amplihack codex`, the launcher resolves Codex through the existing tool-availability policy before preparing the framework, then reconciles the native package and hooks even when framework staging already exists. Missing-client auto-install respects the existing opt-out and noninteractive/subprocess restrictions; if installation is disallowed or fails, launch stops with remediation. The resolved executable is reused for registration and launch. Re-run `amplihack install` to reconcile an installation or update owned assets. It preserves unrelated plugins and records ownership for uninstall.

Open `/hooks` in Codex, inspect the Amplihack command definitions, and trust the definitions you want to execute. Plugin enablement does not grant hook trust. Updated definitions may require review again. Managed policy can prevent these hooks from loading. See [official hook trust documentation](https://learn.chatgpt.com/docs/hooks).

Interactive launch retains terminal input and passes the initial instructions as a positional prompt. Pass native options after `--`:

```bash
amplihack codex -- --sandbox read-only "Review this repository without changing files"
amplihack codex -- resume --last
amplihack codex -- resume SESSION_ID "Continue the investigation"
```

Replace `SESSION_ID` with a recorded session ID. A caller-selected `--model MODEL` is supported; omit it to use Codex's configured choice. `-p` selects a profile, never a prompt. Amplihack does not generate `--prompt`, Claude permission flags, or Copilot permission flags for Codex. Additional writable directories require an explicit `--add-dir` choice; staging instructions does not grant additional workspace access.

## Run an unattended recipe

Select Codex through the existing agent routing environment:

```bash
AMPLIHACK_AGENT_BINARY=codex amplihack recipe run default-workflow \
  -c task_description="Document the public interfaces" \
  -c repo_path=. --format json
```

Recipe steps use the production `amplihack codex` route with `codex exec`. The runner sends the complete effective instructions, persona, task, and recipe provenance through stdin and closes the pipe. Large prompts use the same transport. Provenance suppresses recursive orchestration in leaf steps; it does not authorize tool access. Interactive prompts that cannot be delivered completely fail visibly instead of being truncated or silently converted into unattended tasks.

The runner captures the final assistant message with `--output-last-message` in a private per-task file. Progress is diagnostic output. Success requires completed input delivery, a successful child exit, and a readable UTF-8 final-message file (an existing empty file is valid). Timeout, cancellation, missing or invalid output, and child failure are reported as failures even when partial output exists. Successful final text is preserved within the runner's `MAX_STEP_OUTPUT_BYTES` limit (10,000,000 bytes); larger output fails instead of being truncated. SIGINT and SIGTERM remain terminal through nonfatal policies, nested recipes, parallel dispatch, JSON repair, and recovery; no subsequent step or recovery starts after cancellation. Owned child processes and input/output resources receive bounded cleanup. Concurrent steps use separate files.

A cleanup failure is terminal even without cancellation. Once a worker
classifies that failure, pending dispatch stops before joins. Already admitted
work may finish and is joined. Cleanup failures prevent rate-limit retries,
JSON repair, recovery and continuation under nonfatal policies, including in
nested recipes.

Ordinary errors retain their existing retry, recovery, and nonfatal
policies. Explicitly configured rate-limit retries remain bounded when cleanup
succeeds; failed attempts may already have side effects.

For native CLI troubleshooting, the equivalent fresh-task transport is:

```bash
final_dir=$(mktemp -d)
final_path="$final_dir/final.txt"
printf '%s\n' 'Summarize the public interfaces without changing files' | \
  codex exec --sandbox read-only --output-last-message "$final_path" -
cat "$final_path"
rm -rf -- "$final_dir"
```

The final file contains the assistant's summary; progress remains diagnostic output. `mktemp -d` creates a private task directory using the platform temporary directory. This direct command illustrates Codex transport; recipes remain behind `amplihack recipe run`. See [official noninteractive mode](https://learn.chatgpt.com/docs/non-interactive-mode).

Recipe verdicts, documentation feedback and deterministic completion records
retain their native JSON objects. Shared consumer transport selects an authoritative context file when supplied,
otherwise relevant namespaced context, then legacy-only scalars. Empty, invalid
or negative canonical input must not revive stale positive or no-op flags.
File-only and whole-finalizer compatibility require verification of the inline
verification producer and finalization collection, validation and completion;
the verdict reader alone does not establish that support. See
[workflow context transport](../reference/workflow-context-transport.md) for the
authority rules, preserved policies and acceptance requirements.

## Preserve configuration

Codex reads `$CODEX_HOME/config.toml`, defaulting to `~/.codex/config.toml`, and applicable trusted project configuration in `.codex/config.toml`. Authentication stays with Codex. Set `CODEX_HOME` consistently for installation, plugin inspection, launch, and recipes. See [official configuration precedence](https://learn.chatgpt.com/docs/config-file/config-basic).

Keep selected directories and aliases stable until installation or recovery
returns. The installer captures caller-selected anchors before native probes,
lock creation or mutation, rechecks their mapping and identity throughout the
operation, and bounds directory synchronization to those anchors.

Choose a regular `HOME`; existing `CODEX_HOME` and its immediate parent must also
be regular directories. A stable alias higher in the path is supported,
provided it already exists before installation and keeps resolving to the same
directory throughout the operation. This applies
to both `HOME` with the default `.codex` directory and an explicit `CODEX_HOME`.
Keep `.amplihack`, staging and marketplace directories, package/backup roots and
configuration files beneath the selected anchors free of symlink replacements.
The same rules apply when retrying rollback or committed cleanup. See
[trusted home boundaries](../concepts/codex-installation-ownership.md#trusted-home-boundaries)
for missing-directory handling and package-resource links.

For example, with a pre-existing `/srv/users/alias` pointing to `/srv/users/real`
and regular `member` and `member/codex` directories beneath it, both of these
layouts use a regular home boundary:

| Layout | Settings |
| --- | --- |
| Default Codex home below an alias | `HOME=/srv/users/alias/member`, with `CODEX_HOME` unset. |
| Explicit Codex home below an alias | Regular `HOME`, with `CODEX_HOME=/srv/users/alias/member/codex`. |

Using the alias itself as `HOME` or as the immediate `CODEX_HOME` parent is
rejected. When explicit-home parents are missing, the nearest existing ancestor
must be regular; the installer never skips a symlink there to find another anchor.
Keep the alias and home directories stable until installation or recovery returns.

An example user configuration is:

```toml
# Keep approvals interactive and limit writes to the workspace.
approval_policy = "on-request"
sandbox_mode = "workspace-write"
```

Amplihack preserves existing approval policies, including granular policies, sandbox settings, comments, profiles, model choices, and unrelated plugin entries. Ordinary launch does not insert approval overrides. Historical bootstrap fills `approval_policy = "never"` only when that key is absent at the selected user configuration target. Set your preferred policy before bootstrap if you want approvals requested. `never` disables approval prompts; it does not disable the sandbox or trust hooks.

Configuration edits are parsed before mutation, preserve permissions, and use a lock, an unchanged-source check, and destination-adjacent atomic replacement. Malformed, unreadable, symlinked, or concurrently changed configuration is preserved and produces remediation instead of destructive replacement. The lock coordinates Amplihack writers; unrelated editors must still avoid simultaneous writes. Invocation overrides apply only to that child process.

## Skills, commands, and personas

The native package includes `plugin.json`, `skills/`, an empty `hooks/hooks.json` declaration, and executable runtime wrappers. Codex 0.160.0 does not execute bundled plugin hooks in observed runtime checks. Installation registers command hooks once in `$CODEX_HOME/hooks.json`, preserving foreign definitions and recording exact owned entries; the empty bundled declaration avoids future duplicates. Skills retain canonical names, nested resources, and shared provider-neutral content. Nested roots beneath parent skills also receive generated top-level runtime copies because Codex 0.160.0 plugin discovery does not descend through a parent skill or follow directory symlink aliases. Reusable commands and personas are exposed as generated skills named `amplihack-command-<slug>` and `amplihack-persona-<slug>`. Ask Codex to use the relevant skill or select it through Codex's skill UI; Claude slash-command syntax is not a Codex contract.

Canonical skill trees retain their directory layout. Generated instruction name collisions fail staging before replacing the installed package. Plugin packaging is the primary deployment path; installation does not duplicate the package into `.agents/skills`. Your own repository and user `.agents/skills` continue to follow [native skill discovery](https://learn.chatgpt.com/docs/build-skills). No orchestration instructions are added to `AGENTS.md`.

## Hook behavior and limitations

| Native event | Amplihack behavior |
| --- | --- |
| SessionStart | Supplies initialization context |
| UserPromptSubmit | Supplies advisory workflow context |
| PreToolUse | Supplies advisory context or a native permission denial |
| PostToolUse | Supplies tool-result context |
| Stop | Native no-op; Claude transcript-driven continuation is unavailable |
| SessionEnd | Performs session cleanup without continuing a turn |

Only command handlers are registered. Prompt and agent hook handlers, unsupported events, and input rewriting are outside this integration. Codex transcripts are not treated as Claude transcripts; transcript-dependent checks cannot be claimed when the required data is unavailable. Advisory hook errors retain existing fail-open behavior with diagnostics; explicit policy denials remain denials. Protocol responses go to stdout and diagnostics to stderr.

Trust is user-controlled. Ordinary install and launch never insert `--dangerously-bypass-hook-trust`. Automation that independently vets hook sources may explicitly pass that native flag for its invocation; it neither persists trust nor changes approval or sandbox policy. Hooks execute with user privileges, so the model sandbox does not isolate hook commands. [Official packaging documentation](https://developers.openai.com/plugins/build/plugins) describes the native manifest and portable plugin runtime variables.

## Runner delivery and removal

Managed installation, freshness checks, and the Claude plugin bootstrap consume the immutable revision in `claude-plugin/recipe-runner.rev`, using locked dependency installation. A binary's presence alone does not establish compatibility. The side-effect-free probe is:

```bash
recipe-runner-rs --capabilities
```

The probe returns one schema-1 JSON object. Property order does not matter; a nonblank version and an array of strings are required. Compatible additional fields and capabilities are accepted. Codex requires `codex_exec`; Claude/Copilot managed delivery accepts an empty capability array, including on platforms without Codex execution support. Python and Node are optional for runner validation under the existing Cargo-only install contract.

Set `RECIPE_RUNNER_RS_PATH` to use your own runner. Codex requires a valid executable with compatible capabilities and fails with remediation for missing or incompatible overrides. Claude/Copilot retain historical discovery fallback when an explicit path does not exist, and retain support for legacy custom runners. User-owned binaries are never overwritten.

Managed delivery requires both semantic validation and exact Cargo provenance: package, repository, full immutable revision and binary belong to the same receipt entry, and the selected executable is that managed binary. Missing or mismatched receipts and shadowed executable selection fail visibly. A different managed revision triggers installation of the bundled revision and revalidation; failed upgrades never stamp success. See the [runner validation reference](../reference/recipe-runner-validation.md) for configuration, schema and diagnostics.

`amplihack uninstall` unregisters the owned Codex identity before removing owned package files. It retains user configuration, authentication, unrelated plugins, and shared marketplaces with other consumers. Unknown ownership-ledger versions or identity conflicts prevent destructive cleanup. Partial registration does not publish a new ownership ledger. Before ledger commit, the installer attempts rollback; recovery failures before journal deletion retain the pending journal and available prior state for another install or uninstall attempt. Modified owned package files are preserved and require manual reconciliation.

## Migration and troubleshooting

Legacy `config.yaml` with `approval_mode: auto` and legacy JSON are not current Codex configuration. Only positively identified Amplihack-generated files are automatically migrated or removed. Ambiguous files remain for manual review: back them up, translate intended settings into TOML using the official configuration reference, and remove obsolete files yourself after verification. Never replace your TOML wholesale.

If hooks do not run, inspect plugin enablement, `/hooks` trust, and managed hook policy. If a recipe fails compatibility checks, inspect `RECIPE_RUNNER_RS_PATH` and the capability probe before reinstalling the managed runner. If configuration cannot be edited, repair its syntax or explicitly edit the resolved symlink target; Amplihack preserves the original. Unsupported fresh/resume option combinations fail rather than silently discarding requested settings. See the [mode-specific flag matrix](../reference/flag-matrix.md).

Claude and Copilot retain their existing launch, environment, install, and hook contracts. Installing Codex support does not change their permission flags or reinterpret Copilot session teardown as a per-turn Stop event.

Codex tool security hooks deny the tool explicitly if input cannot be read,
parsed, or checked, including payloads above 4 MiB. Claude and Copilot retain
their existing input handling. UserPromptSubmit context is advisory;
PreToolUse emits explicit tool denials.

Installation checks hook and marketplace ownership before replacing the package.
Before the new ownership ledger commits, failures trigger an attempt to restore
prior configuration and package state. Native registration and hook reconciliation
must be verified before that commit. After commit, journal removal or previous
package cleanup can still fail; recovery respects the committed ledger rather
than restoring obsolete ownership. Errors report the primary failure and any
recovery or cleanup failure.

Successful rollback restores the original package, native registration and exact
configuration bytes. A plugin disabled before a failed upgrade remains disabled;
native re-registration finishes before the final configuration restoration.
The failed upgrade still returns an error after successful rollback. Resolve its
reported cause before retrying with the same `HOME` and `CODEX_HOME`.

If rollback fails before journal deletion, the installer retains
`~/.amplihack/codex/pending.json` for another attempt on the next install or
uninstall. This includes failures synchronizing restored files, directories or
original absence. Preserve the journal and any remaining `previous-package`
directory; partial restoration may already have moved the backup into place.
Resolve the reported filesystem or ownership conflict before retrying. If the
journal has been deleted and synchronizing its directory fails, the installer
reports an error without claiming the journal is retained. Staging is not
guaranteed to remain.

See [native installer ownership](../concepts/codex-installation-ownership.md) for packaging, transaction and recovery boundaries.

Interrupted installation recovery uses a versioned journal containing exact
original and planned file bytes and package digests. Before rollback or cleanup,
the installer checks every affected config, hook, marketplace, ownership record,
package and backup against those states. Foreign edits, disallowed symlinks in
managed resources, modified backups, and older journals without ownership proof
stop recovery and retain `~/.amplihack/codex/pending.json` and any
`previous-package` directory. Preserve
those files and reconcile the reported conflict manually before retrying; do not
delete the journal to force replacement. Native-client configuration changes
that cannot be proven transaction-owned also require manual reconciliation.

Committed backup cleanup records a durable inventory before deleting entries.
An interrupted cleanup can resume when every remaining entry matches that
inventory. Changed or extra entries stop cleanup and retain the journal. A
partially deleted historical backup without an inventory requires manual
reconciliation; preserve the available backup and journal.

Package identities use `v2:` SHA-256 digests over a versioned tree encoding.
Each sorted entry includes a length-prefixed relative path, an entry type, and
length-prefixed file bytes or symlink target. Legacy unframed digest records
cannot prove original ownership: install, uninstall and recovery retain the
package, backup and journal for manual reconciliation instead of adopting
current contents. Do not remove these records to force cleanup.
