# Self-Heal: Auto-Restage Framework Assets on Version Change

> [Home](../index.md) > [Features](README.md) > Self-Heal Asset Re-Stage

`amplihack` re-stages framework assets in `~/.amplihack` automatically the
first time a new binary version runs, so a binary upgrade is never silently
out-of-sync with the on-disk framework.

## Problem

PR [#488](https://github.com/rysweet/amplihack-rs/pull/488) added a
post-install hook to `amplihack update` that re-stages framework assets after
the binary is replaced. That hook only fires when the **old** running binary
already contains the post-install code path. Users on pre-#488 versions who
ran `amplihack update` got the new binary but not the asset re-stage — the
new binary had no idea the prior install was stale.

The result was silent drift: a user upgrades from `0.8.55` → `0.8.111`, then
runs a command that depends on assets shipped with the newer binary, and the
command fails or behaves like the older asset version because `~/.amplihack`
was never re-staged.

## How it works

Every launch, before command dispatch, `amplihack` performs a startup-time
**version-stamp check**:

1. Read `crate::VERSION` (the currently running binary version, honoring the
   `AMPLIHACK_RELEASE_VERSION` build-time override).
2. Read the version stamp at `~/.amplihack/.installed-version`.
3. If the stamp is missing **or** differs from the binary version, run
   `amplihack install` automatically (equivalent to
   `commands::install::run_install(None, false)`).
4. On success, write the new version into the stamp file and emit a single
   line on stderr:

   ```
   amplihack: framework assets re-staged for vX.Y.Z
   ```
5. On failure, the error propagates and `amplihack` exits with a non-zero
   status — there is **no silent fallback** to "continue with stale assets"
   (Zero-BS principle).

Manual `amplihack install` invocations also write the stamp, so both the
self-heal path and the explicit install path converge on the same source of
truth.

Once `0.8.112+` ships and a user runs it once, every subsequent launch
self-heals automatically — closing the upgrade gap permanently.

## Skip rules

The check is intentionally bypassed in cases where running an install would
recurse, undo intent, or hurt the fast-path UX:

| Trigger | Reason |
|---------|--------|
| `AMPLIHACK_SKIP_AUTO_INSTALL=<non-empty>` | Explicit opt-out for CI/testing. |
| Subcommand `install` / `uninstall` / `update` | Would recurse or undo user intent. |
| Subcommand `completions` / `doctor` / `help` | Read-only/diagnostic; should stay fast. |
| Top-level flag `--help`, `-h`, `--version`, `-V` | Short-circuits clap before dispatch. |
| No arguments | Clap will print help; nothing to dispatch. |

The argument scan runs **before** clap parses, so it adds no measurable
latency to short-circuit invocations.

## Stamp file

| Path | `~/.amplihack/.installed-version` |
|------|-----------------------------------|
| Format | Plain text, single line, no trailing newline. |
| Contents | A semantic version string (e.g. `0.8.111`). |
| Write semantics | Atomic — staged at `.installed-version.tmp` and renamed into place, mirroring the existing `write_layout_marker` pattern in `commands::install::mod`. A crashed write can never leave a half-written stamp. |
| Read semantics | Missing file returns `None` (treated as "no prior install"). All other I/O errors propagate. |

## Bypass: `AMPLIHACK_SKIP_AUTO_INSTALL`

Set `AMPLIHACK_SKIP_AUTO_INSTALL` to any non-empty value to disable the
check. Intended for CI pipelines and unit tests that pre-stage assets and do
not want the binary to mutate `~/.amplihack` mid-run.

```sh
# CI: stage once during job setup, then run many commands without re-stages
amplihack install
export AMPLIHACK_SKIP_AUTO_INSTALL=1
amplihack claude --print 'run tests'
amplihack copilot --print 'run tests'
```

An empty value (`AMPLIHACK_SKIP_AUTO_INSTALL=""`) is **not** treated as a
bypass — the check still runs.

See also: [Environment Variables — `AMPLIHACK_SKIP_AUTO_INSTALL`](../reference/environment-variables.md#amplihack_skip_auto_install).

## Failure mode

Per the project's Zero-BS philosophy, install failures during self-heal
**propagate**:

- The error is printed to stderr.
- The process exits with status `1`.
- The stamp file is **not** updated, so the next launch will retry.

There is no `|| true`, no silent skip, and no "continue with whatever assets
happen to be on disk" fallback. A broken install is surfaced to the user.

## Implementation

| File | Role |
|------|------|
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic and public entrypoint `ensure_assets_match_binary_version(args)`. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `write_installed_version`, `installed_version_path`). |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `local_install` writes the stamp on every successful install (covers both bundled and network-fallback paths). |
| `bins/amplihack/src/main.rs` | Calls `self_heal::ensure_assets_match_binary_version(&args)` after the existing update notice and before `Cli::parse_from`. |

Both new modules are within the project's 500-line module cap (self_heal.rs
= 451 LOC, version_stamp.rs = 189 LOC).

## See also

- [Install Command Reference](../reference/install-command.md) — the install
  procedure invoked by self-heal.
- [Environment Variables Reference](../reference/environment-variables.md) —
  full env var contract, including `AMPLIHACK_SKIP_AUTO_INSTALL` and
  `AMPLIHACK_RELEASE_VERSION`.
- PR [#488](https://github.com/rysweet/amplihack-rs/pull/488) — the
  post-update install hook that this feature complements.
- PR [#500](https://github.com/rysweet/amplihack-rs/pull/500) — the change
  documented here.
- Issue [#499](https://github.com/rysweet/amplihack-rs/issues/499) — the
  upgrade gap closed by this feature.
