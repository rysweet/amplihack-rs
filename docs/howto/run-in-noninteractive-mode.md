# How to Run amplihack in Non-interactive Mode

Use this guide when running `amplihack` in CI pipelines, Docker containers, batch scripts, or any environment where stdin is not a terminal and interactive prompts would cause the process to hang.

## When to use this

- CI pipelines (GitHub Actions, Azure DevOps, Jenkins)
- Docker / devcontainer builds
- Scripts that pipe output to other commands
- Sandboxed test environments
- Any invocation where stdin is not a TTY

## The two ways to trigger non-interactive mode

### 1. Pipe stdin (automatic)

If stdin is not a terminal — because you piped input or redirected from a file — `amplihack` detects this automatically and skips interactive prompts.

```sh
# Pipe a prompt directly
echo 'Fix all lint errors and commit' | amplihack claude --print -

# Redirect from a file
amplihack claude --print - < prompt.txt

# In a script where stdin is never a TTY
amplihack claude --print 'Run the test suite and report failures'
```

No environment variable is needed. The TTY check is automatic.

### 2. Set AMPLIHACK_NONINTERACTIVE=1 (explicit)

Set this variable when you want non-interactive mode but cannot guarantee stdin is a pipe. This is the recommended approach for CI where stdin may be unexpectedly attached.

```sh
export AMPLIHACK_NONINTERACTIVE=1
amplihack claude --print 'Summarize the changes in the last 10 commits'
```

Or inline for a single command:

```sh
AMPLIHACK_NONINTERACTIVE=1 amplihack claude --print 'Fix the type errors'
```

## GitHub Actions example

```yaml
- name: Run amplihack task
  env:
    AMPLIHACK_NONINTERACTIVE: "1"
  run: |
    amplihack claude --print 'Review the diff and suggest fixes'
```

## Azure DevOps example

```yaml
- script: amplihack claude --print 'Check for security issues'
  displayName: 'amplihack security scan'
  env:
    AMPLIHACK_NONINTERACTIVE: "1"
```

## Docker example

```dockerfile
# In a Dockerfile or docker run command
ENV AMPLIHACK_NONINTERACTIVE=1
RUN amplihack claude --print 'Generate the API docs'
```

## What changes in non-interactive mode

| Behaviour | Interactive | Non-interactive |
|-----------|-------------|-----------------|
| Tool presence warnings (tmux missing, etc.) | printed to stderr | suppressed |
| `ensure_framework_installed()` check | runs | skipped |
| Nested invocations | propagate mode automatically | `AMPLIHACK_NONINTERACTIVE=1` set in child env |

The non-interactive path assumes the environment is **pre-provisioned** — amplihack is already installed and all required tools are present. Use `amplihack install` before the non-interactive run if this is not guaranteed.

## Pre-provision before non-interactive use

If your CI runner starts from a clean image, install amplihack interactively in a setup step before using `AMPLIHACK_NONINTERACTIVE=1` in later steps:

```yaml
steps:
  - name: Install amplihack
    run: amplihack install   # interactive step — OK in setup

  - name: Run task
    env:
      AMPLIHACK_NONINTERACTIVE: "1"
    run: amplihack claude --print 'Describe the architecture'
```

## Troubleshooting

### The process still hangs

Verify that non-interactive mode was actually detected:

```sh
AMPLIHACK_NONINTERACTIVE=1 amplihack claude --print 'hello' 2>&1 | head -5
```

If the process still waits for input, check whether `AMPLIHACK_NONINTERACTIVE` is being inherited. Some CI systems strip environment variables by default; consult your CI provider's documentation for passing variables to child steps.

### Framework not installed warning

If you see:

```
amplihack framework not found — run 'amplihack install' first
```

Non-interactive mode skips the automatic install check. Run `amplihack install` once in an interactive or setup step.

### Wrong value for AMPLIHACK_NONINTERACTIVE

Only the value `"1"` activates non-interactive mode. The values `"true"`, `"yes"`, `"on"`, and `"TRUE"` are **not** recognised.

```sh
# ✓ Correct
AMPLIHACK_NONINTERACTIVE=1 amplihack claude ...

# ✗ Not recognised — will NOT trigger non-interactive mode
AMPLIHACK_NONINTERACTIVE=true amplihack claude ...
```

## Related

- [Environment Variables](../reference/environment-variables.md#amplihack_noninteractive) — Full reference for `AMPLIHACK_NONINTERACTIVE`
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — How the Rust CLI matches Python's non-interactive detection
