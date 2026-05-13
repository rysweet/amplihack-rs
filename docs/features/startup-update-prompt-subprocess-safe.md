# Startup Self-Update Prompt: Subprocess-Safe Skip

> [Home](../index.md) > [Features](README.md) > Startup Update Prompt — Subprocess-Safe Skip

**Issue:** [#625](https://github.com/rysweet/amplihack-rs/issues/625)
**Status:** Shipped
**Scope:** `crates/amplihack-cli` — startup self-update prompt
(`update::maybe_print_update_notice_from_args`). Distinct from the npm
pre-launch tool notice — see [Manage Tool Update
Notifications](../howto/manage-tool-update-checks.md) for that path.

## Problem

`amplihack` runs a startup self-update check before dispatch on launch
subcommands (`launch`, `claude`, `copilot`, `codex`, `amplifier`). If a newer
GitHub release is available, it prints to stderr:

```
A newer version of amplihack is available: 0.9.2 → 0.9.3
Update now? [y/N] (5s timeout):
```

The prompt has a hard 5-second `libc::poll` wall-clock timeout on stdin, after
which it defaults to `N` and continues. That timeout is correct for an
interactive terminal — but in a delegated subprocess (engineer agent, recipe
runner step, CI shell pipeline), stdin is typically a pipe or `/dev/null` and
the process *never* sees a newline. The 5-second wall-clock timeout fires
correctly, but 5 seconds per delegated subprocess invocation is unacceptable
for engineer agents that may spawn `amplihack copilot` dozens of times per
task — and the prompt itself appearing in delegated agent logs is itself a
surprise that pollutes captured stderr.

The result was a 5-second stall in front of every subprocess invocation of
`amplihack copilot`, `amplihack claude`, etc., even though the caller had no
way to answer the prompt.

## How it works

Before the prompt is printed, `amplihack` classifies the invocation. If
**any** of five subprocess-safe paths is present (four classified by
`classify_skip_reason`, plus the stdin-TTY check applied separately), the
entire update check is skipped and a single notice is written to stderr:

```
amplihack: skipping update check (subprocess-safe / no TTY)
```

The skipped path returns immediately — no network call to the GitHub release
API, no prompt, no read on stdin.

### Subprocess-safe signals

The check is skipped when any of these is true (logical OR; the precedence
shown here matches the evaluation diagram below — within the
`SubprocessSafe` class the order is behaviorally irrelevant since all arms
have the same outcome):

| Signal                      | Detection                                           | Skip-line emitted? |
| --------------------------- | --------------------------------------------------- | :----------------: |
| `AMPLIHACK_NO_UPDATE_CHECK` | Env var set to `1`                                  |        ❌ silent   |
| `AMPLIHACK_PARITY_TEST`     | Env var set to `1`                                  |        ❌ silent   |
| `AMPLIHACK_NONINTERACTIVE`  | Env var set to non-empty value                      |        ✅          |
| `AMPLIHACK_AGENT_BINARY`    | Env var set to non-empty value                      |        ✅          |
| `CI`                        | Env var set to non-empty value (`1`, `true`, etc.)  |        ✅          |
| `--subprocess-safe` in argv | Literal long-form match in pre-clap argument scan   |        ✅          |
| Non-launch subcommand       | `args[1]` not in `{launch, claude, copilot, codex, amplifier}` | ❌ silent   |
| stdin is not a TTY          | `io::stdin().is_terminal() == false` (checked after `classify_skip_reason`) | ✅          |

The skip-line is intentionally **not** emitted for the three "silent" cases so
that:

* `AMPLIHACK_NO_UPDATE_CHECK` users who set the variable to silence the
  update banner permanently do not see a *new* line in its place.
* `AMPLIHACK_PARITY_TEST` runs continue to produce byte-identical stderr
  against a pre-#625 baseline.
* Non-launch subcommands (`amplihack mode`, `amplihack plugin`, `amplihack
  recipe`, `amplihack install`, `amplihack doctor`, …) never produced an
  update check before #625 and continue to produce no extra stderr.

The skip-line **is** emitted for the four `SubprocessSafe` arms and the
non-TTY stdin check (the five emitting paths above) so that operators can
verify in logs that the check was correctly bypassed and is not silently
failing.

### Order of evaluation

```
maybe_print_update_notice_from_args(args)
   │
   ├── Is the platform supported? ──── no ──→ Continue (silent)
   │
   ├── classify_skip_reason(args)              ← pure function: env + argv only
   │     ├── AMPLIHACK_NO_UPDATE_CHECK=1       → ExplicitOptOut  → Continue (silent)
   │     ├── AMPLIHACK_PARITY_TEST=1           → ExplicitOptOut  → Continue (silent)
   │     ├── AMPLIHACK_NONINTERACTIVE non-empty→ SubprocessSafe  → emit skip-line, Continue
   │     ├── AMPLIHACK_AGENT_BINARY non-empty  → SubprocessSafe  → emit skip-line, Continue
   │     ├── CI non-empty                       → SubprocessSafe  → emit skip-line, Continue
   │     ├── argv contains "--subprocess-safe"  → SubprocessSafe  → emit skip-line, Continue
   │     ├── args[1] not in launch allowlist    → NotLaunch       → Continue (silent)
   │     └── (none of the above)                → None — proceed to next step
   │
   ├── Is stdin a TTY? ──── no ──→ emit skip-line, Continue
   │
   └── Run the check, print prompt, read stdin with 5000ms libc::poll timeout
```

The TTY check sits **outside** `classify_skip_reason` so that the function
remains pure (env + argv only). This keeps the existing unit tests at
`crates/amplihack-cli/src/update/tests.rs` deterministic regardless of how
the test runner is wired to a controlling terminal.

### Single emission

The skip-line is emitted at most **once** per process invocation. The match
arms in the entry point are mutually exclusive and each arm `return`s after
emitting, so a process satisfying both `AMPLIHACK_NONINTERACTIVE=1` *and* a
non-TTY stdin sees exactly one line — not two.

### 5-second timeout preserved

When the check **does** run (interactive TTY, no skip signals), the existing
hard wall-clock timeout in `read_user_input_with_timeout` is unchanged:
`libc::poll` with a 5000ms deadline, after which the prompt defaults to `N`
and dispatch continues. This path is exercised by an end-to-end PTY test that
spawns `amplihack copilot --help` under a real pseudo-terminal, asserts the
prompt is printed, sends no input, and asserts the process exits within 7
seconds.

## Usage

### CI / scripted shell pipelines

Set `CI=true` (most CI runners do this automatically — GitHub Actions,
GitLab CI, CircleCI, Jenkins, Buildkite). No further configuration is
needed:

```yaml
# .github/workflows/agent.yml
jobs:
  agent:
    runs-on: ubuntu-latest
    # CI=true is set automatically by the runner.
    steps:
      - run: amplihack copilot -p "Run the test suite"
```

Stderr will contain:

```
amplihack: skipping update check (subprocess-safe / no TTY)
```

### Delegated agent invocation

When a parent agent process spawns `amplihack copilot` as a subprocess, set
`AMPLIHACK_AGENT_BINARY` to mark the child as a delegate:

```bash
AMPLIHACK_AGENT_BINARY=copilot amplihack copilot -p "Implement the design spec"
```

This is also what the recipe runner does internally when launching agent
sessions, so recipe steps that invoke `amplihack copilot` already trigger the
skip without further configuration.

### Explicit per-invocation opt-out

Add `--subprocess-safe` to any launch subcommand to force the skip
unconditionally, even at an interactive terminal:

```bash
amplihack copilot --subprocess-safe -p "Refactor this module"
```

### Permanent per-user opt-out (silent — no skip-line)

Set `AMPLIHACK_NO_UPDATE_CHECK=1` in your shell profile to disable the
prompt without producing the new skip-line in stderr. Use this if you want
the pre-#625 silent-skip experience:

```sh
# ~/.bashrc
export AMPLIHACK_NO_UPDATE_CHECK=1
```

### Piped stdin

Even with no env vars and no flags, redirecting stdin from a pipe or
`/dev/null` is enough to trigger the skip:

```bash
amplihack copilot </dev/null -p "Headless run"
# Skip-line printed; no prompt; returns within seconds.
```

This is the bug-fix path for the original report — engineer subprocesses that
inherited closed/redirected stdin no longer hang on the prompt.

## Configuration Reference

### Environment variables

| Variable                    | Effect                                                                                          | Skip-line? |
| --------------------------- | ----------------------------------------------------------------------------------------------- | :--------: |
| `AMPLIHACK_NONINTERACTIVE`  | Set to any non-empty value → skip update check.                                                  |    ✅      |
| `AMPLIHACK_AGENT_BINARY`    | Set to any non-empty value → skip update check. (Set automatically by parent agent runtimes.)    |    ✅      |
| `CI`                        | Set to any non-empty value (`1`, `true`, anything) → skip update check.                          |    ✅      |
| `AMPLIHACK_NO_UPDATE_CHECK` | Set to `1` → silently skip update check; no skip-line.                                           |    ❌      |
| `AMPLIHACK_PARITY_TEST`     | Set to `1` → silently skip update check; no skip-line; preserves byte-identical stderr.          |    ❌      |

> **Empty string semantics:** `AMPLIHACK_NONINTERACTIVE`,
> `AMPLIHACK_AGENT_BINARY`, and `CI` skip on **non-empty** values only.
> Setting `CI=""` does **not** trigger skip — this matches the convention
> used by `commands::launch::command::resolve_subprocess_safe`.

### Flags

| Flag                | Type | Effect                                                                                       |
| ------------------- | ---- | -------------------------------------------------------------------------------------------- |
| `--subprocess-safe` | bool | Pre-clap argv literal scan. When present anywhere in argv, skip update check + emit skip-line. |

The flag is matched by literal `OsStr` equality against the long-form
`--subprocess-safe` token. Short forms, prefix matches, and embedded
substrings are not recognized.

### Skip-line wording

The exact line emitted to stderr is:

```
amplihack: skipping update check (subprocess-safe / no TTY)
```

The wording is intentionally identical regardless of which signal fired — it
is a single ASCII string with no env value interpolation, so log scrapers can
match on the literal substring. The line is written via `eprintln!` (newline
included).

### Test-only synthetic release

For deterministic integration tests of the prompt code path,
`fetch_latest_release` honors a test-only environment variable:

| Variable                            | Effect                                                                                                                           |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `AMPLIHACK_TEST_FAKE_LATEST_VERSION`| When set non-empty, returns a synthetic `UpdateRelease` with the given tag (validated through `normalize_tag`) and no network call. |

The synthetic release's `asset_url` points to an allowlisted `github.com`
host and `checksum_url` is `None`, so any code that *would* download is still
gated by the existing URL allowlist — the variable cannot be used to redirect
real downloads. It exists exclusively to drive the prompt code path
deterministically from the integration test suite.

This variable is documented for completeness; production deployments should
not set it.

## Examples

### Verifying the skip in a CI pipeline

```bash
$ CI=true amplihack copilot --help 2>&1 | head -1
amplihack: skipping update check (subprocess-safe / no TTY)
```

### Verifying interactive behavior is unchanged

At a real terminal, with no env vars and no flag, the prompt still appears
and still honors the 5-second timeout:

```bash
$ amplihack copilot --help
A newer version of amplihack is available: 0.9.2 → 0.9.3
Update now? [y/N] (5s timeout):
   ⏱  (waits up to 5s, then defaults to N)
   ... copilot --help output follows ...
```

### Inspecting which signal fired

The skip-line is identical for all signals (no enumeration), so to identify
which signal triggered the skip in a debugging session, inspect the
environment directly:

```bash
$ env | grep -E '^(CI|AMPLIHACK_AGENT_BINARY|AMPLIHACK_NONINTERACTIVE)='
CI=true
```

If you need per-signal accounting in CI logs, wrap the invocation:

```bash
echo "[debug] CI=${CI:-} AGENT_BINARY=${AMPLIHACK_AGENT_BINARY:-} NONINT=${AMPLIHACK_NONINTERACTIVE:-}"
amplihack copilot -p "..."
```

### Engineer subprocess (delegated agent)

A parent agent that delegates to `amplihack copilot` should set
`AMPLIHACK_AGENT_BINARY=copilot` before spawning the child:

```rust
let mut cmd = std::process::Command::new("amplihack");
cmd.arg("copilot")
   .arg("-p").arg(task)
   .env("AMPLIHACK_AGENT_BINARY", "copilot");
let status = cmd.status()?;
```

The child inherits the env var, classifies as subprocess-safe, skips the
prompt, emits the skip-line, and proceeds to dispatch within milliseconds.

## Migration notes

### For interactive `amplihack` users

**No action required.** Interactive TTY behavior is unchanged. You still see
the `Update now? [y/N] (5s timeout):` prompt and still have 5 seconds to
answer.

### For CI maintainers

**No action required for most CI runners.** GitHub Actions, GitLab CI,
CircleCI, Jenkins, and Buildkite all set `CI=true` automatically. The skip
fires without configuration changes. If your CI runner does *not* set `CI`,
either:

* Add `env: CI: "true"` to the workflow, or
* Set `AMPLIHACK_NONINTERACTIVE=1` (preserves the older convention).

### For parity-test harnesses

**No action required.** `AMPLIHACK_PARITY_TEST=1` continues to be the
silent-skip path. It does not produce the new skip-line and so does not
introduce stderr diffs against a pre-#625 baseline.

### For callers that previously worked around the hang

Before #625, callers worked around the hang by piping `</dev/null` and
adding short shell timeouts. These workarounds remain harmless but are no
longer required:

```bash
# Before: defensive workaround
timeout 10 amplihack copilot </dev/null -p "Headless task" || true

# After (any of these works without timeout/redirection):
CI=true                           amplihack copilot -p "Headless task"
AMPLIHACK_AGENT_BINARY=copilot    amplihack copilot -p "Headless task"
AMPLIHACK_NONINTERACTIVE=1        amplihack copilot -p "Headless task"
                                  amplihack copilot --subprocess-safe -p "Headless task"
```

The implicit non-TTY skip also covers the `</dev/null` case automatically.

> **Note for permanent `AMPLIHACK_NONINTERACTIVE=1` users:** Pre-#625 this
> variable silently skipped the prompt. Post-#625 it is classified as
> `SubprocessSafe` and emits the new skip-line
> (`amplihack: skipping update check (subprocess-safe / no TTY)`). If you
> rely on byte-identical stderr (e.g. snapshot tests or stderr-diff parity
> harnesses), switch to `AMPLIHACK_NO_UPDATE_CHECK=1` or
> `AMPLIHACK_PARITY_TEST=1` — both remain silent-skip paths.

## Out of Scope

This feature does **not**:

* Modify `update::auto_update::prompt_and_upgrade` (a separate dead-code
  prompt path scheduled for removal in a follow-up).
* Modify the npm pre-launch tool notice (`tool_update_check/`) — that path
  is non-interactive and never blocked. See [Manage Tool Update
  Notifications](../howto/manage-tool-update-checks.md).
* Change the default interactive behavior — the prompt still appears at a
  real terminal and still has a 5-second timeout.
* Introduce a new update mechanism, telemetry, or auto-install policy.
* Modify any subcommand other than the launch allowlist (`launch`, `claude`,
  `copilot`, `codex`, `amplifier`); non-launch subcommands continue to skip
  the check silently as they always did.

## Security Considerations

* **No new attack surface.** `classify_skip_reason` is a pure function over
  env vars and argv — it never shells out, never opens files, and never
  constructs paths from env values. The five subprocess-safe signals are
  presence checks (`!is_empty()`) only; the values themselves are never
  logged, interpolated, or otherwise reflected back into output.
* **No log injection.** The skip-line is a hard-coded ASCII literal
  containing no env-derived bytes. A hostile parent process cannot craft a
  `CI` value that injects ANSI escape sequences or trailing log lines.
* **`--subprocess-safe` matched by literal `OsStr` equality.** Prefix
  matches (`--subprocess-safe-extra`), short forms (`-s`), and embedded
  substrings are not recognized.
* **5-second hard timeout preserved.** `read_user_input_with_timeout` keeps
  its `libc::poll` 5000ms deadline. The fix narrows the cases in which the
  prompt is shown at all; it does not weaken the timeout for cases in which
  it still is.
* **Synthetic release env var (`AMPLIHACK_TEST_FAKE_LATEST_VERSION`) is
  guarded** by `!tag.is_empty()`, validates through `normalize_tag`, returns
  an `asset_url` on the allowlisted `github.com` host, and sets
  `checksum_url=None` so any download path remains gated by the existing
  URL allowlist. The variable cannot be used to redirect real downloads or
  bypass SHA-256 verification.
* **No `unsafe`, no `unwrap`, no panics added.** All env reads use
  `std::env::var(...).ok()` and `.is_empty()` checks; argv scanning uses
  iterator combinators.

## See also

* [Manage Tool Update Notifications](../howto/manage-tool-update-checks.md) —
  npm pre-launch tool notice (separate code path).
* [`COPILOT_SUBPROCESS_SAFE.md`](../COPILOT_SUBPROCESS_SAFE.md) — Related
  subprocess-safe defaults for `amplihack copilot` argv injection (issue
  [#621](https://github.com/rysweet/amplihack-rs/issues/621)).
* [Environment Variables](../reference/environment-variables.md) — Full
  reference for `AMPLIHACK_NONINTERACTIVE`, `AMPLIHACK_NO_UPDATE_CHECK`,
  `AMPLIHACK_PARITY_TEST`, `AMPLIHACK_AGENT_BINARY`, and `CI`.
* [`cli.md` — `amplihack update` reference](../reference/cli.md) —
  `amplihack update` subcommand and the startup-prompt flow.
* [Issue #625](https://github.com/rysweet/amplihack-rs/issues/625) — Source
  issue.
