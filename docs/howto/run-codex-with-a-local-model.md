---
title: Run amplihack codex with a Local Model
description: Start interactive sessions, single tasks and recipe agent steps on a model served from your own machine, with a small prompt.
type: howto
updated: 2026-11-24
---

# Run amplihack codex with a Local Model

Use `amplihack codex --local-model <name>` when Codex talks to a model served
from your own machine. amplihack starts Codex with a Codex home of its own,
adds no hook text to the model's context and keeps the command-check hook, so
the first prompt stays small and a destructive command is still blocked.
Claude, Copilot and ordinary `amplihack codex` sessions are unaffected.

## Contents

- [Before you start](#before-you-start)
- [Start an interactive session](#start-an-interactive-session)
- [Run one task unattended](#run-one-task-unattended)
- [Run recipe agent steps on the local model](#run-recipe-agent-steps-on-the-local-model)
- [Check what the session uses](#check-what-the-session-uses)
- [Change the model or provider settings](#change-the-model-or-provider-settings)
- [Go back to an ordinary launch](#go-back-to-an-ordinary-launch)
- [Remove a generated home](#remove-a-generated-home)
- [Troubleshooting](#troubleshooting)

## Before you start

You need:

- Linux, macOS or WSL. Local-model launches are not available on other
  platforms.
- Codex installed. The settings amplihack writes were checked against
  codex-cli 0.166.0.
- `amplihack-hooks` installed, as `amplihack install` leaves it.
- A server for the model that Codex can reach, for example llama.cpp's
  `llama-server` on `http://127.0.0.1:8080/v1`.
- A Codex profile file for the model in your Codex home, named
  `<name>.config.toml`. The name is 1 to 64 characters of ASCII letters,
  digits, `.`, `_` and `-`.

This profile, saved as `~/.codex/inkling.config.toml`, is used in every
example below:

```toml
model = "inkling-2-mini"
model_provider = "llamacpp-local"
model_context_window = 65536

[model_providers.llamacpp-local]
name = "llama.cpp (local)"
base_url = "http://127.0.0.1:8080/v1"
wire_api = "responses"
stream_idle_timeout_ms = 1800000
stream_max_retries = 2
```

If the provider needs a key, name the variable that holds it with `env_key` in
the provider table. Your `~/.codex/auth.json` is not copied into the local
session.

## Start an interactive session

```sh
amplihack codex --local-model inkling
```

amplihack builds or refreshes `~/.amplihack/codex-local/inkling/`, prints one
line to stderr and starts Codex:

```text
amplihack: codex local model "inkling": added --dangerously-bypass-hook-trust (Codex home "/home/ryan/.amplihack/codex-local/inkling", 2 user PreToolUse handlers carried over)
```

Codex runs as `codex --dangerously-bypass-hook-trust` with
`CODEX_HOME=/home/ryan/.amplihack/codex-local/inkling`. Hook trust is bypassed
so that the command-check hook runs in a new Codex home without a `/hooks`
review; see
[What differs in a local session](../reference/codex-local-model-launch.md#what-differs-in-a-local-session)
before you trust a project in that session.

Codex options go after `--`, as for any `amplihack codex` launch:

```sh
amplihack codex --local-model inkling -- resume --last
```

## Run one task unattended

Send the task on stdin and collect the final message from a file:

```sh
final_dir=$(mktemp -d)
printf '%s\n' 'Add a doc comment to fn parse_name in src/name.rs. Run cargo test name and report the result.' | \
  amplihack codex --local-model inkling -- exec --output-last-message "$final_dir/final.txt" -
cat "$final_dir/final.txt"
rm -rf -- "$final_dir"
```

Codex runs as
`codex --dangerously-bypass-hook-trust exec --sandbox danger-full-access --output-last-message <file> -`.
The `--sandbox` comes from the usual
[agent-step flags](../reference/launch-flag-injection.md#codex-agent-step-flags);
pass your own `--sandbox` after `exec` to choose another.

## Run recipe agent steps on the local model

Agent steps cannot add flags, so select the model through the environment.
Every Codex agent step inherits it:

```sh
AMPLIHACK_AGENT_BINARY=codex AMPLIHACK_CODEX_LOCAL_MODEL=inkling \
  amplihack recipe run default-workflow \
  -c task_description="Add a doc comment to fn parse_name in src/name.rs" \
  -c repo_path=. --format json
```

Each agent step is its own `amplihack codex` launch. When the recipe is run
from a shell, as here, every step refreshes the home from the profile. A
refresh writes no file when nothing the home is built from, such as the
profile or your `hooks.json`, has changed. Only a launch made inside a local
session with the same
name, such as a recipe the model runs from that session, reuses the home
without reading the profile.

`--local-model` on the command line wins over the variable. An empty variable
counts as unset.

## Check what the session uses

List the generated home:

```sh
ls -la ~/.amplihack/codex-local/inkling/ ~/.amplihack/codex-local/inkling/rules/
```

amplihack owns four files, each at mode 0600; anything else in the directory
was written by Codex:

```text
config.toml
hooks.json
instructions.md
rules/amplihack-dangerous-commands.rules
```

Check amplihack's settings in `config.toml`. The script needs Python 3.11 or
later for `tomllib`, and prints the settings in a fixed order whatever their
order in the file:

```sh
python3 -I - ~/.amplihack/codex-local/inkling/config.toml <<'EOF'
import json, sys, tomllib
with open(sys.argv[1], "rb") as f:
    config = tomllib.load(f)
features = config.get("features", {})
for key in ("project_doc_max_bytes", "model_instructions_file", "web_search", "approval_policy"):
    print(key, "=", json.dumps(config.get(key)))
for key in ("hooks", "multi_agent", "goals", "view_image", "plugins", "apps"):
    print(f"features.{key} =", json.dumps(features.get(key)))
EOF
```

```text
project_doc_max_bytes = 0
model_instructions_file = "/home/ryan/.amplihack/codex-local/inkling/instructions.md"
web_search = "disabled"
approval_policy = "never"
features.hooks = true
features.multi_agent = false
features.goals = false
features.view_image = false
features.plugins = false
features.apps = false
```

`approval_policy` is your profile's value when the profile sets one.
`features.hooks` is always `true`, even if the profile sets it to `false`. Check
that `hooks.json` holds only PreToolUse, with amplihack's entry first:

```sh
python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(list(d["hooks"])); print(d["hooks"]["PreToolUse"][0]["hooks"][0]["command"])' \
  ~/.amplihack/codex-local/inkling/hooks.json
```

```text
['PreToolUse']
env AMPLIHACK_HOOK_PROTOCOL=codex '/home/ryan/.local/bin/amplihack-hooks' pre-tool-use
```

Your own PreToolUse handlers from `~/.codex/hooks.json` follow it. The full
contents of each file are in the
[reference](../reference/codex-local-model-launch.md#the-generated-codex-home).

## Change the model or provider settings

Edit the profile file, not the generated `config.toml`, then launch again:

```sh
sed -i 's/^model_context_window = .*/model_context_window = 32768/' ~/.codex/inkling.config.toml
amplihack codex --local-model inkling
```

Each launch from outside a local session refreshes the home from the profile.
A change you make by hand to an amplihack setting in the generated
`config.toml` is undone at the next launch. Projects you trusted, hooks you
trusted and notices you dismissed in the local session are kept.

To use a second model, add another profile file, for example
`~/.codex/qwen.config.toml`, and launch with `--local-model qwen`. Each name
has its own home.

## Go back to an ordinary launch

Drop the flag and clear the variable:

```sh
unset AMPLIHACK_CODEX_LOCAL_MODEL
amplihack codex
```

A local session exports `AMPLIHACK_HOOK_TEXT=none` to everything it starts.
If you start Claude or Copilot from inside one and want amplihack's hook text,
clear that too:

```sh
env -u AMPLIHACK_HOOK_TEXT amplihack claude
```

## Remove a generated home

amplihack never deletes a generated home. To remove one:

```sh
rm -rf -- ~/.amplihack/codex-local/inkling
```

This also deletes the local session history and any login made in it. The
next local-model launch with that name builds a fresh home.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Error naming `/home/ryan/.codex/inkling.config.toml` | The profile file is missing | Create it, or check the name. With `CODEX_HOME` set, the file is looked up there. |
| Error about the name rule, naming the flag or `AMPLIHACK_CODEX_LOCAL_MODEL` | The name has a character outside `[A-Za-z0-9._-]`, is longer than 64 characters, or is `.` or `..` | Rename the profile file and use the new name. |
| Error naming `--docker`, `AMPLIHACK_USE_DOCKER` or `--auto` | These modes are not available with a local model | Drop the option, or unset `AMPLIHACK_USE_DOCKER`. |
| Error naming `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | The hooks binary was not found, or was found by a relative path | Run `amplihack install`, or set the variable to the absolute path of `amplihack-hooks`. |
| Error naming `~/.codex/hooks.json` | Your `hooks.json` does not parse or has the wrong shape | Fix the file at the line given. |
| Error naming the generated `config.toml` | The generated file does not parse | Fix it, or delete it and launch again. |
| Error naming `~/.amplihack` | It is not a directory, is not owned by you, or is writable by others. A symbolic link at `~/.amplihack` is followed and is accepted when its target passes these checks. | Make it a directory you own and remove write permission for others, for example `chmod go-w ~/.amplihack`. amplihack does not change its mode. |
| Error naming a path under `~/.amplihack/codex-local/` | A directory there has permissions looser than 0700, or the path is a symbolic link | `chmod 700` the directory. For a link, remove it and launch again, and amplihack creates a real directory or file in its place. amplihack does not change modes itself. |
| A nested launch fails and asks you to relaunch from outside the session | The reused home no longer passes its checks, for example after `policy.deny_commands` changed | Exit the session and launch again from a shell outside it. |
| Codex reports an unknown setting such as `web_search` | A Codex release renamed a setting amplihack writes | Report it, with your `codex --version`. |
| No amplihack hook text in a Claude session | `AMPLIHACK_HOOK_TEXT=none` was inherited from a local session | Start Claude with `env -u AMPLIHACK_HOOK_TEXT`. |

For every rule behind these messages, see
[Codex Local-Model Launches](../reference/codex-local-model-launch.md).
