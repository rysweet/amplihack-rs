# Self-Heal: Auto-Restage Framework Assets on Version Change

> [Home](../index.md) > [Features](README.md) > Self-Heal Asset Re-Stage

`amplihack` re-stages framework assets in `~/.amplihack` automatically the
first time a new binary version runs, so a binary upgrade is never silently
out-of-sync with the on-disk framework. It never does the reverse: an
**older** binary refuses to re-stage implicitly over a newer install and
prints one warning line instead.

## Contents

- [Problem](#problem)
- [How it works](#how-it-works)
- [Downgrade refusal](#downgrade-refusal)
- [How source builds report their version](#how-source-builds-report-their-version)
- [Skip rules](#skip-rules)
- [Stamp file](#stamp-file)
- [Bypass: `AMPLIHACK_SKIP_AUTO_INSTALL`](#bypass-amplihack_skip_auto_install)
- [Failure mode](#failure-mode)
- [Implementation](#implementation)
- [See also](#see-also)

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
**version-stamp check**. The steps run in this order, and the first one that
returns ends the check:

1. If `HOME` is unset or empty, return silently (see
   [the `HOME` carve-out](#one-documented-carve-out-unresolvable-home-directory)).
   If the arguments match the [skip list](#skip-rules), return silently.
2. Read `crate::VERSION` (the currently running binary version; see
   [How source builds report their version](#how-source-builds-report-their-version)).
3. Read the version stamp at `~/.amplihack/.installed-version`. A missing
   stamp, or one that fails the [stamp regex](#stamp-file), is treated as
   "no stamp".
4. Check whether the staged bundle in `~/.amplihack/amplifier-bundle` is
   compatible with this binary (issue
   [#1271](https://github.com/rysweet/amplihack-rs/issues/1271)). The verdict
   is cached on a stat fingerprint of the bundle, so a steady-state launch
   does not re-parse the recipes.
5. If `AMPLIHACK_SKIP_AUTO_INSTALL` is set to a non-empty value, return. If
   the stamp differs from the binary version or the bundle is incompatible,
   print the [bypass diagnostic](#bypass-diagnostic) first.
6. If the stamp is byte-equal to the binary version **and** the bundle is
   compatible, return.
7. If the stamp is valid semver and **newer** than the binary, refuse: print
   one warning line and continue to the requested command without
   installing. See [Downgrade refusal](#downgrade-refusal).
8. Otherwise, take the [install lock](#concurrency), then re-read the stamp
   and re-check the bundle (uncached). If the stamp now matches **and** the
   bundle is compatible, return. If the stamp is now newer, refuse as in
   step 7. Either way, nothing is installed.
9. Otherwise, run `amplihack install` automatically. This covers every
   remaining case:
   - the stamp is missing;
   - the stamp is malformed;
   - the stamp passes the regex but is not strict semver (`01.2.3`,
     `1.2.3-a_b`);
   - the stamp is older than the binary;
   - the stamp and binary have equal semver precedence but are not the same
     string, which is possible only when the running version carries
     `+build` metadata;
   - the stamp is equal but the bundle is incompatible.

   The install is equivalent to
   `commands::install::run_install(None, false, false)`.
   The third argument (`force_refresh: false`) means self-heal prefers the
   compatible local source selected by normal install source resolution,
   falling back to a network download only when no compatible local source is
   found. For the post-update install path (where the **new** binary is
   spawned as a subprocess with `--force-refresh`), see
   [Post-Update Install — Re-exec New Binary](update-reexec-new-binary.md).
10. On success, write the new version into the stamp file and emit a single
    line on stderr:

    ```
    amplihack: framework assets re-staged for vX.Y.Z
    ```
11. On failure, the error propagates and `amplihack` exits with a non-zero
    status — there is **no silent fallback** to "continue with stale assets"
    (Zero-BS principle).

Manual `amplihack install` invocations also write the stamp, so both the
self-heal path and the explicit install path converge on the same source of
truth.

## Downgrade refusal

A re-stage runs the full installer: it replaces `~/.local/bin/amplihack` and
`~/.local/bin/amplihack-hooks`, rewrites `~/.claude/settings.json`, and
re-stages `~/.amplihack`. That is the right thing when a newer binary meets an
older install. When an **older** binary meets a **newer** install, it would
silently replace the newer binaries and rewrite the user's settings with
older content.

Issue [#1526](https://github.com/rysweet/amplihack-rs/issues/1526) is that
case. A `cargo install --git` build of the v0.18.39 release commit reported
`0.18.0`, because release numbers are assigned after merge and are not in
`Cargo.toml`. A `recipe run` with that build saw the stamp `0.18.39`, treated
it as a mismatch, and re-installed itself over the release.

Implicit re-stages now compare versions first and refuse when the running
binary is older than the install.

### The rule

The comparison uses semver precedence (`semver::Version::cmp_precedence`).
Build metadata (`+…`) is ignored, and versions are never compared as strings,
so `0.18.9` is correctly older than `0.18.39`.

| Stamp in `~/.amplihack/.installed-version` | Running binary | Result |
|---|---|---|
| Missing | any | Proceed: first install. |
| Malformed (fails the stamp regex) | any | Proceed: repair, as before (#502). |
| Passes the stamp regex but is not strict semver (`01.2.3`, `1.2.3-a_b`) | any | Proceed: re-stage, as for a malformed stamp. No malformed-stamp warning is printed, because the regex accepted it. |
| Valid semver | Lower precedence (`0.18.0-dev` against `0.18.39`) | **Refuse.** |
| Valid semver | Equal precedence, same string (`0.18.39` against `0.18.39`) | Proceed: skip, or repair an incompatible bundle (#1271). |
| Valid semver | Equal precedence, different string (`0.18.39+ci.7` against `0.18.39`) | Proceed: re-stage. Only possible when the running version carries build metadata. |
| Valid semver | Higher precedence (`0.18.40` against `0.18.39`) | Proceed: full re-stage, including `settings.json`. |
| Valid semver | Not valid semver | **Refuse** (fail safe). |

The last row cannot happen in a normal build, because the running version is
a compile-time constant. It refuses so that a broken comparison can never
cause a downgrade.

### The warning

A refusal writes exactly one line to stderr and nothing to stdout:

```
amplihack: refusing implicit re-stage: running "/home/dev/.cargo/bin/amplihack" is v0.18.0-dev, older than installed "/home/dev/.local/bin/amplihack" v0.18.39; nothing was changed. Run 'amplihack install' to deploy this build explicitly.
```

This document is the specification for that line. The implementation must
produce it byte for byte from this template, followed by one `\n`:

```
amplihack: refusing implicit re-stage: running {RUNNING_PATH} is v{RUNNING_VERSION}, older than installed {INSTALLED_PATH} v{STAMP}; nothing was changed. Run 'amplihack install' to deploy this build explicitly.
```

| Placeholder | Source | Format |
|---|---|---|
| `{RUNNING_PATH}` | `std::env::current_exe()` | `{:?}` of the path, so it is quoted. If `current_exe()` fails, the bare literal `<unknown>` with no quotes. |
| `{RUNNING_VERSION}` | `crate::VERSION` | Plain. |
| `{INSTALLED_PATH}` | `~/.local/bin/amplihack`, the location `amplihack install` deploys to | `{:?}` of the path, so it is quoted. If it cannot be resolved, the bare literal `<unknown>` with no quotes. |
| `{STAMP}` | The stamp value. `amplihack` never executes the installed binary to read its version. | Plain. |

`<unknown>` is never quoted. That keeps it distinct from a real file named
`<unknown>`, which would print as `"<unknown>"`. For example:

```
amplihack: refusing implicit re-stage: running <unknown> is v0.18.0-dev, older than installed "/home/dev/.local/bin/amplihack" v0.18.39; nothing was changed. Run 'amplihack install' to deploy this build explicitly.
```

Rust `Debug` form escapes newlines and control characters, so a hostile
`HOME` cannot split the line or inject terminal escapes. Both versions are
printed plain, which is safe: `crate::VERSION` is a compile-time constant, and
the stamp is printed only after it has passed the
[stamp regex](#stamp-file) and parsed as semver.

`'amplihack install'` in the line means "the `install` subcommand of the
running binary". A bare `amplihack` on `PATH` may resolve to the newer
installed binary instead; see [Resolving a refusal](#resolving-a-refusal).

Each refusal is one line. An interactive launch reaches both guard sites (see
below), so it can print the line twice. That is expected; there is no
per-process de-duplication.

### Where the guard runs

There are two implicit triggers. Both call the same function,
`install::downgrade_guard::warn_if_implicit_downgrade`.

| Trigger | Reached by | Guard position |
|---|---|---|
| Startup self-heal, `self_heal::ensure_assets_match_binary_version` | Every command not in the [skip list](#skip-rules): `recipe run`, `launch`, `claude`, `copilot`, `version`, … | After the bypass check and the equal-stamp early return, before the install lock is taken. Checked again under the lock, after the stamp is re-read, so a newer stamp written by a concurrent process is also honoured. |
| Launch bootstrap, `install::ensure_framework_installed` | Interactive tool launches (`amplihack launch`, `amplihack claude`, `amplihack copilot`, …) through `bootstrap::prepare_launcher`. Launches that are non-interactive or subprocess-safe skip the bootstrap entirely. | First statement, before the staging probe, `run_install`, slash-command staging and the `settings.json` hook auto-repair. |

Explicit `amplihack install` and `amplihack update` are not guarded. They are
the way to deploy a specific build on purpose, including an older one.

`AMPLIHACK_SKIP_AUTO_INSTALL` is read only by the startup self-heal. The launch
bootstrap does not check it, so the bypass does not suppress the bootstrap
guard. With the bypass set, an interactive launch over a newer stamp prints
two lines: the [bypass diagnostic](#bypass-diagnostic) from self-heal, then
the refusal line from the bootstrap. Neither path installs anything.

### What a refusal changes

Nothing that belongs to the install:

- `~/.local/bin/amplihack` and `~/.local/bin/amplihack-hooks` are untouched.
- `~/.claude/settings.json` is untouched, and no `settings.json.backup.*` is
  written.
- `~/.amplihack/.installed-version` keeps the newer stamp.
- No `~/.amplihack/.claude`, `~/.claude/commands/amplihack` or
  `install_*_backup.json` is created.
- The requested command still runs, and its exit status is unchanged.

The bundle-compatibility cache next to the stamp may still be refreshed,
because it is computed before the version comparison. It is neither a binary
nor a setting.

The refusal covers the installer only. During an interactive launch, the
steps of `bootstrap::prepare_launcher` that come after
`ensure_framework_installed` still run, because they are outside the scope of
this guard:

| Step | What it writes after a refusal |
|---|---|
| Claude plugin sync (`amplihack claude`) | Agents, skills and commands are copied from the tree the newer install staged in `~/.amplihack`. The generated `plugin.json` carries the running binary's version (`crate::VERSION`, for example `0.18.0-dev`). |
| Copilot home staging (`amplihack copilot`) | Agents, skills, commands and context are copied from the newer staged tree. Hook wrapper scripts are generated by the running binary. |
| `freshness::ensure_recipe_runner_up_to_date` | May update the recipe runner, as on any launch. |
| `configure_codex` (`amplihack codex`) | Writes the Codex config, as on any launch. |

So copied content comes from the newer install, and generated files (the
plugin manifest version, the Copilot wrappers) come from the running binary.

After a refusal, the bootstrap also skips its `settings.json` hook
auto-repair. If hooks are missing from `settings.json`, they stay missing
until you run `amplihack install`.

### Resolving a refusal

The warning names two binaries. Pick the one you actually want.

**Keep the newer install.** Put `~/.local/bin` first on `PATH` so a bare
`amplihack` resolves to the installed binary, not the older one:

```sh
export PATH="$HOME/.local/bin:$PATH"
hash -r
command -v amplihack
# /home/dev/.local/bin/amplihack
amplihack --version
# amplihack 0.18.39
```

Add the `export` line to your shell profile to make it permanent.

**Deploy the older build on purpose.** Run `install` with the exact
**running** path from the warning, not a bare `amplihack`, which may resolve
to the newer binary and reinstall that instead:

```sh
/home/dev/.cargo/bin/amplihack install
```

This replaces `~/.local/bin/amplihack` and `~/.local/bin/amplihack-hooks` with
the running build and writes its version into the stamp, so later implicit
checks compare against it and stop refusing.

### Moving from an older unstamped source build

Source builds made before this change reported a bare `0.18.0` and wrote
`0.18.0` into the stamp. Newer source builds report `0.18.0-dev`, which semver
orders **below** `0.18.0`. The first run of a new source build over that
stamp is therefore refused. Run `install` once with the new build's path, for
example `~/.cargo/bin/amplihack install`, to clear it.

## How source builds report their version

`crate::VERSION`, the `amplihack-hooks` version and the hooks' session-start
fallback version share one formula:

| Build | `AMPLIHACK_RELEASE_VERSION` at compile time | Reported version |
|---|---|---|
| Release or snapshot workflow | Set, for example `0.18.39` | `0.18.39` |
| `cargo build`, `cargo install --git`, `cargo install --path` | Unset | `<CARGO_PKG_VERSION>-dev`, for example `0.18.0-dev` |

```sh
cargo build --locked --bin amplihack --bin amplihack-hooks
./target/debug/amplihack --version
# amplihack 0.18.0-dev
./target/debug/amplihack-hooks --version
# amplihack-hooks 0.18.0-dev
```

The release workflow assigns the patch number after the commit merges, and
`Cargo.toml` is never updated with it. An untagged source build therefore
cannot know which release it will become. The `-dev` suffix makes that
explicit: the build is a pre-release of the `Cargo.toml` version, so semver
orders it below every release from that line, and the downgrade refusal keeps
it from replacing one implicitly.

The hooks' session-start check compares the session's `AMPLIHACK_VERSION`
(or, when that is unset, the hooks' own build version) with the project's
`.claude/.version`, and prints `⚠️ Version mismatch detected` when they
differ. A project stamped `0.18.0` and a source build reporting `0.18.0-dev`
trigger that notice. It is informational and changes no behaviour. The notice
ends with "Run `amplihack update` to update."; for a source build that is
usually not what you want, because `amplihack update` replaces it with the
latest release binary.

See [Environment Variables — `AMPLIHACK_RELEASE_VERSION`](../reference/environment-variables.md#amplihack_release_version).

## Skip rules

The check is intentionally bypassed in cases where running an install would
recurse, undo intent, or hurt the fast-path UX. None of the skipped
subcommands or flags runs the launch bootstrap, so they never reach either
downgrade guard and never print the refusal line. The
`AMPLIHACK_SKIP_AUTO_INSTALL` row is narrower: it skips self-heal only, and an
interactive launch still reaches the bootstrap guard (see
[Bypass](#bypass-amplihack_skip_auto_install)).

| Trigger | Reason |
|---------|--------|
| `AMPLIHACK_SKIP_AUTO_INSTALL=<non-empty>` | Explicit opt-out for CI/testing. |
| Subcommand `install` / `uninstall` / `update` | Would recurse or undo user intent. |
| Subcommand `completions` / `doctor` / `help` | Read-only/diagnostic; should stay fast. |
| `orch helper <sub>` | Text-transform primitives inside recipe pipelines; install output would corrupt their stdout (#1062). |
| `hygiene artifact-guard` | Pre-commit and workflow-publication check (#759); must stay read-only and fast. |
| Top-level flag `--help`, `-h`, `--version`, `-V` | Short-circuits clap before dispatch. |
| No arguments | Clap will print help; nothing to dispatch. |

The argument scan runs **before** clap parses, so it adds no measurable
latency to short-circuit invocations.

## Stamp file

| Path | `~/.amplihack/.installed-version` |
|------|-----------------------------------|
| Format | Plain text, single line, no trailing newline. |
| Contents | A version string matching `^\d+\.\d+\.\d+(-[\w.]+)?$` (e.g. `0.18.39`, `0.18.0-dev`). Build metadata (`+…`) is rejected. |
| Write semantics | Atomic — staged at `.installed-version.tmp` and renamed into place, mirroring the existing `write_layout_marker` pattern in `commands::install::mod`. A crashed write can never leave a half-written stamp. |
| Read semantics | Missing file returns `None` (treated as "no prior install"). Malformed contents (failing the regex) print one line, `amplihack: ignoring malformed install stamp at <path> (contents=<value>); will re-stage`, and are treated as "no prior install" so a corrupt stamp triggers a clean re-install rather than wedging the binary. All other I/O errors propagate. |
| File mode | `0o600` (owner read/write only). The stamp lives under `~/.amplihack` which is also owner-private; the explicit mode prevents drift if the user has loosened the parent's umask. |
| Symlink policy | The stamp path is checked with `symlink_metadata` before any read or write. If it is a symlink (or any non-regular file), self-heal **refuses to operate** on it — neither reads nor overwrites — and surfaces an error. This blocks a class of attacks where a hostile process points the stamp at a sensitive file to coerce truncation. |

### Concurrency

A second `amplihack` process launched on the same machine while a self-heal
install is in flight could otherwise race into `run_install` and stomp on the
first install's partially-written tree. To prevent this, self-heal acquires
an **advisory exclusive file lock** on `~/.amplihack/.install.lock` (created
on demand) for the duration of the decision-and-install window.

- The lock is held only while the check runs and, if needed, the install
  executes; it is released before command dispatch.
- A second process that arrives during the install **blocks** on the lock,
  then re-reads the stamp and re-checks the bundle on the other side. If the
  stamp now matches **and** the bundle is compatible, it proceeds without
  re-installing. If the stamp is now newer than its own version, it refuses
  with the [downgrade warning](#the-warning).
- The lock is advisory; processes that do not honour it (e.g. a manual
  `rm -rf ~/.amplihack`) can still race, but no normal `amplihack`
  invocation will.

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

When the bypass is active **and** self-heal would have acted (the stamp does
not match the binary version, or the installed bundle is incompatible),
`amplihack` emits a single diagnostic line on stderr before dispatch:

```
amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage (stamp=0.8.55 current=0.8.111; installed_bundle="compatible")
```

This makes the "stale assets, intentionally" state visible in CI logs and
test output so a downstream failure can be traced back to the version skew
without requiring the user to remember the bypass was set. Matching versions
with a compatible bundle produce no output.

Inside self-heal, the bypass is checked before the downgrade guard, so with
the bypass set and a newer stamp, self-heal prints the bypass diagnostic and
not the refusal line. The bypass applies to self-heal only. The launch
bootstrap does not read it, so an interactive `amplihack launch`, `claude` or
`copilot` still reaches the bootstrap guard and prints the refusal line as
well:

```
amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage (stamp=0.18.39 current=0.18.0-dev; installed_bundle="compatible")
amplihack: refusing implicit re-stage: running "/home/dev/.cargo/bin/amplihack" is v0.18.0-dev, older than installed "/home/dev/.local/bin/amplihack" v0.18.39; nothing was changed. Run 'amplihack install' to deploy this build explicitly.
```

Neither path installs anything. Non-interactive launches and commands that
never bootstrap (such as `recipe run`) print only the bypass diagnostic.

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

A [downgrade refusal](#downgrade-refusal) is not a failure. Nothing was
attempted, so the requested command runs and its exit status is its own.

### One documented carve-out: unresolvable home directory

If `HOME` is unset or empty, self-heal **silently skips** rather than failing
the launch. Rationale:

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
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic, advisory lock, bypass diagnostic, and public entrypoint `ensure_assets_match_binary_version(args)`. The under-lock body is `restage_under_lock`, which re-reads the stamp and re-applies the downgrade guard before installing. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. |
| `crates/amplihack-cli/src/commands/install/downgrade_guard.rs` | The only downgrade decision and the only refusal line: `is_implicit_downgrade(stamp, running) -> bool` and `warn_if_implicit_downgrade(stamp, running, &mut notice) -> Result<bool>`, which returns `true` when it refused. Has no filesystem or process side effects. |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `ensure_framework_installed` (launch bootstrap) delegates to `ensure_framework_installed_with(&mut notice)`, whose first step is the downgrade guard. `local_install` writes the stamp on every successful install. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `write_installed_version`, `installed_version_path`): symlink refusal, regex validation and `0o600` on write. |
| `crates/amplihack-cli/src/lib.rs`, `bins/amplihack-hooks/src/main.rs`, `crates/amplihack-hooks/src/session_start/context_loaders.rs` | The three version constants: `AMPLIHACK_RELEASE_VERSION` when set at compile time, otherwise `concat!(env!("CARGO_PKG_VERSION"), "-dev")`. |
| `bins/amplihack/src/main.rs` | Calls `self_heal::ensure_assets_match_binary_version(&args)` after the existing update notice and before `Cli::parse_from`. |

Every guard call site has the same fail-closed shape:

```rust
if downgrade_guard::warn_if_implicit_downgrade(stamp.as_deref(), expected, notice)? {
    return Ok(());
}
```

### Regression tests

| Test | Covers |
|---|---|
| `bins/amplihack/tests/issue_1526_no_implicit_downgrade.rs` | Runs the real binary against a temp `HOME` with stamp `9999.0.0`; asserts `settings.json`, both `~/.local/bin` binaries and the stamp are byte-identical, no backups exist, and stderr has exactly one refusal line. |
| `crates/amplihack-cli/src/commands/install/tests/issue_1526_downgrade_guard.rs` | Every row of [the rule](#the-rule); the bootstrap refusal leaving `settings.json` untouched; a single-line warning under a `HOME` containing a newline and an escape sequence. |
| `self_heal.rs` unit tests | Refusal for `recipe run` and `launch`; refusal under the lock; silence for `orch helper`; `stamp_mismatch_triggers_install` for the upgrade path. |
| `tests/integration/cli_golden_tests.rs`, `tests/integration/hook_dispatch_test.rs` | `amplihack --version` and `amplihack-hooks --version` both report the release version or `<CARGO_PKG_VERSION>-dev`. |

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
  hardening pass (symlink refusal, `0o600`, semver validation, advisory lock,
  bypass diagnostic, `HOME` carve-out).
- Issue [#1526](https://github.com/rysweet/amplihack-rs/issues/1526) — an
  older source build re-staged itself over a newer release during a
  `recipe run`; fixed by the downgrade refusal and the `-dev` version suffix.
