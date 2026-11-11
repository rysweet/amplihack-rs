# Hook Specifications Reference

amplihack registers 8 hooks in `~/.claude/settings.json` when you run `amplihack install`. This page documents each hook: its Claude Code event, the binary or script that runs, and its timeout.

## Canonical Hook Table

| # | Claude Code Event | Command Kind | Command | Timeout |
|---|------------------|--------------|---------|---------|
| 1 | `SessionStart` | Binary subcommand | `amplihack-hooks session-start` | 10 s |
| 2 | `Stop` | Binary subcommand | `amplihack-hooks stop` | 120 s |
| 3 | `PreToolUse` | Binary subcommand | `amplihack-hooks pre-tool-use` | *(none)* |
| 4 | `PostToolUse` | Binary subcommand | `amplihack-hooks post-tool-use` | *(none)* |
| 5 | `UserPromptSubmit` | Binary subcommand | `amplihack-hooks workflow-classification-reminder` | 5 s |
| 6 | `UserPromptSubmit` | Binary subcommand | `amplihack-hooks user-prompt-submit` | 10 s |
| 7 | `PreCompact` | Binary subcommand | `amplihack-hooks pre-compact` | 30 s |
| 8 | `StopFailure` | Binary subcommand, matcher `rate_limit`, `asyncRewake` | `amplihack-hooks limit-resume` | 608400 s |

## Hook Format in settings.json

Each hook is written as a wrapper object under the event key:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks session-start",
            "timeout": 10
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks pre-tool-use"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks workflow-classification-reminder",
            "timeout": 5
          }
        ]
      },
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks user-prompt-submit",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

The `StopFailure` entry is the only one with a matcher other than `"*"` and
the only one with `asyncRewake`:

```json
{
  "hooks": {
    "StopFailure": [
      {
        "matcher": "rate_limit",
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks limit-resume",
            "timeout": 608400,
            "asyncRewake": true
          }
        ]
      }
    ]
  }
}
```

**Key points:**

- `PreToolUse` and `PostToolUse` have no `timeout` field. Absence means fail-open (no timeout enforced by Claude Code).
- Both `PreToolUse` and `PostToolUse` use matcher `"*"` to intercept all tool calls.
- The two `UserPromptSubmit` entries must appear in order: `workflow-classification-reminder` first, then `user-prompt-submit`. The installer preserves this order on both fresh install and idempotent update.
- `asyncRewake: true` is written only on the `StopFailure` entry. Claude Code
  runs that hook in the background, and an exit status of `2` is expected to
  start a new turn with the hook's stderr shown to the model; this was
  verified live on Claude Code 2.1.306 and 2.1.307. Every other wrapper is
  written exactly as before. When the installer checks whether an existing
  wrapper is amplihack's, a missing `asyncRewake` counts as `false`, so a
  `StopFailure` wrapper without it is replaced.
- The Claude Code plugin manifest `claude-plugin/hooks.json` registers the same
  `StopFailure` entry through `amplihack-hook limit-resume`.
- Hook commands are written with the **absolute path** to `amplihack-hooks` resolved at install time. If the binary moves, re-run `amplihack install` to update the paths.

## Hook Input JSON Contract

Hook binaries read one JSON object from stdin and deserialize it into the shared
`HookInput` type. The compatibility contract is host-agnostic: Claude Code,
Copilot wrappers, Amplifier, and tests can send the same semantic payload.

The contract has two layers:

- **Typed input boundary:** known events deserialize with their required fields;
  malformed known-event payloads are invalid.
- **Hook executable boundary:** hooks keep their existing fail-open policy for
  processing errors and panics, but do not convert malformed known-event JSON
  into `Unknown`. `Unknown` is reserved for valid payloads whose event name is
  not recognized yet.

### Event discriminator

The canonical discriminator is `hook_event_name`. The deserializer also accepts
the known camelCase alias `hookEventName`.

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {
    "command": "pwd"
  }
}
```

Equivalent camelCase input:

```json
{
  "hookEventName": "PreToolUse",
  "toolName": "Bash",
  "toolInput": {
    "command": "pwd"
  }
}
```

### Field aliases

| Canonical field | Accepted alias | Used by |
|-----------------|----------------|---------|
| `hook_event_name` | `hookEventName` | All hook events |
| `tool_name` | `toolName` | `PreToolUse`, `PostToolUse` |
| `tool_input` | `toolInput` | `PreToolUse`, `PostToolUse` |
| `tool_result` | `toolResult` | `PostToolUse` |
| `session_id` | `sessionId` | All events with session context |
| `stop_hook_active` | `stopHookActive` | `Stop`, `StopFailure` |
| `transcript_path` | `transcriptPath` | `Stop`, `SessionStop`, `PreCompact` |
| `user_prompt` | `userPrompt` | `UserPromptSubmit` |
| `error` | — | `StopFailure` |
| `error_details` | `errorDetails` | `StopFailure` |
| `last_assistant_message` | `lastAssistantMessage` | `StopFailure` |

Unknown additional fields are ignored unless a variant explicitly captures them
as extra event data.

### Required and optional fields

Missing optional fields deserialize to `None` / absent values. Required semantic
fields remain strict at both the typed boundary and the hook executable entry.

| Event | Required fields | Optional fields |
|-------|-----------------|-----------------|
| `PreToolUse` | `tool_name`, `tool_input` | `session_id` |
| `PostToolUse` | `tool_name`, `tool_input` | `tool_result`, `session_id` |
| `Stop` | none beyond event name | `stop_hook_active`, `transcript_path`, `session_id` |
| `SessionStart` | none beyond event name | `session_id`, `cwd`, extra fields |
| `SessionStop` | none beyond event name | `session_id`, `transcript_path`, extra fields |
| `UserPromptSubmit` | none beyond event name | `user_prompt`, `session_id`, extra fields |
| `PreCompact` | none beyond event name | `session_id`, `transcript_path`, extra fields |
| `StopFailure` | none beyond event name | `error`, `error_details`, `last_assistant_message`, `stop_hook_active`, `session_id`, `transcript_path`, extra fields |

Malformed JSON, a missing event discriminator, or a missing required field for a
tool event is invalid input. The compatibility layer is intentionally narrow: it
accepts known host schema drift without treating incomplete tool payloads as
valid or silently mapping them to `Unknown`.

A `StopFailure` payload is read loosely, so a change in Claude Code's payload
cannot turn the hook off: a field with an unexpected type, such as an `error`
that is not a string, is read as absent and the rest of the payload is kept.
The exception is the loop guard: a `stop_hook_active` that is present but not
a boolean (for example `"true"`) is read as `true`, so the hook stops instead
of skipping the guard. `null` is read as absent.

Example `StopFailure` input from Claude Code:

```json
{
  "hook_event_name": "StopFailure",
  "session_id": "0f6d3c2e-7a41-4b8e-9c1d-5e2f8a9b0c34",
  "transcript_path": "/home/alice/.claude/projects/-home-alice-src-app/0f6d3c2e-7a41-4b8e-9c1d-5e2f8a9b0c34.jsonl",
  "cwd": "/home/alice/src/app",
  "error": "rate_limit",
  "error_details": "429",
  "last_assistant_message": "You've hit your session limit · resets 3pm (Europe/London)"
}
```

### Unknown events

Future hook events with a valid event discriminator deserialize to `Unknown`
instead of failing. This makes the hook boundary forward-compatible while
keeping known event payloads typed and strict. Malformed known events are not
future events and must not use this path.

```json
{
  "hook_event_name": "FutureEvent",
  "payload": {
    "anything": true
  }
}
```

## Hook Descriptions

### 1. SessionStart — `session-start`

Runs when Claude Code starts a new session. Initializes session context, checks for version mismatches natively in Rust, and migrates stale global amplihack hook registrations out of `~/.claude/settings.json`.

When no Claude account has capacity left, it also shows a [capacity notice](#capacity-notice-in-hooks).

### 2. Stop — `stop`

Runs when Claude Code stops. Triggers native Rust reflection prompt assembly plus Claude CLI analysis, saves session memory, and handles lock-mode signaling. 120-second timeout allows the reflection write to complete.

### 3. PreToolUse — `pre-tool-use`

Runs before every tool call (matcher `*`). Validates Bash commands against the amplihack allow-list and records metrics. No timeout — fail-open so tool calls are never blocked by a slow hook.

The same handler runs for Copilot (`preToolUse`) and Codex (`PreToolUse`, command `pre-tool-use`), so every check below applies on all three hosts.

#### tmux server guard

The tmux server guard denies a shell command that would kill a whole tmux
server, or every session on it, without naming a private socket. It always
runs: no environment variable or setting turns it off, and it reads no
environment variables.

**Why it exists.** When `$TMUX` is set, as it is inside any tmux pane, tmux
uses the socket named in `$TMUX` whenever no `-S` or `-L` is given, and ignores
`TMUX_TMPDIR`. An agent running in a pane that runs
`TMUX_TMPDIR=$SB/tmuxsock tmux kill-server` therefore kills the user's own
server and every session on it (#1733).

**Tools it checks.** Tool names `bash`, `shell`, `shell_command`,
`exec_command`, `powershell` and `monitor`, compared case-insensitively.
Claude Code's `Monitor` tool takes a `command` that is a shell command or
script, run in the same shell environment as `Bash`, so it is checked like
`Bash`; a `Monitor` call that watches a WebSocket (`ws`) has no `command` and
is not. In Claude Code 2.1.314 the tools that run code and take a `command`
are `Bash`, `PowerShell` and `Monitor`; the list in `SHELL_TOOLS` needs
checking against each new release, since a shell tool missing from it skips
the guard. It checks both `tool_input.command` and `tool_input.cmd`, and
either one can deny, so a harmless `command` does not hide a `cmd` that kills
the user's server.

**Position.** The guard is the first check `PreToolUseHook::process` makes for
a shell tool: it runs before the XPIA check, the Skill redirect, the merge
guard and the non-Bash early return. The checks after it answer with a bare
`{"block": true, "message": …}`, which Claude Code does not act on (#1773).
XPIA is off by default (#1734). When a user opts in with `XPIA_ENABLED=true`,
its command-injection pattern matches a separator or newline followed by `rm`,
`kill` and similar words. If XPIA ran first, it would answer for `TMUX_TMPDIR=$SB/tmuxsock tmux
kill-server; rm -rf "$SB"`, for a `tmux -C attach <<EOF` heredoc whose body is
`kill-server`, and for `tmux new -d 'sleep 1' \; kill-server`, and on Claude
Code the command would run. Running the guard first gives those commands the
deny shown under **Response** below. The guard only ever denies, so running
it first cannot let through a command that a later check refuses: a command
the guard allows goes on to XPIA and the other checks unchanged.

On Codex, `normalize_codex_input` in `protocol/codex.rs` has already renamed
the `shell`, `exec_command` and `shell_command` tools to `Bash`, and copied
`cmd` into `command`, before the handler runs, so a Codex call reaches the
guard as `Bash` with `tool_input.command` set. The tool-name list matters for
names that arrive unchanged, such as Copilot's `bash`. The normalisation does
not change the value's type: an array of strings (the Codex argv form, such as
`["bash","-lc","…"]`) is shell-quoted and joined by the guard before it is
checked. An array with a non-string element has its string elements joined and
is given the text test described under [Uncertain input](#uncertain-input).

**What it denies.** A tmux invocation that runs, anywhere in its command
sequence (split on `;` or `\;`):

| Subcommand | Denied when |
|---|---|
| `kill-server` | The word starts with `kill-s` and is a prefix of `kill-server`: `kill-s`, `kill-se`, `kill-ser`, `kill-server`. |
| `kill-session` | The word is a prefix of `kill-session` (`kill-ses`, `kill-sess`, …) and `-a` is set. Flags are read getopt-style with spec `aCt:`, so `-a`, `-aC` and `-at x` set `-a`; `-ta` and `-t a` name target `a` and do not. An unknown flag denies. |

`kill-session -t`, `kill-window` and `kill-pane` are not guarded.

Several tmux commands take another tmux command as an argument and run it at
once or later: `if-shell`, `run-shell -C`, `confirm-before`, `bind-key`,
`set-hook`, `display-menu`, `command-prompt` and `command-alias`, among
others. So in an unscoped invocation, any argument after the first
subcommand word that contains a kill command also counts as a kill. Each
argument is split on whitespace, `;` and `=` first, so
`tmux if-shell true kill-server`, `tmux if-shell -b true "kill-session -a"`
and `tmux set -g command-alias[100] zz=kill-server` are denied. Only a word
that is `kill-server` or one of its prefixes, or `kill-session` with `-a`
after it, counts: `tmux new -s kill-switch` and
`tmux kill-session -t foo` are allowed. An unscoped
`tmux send-keys 'tmux kill-server'`, which only types the text, is denied as
well.

**What counts as scoped.** The invocation is allowed only when `-S <path>` or
`-L <name>` appears among the global options, which are the words before the
first subcommand:

- Flags without a value are `-2CDlNuVv`, as in tmux's own usage line, and may
  be grouped. `-d` and `-h` are not tmux global options, so they end the
  global options as unknown options do.
- `-c`, `-f`, `-L`, `-S` and `-T` take a value, as a separate word or joined
  (`-S/path`, `-Lname`).
- `--` ends the global options. An unknown option ends them and makes the
  invocation unscoped.

These still deny, because they name the user's own server:

- `-L default`, or an empty or missing `-L` or `-S` value.
- An `-S` value that refers to `TMUX`, matches `/tmp/tmux-*/default`, or has
  `default` as its final component, ignoring a trailing `/` or `}` (so
  `"${S:-/tmp/tmux-1000/default}"` is the default socket). A value refers to
  `TMUX` when it contains `$TMUX` or `${TMUX` not followed by a letter, digit
  or `_`, as in `${TMUX}`, `${TMUX%%,*}` or `${TMUX:-x}`. `$TMUX_TMPDIR` and
  `${TMUX_TMPDIR}` do not refer to `TMUX`.
- An `-S` or `-L` value that refers, in the same way, to a variable assigned
  anywhere in the command a value that this list denies, directly or through
  another such variable. That covers a value taken from `TMUX` or from tmux's
  output, a value naming the default socket, and an empty value:
  `S=$TMUX; tmux -S "$S" kill-server`,
  `s=${TMUX%%,*}; tmux -S "$s" kill-server`,
  `S=/tmp/tmux-$(id -u)/default; tmux -S "$S" kill-server`,
  `SOCK="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/default"; tmux -S "$SOCK" kill-server`
  and `L=default; tmux -L "$L" kill-server` are denied. A value that refers
  to such a variable anywhere is denied, so
  `D=/srv/default; tmux -S "$D/qa.sock" kill-server` is denied too.
- An `-S` or `-L` value that holds a `$( )` or backtick substitution running
  tmux, or refers to a variable assigned one. Inside a pane,
  `tmux display -p '#{socket_path}'` prints the user's own socket, so
  `tmux -S "$(tmux display -p '#{socket_path}')" kill-server` and
  `sock=$(tmux display -p '#{socket_path}'); env -u TMUX tmux -S "$sock" kill-server`
  are denied. A substitution runs tmux when its body has a `tmux` word, such
  as `tmux`, `/usr/bin/tmux`, `command tmux` or `${T:-tmux}`, read the three
  ways described under layer 3 below. A sandbox path that only
  contains the letters, as in `SB=$(mktemp -d /tmp/tmux-qa.XXXXXX)`, is
  still a private socket.
- A `TMUX=…` prefix or `env TMUX=…`. Setting `TMUX` is never scope.

A socket file named `default` is denied even under a sandbox directory, such
as `tmux -S "$SB/tmuxsock/tmux-1000/default" kill-server` for the server that
`TMUX_TMPDIR=$SB/tmuxsock` starts, because the text alone cannot tell it from
the user's own socket. The deny message says so and gives the way out: give a
private server another name (`-S "$SB/tmux.sock"`), and end a server already
running on a `default` path with `tmux -S <path> kill-session -t <name>` for
each of its sessions. `kill-session -t` is not guarded, and with the default
`exit-empty` option a tmux server exits when its last session ends.

| Command | Result |
|---|---|
| `tmux kill-server` | denied |
| `TMUX_TMPDIR=$SB/tmuxsock tmux kill-server` | denied |
| `FOO=1 BAR=2 tmux kill-server` | denied |
| `env FOO=1 tmux kill-server` | denied |
| `env -u TMUX tmux kill-server` | denied |
| `/usr/bin/tmux kill-server` | denied |
| `TMUX=/tmp/x,1,0 tmux kill-server` | denied |
| `tmux kill-session -a` | denied |
| `tmux kill-session -a -t foo` | denied |
| `tmux kill-ses -a` | denied |
| `tmux -L default kill-server` | denied |
| `tmux -S "$TMUX" kill-server` | denied |
| `tmux -S "$(tmux display -p '#{socket_path}')" kill-server` | denied |
| `S=/tmp/tmux-$(id -u)/default; tmux -S "$S" kill-server` | denied |
| `L=default; tmux -L "$L" kill-server` | denied |
| `tmux -S "$SB/tmuxsock/tmux-1000/default" kill-server` | denied |
| `tmux new -d 'sleep 1' \; kill-server` | denied |
| `tmux -L x kill-server` | allowed |
| `tmux -S /p kill-server` | allowed |
| `tmux -L x kill-session -a` | allowed |
| `tmux -S /p kill-session -a` | allowed |
| `env -u TMUX tmux -S /tmp/x/sock kill-server` | allowed |
| `env -u TMUX tmux -S "$SB/tmux.sock" kill-server` | allowed |
| `tmux -S "$(mktemp -d)/s" kill-server` | allowed |
| `tmux -L qa-1733 kill-server` | allowed |
| `tmux kill-session -t foo` | allowed |
| `tmux -S "$SB/tmuxsock/tmux-1000/default" kill-session -t qa` | allowed |
| `tmux ls` | allowed |

**Where tmux is found.** Commands are split by the shared shell tokeniser in
`pre_tool_use/shell.rs` on `;`, `&&`, `||`, `|`, `&` and newlines, after line
continuations are joined. Three layers are checked:

1. **Structural.** Every simple command whose program basename is `tmux`
   (case-insensitive). Assignments, shell reserved words such as `if`, `then`
   and `{`, a leading backslash (`\tmux`) and the wrappers `env`, `command`,
   `exec`, `nice`, `nohup`, `timeout`, `time` and `sudo`, with their options,
   are seen through to find the program word. A program word that is an
   expansion naming tmux, such as `"$(command -v tmux)"` or
   `${TMUX_BIN:-tmux}`, is read as tmux too, so `"$(command -v tmux)" $c` is
   denied like `tmux $c`. Any program word that is an expansion also gets
   the stricter rule under [Uncertain input](#uncertain-input).
2. **Embedded tmux word.** In any other command, a later word with basename
   `tmux` starts a tmux invocation, as in `xargs tmux …`, `ssh host tmux …`,
   `setsid tmux …`, `watch tmux …` or `stdbuf -oL tmux …`.
3. **Embedded script.** A word that contains both a `tmux` token and a `kill-s`
   token, such as the argument of `bash -c`, `python -c`, `$( )`, `<( )` or
   backticks, is parsed again with all three layers, up to 4 levels deep. The
   word is allowed when that parse finds at least one tmux invocation, none
   of them kills, and either every one is scoped or no run of tokens anywhere
   in the word would kill (`kill-server` or a prefix of it, or
   `kill-session` with `-a` after it). So an embedded script gets the same
   verdict as the command unwrapped: `bash -c "tmux kill-session -t amp-x"`
   and `ssh vm1 "tmux kill-session -t amp-x"` are allowed, and
   `ssh vm1 "tmux kill-session -a"` is denied. The token test keeps a tmux
   call that the shell tokeniser cannot read as one, such as the argv list in
   `python3 -c 'subprocess.run(["tmux", "kill-server"])'`, from passing.
   The shell has not yet removed the quoting inside such a word, so its
   tokens are read three ways: as written; with the one-letter printf escapes
   (`\n`, `\t`, …) as separators; and with each `$'…'` decoded and every
   `'`, `"` and `\` removed. A token counts as `tmux` when its last path
   component is `tmux` or ends in `-tmux`, which a parameter expansion's
   fallback leaves joined to its operator. So `bash -c "t''mux kill-server"`,
   `bash -c 't\mux kill-session -a'`, `bash -c '${TMUX_BIN:-tmux} kill-server'`,
   `bash -c '${T-tmux} kill-server'` and `echo "$(${TMUX_BIN:-tmux} kill-server)"`
   are parsed again and denied, as the same spellings are at top level. A
   word with no `kill-s` token that would parse back into the very text being
   checked, such as a lone `${TMUX_BIN:-tmux}`, is not parsed again, since
   layers 1 and 2 have already read it. A single word that holds both tokens
   is still parsed again, and reaching the depth limit denies it.
   The body of each `$( )` or backtick substitution in the word runs as shell
   code wherever it appears, so it is judged the same way. Its output may be
   run too, as in `bash -c "$(echo tmux kill-server)"`,
   `x=$(echo tmux kill-server); $x` or `eval "$(printf 'tmux kill-server')"`,
   so a body that mentions both tokens must also not print them through a
   data-only command. The one exception is a substitution in an argument of a
   data-only command, whose output stays data, so
   `git commit -m "$(cat <<'EOF' … EOF)"` can still mention the guard.

Layers 2 and 3 exempt data-only commands, so a commit message can mention the
guard. The command is data-only when its program is one of `git`, `gh`,
`echo`, `printf`, `grep`, `egrep`, `fgrep`, `rg`, `cat`, `head`, `tail`, `wc`,
`jq`, `test`, `[`, `true` or `:`, no stage of the command line is a shell
evaluator, and the word holds no `$(` or backtick.

`git` is data-only only with one of the subcommands `add`, `commit`, `diff`,
`log`, `notes`, `show`, `stash`, `status` and `tag`, none of which runs shell
text given as an argument. This list fails closed: any other git command is
judged like any other program, so `git fetch --upload-pack 'tmux kill-server #' /tmp/r`,
`git push --receive-pack=…`, `git archive --exec=…` and
`git grep --open-files-in-pager=…` are denied without each option being
listed. A `git -c` value, or any git word
containing `=!`, is never data. A git command whose subcommand runs its
arguments, or stores shell text for a later git command on the same line
(`config`, `clone`, `rebase`, `bisect`, `submodule`, `difftool`,
`mergetool`, `filter-branch`, `for-each-repo`), is a shell evaluator for the
whole line, and its arguments are also checked joined together, so
`git bisect run tmux kill-server`, `git rebase -x 'tmux kill-server' HEAD~1`
and `git config alias.k '!tmux kill-server' && git k` are denied. `gh alias`,
and a `git` or `gh` command preceded by an environment assignment (such as
`GIT_SSH_COMMAND=… git fetch` or `GIT_EXTERNAL_DIFF=… git diff`), are shell
evaluators as well. These rules only matter once the line contains `tmux` and
`kill-s` text; an ordinary `git config user.name …` is never affected.

The `-c` and `--context` values of `amplihack recipe run` (`-c KEY=VALUE`,
`-cKEY=VALUE`, `--context KEY=VALUE`, `--context=KEY=VALUE`) are data under
the same conditions. amplihack hands them to the recipe as text, exactly as
it hands over a `--context-from-file` file, which the guard never reads, so
`amplihack recipe run default-workflow --detach -c task_description="Follow-up to #1733: tmux kill-server guard"`
is allowed. A positional argument, another option's value, a value holding
`$(` or a backtick, or an evaluator anywhere on the line still denies. The
guard knows only these four forms: if `recipe run` gains another way to pass
context inline, text passed that way is judged as shell text until the guard
learns the form, so a task description that mentions the guard is denied
rather than a kill being missed.
`amplihack recipe stop <session>` and the exact-match
`tmux kill-session -t '=<session>'` that `recipe stop` and `--detach` use are
allowed too.

Before any layer runs, the tokeniser removes redirections and their targets
from the words (`>`, `>>`, `<`, `>|`, `<>`, `>&`, `<&`, `&>`, `&>>`, with an
optional fd number such as `2>`), so `tmux 2>/dev/null kill-server` and
`tmux kill-server>/dev/null` are denied and the `&` in `2>&1` does not split
the command. It also decodes `$'…'` escapes as bash does, so
`tmux $'\x6bill-server'` is denied, and, as bash does, keeps a backslash
inside double quotes unless it comes before `$`, a backtick, `"`, `\` or a
newline, so `printf "ls\nkill-server"` keeps its `\n`.

| Command | Result |
|---|---|
| `true; tmux kill-server` | denied |
| `true && tmux kill-server` | denied |
| `false \|\| tmux kill-server` | denied |
| `echo x \| tmux kill-server` | denied |
| `echo \| tmux kill-server` | denied |
| `echo start` *newline* `tmux kill-server` | denied |
| `sh -c 'tmux kill-server'` | denied |
| `bash -c "tmux kill-server"` | denied |
| `echo "$(tmux kill-server)"` | denied |
| `bash -c "$(echo tmux kill-server)"` | denied |
| `x=$(echo tmux kill-server); $x` | denied |
| `x=$(echo "notes on tmux kill-server")` | denied |
| `python -c "os.system('tmux kill-server')"` | denied |
| `ssh vm1 "tmux kill-session -a"` | denied |
| `echo tmux kill-server \| sh` | denied |
| `git config alias.k '!tmux kill-server'` | denied |
| `git fetch --upload-pack 'tmux kill-server #' /tmp/r` | denied |
| `bash -c 'tmux -S /tmp/tx1/a.sock kill-server'` | allowed |
| `bash -c "tmux kill-session -t amp-x"` | allowed |
| `ssh vm1 "tmux kill-session -t amp-x"` | allowed |
| `echo "$(tmux -S /p kill-server)"` | allowed |
| `git commit -m "$(echo 'deny tmux kill-server')"` | allowed |
| `echo kill-server` | allowed |
| `grep kill-server file` | allowed |
| `git commit -m "add tmux kill-server guard"` | allowed |
| `amplihack recipe run default-workflow -c task_description="tmux kill-server guard"` | allowed |
| `amplihack recipe stop amp-default-workflow-repo-20261029-101500` | allowed |
| `tmux kill-session -t '=amp-default-workflow-repo-20261029-101500'` | allowed |

Shell evaluators are `sh`, `bash`, `zsh`, `dash`, `ksh`, `fish`, `eval`,
`source`, `.`, `xargs`, `env -S`, `nohup`, `exec`, `parallel`, `watch`,
`script`, `su`, `sudo`, `setsid`, `ssh`, `docker`, `kubectl`, `find`,
`python`, `python3`, `node`, `perl`, `ruby` and `tmux` itself (so
`tmux new "tmux kill-server"` is denied).

A heredoc body is checked as commands when a shell evaluator appears anywhere
on the command line: `bash <<EOF … tmux kill-server … EOF` is denied, so is
`cat > k.sh <<'EOF' … tmux kill-server … EOF` followed by `bash k.sh`, and
`cat <<EOF > notes.md … tmux kill-server … EOF` alone is allowed. In any other
heredoc whose delimiter is unquoted, the shell still runs each `$( )` and
backtick in the body, so those are checked as commands.

<a id="uncertain-input"></a>
**Uncertain input.** The guard prefers a false positive to a missed kill:

- A program word that is an expansion (it, or its last path component,
  starts with `$` or a backtick: `$T`, `"$(command -v tmux)"`,
  `` `echo tmux` ``, `/usr/bin/$T`) may be tmux and may split into several
  words, so it is treated as an unscoped tmux whose arguments are unknown.
  Any later word holding a token that starts with `kill-s` denies, and so
  does a `kill-s` token on its stdin, by the next rule:
  `echo kill-server | "$(command -v tmux)" -C attach` and
  `${TMUX_BIN:-tmux} -C attach <<EOF` with a `kill-server` body are denied.
- An unscoped tmux denies when an earlier stage of its pipeline has a word
  holding a token that starts with `kill-s`, or a heredoc in the pipeline
  mentions one, because a `tmux -C` client runs each line of its stdin.
  Words are split on whitespace and shell punctuation, and read a second time
  with the one-letter printf escapes (`\n`, `\t`, …) as separators, so
  `echo kill-server | tmux -C attach`, `echo ' kill-server' | tmux -C attach`
  and `printf 'list-sessions\nkill-server\n' | tmux -C attach` are all
  denied. Stdin flows left to right, so a `kill-s` word in the same or a later
  stage is not input to the tmux: in
  `tmux ls -F '#S' | grep '^amp-' | xargs -r -n1 tmux kill-session -t`, the
  `tmux ls` reads nothing the `xargs` stage holds, and the command is allowed.
  A pipeline broken across lines after `|` or `|&` is still one pipeline, as
  in POSIX (`pipe_sequence '|' linebreak command`) and bash, including when a
  comment or blank lines follow the `|`: `echo kill-server |`, a newline, then
  `tmux -C attach` is denied like the one-line form.
- An unscoped tmux invocation whose subcommand word contains `$` or a
  backtick denies, because the shell decides the subcommand at run time. The
  same rule applies to the words the guard inspects after an unknown global
  option. An expansion in an option value, as in `tmux new -s "$name"`, and
  any expansion in a scoped invocation are still allowed.
- Unbalanced quotes, an unterminated `$(`, a `<<` with no delimiter, a
  heredoc whose delimiter line never appears, or a redirection with no target
  fall back to a text test: the command is denied when its text, with every
  `'`, `"` and `\` removed, contains both `tmux` and `kill-s`. Removing
  them undoes quoting such as `t''mux` and `t\mux`. The tokeniser reads every unquoted `<<` outside a
  `$(…)` as a heredoc, including an arithmetic shift such as
  `(( x = 1 << 2 ))` or `$[1<<2]`; its "delimiter" (`2`) seldom appears on a
  line of its own, so the whole command gets the text test, and the lines
  after the shift are not read as heredoc data.
- Input over 64 KiB gets only this text test, not the full parse, so a
  word spelt through an escape (`$'\x74mux'`) or assembled from pieces
  inside over-size input is not caught.
- Reaching the depth limit (4) or the re-parse limit (256) while a candidate
  remains denies, and so does a simple command with more than 64 tmux words.

| Command | Result |
|---|---|
| `c=kill-server; tmux $c` | denied |
| `tmux ${C}` | denied |
| `tmux $(echo kill-server)` | denied |
| ``tmux `echo kill-server` `` | denied |
| `tmux kill-server 'unterminated` | denied |
| `t''mux kill-server 'unterminated` | denied |
| `(( x = 1 << 2 ))`, newline, `tmux kill-server` | denied |
| `"$(command -v tmux)" $c` | denied |
| `echo kill-server \| "$(command -v tmux)" -C attach` | denied |
| `"$(command -v tmux)" -C attach <<EOF`, `kill-server`, `EOF` on separate lines | denied |
| `echo kill-server \| $PAGER` | denied |
| `tmux -S /p $cmd` | allowed |
| `tmux -L x "$c"` | allowed |
| `tmux new -s "$name"` | allowed |
| `"$(command -v tmux)" -C attach </dev/null` | allowed |
| `${TMUX_BIN:-tmux} -L qa-1733 new -d` | allowed |

The guard never panics on any input. On Claude Code and Copilot a panic or
error in the handler fails open: the hook returns `{}` and the command is
allowed. On Codex, `pre_tool_use` runs as a security hook, so a panic or error
goes through `deny_on_failure()` and the command is denied with the generic
reason "Amplihack security check could not process tool input" instead of the
guard's message.

**Response.** On a deny the hook exits `0`. Claude Code and Copilot receive
this JSON object on stdout:

```json
{
  "block": true,
  "message": "<M>",
  "decision": "block",
  "reason": "<M>",
  "permissionDecision": "deny",
  "permissionDecisionReason": "<M>",
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "<M>"
  }
}
```

On Codex, `codex_output()` reduces the deny to its `hookSpecificOutput` member
alone, which `write_security_response` writes with a trailing newline and a
flush:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "<M>"
  }
}
```

`<M>` is this fixed message. The command itself is never echoed.

> Blocked: this command looks like a tmux kill-server or kill-session -a with no -S or -L, which acts on the tmux server named in $TMUX (normally the user's own server, with all of their sessions). TMUX_TMPDIR alone does not redirect tmux while $TMUX is set. Use `tmux -S <private socket> …`, or `env -u TMUX tmux -S <private socket> …`, and only for a server you started on that socket; a socket path taken from $TMUX or from tmux itself (`#{socket_path}`) is the user's server. A socket file named `default`, or `-L default`, also counts as the user's server, even under a sandbox directory: give a private server another name (`-S "$SB/tmux.sock"`), and end one already running on a `default` path with `tmux -S <path> kill-session -t <name>` for each of its sessions, since by default a server exits when its last session ends. If the command only mentions these words as text, such as a commit message or a task description, pass the text in a file (`git commit -F`, `--body-file`, `--context-from-file`) or run that command in its own tool call.

The last sentence covers the false positives listed below, so an agent that
was only writing about the guard is told the supported way out.

An allowed command gets the hook's usual output, so the other PreToolUse checks
still apply.

**Known limits.** The guard is a guard rail against agent mistakes, not a
sandbox. It does not see through:

- shell aliases or functions, script files written by an earlier command
  (`bash script.sh`), or commands assembled at run time from pieces such as
  `"kill-"+"server"`;
- a tmux named by an expansion after the program word, as in
  `xargs "$T" kill-server`, `xargs ${T:-tmux} kill-server` or
  `ssh vm1 "$T" kill-server`. Layer 2 starts an invocation only at a word
  whose basename is `tmux`. The wrappers that layer 1 sees through leave the
  expansion as the program word, so `sudo "$T" kill-server` and
  `nohup "$(command -v tmux)" kill-server` are denied;
- the tmux commands that `tmux source-file` or `tmux source` reads, whether
  from a file, from `-` or `/dev/stdin`, or from a heredoc or here-string. A
  pipeline that puts a `kill-s` word in a stage before an unscoped tmux, or in
  one of its heredocs, is still denied by the
  [uncertain-input](#uncertain-input) rule, but `tmux source-file k.conf`,
  after an earlier call wrote `kill-server` into `k.conf`, is allowed;
- a `tmux -C` client fed by a subshell, a `{ …; }` group or a process
  substitution, as in `(echo kill-server) | tmux -C attach` or
  `tmux -C attach < <(echo kill-server)`. The tokeniser does not read these as
  earlier stages of the client's pipeline, so they are allowed;
- an `-S` path that is a symbolic link to the default socket or to the socket
  in `$TMUX`. The guard reads no files and does not resolve links, so
  `tmux -S /tmp/link kill-server` is allowed whatever the link points to;
- a `tmux -C` client inside a compound command (`if`, `while`, `until`,
  `for`, `case` or a `{ …; }` group) that receives a pipe or heredoc, as in
  `echo kill-server | if true; then tmux -C attach; fi` or
  `printf kill-server | { true; tmux -C attach; }`. The tokeniser ends a stage
  at the first `;` or `)`, so the client starts a new pipeline, reads no
  earlier stage and is allowed;
- a variable copied from `$TMUX`, from tmux's output or from a default socket
  path other than by assignment (`read`, `printf -v`, `declare -n`, a `for`
  loop), or in an earlier tool call;
- a `default` name assembled from pieces, or given only as a parameter
  expansion's fallback other than at the end of the value, as in
  `-S "/tmp/tmux-1000/${N}ault"` or `-L "${L:-default}"`;
- an `-S` value that is a glob or a file read at run time, such as
  `-S /tmp/tmux-*/*` or `-S "$(cat sock.txt)"`;
- a command glued to a short option with no separator, as in
  `git grep -O'tmux kill-server'`, where `-Otmux` reads as one word;
- printf escapes other than the one-letter ones (`\153ill-server`), which
  assemble a word at run time;
- shell text that git runs from a hook file or from configuration set
  outside the command line, and gh subcommands other than `gh alias` that run
  commands (`gh codespace ssh`, `gh extension exec`): `gh` is otherwise
  data-only;
- in input over 64 KiB, which gets only the text test, a word spelt through
  an escape (`$'\x74mux'`) or assembled from pieces. The text test removes
  `'`, `"` and `\` first, so `t''mux` and `t\mux` are caught;
- named-fd redirections (`{fd}>file`), whose `{fd}` is read as a word;
- an arithmetic shift followed, on a later line, by a line equal to its right
  operand, as in `(( x = 1 << 2 ))`, `tmux kill-server`, then `2` on separate
  lines: the lines between are read as heredoc data;
- a shell tool whose name is not in the list above, such as one a new Claude
  Code, Copilot or Codex release adds;
- shell text stored by one tool call and run by a later one, such as a git
  alias, a git pager or a tmux `command-alias` set in one call and used in
  the next.

It does not guard these commands at all:

- `tmux kill-session` with neither `-a` nor `-t`. Inside a pane this kills the
  caller's current session, which may be the user's;
- `kill-window` and `kill-pane`;
- signals sent by other programs, such as `pkill tmux`, `killall tmux` or
  `kill <pid>`.

If the host harness times out the hook before it answers, for example on the
first start of a cold `amplihack-hooks` binary, Claude Code and Copilot run
the command as though the hook had allowed it. What Codex does when it times
out a security hook has not been verified. The guard's own cost grows
linearly with the input: it reads at most 64 tmux invocations per simple
command (a command with more is denied), each reading the rest of that command
once; it parses embedded scripts again at most 256 times and 4 levels deep; and
input over 64 KiB gets only the text test. The slowest inputs found, each just
under 64 KiB, take about 0.3 s in a release build. The harness timeout is
outside the guard's control.

It also errs toward false positives. A shell evaluator anywhere on the line
(including `tmux`, `find`, `docker`, `kubectl` and the git and gh commands
above) removes the data-only exemption for the whole line, so
`tmux ls; git commit -m "add tmux kill-server guard"` is denied. Quoting is
removed before the check, so a commit message containing a single-quoted
`$(tmux kill-server)` is denied too. A substitution that only prints the
text, outside an argument of a data-only command, is denied, as in
`x=$(echo "notes on tmux kill-server")`; so is an unscoped tmux with a kill
command in any argument, as in `tmux send-keys 'tmux kill-server'`. A
hand-written `tmux new-session -d "$(amplihack agent-binary --shell) amplihack recipe run … -c task_description='… tmux kill-server …'"`
is denied as well, because tmux runs that argument as a shell command; use
`amplihack recipe run --detach`, or `--context-from-file`, for such a task.
Put such commands in separate tool calls, or pass the text in a file. A
sandbox socket named `default`, and a variable whose value ends in `default`
used anywhere in an `-S` value (`D=/srv/default; tmux -S "$D/qa.sock" …`), are
denied as well; see [What counts as scoped](#tmux-server-guard).

Three more false positives come from reading tmux where the text alone cannot
rule it out:

- A `kill-s` word piped or heredoc-fed to any program word that is an
  expansion, since it may be a `tmux -C` client: `echo kill-server | $PAGER`
  and `grep kill-session log | "$PAGER"` are denied. So is
  `$T -S "$SB/t.sock" -C attach` fed a kill, because a program word that is
  an expansion may split into several words, and the guard cannot trust the
  scope that follows it.
- A token ending in `-tmux` next to a `kill-s` token in an embedded script
  that holds no tmux invocation, such as
  `bash -c 'ls ~/qa-tmux; echo kill-server'`, or a `--no-tmux` flag, since
  the token may be the fallback in `${T-tmux}`.
- A socket file named `tmux` followed by a kill. Layer 2 reads the `-S`
  value in `env -u TMUX tmux -S "$SB/tmux" kill-server` as a second, unscoped
  tmux. Name the socket `tmux.sock`, as the deny message does.

**Safe form for agents.** `TMUX_TMPDIR` alone is not enough while `$TMUX` is
set. Start, use and kill a private server on its own socket, with `TMUX`
removed from the environment:

```bash
SB=$(mktemp -d)
env -u TMUX tmux -S "$SB/tmux.sock" new-session -d -s qa-1733 'sleep 60'
env -u TMUX tmux -S "$SB/tmux.sock" ls
env -u TMUX tmux -S "$SB/tmux.sock" kill-server
```

The guard allows all four commands, and the user's own server is never
touched. The qa-team, outside-in-testing, e2e-outside-in-test-generator and
other tmux-using skills give the same rule.

**Source.** `crates/amplihack-hooks/src/pre_tool_use/tmux_guard.rs`, with unit
tests in `pre_tool_use/tests/tmux_guard_tests.rs`, host-payload tests in
`bins/amplihack-hooks/tests/tmux_guard_protocol.rs` (the Codex cases assert
the reduced `hookSpecificOutput`-only shape), and a real-tmux test of
`$TMUX` precedence in `crates/amplihack-hooks/tests/tmux_socket_precedence.rs`
that skips when tmux is not installed.

#### tail-follow guard

The tail-follow guard denies a shell command that runs `tail` with a follow
option on a device that never ends, such as `/dev/null`. Agents used
`timeout 10 tail -f /dev/null` as a sleep; under uutils `timeout` the `tail`
could outlive the wait, and each orphan spun on every write to `/dev/null` on
the host (#1794). The supported ways to wait are `amplihack wait --seconds N`
and `cmd & pid=$!; wait $pid`; see
[Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md).

It always runs: no environment variable, flag or setting turns it off, it
reads no environment variables, files or clock, and it does not depend on
XPIA.

**Tools it checks.** The same tool names as the tmux guard
(`shell::is_shell_tool`): `bash`, `shell`, `shell_command`,
`exec_command`, `powershell` and `monitor`, compared case-insensitively. It
reads `tool_input.command`, then `tool_input.cmd`, as a string or as an argv
array of strings. An array is shell-quoted and joined with the shared
`shell::join_argv` before it is checked. An array with a non-string element
is allowed; this differs from the tmux guard, which joins the string
elements and checks them.

**Position.** The guard runs in `PreToolUseHook::process` directly after the
tmux guard, and before the XPIA check, the Skill redirect, the merge guard and
the non-Bash early return. When both guards match, the tmux guard's deny is
returned.

**What it denies.** A command in which, after tokenising with the shared
shell tokeniser in `pre_tool_use/shell.rs`:

1. The program word, found by `shell::program()`, is `tail`. `program()`
   removes `timeout`, `nohup`, `env`, `nice`, `exec`, `command`, `time`,
   `sudo` and variable assignments. The guard itself also removes `setsid`
   (with `-w`, `-f`, `-c`), `stdbuf` (with `-i`, `-o`, `-e` and their
   values, joined or separate), `ionice` (with `-c`, `-n` and their values,
   and `-t`), `chrt` (with its policy flags, `-T`, `-P`, `-D` and their
   values, and a numeric priority), `taskset` (with `-a`, `-c` and the CPU
   mask or list), `unbuffer` (with `-p`), and `function NAME` before a function body
   (`function f { tail -f /dev/null; }`), then calls `program()` again. It repeats this
   up to 8 times, and each round must remove at least one word. `ionice`
   with `-p`, `-P` or `-u`, and `chrt` or `taskset` with `-p`, act on a
   running process and start no command, so nothing after them is read.
2. A follow option is set. Options are read as GNU and uutils `tail` read
   them:

   | Form | Effect |
   |------|--------|
   | `-f`, `-F`, `--follow`, `--follow=name`, `--follow=descriptor` | Follow |
   | Any shortened `--follow`, such as `--f`, `--fo`, `--foll=name` | Follow: `tail` accepts any unique start of a long option, and `follow` is its only option starting with `f` |
   | Grouped short options, such as `-fn0`, `-qf` | Read left to right; `f` or `F` before an option that takes a value (`n`, `c`, `s`) follows. `-n5f` does not follow: `5f` is the value of `-n`. |
   | `-` and a digit, such as `-5f` | Old numeric form: a trailing `f` follows |
   | `--lines`, `--bytes`, `--pid`, `--sleep-interval`, `--max-unchanged-stats` | Take the next word as their value, or a value after `=` |
   | `--` | Ends options; later words are operands |
   | `+5f` | A file operand, not an option |

3. An operand, after repeated `/` and `/./` are collapsed, is one of
   `/dev/null`, `/dev/zero`, `/dev/random`, `/dev/urandom`, `/dev/full`,
   `/dev/stdin`, `/dev/tty`, `/dev/fd/0` or `/proc/self/fd/0`. The path is
   compared as text; the file system is never read.

`--pid=PID` does not exempt a command: `tail -f --pid=1234 /dev/null` is
denied.

**Nested shells.** When the program word's basename is `sh`, `bash`, `zsh`,
`dash` or `ksh`, and a flag word (one that starts with `-` but not `--`)
contains `c`, the guard checks the next operand word as a command of its own,
skipping `--` and long options. The word after `--rcfile` or `--init-file`,
and after a flag word that ends in `o` or `O` (`-o pipefail`,
`-euo pipefail`, `+O extglob`), is an option value, not a script. So
`bash -c 'tail -f /dev/null'`, `bash -lc '…'` and `bash -c -- '…'` are
checked. When such a shell has no `-c`, each heredoc body given to it is
checked in the same way, unless the shell is given a script file without
`-s` (`bash run.sh <<EOF`), when the heredoc is the script's input. A heredoc
given to any other program, such as `cat > Dockerfile <<'EOF'`, is not
checked.

**Command substitutions.** The body of each `$(…)` and backtick substitution
that the shell runs is checked as a command of its own, at one more level of
nesting. That covers a bare substitution and one inside double quotes, so
`x=$(timeout 5 tail -f /dev/null)` and `` echo "`tail -f /dev/null`" `` are
denied. A substitution inside single quotes, `$'…'`, a comment or a heredoc
body is text and is not checked; one inside a `-c` operand is found when that
operand is checked. `docker`, `podman` and `kubectl` are not unwrapped, so
`docker run -d alpine tail -f /dev/null`, a container entry point, is
allowed.

**Fails open.** Unlike the tmux guard, this guard allows a command whenever it
cannot be sure. Each limit below returns no answer, and the command goes on to
the next check:

| Limit | Value |
|-------|-------|
| Input size, for a string or a joined argv array, checked before tokenising | 64 KiB |
| Nesting of `-c` commands and heredocs (top level is depth 0; stops at `depth >= 4`, so up to three nested shells are checked and a fourth is allowed) | 4 |
| Re-parses across the whole command, shared by all nesting | 256 |
| Wrapper-removal rounds per command | 8 |
| `tokenise(..).complete` is false | Always allowed |

A missed deny leaves at worst one orphaned `tail`. A false deny would block
ordinary commands that mention the words as data, so the guard does not fail
closed.

**Examples.**

| Command | Result |
|---------|--------|
| `timeout 10 tail -f /dev/null` | denied |
| `tail -f /dev/null &` | denied |
| `tail -F /dev/null` | denied |
| `tail -fn0 /dev/null` | denied |
| `tail -5f /dev/null` | denied |
| `tail --follow=name //dev//null` | denied |
| `tail -f --pid=1234 /dev/null` | denied |
| `setsid -w stdbuf -oL tail -f /dev/null` | denied |
| `ionice -c3 tail -f /dev/null` | denied |
| `chrt -i 0 tail -f /dev/null` | denied |
| `taskset 1 tail -f /dev/null` | denied |
| `unbuffer tail -f /dev/null` | denied |
| `bash -lc 'tail -f /dev/null'` | denied |
| `bash -c -- 'tail -f /dev/null'` | denied |
| `tail --fo /dev/null` | denied |
| `bash -euo pipefail -c 'tail -f /dev/null'` | denied |
| `bash <<'EOF'`, `tail -f /dev/null`, `EOF` on separate lines | denied |
| `x=$(timeout 5 tail -f /dev/null)` | denied |
| `` echo `tail -f /dev/null` `` | denied |
| `["bash","-lc","tail -f /dev/null"]` (argv array) | denied |
| `tail -f build.log` | allowed |
| `tail -n 5 /dev/null` | allowed |
| `tail -n5f /dev/null` | allowed |
| `tail -f +5f` | allowed |
| `tail -f` and `tail -f -` | allowed |
| `echo 'tail -f /dev/null'` | allowed |
| `echo "$(date)"` | allowed |
| `echo '$(tail -f /dev/null)'` | allowed |
| `cat > Dockerfile <<'EOF'`, `CMD ["tail","-f","/dev/null"]`, `EOF` | allowed |
| `docker run -d alpine tail -f /dev/null` | allowed |
| `tail -f "$DEV"` | allowed |
| `bash <<< 'tail -f /dev/null'` | allowed (known limit) |
| `echo 'tail -f /dev/null' \| bash` | allowed (known limit) |
| `["bash", 1]` (argv array with a number) | allowed |

The guard does not panic on any input: every index into the command text is
bounds-checked first, and grouped short options are read by character.

**Response.** The same full-deny shape as the tmux guard, key for key, shown
under the tmux guard's **Response** above, with `<M>` set to this fixed
message:

> Blocked: `tail` following a device that never ends (such as `tail -f /dev/null`) must not be used as a way to wait. The tail outlives the tool call and is left running as an orphan process (#1794). To wait for a fixed time, run `amplihack wait --seconds N`. To wait for work to finish, run it in the background and wait on its PID: `cmd & pid=$!; wait $pid`. Do not use `tail -f /dev/null` or `timeout N tail -f /dev/null` as a sleep.

The message is a constant. Neither the deny nor the one `warn!` log line,
which cites #1794, includes the command, its operands, the working
directory, the tool name or any environment variable.

**Known limits.** The guard is a guard rail against agent mistakes, not a
sandbox. It does not see:

- a device given by expansion (`$DEV`, `$(…)`, globs);
- `tail` run by `xargs`, `find -exec`, `eval` or `env -S`;
- `tail` run through a multi-call binary, such as `busybox tail`;
- `tail` behind a launch wrapper other than those listed under **What it
  denies**, such as `flock`, `systemd-run`, `cgexec` or `firejail`;
- `tail` in a script file run by a later command;
- a script given to a shell as a here-string (`bash <<< 'tail -f /dev/null'`)
  or through a pipe (`echo 'tail -f /dev/null' | bash`): the script is an
  ordinary word or another command's output, and following it would need
  data-flow tracking;
- a `$(…)` or backtick substitution inside an expanding heredoc fed to a
  program that is not a shell, such as `cat <<EOF` with
  `$(tail -f /dev/null)` in its body;
- `..` in a device path, or a symbolic link to a device;
- nesting deeper than three nested shells, input over 64 KiB, or input the
  tokeniser cannot fully parse;
- `sleep`, or `tail -f` on a regular file.

**Source.** `crates/amplihack-hooks/src/pre_tool_use/tail_follow_guard.rs`,
with unit tests in `pre_tool_use/tests/tail_follow_guard_tests.rs` and
`process` path tests in `pre_tool_use/tests/pre_tool_use_tests.rs`.

### 4. PostToolUse — `post-tool-use`

Runs after every tool call (matcher `*`). Records tool usage metrics. No timeout — fail-open.

Metrics, workflow enforcement state and the blarify staleness marker are written under the [hook state root](./hook-state-root.md), never into the session's working directory.

### 5. UserPromptSubmit — `workflow-classification-reminder`

The **first** of two `UserPromptSubmit` hooks. Injects a workflow classification reminder into the prompt context. Runs via `amplihack-hooks`. 5-second timeout.

### 6. UserPromptSubmit — `user-prompt-submit`

The **second** of two `UserPromptSubmit` hooks. Injects user preferences and native Rust memory context for referenced agents into the prompt. Runs via `amplihack-hooks`. 10-second timeout.

On Unix it also records the prompt's time for a waiting [`limit-resume`](#8-stopfailure--limit-resume) hook, in `<root>/.auto-switch.d/resume/prompted/`, keyed by a hash of the session id. It writes only when that `resume/` directory already exists and is private (a waiting `limit-resume` hook creates it), takes no lock, does not sync to disk, and ignores every error.

When no Claude account has capacity left, it also shows a [capacity notice](#capacity-notice-in-hooks).

### 7. PreCompact — `pre-compact`

Runs before Claude Code compacts the context window. Exports the current transcript so it can be referenced after compaction.

The transcript copy, the original request and the compaction metadata are written to `<state root>/runtime/sessions/<session key>/logs/`; see [Hook State Root Reference](./hook-state-root.md). With no state root, the hook still succeeds and reports `transcript_path` and `metadata` as `null`. When a state root resolves, a failed transcript export or metadata write still returns the error response; see [Write failures](./hook-state-root.md#write-failures).

### 8. StopFailure — `limit-resume`

Runs in the background when a turn ends on an API error that matches
`rate_limit`. Claude Code is expected to run `StopFailure` instead of `Stop`
for these turns, so the `Stop` hook never sees a usage-limit stop; this, the
`stop_hook_active` field on `StopFailure`, and the wake on exit `2` were verified
on Claude Code 2.1.307 (`stop_hook_active` is absent there and treated as false).
Full rules: [Rotate session resume](profile-command.md#rotate-session-resume).

**What it waits for.** When auto-switch is on, the error is a usage limit and
a rotator is running, the hook waits up to 7 days for the rotator to write a
[swap record](profile-command.md#swap-record),
`<root>/.auto-switch.d/rotate/swap.json`, that differs from the one present
at the stop, was swapped after the stop, and shows the new
login below 100% on both windows. Only a swap writes that file; a stay, an
ALERT and `profile switch` do not. It does not wait at all when no rotator is
running.

**When the session stopped.** The hook reads the stop time from the
transcript: only the `timestamp` of the last complete entry in
`transcript_path`, rounded down to whole seconds, from at most the last
65 536 bytes of a regular file you own that is not writable by group or
others (see [Stop time](profile-command.md#stop-time)). A swap counts as
after the stop when `swapped_at` is at or after the earlier of the stop time
and one second before the hook's start (`STOP_SLACK_SECS`). With no usable
stop time, the threshold is one second before the hook's start. So a swap in
the hook's start second or the second before it always counts, and so does a
swap between the stop and a late hook start, including the record present
when the hook starts. Nothing read from the transcript is logged.

**Which event it is.** The event key includes the login the session stopped
on: the noted swap record's `from_login` when that swap counts as after the
stop and is not later than the hook's start, otherwise the ledger's live
login at the hook's start.

**How it waits.** One blocking `poll(2)`, with nothing read on an interval,
on:

- an inotify watch on `rotate/` for `swap.json` being closed after writing or
  renamed into place;
- an inotify watch on `resume/prompted/` for a prompt marker renamed into
  place (the session moved on; see hook 6);
- a pidfd on the hook's parent, the Claude Code process (the session exited);
- a timeout of the time left until 7 days after the start, which ends the
  wait and never resumes.

After the watches are in place it reads `swap.json` once, so a swap made
during setup is not missed. Both directories must be real directories owned
by the user and not writable by group or others, and the parent pid must not
be 1. Watching needs Linux with `pidfd_open` (kernel 5.3 or later). On any
other system, or when a watch cannot be set up or is lost, the hook ends as
`watch-lost`. It never falls back to polling.

In the code, the wait sits behind the hook's `Deps` trait: `arm()` sets up
the watches, `wait_event(deadline)` returns one of `Swap`, `Prompted`,
`SessionGone`, `WatchLost` or `Deadline` (`WatchLost` also when the
deadline cannot be represented as a clock time), `read_swap()` reads the
record, `stop_time(transcript, start)` returns the stop time or nothing,
`pane()` returns the verified pane, `send_keys(pane, key)` sends one key,
and `session_alive_now()` checks the pidfd and the foreground process
group without blocking; with no pidfd it reports the session as not
running, so the check before each key fails and no key is sent. Tests replace
`Deps` with a fake that scripts the events.

**How it resumes.** It records the resume under its lock first, then takes
exactly one path:

- **Keys, exit `0`, outcome `resumed-keys`.** When `$TMUX` is set and
  `$TMUX_PANE` is `%` followed by digits, and that pane's `#{pane_tty}` is
  Claude Code's standard input with Claude Code as the terminal's foreground
  process group (checked once, just before the commit), it runs
  `tmux send-keys -t %N Escape`, then `C-u`, then `-l continue`, then
  `Enter`, as four separate commands, each waited on for its exit status.
  Before each key it checks that the session is still running and still in
  the terminal's foreground, so a session that exits or is suspended
  (Ctrl-Z) gets no more keys. A failure stops the remaining keys and ends as
  `resume-keys-failed`, still exit `0`. If the session exits or is suspended
  in the instant before a key, at most that one key is at risk.
  `C-u` clears the whole input line, so text the user typed in the stopped
  session and did not submit is deleted. Only a submitted prompt counts as
  moving on (hook 6 records it); an unsent draft does not stop the resume.
  The keys are not yet confirmed against a real Claude Code timed wait; see
  [#2215](https://github.com/rysweet/amplihack-rs/issues/2215).
- **Wake, exit `2`, outcome `resumed`.** Without such a pane, it exits `2`
  with a one-line continue message on stderr, which wakes the session.

It never sends a digit, `/usage-credits`, `/rate-limit-options` or any text
other than `continue`, and never sends keys to a pane it did not note at the
stop. `tmux` is found through `PATH` and uses the server named by `$TMUX`;
both are trusted because they belong to the same user as the session.

**Exit codes.** `2` only for `resumed`. Every other outcome, including
`resumed-keys`, `resume-keys-failed`, `watch-lost`, `no-capacity-timeout`,
`moved-on`, `session-gone`, `disabled`, `duplicate`, `cap`,
`stop-hook-active`, `rotator-down` and `guard-failed`, exits `0` and prints
nothing. stdout is always empty.

It resumes at most once per limit event, once per session per wait, and 3
times per session in 24 hours, never reads a credentials or token file, and
reads nothing from the transcript but the last entry's `timestamp`. The
608400-second timeout covers the 7-day wait plus a 300-second margin; Claude
Code 2.1.307 accepts it. Requires Claude Code 2.1.306 or later:
`amplihack install` runs `claude --version` and leaves this entry out (removing an
earlier one) for an older version, because Claude Code before 2.1.78 ignores
a settings file that names `StopFailure`. An entry an earlier release wrote
is also removed by the next launch when that `claude` is too old, and an
automatic re-stage after an upgrade, which never edits `settings.json`,
prints a warning naming `amplihack install`.

## Capacity notice in hooks

When [auto-switch](profile-command.md#auto-switch) is on and every eligible
Claude account is at or above its threshold, `session-start` and
`user-prompt-submit` tell both you and the model. The state comes from the
[capacity file](profile-command.md#capacity-file); the rules for when it is
`low` or `exhausted` and when it clears are in
[Capacity notice](profile-command.md#capacity-notice).

### What the hooks add

Both hooks add a top-level `systemMessage`, which Claude Code shows to you,
and append a note for the model to `hookSpecificOutput.additionalContext`.
For `user-prompt-submit`:

```json
{
  "systemMessage": "⚠ all Claude accounts ≥90% · next reset Oct 22 2am — run 'amplihack profile usage'",
  "hookSpecificOutput": {
    "hookEventName": "UserPromptSubmit",
    "additionalContext": "<the usual context>\n\nClaude account capacity is low across all profiles (next reset Oct 22 2am). Avoid launching large parallel workflows, multi-agent fan-out or long recipe runs; prefer small sequential steps."
  }
}
```

With the `exhausted` state the message reads `at limit` instead of `≥90%`,
and the model note reads `exhausted` instead of `low`. An unknown reset time
is written as `next reset unknown`.

`session-start` adds the same `systemMessage` and appends the same note to
its `additionalContext`; `hookEventName` and `indexing_status` are kept.
When `user-prompt-submit` has no other context, the notice is the whole
`additionalContext` instead of the empty `{}` it returns otherwise. An
empty prompt still returns early, with no notice.

Both texts are built from fixed wording, the state, the threshold and the
reset time. Nothing else from the capacity file reaches the output, so a
profile name or other text written into the file cannot be injected into the
model's context.

### When the hooks show it

- **Once per state per session.** The hooks keep the last state shown for
  each `session_id` in a [notice record](profile-command.md#notice-records).
  `low` after `low` shows nothing; `low` then `exhausted` shows again. When
  the state returns to `ok`, the record is deleted, so a later drop shows
  again.
- **SessionStart counts.** A new session has no record, so `session-start`
  shows the notice and records it, and the first prompt stays quiet.
- **No `session_id`.** The notice is shown every time the state is `low` or
  `exhausted`.
- **Nested recipe sessions** (`AMPLIHACK_SESSION_DEPTH` above 0) get the
  model note only, never a `systemMessage`. The record is kept as usual.
- **Codex** reads only `additionalContext`, so under Codex the model gets
  the note and you see no message.

### Cost and failure

The check reads one file of at most 16 KiB and at most one small record,
and may write one record. It starts no process, uses no network and takes no
lock. A missing, damaged or unreadable file, an unresolvable profile root,
the switch being off, or any error in the check gives no notice, and the
hook's output is byte-identical to what it is without this feature. The
check never makes a hook fail.

## Hook Runtime Location

Fresh installs register hooks through the native `amplihack-hooks` binary deployed at `~/.local/bin/amplihack-hooks`. The installer writes command strings such as `amplihack-hooks session-start` and `amplihack-hooks user-prompt-submit` into `~/.claude/settings.json`.

The hooks keep their state under `${AMPLIHACK_HOME:-~/.amplihack}/runtime/`. The [Hook State Root Reference](./hook-state-root.md) lists every file, how the root is resolved and what happens when it cannot be.

## Generated script line endings

Generated Bash-facing hook scripts are written with LF-only line endings. This
includes Copilot `.github/hooks/*` wrappers, generated `_error_handler` content,
and legacy launcher-staged hook scripts. The hook staging boundary normalizes
CRLF and lone CR input before writing executable script bytes, so generated
hooks do not fail with `$'\r': command not found` or bad-interpreter `\r`
errors on Windows-native checkouts.
