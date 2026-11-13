# Launcher Output Streams Reference

When amplihack launches an agent, standard output carries only the agent's own
output. The launcher's own notices, such as the `NODE_OPTIONS` line, install
progress and the Docker activation message, go to standard error.

This contract matters most for recipe agent steps. The recipe runner captures
the stdout of `amplihack claude --dangerously-skip-permissions -p <prompt>` as
the step's result, so a launcher line on stdout would become part of that
result. Issue #2585 moved the last launch-path notices off stdout; see also
issues #2575 and #2556.

## Contents

- [The contract](#the-contract)
- [Example](#example)
- [Notices that go to stderr](#notices-that-go-to-stderr)
- [Output that stays on stdout](#output-that-stays-on-stdout)
- [Reading the notices](#reading-the-notices)
- [Behaviour test](#behaviour-test)
- [Adding a new notice](#adding-a-new-notice)

## The contract

For any launch of the form `amplihack <agent> ... -p <prompt>`:

| Stream | Carries |
| ------ | ------- |
| stdout | The agent's stdout, byte for byte. Nothing is added before, after or in between. |
| stderr | Launcher notices, progress lines, warnings and errors, and the agent's own stderr. The order of lines from the two sources is not guaranteed. |

The notice text is unchanged. Only the stream it is written to differs from
earlier releases. On a terminal both streams are shown, so an interactive user
sees the same lines as before.

The contract does not depend on `--subprocess-safe` or on any environment
variable. It covers the notices written by the files listed under
[Notices that go to stderr](#notices-that-go-to-stderr). Code outside those
files, such as `crates/amplihack-launcher/src`, is not covered by this page.

## Example

With `node.heap_consent = true` saved in `~/.amplihack/config.toml`, and an
agent that prints `{"ok":true}`:

```console
$ amplihack claude --dangerously-skip-permissions -p "Return a JSON status" 2>notices.txt
{"ok":true}
$ cat notices.txt
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: amplihack config set node.heap_consent <true|false>
```

The stdout of the command is exactly the agent's output, so a recipe step or a
script can parse it directly:

```console
$ amplihack claude --dangerously-skip-permissions -p "Return a JSON status" 2>/dev/null | jq .ok
true
```

Earlier releases wrote the `ℹ NODE_OPTIONS=...` line to stdout, and the same
pipeline failed with a `jq` parse error.

## Notices that go to stderr

All of these are written with `eprintln!`. Files are under
`crates/amplihack-cli/src/`.

| Source | Notice |
| ------ | ------ |
| `memory_config.rs` (`prepare_memory_config`) | `ℹ NODE_OPTIONS=... (saved preference). To change: ...` and the other memory configuration summaries |
| `memory_config.rs` (`prompt_user_consent`) | `Memory: <n> GB RAM detected, recommend <n> MB limit` |
| `commands/launch/mod.rs` (`run_launch_core`) | The Docker activation message |
| `commands/launch/checkout.rs` | `Using existing repository: <dir>` and `Cloned repository to: <dir>` |
| `commands/launch/power_steering.rs` | Power-Steering disabled and re-enabled messages |
| `commands/launch/blarify.rs` | The code indexing banner, compatibility notice, skip and progress lines |
| `bootstrap.rs` | Managed Node.js, npm tool, platform binary and amplifier install and upgrade progress, and stale npm temp directory removal |

Model-passing and update-check-skip notices were already written to stderr or
through `tracing`, and are unchanged.

The blarify banner ends with a flush of stderr. The indexer itself still runs
with its stdout and stderr set to `Stdio::null()`.

## Output that stays on stdout

These are output the user asked for, or are not on the launch path:

| Source | Why it stays |
| ------ | ------------ |
| `util.rs` (`read_user_input_with_timeout`) | The interactive question has to appear next to the input. Non-interactive runs never reach it. |
| `amplihack --version` | Requested output. |
| `amplihack install`, `update`, `doctor` result lines | Requested output. Their progress lines from `bootstrap.rs` now go to stderr. |
| `query_code` | Requested output. |

## Reading the notices

To keep notices out of a captured result, capture stdout only. To keep them for
a log, redirect stderr:

```bash
amplihack claude --dangerously-skip-permissions -p "Summarise the diff" \
  >result.txt 2>launch.log
```

A script that used to read the `NODE_OPTIONS` line from stdout must read it from
stderr instead. The notices were never a stable interface, and their wording
may change.

## Behaviour test

`crates/amplihack-cli/tests/issue_2585_launch_notices_stderr.rs` (Linux only)
runs `amplihack claude --dangerously-skip-permissions -p step` against a fake
`claude` with a saved `heap_consent`, all inside one temporary directory. It
asserts that stdout is exactly `{"ok":true}\n` and that the `NODE_OPTIONS`
notice is on stderr.

```bash
cargo test -p amplihack-cli --test cli_integration issue_2585
```

## Adding a new notice

Write any new launcher notice, banner or progress line with `eprintln!` or
`tracing`. Use `println!` only for output the command was asked to produce.

The behaviour test checks only the stages it runs. It sets
`AMPLIHACK_SKIP_POWER_STEERING`, `AMPLIHACK_SKIP_AUTO_INSTALL`,
`AMPLIHACK_NO_FRESHNESS_CHECK` and `AMPLIHACK_SKIP_REFLECTION`, so a notice
added to power-steering, auto-install, the freshness check or reflection can
reach stdout without the test failing. Review such changes against the
`eprintln!` rule.

## See also

- [Memory Configuration Consent](../features/memory-consent-prompt.md) for the `NODE_OPTIONS` notice and `node.heap_consent`
- [Agent Prompt Channels Reference](./agent-prompt-channels.md) for how the prompt reaches each agent
- [Clean Result Channel Reference](./clean-result-channel.md) for returning a step result through a file instead of stdout
