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
| Contents | A semantic version string matching `^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.\-]+)?$` (e.g. `0.8.111`, `0.9.0-rc1`). |
| Write semantics | Atomic — staged at `.installed-version.tmp` and renamed into place, mirroring the existing `write_layout_marker` pattern in `commands::install::mod`. A crashed write can never leave a half-written stamp. |
| Read semantics | Missing file returns `None` (treated as "no prior install"). Malformed contents (failing the semver regex) are treated as "no prior install" so a corrupt stamp triggers a clean re-install rather than wedging the binary. All other I/O errors propagate. |
| File mode | `0o600` (owner read/write only). The stamp lives under `~/.amplihack` which is also owner-private; the explicit mode prevents drift if the user has loosened the parent's umask. |
| Symlink policy | The stamp path is checked with `symlink_metadata` before any read or write. If it is a symlink (or any non-regular file), self-heal **refuses to operate** on it — neither reads nor overwrites — and surfaces an error. This blocks a class of attacks where a hostile process points the stamp at a sensitive file to coerce truncation. |

### Concurrency

A second `amplihack` process launched on the same machine while a self-heal
install is in flight could otherwise race into `run_install` and stomp on the
first install's partially-written tree. To prevent this, self-heal acquires
an **advisory exclusive file lock** on `~/.amplihack/.install.lock` (created
on demand, mode `0o600`) for the duration of the decision-and-install window.

- The lock is held only while the check runs and, if needed, the install
  executes; it is released before command dispatch.
- A second process that arrives during the install **blocks** on the lock,
  then re-reads the stamp on the other side. Because the first process
  wrote the new stamp before releasing, the second process sees a match and
  proceeds without re-installing.
- The lock is advisory (`fs2::FileExt::lock_exclusive`); processes that do
  not honour it (e.g. a manual `rm -rf ~/.amplihack`) can still race, but
  no normal `amplihack` invocation will.

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

### Bypass diagnostic

When the bypass is active **and** the stamp does not match the binary
version (i.e. self-heal would have run), `amplihack` emits a single
diagnostic line on stderr before dispatch:

```
amplihack: self-heal skipped (AMPLIHACK_SKIP_AUTO_INSTALL set); stamp=0.8.55 current=0.8.111
```

This makes the "stale assets, intentionally" state visible in CI logs and
test output so a downstream failure can be traced back to the version skew
without requiring the user to remember the bypass was set. The line is
written exactly once per process and only when there is an actual mismatch;
matching versions produce no output.

See also: [Environment Variables — `AMPLIHACK_SKIP_AUTO_INSTALL`](../reference/environment-variables.md#amplihack_skip_auto_install).

## Failure mode

Per the project's Zero-BS philosophy, install failures during self-heal
**propagate**:

- The error is printed to stderr.
- The process exits with status `1`.
- The stamp file is **not** updated, so the next launch will retry.
- The advisory lock is released (RAII drop) so the retry is not blocked.

There is no `|| true`, no silent skip, and no "continue with whatever assets
happen to be on disk" fallback. A broken install is surfaced to the user.

### One documented carve-out: unresolvable home directory

If `dirs::home_dir()` returns `None` (no `$HOME`, no platform fallback),
self-heal **silently skips** rather than failing the launch. Rationale:

- A binary that cannot find a home directory cannot install anywhere
  meaningful, so failing here would produce a confusing error far from the
  real misconfiguration.
- Subcommands that genuinely need `~/.amplihack` (e.g. `claude`, `copilot`)
  will fail later with their own home-directory error, which is the
  appropriate place to surface the problem.
- Subcommands that do not need a home directory (e.g. `--version`,
  `doctor`) should continue to work in restricted environments.

This is the **only** intentionally silent path in self-heal. It is called
out explicitly so reviewers do not mistake it for a Zero-BS violation.

## Implementation

| File | Role |
|------|------|
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic, advisory lock acquisition, bypass diagnostic, and public entrypoint `ensure_assets_match_binary_version(args)`. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `write_installed_version`, `installed_version_path`). Performs symlink refusal via `symlink_metadata`, semver-regex validation of contents, and `0o600` permission enforcement on write. |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `local_install` writes the stamp on every successful install (covers both bundled and network-fallback paths). |
| `bins/amplihack/src/main.rs` | Calls `self_heal::ensure_assets_match_binary_version(&args)` after the existing update notice and before `Cli::parse_from`. |

Dependencies introduced: [`fs2`](https://crates.io/crates/fs2) for the
advisory file lock, [`regex`](https://crates.io/crates/regex) (already in
the workspace) for stamp validation. Both new modules are kept within the
project's 500-line module cap.

## See also

- [Install Command Reference](../reference/install-command.md) — the install
  procedure invoked by self-heal.
- [Environment Variables Reference](../reference/environment-variables.md) —
  full env var contract, including `AMPLIHACK_SKIP_AUTO_INSTALL` and
  `AMPLIHACK_RELEASE_VERSION`.
- PR [#488](https://github.com/rysweet/amplihack-rs/pull/488) — the
  post-update install hook that this feature complements.
- PR [#500](https://github.com/rysweet/amplihack-rs/pull/500) — the initial
  shipping change (decision flow, stamp, bypass env var).
- Issue [#499](https://github.com/rysweet/amplihack-rs/issues/499) — the
  upgrade gap closed by this feature.
- Issue [#502](https://github.com/rysweet/amplihack-rs/issues/502) — the
  hardening pass tracked here (symlink refusal, `0o600`, semver validation,
  advisory lock, bypass diagnostic, `home_dir()` carve-out).
