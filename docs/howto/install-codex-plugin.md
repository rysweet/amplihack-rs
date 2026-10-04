# Use Amplihack with Codex

Amplihack supports interactive Codex sessions and unattended Codex recipe steps alongside Claude and Copilot. The integration uses native Codex plugins, command hooks, and provider-neutral skills. The CLI contract targets codex-cli 0.160.0; no model is pinned.

## Install and launch

Install Codex using the [official CLI instructions](https://developers.openai.com/codex/cli), authenticate with Codex, then run:

```bash
codex --version
amplihack install
codex plugin list --json
amplihack codex -- "Explain the structure of this repository"
```

Installation stages the Amplihack package from canonical assets and registers `amplihack@amplihack-local` through Codex's native local marketplace commands. Registration uses the same `CODEX_HOME` as launch. Optional registration is skipped when Codex is absent; an explicitly requested Codex operation reports missing dependencies. Re-run `amplihack install` to reconcile an installation or update owned assets. It preserves unrelated plugins and records ownership for uninstall.

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

The runner captures the final assistant message with `--output-last-message` in a private per-task file. Progress is diagnostic output. Success requires completed input delivery, a successful child exit, and a readable UTF-8 final-message file (an existing empty file is valid). Timeout, cancellation, missing or invalid output, and child failure are reported as failures even when partial output exists. Concurrent steps use separate files. Existing explicitly configured rate-limit retries remain bounded; failed attempts may already have side effects.

For native CLI troubleshooting, the equivalent fresh-task transport is:

```bash
final_dir=$(mktemp -d)
final_path="$final_dir/final.txt"
printf '%s\n' 'Summarize the public interfaces without changing files' | \
  codex exec --sandbox read-only --output-last-message "$final_path" -
```

Choose a private output path for your own task. This direct command illustrates Codex transport; recipes remain behind `amplihack recipe run`. See [official noninteractive mode](https://learn.chatgpt.com/docs/non-interactive-mode).

## Preserve configuration

Codex reads `$CODEX_HOME/config.toml`, defaulting to `~/.codex/config.toml`, and applicable trusted project configuration in `.codex/config.toml`. Authentication stays with Codex. Set `CODEX_HOME` consistently for installation, plugin inspection, launch, and recipes. See [official configuration precedence](https://learn.chatgpt.com/docs/config-file/config-basic).

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
| UserPromptSubmit | Supplies workflow context or an explicit blocking reason |
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

Its stdout contract is one JSON object:

```json
{"schema_version":1,"version":"<package version>","capabilities":["codex_exec"]}
```

Schema 1 and membership of `codex_exec` establish the Codex transport/result capability. Unknown extra fields or capabilities are accepted. The probe does not invoke a model, access the network, or update software. Managed revision provenance comes from installation records, not this capability claim.

An explicit `RECIPE_RUNNER_RS_PATH` is preserved. Compatible custom or newer builds are accepted by capability, regardless of package version or source SHA. Incompatible explicit binaries fail with remediation; they are never overwritten. Managed binaries must both pass the capability probe and match `claude-plugin/recipe-runner.rev` according to trusted installation records. A different or unknown managed revision triggers reinstallation at that immutable revision and a new capability probe, even when the existing binary advertises `codex_exec`. Capability-only acceptance applies to explicit/custom runners, not managed installations. Offline installation failures are visible. A reviewed committed upstream revision can be used before release; the consuming change must identify any pending upstream merge or release dependency rather than claim it resolved.

`amplihack uninstall` unregisters the owned Codex identity before removing owned package files. It retains user configuration, authentication, unrelated plugins, and shared marketplaces with other consumers. Unknown ownership-ledger versions or identity conflicts prevent destructive cleanup. Partial registration failures retain the staged package and ownership record and can be reconciled by reinstalling. Modified owned package files are preserved and require manual reconciliation.

The implementation handoff still requires a corrected immutable runner revision from [companion issue 153](https://github.com/rysweet/amplihack-recipe-runner/issues/153). The current checked-in pin is the legacy baseline and lacks `codex_exec`; the capability gate rejects it. Local verification uses an explicit compatible companion build. Managed delivery and publication must not be declared complete until that tested revision is published and consumed here.

## Migration and troubleshooting

Legacy `config.yaml` with `approval_mode: auto` and legacy JSON are not current Codex configuration. Only positively identified Amplihack-generated files are automatically migrated or removed. Ambiguous files remain for manual review: back them up, translate intended settings into TOML using the official configuration reference, and remove obsolete files yourself after verification. Never replace your TOML wholesale.

If hooks do not run, inspect plugin enablement, `/hooks` trust, and managed hook policy. If a recipe fails compatibility checks, inspect `RECIPE_RUNNER_RS_PATH` and the capability probe before reinstalling the managed runner. If configuration cannot be edited, repair its syntax or explicitly edit the resolved symlink target; Amplihack preserves the original. Unsupported fresh/resume option combinations fail rather than silently discarding requested settings. See the [mode-specific flag matrix](../reference/flag-matrix.md).

Claude and Copilot retain their existing launch, environment, install, and hook contracts. Installing Codex support does not change their permission flags or reinterpret Copilot session teardown as a per-turn Stop event.
