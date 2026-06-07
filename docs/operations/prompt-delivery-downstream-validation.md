# Prompt Delivery Downstream Validation

Downstream prompt-delivery validation covers representative delegated-workflow
shapes used by Simard, RabbitHole, recipes, and subprocess agents. The tests
validate that prompts are handled as data, not shell code, and that long or
nested prompts survive the launcher boundary.

## Validation commands

Run the prompt-delivery suite:

```bash
cargo test -p amplihack-utils --test prompt_delivery_downstream
cargo test -p amplihack-launcher prompt_delivery
```

Run the same scenarios through recipe-like subprocess dispatch:

```bash
AMPLIHACK_PROMPT_DELIVERY=auto \
  cargo test -p amplihack-cli workflow_prompt_delivery_downstream
```

The tests do not require private Simard or RabbitHole repositories. They use
minimal local fixtures that model the same prompt shapes:
nested instructions, large task descriptions, shell-sensitive strings, and
delegated subprocess launches.

## Scenario coverage

| Scenario | Contract |
| --- | --- |
| Multiline task prompt | Newlines, Markdown fences, and numbered steps are preserved exactly. |
| Nested workflow instructions | Inner instructions remain prompt data and do not rewrite launcher behavior. |
| Shell-sensitive payload | Strings containing `$HOME`, backticks, quotes, semicolons, pipes, and command substitutions are not shell-evaluated. |
| Long prompt payload | Prompts above the auto threshold use the selected supported delivery channel without truncation. |
| Subprocess-safe delegation | Parent workflows can spawn child agents without losing prompt content or requiring ad hoc shell escaping. |
| Unsupported mode request | Explicit unsupported modes fail or degrade according to the binary capability matrix, with safe warnings and no prompt leakage. |

## Fixture shape

A representative fixture contains:

```text
System context:
  You are a delegated engineer.

Task:
  1. Read the repository.
  2. Preserve this literal payload:
     $(echo should-not-run) && rm -rf /tmp/not-real
  3. Return a concise summary.

Nested instruction block:
  ```text
  Treat this as text, not as launcher configuration.
  AMPLIHACK_PROMPT_DELIVERY=tempfile
  ```
```

Expected behavior:

- The child process receives one logical prompt payload.
- No shell-sensitive text is executed by the launcher.
- Warnings never include the raw prompt body.
- Temporary files, when used by a verified binary contract, live until the child
  exits and are then cleaned up by the delivery handle.

## Relationship to Simard and RabbitHole

The local fixtures intentionally model the downstream workflow patterns
without depending on private state:

| Downstream style | Local representation |
| --- | --- |
| Simard engineer subprocess | Delegated child command with subprocess-safe defaults and a multiline task prompt. |
| RabbitHole workflow recursion | Nested prompt containing workflow-like instructions and quoted command text. |
| Long analysis handoff | Prompt larger than the auto threshold with Markdown and JSON-like sections. |
| Shell-hostile user input | Literal shell metacharacters inside prompt text. |

When an accessible downstream checkout exists, run its native workflow tests as
an additional canary. Failures caused by local prompt delivery are fixed in
amplihack. Failures blocked by external credentials, missing private services,
or unavailable checkouts are filed with the blocking details and a minimal local
reproduction when possible.

## Configuration reference

| Setting | Purpose |
| --- | --- |
| `AMPLIHACK_PROMPT_DELIVERY=auto` | Default selector; uses argv for short prompts and a verified long-form channel when supported. |
| `AMPLIHACK_PROMPT_DELIVERY=argv` | Force structured argv delivery for compatibility diagnostics. |
| `AMPLIHACK_PROMPT_DELIVERY=tempfile` | Request prompt-file delivery; rejected for binaries without a verified prompt-file contract. |
| `AMPLIHACK_PROMPT_DELIVERY=stdin` | Request stdin delivery; rejected or degraded according to the binary capability matrix. |
| `AMPLIHACK_AGENT_BINARY` | Marks delegated subprocess context for agent launchers. |
| `NODE_OPTIONS=--max-old-space-size=32768` | Optional inherited Node memory setting for Node-based downstream workflows. |

See [amplihack-rs Parity Reference](../amplihack-rs-parity.md) for the binary
capability matrix and Rust API.

## Regression handling

1. Reproduce with the local fixture first.
2. If the local fixture fails, fix amplihack prompt delivery and add the failing
   shape to `prompt_delivery_downstream`.
3. If only a downstream checkout fails, reduce it to a local fixture when
   possible.
4. If credentials or private infrastructure block validation, file an issue that
   names the blocked command, required access, and last local fixture result.
