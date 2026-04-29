# Manage Tool Update Notifications

> **Scope of this guide**: This guide covers pre-launch npm tool update notices
> only — the check that runs before `claude`, `copilot`, or `codex` is invoked.
> For the separate `amplihack` binary self-update system (GitHub release
> downloads with SHA-256 verification), see the `amplihack update` subcommand.

Before launching `claude`, `copilot`, or `codex`, `amplihack` checks whether a
newer version of the npm-distributed tool is available. When an update is found,
it prints a one-line notice to stderr and continues. This guide explains how to
control that behavior.

> **Command scope:** The update check runs **only** for launch commands
> (`launch`, `claude`, `copilot`, `codex`, `amplifier`). Non-launch subcommands
> (`mode`, `plugin`, `recipe`, `memory`, `install`, `update`, `doctor`, etc.)
> never trigger the check, regardless of environment.

## Contents

- [Default behavior](#default-behavior)
- [Disable the check for one launch](#disable-the-check-for-one-launch)
- [Disable the check permanently](#disable-the-check-permanently)
- [Suppress in CI and pipelines](#suppress-in-ci-and-pipelines)
- [Suppress in parity tests and automation](#suppress-in-parity-tests-and-automation)
- [What happens during `amplihack update`](#what-happens-during-amplihack-update)
- [What the check does](#what-the-check-does)
  - [When the check runs](#when-the-check-runs)
  - [Command allowlist](#command-allowlist)
  - [Tools checked](#tools-checked)
  - [Timeout and failure handling](#timeout-and-failure-handling)
  - [Non-interactive guard](#non-interactive-guard)
  - [Version string sanitisation](#version-string-sanitisation)
- [Security](#security)
- [Related](#related)

---

## Default behavior

When you run `amplihack claude` (or `amplihack copilot`, `amplihack codex`),
`amplihack` runs two quick `npm` queries before handing control to the tool:

1. `npm list -g --depth=0 --json` — reads the installed version.
2. `npm show <package> version` — queries the registry for the latest version.

If the installed version is behind the latest, you see a one-line notice on
stderr:

```
amplihack: claude-code update available: 1.2.3 → 1.4.0  (run: npm install -g @anthropic-ai/claude-code)
```

The launch then proceeds normally. `amplihack` **never auto-installs** the
update.

The check completes in under 3 seconds. If `npm` is not on `PATH` or the
registry is unreachable, the check is silently skipped.

---

## Disable the check for one launch

Pass `--skip-update-check` to suppress the check for a single invocation:

```sh
amplihack claude --skip-update-check
```

```sh
amplihack copilot --skip-update-check --resume
```

The flag is available on every launch subcommand (`claude`, `copilot`, `codex`,
`amplifier`). It is not persisted — the next invocation without the flag will
check again.

---

## Disable the check permanently

Set `AMPLIHACK_NONINTERACTIVE=1` in your shell profile to suppress the check on
every invocation. This also suppresses interactive bootstrap prompts:

```sh
# ~/.bashrc or ~/.zshrc
export AMPLIHACK_NONINTERACTIVE=1
```

After reloading your shell (`source ~/.bashrc`), every `amplihack` invocation
skips the update check.

To suppress only the update check without enabling full non-interactive mode,
add a shell alias:

```sh
# ~/.bashrc or ~/.zshrc
alias amplihack='amplihack --skip-update-check'
```

---

## Suppress in CI and pipelines

In CI environments, use `AMPLIHACK_NONINTERACTIVE=1`. This is the recommended
approach for GitHub Actions, Docker containers, and any scripted usage:

```yaml
# .github/workflows/example.yml
env:
  AMPLIHACK_NONINTERACTIVE: "1"

steps:
  - run: amplihack claude --print 'Fix the lint errors'
```

Or inline on a single step:

```sh
AMPLIHACK_NONINTERACTIVE=1 amplihack claude --print 'Run the test suite'
```

See [Run amplihack in Non-interactive Mode](./run-in-noninteractive-mode.md) for
the full CI configuration guide.

---

## Suppress in parity tests and automation

Test harnesses that compare the Rust and Python CLIs must suppress the update
check to avoid spurious stderr mismatches. Set `AMPLIHACK_PARITY_TEST=1`:

```sh
AMPLIHACK_PARITY_TEST=1 amplihack claude --print 'Run tests'
```

The parity test harness (`tests/parity/parity_audit_cycle.py`) sets this
variable automatically in every sandbox it creates. You do not need to set it
manually when running the harness; it is documented here so that custom
automation scripts know how to replicate the same suppression.

`AMPLIHACK_PARITY_TEST=1` suppresses **only** the update check. It does not
enable non-interactive mode or change any other launch behaviour. Use
`AMPLIHACK_NONINTERACTIVE=1` if you also need to suppress interactive bootstrap
prompts.

> **Isolation:** `AMPLIHACK_PARITY_TEST` is intentionally separate from
> `AMPLIHACK_NONINTERACTIVE` so that tests can verify interactive-mode behaviour
> without triggering the update check.

---

## What happens during `amplihack update`

This section is about the **separate** `amplihack update` subcommand (binary
self-update), not the npm pre-launch notice covered above. It is included here
because users who manage update notices commonly also want to know what the
self-update does.

When you run `amplihack update`, the following happens in order:

1. **Binary self-update.** `amplihack` checks GitHub for a newer release,
   downloads the platform archive, verifies its SHA-256, and atomically
   replaces the running executable.
2. **Automatic framework install.** As soon as the binary swap succeeds,
   `amplihack` runs the same logic as `amplihack install` (in-process — no
   subprocess) to re-stage framework assets (agents, hooks, prompts, recipes)
   under `~/.amplihack/.claude`. This step is non-interactive and uses the
   default install location.
3. **Done.** Both the binary and the on-disk framework now match.

If the binary swap fails, the install step does **not** run and the original
binary stays in place.

### Opt out of the automatic install

Pass `--skip-install` (or its alias `--no-install`) to perform a binary-only
update — the legacy behavior:

```sh
amplihack update --skip-install
# or
amplihack update --no-install
```

You may want this when:

- You have hand-edited files under `~/.amplihack/.claude` and want to merge
  them manually.
- You are testing a new binary against an older framework layout.
- You manage framework assets out-of-band (e.g. via configuration management).

After a `--skip-install` update, you can refresh assets later with:

```sh
amplihack install
```

### Startup-prompt updates

When `amplihack` prompts you to update at startup and you answer `y`, the
**full** flow runs (binary swap **and** install). There is no way to pass
`--skip-install` through the startup prompt. To get the legacy binary-only
behavior, answer `N` to the prompt and then run
`amplihack update --skip-install` manually.

### Failure handling

| Phase                | Outcome                                                                                                          |
| -------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Binary download/swap fails | Original binary preserved; install step is skipped; error printed to stderr; exit non-zero.                |
| Binary swap succeeds, install fails | New binary installed; framework assets may be in an inconsistent state relative to the new binary; error printed to stderr; exit non-zero. Re-run `amplihack install` to retry. |
| `--skip-install` passed | Binary updated; install step is skipped intentionally; a "skipping post-update install" notice is logged at info level. |

---

## What the check does

### When the check runs

The update check runs **before** `main()` parses CLI arguments and **before**
the subcommand dispatch loop. This means it is the first thing that executes on
every invocation — but it is immediately short-circuited by the [command
allowlist](#command-allowlist) and the [non-interactive guard](#non-interactive-guard)
for the vast majority of subcommands.

In the full launch sequence for `amplihack claude [args]`:

```
amplihack claude [args]
   │
   ├── 1. maybe_print_update_notice_from_args()   ← THIS STEP
   │      (no-op unless: launch cmd + interactive + not suppressed)
   ├── 2. Cli::parse_from(args)
   ├── 3. commands::dispatch()
   │      └── launch::run_launch("claude", ...)
   │             ├── a. Nested-launch detection
   │             ├── b. bootstrap::prepare_launcher()
   │             └── c. bootstrap::ensure_tool_available()
   └── (tool starts)
```

### Command allowlist

The update check runs **only** when the first argument matches one of these
launch commands:

| Argument      | Triggers update check |
|---------------|-----------------------|
| `launch`      | yes                   |
| `claude`      | yes                   |
| `copilot`     | yes                   |
| `codex`       | yes                   |
| `amplifier`   | yes                   |
| anything else | **no**                |

This means `amplihack mode detect`, `amplihack plugin list`, `amplihack recipe
run`, `amplihack memory tree`, `amplihack install`, `amplihack update`, and
every other non-launch subcommand never spawn `npm` subprocesses. The check is
allowlist-based (not denylist-based) so that new subcommands added in the future
default to the safe, non-checking behaviour.

### Tools checked

| Launch command       | npm package                      |
|----------------------|----------------------------------|
| `amplihack claude`   | `@anthropic-ai/claude-code`      |
| `amplihack copilot`  | `@github/github-copilot-cli`     |
| `amplihack codex`    | `@openai/codex`                  |
| `amplihack amplifier`| *(not npm-distributed, skipped)* |

### Timeout and failure handling

Each `npm` subprocess has a hard 3-second timeout. If it does not respond in
time, the check is silently abandoned and the launch proceeds. The timeout
applies independently to the `list` and `show` calls.

The check never fails the launch. All errors (missing `npm`, network timeout,
malformed registry response) are silently ignored.

### Non-interactive guard

The check is skipped unconditionally when **any** of the following conditions is
true (checked in this order):

| Condition | Variable / flag | Typical use |
|-----------|----------------|-------------|
| Legacy explicit opt-out | `AMPLIHACK_NO_UPDATE_CHECK=1` | permanent per-user disable |
| Non-interactive mode | `AMPLIHACK_NONINTERACTIVE=1` | CI, pipes, Docker |
| Parity / automation test | `AMPLIHACK_PARITY_TEST=1` | test harnesses |
| Per-invocation opt-out | `--skip-update-check` flag | one-off suppression |

If any condition is true, no `npm` subprocesses are spawned. The conditions are
evaluated before the command-allowlist check, so a suppressed invocation exits
the guard immediately without inspecting the subcommand name.

### Version string sanitisation

Registry responses are sanitised before display: only characters matching
`[a-zA-Z0-9.\-+]` are printed. This prevents ANSI escape sequences from
corrupting your terminal if a malicious or misconfigured registry returned
unexpected content.

---

## Security

All version strings returned from the npm registry are passed through
`sanitize_version()` before being written to stderr. This function allows only
`[a-zA-Z0-9.\-+]` characters — stripping anything else, including ANSI terminal
escape sequences.

**Threat model:** A compromised or malicious npm registry could return a version
string containing escape sequences that manipulate terminal state (e.g. moving
the cursor, clearing lines, or injecting false output). The filter ensures that
even a worst-case registry response cannot corrupt your terminal or inject
visible text.

> **Do not remove or bypass this filter.** Stripping the `sanitize_version()`
> call or widening its character set is a security regression, not a cleanup.
> The filter must run on all registry-sourced strings before any display or
> logging.

---

## Related

- [Run amplihack in Non-interactive Mode](./run-in-noninteractive-mode.md) — Full CI and pipeline guide
- [Environment Variables](../reference/environment-variables.md) — `AMPLIHACK_NONINTERACTIVE`, `AMPLIHACK_PARITY_TEST`, and `AMPLIHACK_NO_UPDATE_CHECK` reference
- [Launch Flag Injection](../reference/launch-flag-injection.md) — How `amplihack` builds the subprocess command line
- [Launch Flag Injection](../reference/launch-flag-injection.md) — How `amplihack` builds the subprocess command line for launch subcommands
