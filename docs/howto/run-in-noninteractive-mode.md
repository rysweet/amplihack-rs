# How to Run amplihack in Non-Interactive Mode

Use this guide when running `amplihack` in CI pipelines, Docker containers, or any
environment where there is no interactive terminal.

## Contents

- [When to use non-interactive mode](#when-to-use-non-interactive-mode)
- [Activating non-interactive mode](#activating-non-interactive-mode)
  - [Automatic: pipe stdin](#automatic-pipe-stdin)
  - [Explicit: set the environment variable](#explicit-set-the-environment-variable)
- [What changes in non-interactive mode](#what-changes-in-non-interactive-mode)
- [Pre-provisioning requirements](#pre-provisioning-requirements)
- [CI examples](#ci-examples)
  - [GitHub Actions](#github-actions)
  - [Azure DevOps](#azure-devops)
  - [Docker](#docker)
- [Troubleshooting](#troubleshooting)

---

## When to use non-interactive mode

amplihack's bootstrap process (`prepare_launcher`) may prompt the user or open a
browser window to complete framework setup. These steps assume a live terminal and will
hang indefinitely when stdin is not a TTY.

Non-interactive mode skips those steps and assumes the environment has already been
provisioned. Use it whenever:

- Running in a CI pipeline (GitHub Actions, Azure DevOps, Jenkins, etc.)
- Running inside a Docker container without an attached terminal
- Piping input to amplihack from a script
- Running amplihack as a sub-process from another program

---

## Activating non-interactive mode

There are two ways to activate non-interactive mode. Either is sufficient.

### Automatic: pipe stdin

When stdin is not a TTY, non-interactive mode activates automatically. This covers
most CI environments without any configuration:

```sh
echo "" | amplihack launch claude
```

Or run amplihack as a subprocess from any program that does not attach a TTY to stdin.

### Explicit: set the environment variable

Set `AMPLIHACK_NONINTERACTIVE=1` before running amplihack:

```sh
AMPLIHACK_NONINTERACTIVE=1 amplihack launch claude
```

> **Important:** Only the value `"1"` activates non-interactive mode. The values
> `"true"`, `"yes"`, `"on"`, and `"0"` are all treated as falsy. This is a
> cross-language contract shared between the Rust CLI and the Python recipe runner.

---

## What changes in non-interactive mode

| Feature | Interactive mode | Non-interactive mode |
|---------|-----------------|----------------------|
| `prepare_launcher` bootstrap | Runs normally | **Skipped** — returns `Ok(())` immediately |
| User prompts | Displayed | Never shown |
| Browser OAuth flows | May open | Never triggered |
| `AMPLIHACK_NONINTERACTIVE` in child env | Not set | Propagated as `"1"` |
| Tracing output | Normal | `DEBUG: skipping prepare_launcher (non-interactive)` emitted |

The bootstrap skip is logged via `tracing::debug` before returning. This provides an
audit trail in structured log output (visible with `RUST_LOG=debug`).

---

## Pre-provisioning requirements

Because `prepare_launcher` is skipped, the environment must already contain everything
the recipe runner needs:

1. **`AMPLIHACK_HOME`** — set automatically by the launcher from `$HOME/.amplihack`
   unless overridden. The directory must already exist and contain the helper scripts
   installed by a prior `amplihack install` run.

2. **Agent binary** — the tool named by `AMPLIHACK_AGENT_BINARY` (e.g. `claude`) must
   be on `PATH`.

3. **Python environment** — if recipe runner scripts require Python packages, install
   them before the launch step.

**Recommended pre-provision order:**

```sh
# 1. Install amplihack (interactive, do this once)
amplihack install

# 2. In CI — run the launch step
AMPLIHACK_NONINTERACTIVE=1 amplihack launch claude
```

---

## CI examples

### GitHub Actions

```yaml
jobs:
  amplihack:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install amplihack
        run: amplihack install
        # This step is interactive; run it only in jobs that have terminal access
        # or use a pre-built image where install was already run.

      - name: Run amplihack
        env:
          AMPLIHACK_NONINTERACTIVE: "1"
        run: amplihack launch claude
```

> GitHub Actions runners do not attach a TTY to the job shell, so
> `AMPLIHACK_NONINTERACTIVE=1` is equivalent to the automatic piped-stdin detection.
> Setting it explicitly makes the intent clear.

### Azure DevOps

```yaml
steps:
  - script: amplihack launch claude
    displayName: Run amplihack
    env:
      AMPLIHACK_NONINTERACTIVE: "1"
```

### Docker

```dockerfile
FROM ubuntu:24.04

# ... install amplihack, agent binary, etc. ...

# AMPLIHACK_NONINTERACTIVE avoids hangs when container is run without -it
ENV AMPLIHACK_NONINTERACTIVE=1

CMD ["amplihack", "launch", "claude"]
```

Or set it at `docker run` time without modifying the image:

```sh
docker run -e AMPLIHACK_NONINTERACTIVE=1 my-amplihack-image amplihack launch claude
```

---

## Troubleshooting

### amplihack hangs waiting for input

**Cause:** Non-interactive mode was not activated and the bootstrap prompts are waiting
for keyboard input.

**Fix:** Set `AMPLIHACK_NONINTERACTIVE=1` or pipe stdin.

---

### `AMPLIHACK_NONINTERACTIVE=true` has no effect

**Cause:** Only the value `"1"` triggers non-interactive mode. Boolean-like strings
(`"true"`, `"yes"`, `"on"`) are not recognised.

**Fix:**

```sh
# Wrong
AMPLIHACK_NONINTERACTIVE=true amplihack launch claude

# Correct
AMPLIHACK_NONINTERACTIVE=1 amplihack launch claude
```

---

### Recipe runner prompts even though non-interactive mode is active

**Cause:** `AMPLIHACK_NONINTERACTIVE=1` is set in the parent environment but the child
process was launched in a way that strips environment variables.

**Fix:** Verify that `AMPLIHACK_NONINTERACTIVE` is visible inside the child process.
The launcher propagates it automatically via the `EnvBuilder` chain when non-interactive
mode is detected. If you are launching amplihack as a sub-subprocess (e.g. from a
Makefile target that clears the environment), set `AMPLIHACK_NONINTERACTIVE=1`
explicitly in that sub-shell.

---

### Framework resources not found in non-interactive mode

**Cause:** `amplihack install` was never run, so `$HOME/.amplihack` does not contain
the required helper scripts.

**Fix:** Run `amplihack install` once in an interactive session (or in a CI step that
runs before the non-interactive launch step). See
[Install amplihack for the First Time](./first-install.md).
