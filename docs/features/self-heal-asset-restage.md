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
3. If the stamp is missing or unparseable, or differs from the binary version and is not
   newer by semver precedence, or matches but the installed framework bundle
   is incompatible (#1271), run `amplihack install` automatically (equivalent
   to `commands::install::run_install(None, false, false)`). A stamp **newer**
   than the binary never triggers an install; see
   [Never an implicit downgrade](#never-an-implicit-downgrade-1526).
   The third argument (`force_refresh: false`) means self-heal prefers the
   compatible local source selected by normal install source resolution,
   falling back to a network download only when no compatible local source is
   found. For
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

A re-stage replaces the binaries in `~/.local/bin` and rewrites
`~/.claude/settings.json`, so an automatic one only ever moves an install
forward. In [#1526](https://github.com/rysweet/amplihack-rs/issues/1526) a
recipe run used a `main` build reporting `0.18.0` and re-staged over v0.18.39.

### Version precedence

The running version and the stamp are compared by semver precedence: a
pre-release ranks below its release, and build metadata on the running version
is ignored. The [stamp reader](#stamp-file) rejects build metadata, so a stamp
with a `+…` suffix counts as unparseable and stages like a missing one.

| Running | Stamp | Result |
| --- | --- | --- |
| `0.18.0` | `0.18.39` | Refused: warn and run the command |
| `0.18.9` | `0.18.39` | Refused (numeric order, not string order) |
| `1.0.0-rc.1` | `1.0.0` | Refused |
| `1.0.0+snapshot.1` | `1.0.2` | Refused |
| `1.0.1` | `1.0.0` | Re-stage |
| `1.0.0+snapshot.1` | `1.0.0` | Not a downgrade: re-stage, as before |
| `1.0.0` | `1.0.0` | Nothing to do, or the #1271 repair |
| `1.0.0` | `1.0.1+m` | Stage: the reader rejects the stamp, so it counts as unparseable |
| any | missing or unparseable | Stage, as before (#502) |

### Order of checks

Startup self-heal checks in this order:

1. [Skip rules](#skip-rules), `HOME`, then read the stamp.
2. **Guard #1**, only when `AMPLIHACK_SKIP_AUTO_INSTALL` is not set: if the
   stamp is newer, print the [warning](#the-warning) and stop.
3. The #1271 bundle check, memoised in `~/.amplihack/.bundle-compat-cache.json`.
4. **Bypass.** With `AMPLIHACK_SKIP_AUTO_INSTALL` set, print the
   [bypass diagnostic](#bypass-diagnostic) if there is a skew (it includes the
   bundle check's result) and stop. Nothing is installed. Because Guard #1
   skips itself under the bypass, an older binary prints the bypass line
   instead of the warning, never both.
5. A matching stamp with a compatible bundle: nothing to do.
6. Take `~/.amplihack/.install.lock`, re-read the stamp, and apply
   **Guard #2**, so a newer install that finished while this process waited
   still wins. Then install.

Guard #1 runs before the bundle check, so its refusal parses no recipe YAML
and writes nothing. These stay byte-identical: `~/.local/bin/amplihack` and
`amplihack-hooks`, `~/.claude/settings.json` and its backups, the staged
assets, the stamp, `.install.lock`, and `.bundle-compat-cache.json`. Guard #2
only fires in a race, after the bundle check and the lock, so only those last
two may have been written.

### The warning

A refusal prints one block on stderr, at most once per process:

```
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   "/home/dev/.cargo/bin/amplihack" (v0.18.0)
  installed: "/home/dev/.local/bin/amplihack" (v0.18.39)
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build, or put "/home/dev/.local/bin" ahead of "/home/dev/.cargo/bin" on PATH.
```

- Paths are escaped, so a path cannot forge a line. The `PATH` clause appears
  only when the two directories differ.
- A refusal is not an error: the command runs and its exit status is
  unchanged. Printing the warning is best effort. If the running executable
  cannot be resolved, the warning shows it as `<unknown>`. If the installed
  path cannot be resolved, no warning is printed. That failure, and a failed
  write to stderr, is logged at `debug`, and the refusal still stands.
- To downgrade on purpose, run `amplihack install` with the older binary.
  Explicit `install` and `update` are not guarded.

### Launch bootstrap

The launch bootstrap (`ensure_framework_installed`, run by interactive
launches such as `amplihack claude`) applies the same rule as its first step,
before it stages assets, tops up `~/.claude/commands/amplihack/` or repairs
`settings.json`. The check runs whether or not `AMPLIHACK_SKIP_AUTO_INSTALL`
is set.

- Usually startup self-heal has already printed the [warning](#the-warning)
  in the same process, so the bootstrap prints nothing more.
- Some launches skip self-heal: its [argument scan](#skip-rules) matches
  `-h`, `--help`, `-V` and `--version` anywhere on the command line, so a
  flag forwarded to the tool, as in `amplihack claude -- --version`, skips
  it. The bootstrap then prints the same warning itself, so it still appears
  once.
- With `AMPLIHACK_SKIP_AUTO_INSTALL` set, the bootstrap refuses without a
  warning. Self-heal prints the
  [bypass diagnostic](#bypass-diagnostic) instead, when it runs.

The bootstrap also logs one `debug` tracing event, not a warning, because
`tracing` output goes to stdout when `RUST_LOG` enables it. A stamp that
exists but cannot be read (for example, permission denied) stops the launch
with an error before anything is staged.

The bootstrap reads the stamp without the [stamp reader](#stamp-file)'s format
check. A stamp with build metadata, such as `1.0.1+m`, still compares as newer
here, so with `AMPLIHACK_SKIP_AUTO_INSTALL` set the bootstrap skips even
though the bypass diagnostic reports `stamp=<missing>`. Without the bypass,
self-heal has already re-staged and rewritten the stamp before the bootstrap
reads it.

### Limits

- Launch steps after the bootstrap are outside the guard. They republish
  from the existing staging directory: an older `amplihack claude` still
  re-publishes the Claude Code plugin and skills
  (`claude_plugin::ensure_claude_plugin_installed`), and an older
  `amplihack copilot` still re-stages the Copilot home
  (`copilot_setup::ensure_copilot_home_staged`). The binaries and
  `settings.json` are not touched, and the newer binary's next launch
  publishes its own copy again.
- A build of an untagged `main` commit, including the one a release was cut
  from and any `cargo install --git … --rev <SHA>`, reports the base version
  from `Cargo.toml`. So does a build of a tag made before #1526, because
  existing tags are not re-tagged: `--tag v0.18.39` still reports `0.18.0`.
  Builds of later release tags report the release (see
  [Release tags point at a stamped commit](../reference/ci-pipeline.md#release-tags-point-at-a-stamped-commit-1526)).
  For the other builds, this guard turns a downgrade into a warning.
- The stamp is not a trust anchor. Anything that can write `$HOME` already
  controls the install; a forged high stamp only stops automatic re-stages.
- The bootstrap reads the stamp and installs without the install lock, so a
  newer install landing between the two can still be overwritten.

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
| Contents | `MAJOR.MINOR.PATCH` with an optional `-PRERELEASE`, matching `^\d+\.\d+\.\d+(-[\w.]+)?$` (e.g. `0.8.111`, `0.9.0-rc1`). Build metadata (`+…`) is rejected. |
| Write semantics | Atomic — staged at `.installed-version.tmp` and renamed into place, mirroring the existing `write_layout_marker` pattern in `commands::install::mod`. A crashed write can never leave a half-written stamp. |
| Read semantics | Surrounding whitespace is trimmed. Missing file returns `None` (treated as "no prior install"). Malformed contents (failing the regex, including any `+…` suffix) are also treated as "no prior install", with one stderr line such as `amplihack: ignoring malformed install stamp at /home/dev/.amplihack/.installed-version (contents="1.0.1+m"); will re-stage`, so a corrupt stamp triggers a clean re-install rather than wedging the binary. All other I/O errors propagate. |
| File mode | `0o600` (owner read/write only). The stamp lives under `~/.amplihack` which is also owner-private; the explicit mode prevents drift if the user has loosened the parent's umask. |
| Symlink policy | Before a write, the stamp path is checked with `symlink_metadata`. If it is a symlink, the write is refused with an error, so a hostile process cannot point the stamp at a sensitive file to coerce truncation. Reads do not check and follow a symlink. |

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

When the bypass is active **and** there is a skew, `amplihack` emits a
single diagnostic line on stderr before dispatch. A skew is either of:

- the stamp differs from the binary version: missing, older, or newer;
- the installed framework bundle is incompatible (#1271), even with a
  matching stamp.

```
amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage (stamp=0.8.55 current=0.8.111; installed_bundle="compatible")
```

`stamp` is `<missing>` when there is no stamp. `installed_bundle` is
`"compatible"` or the reason the bundle is incompatible. With a stamp newer
than the binary, this line is the only notice: the
[downgrade warning](#the-warning) is not printed.

This makes the "stale assets, intentionally" state visible in CI logs and
test output so a downstream failure can be traced back to the version skew
without requiring the user to remember the bypass was set. The line is
written at most once per process. A matching stamp with a compatible bundle
produces no output.

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
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic, advisory lock acquisition, bypass diagnostic, downgrade guards and warning (`is_implicit_downgrade`, #1526), and public entrypoint `ensure_assets_match_binary_version(args)`. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `write_installed_version`, `installed_version_path`). Performs symlink refusal via `symlink_metadata`, semver-regex validation of contents, and `0o600` permission enforcement on write. |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `local_install` writes the stamp on every successful install (covers both bundled and network-fallback paths). `ensure_framework_installed` applies the #1526 downgrade check before the launch bootstrap changes anything. |
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
