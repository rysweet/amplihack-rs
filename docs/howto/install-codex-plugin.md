---
title: Use Amplihack with Codex
description: Install native Codex assets, preserve configuration, recover interrupted installs and run recipes.
type: howto
updated: 2026-11-10
---

# Use Amplihack with Codex

Amplihack supports interactive Codex sessions and unattended Codex recipe steps alongside Claude and Copilot. The integration uses native Codex plugins, command hooks, and provider-neutral skills. The CLI contract targets codex-cli 0.160.0; no model is pinned.

## Contents

- [Install and launch](#install-and-launch)
- [Run an unattended recipe](#run-an-unattended-recipe)
- [Sandbox and hook trust for agent steps](#sandbox-and-hook-trust-for-agent-steps)
- [Preserve configuration](#preserve-configuration)
- [Skills, commands, and personas](#skills-commands-and-personas)
- [Hook behavior and limitations](#hook-behavior-and-limitations)
- [Runner delivery and removal](#runner-delivery-and-removal)
- [Recover an interrupted installation](#recover-an-interrupted-installation)
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

Native Windows is not supported (no Windows binary is built or released); use
WSL, where Codex plugin registration works as described above.

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

Recipe steps use the production `amplihack codex` route with `codex exec`, which adds a workspace-write sandbox with network access and bypasses hook trust; see [Sandbox and hook trust for agent steps](#sandbox-and-hook-trust-for-agent-steps). The runner sends the complete effective instructions, persona, task, and recipe provenance through stdin and closes the pipe. Large prompts use the same transport. Provenance suppresses recursive orchestration in leaf steps; it does not authorize tool access. Interactive prompts that cannot be delivered completely fail visibly instead of being truncated or silently converted into unattended tasks. Empty prompts are likewise rejected before Codex is spawned, and arguments are passed as an argv vector, never through a shell string. Prompts and native arguments containing NUL bytes are rejected with an error rather than being truncated at the NUL, because operating-system argument passing cannot carry them.

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

## Sandbox and hook trust for agent steps

When a recipe agent step runs on Codex, it is launched as `amplihack codex -- exec …`. On that path amplihack adds three flags directly after the `exec` token (or its alias `e`):

```text
--sandbox workspace-write
-c sandbox_workspace_write.network_access=true
--dangerously-bypass-hook-trust
```

A step that the runner starts as

```bash
amplihack codex -- exec --output-last-message /d0/run/final-message -
```

therefore runs

```bash
codex exec --sandbox workspace-write -c sandbox_workspace_write.network_access=true \
  --dangerously-bypass-hook-trust --output-last-message /d0/run/final-message -
```

For `exec resume`, the flags go between `exec` and `resume`, because `--sandbox` is not an `exec resume` option. On codex-cli 0.166.0, `codex exec resume --sandbox read-only` fails with `unexpected argument '--sandbox' found`:

```bash
codex exec --sandbox workspace-write -c sandbox_workspace_write.network_access=true \
  --dangerously-bypass-hook-trust resume 0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b -
```

The flags were checked against codex-cli 0.166.0. There is no environment variable or configuration key to turn them off; a recipe that needs a different sandbox passes its own, as described below. The hook-trust flag is added to every `amplihack codex -- exec` launch, including one a person types, not only to recipe agent steps; issue #2009 tracks limiting it to recipe runs or adding an opt-out.

When amplihack adds the sandbox, it writes one line to stderr before Codex starts (issue #2010):

```text
amplihack: codex exec: added --sandbox workspace-write, network access, --dangerously-bypass-hook-trust (#1985)
```

The line names only what was added. If the caller already set the network override, `network access` is left out; if the caller already passed `--dangerously-bypass-hook-trust`, that flag is left out. The line goes only to stderr, so a step's stdout and its `--output-last-message` file are unchanged. No line is printed when the caller passed its own sandbox, even though the hook-trust flag is still added, and none is printed for an argv without `exec`. There is no option to turn the line off.

The line is reliably seen only when you run `amplihack codex -- exec` yourself. In a recipe run it usually is not seen. The recipe runner pinned in `claude-plugin/recipe-runner.rev` (recipe-runner-rs `4d5187e`) does not stream an agent step's stderr to the recipe log. It writes the stderr to a file under `/tmp/amplihack-agent-stderr/` and deletes that file when the step succeeds. When the step fails, the error quotes only the last 4000 bytes, which after a long Codex run usually leaves the notice out; the full file is kept at the path printed after `--- stderr-log:`. Finding no line in a recipe log therefore does not mean no flags were added. Issue #2066 tracks keeping these lines from agent steps. The full rules are in [launch flag injection](../reference/launch-flag-injection.md).

### Why the flags are added

- **Sandbox.** The host `$CODEX_HOME/config.toml` may set `sandbox_mode = "danger-full-access"` for interactive lanes and coordinators. Without an explicit `--sandbox`, every agent step would inherit it and run with no sandbox at all. The injected `--sandbox workspace-write` takes precedence over the host setting for the agent step only.
- **Network.** A recipe run of `default-workflow` with every agent step under workspace-write with network access completed the same steps, and pushed the same result, as a run under danger-full-access. No sandbox denial appeared in the run log and no per-job preparation was needed. Agent steps need the network for `git push`, `gh` and package downloads.
- **Hook trust.** Codex runs a user hook only when a trusted hash for it is recorded in `config.toml`, and it skips an untrusted hook without any message. After a `hooks.json` change, or on a new Codex home, the command-check hook (software-factory #149) would be off for every agent step. `--dangerously-bypass-hook-trust` runs the enabled hooks for that one invocation without persisted trust.

### Rules

- **The caller's sandbox choice wins.** If the step's own arguments already choose a sandbox, amplihack adds neither `--sandbox workspace-write` nor the network override. These forms count as a choice, before or after `exec`:
  - `--sandbox <value>`, `--sandbox=<value>` and `-s <value>`, whatever the value;
  - attached `-s<mode>` or `-s=<mode>`, only when `<mode>` is `read-only`, `workspace-write` or `danger-full-access`;
  - `--approve-for-me` (which selects the workspace-write sandbox, and which codex-cli 0.166.0 refuses alongside `--sandbox`), `--dangerously-bypass-approvals-and-sandbox` and its hidden alias `--yolo`;
  - a configuration override whose key is `sandbox_mode`, in any spelling: `-c <value>`, `-c<value>`, `-c=<value>`, `--config <value>` or `--config=<value>`. Spaces around the key are ignored, as Codex ignores them, so `-c 'sandbox_mode = "read-only"'` counts.

  Hook trust is still bypassed.
- **A caller network setting wins.** A `-c sandbox_workspace_write.network_access=<value>` from the caller, in any of the spellings above, suppresses only the injected network override; `--sandbox workspace-write` is still added.
- **Hook trust is added once.** If the caller already passes `--dangerously-bypass-hook-trust`, it is not added again.
- **The approvals bypass is never added.** amplihack never passes `--dangerously-bypass-approvals-and-sandbox` or `--yolo`. If a caller passes one, it is forwarded unchanged and counts as the caller's sandbox choice.
- **Interactive sessions are unchanged.** An argv with no `exec` subcommand is passed to Codex byte for byte. This covers bare `amplihack codex`, `resume`, `resume --last` and a positional prompt. A `-c exec` or `-m exec` value, or an `exec` after a bare `--`, is not treated as the subcommand.
- **Nothing is written to configuration.** The flags exist only on the command line. amplihack writes no `sandbox_mode`, hook trust or bypass setting to any `config.toml`.

To run an agent step with a stricter sandbox, pass it in the step's Codex arguments:

```bash
amplihack codex -- exec --sandbox read-only --output-last-message /d0/run/final-message -
# spawns: codex exec --dangerously-bypass-hook-trust --sandbox read-only --output-last-message /d0/run/final-message -
```

### Preconditions

- **The argv comes from the recipe author.** The caller-wins rule trusts the arguments after `--`. Prompt text, issue bodies and other untrusted content must reach Codex on stdin (`-`), never in argv, where a crafted `--sandbox danger-full-access` would be honoured.
- **Hook sources must be trusted.** `--dangerously-bypass-hook-trust` runs every enabled hook that Codex loads for the invocation without review. Hooks run with user privileges outside the model sandbox. In codex-cli 0.166.0:
  - Codex loads hooks from `CODEX_HOME`, from installed plugins, and from `.codex/` (`hooks.json` or `[hooks]` in `config.toml`) in the working directory and each parent up to the project root.
  - The project `.codex/` layers load only when the project is marked trusted (`trust_level = "trusted"` in the user `config.toml`). The bypass flag does not mark a project trusted, and amplihack writes no trust entry.
  - Workspace-write keeps the top-level `.codex/` of each writable root read-only, so a step that runs at the checkout root cannot write a hook that a later step at the root loads.
  - A `.codex/` in a subdirectory is writable. In a trusted project, one step can therefore write `sub/.codex/hooks.json` and a later step whose working directory is `sub/` runs it.

  Run Codex agent steps only in checkouts you trust, and run them from the checkout root. Issue #1993 tracks the subdirectory case.
- **The injected sandbox overrides a stricter host setting.** Workspace-write with network access is narrower than danger-full-access but wider than a host `config.toml` that sets `read-only` or turns network access off. A recipe that needs the stricter mode passes it explicitly with `--sandbox <mode>`, as shown above, or keeps workspace-write without the network by passing `-c sandbox_workspace_write.network_access=false`. The stderr line reports that the sandbox was added, but amplihack does not read `config.toml` or `-p` profiles, so the line cannot say whether a stricter host setting was overridden.
- **Profiles do not count as a sandbox choice.** Command-line flags take precedence over a `-p <profile>` profile's `sandbox_mode`, so `amplihack codex -- -p ci exec -` still receives `--sandbox workspace-write`. To keep the profile's mode, pass the same `--sandbox` value explicitly.

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

Configuration edits are parsed before mutation, preserve permissions, and use a lock, an unchanged-source check, and destination-adjacent atomic replacement. A missing `config.toml` is treated as empty: installation creates it with only Amplihack-owned entries, and rollback removes it again if it did not exist beforehand. Repeating an install with an unchanged configuration is idempotent and leaves the bytes untouched. If a symlink replaces or redirects a target during install, rollback refuses to follow it outside the selected anchors and leaves the foreign target untouched. Malformed, unreadable, symlinked, or concurrently changed configuration is preserved and produces remediation instead of destructive replacement. The lock coordinates Amplihack writers; unrelated editors must still avoid simultaneous writes. Invocation overrides apply only to that child process.

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

### SessionStart output by host

For Codex, SessionStart emits the native context-injection shape (`hookSpecificOutput` with `hookEventName: "SessionStart"` and `additionalContext`). The output carries initialization context only, with no secrets, authentication data or environment dumps. Claude Code and GitHub Copilot keep their existing SessionStart output byte for byte.

### How the hook protocol is selected

`amplihack-hooks` speaks the Codex protocol only when the environment variable `AMPLIHACK_HOOK_PROTOCOL` is exactly `codex`. Installation writes each Codex hook command as `'<plugin>/bin/hook' <subcommand>`; that wrapper exports that variable for the single command it launches, then `exec`s `amplihack-hooks` with quoted arguments so the exit code propagates. Nothing else selects the protocol: `AMPLIHACK_AGENT_BINARY`, the payload shape, and the presence or absence of `hook_event_name` are ignored. The wrapper sets the variable to the fixed literal `codex`; no host value is forwarded from the caller's environment or interpolated into shells or paths. A missing variable or any other value (`Codex`, `codex `, `1`, empty) selects the default `{"version":1}` behavior used by Claude Code and GitHub Copilot, including Copilot's eventless payloads. Both the protocol layer and the SessionStart hook perform this same exact-match check on the variable.

On the Codex path, payload parse errors and handler errors during PreToolUse fail closed: the hook emits `permissionDecision: "deny"` with the fixed reason "Amplihack security check could not process tool input" (error details go only to telemetry). A PreToolUse payload that does not deserialize to a PreToolUse event is treated as an error and denied, never allowed. If the denial cannot be written, the hook exits 2, which Codex treats as blocking. Advisory events keep the fail-open behavior described above. The 4 MiB stdin limit applies to the Codex path only; the default (Claude/Copilot) path keeps its existing unbounded read and behavior.

> **Wrapper-only variable.** Do not `export AMPLIHACK_HOOK_PROTOCOL=codex` in your shell profile, CI environment, or launcher settings. A global export flips Claude Code and Copilot hooks to the Codex protocol and breaks them. If you run a hook by hand for testing, set it inline: `echo '{"hook_event_name":"PreToolUse","tool_name":"shell_command","tool_input":{"command":"ls"}}' | AMPLIHACK_HOOK_PROTOCOL=codex amplihack-hooks pre-tool-use`. Hand-running the installed `bin/hook` wrapper is equivalent, since it sets the variable itself.

Trust is user-controlled for installation and interactive sessions. `amplihack install` and interactive `amplihack codex` launches never insert `--dangerously-bypass-hook-trust`; review and trust the command definitions through Codex's `/hooks` interface, and do not bypass hook trust to make an installation succeed. Recipe agent steps (`amplihack codex -- exec …`) are the exception: they receive `--dangerously-bypass-hook-trust` so that an untrusted or changed hook definition cannot silently turn off the command-check hook. See [Sandbox and hook trust for agent steps](#sandbox-and-hook-trust-for-agent-steps) for the rules and the trusted-checkout precondition. Hooks execute with user privileges, so the model sandbox does not isolate hook commands. [Official packaging documentation](https://developers.openai.com/plugins/build/plugins) describes the native manifest and portable plugin runtime variables.

## Runner delivery and removal

Managed installation, freshness checks, and the Claude plugin bootstrap consume the immutable revision in `claude-plugin/recipe-runner.rev`, using locked dependency installation. A binary's presence alone does not establish compatibility. The side-effect-free probe is:

```bash
recipe-runner-rs --capabilities
```

The probe returns one schema-1 JSON object. Property order does not matter; a nonblank version and an array of strings are required. Compatible additional fields and capabilities are accepted. Codex requires `codex_exec`; Claude/Copilot managed delivery accepts an empty capability array, including on platforms without Codex execution support. Python and Node are optional for runner validation under the existing Cargo-only install contract.

Set `RECIPE_RUNNER_RS_PATH` to use your own runner. Codex requires a valid executable with compatible capabilities and fails with remediation for missing or incompatible overrides. Claude/Copilot retain historical discovery fallback when an explicit path does not exist, and retain support for legacy custom runners. User-owned binaries are never overwritten.

Managed delivery requires both semantic validation and exact Cargo provenance: package, repository, full immutable revision and binary belong to the same receipt entry, and the selected executable is that managed binary. Missing or mismatched receipts and shadowed executable selection fail visibly. A different managed revision triggers installation of the bundled revision and revalidation; failed upgrades never stamp success. See the [runner validation reference](../reference/recipe-runner-validation.md) for configuration, schema and diagnostics.

`amplihack uninstall` unregisters the owned Codex identity before removing owned package files. It retains user configuration, authentication, unrelated plugins, and shared marketplaces with other consumers. Unknown ownership-ledger versions or identity conflicts prevent destructive cleanup. Partial registration does not publish a new ownership ledger. Before ledger commit, the installer attempts rollback; recovery failures before journal deletion retain the pending journal and available prior state for another install or uninstall attempt. Modified owned package files are preserved and require manual reconciliation.

## Recover an interrupted installation

If launch repeatedly stops with:

```text
failed native Codex reconciliation: foreign Codex config changed; reconcile manually; recovery record retained
```

inspect the retained `$HOME/.amplihack/codex/pending.json` and the current
configuration in the selected `CODEX_HOME`. The journal records an interrupted
installation. Deleting `~/.codex` or `~/.codex-profiles` does not remove it.

First retry normally with the same `HOME` and intended `CODEX_HOME`:

```bash
amplihack codex
```

Startup automatically archives a journal only when both recorded Codex targets
(`config.toml` and `hooks.json`) have disappeared in a way no permitted transaction
state explains, or when verified completion records prove a later reconciliation
superseded it. Newer timestamps alone, foreign edits, partial disappearance and
mixed old/new state keep recovery protection. See the
[automatic recovery rules](../concepts/codex-installation-ownership.md#automatic-stale-journal-recovery)
for the exact boundaries.

If the journal still blocks launch and you have decided to abandon its recovery,
archive it explicitly:

```bash
amplihack codex --reset-recovery
```

Reset runs before startup self-healing, client resolution or native
reconciliation. It accepts a safely accessible regular journal even when its
contents cannot be parsed. It preserves the original bytes and reports the
archive path in one status line, then continues normal Codex launch. For example,
with `HOME=/Users/ryan`, the recovery status line is:

```text
amplihack: preserved Codex recovery journal: /Users/ryan/.amplihack/codex/pending.json.stale-20261110T180000Z
```

The UTC timestamp varies. Existing destinations are preserved; collisions use a
numeric suffix such as `.1`. Automatic archival uses the same preservation and
reporting rules. If there is nothing to archive or resume, reset reports
`amplihack: no pending Codex recovery journal` and continues normally.

Place `--reset-recovery` before the explicit `--` separator. Native Codex
arguments follow the separator:

```bash
amplihack codex --reset-recovery -- exec "Summarize the public interfaces without changing files"
```

This archives any pending journal before starting the unattended task; the task's
final response is the repository summary. Amplihack consumes the reset flag,
including for auto-mode and Docker launches. A flag after the separator is passed
to Codex and does not request recovery reset. See the
[CLI option reference](../reference/cli.md#codex).

Recovery metadata always lives under `$HOME/.amplihack/codex`. `CODEX_HOME`
selects Codex configuration targets and does not relocate that metadata. Keep
the same intended `CODEX_HOME` when retrying or resetting.

Reset moves recovery metadata only. It does not remove configuration,
authentication, packages, ownership records or profiles. Normal reconciliation
afterward still checks package ownership, marketplace registration, hooks and
leftover backups; it can refuse those conflicts independently. Preserve the
archive and any `previous-package` directory when resolving such a refusal.

If archival fails, launch stops with the affected path and corrective action.
Resolve the reported access, path-safety or filesystem error and retry. Do not
delete retained evidence: a failed rename or synchronization can leave a pending
journal or an archive with a recovery intent that the next attempt must finish.

## Migration and troubleshooting

Legacy `config.yaml` with `approval_mode: auto` and legacy JSON are not current Codex configuration. Only positively identified Amplihack-generated files are automatically migrated or removed. Ambiguous files remain for manual review: back them up, translate intended settings into TOML using the official configuration reference, and remove obsolete files yourself after verification. Never replace your TOML wholesale.

If `amplihack codex` stops with `failed native Codex reconciliation`, or `amplihack install` or `amplihack update` stops with `failed native Codex installation`, read the rest of the message for the cause. For a retained recovery record, use [Recover an interrupted installation](#recover-an-interrupted-installation). Probe failures include a timeout, a non-zero exit, output that is not JSON, or a Codex without plugin support. Only the last case asks you to update Codex. See the [native plugin probe reference](../reference/codex-native-plugin-probe.md) for those messages and their fixes.

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
package and backup against those states. Except for the
[proven stale journals](#recover-an-interrupted-installation) archived at startup,
foreign edits, disallowed symlinks in managed resources, modified backups, and
older journals without ownership proof
stop recovery and retain `~/.amplihack/codex/pending.json` and any
`previous-package` directory. Preserve
those files and reconcile the reported conflict manually before retrying. To
abandon journal recovery explicitly, use `--reset-recovery` to preserve it rather
than deleting it. Native-client configuration changes
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
