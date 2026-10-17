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
3. If the stamp is missing, unparseable or **older** than the binary version,
   re-stage automatically through `repair_framework_assets` in
   [implicit mode](#never-an-implicit-downgrade-1526). A stamp **newer** than
   the binary never triggers a re-stage.
   The third argument (`force_refresh: false`) means self-heal prefers the
   compatible local source selected by normal install source resolution,
   falling back to a network download only when no compatible local source is
   found. Self-heal does not perform a separate startup compatibility scan; the
   automatic install run validates candidate and staged framework bundles. For
   the post-update install path (where the **new** binary is spawned as a
   subprocess with `--force-refresh`), see
   [Post-Update Install — Re-exec New Binary](update-reexec-new-binary.md).
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

## Never an implicit downgrade (#1526)

An automatic re-stage (self-heal or the launch bootstrap) runs in **implicit
mode**: it never writes `~/.claude/settings.json`, and it replaces a binary in
`~/.local/bin` only when the running version is strictly newer than the
stamp's binaries version by semver precedence (`0.18.9 < 0.18.10`,
`1.2.0-dev < 1.2.0`). It replaces `amplihack-hooks` and
`amplihack-asset-resolver` only with the copies next to the running
executable (or `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`); a copy found
elsewhere on `PATH` has an unknown version and may only fill an empty slot.
An empty slot is always filled. Explicit `amplihack install` is unchanged,
and is the way to downgrade on purpose.

| Running vs stamp | Assets | Binaries | Stamp |
|---|---|---|---|
| older | untouched | untouched | untouched |
| equal | no-op, or the #1271 bundle repair | untouched | untouched |
| newer | re-staged | replaced | written |
| missing or unparseable | re-staged (#502) | empty slots filled | written; binaries recorded as unknown if one was kept |

When a re-stage keeps any installed binary, one line goes to stderr:

```
amplihack: warning: kept /home/dev/.local/bin/amplihack-hooks, /home/dev/.local/bin/amplihack (installed version: unknown) during an implicit re-stage for v1.2.0; run `amplihack install` to replace them.
```

The stamp then records the framework as current and the binaries at the
version the previous stamp gave them, or as unknown (see
[Stamp file](#stamp-file)). The next launch finds the framework current and
neither re-stages nor warns, and the guard still refuses to replace the kept
binaries until `amplihack install` is run. On a downgrade nothing is written, and one warning is printed per process; the
command keeps its usual exit status:

```
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   /home/dev/.cargo/bin/amplihack (v0.18.0)
  installed: /home/dev/.local/bin/amplihack (v0.18.39)
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build.
```

Control characters in printed paths appear as `?`. The launch bootstrap
(`ensure_framework_installed`) applies the same check first; an unreadable
stamp now fails the launch, as it already failed self-heal. When the binary is
not older, its #202 hook repair still writes `settings.json` if the file is
missing or parses with no amplihack hooks. A `settings.json` that does not
parse is left unchanged and a warning is printed. Builds of untagged `main`
commits still report `MAJOR.MINOR.0` (see
[Release tags point at a stamp commit](../reference/ci-pipeline.md#release-tags-point-at-a-stamp-commit)).

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
| Format | Plain text, no trailing newline. One line, or two after an implicit re-stage kept a binary (#1526). |
| Contents | Line one: the framework version, matching `^\d+\.\d+\.\d+(-[\w.]+)?$` (e.g. `0.8.111`, `0.9.0-rc1`). Optional line two: `binaries: <version>` or `binaries: unknown`, the version of the binaries in `~/.local/bin` when it differs from line one. Builds that predate the second line treat a two-line stamp as malformed and re-stage. |
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

When the bypass is active **and** self-heal would have run (the stamp is
missing or older than the binary, or the installed bundle is incompatible),
`amplihack` emits a single diagnostic line on stderr before dispatch:

```
amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage (stamp=0.8.55 current=0.8.111; installed_bundle="compatible")
```

This makes the "stale assets, intentionally" state visible in CI logs and
test output so a downstream failure can be traced back to the version skew
without requiring the user to remember the bypass was set. The line is
written exactly once per process. A matching stamp with a compatible bundle
produces no output.

Under the bypass a stamp newer than the binary also prints this line rather
than the [downgrade warning](#never-an-implicit-downgrade-1526): the bypass
never re-stages, so there is nothing for the guard to refuse.

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
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic, advisory lock acquisition, bypass diagnostic, the #1526 downgrade guards and once-per-process warning, and public entrypoint `ensure_assets_match_binary_version(args)`. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `read_binaries_version`, `write_installed_version`, `write_installed_versions`, `installed_version_path`) and the #1526 semver comparison (`relation`). Performs symlink refusal via `symlink_metadata`, semver-regex validation of contents, and `0o600` permission enforcement on write. |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `local_install` writes the stamp on every successful install (covers both bundled and network-fallback paths); an implicit one that kept an installed binary also records the binaries' version, or `unknown`. `InstallMode::{Explicit, Implicit}` selects whether settings are written and binaries may be replaced; `ensure_framework_installed` applies the downgrade check before the launch bootstrap changes anything. |
| `crates/amplihack-cli/src/commands/install/binary.rs` | `deploy_binaries` keeps an existing destination in implicit mode unless the running binary is strictly newer than the stamp's binaries version (`implicit_deploy_allowed`) and the source is this build's own copy, and returns the binaries it kept. |
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
