# Post-Update Install: Re-exec New Binary

> [Home](../index.md) > [Features](README.md) > Post-Update Install Re-exec

When `amplihack update` installs a new Rust binary, the post-update install
step spawns the **new** binary as a subprocess rather than running the old
binary's compiled-in install code. This ensures that binary precedence repair,
stale wrapper quarantine, asset refresh, and configuration migrations shipped
in the new version take effect immediately.

## Problem

Issue [#683](https://github.com/rysweet/amplihack-rs/issues/683).

`amplihack update` performs two major steps:

1. **Binary swap** — downloads and atomically replaces the on-disk binary.
2. **Post-update repair install** — runs the new binary's
   `amplihack install --force-refresh` path.

Prior to this fix, step 2 called `crate::commands::install::run_install`
**in-process**. Because the old binary was still the running process image,
the install step executed the **old** binary's code — not the newly downloaded
version's code. Any fixes shipped in the new binary (such as PR
[#687](https://github.com/rysweet/amplihack-rs/pull/687) removing stale XPIA
hook file verification) would not take effect until the user manually ran
`amplihack install` with the new binary.

The result was a class of "phantom failures": users ran `amplihack update`,
saw a success message, but the pre-update install logic — with its old bugs — had
just re-staged assets. The new binary's fixes were present on disk but had
never executed.

The durable repair also prevents stale Python/uvx wrappers and stale installed
`amplifier-bundle` assets from regaining control after update. Update must end
with the Rust user-level binary selected and the installed bundle replaced by
the one belonging to the release it just installed.

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
  │    -> inspect ordered candidates for amplihack + amplihack-hooks
  │    -> select ~/.local/bin as the preferred user-level target
  │    -> classify stale wrappers, unknown executables, and inaccessible paths
  │
  ├─ download_and_replace(&release)
  │    → downloads, verifies SHA-256, atomic rename
  │    → returns Result<PathBuf> with the installed binary path
  │
  ├─ write version cache
  │
  └─ run_post_update_install(skip_install, || { ... })
       │
       ├─ skip_install = true  → intentional bypass: skip durable repair
       │
       └─ skip_install = false →
            build_install_command(&installed_exe)
              -> Command::new(installed_exe)
              -> args: ["install", "--force-refresh"]
              -> env: AMPLIHACK_NO_UPDATE_CHECK=1
              -> env: AMPLIHACK_NONINTERACTIVE=1
              -> env: AMPLIHACK_POST_UPDATE_INSTALL=1
              -> env: AMPLIHACK_REPAIR_ORIGINAL_PATH=<parent PATH>
              -> env: PATH=<repaired PATH>, only when repair_subprocess_path
                 returns one
            cmd.status()?
              -> spawns NEW binary as subprocess
              -> inherits stdin/stdout/stderr
              -> non-zero exit -> bail with path + status
```

### Key design decisions

1. **Explicit path, not `current_exe()`** — `download_and_replace()` returns
   the destination `PathBuf` — the same path as `current_exe()`, resolved
   before the atomic rename overwrites it — so the caller has a valid
   filesystem path to the new binary without needing to re-resolve
   `current_exe()` (which would point to a deleted inode on Linux). This path
   is passed directly to `Command::new()`.

2. **`--force-refresh` hidden flag** - The subprocess runs with
   `amplihack install --force-refresh`, a hidden CLI flag that bypasses the
   installed `~/.amplihack/amplifier-bundle` as a source. It stages from
   `AMPLIHACK_HOME` when that tree has the binary's version, and otherwise
   fetches the tag `v<version>` of the binary's release. It doesn't consult the
   current directory or fetch `main`. This flag is not shown in `--help`
   output; it exists for the update-to-install repair path and direct
   stale-bundle repair. The source and staged destination are validated by the
   framework bundle compatibility checker. See
   [Framework source selection](../reference/framework-source-selection.md).

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

6. **Rust-first repair** - Before replacement, update analyzes ordered `PATH`
   candidates for `amplihack` and `amplihack-hooks`. The repair install then
   deploys to `~/.local/bin`, quarantines positively identified stale Python/uvx
  wrappers only when they shadow Rust and are in safe locations, writes the
  managed PATH profile block, and fails if an unknown or inaccessible
  executable still shadows the Rust binary.

7. **Staged bundle activation** - The repair install stages
   `~/.amplihack/amplifier-bundle` from the release's source, compares the copy
   with that source, sets its modes, validates it, then activates it as the
   installed bundle. It does not merge source files over the old installed
   bundle.

### Which binary re-stages the bundle

The new binary does. The old binary only replaces the two files in
`~/.local/bin` by rename, which leaves running processes on their old inode,
and then starts the new binary as a child. All bundle staging, link switching
and cleanup run in that child.

So the first update from 1.2.107 to a release with the
[versioned install layout](../concepts/versioned-install-layout.md#the-first-update-to-this-release)
is safe for live recipe runs and needs no quiet window. The same hand-off has
been in `amplihack update` since 0.9.69 (#690), so updates from 0.9.69 to
1.2.106 should behave the same way, but only the 1.2.107 hand-off is tested.
Installs older than 0.9.69 run the install in-process with their own code
and re-stage the bundle in place; stop recipe runs before updating them.

Each release records the process that committed it in `.installed-by`
(`version`, `exe`, `pid`, `post_update`). After an update, `post_update=1`
and the new version show that the new binary did the work. On the first
update `exe` is `$HOME/.local/bin/amplihack`, the regular file the old
binary renamed into place:

```bash
amplihack update
grep -E '^(version|exe|post_update)=' \
  "$(readlink -f ~/.amplihack/amplifier-bundle)/../.installed-by"
# version=<version>
# exe=$HOME/.local/bin/amplihack
# post_update=1
```

### `--skip-install` bypass

The existing `--skip-install` (alias `--no-install`) flag on
`amplihack update` continues to work as an intentional bypass. When set, the
entire post-update install step is skipped — no subprocess is spawned. The
binary is updated on disk, but durable repair is not performed: stale wrappers
are not quarantined, PATH precedence is not persisted, and framework assets are
not re-staged:

```sh
# Update binary only, skip post-update install
amplihack update --skip-install
```

Use this flag only when you intentionally want a binary-only update. Complete
the skipped repair later with:

```sh
amplihack install --force-refresh
```

The [self-heal](self-heal-asset-restage.md) mechanism will also catch the
version mismatch on the next launch and trigger an automatic re-install when
the version stamp still indicates the framework was not staged successfully.

## Failure modes

### Subprocess exits non-zero

The error message includes both the executable path and the exit status:

```
Error: post-update install subprocess /home/user/.local/bin/amplihack exited with exit status: 1
```

The binary update itself has already succeeded, and `amplihack update` exits
non-zero. The user can re-run `amplihack install` manually. The self-heal
mechanism will also retry automatically on the next `amplihack` invocation when
the version stamp still requires an install.

### Release tag can't be fetched

The child install waits up to 180 seconds across git and the tarball, which is
inside the parent's 300-second limit for the child. On failure it keeps the
bundle and stamp, names `amplihack install --local`, and exits non-zero. It
doesn't fall back to `main` or to an older tree.

```
error: could not fetch the framework bundle from release tag v1.2.99: network request timed out
The installed bundle and version stamp were not changed.
Run `amplihack install --local <PATH>` to install from a checkout.
```

To skip the tag fetch, set `AMPLIHACK_HOME` to a worktree of the release tag:

```sh
git -C ~/src/amplihack-rs worktree add ~/src/amplihack-v1.2.99 v1.2.99
AMPLIHACK_HOME=~/src/amplihack-v1.2.99 amplihack update
```

The child install prints the worktree path and version and stages from it.
Update itself still needs the network to download the binary, and the worktree
must have the version of the binary being installed.

### New binary missing or not executable

If the path returned by `download_and_replace()` does not exist or lacks
execute permission, `Command::new().status()` returns an I/O error that
propagates with full context. This should not happen in practice because
`download_and_replace()` sets `0o755` permissions and uses atomic rename.

### Unknown executable shadows the Rust binary

If an executable named `amplihack` appears before `~/.local/bin/amplihack` and
cannot be classified as the current Rust binary, preferred Rust binary, or stale
Python/uvx wrapper, update reports the conflict. The file is not modified.

When the managed PATH block and wrapper quarantine still cannot make the Rust
binary resolve first, update exits non-zero with the conflicting path and manual
repair guidance.

See [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
for the candidate classification and neutralization rules.

### Stale wrapper quarantine fails

If a shadowing stale wrapper is positively identified but cannot be moved into
quarantine, the post-update install fails when that wrapper would continue to
shadow the Rust binary. The failure includes the source path and quarantine
destination.

### Old binary without `--force-refresh`

If an older binary (pre-#683) is somehow spawned as the subprocess, it will
not recognize the `--force-refresh` flag and clap will exit with an error.
This is the correct behavior — it surfaces the version mismatch explicitly
rather than silently running stale install code. The error message will
include the unrecognized flag name.

## Implementation

| File | Role |
|------|------|
| `crates/amplihack-cli/src/path_conflicts.rs` | Shared side-effect-free PATH analysis, candidate classification, warning formatting, and install target decision logic. |
| `crates/amplihack-cli/src/update/mod.rs` | Exposes update submodules so resolver-backed target selection is part of the update flow without leaking file layout. |
| `crates/amplihack-cli/src/update/install.rs` | `download_and_replace()` returns `Result<PathBuf>` with the installed binary path (captured before atomic rename). |
| `crates/amplihack-cli/src/update/check.rs` | `run_update()` captures the returned path, passes it to `build_install_command()`. The closure given to `run_post_update_install` spawns the subprocess and checks its exit status. |
| `crates/amplihack-cli/src/update/check.rs` | `build_install_command(installed_exe: &Path) -> Command` — constructs the subprocess command with args and env vars. Visibility: `pub(super)` for testability. |
| `crates/amplihack-cli/src/commands/install/binary.rs` | Deploys user-local binaries and invokes PATH conflict analysis. |
| `crates/amplihack-cli/src/commands/install/stale_wrappers.rs` | Quarantines positively identified shadowing stale Python/uvx wrappers and writes manifests. |
| `crates/amplihack-cli/src/commands/install/paths.rs` | Owns user-local path construction, safe path helpers, and managed PATH profile blocks. |
| `crates/amplihack-cli/src/commands/install/settings.rs` | Keeps hook registrations aligned with the selected `amplihack-hooks` binary path. |
| `crates/amplihack-cli/src/commands/install/framework_source.rs` | Chooses the source for `--force-refresh`: `--local`, a same-version `AMPLIHACK_HOME`, or the release tag. |
| `crates/amplihack-cli/src/commands/install/tag_fetch.rs` | Fetches the tree for `v<binary version>` and checks its version. |
| `crates/amplihack-cli/src/commands/install/bundle_compat.rs` | Validates source and staged framework bundles, rejects active `orch_helper.py` dependencies, and supports staged installed-bundle activation. |
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
| `exact_handoff_contract_matches_v1_2_107` | `update/tests/build_install_command.rs` | Argv is exactly `["install", "--force-refresh"]` and `AMPLIHACK_NO_UPDATE_CHECK`, `AMPLIHACK_NONINTERACTIVE` and `AMPLIHACK_POST_UPDATE_INSTALL` are set to `1`, as in 1.2.107 |
| First update from 1.2.107 | `tests/issue_1954_first_install_restager.rs` | A stub old binary hands off as 1.2.107 does; `.installed-by` shows the new binary built and switched the release, and a live run finishes |
| `update_check_source_includes_framework_restage` | `tests/bugfix_install_tests.rs` | Source-level assertion that `run_update` uses `build_install_command` (not `run_install` in-process) |
| Existing `run_post_update_install` tests | `update/post_install.rs` | Skip-install intentional bypass, closure invocation, error propagation — all still pass unchanged |
| PATH conflict resolver tests | `path_conflicts.rs` | User-bin first, candidate classification, unknown conflict reporting, inaccessible path handling, and manual repair decisions |
| Stale wrapper repair tests | `tests/install_stale_wrapper_repair.rs` | Python wrapper quarantine, uvx wrapper quarantine, safe symlink handling, and no mutation of unknown executables |
| Update repair flow tests | `tests/update_repair_flow.rs` | Update invokes the new binary's install repair path and ends with Rust-first resolution |
| Install/update smoke output assertions | install/update tests | Normal output excludes stale hook-file `❌` lines, `profile_management` warnings, and literal `Skipping symlink` noise |
| Release source tests | `commands/install/tests/issue_1955_force_refresh.rs`, `issue_1955_tag_fetch.rs` | `--force-refresh` uses a same-version `AMPLIHACK_HOME` with no false "not found" message, fetches only the release tag, and fails non-zero without touching the bundle |
| Bundle compatibility tests | `commands/install/bundle_compat.rs` and `tests/active_recipe_guard.rs` | Stale monolithic smart-orchestrator bundles are rejected; active `orch_helper.py` dependencies fail; stale installed bundles are atomically replaced |

## Interaction with related features

### Self-heal asset re-stage

The [self-heal](self-heal-asset-restage.md) mechanism acts as a safety net.
If the post-update subprocess install fails for any reason, the next
`amplihack` launch will detect the version-stamp mismatch and re-run install
automatically. The self-heal path calls `run_install` **in-process** (not
via subprocess), which is correct because there is no binary replacement in
flight — the running binary IS the new binary by that point.

Self-heal does not perform a separate startup-time framework compatibility
scan. When it re-runs install, that install run uses the same source and staged
bundle compatibility validation as an explicit `amplihack install`.

The post-update install subprocess still passes `--force-refresh`, so update
continues to bypass installed-bundle reuse and validate the release source and
staged destination.

### Versioned install layout

`amplihack update` copies the downloaded binaries into a new release under
`~/.amplihack/releases/`, checks the release binary's version, then switches
only the links it planned: `~/.local/bin/amplihack` and `amplihack-hooks`.
`amplihack-asset-resolver` and `~/.amplihack/amplifier-bundle` stay on their
earlier release. The post-install child then switches `amplifier-bundle`, and
`amplihack-asset-resolver` only if it has a resolver binary to deploy. It holds `~/.amplihack/.install.lock` while it does
this and releases it before it starts the post-install child, which takes the
lock again. Runs already alive keep their old binary and bundle. See
[Versioned Install Layout](../concepts/versioned-install-layout.md#self-heal-and-update).

### Startup self-update prompt

The [subprocess-safe skip](startup-update-prompt-subprocess-safe.md)
mechanism prevents the post-update install subprocess from triggering its
own update prompt. The `AMPLIHACK_NO_UPDATE_CHECK=1` env var set by
`build_install_command` is one of the skip signals documented there.

## See also

- [Versioned Install Layout](../concepts/versioned-install-layout.md) — release
  directories, link switch and in-use cleanup used by update and install.
- [Install Command Reference](../reference/install-command.md) — the install
  procedure invoked by the subprocess.
- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md) —
  shadowing stale wrapper quarantine, PATH precedence, and manual repair guidance.
- [Framework source selection reference](../reference/framework-source-selection.md) -
  which source update stages from and what staging checks.
- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md) -
  active smart-orchestrator guard and staged bundle activation.
- [Repair install/update PATH conflicts](../howto/repair-install-update-path-conflicts.md) —
  user-facing repair steps for stale Python/uvx wrappers and unknown conflicts.
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
