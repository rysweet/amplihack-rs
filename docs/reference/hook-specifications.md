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
`exec_command` and `powershell`, compared case-insensitively. It runs after
the XPIA check and the Skill redirect, beside the merge guard, and before the
non-Bash early return. It checks both `tool_input.command` and
`tool_input.cmd`, and either one can deny, so a harmless `command` does not
hide a `cmd` that kills the user's server.

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
  as `tmux`, `/usr/bin/tmux` or `command tmux`. A sandbox path that only
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
   are seen through to find the program word.
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

- A program word that starts with `$`, together with any word starting with
  `kill-s`, denies.
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
- Unbalanced quotes, an unterminated `$(`, a `<<` with no delimiter, or a
  redirection with no target fall back to a text test: the command is denied
  when its text contains both `tmux` and `kill-s`.
- Input over 64 KiB gets only this plain text test, not the full parse, so
  quoting such as `t''mux` inside over-size input is not caught.
- Reaching the depth limit (4) or the re-parse limit (256) while a candidate
  remains denies, and so does a simple command with more than 64 tmux words.

| Command | Result |
|---|---|
| `c=kill-server; tmux $c` | denied |
| `tmux ${C}` | denied |
| `tmux $(echo kill-server)` | denied |
| ``tmux `echo kill-server` `` | denied |
| `tmux kill-server 'unterminated` | denied |
| `tmux -S /p $cmd` | allowed |
| `tmux -L x "$c"` | allowed |
| `tmux new -s "$name"` | allowed |

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
- quoting tricks in input over 64 KiB, which gets only the plain text test;
- named-fd redirections (`{fd}>file`), whose `{fd}` is read as a word;
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
under 64 KiB, take about 0.2 s in a release build. The harness timeout is
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

### 4. PostToolUse — `post-tool-use`

Runs after every tool call (matcher `*`). Records tool usage metrics. No timeout — fail-open.

### 5. UserPromptSubmit — `workflow-classification-reminder`

The **first** of two `UserPromptSubmit` hooks. Injects a workflow classification reminder into the prompt context. Runs via `amplihack-hooks`. 5-second timeout.

### 6. UserPromptSubmit — `user-prompt-submit`

The **second** of two `UserPromptSubmit` hooks. Injects user preferences and native Rust memory context for referenced agents into the prompt. Runs via `amplihack-hooks`. 10-second timeout.

On Unix it also records the prompt's time for a waiting [`limit-resume`](#8-stopfailure--limit-resume) hook, in `<root>/.auto-switch.d/resume/prompted/`, keyed by a hash of the session id. It writes only when that `resume/` directory already exists and is private (a waiting `limit-resume` hook creates it), takes no lock, does not sync to disk, and ignores every error.

When no Claude account has capacity left, it also shows a [capacity notice](#capacity-notice-in-hooks).

### 7. PreCompact — `pre-compact`

Runs before Claude Code compacts the context window. Exports the current transcript so it can be referenced after compaction.

### 8. StopFailure — `limit-resume`

Runs in the background when a turn ends on an API error that matches
`rate_limit`. Claude Code is expected to run `StopFailure` instead of `Stop`
for these turns, so the `Stop` hook never sees a usage-limit stop; this, the
`stop_hook_active` field on `StopFailure`, and the wake on exit `2` were verified
on Claude Code 2.1.307 (`stop_hook_active` is absent there and treated as false). When auto-switch is on and
the error is a usage limit, the hook waits up to 7 days for
`amplihack profile rotate` to record a cycle with capacity that started
measuring after the hook did, then exits `2` with
a one-line continue message on stderr, which wakes the session. It reads the
ledger every 15 seconds for the first hour, then every 5 minutes. It stops
waiting if auto-switch is turned off, you send a prompt in that session (the
session moved on; see hook 6), or the session exits, and does not wait at all
when no rotator is running. In every other case it exits `0` and prints
nothing; stdout is always empty. It resumes at most once per limit event, once
per session per wait, and 3 times per session in 24 hours, and never reads a
credentials or token file or the transcript. The 608400-second timeout covers the 7-day wait
plus one 5-minute ledger read; Claude Code 2.1.307 accepts it. Requires Claude Code 2.1.306 or later:
`amplihack install` runs `claude --version` and leaves this entry out (removing
an earlier one) for an older version, because Claude Code before 2.1.78
ignores a settings file that names `StopFailure`. An entry an earlier release
wrote is also removed by the next launch when that `claude` is too old, and an
automatic re-stage after an upgrade, which never edits `settings.json`, prints a
warning naming `amplihack install`. Full
rules: [Rotate session resume](profile-command.md#rotate-session-resume).

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

## Generated script line endings

Generated Bash-facing hook scripts are written with LF-only line endings. This
includes Copilot `.github/hooks/*` wrappers, generated `_error_handler` content,
and legacy launcher-staged hook scripts. The hook staging boundary normalizes
CRLF and lone CR input before writing executable script bytes, so generated
hooks do not fail with `$'\r': command not found` or bad-interpreter `\r`
errors on Windows-native checkouts.
