# User Configuration — Reference

`amplihack` reads persistent defaults from two TOML files: one for you, one for
a project. `amplihack config` reads and edits them. Every environment variable
and command-line flag that existed before still works and still wins over the
files.

## Contents

- [Quick start](#quick-start)
- [Files](#files)
- [Precedence](#precedence)
- [Keys](#keys)
  - [agent.binary](#agentbinary)
  - [paths.home](#pathshome)
  - [node.heap_consent and node.max_old_space_mb](#nodeheap_consent-and-nodemax_old_space_mb)
  - [recipe.verbose](#recipeverbose)
  - [recipe.step_timeout](#recipestep_timeout)
  - [recipe.detach, recipe.tmux_session_pattern and recipe.log_dir](#recipedetach)
  - [recipe.usage_limit_wait and recipe.usage_limit_max_wait_secs](#recipeusage_limit_wait)
  - [orchestrator.max_depth](#orchestratormax_depth)
  - [Governor keys](#governor-keys)
- [The config command](#the-config-command)
  - [config get](#config-get)
  - [config set](#config-set)
  - [config unset](#config-unset)
  - [config list](#config-list)
  - [config edit](#config-edit)
  - [config path](#config-path)
  - [Exit codes](#exit-codes)
- [Validation and diagnostics](#validation-and-diagnostics)
- [Trust rules](#trust-rules)
- [Migration from ~/.amplihack/config](#migration-from-amplihackconfig)
  - [When migration runs](#when-migration-runs)
  - [Details](#details)
- [Where the effective config is reported](#where-the-effective-config-is-reported)
- [How files are written](#how-files-are-written)
- [Related](#related)

## Quick start

Choose your agent CLI once, then check what amplihack will use:

```sh
amplihack config set agent.binary claude
amplihack config set recipe.verbose true
amplihack config list --show-origin
```

```text
agent.binary = "claude"  # user (/home/alice/.amplihack/config.toml)
paths.home = (unset)  # default
node.heap_consent = true  # user (/home/alice/.amplihack/config.toml)
node.max_old_space_mb = (unset)  # default
recipe.verbose = true  # user (/home/alice/.amplihack/config.toml)
recipe.step_timeout = (unset)  # default
recipe.detach = "none"  # default
recipe.tmux_session_pattern = "amp-{recipe}-{repo}-{ts}"  # default
recipe.log_dir = (unset)  # default
recipe.usage_limit_wait = true  # default
recipe.usage_limit_max_wait_secs = (unset)  # default
orchestrator.max_depth = 3  # default
```

`amplihack agent-binary` now answers `claude (user_config)`, even inside a tmux
server that some other CLI started.

## Files

| Layer | Path | Written by |
|-------|------|------------|
| user | `$HOME/.amplihack/config.toml` | `amplihack config set`, `unset`, `edit` |
| project | `<dir>/.amplihack/config.toml` | `amplihack config set --project`, `unset --project`, `edit --project` |
| legacy | `$HOME/.amplihack/config` (JSON) | Only the migration, which renames it or removes its two `node_options_*` keys. See [Migration](#migration-from-amplihackconfig). |

- `HOME` comes from the environment. `AMPLIHACK_HOME` never moves the user file.
- **Finding the project file:** amplihack first finds the git root: the
  nearest directory, from the current directory upward, that holds a `.git`
  file (a worktree or submodule) or a `.git` directory containing `HEAD`. An
  empty `.git` directory does not count, as it does not for git. It then searches
  from the current directory up to that root, and uses the first
  `.amplihack/config.toml` it finds.
  - The search never goes above the git root. In a nested repository or a
    submodule, the outer repository's project file is not read.
  - Outside a git repository there is no project layer. A
    `.amplihack/config.toml` in a directory that is not inside a git
    repository is ignored, even in the current directory.
  - When the project file is the same file as the user file (you are in
    `$HOME` and `$HOME` is a git repository), it is read once, as the user
    layer.
  - When no project file exists yet, `--project` writes
    `<git root>/.amplihack/config.toml`.
- Both files are plain TOML. Keys are grouped into tables:

```toml
# ~/.amplihack/config.toml
[agent]
binary = "claude"

[node]
heap_consent = true

[recipe]
verbose = true
step_timeout = 3600

[orchestrator]
max_depth = 4
```

`amplihack config set agent.binary claude` and writing `[agent]` /
`binary = "claude"` by hand are equivalent.

## Precedence

For each key, the first layer that sets it wins:

| Rank | Origin label | Source |
|------|--------------|--------|
| 1 | `flag` | A command-line flag, for the keys that have one (`--verbose`, `--step-timeout`) |
| 2 | `env` | The key's environment variable, for the keys that have one |
| 3 | `project` | The project file |
| 4 | `user` | The user file |
| 5 | `legacy` | `~/.amplihack/config`, only while a migration could not complete |
| 6 | `default` | The built-in default, or `(unset)` when the key has none |

Two keys bend this order so that a cloned repository cannot raise your limits:
the project file can only lower `orchestrator.max_depth` and
`recipe.step_timeout`. See [Trust rules](#trust-rules).

`config get --show-origin`, `config list --show-origin` and `amplihack doctor`
print the labels from rank 2 down. `config` cannot see the flags of another
command, so it never prints `flag`.

## Keys

| Key | Type | Default | Env var | Flag | Files |
|-----|------|---------|---------|------|-------|
| `agent.binary` | `claude`, `copilot`, `codex` or `amplifier` | (unset) | `AMPLIHACK_AGENT_BINARY` | — | user only |
| `paths.home` | absolute path to an existing directory | (unset) | `AMPLIHACK_HOME` | — | user only |
| `node.heap_consent` | bool | (unset) | — | — | user only |
| `node.max_old_space_mb` | integer, 8192 to 32768 | (unset) | — | — | user only |
| `recipe.verbose` | bool | `false` | — | `--verbose` | user, project |
| `recipe.step_timeout` | integer seconds, at least 1 | (unset) | `AMPLIHACK_STEP_TIMEOUT` | `--step-timeout` | user, project (lower only) |
| `recipe.detach` | `tmux` or `none` | `none` | — | `--detach`, `--no-detach` | user, project |
| `recipe.tmux_session_pattern` | session-name pattern | `amp-{recipe}-{repo}-{ts}` | — | — | user, project |
| `recipe.log_dir` | absolute path | (unset: `~/.amplihack/recipe-logs`) | — | — | user only |
| `recipe.usage_limit_wait` | bool | `true` (`false` in CI) | `AMPLIHACK_USAGE_LIMIT_WAIT` | — | user, project |
| `recipe.usage_limit_max_wait_secs` | integer seconds, at least 0 | (unset) | `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | — | user, project (lower only) |
| `orchestrator.max_depth` | integer, 0 to 32 | `3` | `AMPLIHACK_MAX_DEPTH` | — | user, project (lower only) |
| `governor.enabled` | bool | `true` | — | — | user only |
| `governor.max_concurrency` | integer, 1 to 64 | `4` | — | — | user only |
| `governor.reserve_points` | integer, 0 to 100 | `10` | — | — | user only |
| `governor.step` | integer, 1 to 8 | `1` | — | — | user only |
| `governor.band_pct` | integer, 0 to 50 | `10` | — | — | user only |
| `governor.eval_interval_secs` | integer seconds, 1 to 3600 | `60` | — | — | user only |
| `governor.wait_secs` | integer seconds, 0 to 86400 | `600` | — | — | user only |

Any other key is an error for `config set` and a diagnostic when found in a
file. A user-only key in a project file is reported and ignored.

### agent.binary

The agent CLI that recipe steps, hooks and sub-agents call back into. A
configured value is an explicit choice of yours, so it is labelled
`user_config`. It is user-only: a cloned repository must not switch the CLI
your agent steps run under, so a project file's `agent.binary` is reported and
ignored, and `config set agent.binary --project` is a usage error. The
resolver order becomes:

| # | Source | Label |
|---|--------|-------|
| 1 | `AMPLIHACK_AGENT_BINARY` that you set: untagged, carrying a tag amplihack does not recognise, or carrying a `user_config:` tag for a *different* binary | `env` |
| 2 | The CLI this process actually runs in: a live session marker such as `CLAUDECODE` or `COPILOT_CLI`, or a launcher's `session:<cli>` tag (what `amplihack claude` exports) | `session_marker` |
| 3 | `agent.binary` from the user file, else an inherited `AMPLIHACK_AGENT_BINARY` tagged `user_config:<same binary>` | `user_config` |
| 4 | What a tmux server supplied: a `tmux_server:*`-tagged value, then a session marker the server's global environment also holds | `env`, `session_marker` |
| 5 | `.claude/runtime/launcher_context.json`, then the built-in default `copilot` (a `default:*` tag is ignored) | `launcher_context`, `default` |

Rank 3 sits below the session you are actually in and above anything a tmux
server handed down. Inside tmux, a session marker counts as rank 2 only when
the server's global environment does not hold it too (or, when amplihack
launched the session, does not hold the same `AMPLIHACK_SESSION_NONCE`, a
random value each `amplihack <cli>` launch exports). When the config answers
over a marker of another CLI, `recipe run` and `agent-binary` say so on
stderr. A tmux server started from a Copilot session holds `COPILOT_CLI=1`
and gives it to every pane, so in that server a run with
`agent.binary = "claude"` configured still uses claude.
Inside `amplihack claude`, or a Claude Code session, steps run under claude
whatever `agent.binary` says.

```console
$ amplihack config set agent.binary claude
set agent.binary = "claude" in /home/alice/.amplihack/config.toml
$ amplihack agent-binary            # in a pane of a server Copilot started
claude (user_config)
$ COPILOT_CLI=1 amplihack agent-binary   # a Copilot session, outside tmux
copilot (session_marker)
$ AMPLIHACK_AGENT_BINARY=codex amplihack agent-binary
codex (env)
```

When amplihack exports a `user_config` answer to a child, it tags it:

```sh
AMPLIHACK_AGENT_BINARY=claude
AMPLIHACK_AGENT_BINARY_SOURCE=user_config:claude
```

- The tag uses the same `kind:<binary>` form as `session:` and `default:`, so
  it binds to its value. If you later set `AMPLIHACK_AGENT_BINARY=codex` by
  hand next to a stale `user_config:claude` tag, the tag no longer describes
  the value and `codex` is treated as your explicit choice (rank 1).
- A bare `user_config` tag, with no binary, binds to whatever value is next to
  it.
- This process's own `agent.binary` beats an inherited `user_config:<binary>`
  value, so a `config set` takes effect even in the panes of a tmux server
  that captured the old value, and `config list --show-origin` shows what
  runs. With nothing configured, the inherited value answers.
- Tags are bookkeeping, not a security control. Every value, tagged or not, is
  checked against the allowlist before its tag is read.
- A `user_config` answer may be written to `launcher_context.json`, unlike a
  `default` guess.

`amplihack agent-binary --shell` renders the same tag. It does so even inside a
tmux server whose global environment holds another CLI's session marker. The configured value decided the answer,
not the marker, so the tag is `user_config:<binary>` and never
`tmux_server:<marker>`:

```console
$ amplihack agent-binary --shell
env -u CLAUDECODE -u CLAUDE_CODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PROJECT_DIR -u CLAUDE_CODE_ENTRYPOINT -u COPILOT_CLI -u GITHUB_COPILOT -u GITHUB_COPILOT_AGENT -u COPILOT_AGENT AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=user_config:claude
```

The full resolver is described in [Active Agent Binary](./active-agent-binary.md).

### paths.home

Where amplihack loads its framework assets, hooks and runtime state from. The
resolution order becomes:

1. `AMPLIHACK_HOME` from the environment
2. `paths.home` from the user file
3. The bundle root found by walking up from the working directory (a checkout
   or worktree with an `amplifier-bundle/` directory)
4. `$HOME/.amplihack`
5. The executable's directory

`config set` accepts only an absolute path, without `..` components, control
characters or NUL, at most 4096 bytes, naming an existing directory that you
own. If the directory is later removed or changes owner, the value is skipped
with a logged warning and the next step answers.

`paths.home` is user-only. A project file cannot set it, because the home
decides which hooks run.

```sh
amplihack config set paths.home /opt/amplihack
```

### node.heap_consent and node.max_old_space_mb

The launcher asks once whether it may set `NODE_OPTIONS=--max-old-space-size=<mb>`
for the agent CLIs it starts. Your answer is saved as `node.heap_consent` in
the user file. The launcher reports the saved choice on every launch:

```text
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: amplihack config set node.heap_consent <true|false>
```

- `node.heap_consent = true` applies the limit without asking.
- `node.heap_consent = false` skips it without asking.
- Unset, the launcher asks, as before.
- `node.max_old_space_mb` overrides the limit. Unset, the limit is computed
  from the machine's RAM on every launch, as before. Values outside 8192 to
  32768 are rejected.

Both keys are user-only.

### recipe.verbose

Turns on `amplihack recipe run --verbose` by default. The flag and the config
value are combined with OR: `--verbose` always turns it on, and there is no
flag to turn a configured `true` off for one run. Use
`amplihack config unset recipe.verbose` instead.

### recipe.step_timeout

A per-step timeout ceiling, in seconds, for every step of `recipe run`. The
first of these wins:

1. `--step-timeout <SECONDS>`. It is exported to the runner as
   `AMPLIHACK_STEP_TIMEOUT`.
2. `AMPLIHACK_STEP_TIMEOUT` already in the environment. It is passed on
   unchanged, and its origin is `env`.
3. `recipe.step_timeout` from the config files. It is exported as
   `AMPLIHACK_STEP_TIMEOUT`.

Unset everywhere, the runner's own default applies (agent steps have no
timeout). The config value must be at least 1; to disable timeouts for one
run, pass `--step-timeout 0`.

A project file can only lower this value below the user file's. When the user
file does not set it, the project value applies.

### recipe.detach

`recipe.detach = "tmux"` makes every `amplihack recipe run` start detached, as
`--detach` does: in a new tmux session with a private log. `--no-detach` runs
one in the foreground anyway. See
[Detached runs](./recipe-command.md#detached-runs).

### recipe.tmux_session_pattern

The name of the tmux session a detached run creates. `{recipe}` is the
recipe's file stem or name, `{repo}` the directory name of the git root above
the working directory, and `{ts}` the UTC time as `YYYYMMDD-HHMMSS`. The
pattern is 1 to 64 characters of letters, digits, `-` and `_` besides those
placeholders, so a session name never needs quoting.

### recipe.log_dir

Where detached runs write their logs, as `<session>.log`. Unset, it is
`~/.amplihack/recipe-logs`. The directory is created with mode 0700, and an
existing one must be yours; each log is created with mode 0600. User-only,
like every path.

### recipe.usage_limit_wait

Whether a recipe agent step that fails on a Claude usage limit waits for the
reset and runs once more, instead of ending the run. See the
[usage-limit wait](./profile-command.md#usage-limit-wait). With no setting
in the environment or a file, it is on, except in CI: any `CI` that is
not, after trimming, empty, `0` or `false` in any case. Setting it in a file or the variable turns it on
in CI too. The environment variable also accepts `1` and `0`.

`recipe.usage_limit_max_wait_secs` caps one wait; it only shortens it, and
`0` means one more attempt with no wait.

```sh
amplihack config set recipe.usage_limit_wait false
amplihack config set recipe.usage_limit_max_wait_secs 3600
```

### orchestrator.max_depth

The session-tree recursion ceiling for a new root session, replacing the
built-in default of 3. `AMPLIHACK_MAX_DEPTH` still wins over the config.

A tree that is already running keeps its sealed ceiling. Neither the
environment nor the config can raise it: a child session in a tree sealed at 2
stays at 2 even with `orchestrator.max_depth = 10` configured. See
[Session Tree Recursion Control](./session-tree-recursion-control.md).

A project file can only lower this value: the result is the smaller of the
project value and the user value (or the default, 3).

### Governor keys

Settings for the [capacity governor](./capacity-governor.md), which decides
when `recipe run`, `claude` and `launch` may start.

| Key | Effect |
|-----|--------|
| `governor.enabled` | Turns the governor on or off. `AMPLIHACK_GOVERNOR` (`off`, `0`, `false`, `on`, `1`, `true`) outranks it. |
| `governor.max_concurrency` | Highest concurrency limit; the limit never goes below 1. |
| `governor.reserve_points` | Weekly points kept for coordination, recovery and review work. |
| `governor.step` | How far one rate evaluation moves the limit. |
| `governor.band_pct` | Width of the band around the sustainable rate in which the limit stays put. |
| `governor.eval_interval_secs` | Shortest time between two rate evaluations. |
| `governor.wait_secs` | Default longest wait for admission; `0` means do not wait. |

All seven are user-only, so a project file cannot turn the governor off or
loosen it.

The Keys table lists no environment variable or flag for these keys. The
governor reads `AMPLIHACK_GOVERNOR`, `AMPLIHACK_GOVERNOR_WAIT` and
`--governor-wait` itself, because they accept values the config file does not
(`off`, `15m`). They outrank `governor.enabled` and `governor.wait_secs`, but
`config get --show-origin` does not see them and reports the file or default
the key came from.

```sh
amplihack config set governor.max_concurrency 2
amplihack config set governor.wait_secs 1800
```

## The config command

```
amplihack config get <KEY> [--show-origin]
amplihack config set <KEY> <VALUE> [--project]
amplihack config unset <KEY> [--project]
amplihack config list [--show-origin]
amplihack config edit [--project]
amplihack config path
```

`set`, `unset` and `edit` act on the user file unless you pass `--project`.
`--project` outside a git repository is a usage error.

Every subcommand, including the read-only `get`, `list` and `path`, loads the
config, and loading the config runs the one-time
[migration](#migration-from-amplihackconfig) when a legacy file is waiting.

stdout carries only the data described below. Warnings and errors go to
stderr.

### config get

Prints the effective value of one key, as a bare value with no quotes. An
unset key prints an empty line and exits 0. An unknown key is a usage error.

```console
$ amplihack config get agent.binary
claude
$ amplihack config get agent.binary --show-origin
claude  # user (/home/alice/.amplihack/config.toml)
$ amplihack config get recipe.step_timeout

```

### config set

Validates the value, writes it, and prints what it wrote and where.

```console
$ amplihack config set orchestrator.max_depth 2 --project
set orchestrator.max_depth = 2 in /home/alice/src/myapp/.amplihack/config.toml
$ amplihack config set orchestrator.max_depth 99
error: orchestrator.max_depth: must be an integer from 0 to 32
$ echo $?
2
$ amplihack config set paths.home /opt/amplihack --project
error: paths.home: can only be set in the user file
```

`set` refuses to write a file that has a TOML syntax error, and exits 1. Fix
it with `amplihack config edit` first. Comments and formatting in the file are
kept.

Bools accept `true` and `false`. Integers are decimal.

### config unset

Removes a key from the file. Removing a key that is not set is a no-op that
exits 0.

```console
$ amplihack config unset recipe.verbose
unset recipe.verbose in /home/alice/.amplihack/config.toml
$ amplihack config unset recipe.verbose
recipe.verbose is not set in /home/alice/.amplihack/config.toml
```

### config list

Prints every key, in the order of the [Keys](#keys) table, one per line:

```text
<key> = <value>
```

Values are written in TOML syntax: strings quoted, bools and integers bare.
A key with no value prints `(unset)`. With `--show-origin`, each line gains two
spaces and `# <origin>`:

| Origin | Printed as |
|--------|------------|
| environment | `# env (AMPLIHACK_MAX_DEPTH)` |
| environment, invalid value | `# env (invalid: AMPLIHACK_MAX_DEPTH is not an integer)` |
| a file | `# project (<path>)`, `# user (<path>)`, `# legacy (<path>)` |
| built-in | `# default` |

When any file has a problem, a `Config problems:` block follows the rows. See
[Validation and diagnostics](#validation-and-diagnostics). `list` exits 0 even
when it prints problems.

```console
$ AMPLIHACK_MAX_DEPTH=5 amplihack config list --show-origin
agent.binary = "claude"  # user (/home/alice/.amplihack/config.toml)
paths.home = (unset)  # default
node.heap_consent = true  # user (/home/alice/.amplihack/config.toml)
node.max_old_space_mb = (unset)  # default
recipe.verbose = false  # default
recipe.step_timeout = 1800  # project (/home/alice/src/myapp/.amplihack/config.toml)
recipe.detach = "none"  # default
recipe.tmux_session_pattern = "amp-{recipe}-{repo}-{ts}"  # default
recipe.log_dir = (unset)  # default
orchestrator.max_depth = 5  # env (AMPLIHACK_MAX_DEPTH)

Config problems:
  /home/alice/src/myapp/.amplihack/config.toml: paths.home: can only be set in the user file; ignored
```

An invalid environment value is shown with its origin, but amplihack keeps its
existing handling of that variable: each consumer parses its own variable, as
before.

### config edit

Opens the file in `$VISUAL`, then `$EDITOR`, then `vi`. The editor is never
read from a config file. The path is passed as a separate argument, so a path
with spaces or shell metacharacters is safe.

- The parent directory is created (mode 0700) if it does not exist.
- After the editor exits, the file is validated. Any problems are printed and
  the command exits 1. The file is kept as you wrote it.

```sh
amplihack config edit --project
```

### config path

Prints the files amplihack reads:

```console
$ amplihack config path
user: /home/alice/.amplihack/config.toml
project: /home/alice/src/myapp/.amplihack/config.toml
legacy: /home/alice/.amplihack/config
```

- `project:` is the file the search found, or the file `--project` would
  create. Outside a git repository it reads
  `project: (none: not in a git repository)`.
- The `legacy:` line appears only while `~/.amplihack/config` still holds
  `node_options_consent` or `node_options_limit_mb`, that is, while a
  migration is waiting or could not complete. A legacy file that holds only
  other settings, such as `git_identity`, is not listed.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. Also: `get` of an unset key, `unset` of an absent key, and `list` with problems. |
| `1` | Runtime or state error: an unreadable or unwritable file, `set` on a file with a syntax error, `edit` that leaves problems. |
| `2` | Usage error: an unknown key, an invalid value, a user-only key with `--project`, `--project` outside a git repository. |

## Validation and diagnostics

`config set` and `config unset` validate before writing. A hand-edited file is
checked every time it is read:

- **An unknown key, a bad value, or a key in the wrong file** is reported and
  that key falls back to the lower layers. The rest of the file still applies.
- **A TOML syntax error** makes amplihack ignore the whole file. The report
  gives the line and column, never the file's contents.
- **A file that is not a regular file, or is larger than 64 KiB,** is ignored
  and reported. A FIFO or `/dev/zero` cannot hang a command.

Every command that reads the config prints each problem once per process, on
stderr, and keeps running:

```text
warning: /home/alice/.amplihack/config.toml: orchestrator.max_depth: must be an integer from 0 to 32
warning: /home/alice/src/myapp/.amplihack/config.toml: TOML syntax error at line 4, column 9; file ignored
```

`config list` and `amplihack doctor` print the same problems in a
`Config problems:` block, one per line, indented by two spaces, as
`<path>: <key>: <reason>`, or `<path>: <reason>` for a whole-file problem.

Keys, values and paths taken from a file are escaped before they are printed:
control characters, DEL and bidirectional overrides are shown as escapes, and
anything longer than 200 characters is truncated.

## Trust rules

The user file is yours. The project file arrives with a repository you may
have just cloned, so it is trusted less.

- **The user file must be owned by you and not writable by group or others.**
  If it is not, its user-only keys (`agent.binary`, `paths.home`, `node.*`,
  `recipe.log_dir`, `governor.*`) are ignored and reported. Its other keys still apply.
- **The project file never names a path, a command, an executable or the
  agent CLI.** `agent.binary` and every path-valued key are user-only.
- **The project file cannot change the capacity governor.** Every
  `governor.*` key is user-only.
- **`config set --project` and `config edit --project` refuse a project
  `config.toml` that is a symlink, or whose directory resolves outside the git
  repository**, and exit 1.
- **The project file can only lower `orchestrator.max_depth` and
  `recipe.step_timeout`.** The result is the smaller of the project value and
  the user value (or the default). A higher project value is ignored and
  reported:

  ```text
  warning: /home/alice/src/myapp/.amplihack/config.toml: orchestrator.max_depth: 8 is above the user value 3; a project file can only lower it
  ```

- **Environment variables still win.** A project file never overrides a
  variable you exported.

## Migration from ~/.amplihack/config

Earlier versions saved the NODE_OPTIONS consent in `~/.amplihack/config` as
JSON (`node_options_consent`, `node_options_limit_mb`). Every amplihack
process that loads the config checks that file. When it holds either key, the
process migrates it:

1. It parses the JSON. A file that is not a JSON object, or has values of the
   wrong type, is reported as a config problem and left untouched. It never
   causes a crash.
2. It writes `node_options_consent` to `node.heap_consent` in
   `~/.amplihack/config.toml`, unless that file already sets
   `node.heap_consent`. If the user file already sets it, that value is kept
   and the legacy value is discarded.
3. It removes the two keys from the legacy file:
   - If the file holds nothing but `node_options_consent` and
     `node_options_limit_mb`, it is renamed to
     `~/.amplihack/config.json.migrated`.
   - Otherwise it is rewritten in place without those two keys. Every other
     key, such as the `git_identity` block, keeps its value. Whitespace and
     key order may change. The rewrite is atomic, like every other config
     write (see [How files are written](#how-files-are-written)).

Step 3 runs only after step 2 has succeeded, so the consent always exists in
at least one file. After step 3 the legacy file holds nothing amplihack reads,
so the user file is the only place the consent lives.

### When migration runs

- The migration runs on any config load. Read-only commands count: `config
  get`, `config list`, `config path`, `amplihack doctor` and every launch.
  The first of them after an upgrade may create `~/.amplihack/config.toml` and
  rename or rewrite `~/.amplihack/config`. Later loads find no legacy keys
  and write nothing.
- A legacy file with neither key, for example one that holds only
  `git_identity`, is never written and is not a config layer.

### Details

- `node_options_limit_mb` is **not** copied to `node.max_old_space_mb`. It
  only recorded the recommendation at the time it was saved; the limit is
  still computed from RAM on each launch, exactly as before. Set
  `node.max_old_space_mb` yourself to pin a limit.
- `amplifier-bundle/tools/git-identity.sh` reads a `git_identity` block from
  JSON files. It checks `$AMPLIHACK_CONFIG`, `$HOME/.amplihack/config`,
  `$AMPLIHACK_HOME/config` and `$AMPLIHACK_HOME/.amplihack/config`. Only
  `$HOME/.amplihack/config` is migrated, and the migration keeps its
  `git_identity` block where the script looks for it. amplihack never reads or
  writes the other three paths.
- If step 2 or step 3 fails, for example because `$HOME` is read-only, the
  remaining legacy keys are still read as the `legacy` layer. The migration
  is retried on the next load and a config problem is printed until it
  succeeds. Your consent is never lost:

  ```console
  $ amplihack config get node.heap_consent --show-origin
  true  # legacy (/home/alice/.amplihack/config)
  ```

- `config set node.heap_consent` and `config unset node.heap_consent` run
  the migration first. If the legacy keys cannot be removed, the command
  exits 1 without changing anything. Otherwise the legacy layer would serve
  the old consent again after an `unset`, or migrate it back on the next load.
- Apart from the migration, nothing writes the JSON file.

## Where the effective config is reported

- **`amplihack doctor`** prints an `Effective config` section with the same
  rows as `config list --show-origin`, followed by any `Config problems:`.
  The section is informational: it never makes `doctor` fail. See
  [amplihack doctor](./doctor-command.md).
- **`amplihack recipe run`** adds `agent_binary`, `agent_binary_source` and
  `amplihack_home` to both its `early` and `final`
  `amplihack.recipe.log_pointer` events, so a run's log records which CLI it
  used and why:

  ```text
  amplihack.recipe.log_pointer {"schema_version":1,"event":"early","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","cwd":"/home/alice/src/myapp","worktree":"/home/alice/src/myapp","branch":"main","runner_path":"/home/alice/.local/bin/recipe-runner-rs","agent_binary":"claude","agent_binary_source":"user_config","amplihack_home":"/home/alice/.amplihack","timestamp":"2026-06-12T03:55:11Z"}
  ```

  See [Recipe Run Correlation](./recipe-run-correlation.md#pointer-event-schema).

## How files are written

- Writes are atomic: a temporary file with a random name in the same
  directory, set to mode 0600 before anything is written, synced, then
  renamed over the target. A reader never sees a half-written file.
- The user file is created with mode 0600. A new `~/.amplihack` is created
  with mode 0700; an existing one that you own loses group and other access.
  Your own permission bits are never widened.
- An existing project `.amplihack/` directory is never chmodded.
- If the target is a symlink, amplihack writes the file it points to and keeps
  the symlink. A dotfiles setup that links `~/.amplihack/config.toml` into a
  repository keeps working.

## Related

- [AMPLIHACK_* Environment Variable Audit](./amplihack-env-vars.md) — Every `AMPLIHACK_*` name in the code, classified, with its config key where one exists
- [Environment Variables](./environment-variables.md) — What each variable does
- [Active Agent Binary](./active-agent-binary.md) — The full agent-binary resolver, including the `user_config` layer
- [amplihack doctor](./doctor-command.md) — The `Effective config` section
- [Capacity Governor](./capacity-governor.md): What the `governor.*` keys control
- [Memory Configuration Consent](../features/memory-consent-prompt.md) — The NODE_OPTIONS prompt whose answer `node.heap_consent` stores
