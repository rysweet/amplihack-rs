---
title: "amplihack-rs Parity Reference"
description: "Rust parity reference for subprocess prompt delivery across Claude, Copilot, Codex, and Amplifier launch paths."
last_updated: 2026-06-05
review_schedule: quarterly
owner: rysweet
doc_type: reference
---

# amplihack-rs Parity Reference

This reference describes Rust subprocess prompt delivery parity for `amplihack`
launcher, orchestration, auto-mode, and doctor surfaces.

## Contents

- [Subprocess prompt delivery](#subprocess-prompt-delivery)
- [Configuration](#configuration)
- [Binary capability matrix](#binary-capability-matrix)
- [Amplifier prompt-delivery contract](#amplifier-prompt-delivery-contract)
- [Mode selection](#mode-selection)
- [Usage examples](#usage-examples)
- [Rust API](#rust-api)
- [Doctor reporting](#doctor-reporting)
- [Regression contract](#regression-contract)
- [Security and lifecycle guarantees](#security-and-lifecycle-guarantees)
- [Troubleshooting](#troubleshooting)

## Subprocess prompt delivery

amplihack-rs routes prompt-bearing subprocess launches through
`amplihack-utils::prompt_delivery`. The delivery engine supports three concrete
prompt channels:

| Mode | Behavior | Primary use |
| --- | --- | --- |
| `argv` | Appends the prompt as one structured process argument. | Short prompts and binaries with no long-form prompt channel. |
| `tempfile` | Writes the prompt to a restricted temporary file and passes the file path using the binary's verified prompt-file contract. | Long prompts when the target binary supports prompt files. |
| `stdin` | Pipes the prompt to child stdin, then closes stdin before waiting for the child. | Long prompts when the target binary supports prompt input from stdin. |

The Rust launcher, orchestration, and auto-mode paths use the same selector and
capability metadata. A long prompt is not manually shell-escaped or interpolated
into a shell command string.

### Parity status

| Parity area | Rust behavior |
| --- | --- |
| Subprocess prompt delivery | `argv`, `tempfile`, and `stdin` are selected through `amplihack-utils::prompt_delivery` using static per-binary capabilities. Unsupported explicit `tempfile` requests degrade to `stdin` when supported, otherwise `argv`, unless a binary-specific contract requires rejection. Unsupported explicit `stdin` requests degrade directly to `argv`, unless a binary-specific contract requires rejection. |

## Configuration

Set `AMPLIHACK_PROMPT_DELIVERY` to choose the requested prompt delivery mode.
This setting applies to migrated subprocess launch paths.

```bash
# Default: choose the safest supported channel for the target binary and prompt size.
unset AMPLIHACK_PROMPT_DELIVERY
amplihack claude -- "Summarize this repository"

# Request prompt-file delivery when the selected binary supports it.
AMPLIHACK_PROMPT_DELIVERY=tempfile \
  amplihack claude -- "Review this 64 KiB prompt without shell parsing"

# Request stdin delivery once the selected binary has a verified stdin contract.
AMPLIHACK_PROMPT_DELIVERY=stdin \
  amplihack codex -- "Analyze this generated task description"

# Force argv delivery for compatibility diagnostics.
AMPLIHACK_PROMPT_DELIVERY=argv \
  amplihack copilot -- "Short prompt"
```

### `AMPLIHACK_PROMPT_DELIVERY`

| Value | Meaning |
| --- | --- |
| unset, empty, or `auto` | Use automatic mode selection. |
| `argv` | Request structured argv delivery. |
| `tempfile` | Request temporary-file delivery. |
| `stdin` | Request stdin delivery. |

Values are case-insensitive. Unknown values resolve to `auto` and emit a
warning. The environment variable is a request, not a capability override: a
binary only uses `tempfile` or `stdin` when the launcher has a verified contract
for that binary.

Amplifier is the explicit rejection case: `AMPLIHACK_PROMPT_DELIVERY=tempfile`
and `AMPLIHACK_PROMPT_DELIVERY=stdin` must fail before launching Amplifier
because Amplifier has no documented task-prompt file or stdin contract.

## Binary capability matrix

Capabilities are static launcher metadata, not user-controlled configuration.
Tempfile and stdin support are only marked when the binary has a verified
task-prompt contract for that channel.

| Binary | `argv` | `tempfile` | Tempfile flag | `stdin` | Notes |
| --- | --- | --- | --- | --- | --- |
| Claude Code | Supported | Pending verification | None for task prompts | Unsupported | `--append-system-prompt` is a system-prompt file contract. It must not be used for task-prompt delivery unless the implementation intentionally changes prompt role and documents that role change. Until a verified task-prompt file contract exists, Claude task prompts remain on `argv`. |
| GitHub Copilot CLI | Supported | Unsupported | None | Unsupported | Copilot receives prompts through argv because no verified prompt-file or stdin prompt contract is advertised. |
| Codex | Supported | Unsupported | None | Pending verification | Codex stdin delivery requires a named, tested command contract that reads the task prompt from stdin. Until that contract is verified, Codex task prompts remain on `argv`. |
| Microsoft Amplifier | Supported | Unsupported | None | Unsupported | Amplifier is routed through `prompt_delivery`, but tempfile and stdin remain unsupported because Amplifier documents `run [PROMPT]` and does not document a task-prompt file or stdin contract. |

When a future binary version adds a prompt-file or stdin contract, update
`crates/amplihack-launcher/src/flag_matrix.rs` first, then update this table.

## Amplifier prompt-delivery contract

Amplifier task prompts are delivered through documented argv only.

The supported upstream prompt shape is:

```text
amplifier run [OPTIONS] [PROMPT]
```

`amplihack` must therefore treat Amplifier as `argv`-only:

| Capability | Amplifier behavior |
| --- | --- |
| Structured argv prompt | Supported. The task prompt is one `Command::arg` value. |
| Temporary prompt file | Unsupported. Amplifier does not document a `--prompt-file`, `--prompt-path`, or equivalent task-prompt file option. |
| Stdin task prompt | Unsupported. Amplifier does not document stdin as a task-prompt input channel. |
| Shell command prompt delivery | Unsupported. `amplihack` must not build `sh -c` strings or interpolate prompt text into shell commands. |

This is a capability boundary, not a size optimization. A long Amplifier prompt
may still appear as one child argv element because no stable long-form
task-prompt channel exists for Amplifier. `AMPLIHACK_PROMPT_DELIVERY=tempfile`
and `AMPLIHACK_PROMPT_DELIVERY=stdin` do not override this capability matrix.

### Requesting unsupported modes with Amplifier

If `AMPLIHACK_PROMPT_DELIVERY=tempfile` is set for Amplifier, `amplihack` must
return an error before spawning `amplifier`. It must not create a temporary
prompt file and must not silently continue with `argv`.

If `AMPLIHACK_PROMPT_DELIVERY=stdin` is set for Amplifier, `amplihack` must
return an error before spawning `amplifier`. It must not write prompt bytes to
child stdin, must not silently continue with `argv`, and must not substitute
tempfile delivery.

The error message must name the requested mode, state that Amplifier supports
only argv prompt delivery, and point to `amplifier run [OPTIONS] [PROMPT]` as
the supported prompt contract.

The Amplifier launch path must build the upstream command as:

```text
amplifier run [OPTIONS] [PROMPT]
```

It must not add a synthetic `--prompt` flag for Amplifier unless upstream
Amplifier documents that flag as a stable task-prompt contract.

### Evidence required to enable Amplifier long-form delivery

Do not enable Amplifier tempfile or stdin support from observed behavior,
internal experiments, or generic `prompt_delivery` support alone. The launcher
capability matrix can mark Amplifier as supporting a long-form channel only when
upstream Amplifier exposes a documented, stable task-prompt contract such as:

- a named prompt-file option in `amplifier run --help`,
- documentation that stdin is read as the task prompt for a named command mode,
- or an upstream release note/API document that commits to one of those
contracts.

When that evidence exists, update `flag_matrix.rs`, add round-trip regression
coverage for a 64 KiB prompt containing apostrophes, and update this reference.

## Mode selection

`prompt_delivery` resolves a requested mode into an effective mode using the
prompt size and the selected binary's capabilities.

| Requested mode | Selection rule |
| --- | --- |
| `auto` | Use `argv` for prompts at or below 4096 bytes when supported. For larger prompts, prefer `tempfile`, then `stdin`, then `argv`. |
| `argv` | Use `argv` when supported; otherwise degrade to `tempfile`, then `stdin`, then `argv`. |
| `tempfile` | Use `tempfile` when supported; otherwise degrade to `stdin`, then `argv`. |
| `stdin` | Use `stdin` when supported; otherwise degrade directly to `argv`. Do not degrade an explicit stdin request to tempfile, because stdin and prompt-file delivery have different process contracts. |

Every unsupported explicit-mode degradation emits a warning that names the
requested mode and the effective mode. The warning does not include the raw
prompt, temporary-file contents, or stdin payload.

Binary-specific rejection rules override the generic degradation table. For
Amplifier, explicit `tempfile` and `stdin` requests are invalid and must fail
before execution.

## Usage examples

### Send a long prompt safely through auto mode

Create a long prompt and let amplihack choose the safest supported delivery
channel for the child binary.

```bash
python3 - <<'PY' > /tmp/amplihack-long-prompt.txt
print("Review this text safely: " + ("don't shell-expand $HOME; " * 4096))
PY

AMPLIHACK_PROMPT_DELIVERY=auto \
  amplihack claude -- "$(cat /tmp/amplihack-long-prompt.txt)"
```

This shell form still passes the prompt to the parent `amplihack` process as one
argv element. The parity guarantee in this document applies to the child agent
subprocess launched by amplihack. Until Claude has a verified task-prompt
tempfile contract, the effective child delivery mode for Claude is `argv`.

### Diagnose unsupported requested modes

Requesting tempfile delivery for a generic binary without a verified tempfile
contract degrades safely unless the binary has a stricter rejection rule.

```bash
AMPLIHACK_PROMPT_DELIVERY=tempfile \
  amplihack doctor
```

For Amplifier, doctor reports `capabilities: argv` and a runtime policy that
explicit `tempfile` and `stdin` requests are rejected before launch. Runtime
Amplifier launches must pass prompts as structured argv only when the requested
mode is unset, `auto`, or `argv`.

### Run Amplifier with the documented prompt channel

Amplifier's documented prompt input is the positional `PROMPT` argument on
`amplifier run`.

```bash
amplihack amplifier -- run "Explain the authentication flow in this repository"
```

This remains argv delivery even when the prompt is long. If the prompt must stay
out of child argv, use a different agent binary with a verified long-form prompt
contract; Amplifier has no supported tempfile or stdin task-prompt channel.

### Use prompt delivery in auto-mode

Auto-mode uses the same prompt delivery path as ordinary launcher execution.

```bash
AMPLIHACK_PROMPT_DELIVERY=auto \
  amplihack auto --agent claude --task "$(cat /tmp/amplihack-long-prompt.txt)"
```

The `DeliveryHandle` returned by the prompt delivery engine is owned until the
subprocess exits. For future verified tempfile contracts, that keeps the
temporary prompt file available for the entire child lifetime.

## Rust API

The generic delivery engine lives in `amplihack-utils`. The low-level API exists
for delivery mutation; launcher-level command building wraps it with binary
capability metadata.

```rust
use std::process::Command;

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryHandle, DeliveryMode, PromptDelivery, deliver, from_env,
};

fn build_command(prompt: &str) -> std::io::Result<(Command, DeliveryHandle)> {
    let mut cmd = Command::new("claude");
    let requested = from_env();
    let caps = DeliveryCaps::argv_only();
    let handle = deliver(&mut cmd, prompt, requested, &caps)?;
    Ok((cmd, handle))
}
```

### Public types

| Type | Purpose |
| --- | --- |
| `PromptDelivery` | Caller-requested mode: `Auto`, `Argv`, `Tempfile`, or `Stdin`. |
| `DeliveryMode` | Effective mode selected after applying capabilities and degradation rules. |
| `DeliveryCaps` | Per-binary capability descriptor for argv, tempfile, stdin, and tempfile flag support. |
| `DeliveryHandle` | RAII owner for delivery resources. Keep it alive until the child process has exited. |

### Public functions

| Function | Purpose |
| --- | --- |
| `from_env()` | Parses `AMPLIHACK_PROMPT_DELIVERY` and returns a `PromptDelivery` request. |
| `select_mode(requested, prompt_size, caps)` | Resolves a requested mode to an effective `DeliveryMode`. |
| `deliver(cmd, prompt, requested, caps)` | Mutates a structured `std::process::Command` for the effective delivery mode and returns a `DeliveryHandle`. |

### Launcher API

Launcher command builders expose delivery-aware construction instead of pushing
prompt strings directly into argv.

```rust
use amplihack_launcher::flag_matrix::AgentBinary;
use amplihack_launcher::prompt_delivery::build_tool_command_with_prompt_delivery;
use amplihack_utils::prompt_delivery::PromptDelivery;

let delivered = build_tool_command_with_prompt_delivery(
    AgentBinary::Claude,
    std::env::current_dir()?.as_path(),
    &[],
    "Summarize this repository",
    PromptDelivery::Auto,
)?;

let mut child = delivered.command.spawn()?;
let status = child.wait()?;
drop(delivered.delivery_handle);
```

The `DeliveredCommand` shape is:

| Field | Meaning |
| --- | --- |
| `command` | Structured `std::process::Command` with prompt delivery applied. |
| `delivery_handle` | RAII handle that owns tempfile resources or marks stdin ownership. |
| `requested_mode` | Mode requested by configuration. |
| `selected_mode` | Mode actually selected for the target binary. |
| `warnings` | Deterministic degradation or invalid-configuration warnings safe for logs. |
| `stdin_payload` | Prompt bytes to write when the effective mode is `stdin`; absent for `argv` and `tempfile`. |

### Async orchestration contract

Async orchestration builds a `std::process::Command`, applies
`prompt_delivery::deliver`, then convert the command to `tokio::process::Command`
for spawning. It does not duplicate delivery selection logic.

When the effective mode is `stdin`, orchestration writes the complete prompt to
child stdin, flushes it, drops stdin to close the pipe, then awaits child exit.
The `DeliveryHandle` remains owned until after `wait` completes.

## Doctor reporting

`amplihack doctor` reports prompt delivery diagnostics without printing prompt
contents.

```bash
AMPLIHACK_PROMPT_DELIVERY=tempfile amplihack doctor
```

Example output:

```text
Prompt delivery
  requested: tempfile
  auto threshold: 4096 bytes

  claude
    capabilities: argv
    note: task-prompt tempfile support pending verified Claude contract
    effective for long prompt: argv

  copilot
    capabilities: argv
    effective for long prompt: argv
    warning: requested tempfile is unsupported; degrading to argv

  codex
    capabilities: argv
    note: stdin support pending verified Codex command contract
    effective for long prompt: argv
    warning: requested tempfile is unsupported; degrading to argv

  amplifier
    capabilities: argv
    effective for long prompt: argv
    runtime policy: explicit tempfile/stdin requests are rejected before launch
```

Doctor diagnostics are deterministic:

- they list the requested mode,
- they list static capabilities per binary,
- they show the effective mode for a long prompt,
- they show degradation warnings or binary-specific rejection policies when a
  requested mode is unsupported,
- they never include raw prompt data.

## Regression contract

Prompt delivery behavior is defined by these externally visible contracts:

| Contract | Required behavior |
| --- | --- |
| Long prompt privacy | For every migrated command builder with a verified long-form mode, a raw 64 KiB prompt containing apostrophes is absent from child argv when `auto`, `tempfile`, or `stdin` selects that long-form delivery mode. Binaries with only `argv` support are explicitly exempt and must still pass the prompt as one structured argv element. |
| Prompt fidelity | The same 64 KiB apostrophe-containing prompt round-trips unchanged through the selected delivery channel. |
| Structured argv safety | When `argv` is selected, the prompt is passed as one argv element. Apostrophes, quotes, semicolons, dollar signs, and shell metacharacters are not interpreted by a shell. |
| Unsupported tempfile request | A requested `tempfile` mode degrades to `stdin` when supported, otherwise to `argv`, and emits a warning, unless the target binary has a stricter rejection policy. |
| Unsupported stdin request | A requested `stdin` mode degrades directly to `argv` and emits a warning, unless the target binary has a stricter rejection policy. It does not degrade to `tempfile`. |
| Amplifier unsupported mode request | For Amplifier, explicit `tempfile` and `stdin` requests fail before spawning `amplifier`; they do not degrade to `argv`. |
| Tempfile lifetime | Tempfile prompts remain readable until the child process exits and are unlinked when the `DeliveryHandle` is dropped. |
| Stdin lifecycle | Stdin payloads are written completely, flushed, and closed before the child is awaited. |
| Diagnostics safety | Doctor output, logs, and warnings include modes, capabilities, and degradation messages, but never include raw prompt bytes. |

## Security and lifecycle guarantees

Prompt delivery follows these guarantees:

| Guarantee | Detail |
| --- | --- |
| No shell command construction | Prompts are passed through `Command` argv, restricted tempfiles, or child stdin. |
| No prompt logging | Diagnostics and warnings describe modes and capabilities only. |
| Restricted tempfiles | Tempfile prompts are created with owner-only permissions on Unix and are unlinked by RAII drop. |
| Child-lifetime ownership | `DeliveryHandle` lives until the child exits, preventing premature tempfile deletion. |
| Closed stdin | Stdin payloads are written, flushed, and closed before waiting, preventing deadlocks. |
| Static capabilities | Users cannot force an unsupported binary to claim tempfile or stdin support. |
| Deterministic handling | Unsupported modes either degrade through the documented order with warnings or fail through a documented binary-specific rejection policy. |

## Troubleshooting

### A requested mode is not used or is rejected

Run doctor with the same environment:

```bash
AMPLIHACK_PROMPT_DELIVERY=tempfile amplihack doctor
```

If the selected binary lists only `argv`, the requested long-form mode is not
available for that binary. Generic launch paths may degrade to `argv` with a
warning; Amplifier must reject explicit `tempfile` and `stdin` requests before
launch. Use a binary with a verified long-form prompt contract or leave
`AMPLIHACK_PROMPT_DELIVERY=auto`.

### A long prompt still appears in argv

Check the binary capability row in doctor output. Long prompts remain in child
argv when the target binary has no verified `tempfile` or `stdin` task-prompt
contract.

### A tempfile prompt disappears before the child reads it

Keep the returned `DeliveryHandle` alive until after the child process has been
waited on. Dropping the handle unlinks the tempfile.

### A stdin-launched child hangs

Write the full stdin payload, flush it, then drop the stdin handle before
awaiting process exit. Holding stdin open can make the child wait for more input.
