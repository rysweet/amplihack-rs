---
title: No default time limits on work
type: explanation
issue: 2022
---

# No default time limits on work

amplihack puts no time limit on work unless you set one. A recipe run, an
agent step, a build, a test, a hook's real job and an auto-drive loop each
run until they finish, fail or are stopped. A limit exists only when someone
sets it in an environment variable, a config key, a command-line flag or a
builder call.

## Contents

- [What counts as work](#what-counts-as-work)
- [Why there is no default](#why-there-is-no-default)
- [Limits you can set](#limits-you-can-set)
- [Upgrading from the 6-hour default](#upgrading-from-the-6-hour-default)
- [How to stop a run that has no limit](#how-to-stop-a-run-that-has-no-limit)
- [Timeouts that stay: hang guards on probes](#timeouts-that-stay-hang-guards-on-probes)
- [Hook timeouts in settings.json](#hook-timeouts-in-settingsjson)
- [Values that look like limits but are not](#values-that-look-like-limits-but-are-not)
- [Rules for new code](#rules-for-new-code)
- [Related](#related)

## What counts as work

Work is anything whose length depends on the task:

- a whole recipe run (`amplihack recipe run`)
- one agent step, including a Copilot CLI, Claude or Codex call
- `cargo build`, `cargo test` and other builds and test runs
- the real job of a hook, such as a review or a precommit check
- an auto-drive loop and the recipes it calls
- a wait for a usage limit to reset

A **probe** is a short call whose length does not depend on the task: a
version check, a `git`, `gh` or `tmux` query, a start-up handshake, taking a
lock or draining a pipe. Probes keep short timeouts, because a probe that
hangs is a fault, not slow work.

## Why there is no default

A default limit stops long, correct work. A six-hour recipe run that is
halfway through a large refactor loses that work when the limit fires, and
the run's state is left `indeterminate`. The owner's rule (issue #2022) is
that this must never happen by default.

Hangs are handled by other means:

- The [idle watchdog](./idle-watchdog.md) ends an agent child that produces
  no output for its idle window. It measures silence, not elapsed time.
- `amplihack recipe stop`, Ctrl-C, `SIGTERM` and `SIGHUP` stop a run at any
  time (#1606).
- Probes keep their own short timeouts.

## Limits you can set

Every row below means **no limit unless set**. For the step timeout,
"unless set" covers the recipe too: when none of the step-timeout inputs is
set, a `timeout_seconds` value written in a recipe's own YAML still applies
to that step. The bundled default-workflow agent steps define none.

| Setting | Bounds | Set it with | Effect of `0` |
|---|---|---|---|
| [`AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS`](../reference/environment-variables.md#amplihack_recipe_runner_timeout_secs) | One whole recipe run | Environment variable, whole seconds above 0 | No limit |
| [`AMPLIHACK_STEP_TIMEOUT`](../reference/environment-variables.md#amplihack_step_timeout) / `--step-timeout <SECONDS>` | Each recipe step; overrides each step's YAML `timeout_seconds` | Flag or variable | No limit, even where a step's YAML sets `timeout_seconds` |
| [`recipe.step_timeout`](../reference/user-config.md#recipestep_timeout) | Each recipe step; overrides each step's YAML `timeout_seconds` | Config key, whole seconds of at least 1 | Rejected as invalid; use `--step-timeout 0` to turn step limits off |
| [`recipe.usage_limit_max_wait_secs`](../reference/user-config.md#recipeusage_limit_wait) / [`AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS`](../reference/environment-variables.md#amplihack_usage_limit_max_wait_secs) | One wait for a usage limit to reset | Config key or variable | One more attempt with no wait; a capacity wait fails at once as `usage_limit` |
| `CopilotCliClient::with_timeout` | One Copilot CLI agent call | Builder call in Rust | Not applicable: takes a `Duration` |
| `AgentConfig.timeout_secs`, `TaskSpec.timeout_secs`, `SdkAdapterConfig.timeout_secs` | Recorded on agent-core configs; not enforced | Config field or `with_timeout` | Stored as `Some(0)`; never means "unlimited" |
| `--max-runtime` | One multitask workstream | Command-line flag | Not applicable |
| [`autodrive_budget_wall_seconds`](../reference/auto-drive-to-merge.md) | Time an auto-drive spends in its children, less usage-limit waits | Recipe context, `-c autodrive_budget_wall_seconds=N` | No limit; `0` or empty is the default in every bundled recipe |
| [`GH_RETRY_MAX_RL_WINDOWS`](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md) | Number of GitHub rate-limit reset windows `workflow_gh_retry.sh` waits through | Environment variable, whole number | Fail at once on a rate limit; display-only reads such as `workflow_final_status.sh` pin this |

An unparseable `AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS` is ignored. amplihack
prints one warning on standard error that names the variable but not its
value, and no limit applies:

```
warning: ignored AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS (not a whole number of seconds above 0); no limit applies
```

An invalid `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` is also reported by name
and ignored, but the `recipe.usage_limit_max_wait_secs` value from a config
file then applies; with no config value there is no limit.

To give a CI recipe run at most four hours:

```sh
AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS=14400 \
amplihack recipe run default-workflow \
  -c task_description="Refresh the dependency lock file"
```

When a set limit is reached, amplihack behaves as it did before #2022: it
stops the runner's process group, writes the terminal record, classifies the
run `indeterminate` and prints `recipe-runner-rs timed out after <N>s ...`.

The agent-core types are described in the
[agent-core API reference](../reference/agent-core-api.md#timeout-fields).

## Upgrading from the 6-hour default

Before #2022, `amplihack recipe run` stopped the runner after 6 hours
(`RECIPE_RUNNER_DEFAULT_TIMEOUT`, 21600 s), and
`AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS=0` was ignored and fell back to that
default. Long auto-drives were ended mid-round with
`recipe-runner-rs timed out after 21600s` and classed `indeterminate`.

That default no longer exists. A run that relied on it now runs until it
exits or is stopped. To keep the old ceiling, set it explicitly:

```sh
AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS=21600 \
amplihack recipe run default-workflow \
  -c task_description="Refresh the dependency lock file"
```

Scripts that set `AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS=0` to mean "no limit"
need no change: `0` now means no limit. The same holds for
`autodrive_budget_wall_seconds=0`, and the auto-drive resume hint suggests
`0` rather than doubling the previous budget.

## How to stop a run that has no limit

With no limit set, the stop signal is the only built-in way to end a run
before it finishes:

```sh
amplihack recipe stop <run-id>
```

Ctrl-C, `SIGTERM` and `SIGHUP` to the `amplihack recipe run` process do the
same. While it waits, the runner checks every 100 ms
(`RECIPE_RUNNER_POLL_INTERVAL`) whether the child has exited or a stop has
been requested. This check is how #1606 works. It sets no deadline: with no
limit configured, the wait has no branch that ends it on elapsed time.

## Timeouts that stay: hang guards on probes

These guard a quick call against a hang. They never wrap a build, a test, an
agent step or a recipe run.

| Kind | Examples | Reason |
|---|---|---|
| Version and readiness checks | `claude --version`, `copilot --version`, `node --version` | A version check that hangs means a broken install |
| `git`, `gh`, `az` and `tmux` queries | `timeout 30`/`timeout 60` around `gh` and `az` reads in `workflow-prep`, `workflow-design`, `workflow-terminal-state` | A read query that hangs means a network or auth fault |
| `gh` reads and writes through the retry helper | `timeout 60 gh ...` in `workflow_gh_retry.sh` and `workflow-terminal-state` | A single `gh` call that hangs means a network or auth fault |
| Identity preflight | `AMPLIHACK_IDENTITY_PREFLIGHT_TIMEOUT` (default 30 s) in `workflow_identity_preflight.sh` | Bounds each `gh` identity query before a run starts |
| `gh pr create` in `quick-change` | `timeout 1800` | Hang guard on one GitHub write |
| Auto-drive stop comment | `timeout 900` around one `gh pr comment` in `tools/autodrive_budget.sh` | Bounds one comment post after a loop has already stopped; it does not limit the drive |
| Start-up handshakes | Codex app-server and MCP handshakes; the `amplihack-remote` tmux session launch in `execute_remote_tmux` (60 s) | The peer is not answering; the launch limit covers only starting the session, not the agent run inside it |
| Lock acquisition and pipe draining | Recipe-run lock, child stdout and stderr drain after exit | Prevents a deadlock on a stuck peer |
| Installer and network setup | `curl`, `tar`, `npm`, `apt` and self-upgrade steps (300 s); `git clone` (120 or 300 s); in the `amplihack-remote` executor, the context archive upload (600 s per attempt, retried with backoff within a 60 s retry budget by default), the remote log archive, the git bundle and the file download after a run (300 s each); remote provisioning | Network hang guard on one setup or transfer step; the remote agent run itself (`execute_remote`, `execute_remote_tmux`) has no limit unless a timeout is configured |
| Prompt text in bundled recipes | `setsid -w timeout <secs>` guidance in `autodrive-merge-round` and `workflow-precommit-test`; `primary_timeout` and `timeout_seconds` parameters in `cascade-workflow` | Text the agent reads; amplihack does not enforce it |

## Hook timeouts in settings.json

Claude Code, not amplihack, enforces hook timeouts from `settings.json`.
These are the values amplihack installs:

| Hook | Timeout (seconds) |
|---|---|
| `SessionStart` | 10 |
| `Stop` | 120 |
| `PreToolUse` | none |
| `PostToolUse` | none |
| `UserPromptSubmit`: `workflow-classification-reminder` | 5 |
| `UserPromptSubmit`: `user-prompt-submit` | 10 |
| `PreCompact` | 30 |
| `SessionEnd` | 30 |
| `StopFailure`: `limit-resume` | 608400 |

The Claude login rotator waits at most 7 days (`MAX_WAIT`) for a login to
regain capacity.

The `limit-resume` hook ends its own wait early enough to leave
`LIMIT_RESUME_TIMEOUT_MARGIN` (300 s) before the 608400 s timeout, so the
hook always decides, logs and exits before Claude Code stops it. A
compile-time check in `amplihack-utils` keeps `MAX_WAIT` plus the margin
below the registered timeout. The margin bounds no work.

## Values that look like limits but are not

| Value | What it is |
|---|---|
| `AMPLIHACK_RECIPE_TRANSIENT_BUDGET_SECS` (default 300 s) | Bounds only the backoff sleeps between relaunches after a transient fault. It never counts time while an attempt is running, so it cannot end work. |
| 6 h rate window in the capacity summary | Window used to compute a usage rate |
| 3600 s platform-block estimate | Estimate of how long a platform block lasts |
| `SessionManager` 3600 s | Session bookkeeping; it does not end work |
| `RECIPE_RUNNER_POLL_INTERVAL` (100 ms) | How often the runner wait checks for exit or a stop request; it never ends the wait |
| `DEFAULT_LATEST_TAG_TTL_MS` (6 h) in `npm/lib/bootstrap.js` | How long the npm launcher caches the latest release tag |
| `max_runtime` 7200 and 10800 in the multitask skill's `TIMEOUT_LIFECYCLE.md` | Example values for a workstream that sets its own limit; `max_runtime` is optional and has no default |

## Rules for new code

- Store an optional limit as `Option<Duration>` (or `Option<u64>` /
  `Option<f64>` seconds) with no default. `None` means no limit.
- Never use a sentinel such as `0`, `u64::MAX` or `f64::MAX` to mean
  "unlimited".
- Read an environment variable as "a whole number above 0 means a limit;
  anything else means no limit plus one warning". Never print the raw value.
- Do not fix a slow or flaky run by adding or raising a timeout, delay or
  retry count. Wait on the real event: an exit status, a readiness file, a
  channel, a lock or a FIFO.
- Tests prove "no limit" by blocking a stub on a FIFO the test releases, and
  prove "limit N" with a short explicit N on a stub that is never released.
  Because the stub is never released, only the limit can end the wait, so
  the test checks the result (`RunnerWait::TimedOut(N)`, the
  `timed out after Ns` message and the `indeterminate` class) and never
  measures elapsed time, as a lower or an upper bound. Multi-hour waits use
  an injected clock.

## Related

- [Idle watchdog](./idle-watchdog.md) - How agent children are supervised
  without a wall-clock limit
- [Environment variables](../reference/environment-variables.md) - Every
  setting listed above
- [User config](../reference/user-config.md) - Config keys for step timeouts
  and usage-limit waits
- [Troubleshoot recipe execution](../howto/troubleshoot-recipe-execution.md) -
  What to check when a step is timed out
