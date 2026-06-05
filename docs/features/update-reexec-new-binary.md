# Post-Update Install: Re-exec New Binary

> [Home](../index.md) > [Features](README.md) > Post-Update Install Re-exec

When `amplihack update` downloads a new binary, the post-update install step
spawns the **new** binary as a subprocess rather than running the old binary's
compiled-in install code. This ensures that fixes, asset changes, and
configuration migrations shipped in the new version take effect immediately —
without requiring the user to manually run `amplihack install` a second time.

## Problem

Issue [#683](https://github.com/rysweet/amplihack-rs/issues/683).

`amplihack update` performs two steps:

1. **Binary swap** — downloads and atomically replaces the on-disk binary.
2. **Post-update install** — re-stages framework assets in `~/.amplihack`.

Prior to this fix, step 2 called `crate::commands::install::run_install`
**in-process**. Because the old binary was still the running process image,
the install step executed the **old** binary's code — not the newly downloaded
version's code. Any fixes shipped in the new binary (such as PR
[#687](https://github.com/rysweet/amplihack-rs/pull/687) removing stale XPIA
hook file verification) would not take effect until the user manually ran
`amplihack install` with the new binary.

The result was a class of "phantom failures": users ran `amplihack update`,
saw a success message, but the old install code — with its old bugs — had
just re-staged assets. The new binary's fixes were present on disk but had
never executed.

### Why `current_exe()` is unreliable after binary replacement

On Linux, `/proc/self/exe` points to the running process's inode. After an
atomic rename replaces the binary on disk, the old inode is still referenced
by the running process. Resolving `std::env::current_exe()` may return a path
with a ` (deleted)` suffix, or resolve to a different inode than the
newly-written file. macOS exhibits a similar conceptual problem — the running
process image remains the old executable regardless of filesystem changes.

## How it works

After `run_update()` selects a safe replacement target and
`download_and_replace()` atomically installs the new binary, it spawns the new
binary as a child process instead of calling `run_install` in-process:

```
amplihack update
  │
  ├─ analyze PATH conflicts
  │    → inspect ordered candidates for amplihack + amplihack-hooks
  │    → prefer ~/.local/bin when a safe user-level target exists
  │    → stop with manual repair guidance for system-managed targets
  │
  ├─ download_and_replace(&release)
  │    → downloads, verifies SHA-256, atomic rename
  │    → returns Result<PathBuf> with the installed binary path
  │
  ├─ write version cache
  │
  └─ run_post_update_install(skip_install, || { ... })
       │
       ├─ skip_install = true  → skip, return Ok(())
       │
       └─ skip_install = false →
            build_install_command(&installed_exe)
              → Command::new(installed_exe)
              → args: ["install", "--force-refresh"]
              → env: AMPLIHACK_NO_UPDATE_CHECK=1
              → env: AMPLIHACK_NONINTERACTIVE=1
            cmd.status()?
              → spawns NEW binary as subprocess
              → inherits stdin/stdout/stderr
              → non-zero exit → bail with path + status
```

### Key design decisions

1. **Explicit path, not `current_exe()`** — `download_and_replace()` returns
   the destination `PathBuf` — the same path as `current_exe()`, resolved
   before the atomic rename overwrites it — so the caller has a valid
   filesystem path to the new binary without needing to re-resolve
   `current_exe()` (which would point to a deleted inode on Linux). This path
   is passed directly to `Command::new()`.

2. **`--force-refresh` hidden flag** — The subprocess runs with
   `amplihack install --force-refresh`, a hidden CLI flag that forces a fresh
   network download of `amplifier-bundle/` assets instead of reusing stale
   local copies. This flag is not shown in `--help` output; it exists solely
   for the update→install subprocess path.

3. **Recursion prevention** — The subprocess environment includes
   `AMPLIHACK_NO_UPDATE_CHECK=1`, which prevents the child `amplihack install`
   from triggering its own update check. Combined with `install` being in the
   self-heal skip list (see [Self-Heal Asset Re-Stage](self-heal-asset-restage.md)),
   this prevents infinite recursion.

4. **Non-interactive** — `AMPLIHACK_NONINTERACTIVE=1` is set to suppress any
   interactive prompts in the child process, since the update is already
   user-initiated and no further consent is needed.

5. **Inherited stdio** — `stdin`, `stdout`, and `stderr` are inherited so the
   user sees install progress in real time. The subprocess behaves as if the
   user ran `amplihack install` directly.

6. **Safe target selection** — Before replacing a binary, update analyzes
   ordered `PATH` candidates for `amplihack` and `amplihack-hooks`. If stale
   system-managed entries such as `/usr/local/bin`, `/usr/bin`, `/bin`, or
   `/opt` shadow current user-local binaries, update prefers the writable
   `~/.local/bin` target when available and prints repair guidance. It never
   writes automatically into system-managed prefixes, even when those
   directories are writable.

### `--skip-install` bypass

The existing `--skip-install` (alias `--no-install`) flag on
`amplihack update` continues to work. When set, the entire post-update
install step is skipped — no subprocess is spawned. The binary is updated
on disk but framework assets are not re-staged:

```sh
# Update binary only, skip post-update install
amplihack update --skip-install
```

Users can then run `amplihack install` manually at a time of their choosing.
The [self-heal](self-heal-asset-restage.md) mechanism will also catch the
version mismatch on the next launch and trigger an automatic re-install.

## Failure modes

### Subprocess exits non-zero

The error message includes both the executable path and the exit status:

```
Error: post-update install subprocess /home/user/.local/bin/amplihack exited with exit status: 1
```

The binary update itself has already succeeded. The user can re-run
`amplihack install` manually. The self-heal mechanism will also retry
automatically on the next `amplihack` invocation.

### New binary missing or not executable

If the path returned by `download_and_replace()` does not exist or lacks
execute permission, `Command::new().status()` returns an I/O error that
propagates with full context. This should not happen in practice because
`download_and_replace()` sets `0o755` permissions and uses atomic rename.

### System binary shadows user-local binary

If `/usr/local/bin/amplihack` appears before `~/.local/bin/amplihack` on
`PATH`, update reports the conflict. When `~/.local/bin` is writable, the
user-local binary is updated and the warning explains how to make the shell run
it first. When no safe user-level target exists, update stops before replacement
and prints sudo/manual repair guidance.

See [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
for the target-selection rules.

### Old binary without `--force-refresh`

If an older binary (pre-#683) is somehow spawned as the subprocess, it will
not recognize the `--force-refresh` flag and clap will exit with an error.
This is the correct behavior — it surfaces the version mismatch explicitly
rather than silently running stale install code. The error message will
include the unrecognized flag name.

## Implementation

| File | Role |
|------|------|
| `crates/amplihack-cli/src/path_conflicts.rs` | Shared side-effect-free PATH analysis, canonical duplicate handling, warning formatting, and install target decision logic. |
| `crates/amplihack-cli/src/update/mod.rs` | Exposes update submodules so resolver-backed target selection is part of the update flow without leaking file layout. |
| `crates/amplihack-cli/src/update/install.rs` | `download_and_replace()` returns `Result<PathBuf>` with the installed binary path (captured before atomic rename). |
| `crates/amplihack-cli/src/update/check.rs` | `run_update()` captures the returned path, passes it to `build_install_command()`. The closure given to `run_post_update_install` spawns the subprocess and checks its exit status. |
| `crates/amplihack-cli/src/update/check.rs` | `build_install_command(installed_exe: &Path) -> Command` — constructs the subprocess command with args and env vars. Visibility: `pub(super)` for testability. |
| `crates/amplihack-cli/src/commands/install/binary.rs` | Deploys user-local binaries and invokes PATH conflict analysis for install-time warnings. |
| `crates/amplihack-cli/src/commands/install/paths.rs` | Owns user-local path construction and safe path helpers reused by install/update target selection. |
| `crates/amplihack-cli/src/commands/install/settings.rs` | Keeps hook registrations aligned with the selected `amplihack-hooks` binary path. |
| `crates/amplihack-cli/src/cli_commands.rs` | `Commands::Install` gains a hidden `--force-refresh` flag (`#[arg(long = "force-refresh", hide = true)]`). |
| `crates/amplihack-cli/src/commands/mod.rs` | Destructures and passes `force_refresh` through to `run_install`. |
| `crates/amplihack-cli/src/update/post_install.rs` | Unchanged. The closure injection pattern continues to work — the closure now spawns a subprocess instead of calling `run_install`. |

No new crate dependencies introduced.

## Testing

| Test | Location | What it verifies |
|------|----------|------------------|
| `build_install_command_uses_provided_binary_path` | `update/tests/build_install_command.rs` | Subprocess targets the explicit path, not `current_exe()` |
| `build_install_command_includes_install_and_force_refresh_args` | `update/tests/build_install_command.rs` | Args are `["install", "--force-refresh"]` in order |
| `build_install_command_sets_no_update_check_env` | `update/tests/build_install_command.rs` | `AMPLIHACK_NO_UPDATE_CHECK=1` is set |
| `build_install_command_sets_noninteractive_env` | `update/tests/build_install_command.rs` | `AMPLIHACK_NONINTERACTIVE=1` is set |
| `update_check_source_includes_framework_restage` | `tests/bugfix_install_tests.rs` | Source-level assertion that `run_update` uses `build_install_command` (not `run_install` in-process) |
| Existing `run_post_update_install` tests | `update/post_install.rs` | Skip-install bypass, closure invocation, error propagation — all still pass unchanged |
| PATH conflict resolver tests | `path_conflicts.rs` | User-bin first, system-bin shadowing, duplicate candidates, safe user-bin preference, and manual repair decisions |
| Install/update smoke output assertions | install/update tests | Normal output excludes stale hook-file `❌` lines, `profile_management` warnings, and literal `Skipping symlink` noise |

## Interaction with related features

### Self-heal asset re-stage

The [self-heal](self-heal-asset-restage.md) mechanism acts as a safety net.
If the post-update subprocess install fails for any reason, the next
`amplihack` launch will detect the version-stamp mismatch and re-run install
automatically. The self-heal path calls `run_install` **in-process** (not
via subprocess), which is correct because there is no binary replacement in
flight — the running binary IS the new binary by that point.

The self-heal doc's reference to the post-update install path (line 37–39,
`force_refresh: true`) remains accurate — the subprocess passes
`--force-refresh` which has the same semantic effect.

### Startup self-update prompt

The [subprocess-safe skip](startup-update-prompt-subprocess-safe.md)
mechanism prevents the post-update install subprocess from triggering its
own update prompt. The `AMPLIHACK_NO_UPDATE_CHECK=1` env var set by
`build_install_command` is one of the skip signals documented there.

## See also

- [Install Command Reference](../reference/install-command.md) — the install
  procedure invoked by the subprocess.
- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md) —
  safe target selection, PATH shadowing warnings, and manual repair guidance.
- [Repair install/update PATH conflicts](../howto/repair-install-update-path-conflicts.md) —
  user-facing repair steps for stale `/usr/local/bin` binaries.
- [Self-Heal Asset Re-Stage](self-heal-asset-restage.md) — the startup-time
  safety net that catches missed post-update installs.
- [Startup Self-Update Prompt — Subprocess-Safe Skip](startup-update-prompt-subprocess-safe.md) —
  how the update check is suppressed in the subprocess.
- [Environment Variables Reference](../reference/environment-variables.md) —
  `AMPLIHACK_NO_UPDATE_CHECK`, `AMPLIHACK_NONINTERACTIVE`.
- Issue [#683](https://github.com/rysweet/amplihack-rs/issues/683) — the
  bug report that motivated this change.
- PR [#687](https://github.com/rysweet/amplihack-rs/pull/687) — the XPIA
  hook removal that exposed the stale-code problem.
