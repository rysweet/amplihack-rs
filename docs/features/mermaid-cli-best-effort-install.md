# Best-Effort Mermaid CLI Provisioning

> [Home](../index.md) > [Features](README.md) > Mermaid CLI Provisioning

`amplihack install` provisions the Mermaid CLI (`mmdc`, the npm package
[`@mermaid-js/mermaid-cli`](https://www.npmjs.com/package/@mermaid-js/mermaid-cli))
on a **best-effort** basis. When `mmdc` is available, the
[`pr-guide`](../../amplifier-bundle/skills/pr-guide/reference.md) skill renders
mermaid diagrams to images **locally** for Azure DevOps pull requests — where
mermaid historically does not render inside PR descriptions/comments — instead
of sending diagram source to the third-party `mermaid.ink` service.

This component is **optional**. A failed or skipped `mmdc` install never fails
or blocks `amplihack install`. It emits a warning and continues, in keeping
with the [install-completeness invariant](../reference/install-completeness.md):
`amplihack install` fails loudly only when a **required** component cannot be
staged. `mmdc` is not required.

## Why This Matters

The `pr-guide` skill converts mermaid diagrams to images for Azure DevOps
tenants that do not yet render ` ```mermaid ` fences natively in PR descriptions
and comments. It has two rendering paths (see
[pr-guide §9, "Mermaid on Azure DevOps"](../../amplifier-bundle/skills/pr-guide/reference.md)):

| Path | Renderer | Privacy |
|------|----------|---------|
| **Preferred** | Local `mmdc -i diagram.mmd -o diagram.svg`, published via the PR attachments API | Diagram source never leaves the machine |
| **Fallback** | Hosted `https://mermaid.ink/img/<base64>` | Diagram source is sent to a third-party service |

Provisioning `mmdc` at install time means the preferred, privacy-preserving
local path is available without a separate manual step. Because `mmdc` pulls in
puppeteer and a headless Chromium download (hundreds of MB) and requires
Node.js/npm — which may be absent or restricted in CI, containers, or hardened
servers — the install is deliberately best-effort: it attempts the install when
it cheaply can, and otherwise steps aside quietly so the `pr-guide` skill falls
back to `mermaid.ink`.

## Behavior

The provisioning step runs near the end of `amplihack install`, **after** the
version stamp and install manifest are written and after Copilot-home staging.
Running last guarantees that an `mmdc` failure can never leave required install
state unwritten.

### Decision flow

```
amplihack install
└── (after version stamp + manifest + copilot-home staging)
    └── ensure mermaid CLI (best-effort)
        ├── AMPLIHACK_SKIP_MMDC set (non-empty)?  ── yes ─▶ SkippedByEnv   (info, continue)
        ├── `mmdc --version` succeeds?            ── yes ─▶ AlreadyPresent (info, continue)
        ├── `npm --version` fails / npm absent?   ── yes ─▶ SkippedNoNpm   (info, continue)
        ├── `npm install -g @mermaid-js/mermaid-cli`
        │     ├── success AND re-probe finds mmdc ─────────▶ Installed     (success, continue)
        │     └── failure OR mmdc still off PATH  ─────────▶ Failed        (warn, continue)
        └── (always returns Ok — never aborts install)
```

### Outcomes

The step resolves to exactly one outcome, each mapped to a user-facing line:

| Outcome | Condition | Message | Stream |
|---------|-----------|---------|--------|
| `SkippedByEnv` | `AMPLIHACK_SKIP_MMDC` is set to any non-empty value | `  ℹ AMPLIHACK_SKIP_MMDC set; skipping mermaid CLI install` | stdout |
| `AlreadyPresent` | `mmdc --version` already succeeds on `PATH` | `  ✓ mermaid CLI (mmdc) already installed; skipping` | stdout |
| `SkippedNoNpm` | `npm --version` fails (npm not on `PATH`) | `  ℹ npm not available; skipping mermaid CLI install (pr-guide will fall back to mermaid.ink)` | stdout |
| `Installed` | `npm install -g …` succeeded and the re-probe found `mmdc` | `Installing mermaid CLI for local diagram rendering...` then a success line | stdout |
| `Failed` | install command failed, **or** succeeded but `mmdc` is still not discoverable on `PATH` | `mermaid CLI not installed; pr-guide will fall back to mermaid.ink for Azure DevOps diagrams` | stderr (`⚠️`) + `tracing::warn!` |

In **every** case the step returns `Ok(())`. Install continues to its normal
completion banner regardless of the outcome.

### Idempotency

The step is safe to re-run. On any subsequent `amplihack install`, an
already-installed `mmdc` is detected by the `mmdc --version` probe and the step
short-circuits to `AlreadyPresent` without contacting npm or re-downloading
Chromium. Self-heal re-installs (triggered by a stale version stamp) and
post-update installs therefore do not repeatedly attempt the npm install.

### Non-interactive and offline environments

- The step **never prompts**. It honors `AMPLIHACK_NONINTERACTIVE`
  automatically because there is no interactive confirmation to suppress — the
  global `npm install -g` runs the same way whether or not the flag is set.
- For minimal, offline, or bandwidth-constrained environments, set
  `AMPLIHACK_SKIP_MMDC=1` to disable the attempt entirely (see below). This
  avoids even the fast `mmdc`/`npm` probes.
- When npm is absent, the step skips cleanly with an informational line; it
  does **not** attempt to install Node.js or npm. (Node.js auto-install is a
  separate, launch-time mechanism for `amplihack copilot`; see
  [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md).)

## Configuration

### `AMPLIHACK_SKIP_MMDC`

**Type:** flag
**Values:** any non-empty value disables the attempt; absence or empty string
means the step runs.

When set to any non-empty value (`1`, `true`, `yes`, etc.), `amplihack install`
skips the Mermaid CLI provisioning step entirely — no `mmdc` probe, no `npm`
probe, and no `npm install`. The step resolves to `SkippedByEnv` and prints one
informational line. Use it for:

- **Offline or air-gapped installs** that must not reach npm or download
  Chromium.
- **Minimal containers/CI** where the hundreds-of-MB puppeteer/Chromium
  download is undesirable and the `pr-guide` `mermaid.ink` fallback (or native
  ADO rendering) is acceptable.
- **Deterministic test harnesses** that manage tool availability explicitly.

```sh
# Skip the best-effort mermaid CLI install entirely
AMPLIHACK_SKIP_MMDC=1 amplihack install
```

**Truthiness:** any non-empty value triggers the skip. An empty string
(`AMPLIHACK_SKIP_MMDC=""`) is treated as **unset** and the step runs normally.
This matches the presence-flag convention used elsewhere in the installer (for
example `AMPLIHACK_SKIP_AUTO_INSTALL`).

## Examples

### Default install on a host with npm

```text
$ amplihack install
...
✅ Amplihack installation completed successfully!
...
  ✅ Copilot home staged (~/.copilot/)
Installing mermaid CLI for local diagram rendering...
  ✓ mermaid CLI (mmdc) installed
```

### Install on a host without npm

```text
$ amplihack install
...
✅ Amplihack installation completed successfully!
...
  ✅ Copilot home staged (~/.copilot/)
  ℹ npm not available; skipping mermaid CLI install (pr-guide will fall back to mermaid.ink)
```

Install still succeeds. The `pr-guide` skill will use the `mermaid.ink`
fallback (or native ADO rendering) for Azure DevOps diagrams.

### Re-running install when mmdc is already present

```text
$ amplihack install
...
✅ Amplihack installation completed successfully!
...
  ✅ Copilot home staged (~/.copilot/)
  ✓ mermaid CLI (mmdc) already installed; skipping
```

### Install with the npm step failing (e.g. permission or network error)

```text
$ amplihack install
...
✅ Amplihack installation completed successfully!
...
  ✅ Copilot home staged (~/.copilot/)
Installing mermaid CLI for local diagram rendering...
  ⚠️  mermaid CLI not installed; pr-guide will fall back to mermaid.ink for Azure DevOps diagrams
```

The warning is advisory. Exit status is **success** — the optional component
did not gate the install.

### Skipping the attempt explicitly

```text
$ AMPLIHACK_SKIP_MMDC=1 amplihack install
...
✅ Amplihack installation completed successfully!
...
  ✅ Copilot home staged (~/.copilot/)
  ℹ AMPLIHACK_SKIP_MMDC set; skipping mermaid CLI install
```

### Installing mmdc manually later

If you skip the attempt or it fails, you can provision `mmdc` yourself at any
time and the `pr-guide` skill will pick it up automatically on the next run:

```sh
npm install -g @mermaid-js/mermaid-cli
mmdc --version   # verify it is on PATH
```

## How It Works (Implementation Notes)

The step is implemented in
`crates/amplihack-cli/src/commands/install/mermaid_cli.rs` and invoked from
`local_install()` in `crates/amplihack-cli/src/commands/install/mod.rs`, after
the Copilot-home staging block and before the final `Ok(())`.

### `ensure_mermaid_cli() -> Result<Outcome>`

Owns the entire probe → install → re-probe sequence and prints the user-facing
status line for the resolved outcome. It **always returns `Ok`** — a failed or
skipped install is encoded in the returned [`Outcome`](#outcome), never as an
`Err`. The caller pretty-prints based on the outcome and logs `tracing::warn!`
on `Failed`.

| Step | Action |
|------|--------|
| 1 | If `AMPLIHACK_SKIP_MMDC` is non-empty → `Outcome::SkippedByEnv`. |
| 2 | Probe `mmdc --version`. Success → `Outcome::AlreadyPresent`. |
| 3 | Probe `npm --version`. Failure → `Outcome::SkippedNoNpm`. |
| 4 | Run `npm install -g @mermaid-js/mermaid-cli`. On command failure → `Outcome::Failed` (warn + continue). |
| 5 | Re-probe `mmdc --version`. Success → `Outcome::Installed`. Still absent (e.g. npm global prefix not on `PATH`) → `Outcome::Failed` (warn + continue). |

### Outcome

```rust
enum Outcome {
    AlreadyPresent, // mmdc was already discoverable on PATH
    Installed,      // npm install -g succeeded and re-probe found mmdc
    SkippedByEnv,   // AMPLIHACK_SKIP_MMDC was set
    SkippedNoNpm,   // npm not available on PATH
    Failed,         // install failed, or mmdc not on PATH after install
}
```

### Process execution and safety

- Commands are run with `std::process::Command` using the **argument-vector**
  form (`Command::new("npm").args(["install", "-g", "@mermaid-js/mermaid-cli"])`),
  never through a shell. This prevents command injection.
- The package spec `@mermaid-js/mermaid-cli` is hardcoded. It is never built
  from environment, config, or user input.
- `AMPLIHACK_SKIP_MMDC` is read as a **presence flag only**
  (`var_os(..).map(|v| !v.is_empty())`); its value never reaches any command.
- No privilege escalation: the step never invokes `sudo`. Permission failures
  resolve to `Failed` (warn-and-continue), not a retry with elevated
  privileges.
- No npm integrity bypass flags (no `--unsafe-perm`, no audit/integrity
  disabling) are used.
- The `--version` probes are fast and use the abstraction-free `Command`
  directly. The `npm install -g` has **no hard timeout kill**, because the
  Chromium download is legitimately slow; it is bounded by npm's own network
  timeouts. Because the step is non-fatal, a slow or stalled download can never
  block required install state.

### PATH resolution caveat

`mmdc` and `npm` are resolved via `PATH`, consistent with the installer's other
`npm`/`cargo`/Copilot invocations. If `npm install -g` succeeds but the npm
global-prefix `bin` directory is not on `PATH`, the re-probe will not find
`mmdc` and the step resolves to `Failed` with the fallback warning — even though
the binary exists off-`PATH`. This mirrors the `~/.cargo/bin` caveat in the
recipe-runner install path. The installer does not prepend to `PATH`; add the
npm global bin directory to your `PATH` and re-run, or invoke `mmdc` by its
absolute path.

## Testing

Hermetic, no-network unit tests live in
`crates/amplihack-cli/src/commands/install/tests/mermaid_cli_tests.rs` and are
serialized via `home_env_lock()`. They cover:

- `AMPLIHACK_SKIP_MMDC` set → `SkippedByEnv` short-circuit (no commands run).
- npm absent (via `PATH` manipulation) → `SkippedNoNpm`, handled gracefully.
- A stub `mmdc` on `PATH` → `AlreadyPresent` skip.
- The function **never returns `Err`** — failure does not propagate as an
  install error.

The tests never perform a real npm install or contact the network.

## See Also

- [pr-guide skill — Mermaid on Azure DevOps](../../amplifier-bundle/skills/pr-guide/reference.md) — how `mmdc` and `mermaid.ink` are used at PR-creation time
- [amplihack install / uninstall — Command Reference](../reference/install-command.md) — full install phase list and environment variables
- [Install Completeness Verification](../reference/install-completeness.md) — required vs. optional component contract
- [Environment Variables — Reference](../reference/environment-variables.md#amplihack_skip_mmdc) — `AMPLIHACK_SKIP_MMDC`
- [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md) — separate launch-time Node.js provisioning for `amplihack copilot`
