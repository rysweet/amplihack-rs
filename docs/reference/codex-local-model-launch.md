---
title: Codex Local-Model Launches
description: How amplihack codex selects a local model, builds its own Codex home, and starts Codex with no amplihack hook text.
doc_type: reference
last_updated: 2026-11-25
---

# Codex Local-Model Launches

A local-model launch is an `amplihack codex` launch for a model that runs on
your own machine. amplihack starts Codex with a Codex home of its own under
`~/.amplihack/codex-local/<name>/`, keeps only its command-check hook, and sets
`AMPLIHACK_HOOK_TEXT=none` so that its hooks add no text to the model's
context. Your own `~/.codex` is not modified.

For the steps to set one up, see
[Run amplihack codex with a Local Model](../howto/run-codex-with-a-local-model.md).

## Contents

- [Selecting a local-model launch](#selecting-a-local-model-launch)
- [Profile names](#profile-names)
- [Other launch options](#other-launch-options)
- [The user Codex home and the profile](#the-user-codex-home-and-the-profile)
- [The generated Codex home](#the-generated-codex-home)
- [config.toml](#configtoml)
- [hooks.json](#hooksjson)
- [Rules file and instruction file](#rules-file-and-instruction-file)
- [Refresh](#refresh)
- [Nested launches](#nested-launches)
- [Start-up and bootstrap](#start-up-and-bootstrap)
- [Command line and environment of Codex](#command-line-and-environment-of-codex)
- [Model tiers](#model-tiers)
- [Order of a launch](#order-of-a-launch)
- [Errors](#errors)
- [What differs in a local session](#what-differs-in-a-local-session)
- [Limitations](#limitations)
- [Validation scenarios](#validation-scenarios)
- [Related](#related)

---

## Selecting a local-model launch

| Source | Form | Notes |
|---|---|---|
| Flag | `amplihack codex --local-model <name>` | An empty value (`--local-model ""`) is a usage error. |
| Environment | `AMPLIHACK_CODEX_LOCAL_MODEL=<name>` | An empty value counts as unset. The value is not trimmed. |

- The flag wins when both are given.
- A recipe's agent step inherits the variable, so
  `AMPLIHACK_CODEX_LOCAL_MODEL=inkling amplihack recipe run …` with
  `AMPLIHACK_AGENT_BINARY=codex` runs every Codex agent step on the local
  model. Agent steps cannot add flags, which is why the variable exists.
- The selection applies to `amplihack codex` only. `amplihack claude`,
  `amplihack copilot` and `amplihack amplifier` ignore the variable.
- An invalid selection is an error. It never falls back to an ordinary launch,
  which could send the session to a hosted model.

Dispatch prepares the selection before launch and supplies it through a scoped
thread-local value. Model resolution and bootstrap read that value on the same
thread. The scope restores the previous selection when the launch returns or
panics. With no selection, ordinary launches use their existing paths.

```sh
amplihack codex --local-model inkling
AMPLIHACK_CODEX_LOCAL_MODEL=inkling amplihack codex -- exec -
```

## Profile names

The same rule applies to both sources and is checked before the name is used
anywhere:

- 1 to 64 characters;
- ASCII letters, digits, `.`, `_` and `-` only;
- not `.` and not `..`.

`inkling`, `qwen3-coder.30b` and `llama_8b` are valid. `../x`, `a/b`, a name
with a space, a name with a non-ASCII letter and a 65-character name are not.
The error names the rule, the source (flag or variable) and the value, escaped
and cut to 64 characters.

## Other launch options

| Option | With a local-model launch |
|---|---|
| `--docker`, or Docker turned on through `AMPLIHACK_USE_DOCKER` | Error, before anything is created. The message names which of the two turned Docker on. |
| `--auto` | Error, before anything is created. |
| `--append` | Unaffected; it starts no Codex process. |
| `--subprocess-safe` | Allowed, with its usual meaning: the [generic bootstrap](#start-up-and-bootstrap) is skipped. The generated home is still refreshed. |
| `--reset-recovery` | Allowed. It still archives a journal under `~/.amplihack/codex/`. It runs at start-up, before the name is checked, so it archives the journal even when the launch then fails on an invalid name. |
| `--no-reflection` | Allowed. |
| Arguments after `--` (for example `-c`, `-m`, `-p`, `exec`) | Passed to Codex unchanged, after the two additions described in [Command line and environment of Codex](#command-line-and-environment-of-codex). A model tier word (`--model fast`, `standard` or `strong`) is the exception: it is removed, as described in [Model tiers](#model-tiers). |

## The user Codex home and the profile

The user Codex home is `$CODEX_HOME` when that is set, non-empty and not
inside `~/.amplihack/codex-local/`. Otherwise it is `~/.codex`.

The profile is read from `<user Codex home>/<name>.config.toml`. On a host
whose profile is `~/.codex/inkling.config.toml`:

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

The profile and the user's `hooks.json` are read under these rules:

- A symbolic link is followed, but what it reaches must be a regular file. A
  FIFO, a device or a directory is refused without blocking.
- Each file may be at most 1 MiB.
- A parse error reports the file's path and the line (and column, for JSON).
  It never quotes the file's content, so a key in the profile cannot reach a
  log.
- In the profile, `features`, `skills` and `skills.bundled` must be tables
  where present (standard, inline or dotted). Any other type is an error
  naming the path and the key.
- A missing `hooks.json` means there are no user entries. A malformed one is an
  error naming the file.
- Both files decide what runs in a local session: the profile names the
  provider and may set `notify` or MCP servers, and `hooks.json` supplies
  handlers that may run on each matching tool call (see
  [What differs in a local session](#what-differs-in-a-local-session)). Each
  file, the directory holding it and, for a symbolic link, the directory
  holding its target must therefore be owned by the effective user, with no
  group or other write permission. Root-owned inputs are accepted only when
  the effective user is root. Unlike the shared directory rule, this does
  not allow private-group writes. A file or directory that breaks the rule
  is an error identifying it, and amplihack does not change it.

Nothing else is read from the user Codex home. Its `config.toml`, `auth.json`,
history, sessions and `rules/` are not read or copied.

## The generated Codex home

```text
~/.amplihack/                     must exist or is created at 0700; owned by you, not writable by others
└── codex-local/                  0700
    └── inkling/                  0700, the CODEX_HOME Codex receives
        ├── config.toml           0600, owned by amplihack
        ├── hooks.json            0600, owned by amplihack
        ├── instructions.md       0600, owned by amplihack
        ├── rules/                0700
        │   └── amplihack-dangerous-commands.rules   0600, owned by amplihack
        ├── sessions/ …           written by Codex
        └── auth.json             written by Codex, only if you log in there
```

- `~/.amplihack` is created at 0700 only when it is missing. An existing one
  must be a directory you own that is not writable by others; it is never
  modified. The check follows a symbolic link at `~/.amplihack` itself.
- `codex-local`, `<name>` and `rules` are created at 0700. An existing one
  with looser permissions is an error; amplihack does not change its mode.
- A symbolic link at the home, at `rules/` or at an owned file stops the launch.
- amplihack installs no plugin, no `skills/` directory and no marketplace in
  the generated home.

## config.toml

The file is built from the profile in four steps.

1. **Copy the profile**, then remove these top-level keys and tables:
   `marketplaces`, `plugins`, `projects`, `hooks`, `profile` and `profiles`.
   Removing `profile` and `profiles` stops a profile from switching back the
   settings below.
2. **Set amplihack's settings**, which override the profile:

   | Setting | Value | Effect |
   |---|---|---|
   | `project_doc_max_bytes` | `0` | Codex reads no `AGENTS.md` into the prompt. |
   | `model_instructions_file` | absolute path of `<home>/instructions.md` | Replaces Codex's own model instructions with the short file amplihack ships. |
   | `web_search` | `"disabled"` | No web search tool. |
   | `[features]` `hooks` | `true` | Codex runs hooks, so the command-check hook runs. A profile's `hooks = false` is overridden. |
   | `[features]` `multi_agent`, `goals`, `view_image`, `plugins`, `apps` | `false` | The tools and listings these features add are left out of the prompt. |
   | `[skills.bundled]` `enabled` | `false` | No bundled skills are listed. Other keys in the profile's `[skills.bundled]` are kept. |

   Keys in the profile's `[features]` other than these six are kept. A profile
   cannot turn hooks off or turn on any of the five features above.

3. **Fill in `approval_policy`.** The profile's value is kept. When the profile
   has none, `approval_policy = "never"` is set, which is what an ordinary
   launch fills into `~/.codex/config.toml`. This is a default, not an
   override.
4. **Keep Codex state.** Copy `[projects.*]`, `[notice]` and
   `[hooks.state]` from the previous generated file. This preserves project
   trust, dismissed notices and hook reviews or disabled states chosen in the
   local home. Hook definitions are never copied from that file. Restore
   `enabled = true` and the current `trusted_hash` only for amplihack's own
   first PreToolUse entry. The hash follows Codex 0.166.0's normalised handler
   identity, including its default duration. No other handler is automatically
   trusted. The user's ordinary Codex configuration is never read.

The output depends only on these inputs, so two launches with the same inputs
write the same bytes. For the `inkling` profile above, with no previous file
and a home directory of `/work/fixture`, the model settings are shown below.
The generated `[hooks.state]` record is omitted here because its key and hash
depend on the home path and hooks binary.

```toml
approval_policy = "never"
model = "inkling-2-mini"
model_context_window = 65536
model_instructions_file = "/work/fixture/.amplihack/codex-local/inkling/instructions.md"
model_provider = "llamacpp-local"
project_doc_max_bytes = 0
web_search = "disabled"

[features]
apps = false
goals = false
hooks = true
multi_agent = false
plugins = false
view_image = false

[model_providers.llamacpp-local]
base_url = "http://127.0.0.1:8080/v1"
name = "llama.cpp (local)"
stream_max_retries = 2
wire_api = "responses"

[skills.bundled]
enabled = false
```

Keys are written in alphabetical order within each table, not in the
profile's order. Compare settings rather than layout: the layout is fixed for
given inputs but is not part of this contract.

The setting names were checked against codex-cli 0.166.0, where `codex
features list` shows `hooks` as a stable feature that is on by default. A later
Codex that renames or rejects one of them reports it when it reads the file.

## hooks.json

The file holds `PreToolUse` and nothing else:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "hooks": [
          {
            "command": "env AMPLIHACK_HOOK_PROTOCOL=codex '/work/fixture/.local/bin/amplihack-hooks' pre-tool-use",
            "type": "command"
          }
        ]
      },
      {
        "hooks": [
          {
            "command": "env XPIA_ENABLED=false '/work/fixture/.amplihack/codex/market/plugin/bin/hook' pre-tool-use",
            "type": "command"
          }
        ]
      },
      {
        "hooks": [
          {
            "command": "sh ~/.codex/hooks/my-check.sh tmux",
            "type": "command"
          }
        ]
      },
      {
        "hooks": [
          {
            "command": "sh ~/.codex/hooks/my-check.sh shell",
            "type": "command"
          }
        ]
      }
    ]
  }
}
```

**amplihack's group** comes first. It has no matcher and one command handler
that runs the hooks binary directly with `AMPLIHACK_HOOK_PROTOCOL=codex` fixed
in the command text. The path is absolute and single-quoted, with a `'` inside
it written as `'\''`, as the Codex plugin's own hook commands are.

The hooks binary is found as `amplihack install` finds it:
`AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`, then the directory of the `amplihack`
executable, then `PATH`, then `~/.local/bin` and `~/.cargo/bin`. The result
must be an absolute path to a regular, executable file named
`amplihack-hooks`, in valid UTF-8 with no control characters. A relative
result is an error naming `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`; it is not
made absolute, so a checkout cannot supply the guard.

**Your PreToolUse groups** from the user Codex home's `hooks.json` follow, in
their original order and otherwise unchanged, except that handlers which run
amplihack's pre-tool-use hook exactly as amplihack's own entry does are
removed. A handler is removed when its command, split into shell words, is
exactly these words, optionally after `env` or after
`env AMPLIHACK_HOOK_PROTOCOL=codex`:

- a program named `amplihack-hooks`, or an absolute path under
  `~/.amplihack/codex/` ending in `bin/hook` (the Codex plugin's wrapper);
- followed by `pre-tool-use`;

and every word, the `env` prefix included, is one the shell takes as it is:
no whitespace, no control character and none of
``; & | < > ( ) $ ` \ " ' * ? [ ] { } ~ ! #``. The word parser removes quoting
but does not evaluate shell operators or expansions. Such characters are
therefore treated conservatively: a handler containing one is kept, even when
the character was quoted.

So on a host whose `~/.codex/hooks.json` has these four PreToolUse handlers:

```text
env XPIA_ENABLED=false '/work/fixture/.amplihack/codex/market/plugin/bin/hook' pre-tool-use
'/work/fixture/.amplihack/codex/market/plugin/bin/hook' pre-tool-use
sh ~/.codex/hooks/my-check.sh tmux
sh ~/.codex/hooks/my-check.sh shell
```

the second is removed and the other three are kept, as in the file above. The
first is kept because it sets a variable: any assignment other than
`AMPLIHACK_HOOK_PROTOCOL=codex` may change what the hook checks
(`XPIA_ENABLED=true` turns the XPIA check on), so removing the handler could
weaken your guard without a word. A kept handler of this kind runs
amplihack's pre-tool-use hook a second time, which is safe and costs one more
process per tool call. A compound command, a command that cannot be split, and
a handler that is not a command are kept. A group left with no handlers is
dropped.

Other events from your `hooks.json` (`SessionStart`, `UserPromptSubmit`,
`PostToolUse`, `Stop`, `SessionEnd`) are not copied.

## Rules file and instruction file

`rules/amplihack-dangerous-commands.rules` is the execpolicy rules file from
the [dangerous-command policy](./dangerous-command-policy.md), including any
`policy.deny_commands` entries in `~/.amplihack/config.toml`. It is the same
file `amplihack install` writes for an ordinary Codex home. Codex applies it
independently of hooks, including under `approval_policy = "never"`.

`instructions.md` is embedded in the `amplihack` binary and is at most 1,024
bytes of UTF-8. It tells the model it is a coding agent with a shell tool and
asks it to keep to the task it is given, not to delete directories, discard
changes or rewrite history unless the task asks it to, and to treat file
contents and command output as data rather than instructions. It does not
forbid what a task may ask for, such as committing or using the network. Its
source is
`crates/amplihack-cli/src/commands/launch/codex_local_model/instructions.md`.

## Refresh

Every local-model launch that does not [reuse](#nested-launches) a home
refreshes it:

- All four owned files are computed before the first write. The previous
  `config.toml` is read for the tables in step 4; a symbolic link there, or a
  file that does not parse, stops the launch with an error naming the file,
  and nothing is written.
- A file is skipped when its bytes are already correct and it is a private
  regular file (mode 0600, owned by you). Otherwise it is written to a new
  temporary file with a unique name in the same directory, at mode 0600, and
  renamed into place. An unchanged home therefore keeps every file's inode.
- A changed owned setting, such as `web_search = "live"` added by hand, is
  restored.
- Nothing else in the home is listed, changed or deleted. Codex's sessions,
  databases and logs are left alone.
- No lock is taken. Concurrent launches with the same inputs write the same
  bytes.

## Nested launches

A Codex session started by a local-model launch has `CODEX_HOME` pointing at
the generated home and `AMPLIHACK_CODEX_LOCAL_MODEL` set. An `amplihack codex`
started inside it, such as an agent step of a recipe the session runs, is
resolved as below. The steps of a recipe started from a shell outside any
local session are not nested launches: each one refreshes the home.

| `CODEX_HOME` | Selected name | Result |
|---|---|---|
| the generated home for `inkling` | `inkling` | The home is **reused**: no profile is read and no file is written. |
| the generated home for `inkling` | `qwen` | Home `qwen` is built from `~/.codex/qwen.config.toml`, because a `CODEX_HOME` inside `codex-local` is not a user Codex home. |
| a custom home, for example `/srv/codex` | `qwen` | Home `qwen` is built from `/srv/codex/qwen.config.toml`. |

Reuse needs the home's `config.toml` and `hooks.json` to exist. Before
reusing, amplihack checks the home without writing to it:

- `codex-local`, `<name>` and `rules/` are private directories;
- `config.toml`, `hooks.json` and the rules file are private regular files and
  not symbolic links;
- `config.toml` parses and sets `[features]` `hooks = true`;
- `config.toml` does not switch amplihack's handler off: no `[hooks.state]`
  entry for the first PreToolUse group of the home's `hooks.json`, keyed by
  that file's path as `CODEX_HOME` gives it or canonicalised, has an `enabled`
  value other than `true`, and no such entry is anything but a table; a path
  that cannot be canonicalised, or is not valid UTF-8, fails the check;
- the first PreToolUse group in `hooks.json` is exactly amplihack's entry, and
  its hooks binary is still an absolute, executable regular file named
  `amplihack-hooks` that resolves to the same file as the hooks binary this
  launch finds (see [hooks.json](#hooksjson)), so a session cannot point
  nested launches at another program;
- amplihack's entry has the current trusted hash in the generated file;
- the rules file's bytes equal what the current policy renders.

If any check fails, the launch stops with an error naming the home and asking
you to relaunch from outside the session. Codex is not started. Changing
`policy.deny_commands` during a session therefore makes nested launches fail
until you relaunch.

Reuse avoids rewriting `config.toml` under a running outer Codex. Apart from
`[features]` `hooks` and amplihack's `[hooks.state]` entry, it does not check
the settings in `config.toml`; any other setting changed inside the session,
such as `web_search`, lasts until the next launch from outside it.

## Start-up and bootstrap

A local-model launch never writes to the user Codex home:

- Start-up self-heal treats any selection, valid or not, as a launch with no
  Codex provider, so it does not install or update the Codex plugin.
- The Codex-specific bootstrap does not run: no retirement of stale Codex
  configuration, no `configure_codex()` on the user `config.toml`, and no
  plugin or hook reconciliation.
- `~/.amplihack/codex/pending.json` and other recovery metadata are not
  touched, except by `--reset-recovery`.

The generic bootstrap, the part of start-up that is not specific to one tool,
runs as it does for any launch, and it writes outside the generated home:

- it restages missing framework assets under `~/.amplihack/.claude`;
- it tops up the slash commands in `~/.claude/commands/amplihack/`;
- it repairs amplihack's hook registrations and deny entries in
  `~/.claude/settings.json`, which needs the hooks binary, so a missing
  `amplihack-hooks` stops the launch at this point;
- it checks the recipe runner's version.

The generic bootstrap is skipped under `--subprocess-safe` and in
non-interactive mode (`AMPLIHACK_NONINTERACTIVE=1`, or standard input that is
not a terminal). Most recipe agent steps run in non-interactive mode, so for
them it does not run.

## Command line and environment of Codex

amplihack puts up to three things before the Codex arguments (those after
amplihack's own `--`):

- **Hook trust.** Interactive launches use the generated trust record for
  amplihack's entry and retain Codex's review for other handlers. On `exec`,
  `--dangerously-bypass-hook-trust` comes first unless already supplied before
  the Codex end-of-options marker. The agent-step injection does not add it
  again. A caller can explicitly request the bypass for an interactive session.
- **Hooks on.** `-c features.hooks=true` comes next, always. A trusted
  project's `.codex/config.toml` takes precedence over the generated
  `config.toml` but not over the command line, so this keeps hooks on in such
  a project. A `-c features.hooks=false` or `--disable hooks` that you pass
  comes later on the command line and still wins.
- **The profile's sandbox.** When the profile sets `sandbox_mode`,
  `-c sandbox_mode="<value>"` comes next. This makes the profile's sandbox the
  caller's choice under the
  [agent-step rules](./launch-flag-injection.md#codex-agent-step-flags), so an
  `exec` launch does not get `--sandbox danger-full-access`, which would
  outrank the config file. A `--sandbox` or `-c sandbox_mode=…` you pass comes
  later and still wins. Without `sandbox_mode` in the profile, nothing is
  added and an `exec` launch gets `danger-full-access` as an ordinary agent
  step does.

Codex 0.166.0 applies hook state in the configuration layer that supplies
the hook. Native `hooks/list` checks on a loaded thread showed that a
trusted project's `[hooks.state]` entry could neither disable nor remove
trust from the generated home's command check. The project configuration
was loaded, and its own handler remained untrusted. Setting `enabled = false`
in the generated home's configuration did disable the check. Nested reuse
therefore checks that configuration before starting Codex. These checks
inspect Codex's resolved handlers; they do not replace a live command-check
exercise against a model.

```sh
amplihack codex --local-model inkling
# → codex -c features.hooks=true

amplihack codex --local-model inkling -- exec --output-last-message ./final.txt -
# → codex --dangerously-bypass-hook-trust -c features.hooks=true exec --sandbox danger-full-access --output-last-message ./final.txt -

amplihack codex --local-model inkling -- --dangerously-bypass-hook-trust exec -
# → codex -c features.hooks=true --dangerously-bypass-hook-trust exec --sandbox danger-full-access -
```

Every local-model launch writes one line to stderr before Codex starts, saying
that hook text is off and what that costs:

```text
amplihack: codex local model "inkling": AMPLIHACK_HOOK_TEXT=none, so amplihack's hooks add no text and its stop hook approves without running: lock mode, power steering and reflection do not run in this session and in amplihack sessions started from it
```

When amplihack adds the hook-trust flag it writes a second line. The line
names what was added, the profile, the home and the number of your PreToolUse
handlers carried over. It never contains command text:

```text
amplihack: codex local model "inkling": added --dangerously-bypass-hook-trust and -c features.hooks=true (Codex home "/work/fixture/.amplihack/codex-local/inkling", 3 user PreToolUse handlers carried over)
```

With exactly one handler carried over, the count reads
`1 user PreToolUse handler carried over`. Any other number, including `0`,
uses `handlers`. This second line is not written when you passed the
hook-trust flag yourself.

On an `exec` launch the agent-step notice from
[launch flag injection](./launch-flag-injection.md#codex-agent-step-flags)
follows as usual. Because hook trust is already present, it reads
`amplihack: codex exec: added --sandbox danger-full-access (#1985)`. When the
profile sets `sandbox_mode`, nothing is added and no agent-step notice is
written.

No `--profile` is passed: the model and provider come from the generated
`config.toml`.

**Environment.** These are set on the Codex child only:

| Variable | Value |
|---|---|
| `CODEX_HOME` | the generated home, for example `/work/fixture/.amplihack/codex-local/inkling` |
| `AMPLIHACK_HOOK_TEXT` | `none` |
| `AMPLIHACK_CODEX_LOCAL_MODEL` | the selected name |

`AMPLIHACK_HOOK_PROTOCOL` is not set; the hook command sets it for each hook
run. amplihack's own process environment is not changed.

An ordinary `amplihack codex` launch passes `CODEX_HOME` and
`AMPLIHACK_HOOK_TEXT` through exactly as it received them, absent when absent,
and creates no `codex-local` directory. When the value it received means
`none`, it writes one stderr line saying so, as described under
[What differs in a local session](#what-differs-in-a-local-session).

## Model tiers

A recipe agent step may declare a [model tier](./model-tiers.md), which the
recipe runner puts on the command line as `--model fast`, `standard` or
`strong`. On a local-model launch the profile sets the model and the reasoning
effort, so amplihack removes the tier word and adds nothing in its place:

- no `-c model_reasoning_effort=…`;
- no model from `model.force` or `AMPLIHACK_FORCE_MODEL`;
- no `AMPLIHACK_DEFAULT_MODEL`.

One stderr line says that the tier, or `model.force`, was not applied, and the
step's record in `model-tiers.jsonl` has `fallback` set to `local_model`. A
concrete `--model <id>`, such as `--model qwen3`, and any `-c` you pass reach
Codex unchanged.

## Order of a launch

`--reset-recovery`, when given, has already archived the recovery journal at
start-up, before step 1.

1. Select the name and check it.
2. Refuse a non-Unix platform, Docker and `--auto`.
3. Resolve the user Codex home and decide whether to reuse the generated home.
4. Unless reusing: read and check the profile, then read and filter the user
   `hooks.json`.
5. Run the [generic bootstrap](#start-up-and-bootstrap), unless
   `--subprocess-safe` is given or the launch is non-interactive.
6. Find and check the hooks binary, check `~/.amplihack`, and build, refresh
   or verify the home.
7. Start Codex.

A failure before step 6 exits non-zero, starts no Codex process and creates
nothing under `codex-local`, although a bootstrap that fails in step 5 may
already have written under `~/.amplihack` or `~/.claude`. A failure in step 6
starts no Codex process.

## Errors

Each error exits non-zero and Codex is not started.

| Condition | What the message names |
|---|---|
| `--local-model ""` | the flag (usage error from argument parsing) |
| A name that breaks the [name rule](#profile-names) | the rule, the source and the escaped value |
| Not a Unix platform | that local-model launches are Unix only |
| Docker turned on | `--docker` or `AMPLIHACK_USE_DOCKER` |
| `--auto` | `--auto` |
| Profile missing | the expected path, for example `/work/fixture/.codex/inkling.config.toml` |
| Profile not a regular file, over 1 MiB, or not valid TOML | the path, and the line for a parse error |
| `features`, `skills` or `skills.bundled` in the profile is not a table | the path and the key |
| User `hooks.json` malformed, wrong shape, not a regular file or over 1 MiB | the path, and the line and column for a parse error |
| Profile or user `hooks.json`, or a directory holding one, owned by another user or writable by others | the path |
| Hooks binary not found, relative, not a regular file, not executable, not UTF-8, containing control characters or not named `amplihack-hooks` | `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` |
| `~/.amplihack` not a directory, not owned by you, or writable by others | the path |
| A symbolic link at the home, `rules/` or an owned file; a directory with loose permissions | the path |
| Previous generated `config.toml` does not parse | the path and the line |
| A reused home fails its checks, including a `config.toml` that switches amplihack's handler off or a hooks binary other than the one this launch finds | the home, the check, and that you should relaunch from outside the session |

No message quotes a value from the profile, the generated `config.toml` or
your `hooks.json`. Paths and names are printed escaped.

## What differs in a local session

- **No hook text for the model, and no stop checks.** Under
  `AMPLIHACK_HOOK_TEXT=none` the session-start, user-prompt-submit and
  workflow-classification-reminder hooks add nothing to the model's context,
  and the stop hook approves without running, so lock mode, power steering and
  reflection do not run. The launch says so on stderr. See
  [Hook text setting](../howto/omit-hook-context-text.md).
  In a local session only the PreToolUse hook is registered anyway; the
  variable also covers hooks reached another way, described below.
- **The command check still runs.** The pre-tool-use hook and the rules file
  deny a destructive command as in an ordinary session. Hooks are forced on
  twice: `[features]` `hooks = true` in the generated `config.toml`, so the
  profile cannot turn them off, and `-c features.hooks=true` on the command
  line, so a trusted project's `.codex/config.toml` cannot either. A `-c` or
  `--disable hooks` you pass after `--` still turns them off. Apart from
  hooks, a trusted project's `.codex/config.toml` can change settings exactly
  as in an ordinary session.
- **Interactive hook trust is preserved.** Only amplihack's generated
  command-check entry is automatically trusted. Review your copied PreToolUse
  handlers and trusted-project handlers through `/hooks` before they run.
  New or changed definitions need review, as described in the
  [Codex hook documentation](https://learn.chatgpt.com/docs/hooks).
  Trust and disabled states from your ordinary Codex configuration are not
  copied. Choices you make in the local home survive refreshes, except that
  amplihack's entry is always restored. On `exec`, the existing agent-step
  bypass runs enabled handlers without review. A trusted project's hook may
  call other amplihack hooks; `AMPLIHACK_HOOK_TEXT=none` applies there too.
- **XPIA follows your handlers.** A handler such as
  `env XPIA_ENABLED=true '<…>/bin/hook' pre-tool-use` in your `hooks.json` is
  kept, so its XPIA check runs in a local session as it does elsewhere.
  amplihack's own entry sets no XPIA variable: it reads `XPIA_ENABLED` from the
  environment it inherits through Codex, and XPIA is off when that is unset.
- **The profile's sandbox is kept.** A `sandbox_mode` in the profile applies
  to `exec` launches too, recipe agent steps included, because amplihack
  passes it on as the caller's choice. With none, an `exec` launch runs under
  `danger-full-access`, as an ordinary agent step does.
- **Session data stays in the generated home.** History, sessions, trust
  entries and any `auth.json` from a login made in a local session are stored
  under `~/.amplihack/codex-local/<name>/`, not in `~/.codex`. Your existing
  `~/.codex/auth.json` is not copied. A provider that needs a key reads it
  through the provider's own settings, such as `env_key` in the profile's
  `[model_providers.<id>]` table.
- **Less traffic, fewer channels, no guarantee.** `web_search = "disabled"` and
  `project_doc_max_bytes = 0` mean Codex offers no web search and reads no
  `AGENTS.md`, which also removes `AGENTS.md` as a way to inject instructions.
  These are properties of the Codex settings, not a sandbox: depending on the
  sandbox, the model can still reach the network and read files through the
  shell.
- **`AMPLIHACK_HOOK_TEXT=none` is inherited.** A Claude, Copilot or ordinary
  Codex session started from inside a local session, for example by a recipe,
  receives the variable, so it receives no amplihack hook text either and its
  stop hook approves every stop: lock mode, power steering and reflection do
  not run there. Such a launch writes one line to stderr naming the variable.
  Unset it in that child's environment if you want them back.
- **The capacity-notice record is still written.** When no Claude account has
  capacity, the notice is suppressed but its once-per-session record is still
  written, so the notice is not shown later in that session.

## Limitations

- Unix only (Linux, macOS, WSL). On other platforms the launch is an error.
- A nested launch under a different name, inside a session started with a
  custom `CODEX_HOME`, reads its profile from `~/.codex`, because the outer
  `CODEX_HOME` is the generated home and is not a user Codex home.
- `amplihack install` run by hand inside a local session reconciles the Codex
  plugin into the `CODEX_HOME` it inherits, which is the generated home. Run it
  from outside the session.
- Generated homes for profiles you no longer use are not removed. Delete
  `~/.amplihack/codex-local/<name>/` yourself.
- Hooks and provider tables defined in `~/.codex/config.toml`, rather than in
  the profile file or `hooks.json`, are not carried over.
- A table that a later Codex starts writing to `config.toml` is lost at each
  refresh until amplihack adds it to the carry-over list.
- Two launches with the same name at the same moment can lose a project trust
  entry the other session wrote between its read and its write.

## Validation scenarios

The QA descriptor at `tests/gadugi/scenarios/codex-local-model-launch.yaml`
uses the native `cli_integration` tests for launch selection, input errors,
refresh and nested reuse. Each group runs the built executable with private
fixture homes. The interactive bootstrap case uses a terminal and compares
the local launch with an ordinary launch. A stub Codex records arguments and
environment, so these tests need no model provider.

Live validation also needs real Codex interactive and `exec` sessions against
a local model. Check that a dangerous command is blocked in both, that only
the command-check entry is automatically trusted, and that project settings
cannot disable that entry. Record the Codex version, model, provider,
instructions and tool configuration with any first-prompt measurements.
Request bytes and token counts are different units. Compare token counts
only when their measurement method and included prompt content agree.

## Related

- [Run amplihack codex with a Local Model](../howto/run-codex-with-a-local-model.md): the steps to set up and use a local-model launch
- [Hook Specifications](../howto/omit-hook-context-text.md): what each hook prints under `AMPLIHACK_HOOK_TEXT`
- [Launch Flag Injection](./launch-flag-injection.md#codex-local-model-launches): the hook-trust flag and the agent-step flags
- [Use Amplihack with Codex](../howto/install-codex-plugin.md): ordinary Codex installs, launches and hook trust
- [Dangerous Command Policy](./dangerous-command-policy.md): the rules file written into the generated home
- [Environment Variables](./environment-variables.md#amplihack_codex_local_model): `AMPLIHACK_CODEX_LOCAL_MODEL` and `AMPLIHACK_HOOK_TEXT`
