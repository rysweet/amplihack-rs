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
Claude, Copilot and ordinary `amplihack codex` sessions are unaffected, unless
they are started from inside a local session.

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
  codex-cli 0.166.0. An interactive session needs that version, because
  amplihack trusts its command check with the hash that version computes; on
  another version an interactive launch is rejected. `codex exec` launches
  run on any version.
- `amplihack-hooks` deployed by `amplihack install`, at
  `~/.local/bin/amplihack-hooks`. It must be owned by you, and neither it nor
  its directory may be writable by your group or by others.
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
stream_max_retries = 2
```

If the provider needs a key, give the variable with `env_key` in the provider
table. Your `~/.codex/auth.json` is not copied into the local session.

Your own `approval_policy`, `sandbox_mode` and `[sandbox_workspace_write]` from
the top level of `~/.codex/config.toml` apply where the profile sets none, and
your top-level `~/.codex/rules/*.rules` files are copied into the local
session's home. The local session is therefore no looser than an ordinary
one. To use other settings in a local session, set them in the profile.

## Start an interactive session

```sh
amplihack codex --local-model inkling
```

amplihack builds or refreshes `~/.amplihack/codex-local/inkling/`, prints a
line to stderr saying that hook text is off (so lock mode, power steering and
reflection do not run), and starts Codex on that home. Only amplihack's
command-check entry is automatically trusted. Review your own handlers through
`/hooks`. Exec sessions retain the agent-step trust bypass, so read
[What differs in a local session](../reference/codex-local-model-launch.md#what-differs-in-a-local-session)
before you trust a project in it.

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

Codex runs with full access, as any
[agent step](../reference/launch-flag-injection.md#codex-agent-step-flags)
does, unless the profile sets `sandbox_mode`, which is then kept. Pass your
own `--sandbox` after `exec` to choose another for one run. The full command
line is in the
[reference](../reference/codex-local-model-launch.md#command-line-and-environment-of-codex).

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
counts as unset. A [model tier](../reference/codex-local-model-launch.md#model-tiers)
a step declares is not applied: the profile sets the model and the reasoning
effort.

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

What amplihack writes in each file is in the
[reference](../reference/codex-local-model-launch.md#the-generated-codex-home).
`hooks.json` holds amplihack's entry first, then your own PreToolUse handlers
from `~/.codex/hooks.json`.

## Change the model or provider settings

Edit the profile file, not the generated `config.toml`, then launch again:

```sh
sed -i 's/^model_context_window = .*/model_context_window = 32768/' ~/.codex/inkling.config.toml
amplihack codex --local-model inkling
```

Each launch from outside a local session refreshes the home from the profile.
A change you make by hand to an amplihack setting in the generated
`config.toml` is undone at the next launch. Projects you trusted and notices
you dismissed in the local session are kept. Hook reviews and disabled states
chosen in the local home are kept too. Each refresh restores only amplihack's
own command check to its enabled and trusted state.

To use a second model, add another profile file, for example
`~/.codex/qwen.config.toml`, and launch with `--local-model qwen`. Each name
has its own home.

## Go back to an ordinary launch

Drop the flag and clear the variable:

```sh
unset AMPLIHACK_CODEX_LOCAL_MODEL
amplihack codex
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
| Error naming `~/.codex/inkling.config.toml` | The profile file is missing | Create it, or check the name. With `CODEX_HOME` set, the file is looked up there. |
| Error about the name rule, naming the flag or `AMPLIHACK_CODEX_LOCAL_MODEL` | The name has a character outside `[A-Za-z0-9._-]`, is longer than 64 characters, or is `.` or `..` | Rename the profile file and use the new name. |
| Error naming `--docker`, `AMPLIHACK_USE_DOCKER` or `--auto` | These modes are not available with a local model | Drop the option, or unset `AMPLIHACK_USE_DOCKER`. |
| Error giving an `amplihack-hooks` path and suggesting `amplihack install` | The hooks binary was not found, was found by a relative path, is not named `amplihack-hooks`, or it or its directory belongs to another user (root included) or is writable by others | Run `amplihack install`, which deploys `~/.local/bin/amplihack-hooks`. Without a deployed copy, set `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` to the absolute path of an `amplihack-hooks` you own, for example after `chmod go-w` on it and its directory. |
| Error giving a Codex version, for an interactive launch | The hook-trust format of that Codex version is not known, so the command check could be untrusted | Use codex-cli 0.166.0 for interactive sessions, or run `amplihack codex --local-model <name> -- exec …`. |
| Error giving `~/.codex/config.toml` or a file in `~/.codex/rules/` | The file does not parse, is not a regular file, or it or its directory is writable by others | Fix the file at the line given, or `chmod go-w` it and its directory. |
| Error saying a symbolic link's target does not exist | A link at the profile, `hooks.json`, `config.toml` or a rules file points nowhere | Fix the link, or remove it. |
| A stderr line saying the command check does not run in the session | Your Codex arguments include `-c features.hooks=false` or `--disable hooks` | Leave the argument out to keep the command check. |
| Error naming `~/.codex/hooks.json` | Your `hooks.json` does not parse or has the wrong shape | Fix the file at the line given. |
| Error saying the profile, `hooks.json` or `~/.codex` must be owned by you and not writable by others | The file or its directory belongs to another user, or others can write it | Take ownership and remove write permission for others, for example `chmod go-w ~/.codex ~/.codex/inkling.config.toml`. amplihack does not change modes itself. |
| Error naming the generated `config.toml` | The generated file does not parse | Fix it, or delete it and launch again. |
| Error naming `~/.amplihack` | It is not a directory, is not owned by you, or is writable by others. A symbolic link at `~/.amplihack` is followed and is accepted when its target passes these checks. | Make it a directory you own and remove write permission for others, for example `chmod go-w ~/.amplihack`. amplihack does not change its mode. |
| Error naming a path under `~/.amplihack/codex-local/` | A directory there has permissions looser than 0700, or the path is a symbolic link | `chmod 700` the directory. For a link, remove it and launch again, and amplihack creates a real directory or file in its place. amplihack does not change modes itself. |
| A nested launch fails and asks you to relaunch from outside the session | The reused home no longer passes its checks, for example after `policy.deny_commands` changed, or after amplihack's handler was switched off in `/hooks` | Exit the session and launch again from a shell outside it. |
| Codex reports an unknown setting such as `web_search` | A Codex release renamed a setting amplihack writes | Report it, with your `codex --version`. |
| No amplihack hook text, and a stderr line naming `AMPLIHACK_HOOK_TEXT`, in a Claude or Copilot session | It was started from inside a local session and [inherited](../reference/hook-specifications.md#inheritance) `AMPLIHACK_HOOK_TEXT=none` | Start it with `env -u AMPLIHACK_HOOK_TEXT`. |

For every rule behind these messages, see
[Codex Local-Model Launches](../reference/codex-local-model-launch.md).
