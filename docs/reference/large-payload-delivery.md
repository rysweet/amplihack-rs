---
title: "Large Payload Delivery Reference"
description: "API, constants, per-tool channels and shell helpers that keep large prompts, recipe context and GitHub bodies out of single argv elements and environment values."
last_updated: 2026-10-17
review_schedule: quarterly
owner: rysweet
doc_type: reference
related:
  - ../concepts/large-payload-delivery.md
  - ../howto/pass-large-recipe-context.md
  - ../amplihack-rs-parity.md
  - ./recipe-cli-reference.md
  - ./profile-command.md
---

# Large Payload Delivery Reference

Reference for the shared layer that delivers text of unbounded size to child
processes without exceeding Linux `MAX_ARG_STRLEN` (131,072 bytes per argv or
environment string) or `ARG_MAX`. For the reasoning behind it, see
[Large Payload Delivery to Child Processes](../concepts/large-payload-delivery.md).

## Contents

- [Delivery modes](#delivery-modes)
- [Per-tool channels](#per-tool-channels)
- [Rust API: `amplihack_utils::prompt_delivery`](#rust-api-amplihack_utilsprompt_delivery)
- [Rust API: `amplihack_utils::payload_delivery`](#rust-api-amplihack_utilspayload_delivery)
- [Rust API: `amplihack_cli::util::run_output_with_timeout_stdin`](#rust-api-amplihack_cliutilrun_output_with_timeout_stdin)
- [Auto mode](#auto-mode)
- [Fleet reasoner bounds](#fleet-reasoner-bounds)
- [Remote and admiral prompt hand-off](#remote-and-admiral-prompt-hand-off)
- [Recipe context from files](#recipe-context-from-files)
- [Nested recipe environment](#nested-recipe-environment)
- [Reading a context value in a bash step](#reading-a-context-value-in-a-bash-step)
- [Shell helper: `workflow_body_file.sh`](#shell-helper-workflow_body_filesh)
- [Recipe and shell sites](#recipe-and-shell-sites)
- [Claude profiles and auto-switch](#claude-profiles-and-auto-switch)
- [Security contract](#security-contract)
- [Sites that still use argv](#sites-that-still-use-argv)

## Delivery modes

`DeliveryMode` is the mode that `prompt_delivery` selects after considering
the requested mode and the target tool's capabilities.

| Mode | Name in reports | What the child receives |
| --- | --- | --- |
| `Argv` | `argv` | The prompt as one argv element |
| `Tempfile` | `tempfile` | The tool's prompt-file flag followed by the path of a 0600 temporary file |
| `Stdin` | `stdin` | The prompt bytes on stdin, which is closed after the last byte |
| `FileReference` | `file-reference` | One short argument naming a 0600 temporary file that holds the prompt |

### Automatic selection

With `AMPLIHACK_PROMPT_DELIVERY` unset, empty or `auto`:

| Prompt size | Order tried |
| --- | --- |
| 4096 bytes or fewer | `argv`, then the order below if argv is unsupported |
| More than 4096 bytes | `tempfile`, `stdin`, `file-reference`, `argv` |

Selecting `argv` for a prompt above 4096 bytes logs a `warn!` that contains
the prompt size and the selected mode only.

### Explicit requests

`AMPLIHACK_PROMPT_DELIVERY` accepts `auto`, `argv`, `tempfile` and `stdin`.
`file-reference` is not a request value. It is selected only by `auto` or by
the degrade chain.

| Request | Behaviour when unsupported |
| --- | --- |
| `argv` | Degrade through `tempfile`, `stdin`, `file-reference`, `argv` |
| `tempfile` | Degrade through `stdin`, `file-reference`, `argv` |
| `stdin` | Degrade through `file-reference`, `argv` |

Amplifier keeps its binary-specific rule: an explicit `tempfile` or `stdin`
request fails before launch. See [Amplifier Command](./amplifier-command.md#prompt-delivery).

### File-reference argument

The argument is built from a fixed template, with only the temporary file's
path substituted. The path contains no newline, and the argument is under 512
bytes.

```text
Read and follow the complete instructions in the file /tmp/.tmpQ3x9Lk. Its contents are the task prompt.
```

## Per-tool channels

Capabilities are defined in `crates/amplihack-launcher/src/flag_matrix.rs`
and checked by the test `prompt_delivery_caps_match_verified_cli_contracts`.

| Tool | `argv` | `tempfile` | `stdin` | `file-reference` | Large prompt channel |
| --- | --- | --- | --- | --- | --- |
| Claude Code | yes | no | yes | no | `stdin` |
| Codex | yes | no | see note | no | `stdin` (`codex exec`) |
| GitHub Copilot CLI | yes | no | no | yes | `file-reference` |
| Microsoft Amplifier | yes | no | no | yes | `file-reference` |

Codex keeps its binary-specific contract in
`crates/amplihack-launcher/src/prompt_delivery.rs`: `codex exec` receives the
prompt on stdin, and interactive Codex keeps a positional prompt. Its
`flag_matrix.rs` entry is unchanged.

`DeliveredCommand::stdin_payload` is `Some` if and only if the selected mode is
`Stdin`. Every caller of `build_tool_command_with_prompt_delivery` that can
route to Claude writes `stdin_payload` to the child. Auto mode, `llm_client`
and `claude_process` are each covered by a test that asserts this.

## Rust API: `amplihack_utils::prompt_delivery`

```rust
pub enum DeliveryMode {
    Argv,
    Tempfile,
    Stdin,
    FileReference,
}

pub struct DeliveryCaps {
    pub supports_argv: bool,
    pub supports_tempfile: bool,
    pub supports_stdin: bool,
    pub supports_file_reference: bool,
    pub tempfile_flag: Option<&'static str>,
}

impl DeliveryCaps {
    pub fn argv_only() -> Self;
    pub fn argv_and_tempfile(flag: &'static str) -> Self;
    pub fn all_modes(tempfile_flag: &'static str) -> Self;
    /// Returns a copy with `supports_stdin` set.
    pub const fn with_stdin(self) -> Self;
    /// Returns a copy with `supports_file_reference` set.
    pub const fn with_file_reference(self) -> Self;
}

/// Fixed sentence used for file-reference delivery. `{path}` is the only
/// substitution.
pub const FILE_REFERENCE_TEMPLATE: &str;
/// The text before `{path}` in `FILE_REFERENCE_TEMPLATE`.
pub const FILE_REFERENCE_PREFIX: &str;

pub const AUTO_TEMPFILE_THRESHOLD_BYTES: usize = 4096;
pub const ENV_VAR_NAME: &str = "AMPLIHACK_PROMPT_DELIVERY";

pub fn select_mode(
    requested: PromptDelivery,
    prompt_size: usize,
    caps: &DeliveryCaps,
) -> DeliveryMode;

pub fn deliver(
    cmd: &mut Command,
    prompt: &str,
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> std::io::Result<DeliveryHandle>;
```

`PromptDelivery` (the requested mode) is unchanged.

### `DeliveryHandle` lifetime

`DeliveryHandle` owns the temporary file for `Tempfile` and `FileReference`
modes. Dropping it deletes the file. Keep the handle alive until the child's
`wait` has returned, for example by binding it in the same scope as the
`wait` call:

```rust
use std::process::Command;
use amplihack_utils::prompt_delivery::{deliver, DeliveryCaps, PromptDelivery};

let caps = DeliveryCaps::argv_only().with_file_reference();
let prompt = std::fs::read_to_string("/var/tmp/review-prompt.md")?;
let mut cmd = Command::new("copilot");
cmd.arg("-p");
let handle = deliver(&mut cmd, &prompt, PromptDelivery::Auto, &caps)?;
let status = cmd.status()?; // `handle` is still alive here
drop(handle);               // the temporary file is deleted now
```

`DeliveryHandle::tempfile_path()` returns `Some` in `Tempfile` and
`FileReference` modes, and `None` otherwise.

## Rust API: `amplihack_utils::payload_delivery`

```rust
/// Starts a thread that writes `payload` to `stdin` and then closes it.
pub fn spawn_stdin_writer(stdin: ChildStdin, payload: Vec<u8>) -> StdinWriter;

impl StdinWriter {
    /// Joins the writer thread.
    pub fn finish(self) -> std::io::Result<()>;
}

/// Writes `chunks` in order to a tokio child's stdin, then flushes, shuts
/// down and drops it.
pub async fn write_stdin_async(
    stdin: tokio::process::ChildStdin,
    chunks: &[&[u8]],
) -> std::io::Result<StdinDelivery>;

pub enum StdinDelivery {
    /// Every byte was written and stdin was closed.
    Complete,
    /// The child closed its end before reading everything.
    ReaderClosed,
}

/// Keeps the first `max` bytes, cut back to a UTF-8 boundary.
pub fn truncate_head_bytes(s: &str, max: usize) -> Cow<'_, str>;

/// Keeps the last `max` bytes, cut forward to a UTF-8 boundary.
pub fn truncate_tail_bytes(s: &str, max: usize) -> Cow<'_, str>;

/// Keeps the tail behind `TRUNCATION_MARKER`; the result, marker included,
/// is at most `max` bytes.
pub fn truncate_tail_with_marker(s: &str, max: usize) -> Cow<'_, str>;

pub const TRUNCATION_MARKER: &str = "[... earlier output truncated ...]\n";
```

### `StdinWriter::finish` results

| Outcome | Result |
| --- | --- |
| All bytes written and stdin closed | `Ok(())` |
| Child closed stdin first (`BrokenPipe` or `ConnectionReset`) | `Ok(())` |
| Writer thread panicked | `Err(io::Error)` of kind `Other` |
| Any other write error | `Err` with the original kind |

### `write_stdin_async` results

| Outcome | Result |
| --- | --- |
| All chunks written, flushed and stdin shut down | `Ok(StdinDelivery::Complete)` |
| Child closed stdin first (`BrokenPipe` or `ConnectionReset`) | `Ok(StdinDelivery::ReaderClosed)` |
| Any other write, flush or shutdown error | `Err` with the original kind |

The chunks are written back to back with no separator, so
`&[api_key, b"\n", prompt]` sends the key line followed by the prompt. Passing
several slices avoids copying a large prompt into one buffer.

`ReaderClosed` is not a failure by itself. The caller decides what it means:
`claude_process` waits a short grace period for the child to exit and then
reports the child's own status, and `amplihack-remote` reports the `azlin`
exit status.

Error messages from both writers never contain payload bytes.

### Which writer each site uses

| Site | Runtime | Writer |
| --- | --- | --- |
| `amplihack_cli::util::run_output_with_timeout_stdin` | `std::process` | `spawn_stdin_writer` |
| `amplihack_utils::llm_client` | `std::process` | `spawn_stdin_writer` |
| `amplihack-orchestration` `claude_process` | tokio | `write_stdin_async(stdin, &[&payload])` |
| `amplihack-remote` `executor` | tokio | `write_stdin_async(stdin, &[api_key, b"\n", prompt])` |

No other site writes a payload to a child's stdin.

### Truncation rules

| Input | `truncate_head_bytes` / `truncate_tail_bytes` | `truncate_tail_with_marker` |
| --- | --- | --- |
| `s.len() <= max` | `Cow::Borrowed(s)` | `Cow::Borrowed(s)` |
| `max == 0` | `""` | `""` |
| `max` smaller than the marker | Truncated text | Bare tail of `max` bytes, no marker |
| Cut point inside a multi-byte character | Cut moves so the whole character is dropped | Same |

The result is always valid UTF-8 and never longer than `max` bytes.

```rust
use amplihack_utils::payload_delivery::truncate_tail_with_marker;

let log = "é".repeat(40_000); // 80,000 bytes
let kept = truncate_tail_with_marker(&log, 32 * 1024);
assert!(kept.len() <= 32 * 1024);
assert!(kept.starts_with("[... earlier output truncated ...]\n"));
```

## Rust API: `amplihack_cli::util::run_output_with_timeout_stdin`

```rust
pub fn run_output_with_timeout_stdin(
    cmd: Command,
    timeout: Duration,
    stdin_payload: Option<Vec<u8>>,
) -> anyhow::Result<Output>;
```

| Behaviour | Detail |
| --- | --- |
| Payload `Some` | stdin is piped; `spawn_stdin_writer` starts after spawn |
| Payload `None` | stdin is `Stdio::null()` (this entry point only) |
| Child closed stdin first | `BrokenPipe` or `ConnectionReset`; the child's exit status is returned unchanged |
| Any other writer error | `Err` naming only the program and the error kind, never argv or payload bytes |
| Writer still running 2 s after the child exits | Logged at `warn` and detached; the child's exit status decides |
| Timeout | The child is killed; the writer thread is not joined, as with the output readers |

`run_output_with_timeout` and `run_output_with_timeout_limited` keep their
existing behaviour.

## Auto mode

### Prompt delivery per turn

Each auto-mode turn builds its command through the shared layer and runs it
with `run_output_with_timeout_stdin`. Claude receives turn prompts above
4096 bytes on stdin; Copilot and Amplifier receive a file reference.
`AMPLIHACK_NONINTERACTIVE=1` is set on the child whenever a stdin payload is
present, so a nested `amplihack claude` does not read the payload as
interactive input.

### `--prompt-file`

`--prompt-file <PATH>` is an option of the `claude`, `copilot`, `codex` and
`amplifier` launch subcommands. It reads the initial auto-mode prompt from a
file and is accepted only together with `--auto`. It is a subcommand option,
so it goes before `--`, not among the arguments passed through to the agent:

```bash
amplihack claude --auto --max-turns 20 --prompt-file /var/tmp/task.md
amplihack copilot --auto --prompt-file=/var/tmp/task.md
```

| Case | Result |
| --- | --- |
| `--prompt-file` without `--auto` | Usage error from the argument parser, exit 2 |
| Option given without a path | Usage error from the argument parser, exit 2 |
| `--prompt-file` and a `-p` after `--` | The file is used; a warning names the ignored `-p` |
| File missing or unreadable | Exit 1 with a message naming the path |
| File empty after trimming | Exit 1 with a message naming the path |

`--prompt-file` exists only for auto mode. The launch subcommands have no
`--ultrathink`, `--analyze` or `--fix` option, so there is no prompt-file form
for those modes.

An amplihack release without this option rejects it with the argument
parser's usage error, exit 2:

```text
error: unexpected argument '--prompt-file' found
```

That error, rather than a run that starts with the wrong prompt, is the
reason the option is a subcommand option. An older release treats any text
after `--` as agent arguments, so `-- --prompt-file /var/tmp/task.md` would
start auto mode with the path itself as the prompt.

### Turn output caps

| Constant | Value | Applies to |
| --- | --- | --- |
| `MAX_OBJECTIVE_BYTES` | 32 KiB | Objective carried from turn 1 |
| `MAX_PLAN_BYTES` | 32 KiB | Plan carried from turn 2 |
| `MAX_EVALUATION_BYTES` | 32 KiB | Latest evaluation carried to the next turn |

Each value is capped with `truncate_tail_with_marker`.

## Fleet reasoner bounds

The fleet reasoner bounds its input in `to_prompt_context`, so a cached tmux
capture is bounded as well.

| Constant | Value | Rule |
| --- | --- | --- |
| `REASONER_CAPTURE_LINES` | 2000 | `azlin tmux capture-pane -p -S -2000` |
| `REASONER_SCROLLBACK_MAX_BYTES` | 48 KiB | Keep the tail of the capture |
| `REASONER_TRANSCRIPT_MAX_MESSAGES` | 200 | Keep the most recent messages |
| `REASONER_TRANSCRIPT_MESSAGE_MAX_BYTES` | 4 KiB | Keep the head of each message |
| `REASONER_TRANSCRIPT_MAX_BYTES` | 64 KiB | Keep the tail of the joined transcript |

The scrollback section of the prompt is labelled `recent scrollback (bounded)`.
`reasoner_command(path, decision)` returns the argv
`["--dangerously-skip-permissions", "-p"]`, and the prompt is written to
stdin. The argv gains no other permission flags.

## Remote and admiral prompt hand-off

### `amplihack-remote`

The remote command is `amplihack claude --<command> --max-turns <n>`. Only
`auto` has a prompt-file form, so `build_remote_script` and
`build_tmux_script` accept `command == "auto"` and return a validation error
for any other command, before anything is sent to the VM:

```text
remote command 'ultrathink' is not supported: only 'auto' accepts --prompt-file
```

Other commands, such as `ultrathink`, `analyze`, `fix` or a recipe name, already
failed on the VM before this change, because `amplihack claude` has no such
option. They now fail locally with the message above.

The remote script reads its stdin in two parts: the API key on the first line,
then the prompt up to end of file. It saves the prompt before running any
other command, so no later command can consume stdin.

```bash
set -e
IFS= read -r ANTHROPIC_API_KEY
export ANTHROPIC_API_KEY
PROMPT_FILE=$(umask 077; mktemp)
trap 'rm -f -- "$PROMPT_FILE"' EXIT
cat > "$PROMPT_FILE"
# ... unpack context.tar.gz and clone the workspace, as before ...
amplihack claude --auto --max-turns 10 --prompt-file "$PROMPT_FILE"
```

The tmux variant cannot use an `EXIT` trap. `tmux new-session -d` returns as
soon as the session exists, so a trap in the outer script would delete the
file before the agent reads it. The removal therefore goes inside the tmux
command string, and the outer script removes the file only if
`tmux new-session` itself fails:

```bash
set -e
IFS= read -r ANTHROPIC_API_KEY
export ANTHROPIC_API_KEY
PROMPT_FILE=$(umask 077; mktemp)
cat > "$PROMPT_FILE"
tmux new-session -d -s sess-1 \
  "cd ~/workspace && amplihack claude --auto --max-turns 10 --prompt-file '$PROMPT_FILE'; rm -f -- '$PROMPT_FILE'" \
  || { rm -f -- "$PROMPT_FILE"; exit 1; }
```

`$PROMPT_FILE` is expanded by the outer shell. It is a `mktemp` path, so it
contains no quote or shell metacharacter. The session name has already passed
`validate_session_id`.

The prompt file never contains the key. The prompt is not embedded in the
script, in base64 or otherwise, and there is no argv fallback.

### Fleet admiral `start_agent`

`start_agent` launches `amplihack <agent> --auto --max-turns <n>` in a new
tmux session on a VM. Its mode allowlist is reduced to `auto`, the only mode
with a prompt-file form. A task with mode `ultrathink` is refused with the
existing `ERROR: Invalid agent mode: "ultrathink"` result; it already failed on
the VM before this change.

The prompt is sent in two stages:

| Stage | What runs on the VM | Payload channel |
| --- | --- | --- |
| 1 | `umask 077; f=$(mktemp); cat > "$f"; printf '%s\n' "$f"` | Prompt on `azlin connect` stdin |
| 2 | `tmux new-session -d -s '<session>' && tmux send-keys -t '<session>' "amplihack '<agent>' --auto --max-turns '<n>' --prompt-file '<path>'; rm -f -- '<path>'" C-m` | Path only |

The format string must be `'%s\n'` in single quotes. Unquoted, the remote
shell turns `%s\n` into `%sn`, and the printed path ends in `n`.

The last non-empty line printed by stage 1 is taken as the path, so banner
text the remote shell prints first (a `.bashrc` echo, a message of the day) is
ignored. That line must match `^/[A-Za-z0-9._/-]+$` and must not contain `..`;
otherwise `start_agent` returns an error before stage 2.

In stage 2 the single-quoted values sit inside the double-quoted `send-keys`
argument, so the outer shell would expand `$`, a backquote or `\` in them. That
is safe only because every value is validated before it is formatted:

| Value | Validation |
| --- | --- |
| `<agent>` | Allowlist: `claude`, `amplifier`, `copilot` |
| mode | Allowlist: `auto` |
| `<n>` | Integer from 1 to 1000 |
| `<path>` | `^/[A-Za-z0-9._/-]+$`, no `..` |
| `<session>` | `validate_session_name` |

None of these values can contain a quote, `$`, a backquote or `\`. The prompt
is never part of the stage 2 command, so a prompt such as
`'; touch /tmp/pwned; '` reaches the remote file as text and is never
interpreted by a shell.

The prompt file is removed when the agent command returns. If stage 2 fails,
`start_agent` makes one more `azlin connect` call to run `rm -f -- '<path>'`
and then reports the stage 2 error. A failure of that clean-up call is logged
at `warn` with the path only.

## Recipe context from files

| Option | Value | Repeatable |
| --- | --- | --- |
| `--context-file <PATH>` | A file holding one JSON object | No |
| `--context-from-file <KEY>=<PATH>` | A file whose raw UTF-8 contents become the value of `KEY` | Yes |

Precedence, lowest to highest: `--context-file`, `--context-from-file`,
`-c`. `infer_missing_context` then fills keys that are still empty. The merged
context is passed to the runner unchanged as `--set KEY=VALUE` arguments, the
only context input `recipe-runner-rs` has (it has no `--context-file` option).

A `KEY=VALUE` pair longer than one argv string (131072 bytes,
`MAX_ARG_STRLEN`) is refused before the runner starts, with exit 1 and a message
naming the key and size. The pairs together may exceed 128 KiB; their size is
taken out of the context environment mirror's budget so argv and envp stay
under `ARG_MAX`. Values that grow past that size inside a run, such as step
outputs, are not affected; see
[Values the runner spills](#values-the-runner-spills).

The runner's argv is readable by other local users through `ps` and
`/proc/<pid>/cmdline`, so every context value is visible on the host for the
length of the run. Recipe context is not a channel for secrets. The file
options only keep values out of `amplihack`'s own argv and out of shell history.

`--context-file` here is the `amplihack recipe run` option. The runner at the
revision pinned in `claude-plugin/recipe-runner.rev` (`4d5187e`) has no option
of that name, and `amplihack` never passes one to it.

### JSON value conversion

| JSON value | Context string |
| --- | --- |
| string | The string itself |
| `null` | `""` |
| `true` / `false` | `"true"` / `"false"` |
| number | Its JSON text, for example `"3"` or `"0.85"` |
| array or object | Compact JSON, for example `{"branch":"fix/1570"}` |

### Validation

| Check | Applies to | Error |
| --- | --- | --- |
| File missing or unreadable | Both options | Exit 1, names the path |
| Invalid JSON | `--context-file` | Exit 1, names the path |
| Top level not an object | `--context-file` | Exit 1, names the path |
| Key empty or not `[A-Za-z0-9_]+` | Both options | Exit 1, names the key |
| Key starts with `AMPLIHACK_` | Both options | Exit 1, names the key |
| Missing `=` in `KEY=PATH` | `--context-from-file` | Exit 1, shows the argument |
| Value contains NUL | Both options | Exit 1, names the key |
| File larger than 16 MiB | Both options | Exit 1, names the path and size |
| File not valid UTF-8 | Both options | Exit 1, names the path |

No error message contains file contents. These messages are fixed text, quoted
in [Pass Large Recipe Context](../howto/pass-large-recipe-context.md#troubleshooting)
and asserted by tests:

| Check | Message |
| --- | --- |
| Top level not an object | `--context-file <PATH>: top-level JSON value must be an object` |
| Key starts with `AMPLIHACK_` | `--context-from-file: key '<KEY>' is reserved` (or `--context-file: ...`) |
| Missing `=` in `KEY=PATH` | `--context-from-file: expected KEY=PATH` |

See [Recipe CLI Reference](./recipe-cli-reference.md#amplihack-recipe-run) for
the full option list, and [Pass Large Recipe Context](../howto/pass-large-recipe-context.md)
for worked examples.

## Nested recipe environment

When `amplihack recipe run` builds the runner command, it removes inherited
names before adding its own context. One predicate, `is_stripped_inherited_name`,
decides what is removed, and `inherited_env_bytes()` uses the same predicate
so removed variables do not count against the
[aggregate environment budget](./recipe-context-environment.md#aggregate-environment-budget).

| Inherited name | Removed |
| --- | --- |
| `RECIPE_VAR_<key>` | Yes |
| Uppercase alias of `<key>`, when `<key>` contains no `__` | Yes |
| `AMPLIHACK_CONTEXT_FILE` | Yes |
| Any name on `RESERVED_ENV_DENYLIST` (for example `PATH`, `HOME`) | Never |
| Any other `AMPLIHACK_*` name | Never |
| A name that is not a valid environment identifier | Never |

`RECIPE_VAR_path=/x` in the parent therefore removes `RECIPE_VAR_path` but not
`PATH`.

`AMPLIHACK_CONTEXT_FILE` is the one `AMPLIHACK_*` name that is removed. It
names the parent run's context file, and a nested step that found it would
read the parent's values in place of its own; see
[Reading a context value in a bash step](#reading-a-context-value-in-a-bash-step).

## Reading a context value in a bash step

### Where a step finds its context

The table describes the recipe runner pinned in
`claude-plugin/recipe-runner.rev` (`4d5187e`) and later runners.

| Channel | Set by | Present when |
| --- | --- | --- |
| `RECIPE_VAR_<key>` | The recipe runner, for each bash step | Always, at any size. Nested object keys also appear as `RECIPE_VAR_<a>__<b>`. A large value is a shell-local variable rather than an exported one; see [Values the runner spills](#values-the-runner-spills). |
| Uppercase alias, such as `TASK_DESCRIPTION` | The recipe runner, for each bash step | The key is top level, its value is a scalar, and the alias is not a reserved name. Spilled under the same rules as `RECIPE_VAR_<key>`. |
| `AMPLIHACK_CONTEXT_FILE` | Runners older than the pin only | Never set by the pinned runner. Older runners set it, in place of most `RECIPE_VAR_*` variables, when the whole context did not fit their environment budget. |

`amplihack` also exports uppercase aliases of up to 96 KiB
(`CONTEXT_ENV_VALUE_MAX_BYTES`) into the runner's own environment once per
run. A bash step sees the runner's value for the same name, so this cap does
not limit what a step reads.

Read context values in the step's own shell: `"$RECIPE_VAR_cycle_history"`,
or `write_context_value` from a sourced helper. A child process the step
starts, such as `bash other.sh` or `python3 tool.py`, inherits only exported
variables, so it sees an empty value once the runner has spilled it.

### Shell helper: `recipe_context_value.sh`

`amplifier-bundle/tools/recipe_context_value.sh` writes one context value to a
file, whichever channel holds it:

```bash
write_context_value <KEY> <OUT>
```

Source the script, which defines `write_context_value`, and call the
function from the step's shell. It needs `bash` and `jq`. An `<OUT>` of `-`
writes the value to stdout, for short values captured with `$(...)`.

```bash
. "$helper_dir/recipe_context_value.sh"
write_context_value cycle_history "$tmp/cycle_history.txt"
```

Running the script as its own process (`bash recipe_context_value.sh KEY OUT`)
is still accepted, but that process cannot see a value the runner spilled,
because spilled values are not exported. The bundled recipes always source
it; see [Helpers that must be sourced](#helpers-that-must-be-sourced).

| Order | Source | Rule |
| --- | --- | --- |
| 1 | `RECIPE_VAR_<name>` | Used when set, even if empty. `<name>` is `<KEY>` with `.` replaced by `__` and `-` by `_`. |
| 2 | `AMPLIHACK_CONTEXT_FILE` | Fallback for runners older than the pin. Used when `RECIPE_VAR_<name>` is unset. `jq` reads the path given by splitting `<KEY>` on `.`. A string is written as raw text; any other JSON value is written as compact JSON. The key itself is passed with `--arg`. |
| 3 | None | Exit 3. An absent key is never written as an empty file. |

| Exit code | Meaning |
| --- | --- |
| 0 | `<OUT>` holds the complete value |
| 1 | Read or write failure, including an unreadable context file |
| 2 | Usage error: a missing argument, or `<KEY>` not matching `^[A-Za-z0-9_][A-Za-z0-9_.-]*$` |
| 3 | The key is in neither channel |

The exit 3 message names the key and the channels checked, never a value:

```text
write_context_value: context key 'cycle_history' is not available (RECIPE_VAR_cycle_history is unset and AMPLIHACK_CONTEXT_FILE is not set)
```

The helper writes a file `<OUT>` through a `mktemp` file under `umask 077` and
moves it into place, uses no `eval`, and never places the value in argv.

Use it in place of a `{{key}}` placeholder inside a quoted heredoc as well;
see [Known limits](../concepts/large-payload-delivery.md#known-limits) for why
such a placeholder is unsafe.

### Values the runner spills

The pinned runner keeps a bash step's spawn under the kernel limits by moving
recipe values out of the environment:

| Value | What the runner does |
| --- | --- |
| `KEY=value` string over 126,976 bytes (`MAX_ARG_STRLEN` 131,072 minus a 4 KiB margin) | Always spilled |
| Whole environment over the runner's measured budget | The largest remaining recipe values are spilled until it fits |
| Spilled value whose name bash reserves or that is not a shell identifier | Removed, with a `Dropping env var <NAME> (<N> bytes) from bash step` warning. See [Names the runner drops](#names-the-runner-drops) |
| Spilled value containing a NUL byte | The step fails before it runs, naming the variable and size |

A spilled value is written to a 0600 file in a private 0700 directory. A
preamble the runner puts before the step's script reads it back, byte for
byte, into a shell variable of the same name, and the directory is removed
when the step ends. Values are never logged; only names and sizes are.

Spilled variables are not exported (see
[Recipe context](../concepts/large-payload-delivery.md#recipe-context)). Pipe or
write the value to the commands that need it:

```bash
printf '%s' "$RECIPE_VAR_cycle_history" | jq -R -s 'length'
write_context_value cycle_history "$tmp/history.txt"
```

### Names the runner drops

The runner drops a spilled value, instead of reading it back, when its name
fails either test:

- It is a shell identifier: a letter or `_`, then letters, digits or `_`.
- It is not a name bash treats specially: `IFS`, `PATH`, `HOME`, `EUID`,
  `PPID`, `SHELLOPTS`, `UID`, `GROUPS`, `FUNCNAME`, `LINENO`, `RANDOM`,
  `SRANDOM`, `SECONDS`, `EPOCHSECONDS`, `EPOCHREALTIME`, `HISTCMD`,
  `DIRSTACK`, `OPTIND`, `OPTARG`, `PS4`, `ENV`, `POSIXLY_CORRECT`, `CDPATH`,
  `GLOBIGNORE`, or any name starting with `BASH` or `COMP_`.

`RECIPE_VAR_<key>` always passes both tests for a key matching
`[A-Za-z0-9_]+`, including a key that starts with a digit, so the
canonical variable is never dropped.

The uppercase alias can be dropped. The runner builds it only for keys that
start with a letter or `_`, so `1foo` gets no alias at all. It also skips
`PATH`, `HOME`, `PWD`, `OLDPWD`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `TMPDIR`,
`TMP`, `LANG`, `LC_*`, `MAIL`, `EDITOR`, `VISUAL`, `DISPLAY`, `HOSTNAME`,
`IFS` and `PS1` to `PS4`. Every other alias is built, including ones that
fail the second test above. A large value under a key such as `bash_env`,
`env`, `seconds`, `uid`, `cdpath` or `comp_words` therefore produces a
`Dropping env var BASH_ENV ...` (or similar) warning. The step still gets the
value as `$RECIPE_VAR_bash_env`. Rename the key if the warning matters. A
small value under such a key is exported under the alias, where it can change
how bash behaves (`BASH_ENV`, `ENV`, `CDPATH`), so avoid these keys anyway.

### Runner-backed test

`recipe_runner_delivers_200k_value_to_bash_step` runs the real runner on a
recipe whose first step prints 200 KiB of multi-byte text into the context key
`payload`. The second step reads it as `$RECIPE_VAR_payload` and through
`write_context_value`, and the test compares both copies byte for byte.

The runner is `RECIPE_RUNNER_RS_PATH` (CI sets it to the runner built at the
pin), then `RECIPE_RUNNER_QA_BINARY`, then `recipe-runner-rs` on `PATH`. A
missing runner fails the test. A runner that does not report unknown step
fields (the probe for runners older than the pin) fails the test when it came
from `RECIPE_RUNNER_RS_PATH`, and otherwise skips it with a printed reason.

### Helpers that must be sourced

These helpers read context from the calling shell's variables. A spilled value
is not exported, so a helper run as its own process (`bash helper.sh`,
`sh helper.sh`, `exec`, or the path as a command) cannot see it. Each one is
sourced with `.`, and each one's header comment says so.

| Helper | What it reads | Sourced by |
| --- | --- | --- |
| `recipe_context_value.sh` | `RECIPE_VAR_<name>`, then `AMPLIHACK_CONTEXT_FILE` | `autodrive-crusty-loop`, `autodrive-merge-loop`, `consensus-issue-worktree`, `consensus-merge-finalize`, `consensus-publish`, `quality-audit-cycle` |
| `workflow_body_file.sh` | Its stdin. It defines `write_body_file` in the caller's shell | `consensus-issue-worktree`, `consensus-publish`, `workflow-prep` |
| `workflow_context.sh` | `RECIPE_VAR_<key>` and `RECIPE_VAR_<key>__*` for its fixed list of keys, their uppercase aliases, and `AMPLIHACK_CONTEXT_FILE` from runners older than the pin | `workflow-design`, `workflow-tdd`, `workflow-precommit-test`, `workflow-finalize` |
| `workflow_doc_review_checkpoint.sh` | Shell variables that `workflow_context.sh` has just set, such as `BRANCH_NAME` and `PR_NUMBER` | `workflow-design` |

`workflow_context.sh` finds `RECIPE_VAR_*` names with `compgen -A variable`,
which lists shell-local variables as well as exported ones, so it sees spilled
values when sourced.

`recipe_helpers_are_sourced_not_executed` scans every recipe under
`amplifier-bundle/recipes/` and fails if a line runs one of these four helpers
as a command, either by file name or through a variable assigned its path
(such as `bash "$CONTEXT_HELPER"`). The failure names the recipe file, the
line number and the helper.

Two more guards cover the same helpers:

| Test | Fails when |
| --- | --- |
| `every_step_that_reads_context_through_a_helper_sources_it` | A step calls `write_context_value` or `write_body_file` without sourcing the helper that defines it |
| `no_production_recipe_evaluates_a_recipe_var_value` | A recipe, recipe test script or bundled tool passes a `RECIPE_VAR_*` value to `eval`, `export`, `.` or `source`. One test harness that stands in for the runner, `recipes/tests/test-default-workflow-reliability.sh`, is allowed by path |

## Shell helper: `workflow_body_file.sh`

`amplifier-bundle/tools/workflow_body_file.sh` writes text from stdin to a
file suitable for `gh ... --body-file` or `git commit -F`.

```bash
write_body_file <OUT> [<SOURCE>] < input
```

Source the script, which defines `write_body_file`, and call the function
from the step's shell. The bundled recipes always source it.

| Argument or variable | Meaning | Default |
| --- | --- | --- |
| `<OUT>` | Path to write | required |
| `<SOURCE>` | Where the full text can be found, used in the truncation note | `the workflow step output` |
| `WORKFLOW_BODY_MAX_BYTES` | Byte limit for the kept text | `60000` |

| Input size | Output |
| --- | --- |
| At most `WORKFLOW_BODY_MAX_BYTES` | The input, byte for byte |
| Larger | The first `WORKFLOW_BODY_MAX_BYTES` bytes, cut back to a UTF-8 boundary, then `\n\n[... truncated N bytes; full text in <SOURCE> ...]` |

| Exit code | Meaning |
| --- | --- |
| 0 | File written |
| 1 | Read or write failure |
| 2 | Usage error, such as a missing `<OUT>` |

The helper runs under `umask 077`, writes through a `mktemp` file and a
`trap ... EXIT` cleanup, uses no `eval`, and writes the note with
`printf '%s'`. It needs only POSIX tools (`head`, `od`, `tail`, `wc`) and no
Python.

## Recipe and shell sites

| Site | Large text | Delivery |
| --- | --- | --- |
| `quality-audit-cycle.yaml` `run-recursive-cycle` | `cycle_history`, `task_description` | `write_context_value` to 0600 temporary files, then `--context-from-file` |
| `workflow-prep.yaml` | Issue body | `gh issue create --body-file` through `write_body_file` |
| `tools/workflow_publish_pr.sh` | PR body | `gh pr create --body-file` through `write_body_file` |
| `consensus-publish.yaml` | PR body, commit message | `--body-file`, `git commit -F` |
| `consensus-issue-worktree.yaml` | Issue body | `--body-file` |
| `consensus-publish.yaml`, `consensus-issue-worktree.yaml` | Final requirements | `write_context_value final_requirements.<field>` to a 0600 file |
| `consensus-merge-finalize.yaml` | Task description | `write_context_value task_description` to a 0600 file, then `jq --rawfile` instead of `--arg` |
| `tools/autodrive_loop.sh` | Task description | Written once to `$STATE_DIR/task_description.txt` (0600, temp file then `mv`); each round passes `--context-from-file task_description=...` |
| `multitask` launcher script | Workstream context | `exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose`, with `context.json` at 0600 |

No recipe declares `max_env_value_bytes`. The runner never limited anything
with that field, and the pinned runner reports it as retired:

```text
recipe 'workflow-tdd': step '<id>': field 'max_env_value_bytes' is retired and ignored — the runner now spills oversized env values to private files automatically; remove it from the recipe
```

`dead_max_env_value_bytes_field_is_removed` fails if the field returns to any
recipe in `amplifier-bundle/recipes`. See
[Values the runner spills](#values-the-runner-spills).

Seven steps, in six recipes, locate `recipe_context_value.sh` with the same
lookup snippet. They copy it because a YAML shell step cannot include a shared
file; the test below keeps the copies in sync, so no include mechanism is
needed. `recipe_helper_lookup_snippets_are_identical` requires exactly seven
copies of the lookup, its not-found check and the `.` that sources the helper,
and fails if any copy differs from the others, ignoring only the name of the
variable that receives the directory.

## Claude profiles and auto-switch

Payload delivery and [Claude profile selection](./profile-command.md) apply to
the same launches. Each launch keeps both: the prompt or context travels by
stdin, a private file or bounded argv, and the profile travels by the small,
bounded variables `AMPLIHACK_CLAUDE_PROFILE` and `AMPLIHACK_CLAUDE_PROFILE_ROOT`.

| Launch | Payload channel | Profile and agent binary |
| --- | --- | --- |
| Fleet reasoner (`NativeReasonerBackend::complete`) | `reasoner_command(path, decision)` argv plus the prompt on stdin through `run_output_with_timeout_stdin` | `EnvBuilder::with_launched_agent_binary("claude")` tags the child as a Claude session |
| `amplihack recipe run` | Context from `--context-file` and `--set`; the environment mirror built by `context_env_pairs` stays inside `resolve_context_env_budget()` | The runner's agent binary comes from `with_resolved_agent_binary(...)` in the same command builder, after the context mirror is applied |
| `multitask` recipe-mode workstream | `launcher.sh` passes `context.json` (mode 0600) to `amplihack recipe run` with `--context-file` | `run.sh` exports the two profile variables, single-quoted; the exports come before `exec bash launcher.sh` |
| Recipe agent step (`recipe-runner-rs`) | The step prompt as `-p <prompt>` argv; the system prompt as `--system-prompt` when small, or as a 0600 file passed with `--append-system-prompt-file` when large. Never stdin | Inherited profile; auto-switch may substitute or rerun under another profile |

A recipe-mode workstream writes two scripts. `run.sh` sets the session
variables and the profile, then hands over to `launcher.sh`. With the profile
`work` set, the end of `run.sh` looks like this (the session-tree and
workstream `export` lines above it are omitted):

```bash
export AMPLIHACK_CLAUDE_PROFILE='work'
export AMPLIHACK_CLAUDE_PROFILE_ROOT='/home/user/.amplihack/claude-profiles'
exec bash launcher.sh
```

`launcher.sh` (from `recipe_launcher_script`) sets `RECIPE` to the
single-quoted recipe name and `CONTEXT_JSON` to the `context.json` beside it,
then passes the context by file:

```bash
exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose
```

Both profile values are quoted with the same `sh_single_quote` helper that
quotes the recipe name. The exported `AMPLIHACK_CLAUDE_PROFILE` is not always
the inherited name: auto-switch may substitute another profile before
`run.sh` is written. A profile variable that is set but not valid UTF-8 stops
the workstream before `run.sh` or `context.json` is written. The error names
the variable, never its value.

### Supervised reruns and stdin

When auto-switch supervises a nested `amplihack claude -p` launch (see
[Usage-limit reruns](./profile-command.md#usage-limit-reruns)), each attempt
inherits the supervisor's stdin. The first attempt reads the prompt to the
end, so a rerun of a launch whose prompt was piped on stdin starts with an
empty stdin. The prompt is lost, not exposed: it is never copied to argv, the
environment or the rerun buffer.

amplihack's own large-payload sites take other paths, and none of these sites are affected:

- the fleet reasoner starts the `claude` binary directly, not through
  `amplihack claude`, so it is never supervised;
- `--auto` launches are excluded from supervision, so auto mode's per-turn
  stdin prompt is never rerun;
- `amplihack recipe run` passes context to the runner as `--set` argv, and
  `recipe-runner-rs` gives each agent step its prompt as `-p` argv (plus a
  0600 system-prompt file for large prompts), never on stdin. A rerun gets the
  same argv and reads the same prompt file again;
- multitask workstreams pass context in `context.json`, which every attempt
  can read again.

The limit applies only when someone runs `amplihack claude -p` and pipes the
prompt on stdin by hand.

The tests that hold this contract are `tests_issue_1570_reasoner` (stdin
delivery and two-argument `reasoner_command`), `multitask/issue_1570_launcher.rs`,
the `profile_exports_*` tests in `multitask/command_builder_tests.rs`, and
`test_execute_recipe_via_rust_propagates_agent_binary_env` in
`recipe/run/tests_execute.rs`.

## Security contract

These rows apply to the APIs above. For the properties of the layer as a whole,
see [Security properties](../concepts/large-payload-delivery.md#security-properties).

| Surface | Guarantee |
| --- | --- |
| `write_stdin_async`, `StdinWriter` | Errors keep their `io::ErrorKind` and never include payload bytes |
| `run_output_with_timeout_stdin` | A failed stdin write is an error naming only the program and the error kind; a timeout names only the program, never argv |
| `amplihack-remote` | The API key is the first stdin line only: never in argv, the `azlin` environment, logs or error text. `validate_api_key` rejects empty keys and keys containing CR or LF |
| Fleet admiral `start_agent` | The prompt travels only on stage 1 stdin; stage 2 receives a validated path (see [Fleet admiral `start_agent`](#fleet-admiral-start_agent)) |
| `file-reference` delivery | The prompt file sits alone in a private 0700 directory, and Copilot's `--add-dir` names only that directory, never the shared temp directory |
| `--context-file`, `--context-from-file`, `--prompt-file` | Files over 16 MiB are refused, and a read limit also stops a file that grows after the size check. Errors name the option, path and key, never contents |
| `recipe_context_value.sh` | No `eval`; the key reaches `jq` with `--arg`; the output is written under `umask 077` and moved into place |
| Helper lookup in recipes | Searches `AMPLIHACK_HOME`, `REPO_PATH`, the working directory, `~/.copilot`, then `~/.amplihack`. With `AMPLIHACK_HOME` unset, the target repository's copy can be sourced. These steps already run repository code (builds, hooks, tests), so this adds no new trust boundary |

## Sites that still use argv

These build argv from a prompt but have no caller that passes large text:

| Site | Reason left on argv |
| --- | --- |
| `platform_cli/mod.rs` | No caller with unbounded input |
| `copilot_strategy.rs` | No caller with unbounded input; Copilot has no stdin contract |
| `turn.rs` | No caller with unbounded input |

## Related

- [Large Payload Delivery to Child Processes](../concepts/large-payload-delivery.md) - Why the layer exists
- [Pass Large Recipe Context](../howto/pass-large-recipe-context.md) - Convert recipe steps
- [amplihack-rs Parity Reference](../amplihack-rs-parity.md) - `AMPLIHACK_PROMPT_DELIVERY` and doctor output
- [Recipe Context Environment Export](./recipe-context-environment.md) - Context in step environments
- [Reference: `amplihack profile`](./profile-command.md) - Profile variables, auto-switch and usage-limit reruns
