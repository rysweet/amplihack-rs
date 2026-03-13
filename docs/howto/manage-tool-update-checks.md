# Manage Tool Update Notifications

> **Scope of this guide**: This guide covers pre-launch npm tool update notices
> only — the check that runs before `claude`, `copilot`, or `codex` is invoked.
> For the separate `amplihack` binary self-update system (GitHub release
> downloads with SHA-256 verification), see
> [Update the amplihack Binary](../reference/update.md).

Before launching `claude`, `copilot`, or `codex`, `amplihack` checks whether a
newer version of the npm-distributed tool is available. When an update is found,
it prints a one-line notice to stderr and continues. This guide explains how to
control that behavior.

## Contents

- [Default behavior](#default-behavior)
- [Disable the check for one launch](#disable-the-check-for-one-launch)
- [Disable the check permanently](#disable-the-check-permanently)
- [Suppress in CI and pipelines](#suppress-in-ci-and-pipelines)
- [What the check does](#what-the-check-does)
  - [When the check runs](#when-the-check-runs)
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

## What the check does

### When the check runs

The update check runs **after** nested-launch detection and **before** tool
availability is verified. This ordering has two consequences:

1. If `amplihack` is called from within a Claude session (nested launch), the
   update check is automatically suppressed — no `npm` subprocesses are spawned.
2. If the tool is not installed, you will see the update notice first, followed
   by the tool-availability prompt. This is expected behaviour, not an error.

In the launch sequence:

```
amplihack claude [args]
   │
   ├── 1. Nested-launch detection         ← check suppressed if nested
   ├── 2. npm tool update check           ← THIS STEP
   └── 3. bootstrap::ensure_tool_available
```

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

The check is skipped unconditionally when:

1. `AMPLIHACK_NONINTERACTIVE=1` is set in the environment, **or**
2. `--skip-update-check` is passed on the command line.

The `AMPLIHACK_NONINTERACTIVE` check runs first. If either condition is true no
`npm` subprocesses are spawned.

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
- [Environment Variables](../reference/environment-variables.md) — `AMPLIHACK_NONINTERACTIVE` reference
- [amplihack launch](../reference/launch-command.md) — Full CLI reference for launch subcommands
