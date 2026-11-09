---
title: "Agent Prompt Channels Reference"
description: "How every amplihack launch path hands prompt, brief and contract text to an agent CLI without putting it in the child's argv: the contract file, the version gate, the passthrough rewrite, per-path channels, errors and tests."
last_updated: 2026-11-08
review_schedule: quarterly
owner: rysweet
doc_type: reference
related:
  - ../concepts/prompt-text-out-of-argv.md
  - ../howto/check-agent-argv-for-prompt-text.md
  - ../SYSTEM_PROMPT_APPEND.md
  - ./large-payload-delivery.md
  - ./amplihack-env-vars.md
---

# Agent Prompt Channels Reference

No agent process that amplihack starts has prompt, brief or contract text in
its argv. This page lists the channel each launch path uses, the files
amplihack writes, the checks that run before a launch, and the errors a user
can see. For the reasons behind the rule, see
[Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md).
Issue: [#2030](https://github.com/rysweet/amplihack-rs/issues/2030).

## Contents

- [The rule](#the-rule)
- [Channels by launch path](#channels-by-launch-path)
- [The contract file](#the-contract-file)
- [The version gate](#the-version-gate)
- [Passthrough rewrite for `amplihack claude`](#passthrough-rewrite-for-amplihack-claude)
- [Passthrough rewrite for `amplihack codex` and `amplihack copilot`](#passthrough-rewrite-for-amplihack-codex-and-amplihack-copilot)
- [Per-session files](#per-session-files)
- [Automatic prompt delivery never selects argv](#automatic-prompt-delivery-never-selects-argv)
- [Codex](#codex)
- [Copilot](#copilot)
- [Recipe shell steps](#recipe-shell-steps)
- [Docker launches](#docker-launches)
- [Environment variables](#environment-variables)
- [Errors](#errors)
- [Behaviour changes](#behaviour-changes)
- [What argv still contains](#what-argv-still-contains)
- [Tests](#tests)

## The rule

For every agent CLI child (`claude`, `codex`, `copilot`, `amplifier`,
`gh copilot`) that amplihack starts:

- `/proc/<pid>/cmdline` contains no part of the operating contract, the task
  prompt, a recipe step's brief or an agent's system prompt;
- the text reaches the child on stdin, through a `*-file` flag naming a 0600
  file, or through a one-line pointer sentence naming a 0600 file;
- if none of those channels exists for the binary, the launch fails.
  amplihack never falls back to argv.

No setting puts a prompt back in argv. `AMPLIHACK_PROMPT_DELIVERY=argv` is no
longer an accepted value: amplihack treats it as unrecognised, logs a warning
and uses `auto`. This matters because the variable is inherited by every
nested step, so one export used to put every prompt below it back within reach
of `pkill -f`.

## Channels by launch path

| Launch path | Before #2030 | After #2030 |
| --- | --- | --- |
| `amplihack claude`, `amplihack launch` (contract) | `--append-system-prompt <contract text>` | `--append-system-prompt-file <home>/private/system-prompt-append.md` |
| `amplihack claude -- -p <task>` (nested, as in recipes) | `-p <task>` passed through | bare `-p`, task on stdin |
| `amplihack claude -- --system-prompt <text>` | passed through | `--system-prompt-file <per-session file>` |
| `amplihack claude -- --append-system-prompt <text>` | passed through | `--append-system-prompt-file <per-session file>`; amplihack's contract is not added |
| Recipe agent steps (the agent CLI child) | `-p <brief> --system-prompt <agent md>` | bare `-p` with the brief on stdin, `--system-prompt-file <per-session file>` (rewritten by `amplihack claude`) |
| Recipe agent steps (the `amplihack claude` wrapper process) | brief in argv | brief in argv until the runner change ([rysweet/amplihack-recipe-runner#205](https://github.com/rysweet/amplihack-recipe-runner/issues/205)) and the pin update ([#2115](https://github.com/rysweet/amplihack-rs/issues/2115)); see [What argv still contains](#what-argv-still-contains) |
| Auto-mode turn, Claude | stdin above 4096 bytes, argv below | stdin at every size |
| Auto-mode turn, Codex | argv below 4096 bytes | `codex exec -`, prompt on stdin |
| Auto-mode turn, Copilot | file reference above 4096 bytes, argv below | pointer sentence at every size, with `--add-dir <private dir>` |
| Fleet and remote launches | `--prompt-file`, prompt on stdin (#1570) | unchanged; the remote file is created under `umask 077` and the started auto-mode child follows the auto-mode rows above |
| `amplihack copilot -- -p <text>` (also `--prompt`, `-i`, `--interactive`) | passed through | the flag and the pointer sentence, with `--add-dir <private dir>` (no `--add-dir` under the external LiteLLM gateway); see [Passthrough rewrite for `amplihack codex` and `amplihack copilot`](#passthrough-rewrite-for-amplihack-codex-and-amplihack-copilot) |
| `amplihack codex -- exec <text>` (also `exec review`, `exec resume`, `exec fork`, top-level `review`) | passed through | `-` in place of `<text>`, prompt on stdin |
| `amplihack codex -- '<prompt>'` (interactive first prompt), `amplihack codex -- resume <session> <text>`, `amplihack codex -- fork <session> <text>` | positional argv | unchanged; documented exception (see [What argv still contains](#what-argv-still-contains)) |
| Auto-mode turn, Amplifier (`amplifier run`) | argv below 4096 bytes | pointer sentence |
| `amplihack amplifier -- run <prompt>` | passed through | unchanged; documented exception (see [What argv still contains](#what-argv-still-contains)) |
| `llm_client` (Claude) | argv below 4096 bytes | stdin |
| `llm_client` (Copilot) | argv below 4096 bytes | pointer sentence |
| `llm_client` (unknown launcher) | argv | error: `NoSafeChannel` |
| `claude_process` (orchestration) | stdin above 4096 bytes | stdin |
| `platform_cli` Claude delegate | `claude -p <prompt>` | `claude -p`, prompt on stdin |
| `platform_cli` `amplifier run` | prompt in argv | pointer sentence |
| `platform_cli` `gh copilot suggest` | prompt in argv | error: no safe channel (it reads neither stdin nor files) |
| `copilot_strategy` (`gh copilot --prompt`) | prompt in argv | pointer sentence with `--add-dir <private dir>` |
| `amplihack-turn` Copilot turn (`build_turn_argv`, Signal chat) | `-p <message>` | `-p <pointer sentence>` with `--add-dir <private dir>`; the file is removed after the turn |
| `quality-loop.yaml` (three `amplihack copilot -p` calls) | `-p "$PROMPT"` | pointer sentence to a 0600 file, removed by an `EXIT` trap |
| Interactive first prompt, `amplihack claude -- '<prompt>'` | positional argv | unchanged; out of scope (see [What argv still contains](#what-argv-still-contains)) |
| litellm preflight probes | fixed probe strings | unchanged; the strings hold no prompt or contract text |

Claude Code 2.1.319 lists both `--system-prompt-file` and
`--append-system-prompt-file` in `claude --help`, so one minimum covers every
row; see [The version gate](#the-version-gate). `rustyclawd` gets no contract;
see [When the contract is injected](#when-the-contract-is-injected).

## The contract file

The operating contract (`amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md`) is
compiled into the binary. At each launch that injects it, amplihack writes
those bytes to a file and passes the path:

```text
claude --model opus[1m] --append-system-prompt-file /home/alice/.amplihack/private/system-prompt-append.md <user args...>
```

| Property | Value |
| --- | --- |
| Path | `<amplihack home>/private/system-prompt-append.md` |
| Amplihack home | `AMPLIHACK_HOME` if set, otherwise `~/.amplihack` |
| Directory mode | `private/` is 0700, owned by the current user |
| File mode | 0600, regular file, owned by the current user |
| Written by | `ensure_contract_file(home)`, at launch, never at install |
| Read by amplihack | never as a source of content; only a bounded byte comparison with the compiled-in fragment |
| Listed in `install::essential_files` | no |
| Removed at session end | no; one file per install is shared by every session |

### What `ensure_contract_file` checks

`ensure_contract_file` uses `lstat` (`symlink_metadata`) and never follows a
symlink. It runs these checks in order:

| Check | If it fails |
| --- | --- |
| `home` contains no newline and no NUL | Launch fails |
| `private/` exists | Created with mode 0700 |
| `private/` is a directory, not a symlink | Launch fails |
| `private/` is owned by the current user | Launch fails |
| `private/` has mode 0700 | `chmod 0700` (the directory is ours) |
| The file is a regular file, not a symlink | Replaced |
| The file is owned by the current user and has mode 0600 | Replaced |
| The file's length equals the fragment's length | Replaced, without reading the file |
| The file's bytes equal the fragment | Replaced |

Replacing the file:

1. create `private/.system-prompt-append.<random>.tmp` with `create_new`,
   `O_NOFOLLOW` and mode 0600 (the suffix has at least 64 random bits);
2. `fchmod` it to 0600, write the fragment and `sync_all`;
3. `rename` it over `system-prompt-append.md`.

A rename replaces a symlink itself, not the file it points to, so a planted
symlink's target is never written. If any step fails, the temporary file is
unlinked. Two first launches that race both write identical bytes, and the
atomic rename means neither launch sees a partial file.

An upgraded binary rewrites the file at its first launch, because the bytes no
longer match. Remote and fleet hosts write their own copy from their own
binary; nothing is copied over ssh.

### When the contract is injected

The gate `should_inject_system_prompt_append(binary_name, extra_args, opt_out)`
is unchanged and pure. Injection happens only when all of these hold:

1. The binary maps to an `AgentBinary` whose `supports_append_prompt` is true
   (`claude`, `rusty`, `rustyclawd`).
2. `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND` is not `1`.
3. The user did not pass `--append-system-prompt` or `--append-system-prompt-file`
   in any form, before or after the passthrough rewrite.

> **`rustyclawd` gets no contract.** Its support for
> `--append-system-prompt-file` could not be confirmed, and the text form is
> never used, so `contract_injection_applies` returns false for a binary named
> `rusty` or `rustyclawd`, and for a `claude` launch that resolves to a file of
> either name. The pure gate above is unchanged; the check is at the launch
> site.

When the gate is true, `build_command_for_dir` must receive the contract path:

```rust
pub fn build_command_for_dir(
    /* existing arguments */
    contract_file: Option<&Path>,
) -> anyhow::Result<Command>;
```

A true gate with `contract_file: None` returns the error `CONTRACT_PATH_MISSING`
and no command is built. The command is never built with contract text instead.

## The version gate

`--append-system-prompt-file` needs Claude Code **2.1.319** or later.

| Constant | Value | Status |
| --- | --- | --- |
| `MIN_APPEND_PROMPT_FILE_VERSION` | `(2, 1, 319)` | Settled; also covers `--system-prompt-file`, which 2.1.319 lists in `--help` |

The gate runs when amplihack is about to inject the contract, or when the
passthrough rewrite produced a `*-file` flag. It does not run when the user has
opted out with `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1` and supplied no text flag
to rewrite, or for Docker launches.

How the version is found:

1. `BinaryInfo.version`, if the launcher already populated it;
2. otherwise one `<resolved binary> --version` probe, with stdin set to
   `/dev/null`, a 10-second timeout and output capped at 4 KiB. The probe runs
   the file the launcher resolved, not the name `claude` looked up again on
   `PATH`.

The 2.1.319 comparison applies to Claude Code only. `rustyclawd` numbers its
versions differently; see the `rustyclawd` note in
[When the contract is injected](#when-the-contract-is-injected).

The first `MAJOR.MINOR.PATCH` in the output is the version. Each part is parsed
as a `u32`; a part that overflows counts as unparseable.

| Result | Outcome |
| --- | --- |
| 2.1.319 or later | Launch continues |
| Below 2.1.319 | Exit non-zero before any agent spawn |
| No version in the output | Exit non-zero before any agent spawn |
| Probe failed or timed out | Exit non-zero before any agent spawn |

The gate always runs before the agent is spawned. After a failed gate the
binary has been invoked with `--version` only, never with prompt text.

```text
$ amplihack claude
error: amplihack passes its operating contract with --append-system-prompt-file, which needs Claude Code 2.1.319 or later; found 2.1.318.
Upgrade Claude Code, or set AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1 to launch without the contract.
```

## Passthrough rewrite for `amplihack claude`

`amplihack claude` rewrites the arguments it passes through to `claude`, so a
caller that puts text on the wrapper's argv does not put it on the agent's
argv. `rewrite_claude_passthrough(args)` runs only when the target binary is
`claude` (including `rusty` and `rustyclawd`), and before the injection gate.

| Caller passes | Child receives |
| --- | --- |
| `-p <text>`, `--print <text>` | bare `-p` or `--print`; `<text>` on stdin |
| `-p=<text>`, `--print=<text>` | bare `-p` or `--print`; `<text>` on stdin |
| `-p -- <text>`, `--print [options] -- <text>` | bare `-p` or `--print` with its options; `<text>` on stdin; `--` dropped |
| bare `-p` with nothing after it | unchanged; stdin inherited from the caller |
| bare `-p` followed by a token that starts with `-` | unchanged; Claude parses that token as an option |
| `--` and everything after it, outside print mode | unchanged |
| `--system-prompt <text>`, `--system-prompt=<text>` | `--system-prompt-file <per-session file>` |
| `--append-system-prompt <text>`, `--append-system-prompt=<text>` | `--append-system-prompt-file <per-session file>`; counts as user-supplied, so the contract is not added |
| `--system-prompt-file`, `--append-system-prompt-file` | unchanged |
| a positional prompt (interactive) and everything after it | unchanged |

A prompt that begins with `-` must follow `--`, as in `-p -- "-v is broken"`,
or use the `=` form, as in `--print=-v is broken`. A bare `-p` followed by a
dash token is left alone, because the rewrite cannot tell a prompt from a
mistyped option. Print mode is decided by the tokens before `--`; a `-p` after
`--` is a positional, not a flag.

The rewrite rejects input it cannot rewrite safely:

| Input | Error |
| --- | --- |
| `-p <text>` given twice | `the prompt was given twice (-p/--print); pass it once` |
| More than one positional after `--` in print mode, or a prompt both before and after `--` | `the prompt was given twice (-p/--print); pass it once` |
| A per-session file name already in use (the same flag given twice) | `--system-prompt was given twice; pass it once` (or the append flag); any other failure to create the file gives `cannot create a private file for <flag>` |
| `--system-prompt <text>` and `--system-prompt-file <path>` together | `--system-prompt and --system-prompt-file were both given; pass one` |
| `--append-system-prompt <text>` and `--append-system-prompt-file <path>` together | `--append-system-prompt and --append-system-prompt-file were both given; pass one` |

Error messages name flags only, never the text.

When the rewrite produced a stdin payload, amplihack spawns the child with a
piped stdin and writes the payload with `spawn_stdin_writer`. If amplihack's
own stdin is a pipe, a socket or a non-empty file, it uses
`spawn_stdin_forwarder` instead: the payload, a blank line, then amplihack's
stdin copied to the child until end of file, so input piped to
`amplihack claude -p <text>` still reaches the agent. Once the child has
exited, amplihack joins the writer only if it has finished; a forwarder still
blocked on a caller pipe that never closes is left to end with the process.
Without a payload, the child inherits amplihack's stdin as before.

Under auto-switch or usage-limit supervision, each attempt receives the same
rewritten payload on stdin and the same per-session files; the payload is held
in memory until amplihack exits and is never written to the rerun buffer.
The caller's own stdin follows the payload as described above. A regular
file on stdin is rewound for each attempt, so every attempt receives the same
bytes; a pipe's contents go to the first attempt only, and later attempts get
`/dev/null` and so the payload alone, as when the agent read the pipe
directly before #2030.

`Rewritten` implements `Debug` by hand. It prints `stdin_payload: Some(<n bytes>)`
and never the text.

### Launch order

`run_launch_core` runs these steps in this order. Nothing is spawned before
step 3 succeeds.

1. `rewrite_claude_passthrough` (Claude), `rewrite_codex_passthrough` (Codex)
   or `rewrite_copilot_passthrough` (Copilot)
2. `should_inject_system_prompt_append` on the rewritten arguments
3. the version gate, if step 2 is true or the rewrite used a `*-file` flag
4. `ensure_contract_file`, if step 2 is true
5. build the command
6. spawn, with piped stdin only when there is a payload
7. wait for the child or a forwarded signal, then drop `Rewritten`

## Passthrough rewrite for `amplihack codex` and `amplihack copilot`

`amplihack codex` and `amplihack copilot` rewrite their passthrough arguments
in the same launch step as `amplihack claude`
(`crates/amplihack-cli/src/commands/launch/codex_copilot_rewrite.rs`). The
stdin payload is delivered the same way: a piped stdin written by
`spawn_stdin_writer`.

Codex (`rewrite_codex_passthrough`):

| Caller passes | Child receives |
| --- | --- |
| `exec <text>`, `e <text>`, `exec review <text>` | `-` in place of `<text>`; `<text>` on stdin |
| `exec resume <session> <text>`, `exec resume --last <text>` | `-` in place of `<text>`; `<text>` on stdin |
| `exec fork <session> <text>` | `-` in place of `<text>`; `<text>` on stdin |
| `exec review --commit <sha>`, `exec review --base <branch>` | unchanged; the values are not prompts |
| `exec review --base <branch> <text>` | `exec review --base <branch> -`; `<text>` on stdin |
| top-level `review <text>`, `review --base <branch> <text>` | `-` in place of `<text>`; `<text>` on stdin |
| top-level `review --commit <sha>`, `review --uncommitted` | unchanged; no prompt |
| `exec fork <session> -i <file> <text>` (also `exec resume`) | `-` in place of `<text>`; `<text>` on stdin |
| `exec -`, or `exec` with no prompt | unchanged |
| `exec -i <a> <b> ...` (one `--image` with several values) and no prompt | error; one of the values may be the prompt |
| `exec` with an option amplihack does not know, written without `=` and followed by a word | error; that word may be the prompt |
| `exec help ...` | unchanged |
| interactive `codex <text>`, top-level `resume <session> <text>`, top-level `fork <session> <text>` | unchanged; documented exception (interactive starts, see [What argv still contains](#what-argv-still-contains)) |
| any other subcommand (`login`, `mcp`, ...) | unchanged; takes no prompt |

Which Codex options take a value depends on the subcommand: `--base`,
`--commit` and `--title` exist only under `exec review` and top-level
`review` (which has no `-m`), and `-i/--image`
takes every following word under plain `exec` but exactly one under
`exec resume` and `exec fork`. So the rewrite reads the command line with one
option table per scope (`codex_exec_args.rs`): before the subcommand, `exec`,
`exec review`, `exec resume`, `exec fork` and top-level `review`. Each table is a copy of
codex-cli 0.166.0's help for that scope, saved under
`crates/amplihack-cli/src/commands/launch/fixtures/codex-*-help-0.166.0.txt`,
and `tables_match_the_codex_0_166_0_help_text` fails when a table and its help
text disagree. Option values are never read as a prompt, and the caller's
arguments are changed only where the prompt was.

The reading follows clap as Codex uses it: `--opt=value` and `-ovalue` carry
their value; a variadic `--image` stops at the next token that starts with
`-`, a lone `-` included; an attached value closes the occurrence. A prompt
after `--` is rewritten too: `exec -- <text>` becomes `exec -- -`. Codex reads
`-` as "read the prompt from stdin" for `exec`, `exec review`, `exec resume`,
`exec fork` and top-level `review` (codex-cli 0.166.0).

Where amplihack cannot tell which word is the prompt it refuses to launch
rather than pass the arguments through, because passing them through would
put the prompt on Codex's argv: an option a later Codex added, followed by a
word, and a plain-`exec` `--image` that took more than one value with no
prompt left. Each error says how to write the command so it is unambiguous. The flags added for recipe agent steps
(#1985) go directly after `exec`, so `exec <text>` reaches Codex as
`exec --sandbox workspace-write ... --dangerously-bypass-hook-trust -`.

Interactive Codex is not rewritten. The first positional of a bare `codex` is
either a prompt or a Codex subcommand, and the subcommand list changes
between Codex releases, so treating it as a prompt could break a subcommand.

Copilot (`rewrite_copilot_passthrough`):

| Caller passes | Child receives |
| --- | --- |
| `-p <text>`, `--prompt <text>`, `--prompt=<text>`, `-p<text>` | the same flag and the pointer sentence |
| `-i <text>`, `--interactive <text>`, `--interactive=<text>`, `-i<text>` | the same flag and the pointer sentence |
| a value that already begins with the pointer sentence (as in `quality-loop.yaml`) | unchanged |
| `--` and everything after it | unchanged |

When a pointer file was written, `--add-dir <per-session directory>` is put in
front of the arguments, except under the external LiteLLM gateway, whose
launch policy refuses `--add-dir`.

Under the gateway the pointer file, in `$TMPDIR`, is outside the workspace,
so Copilot can open it only with path access: `--allow-all-paths`, or
`--allow-all` or `--yolo`, which imply it. amplihack adds `--allow-all` unless
the caller passed an `--allow-all-*` flag, and in subprocess-safe mode (any
non-terminal stream) it adds `--allow-all-paths` unless the caller did. So
`-p <text> --allow-all-tools`, the form Copilot's `--help` shows, works from a
script or recipe step. On a terminal, or with
`AMPLIHACK_COPILOT_NO_ALLOW_ALL=1`, none of those grants is added, and a `-p`
launch under the gateway is refused before any file is written. `-i` is not
refused: interactive Copilot asks the user before reading the file.

Errors, which name flags only:

| Input | Error |
| --- | --- |
| `codex exec` with two positional prompts | ``codex exec was given more than one positional prompt; pass one, or pass `-` and send the prompt on stdin`` |
| `codex exec` with an unknown option followed by a word | ``amplihack does not know whether Codex option <option> takes a value, so it cannot tell which word is the prompt; write it as <option>=<value>, or pass `-` and send the prompt on stdin`` |
| `codex exec -i <a> <b> ...` with no prompt | ``codex exec --image took <n> values and no prompt was found, so one of them may be the prompt; give each image its own -i, or put `--` before the prompt`` |
| `copilot -p <text>` given twice (`-p` and `--prompt` count as one flag; so do `-i` and `--interactive`) | `-p was given twice; pass it once` (the flag as written the second time) |
| `copilot -p <text>` under the gateway with no path access | `amplihack copilot -p: under the external LiteLLM gateway Copilot gets no --add-dir, so it can read the private prompt file only with path access, and this launch has none (...); add --allow-all-paths` |

## Per-session files

The passthrough rewrite and the pointer-sentence channel write the text to a
per-session file.

| Property | Value |
| --- | --- |
| Directory | a new `TempDir` with the prefix `amplihack-prompt-`, mode 0700 |
| File | created with `create_new`, mode 0600 |
| Reuse | none; every session gets a new directory |
| Removed on exit 0 | yes |
| Removed on non-zero exit | yes |
| Removed when amplihack forwards SIGINT, SIGTERM or SIGHUP | yes |
| Removed when amplihack itself is killed with SIGKILL | no; the 0700 directory is left behind |

Removal is by RAII: the `DeliveryHandle`, `Rewritten` or `prompt_guard` that
owns the directory is dropped after the child's `wait` returns. Every
`std::process::exit` in the launch path drops it explicitly first, because
`exit` does not run destructors. No sweeper removes directories left by a
SIGKILL.

## Automatic prompt delivery never selects argv

`amplihack_utils::prompt_delivery` decides the channel for every launch path
except the passthrough rewrite. The 4096-byte threshold is gone: prompt size no
longer affects the choice.

| `AMPLIHACK_PROMPT_DELIVERY` | Order tried | If nothing fits |
| --- | --- | --- |
| unset, empty or `auto` | `stdin`, `file-reference`, `tempfile` | `NoSafeChannel` error |
| `stdin` | `stdin`, `file-reference` (never `tempfile`: the two have different process contracts) | `NoSafeChannel` error |
| `tempfile` | `tempfile`, `stdin`, `file-reference` | `NoSafeChannel` error |
| `argv`, or any other unrecognised value | as `auto`, after a `tracing::warn!` that names the value | `NoSafeChannel` error |

An explicit request degrades only to channels that keep the prompt out of
argv. The warning for an unrecognised value names the variable and the value,
never the prompt:

```text
WARN amplihack_utils::prompt_delivery: unrecognised prompt-delivery mode; falling back to auto. Valid values: auto, tempfile, stdin (case-insensitive). env="AMPLIHACK_PROMPT_DELIVERY" value="argv"
```

The list of valid values does not include `argv`.

`PromptDelivery::Argv` and `DeliveryMode::Argv` remain in the API so that
`amplihack doctor` and `flag_matrix` can report a binary's argv capability.
`from_env` never returns `PromptDelivery::Argv`, and `try_select_mode` and
`deliver` treat it as `Auto`, so no spawn site can build an argv delivery.

The API:

```rust
pub struct NoSafeChannel {
    pub requested: PromptDelivery,
    pub caps: DeliveryCaps,
}

impl From<NoSafeChannel> for std::io::Error; // ErrorKind::InvalidInput

/// Picks the delivery mode, for spawns and for `amplihack doctor` alike.
/// Never returns `Argv`; a `PromptDelivery::Argv` request is treated as
/// `Auto`. The prompt's size never affects the choice. When no channel fits,
/// doctor shows "unavailable" and a spawn fails with `NoSafeChannel`.
pub fn try_select_mode(
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> Result<DeliveryMode, NoSafeChannel>;

/// Calls `try_select_mode` and returns its error as `io::Error`. Prepares
/// `cmd` for stdin or a file reference; never appends the prompt to argv.
pub fn deliver(
    cmd: &mut Command,
    prompt: &str,
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> std::io::Result<DeliveryHandle>;
```

`AUTO_TEMPFILE_THRESHOLD_BYTES` has been removed.

`NoSafeChannel`'s `Display` text is fixed and contains no prompt text.
Callers return it to their own caller; none of them retries the launch with
argv:

```text
no channel keeps this prompt out of the command line: the agent CLI accepts none of stdin, a prompt file or a file reference
```

Example: deliver a review prompt to Copilot, which has only the pointer
channel:

```rust
use std::process::Command;
use amplihack_utils::prompt_delivery::{deliver, DeliveryCaps, PromptDelivery};

let caps = DeliveryCaps::default().with_file_reference();
let prompt = std::fs::read_to_string("/var/tmp/review-prompt.md")?;
let mut cmd = Command::new("copilot");
cmd.arg("-p");
let handle = deliver(&mut cmd, &prompt, PromptDelivery::Auto, &caps)?;
if let Some(dir) = handle.tempfile_path().and_then(|p| p.parent()) {
    cmd.arg("--add-dir").arg(dir);
}
let status = cmd.status()?; // `handle` is still alive here
drop(handle);               // the file and its 0700 directory are removed now
```

The child's argv is then:

```text
copilot -p Read and follow the complete instructions in the file /tmp/amplihack-prompt-Q3x9Lk/prompt.md. Its contents are the task prompt. --add-dir /tmp/amplihack-prompt-Q3x9Lk
```

`DeliveryCaps::default()` has no channels: every field is `false` or `None`.
`llm_client` uses it for a launcher it does not recognise, so that launcher
gets `NoSafeChannel` instead of argv.

### Callers of `deliver`

Every spawn site below calls `deliver` and keeps the returned `DeliveryHandle`
alive until the child's `wait` returns. Dropping the handle earlier would
remove the file before the agent reads it.

| Crate | Call site | Channel |
| --- | --- | --- |
| `amplihack-cli` | `auto_mode/helpers.rs` | stdin (Claude, `codex exec -`), pointer sentence (Copilot) |
| `amplihack-orchestration` | `claude_process.rs` | stdin |
| `amplihack-delegation` | `platform_cli` | stdin (Claude), pointer sentence (Amplifier); `gh copilot suggest` is refused |
| `amplihack-turn` | `turn.rs` | pointer sentence |
| `amplihack-signal` | `chat/turn.rs` | through `amplihack-turn` |
| `amplihack-hooks` | `strategies/copilot_strategy.rs` (power-steer) | pointer sentence |
| `amplihack-utils` | `llm_client.rs` | stdin (Claude), pointer sentence (Copilot), `NoSafeChannel` (unknown) |
| `amplihack-launcher` | `prompt_delivery.rs` | stdin (`codex exec -`), pointer sentence (interactive Codex, Copilot, Amplifier) |

## Codex

| Launch | Channel |
| --- | --- |
| `codex exec ...` | `-` is added after the existing arguments; the prompt is written to stdin |
| interactive `codex` with a first prompt | pointer sentence (stdin is cleared from the caps at the call site, because interactive Codex reads the terminal) |
| auto-mode turn | `codex exec -`, prompt on stdin |

The Codex row in `flag_matrix.rs` sets `supports_prompt_stdin` and
`supports_prompt_file_reference` to true. Codex gets no contract injection,
because it has no append-prompt capability; that is unchanged.

## Copilot

Copilot has no stdin prompt channel and no system-prompt file flag. Every
Copilot prompt uses the pointer sentence, and the launch adds
`--add-dir <private dir>` naming only the per-prompt 0700 directory, never a
parent such as `$TMPDIR`. Copilot gets no contract, as before.

The pointer sentence is `FILE_REFERENCE_TEMPLATE`, unchanged:

```text
Read and follow the complete instructions in the file {path}. Its contents are the task prompt.
```

`{path}` is the only substitution. A path containing a newline or NUL is
rejected, so the sentence cannot be split.

## Recipe shell steps

A recipe shell step that calls an agent CLI writes the prompt to a private file
and passes the pointer sentence. The three `amplihack copilot -p` calls in
`amplifier-bundle/recipes/quality-loop.yaml` use this pattern:

```bash
PROMPT_DIR=$(umask 077; mktemp -d) \
  || { echo "coe-design-critique: could not create a private prompt directory" >&2; exit 1; }
trap 'if [ -n "$PROMPT_DIR" ] && [ -d "$PROMPT_DIR" ]; then rm -rf -- "$PROMPT_DIR"; fi' EXIT
printf '%s' "$PROMPT" > "$PROMPT_DIR/prompt.md"
if ! amplihack copilot \
    -p "Read and follow the complete instructions in the file $PROMPT_DIR/prompt.md. Its contents are the task prompt." \
    --add-dir "$PROMPT_DIR" >"$OUT_FILE"; then
  # ... existing failure handling ...
fi
```

The failure branch matches each step's existing handling: all three steps
already report a failure as a message on stderr followed by `exit 1`, and none
records it in `OUT_FILE`. Each step uses its own name in the message
(`coe-design-critique`, `quality-audit`, `coe-diff-critique`).

Rules for recipe steps:

- install the trap before writing the file, and guard it so an empty or failed
  `mktemp` never runs `rm -rf` on the wrong path;
- if the step already sets an `EXIT` trap, add the removal to that trap's
  command instead of replacing it (the `quality-loop.yaml` steps have none);
- write with `printf '%s'`, never `echo` or an unquoted heredoc;
- never set `AMPLIHACK_PROMPT_DELIVERY` in a recipe, bundle config or the
  runner's environment allowlist.

Existing grants are not widened: the two calls that already pass
`--add-dir "${TMPDIR%/}" --allow-all-tools` keep them, and add the private
directory.

## Docker launches

The Docker launch path does not inject the contract on the host. The amplihack
inside the container runs its own launch, writes its own contract file and
runs its own version gate against the CLI in the container. The host's version
gate is skipped for Docker, because the host's `claude` version says nothing
about the container's.

The hand-off itself passes the launch arguments to `docker` as they were given,
before the passthrough rewrite, because the rewrite's per-session files are
host paths the container cannot see. A `-p <text>` given to a Docker launch is
therefore in the host `docker` process's argv while it runs. It is the same
text, already in the `amplihack` process's own argv; see
[What argv still contains](#what-argv-still-contains).

## Environment variables

| Variable | Effect |
| --- | --- |
| `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1` | No contract, no contract file, no version gate for the contract. Any other value, or unset, injects |
| `AMPLIHACK_PROMPT_DELIVERY` | `auto` (default), `stdin` or `tempfile`. Any other value, including `argv`, logs a warning and is treated as `auto` |
| `AMPLIHACK_HOME` | Home that holds `private/system-prompt-append.md` |

## Errors

| Message | Cause | Fix |
| --- | --- | --- |
| `... needs Claude Code 2.1.319 or later; found 2.1.318.` | Old CLI | Upgrade Claude Code, or set `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1` |
| `... could not read a Claude Code version from '<resolved binary> --version'` | Probe printed no version, failed or timed out | Check that the named binary's `--version` runs; then as above |
| `<home>/private is not a directory owned by you` | `private/` is a symlink, a file or owned by another user | Remove it and launch again |
| `the amplihack home path contains a newline or NUL` | `AMPLIHACK_HOME` or `HOME` is malformed | Fix the variable |
| `CONTRACT_PATH_MISSING: ...` | Internal: the contract applies but no file path reached the command builder | Report a bug; this is never retried with text |
| `no channel keeps this prompt out of the command line: ...` | The binary has no stdin, file or pointer channel | Use an agent CLI that reads stdin or a file |
| `the prompt was given twice (-p/--print); pass it once` | Two `-p <text>` in passthrough | Pass one |
| `--system-prompt and --system-prompt-file were both given; pass one` | Conflicting flags | Pass one |

These strings are the spec. `no_safe_channel_message_is_stable` and the
version-gate tests assert them exactly, so a wording change edits this table
and the tests together.

All of these exit non-zero before the agent is spawned. None of them echo the
prompt, brief or contract text, in the message, a log line, a tracing span or a
panic payload.

## Behaviour changes

- `Auto` delivery no longer puts short prompts in argv.
- `AMPLIHACK_PROMPT_DELIVERY=argv` is no longer accepted; it logs a warning and
  behaves as `auto`.
- Copilot and Codex still get no system contract.
- Claude Code older than 2.1.319 fails to launch through amplihack unless
  `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1` is set.
- `amplihack claude -- -p <text>` with stdin piped from the caller used to
  concatenate the two; the child now receives only `<text>`. The recipe runner
  does not pipe stdin into Claude steps.
- A `-p` prompt that begins with `-` must use `--print=<text>`.
- `llm_client` returns an error for an unknown launcher instead of using argv.
- Auto-mode Copilot turns pass `--add-dir <private dir>`, except under the
  external LiteLLM gateway, whose launch policy forbids `--add-dir`.
- The external LiteLLM gateway pins Claude Code 2.1.247, below 2.1.319, so a
  routed Claude launch needs `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1`.
- Launcher-built interactive `codex` launches with a first prompt receive the
  pointer sentence; the 96 KiB interactive limit is gone.
- Input piped to amplihack is still passed on when the prompt moves to
  stdin. For `git diff | amplihack claude -p "review this"` and
  `git diff | amplihack codex -- exec "review this"`, when amplihack's stdin
  is a pipe, a socket or a non-empty file, the agent's stdin carries the
  prompt, a blank line, then the piped input unchanged, copied as it arrives.
  The agent sees end of input when the caller closes the pipe, as it would
  reading the pipe itself. Codex used to wrap piped input in a `<stdin>`
  block after the prompt; it now reads prompt and input as one text. A pipe
  whose writer never closes after the agent exits does not hold amplihack
  open: the copying thread is left behind and ends with the process.
- `amplihack copilot -- -p <text>` passes Copilot the pointer sentence and
  `--add-dir <private dir>` instead of the text.

## What argv still contains

These are not prompt text and stay in argv: model names, working directories,
session IDs, `--add-dir` paths, the path of the contract file, the paths of
per-session files, and the fixed pointer sentence that Copilot and interactive
Codex receive. The pointer sentence is a constant plus a path; the tests check
that it holds no sentinel phrase and no `tail -f /dev/null`.

Six cases keep text in a process's argv and are documented exceptions:

| Case | Why | Tracking |
| --- | --- | --- |
| The `amplihack claude` wrapper started by a recipe agent step | The recipe runner puts the brief in the wrapper's argv. The agent CLI child below it is clean. The runner change, [rysweet/amplihack-recipe-runner#205](https://github.com/rysweet/amplihack-recipe-runner/issues/205), starts the wrapper with a bare `-p`, the brief on stdin and `--system-prompt-file`; the wrapper is clean once `RECIPE_RUNNER_REV` is updated to include it | [rysweet/amplihack-recipe-runner#205](https://github.com/rysweet/amplihack-recipe-runner/issues/205) (runner); [#2115](https://github.com/rysweet/amplihack-rs/issues/2115) (pin update, test, removal of this row) |
| A Docker launch, `amplihack claude --docker -p '<text>'` | The host `docker` process receives the arguments before the rewrite, because the per-session files are host paths the container cannot see. The text is already in the caller's own `amplihack` argv | Accepted risk; a fix needs the private file mounted into the container |
| `amplihack claude -- '<prompt>'` (interactive first prompt) | The text is in the wrapper's own argv, which the caller supplied, and interactive Claude cannot read its first prompt from stdin | [#2116](https://github.com/rysweet/amplihack-rs/issues/2116) (interactive `--prompt-file`) |
| `amplihack codex -- '<prompt>'` (interactive first prompt) | The text is in the wrapper's own argv, which the caller supplied. The first positional of a bare `codex` may be a subcommand, and that list changes between Codex releases, so the rewrite cannot tell a prompt from a subcommand. `amplihack codex -- exec <text>` and `amplihack codex -- review <text>` are rewritten | [#2116](https://github.com/rysweet/amplihack-rs/issues/2116) |
| `amplihack codex -- resume <session> '<prompt>'` and `amplihack codex -- fork <session> '<prompt>'` (top-level, interactive) | The text is in the wrapper's own argv, which the caller supplied. Both start an interactive Codex session whose terminal UI owns stdin, and their help (codex-cli 0.166.0) offers no `-` or stdin form for the prompt, so there is no other channel. The non-interactive `exec resume` and `exec fork` are rewritten | [#2116](https://github.com/rysweet/amplihack-rs/issues/2116) |
| `amplihack amplifier -- run <prompt>` | The text is in the wrapper's own argv, which the caller supplied. Upstream documents only `amplifier run [OPTIONS] [PROMPT]`, with no option arity table, so the rewrite cannot find the prompt safely. No bundled recipe uses this form; amplihack's own Amplifier launches (auto mode, `platform_cli`) use the pointer sentence | Accepted; use `--auto --prompt-file` |

To start an interactive or auto-mode session without prompt text in any argv,
use `--prompt-file` with `--auto`; see
[Large Payload Delivery Reference](./large-payload-delivery.md#--prompt-file).

## Tests

| Test file | What it checks |
| --- | --- |
| `crates/amplihack-cli/tests/issue_2030_no_prompt_in_argv.rs` | Each launch path with a stub agent on `PATH`: argv has no sentinel phrase and no `tail -f /dev/null`; each `*-file` path is a regular 0600 file in a 0700 directory and holds the sentinel; stdin payloads arrive on stdin; per-session files are gone after exit 0, exit 1 and SIGTERM; version gate cases 2.1.318, unparseable and missing; contract file repair for 0644, wrong content and a symlink (the symlink target is unchanged); `private/` as a symlink or foreign-owned fails; a home path with a newline is rejected; no error path echoes the sentinel. The passthrough forms run through the real binary: `amplihack codex -- exec <text>` (plain, with `--json`, and after `--`) gives Codex argv ending in `-`, the text on stdin, and a `/proc/<pid>/cmdline` without the text; `exec -` is unchanged; two prompts fail before any spawn; `exec review --commit <sha>` is unchanged, while `exec review --base main <text>` and `exec fork <id> -i a.png <text>` end in `-` with the text on stdin; a caller's piped stdin reaches the agent after the prompt and a blank line, for Claude `-p` and `codex exec`, and a null stdin adds nothing; under the gateway, `-p` with `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` is refused before any spawn, and `-p --allow-all-tools` launches with `--allow-all-paths`, no `--add-dir`, and a pointer file the stub can read; `amplihack copilot -- -p`, `--prompt` and `-i` give the pointer sentence and an `--add-dir` naming only its private directory. `auto_mode_codex_turn_reads_stdin` and `auto_mode_copilot_turn_uses_pointer` cover only the `--auto --prompt-file` path |
| `crates/amplihack-cli/tests/issue_2030_guards.rs` | Fails CI if any production code calls a `select_mode(` other than `try_select_mode(` (the old selector that answered `Argv` is removed), if any code under `crates/` builds `--append-system-prompt` followed by text, if any bundled recipe passes inline text to `-p` or `exec`, if a bundle file sets `AMPLIHACK_PROMPT_DELIVERY`, or if a `quality-loop.yaml` Copilot call lacks its private directory and `EXIT` trap |
| `crates/amplihack-cli/src/commands/launch/tests_issue_2030_contract.rs` | A 0644 contract file is repaired to 0600; a symlink is replaced and its target left unchanged; a foreign-owned `private/` is refused; two concurrent first launches both end with one complete file |
| `crates/amplihack-cli/src/commands/launch/tests_passthrough_rewrite.rs` | Every row of the rewrite table; repeated flags are errors; `format!("{:?}", rewritten)` does not contain the sentinel |
| `crates/amplihack-cli/src/commands/launch/tests_codex_exec_args.rs` | Each Codex option table equals the options listed in the saved codex-cli 0.166.0 help for its scope (root, `exec`, `exec review`, `exec resume`, `exec fork`, top-level `review`), with arity; `--image` is variadic only under plain `exec`; clap's attached-value, lone-`-` and `--` rules; unknown options followed by a word and missing values are refused without echoing the word |
| `crates/amplihack-cli/src/commands/launch/tests_codex_copilot_rewrite.rs` | Every row of the Codex and Copilot passthrough tables: `exec <text>`, its `review`, `resume`, `fork` and `--` forms, and top-level `review <text>` put `-` in place of the text and the text in the payload; option values are not prompts, including `exec review --base/--commit/--title`, `--cyber-access-program`, and the single `-i` value under `exec fork` and `exec resume`; an ambiguous variadic `--image` or unknown option is an error that names no text; on a terminal `--allow-all-tools` gives no path access and a gateway `-p` launch is refused; `exec -`, interactive Codex, top-level `resume` and `fork`, and other subcommands are unchanged; two prompts are an error that names no text; Copilot pointer files are 0600 in a 0700 directory named by `--add-dir`, removed on drop |
| `crates/amplihack-cli/src/commands/agent/tests_cmdline_issue_2030.rs` | The `/proc` argv parser recognises bare `-p` with `*-file` flags as print mode |
| `crates/amplihack-utils/tests/prompt_delivery_issue_2030.rs` | `Auto` never returns `Argv` at any size; `AMPLIHACK_PROMPT_DELIVERY=argv` maps to `auto` and the prompt is not in argv; `NoSafeChannel` text is stable |
| `crates/amplihack-launcher/tests/prompt_delivery_issue_2030.rs` | `codex exec -` gets stdin; interactive Codex, Copilot and Amplifier get the pointer sentence and `--add-dir` for the private directory only. The old 96 KiB and "requires terminal stdin" tests are rewritten as pointer-delivery tests |
| `crates/amplihack-delegation/tests/platform_cli_issue_2030.rs` | Each delegate's argv holds no sentinel |
| `crates/amplihack-remote/src/script/issue_2030_tests.rs` | The remote script passes `--prompt-file` and creates the file under `umask 077` |
| `crates/amplihack-turn/src/turn.rs` (unit test) | `build_turn_argv` returns argv with no message text: `-p` is followed by the pointer sentence, `--add-dir` names only the per-turn 0700 directory, and the file is gone after the turn. The `no_argv_append_text` guard does not catch `-p <text>`, so this test is the one that covers the Signal chat turn |
| `crates/amplihack-cli/tests/system_prompt_append_fragment.rs` | Updated. The assertion at line 125 that the contract header contains "process table" or "visible" is replaced, because the new header says neither; it now checks that the header names `--append-system-prompt-file` |
| `STUB` in `issue_2030_no_prompt_in_argv.rs` | The stub `claude`, `codex` and `copilot`, written into each test's isolated `PATH`. Driven by `STUB_VERSION`, `STUB_EXIT` and `STUB_SLEEP`; logs argv, a copy of its own `/proc/$$/cmdline`, stdin, and `stat` of every path after a `*-file` or `--add-dir` flag and in a pointer sentence |

Every test runs under `isolated(..)` and never touches the real `~/.claude`,
`~/.amplihack` or profile root. The `/proc` argv parsers (`agent/cmdline.rs`,
the governor, `auto_switch/capacity.rs` and `limit_wait/claude_args.rs`) are
tested to still recognise a bare `-p` or `--print` and the `*-file` flags with
their values.

## Related

- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md) - The incident and the threat model
- [Check Agent Processes for Prompt Text](../howto/check-agent-argv-for-prompt-text.md) - The `/proc` check and fixes for old CLIs
- [System Prompt Append](../SYSTEM_PROMPT_APPEND.md) - What the contract says and when it is injected
- [Large Payload Delivery Reference](./large-payload-delivery.md) - `prompt_delivery`, stdin writers and `--prompt-file`
- [Environment variables](./amplihack-env-vars.md) - Every `AMPLIHACK_*` variable
