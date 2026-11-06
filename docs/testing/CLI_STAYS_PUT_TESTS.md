---
title: CLI Stays Put Tests
type: reference
issue: 1983
---

# CLI Stays Put Tests

Reference for the regression suite that checks the rule from issue #1983:

> A run started with claude stays in claude, and a run started with codex stays
> in codex, unless the user explicitly asks for another CLI.

The suite lives in `crates/amplihack-cli/tests/issue_1983_cli_stays_put/` and
is a module of the `cli_integration` test target. Every test starts the real
`amplihack` binary under a cleared environment and asserts on what fake
`claude` and `codex` scripts recorded when they ran. No real CLI runs.

For why the rule holds, see
[Agent Binary Routing](../concepts/agent-binary-routing.md#a-run-stays-on-the-cli-it-started-with).

## Contents

- [Running the suite](#running-the-suite)
- [Levels and covering tests](#levels-and-covering-tests)
- [Supporting tests](#supporting-tests)
- [Fixture](#fixture)
- [Fake CLI record format](#fake-cli-record-format)
- [Stub recipe runner](#stub-recipe-runner)
- [Isolation guarantees](#isolation-guarantees)
- [Adding a case](#adding-a-case)
- [Known limits](#known-limits)

## Running the suite

```bash
cargo test -p amplihack-cli --test cli_integration issue_1983_cli_stays_put::
```

Run one level by naming its module:

```bash
cargo test -p amplihack-cli --test cli_integration issue_1983_cli_stays_put::explicit_launch::
```

The suite is Unix only (`#![cfg(unix)]`). It needs `sh`, `flock`, `mktemp`,
`realpath` and `printenv` on `/usr/bin` or `/bin`. It does not need
`recipe-runner-rs`, tmux, git credentials or network access.

## Levels and covering tests

Each row names the test that covers that level in that direction. "Claude
direction" means the case ends with claude running; "codex direction" means it
ends with codex running. The table has 17 rows: three for
level 1, two each for levels 2 and 3, six for level 4 and four for level 5.

| Level | What is checked | Direction | Covering test (`file::name`) |
|---|---|---|---|
| 1 | Nested command | Claude session stays claude | `nested.rs::claude_session_runs_recipe_under_claude` |
| 1 | Nested command | Codex session stays codex | `nested.rs::codex_session_runs_recipe_under_codex` |
| 1 | Nested command, two hops | Codex session stays codex | `nested.rs::codex_session_launching_an_agent_command_stays_codex` |
| 2 | Every workflow step | Claude run, all steps claude | `steps.rs::every_step_of_a_claude_run_uses_claude` |
| 2 | Every workflow step | Codex run, all steps codex | `steps.rs::every_step_of_a_codex_run_uses_codex` |
| 3 | `--subprocess-safe` launch | Codex parent, explicit claude | `subprocess_safe.rs::claude_subprocess_safe_from_a_codex_parent_runs_claude` |
| 3 | `--subprocess-safe` launch | Claude parent, explicit codex | `subprocess_safe.rs::codex_subprocess_safe_from_a_claude_parent_runs_codex` |
| 4 | Inherited `session:codex` | Explicit claude wins | `explicit_launch.rs::claude_beats_inherited_session_codex` |
| 4 | Inherited `default:codex` | Explicit claude wins | `explicit_launch.rs::claude_beats_inherited_default_codex` |
| 4 | Inherited untagged `codex` | Explicit claude wins | `explicit_launch.rs::claude_beats_inherited_untagged_codex` |
| 4 | Inherited `session:claude` | Explicit codex wins | `explicit_launch.rs::codex_beats_inherited_session_claude` |
| 4 | Inherited `default:claude` | Explicit codex wins | `explicit_launch.rs::codex_beats_inherited_default_claude` |
| 4 | Inherited untagged `claude` | Explicit codex wins | `explicit_launch.rs::codex_beats_inherited_untagged_claude` |
| 5 | User-set `AMPLIHACK_AGENT_BINARY` | Claude session told codex | `explicit_change.rs::claude_session_told_codex_dispatches_codex` |
| 5 | User-set `AMPLIHACK_AGENT_BINARY` | Codex session told claude | `explicit_change.rs::codex_session_told_claude_dispatches_claude` |
| 5 | `agent.binary` config | Configured codex | `explicit_change.rs::config_agent_binary_codex_dispatches_codex` |
| 5 | `agent.binary` config | Configured claude | `explicit_change.rs::config_agent_binary_claude_dispatches_claude` |

All file paths are relative to `crates/amplihack-cli/tests/issue_1983_cli_stays_put/`.

### Level 1: nested commands

The test builds the environment that a real launcher exports to its agent and
runs `amplihack recipe run work/probe.yaml` from it.

| Direction | Parent environment |
|---|---|
| Claude | `AMPLIHACK_AGENT_BINARY=claude`, `AMPLIHACK_AGENT_BINARY_SOURCE=session:claude`, `AMPLIHACK_SESSION_NONCE`, `CLAUDECODE=1`, `CLAUDE_CODE_ENTRYPOINT=cli` |
| Codex | `AMPLIHACK_AGENT_BINARY=codex`, `AMPLIHACK_AGENT_BINARY_SOURCE=session:codex`, `AMPLIHACK_SESSION_NONCE`, no Claude markers |

The test passes when the stub runner received the parent's CLI and every step
record shows that CLI ran.

`codex_session_launching_an_agent_command_stays_codex` starts one hop earlier.
It runs `amplihack codex`, and the fake codex runs `amplihack recipe run`
itself, so the chain is test → `amplihack codex` → fake codex →
`amplihack recipe run` → runner → step → fake codex.

### Level 2: workflow steps

The stub runner runs three nested agent steps, `step:outer`, `step:sub` and
`step:subsub`, which stand in for smart-orchestrator, default-workflow and a
sub-recipe. Each test asserts that:

- exactly three `step:*` records exist;
- each shows `ran`, `binary` and `source` for the run's CLI;
- in the codex direction, no record holds a Claude marker variable and every
  `children_see` is `codex`.

### Level 3: subprocess-safe launches

| Test | Command | Parent | Expected record |
|---|---|---|---|
| Claude direction | `amplihack claude --subprocess-safe -p x` | `codex` / `session:codex` | `ran=claude`, `binary=claude`, `source=session:claude` |
| Codex direction | `amplihack codex --subprocess-safe exec -` | `claude` / `session:claude` plus Claude markers | `ran=codex`, `binary=codex`, `source=session:codex`, every Claude marker `<unset>` |

### Level 4: explicit command beats inherited value

One helper, `explicit_beats(explicit, tag)`, drives
all six tests. The parent holds `AMPLIHACK_AGENT_BINARY=<other>` with
`AMPLIHACK_AGENT_BINARY_SOURCE` set to `session:<other>`, `default:<other>` or
unset. In every case the explicit launcher's fake runs and records
`binary=<explicit>`, `source=session:<explicit>` and
`children_see=<explicit>`.

### Level 5: explicit change

Level 5 is the "unless explicitly asked" part of the rule.

| Test | Parent | Expected |
|---|---|---|
| `claude_session_told_codex_dispatches_codex` | Claude session environment with `AMPLIHACK_AGENT_BINARY` changed to `codex`; the `session:claude` tag stays, and a tag naming another binary does not describe the value | Steps run codex |
| `codex_session_told_claude_dispatches_claude` | Codex session environment with `AMPLIHACK_AGENT_BINARY` changed to `claude`; the `session:codex` tag stays | Steps run claude |
| `config_agent_binary_codex_dispatches_codex` | No markers, no `AMPLIHACK_AGENT_BINARY`; `[agent] binary = "codex"` in config | Steps run codex |
| `config_agent_binary_claude_dispatches_claude` | No markers, no `AMPLIHACK_AGENT_BINARY`; `[agent] binary = "claude"` in config | Steps run claude |

The configuration tests write `<scratch HOME>/.amplihack/config.toml`:

```toml
[agent]
binary = "codex"
```

Configuration does not override a live session marker (issue #1606). For a
running session, the explicit request is a user-set `AMPLIHACK_AGENT_BINARY`.

## Supporting tests

These tests check the same rules but are not the covering test for any row,
either because they test a pure function or because they cover only one
direction.

| Test | What it covers |
|---|---|
| `bins/amplihack/tests/issue_1481_recipe_run_agent_binary.rs::a_claude_session_without_claudecode_hands_claude_to_the_runner` | Level 1, claude direction, with `CLAUDECODE` already removed |
| `bins/amplihack/tests/issue_1481_recipe_run_agent_binary.rs::an_explicit_binary_wins_and_is_not_tagged` | Level 5, explicit value passed to the runner untagged |
| `crates/amplihack-cli/src/commands/launch/tests_env.rs` (copilot cases) | Tags the launcher exports, as a pure function |
| `crates/amplihack-cli/src/env_builder/builder.rs` inline tests | `with_launched_agent_binary_from` and `other_clis_markers` |
| `crates/amplihack-cli/tests/active_agent_binary_default_test.rs` | Default-layer tagging |
| `crates/amplihack-cli/tests/issue_1606_config_cli.rs` | `agent.binary` ranks below a live marker |
| `crates/amplihack-utils/tests/agent_binary_*` | Resolver precedence |

## Fixture

`fixture.rs` builds one scratch world per test under `tempfile::tempdir()`,
which honours `$TMPDIR`:

```text
<tmp>/
├── home/                 scratch HOME; config.toml goes in home/.amplihack/
├── amplihack-home/       AMPLIHACK_HOME; keeps runtime assets out of HOME
├── tools/
│   ├── claude            fake CLI
│   ├── codex             fake CLI (same script)
│   └── recipe-runner-rs  stub runner
├── records/              one file per fake invocation, plus runner.env
├── tmux/                 TMUX_TMPDIR
└── work/
    ├── .git/             stops every walk-up here
    └── probe.yaml        recipe handed to recipe run
```

| Item | Purpose |
|---|---|
| `Cli::{Claude, Codex}` | The two CLIs. `Cli::other()` returns the opposite one. |
| `Fixture::new()` | Builds the world above and writes the scripts. |
| `Fixture::cmd(args)` | A `Command` for the `amplihack` binary under test, with the cleared environment described in [Isolation guarantees](#isolation-guarantees). |
| `Fixture::run_ok(cmd)` | Runs the command with a wall-clock limit enforced in Rust. On expiry it kills the process group and panics with `logs()`. |
| `Fixture::records()` | Parses every record file, ordered by `seq`. |
| `Fixture::write_config_binary(cli)` | Writes `[agent] binary` to the scratch config. |
| `Fixture::logs()` | Every record and log file, each cut to 8 KiB, for failure messages. |
| `Record::assert_cli(cli)` | Asserts `ran`, `binary`, `source=session:<cli>` and `children_see`. |
| `Record::assert_no_markers_of(cli)` | Asserts every marker of that CLI is `<unset>`. |

A typical test reads:

```rust
#[test]
fn claude_subprocess_safe_from_a_codex_parent_runs_claude() {
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["claude", "--subprocess-safe", "-p", "x"]);
    cmd.env("AMPLIHACK_AGENT_BINARY", "codex")
        .env("AMPLIHACK_AGENT_BINARY_SOURCE", "session:codex");
    fx.run_ok(cmd);

    let records = fx.records();
    assert_eq!(records.len(), 1, "{}", fx.logs());
    records[0].assert_cli(Cli::Claude);
}
```

## Fake CLI record format

`claude` and `codex` are the same script. Each run answers `--version`, or
writes one record file under `$RECORD_DIR` with `mktemp` and exits 0. A record
is a list of `key=value` lines; the parser splits on the first `=` and rejects
unknown keys.

```text
seq=4
ran=codex
self=/d0/ryan/tmp/.tmpQx3k/tools/codex
role=step:sub
argv=exec --output-last-message /d0/ryan/tmp/.tmpQx3k/records/last.4 -
binary=codex
source=session:codex
nonce_set=true
CLAUDECODE=<unset>
CLAUDE_CODE=<unset>
CLAUDE_CODE_SESSION_ID=<unset>
CLAUDE_PROJECT_DIR=<unset>
CLAUDE_CODE_ENTRYPOINT=<unset>
COPILOT_CLI=<unset>
GITHUB_COPILOT=<unset>
GITHUB_COPILOT_AGENT=<unset>
COPILOT_AGENT=<unset>
children_see=codex
```

| Key | Meaning |
|---|---|
| `seq` | Order of invocation, from a counter file taken under `flock` |
| `ran` | Name the script was invoked as |
| `self` | `realpath` of `$0`; must lie under `<tmp>/tools` |
| `role` | `$FAKE_ROLE`: `top`, `nested`, or `step:outer`, `step:sub`, `step:subsub` |
| `argv` | Arguments, newlines flattened to spaces |
| `binary` | `AMPLIHACK_AGENT_BINARY` as the fake received it |
| `source` | `AMPLIHACK_AGENT_BINARY_SOURCE` as the fake received it |
| `nonce_set` | Whether `AMPLIHACK_SESSION_NONCE` was set; the value is never written |
| marker names | Each name in `agent_binary::SESSION_MARKERS`, or `<unset>` |
| `children_see` | First word of `$AMPLIHACK_UNDER_TEST agent-binary` run by the fake |

`children_see` is the point of the record: it is what any command started by
the agent would resolve, measured in a process that inherited the agent's
environment. Without `--shell`, `amplihack agent-binary` prints the binary
followed by the resolution source in brackets, for example
`codex (session_marker)`. The fake keeps only the first word, so the record
holds `codex` and `Record::assert_cli` compares it with the CLI name directly.
The source label is not asserted, because the `source` key already records the
tag the agent received.

Other behaviour of the fake:

- For `exec ... -` the codex fake reads stdin to the end, so the caller never
  sees a broken pipe.
- An `--output-last-message` path outside `$RECORD_DIR` and `$WORK_DIR` makes
  the fake exit 43.
- If `FAKE_THEN` is set, the fake unsets it and runs it with `sh -c` and
  `FAKE_ROLE=nested`, exiting 42 if it fails. When invoked as claude it first
  exports `CLAUDECODE=1` and `CLAUDE_CODE_ENTRYPOINT=cli`, as Claude Code does.
  `FAKE_THEN` values are Rust string literals in the tests, never input.

## Stub recipe runner

`recipe run` finds the stub through `RECIPE_RUNNER_RS_PATH`. The stub copies
the agent step behaviour of the pinned runner (rev `4d5187ea`,
`src/adapters/cli_subprocess.rs`, `build_agent_command`, lines 781 to 866;
the codex branch ends at line 812):

1. `--capabilities` prints `codex_exec`.
2. Otherwise it writes `AMPLIHACK_AGENT_BINARY` and
   `AMPLIHACK_AGENT_BINARY_SOURCE` to `records/runner.env`.
3. It exits 52 unless the binary is `claude` or `codex`.
4. It runs three nested steps, each as
   `env -u CLAUDECODE "$AMPLIHACK_LAUNCHER_BINARY" <cli> -- <adapter argv>`,
   where `AMPLIHACK_LAUNCHER_BINARY` is the `amplihack` binary under test.
5. It prints a successful recipe result as JSON.

| CLI | Adapter argv | Prompt |
|---|---|---|
| claude | `-p <prompt>`, followed by `--system-prompt` and `--add-dir` | argument |
| codex | `exec --output-last-message <file> -` | stdin |

Each step therefore crosses two process boundaries: runner to `amplihack`, and
`amplihack` to the fake CLI.

The stub does not test the real runner. Steps that the real runner adds later
are not covered until the stub copies them.

## Isolation guarantees

`Fixture::cmd` calls `env_clear()` and adds back only these variables:

| Variable | Value | Why |
|---|---|---|
| `HOME` | `<tmp>/home` | Scratch home |
| `TMPDIR` | the test's `$TMPDIR` | Keeps `HOME` inside the temp dir the guard checks |
| `PATH` | `<tmp>/tools:/usr/local/bin:/usr/bin:/bin` | Fakes first |
| `AMPLIHACK_TEST_HOME_GUARD` | `1` | The config loader refuses to write when `HOME` is outside the temp dir |
| `AMPLIHACK_HOME` | `<tmp>/amplihack-home` | Runtime assets resolve here, not under the user's install |
| `AMPLIHACK_CLAUDE_BINARY_PATH`, `AMPLIHACK_CODEX_BINARY_PATH` | the fakes | Launchers start the fakes |
| `RECIPE_RUNNER_RS_PATH` | the stub runner | `recipe run` starts the stub |
| `AMPLIHACK_LAUNCHER_BINARY`, `AMPLIHACK_UNDER_TEST` | the `amplihack` binary under test | Steps and `children_see` use the build under test |
| `RECORD_DIR`, `WORK_DIR` | `<tmp>/records`, `<tmp>/work` | Where the fakes write |
| `AMPLIHACK_NONINTERACTIVE` | `1` | No prompts |
| `AMPLIHACK_NO_FRESHNESS_CHECK`, `AMPLIHACK_NO_UPDATE_CHECK` | `1` | No release or update check over the network |
| `AMPLIHACK_SKIP_AUTO_INSTALL` | `1` | No self-heal install into the scratch home |
| `AMPLIHACK_SKIP_POWER_STEERING`, `AMPLIHACK_SKIP_REFLECTION` | `1` | No launch-time hook set-up |
| `IS_SANDBOX` | `1` | The root-sandbox check passes when tests run as root in a container |
| `AMPLIHACK_ROTATE_AUTOSTART` | `0` | The login rotator does not start |
| `TMUX_TMPDIR` | `<tmp>/tmux`; `TMUX` is never set | No real tmux server is reached |
| `GIT_CONFIG_NOSYSTEM`, `GIT_CONFIG_GLOBAL`, `GIT_TERMINAL_PROMPT` | `1`, `/dev/null`, `0` | No git credential prompt (issue #1764) |

The working directory is `<tmp>/work` and stdin is `/dev/null`. Because the
environment is cleared first, no depth, session or tree variable reaches the
command unless a test sets it.

Self-checks in `record.rs` (and the marker-name check in `script.rs`) fail
the suite if:

- a marker name taken from `SESSION_MARKERS` does not match `^[A-Z_][A-Z0-9_]*$`;
- `cmd()` sets any variable matching `*TOKEN*`, `*KEY*`, `*SECRET*`, `SSH_*` or
  `CLAUDE_CONFIG_DIR`;
- `command -v claude` or `command -v codex` under the fixture `PATH` resolves
  outside `<tmp>/tools`;
- any record's `self` lies outside `<tmp>/tools`.

No test reads or writes `~/.claude`, `~/.claude.json`, `~/.codex` or a
credential file. The scripts contain no `eval`.

## Adding a case

1. Pick the level module, or add one and register it in
   `issue_1983_cli_stays_put.rs` with a `#[path]` line.
2. Build the parent environment with `fx.cmd(...)` and `.env(...)` calls. Never
   call `std::env::set_var` in the test process.
3. Assert on `fx.records()`, never on a value computed in the test process.
4. Add the case in both directions.
5. Add a row to the table in [Levels and covering tests](#levels-and-covering-tests).

Keep each file under 300 lines. The script templates and the marker-name check
live in `script.rs`, and the record parser and fixture self-checks in
`record.rs`, so that `fixture.rs` stays under the limit.

## Known limits

**A stray Claude marker inside a codex session resolves to claude.** If a
Claude marker sits beside `session:codex`, the resolver answers claude. The
environment cannot tell a stray marker from a plain `claude` started inside a
codex shell, where claude is the right answer. `amplihack codex` removes every
Claude marker from its child, so levels 1 to 4 are not affected, and the level
2 codex test asserts that no Claude marker reaches any step.

**An inherited `default:<same tool>` stays a guess.** When `amplihack codex`
starts on an inherited `AMPLIHACK_AGENT_BINARY=codex` tagged `default:codex`,
it passes the value on still tagged `default:codex` (issue #1490). The recipe
adapter itself launches `amplihack <binary>`, so the command line cannot show
whether a person chose the CLI. A `default:` value means no CLI was chosen,
so no run "started with codex" exists. The suite does not assert on this case;
`launch/tests_env.rs` covers it.

**The real runner is not exercised in CI.** See
[Stub recipe runner](#stub-recipe-runner).

## Related

- [Agent Binary Routing](../concepts/agent-binary-routing.md) - How the active CLI is chosen and passed on
- [Active Agent Binary](../reference/active-agent-binary.md) - Resolution order and export tags
- [Subprocess-Level Env Isolation](SUBPROCESS_ENV_ISOLATION_TESTS.md) - Why tests set the environment on a child, not in-process
