# Self-Heal: Auto-Restage Framework Assets on Version Change

> [Home](../index.md) > [Features](README.md) > Self-Heal Asset Re-Stage

`amplihack` re-stages framework assets in `~/.amplihack` automatically the
first time a new binary version runs, so a binary upgrade is never silently
out-of-sync with the on-disk framework. It never re-stages over an install
that is newer than the running binary: that case is refused with a warning, and
nothing on disk changes.

## Contents

- [Problem](#problem)
- [How it works](#how-it-works)
- [Downgrade refusal](#downgrade-refusal)
- [Skip rules](#skip-rules)
- [Stamp file](#stamp-file)
- [Bypass: `AMPLIHACK_SKIP_AUTO_INSTALL`](#bypass-amplihack_skip_auto_install)
- [Failure mode](#failure-mode)
- [Implementation](#implementation)

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
3. If the stamp equals the binary version **and** the installed bundle passes
   its cached compatibility check, do nothing. This warm path starts no
   subprocess and parses no recipe YAML.
4. Otherwise, run the [downgrade guard](#downgrade-refusal). If an installed
   copy of amplihack is newer than this binary, or its version cannot be
   determined, print one warning on stderr and leave everything on disk as it
   is. The requested command then runs normally, with its exit status
   unaffected.
5. Otherwise, take the [install lock](#concurrency), re-read the stamp, run the
   downgrade guard again, and run `amplihack install` automatically
   (equivalent to `commands::install::run_install(None, false, false)`).
   The third argument (`force_refresh: false`) means self-heal prefers the
   compatible local source selected by normal install source resolution,
   falling back to a network download only when no compatible local source is
   found. The automatic install run validates candidate and staged framework
   bundles. For the post-update install path (where the **new** binary is
   spawned as a subprocess with `--force-refresh`), see
   [Post-Update Install — Re-exec New Binary](update-reexec-new-binary.md).
6. On success, write the new version into the stamp file and emit a single
   line on stderr:

   ```
   amplihack: framework assets re-staged for vX.Y.Z
   ```
7. On failure, the error propagates and `amplihack` exits with a non-zero
   status — there is **no silent fallback** to "continue with stale assets"
   (Zero-BS principle).

Manual `amplihack install` invocations also write the stamp, so both the
self-heal path and the explicit install path converge on the same source of
truth.

Once `0.8.112+` ships and a user runs it once, every subsequent launch
self-heals automatically — closing the upgrade gap permanently.

## Downgrade refusal

An implicit re-stage never moves an install backwards. Only an explicit
`amplihack install` may replace a newer install with an older binary
([issue #1526](https://github.com/rysweet/amplihack-rs/issues/1526)).

### Why it exists

A host can carry more than one copy of amplihack. In issue #1526,
`~/.local/bin/amplihack` was the v0.18.39 release, and `~/.cargo/bin/amplihack`
was a `cargo install --git` build of the same commit that reported `0.18.0`.
A detached recipe run whose `PATH` listed `~/.cargo/bin` first ran the cargo
copy. That copy saw the `0.18.39` stamp, treated the difference as a stale
install, and ran a full install. It replaced both binaries in `~/.local/bin`
with itself and rewrote `~/.claude/settings.json`. The next run of the real
v0.18.39 binary then re-staged everything again.

The guard stops this: a version difference triggers a re-stage only when the
running binary is at least as new as everything already installed.

### The rule

An implicit re-stage is **refused** when either of these holds:

- some installed source reports a version strictly higher than the running
  binary's version, or
- some installed source is present but its version cannot be determined.

Equal versions are allowed, so the issue
[#1271](https://github.com/rysweet/amplihack-rs/issues/1271) repair (stamp
current, bundle stale) and first-run bootstrap from a release tarball keep
working. A machine with no stamp and no installed binaries is always allowed:
there is nothing to downgrade.

Versions are compared by semantic-version precedence. Build metadata is
ignored (`0.18.0+snapshot.3f2a1c` equals `0.18.0`) and pre-releases sort
before their release (`0.19.0-rc1` is lower than `0.19.0`). A stamp whose raw
text equals the running version counts as equal before any parsing.

### Where the guard runs

Two code paths stage assets without being asked to. Both consult the same
guard, and both skip **every** write on refusal and return success.

| Path | Entry point | Writes skipped on refusal |
|------|-------------|---------------------------|
| Startup self-heal | `self_heal::ensure_assets_match_binary_version`, before every command not in the [skip rules](#skip-rules) | `amplihack install` (binaries, `~/.claude/settings.json` and its backup, assets), the stamp write, the bundle-cache invalidation |
| Launch bootstrap | `install::ensure_framework_installed`, reached from interactive `amplihack claude`, `amplihack copilot` and other launch commands | the bootstrap `amplihack install`, the `~/.claude/commands/amplihack/` top-up, the `settings.json` hook auto-repair and its `settings.json.backup.*` |

The startup path evaluates the guard only after the warm-path check (step 3
above), so a launch with a matching stamp and a healthy bundle never probes
anything. The launch path evaluates it lazily, at most once per call, and only
immediately before a write that would actually happen. A launch with nothing
to repair never evaluates it.

These commands are never guarded:

- `amplihack install`, including a deliberate downgrade.
- `amplihack update`, which runs an explicit `install` in the new binary.
- `amplihack uninstall`.

### What the guard checks

Sources are checked in this order, and the first refusal wins:

1. The stamp, `~/.amplihack/.installed-version`.
2. `~/.local/bin/amplihack` (`amplihack.exe` on Windows).
3. `~/.local/bin/amplihack-hooks` (`amplihack-hooks.exe` on Windows).

| Source state | Result |
|--------------|--------|
| Stamp missing | Check the next source. |
| Stamp equal to or lower than the running version | Check the next source. |
| Stamp higher than the running version | **Refuse.** No binary is probed. |
| Stamp empty, not a semantic version, larger than 4 KiB, not UTF-8, or unreadable | **Refuse** (version unknown). |
| `HOME` is a relative path, so `~/.local/bin` is not absolute | **Refuse** (version unknown) before touching the filesystem. |
| Binary missing | Skip it. |
| Binary is the running executable (same canonical path) | Skip it. |
| Binary starts with `#!`: the uvx/pipx Python shim, the npm node launcher, any shell wrapper | Skip it. It is never executed. |
| Binary cannot be opened to read its first two bytes | **Refuse** (version unknown). |
| Unix only: binary owned by a user other than you or root, or world-writable | **Refuse** (version unknown). It is never executed. |
| `<binary> --version` fails to start, exits non-zero, runs longer than 5 s, or prints no parseable version | **Refuse** (version unknown). |
| `<binary> --version` reports a version equal to or lower than the running version | Check the next source. |
| `<binary> --version` reports a higher version | **Refuse.** |
| No source refused | **Allow** the re-stage. |

Script wrappers are skipped rather than probed because running them has side
effects: the uvx shim fetches the latest PyPI `amplihack`, and the npm launcher
calls the GitHub API and may download or build a binary. Every binary that
`amplihack install` deploys is native, so a wrapper carries no version signal
about the install being protected. The consequence is that an install made
only of wrappers, with no stamp, is not protected, which keeps the uvx and npm
migration bootstraps working.

### The `--version` probe

Each candidate binary is run as `<absolute path> --version`:

- No shell is involved, and stdin is `/dev/null`.
- The child gets `AMPLIHACK_SKIP_AUTO_INSTALL=1`, `AMPLIHACK_NO_UPDATE_CHECK=1`
  and `AMPLIHACK_NONINTERACTIVE=1`, so it can never start its own self-heal,
  update check or prompt.
- It is killed, with its whole process tree, after 5 seconds.
- Each output stream is capped at 4 KiB. Stderr is discarded and never shown.

The version is the second whitespace-separated token on the first non-empty
stdout line, with one leading `v` removed, parsed strictly as a semantic
version:

```console
$ ~/.local/bin/amplihack --version
amplihack 0.18.39
$ ~/.local/bin/amplihack-hooks --version
amplihack-hooks 0.18.39
```

Both report `0.18.39`. Probes run only on the cold path: at most two binaries,
checked once before the install lock and once under it.

### The warning

A refusal prints one block on stderr, at most once per process, even when
both the startup and launch paths refuse. Stdout is never touched. For the
issue #1526 scenario it reads:

```text
amplihack: refusing implicit re-stage: it could downgrade the installed amplihack
  running:   /home/ryan/.cargo/bin/amplihack (v0.18.0)
  installed: /home/ryan/.amplihack/.installed-version (v0.18.39)
  Nothing was changed: binaries, ~/.claude/settings.json and framework assets were left as they are.
  To install v0.18.0 over it anyway, run `amplihack install`.
```

When the newer source is a binary, the last line also names it, so you can
switch to it:

```text
  installed: /home/ryan/.local/bin/amplihack (v0.18.39)
  Nothing was changed: binaries, ~/.claude/settings.json and framework assets were left as they are.
  To install v0.18.0 over it anyway, run `amplihack install`, or run /home/ryan/.local/bin/amplihack to keep using v0.18.39.
```

When the version cannot be determined, the `installed:` line gives the reason
instead of a version:

```text
  installed: /home/ryan/.local/bin/amplihack-hooks (version unknown: `--version` exited with status 2)
```

The first line always starts with `amplihack: refusing implicit re-stage`, and
that text appears exactly once per block, so scripts and tests can count it.
Any text taken from a file or a process (a malformed stamp, `--version`
output) is cut to 80 characters and printed quoted and escaped, so control
characters and ANSI escapes cannot forge log lines.

### Resolving a refusal

| Situation | What to do |
|-----------|------------|
| An older copy comes first on `PATH` (the issue #1526 case) | Run the installed binary instead, remove the older copy (`cargo uninstall amplihack`), or put `~/.local/bin` before `~/.cargo/bin` on `PATH`. |
| You want the older version | Run `install` from that binary, for example `~/.cargo/bin/amplihack install`. An explicit install downgrades on purpose. |
| A snapshot build (`0.18.0+snapshot.*`) over an installed 0.18.x release | Run `amplihack install` from the snapshot build if you want it installed. |
| `amplihack-hooks` from v0.7.46 or earlier, which has no `--version` | Run `amplihack install` to replace it. |
| The stamp is malformed or unreadable | Run `amplihack install`; it rewrites the stamp. |
| Unix: a binary in `~/.local/bin` is owned by another user or is world-writable | Fix its ownership or run `chmod o-w` on it, or delete it and run `amplihack install`. |
| `HOME` is a relative path | Set `HOME` to an absolute path. |

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
| Read semantics | `read_installed_version` returns `None` for a missing file (treated as "no prior install"). It also returns `None` for malformed contents (failing the semver regex), with a one-line `ignoring malformed install stamp … will re-stage` notice. All other I/O errors propagate. The [downgrade guard](#downgrade-refusal) reads the stamp separately, through `read_raw_installed_version`, and refuses an implicit re-stage over a malformed, oversized or unreadable stamp. A corrupt stamp is therefore repaired only by an explicit `amplihack install`; when both messages appear, the refusal is the one that applies. |
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
- If the stamp still differs, the waiter runs the
  [downgrade guard](#downgrade-refusal) again under the lock, re-reading the
  stamp and re-probing the binaries with no cached result. A process that
  waited while a **newer** install finished therefore refuses instead of
  overwriting it.
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

### A downgrade refusal is not a failure

A [downgrade refusal](#downgrade-refusal) is a decision, not an error:

- No install runs, so there is nothing to fail.
- The requested command runs, and its exit status is unaffected.
- Stdout is untouched; the warning goes to stderr only.
- If stderr cannot be written (for example, a closed pipe), the refusal still
  stands. The write error is logged through `tracing` and never turns the
  refusal into exit status 1.

An error while *evaluating* the guard, such as an unresolvable home directory
on the cold path, propagates like any other self-heal error. It never falls
through to an install.

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
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic, advisory lock acquisition, bypass diagnostic, and public entrypoint `ensure_assets_match_binary_version(args)`. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. `ensure_assets_match_binary_version_with_probe` also takes an injected `--version` probe; it runs the downgrade guard after the warm-path return and again under the install lock. |
| `crates/amplihack-cli/src/commands/install/restage_guard.rs` | The downgrade guard. `evaluate` returns `Verdict::Allow` or `Verdict::Refuse(Refusal)` from the running version, the raw stamp, `~/.local/bin`, the running executable and a probe, and writes nothing. `probe_version_output` and `parse_version_output` implement the `--version` probe; `render` builds the warning; `warn_once` prints it behind a process-wide latch. `REFUSAL_MARKER` is the `amplihack: refusing implicit re-stage` text. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `read_raw_installed_version`, `write_installed_version`, `installed_version_path`). Performs symlink refusal via `symlink_metadata`, semver-regex validation of contents, and `0o600` permission enforcement on write. `read_raw_installed_version` returns the trimmed stamp text without the regex, reading at most 4 KiB. |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `local_install` writes the stamp on every successful install (covers both bundled and network-fallback paths). `ensure_framework_installed` consults the downgrade guard before the bootstrap install, the slash-command top-up and the `settings.json` hook auto-repair. |
| `bins/amplihack/src/main.rs` | Calls `self_heal::ensure_assets_match_binary_version(&args)` after the existing update notice and before `Cli::parse_from`. |

### Tests

| File | Covers |
|------|--------|
| `crates/amplihack-cli/src/commands/install/restage_guard.rs` | Evaluation order and short-circuit, skipped candidates (running executable, `#!` scripts), every fail-closed case, version ordering, warning contents, the Unix trust check, and the real probe against a fake binary. |
| `crates/amplihack-cli/src/self_heal.rs` | Higher stamp refuses and keeps the stamp; a higher installed binary refuses with no stamp; the warm path never probes; a stamp raised while waiting on the lock refuses; a failing stderr writer still returns `Ok`. |
| `crates/amplihack-cli/src/commands/install/tests/issue_1526_no_implicit_downgrade.rs` | Launch bootstrap and `settings.json` auto-repair are both refused: `settings.json`, binaries and stamp stay byte-identical, and no backup or staging tree appears. |
| `bins/amplihack/tests/issue_1526_no_downgrade_restage.rs` | The built `amplihack version` against a temporary `HOME` with stamp `999.0.0`: exit 0, nothing changed, no `.install.lock` created, and the warning marker exactly once on stderr and never on stdout. |

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
- Issue [#1526](https://github.com/rysweet/amplihack-rs/issues/1526) — the
  implicit downgrade closed by the [downgrade refusal](#downgrade-refusal).
- [CI Pipeline Reference — Release version stamp](../reference/ci-pipeline.md#release-version-stamp-releaseyml)
  — why a build of a release tag now reports that release's version.
