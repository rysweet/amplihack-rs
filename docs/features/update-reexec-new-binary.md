---
title: Post-update installation with the new binary
description: Release executable publication, lock handover and post-update repair behaviour.
doc_type: explanation
review_schedule: as-needed
---

# Post-Update Install: Re-exec New Binary

> [Home](../index.md) > [Features](README.md) > Post-Update Install Re-exec

When `amplihack update` installs a new Rust binary, the post-update install
step spawns the **new** binary as a subprocess rather than running the old
binary's compiled-in install code. This ensures that binary precedence repair,
stale wrapper quarantine, asset refresh, and configuration migrations shipped
in the new version take effect immediately.

Before taking the installation lock or allocating a persistent release, update
runs both downloaded executables with `--version`. Each must exit successfully
and report its own product name and the exact selected release version. An
invalid executable image, failed probe, malformed response or version mismatch
fails the update without switching active links. The staged copies are probed
again before either is activated, so validation applies to the artifacts that
will run. Success messages are emitted only after publication.

## Problem

Issue [#683](https://github.com/rysweet/amplihack-rs/issues/683).

`amplihack update` performs two major steps:

1. **Binary publication** — downloads release binaries and publishes their
   user-level links under the installation lock, then releases the lock.
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

After `run_update()` selects a safe replacement target,
`download_and_replace()` publishes the downloaded binaries. For the user-level
target, the parent stages a new release and switches the `amplihack-hooks` and
`amplihack` links. It returns the installed CLI path after releasing the lock.
The parent then spawns that path as a child instead of calling `run_install`
in-process.

The publication lock is released before the child starts; the child takes its
own installation lock. Explicit update publishes the selected release's
downloaded executables. It never substitutes the running checkout executable,
even when invoked from a checkout or with `AMPLIHACK_SKIP_AUTO_INSTALL=1`.

The child resolves its required executables from an eligible installed release of
the selected version or that exact published platform archive. This uses the
[private resolved-binaries contract](../reference/install-command.md#release-binary-staging-internal).
Framework source selection
remains separate from executable provenance.

```
amplihack update
  │
  ├─ analyze PATH conflicts
  │    -> inspect ordered candidates for amplihack + amplihack-hooks
  │    -> select ~/.local/bin as the preferred user-level target
  │    -> classify stale wrappers, unknown executables, and inaccessible paths
  │
  ├─ download_and_replace(&release)
  │    → downloads and extracts; verifies SHA-256 when a checksum is published
  │    → takes the installation lock
  │    → stages a binary release; atomically switches each planned binary link
  │    → releases the lock before returning
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

1. **Explicit executable path** — `download_and_replace()` returns the installed
   destination `PathBuf`. That path is passed directly to `Command::new()`;
   the old running process's `current_exe()` does not select the child or
   supply executable bytes for publication.

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

The new child binary stages the framework bundle. With the versioned layout,
the parent first publishes a binary-only release under
`~/.amplihack/releases/`, switching the two user-level binary links while
holding `~/.amplihack/.install.lock`. The existing bundle and asset-resolver
links remain on their earlier release. After the parent releases the lock,
the child acquires its own lock, stages framework assets and binaries, and
publishes the links it planned. Cleanup runs in the child. Running processes
retain their earlier binary and bundle.

Each link switch is atomic; the set of switches and the parent-to-child
sequence are not one atomic transaction. A partial publication leaves
successful switches in place. A child failure does not roll back the parent's
binary publication, and links may temporarily name different complete
releases. Retry the failed explicit operation to finish installation.

The first update from 1.2.107 uses the older parent's rename-based replacement
before handing off to the new child. That historical parent behavior differs
from current release-link publication. See
[the first update to the versioned layout](../concepts/versioned-install-layout.md#the-first-update-to-this-release).
Installs older than 0.9.69 run installation in-process with their own code;
stop recipe runs before updating them.

Each release records the process that committed it in `.installed-by`
(`version`, `exe`, `pid`, `post_update`). In the child-installed bundle's
release, `post_update=1` and the new version identify the child that installed
the assets. Its `exe` can resolve into the parent's binary release. On the
first update from an older layout it can instead name the regular file
`$HOME/.local/bin/amplihack` renamed into place by that older parent:

```bash
amplihack update
grep -E '^(version|exe|post_update)=' \
  "$(readlink -f ~/.amplihack/amplifier-bundle)/../.installed-by"
# version=<version>
# exe=<new child executable path>
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

The [self-heal](self-heal-asset-restage.md) mechanism can repair the mismatch on
a later launch only when the running executable is an eligible installed
release and automatic installation is enabled. A standalone downloaded binary
outside the releases tree does not gain implicit installation authority.

## Failure modes

### Subprocess exits non-zero

The error message includes both the executable path and the exit status:

```
Error: post-update install subprocess /home/user/.local/bin/amplihack exited with exit status: 1
```

The binary update itself has already succeeded, and `amplihack update` exits
non-zero. The user can re-run `amplihack install` manually. The self-heal
mechanism may repair it on a later invocation only if executable eligibility,
automatic-install suppression and version policy permit it. A post-publication
asset-installation failure does not imply rollback of the binary publication.
The error identifies the failed phase; there is no checkout-executable fallback.

### Release executables unavailable

Under the exact-release resolution contract, if required
executables cannot be obtained or validated before publication,
the active release is not switched. A checkout build, local framework tree,
sibling executable or `PATH` helper cannot substitute for the selected release.
Restore access to a supported published release, then run explicit `install`
or `update` again.

This pre-publication guarantee concerns the operation resolving those
executables. If resolution fails in the child, the parent's binary publication
has already happened and remains in place.

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

`--local` recovery and `AMPLIHACK_HOME` address framework assets only. Under
the exact-release contract, neither can supply unavailable release
executables to the child.

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
| `crates/amplihack-cli/src/update/install.rs` | `download_and_replace()` stages downloaded binaries, publishes user-level release links under the installation lock, releases the lock, and returns the installed CLI path. Other selected targets use individual atomic file replacements. |
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
If the post-update subprocess install fails, a later launch can retry through
self-heal when its command runs the startup check. Self-heal assesses both the
version stamp and installed-bundle compatibility, including when the stamp
matches. Automatic retry requires the running executable to belong to a
complete, non-retired installed release, `AMPLIHACK_SKIP_AUTO_INSTALL` to be
unset or empty, and the downgrade guard to permit repair. Checkout and Cargo
builds cannot retry automatically. Ineligible or opted-out executions use
uncached, read-only assessment without changing installation state.

The self-heal path calls `run_install` **in-process**, because no binary
replacement is in flight. That install uses the same source and staged-bundle
compatibility validation as an explicit `amplihack install`. Run
`amplihack install` to retry explicitly; explicit install and update remain
available regardless of the automatic-installation opt-out or executable origin,
subject to their existing source validation and publication safeguards.

The post-update install subprocess still passes `--force-refresh`, so update
continues to bypass installed-bundle reuse and validate the release source and
staged destination.

### Versioned install layout

`amplihack update` copies the downloaded binaries into a new release under
`~/.amplihack/releases/`, checks the release binary's version, then switches
only the links it planned: `~/.local/bin/amplihack` and `amplihack-hooks`.
`amplihack-asset-resolver` and `~/.amplihack/amplifier-bundle` stay on their
earlier release. The parent holds `~/.amplihack/.install.lock` through binary
publication, then releases it before starting the post-install child. The
child takes the lock again and publishes its staged bundle and binaries.
The child stages `amplihack-asset-resolver` when the selected executable source
includes it. Every supplied executable is validated before staging. Link switches are individually
atomic, without rollback across the whole operation. Runs already alive keep
their old binary and bundle. See
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
