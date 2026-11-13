---
title: Install and uninstall command reference
description: Release executable sources, framework installation, configuration and failure behaviour.
doc_type: reference
review_schedule: as-needed
---

# amplihack install / uninstall — Command Reference

## Synopsis

```
amplihack install [--interactive] [--local <PATH>] [--verbose]
amplihack uninstall
```

## amplihack install

Bootstraps or repairs the amplihack environment on the current machine. On
first run, it deploys the Rust binaries to `~/.local/bin`, makes that directory
resolve first for future shells, refreshes framework assets from a source with
the binary's version, registers hooks, and finally verifies the selected
`amplihack` is the Rust binary.

Subsequent runs are idempotent. They update existing registrations and managed
profile blocks in place, quarantine newly discovered shadowing stale wrappers,
and replace the installed `amplifier-bundle` instead of merging over it.

The installer stages only a source whose version equals the binary's, unless you
pass `--local`. If `AMPLIHACK_HOME` or the current checkout has another version,
install refuses before changing anything. If no local source qualifies, it fetches
the tag of the binary's release. See
[Framework source selection](./framework-source-selection.md).

The installed `~/.amplihack/amplifier-bundle` is not reused by
`--force-refresh`, and stale smart-orchestrator assets are rejected before they
can become active. See [Framework bundle compatibility](./framework-bundle-compatibility.md).

Explicit `install` and `update`, including post-update `install --force-refresh`,
work with the automatic-install opt-out set and may be invoked outside the
installed releases tree. They publish release executables, never the running
checkout as a substitute. Install first resolves all required executables from
an eligible installed release matching the selected version, or from that exact
published platform archive. The release version determines release allocation
and the version stamp; a local framework override does not select executable
bytes or change that version.

If the release executables cannot be obtained or validated, install exits
non-zero before acquiring framework sources, taking the installation lock,
allocating a release or changing profiles. Existing release contents, the
stamp and active links remain unchanged. Use a supported published release
through `install` or `update`; building a checkout is not an installation source.

A checkout cannot acquire publication authority through startup self-heal or
launch bootstrap, even when its version is newer. See
[Executable eligibility](../features/self-heal-asset-restage.md#executable-eligibility).

You can invoke the same command through the npm wrapper package when desired:

```sh
npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install
```

The wrapper only changes how the native binaries are obtained. Once it hands off
to the Rust CLI, the install phases below are unchanged.

See [Install Completeness Verification](./install-completeness.md) for the hard-fail contract that prevents partial framework staging from being reported as a successful install. See [Framework bundle compatibility](./framework-bundle-compatibility.md) for the smart-orchestrator compatibility contract that prevents stale recipe bundles from being accepted.

Published release archives currently cover Linux and macOS on `x64`/`arm64`.
On Windows, or any other platform without a published release target, the npm
wrapper needs the packaged Rust workspace plus a local Rust toolchain so it can
fall back to a source build.

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--interactive` | `bool` | `false` | Launch a guided setup wizard that prompts for default tool, hook scope, and update-check preference. Requires a TTY; falls back to defaults with a warning if stdin is not a terminal. See [Interactive Install](../howto/interactive-install.md). |
| `--local <PATH>` | `PathBuf` | absent | Select framework assets from a specific local directory. The path must exist, be a directory, contain a framework root marker, and pass framework bundle compatibility validation. Its version may differ from the binary's: install prints both versions and a warning, then stages it. This is the only way to stage a tree with a different version. It does not select or authorise checkout executables. |
| `--verbose` | `bool` | `false` | Accepted for diagnostic scripts. The install command already emits phase-level diagnostics by default. |
| `--force-refresh` | `bool` | `false` | **Hidden.** Refreshes `amplifier-bundle/` while bypassing the installed bundle and the current directory as sources. Uses a same-version `AMPLIHACK_HOME`, or otherwise fetches the release tag `v<binary version>`. Used internally by `amplihack update` when spawning the new binary as a post-update install subprocess. Not shown in `--help` output. See [Post-Update Install - Re-exec New Binary](../features/update-reexec-new-binary.md). |

The `--interactive` and `--local` flags compose: `--interactive` controls configuration preferences while `--local` controls the framework source path.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Install completed successfully |
| `1` | required release executables are unavailable or invalid; the active release was not switched |
| `1` | `--local` path does not exist or is not a directory |
| `1` | `--local` path does not contain a framework root marker |
| `1` | selected framework bundle is stale or incompatible |
| `1` | `AMPLIHACK_HOME` or the current checkout has a different or unknown version; nothing was changed. See [refusal rules](./framework-source-selection.md#source-rules). |
| `1` | no local source qualifies and this dev or snapshot build may not fetch a release tag |
| `1` | fetched release tree has the wrong or an unknown version |
| `1` | staged bundle does not match its source ([manifest check](./framework-source-selection.md#manifest-check)) |
| `1` | unknown or inaccessible executable still shadows the Rust `~/.local/bin/amplihack` after repair |
| `1` | stale wrapper quarantine failed and the wrapper still shadows the Rust binary |
| `1` | Release tag fetch or extraction failed (non-local mode only); bundle and stamp unchanged |

Note: A Node.js version below v24 does **not** cause a non-zero exit from `amplihack install`. It produces a warning at the Copilot plugin step. By contrast, `amplihack copilot` exits with code 1 if Node.js < v24. See [Node.js Version Checking](./node-version-checking.md).

### Install Phases

Executable sources are resolved before framework acquisition and persistent installation writes.

```
amplihack install [--interactive]
│
├── 0a. resolve release binaries  — matching eligible installed release or exact published platform archive; validate required executables
├── 0b. acquire_framework_source() — --local, a same-version local source, or the release tag; refuse on mismatch
├── 1. maybe_run_wizard()         — if --interactive, prompt for tool/scope/update prefs (skipped otherwise)
├── 1b. take ~/.amplihack/.install.lock and allocate a new release ~/.amplihack/releases/<release>/
├── 2. stage resolved binaries    — copy validated release executables into the release's bin/; no current-executable or PATH fallback
├── 2b. analyze PATH candidates   — classify Rust binaries, stale wrappers, unknowns, and inaccessible paths
├── 2c. neutralize stale wrappers — quarantine only shadowing, positively identified Python/uvx wrappers in safe locations
├── 2d. persist PATH precedence   — write/update managed shell profile block so ~/.local/bin resolves first
├── 3. refresh framework assets   — validate source, stage the bundle into the release, compare with source, set modes, validate; swap mapped dirs into ~/.amplihack/.claude and retire the old trees
├── 3b. commit the release        — write .complete, then atomically switch each planned link (~/.local/bin binaries, ~/.amplihack/amplifier-bundle); no rollback on a partial switch; clean up unused releases
├── 4. verify Rust selection      — confirm selected amplihack is the Rust binary after PATH and bundle repair
├── 5. create_runtime_dirs()      — create runtime/ subdirs with 0o755 permissions
├── 6. ensure_settings_json()     — backup settings.json, register hooks, set permissions
├── 7. verify_framework_assets()  — confirm required staged framework assets exist
├── 8. apply_config()             — if wizard ran, write preferences to manifest and settings
├── 9. write_manifest()           — write amplihack-manifest.json for uninstall
├── 9b. ensure_recipe_runner()    — `cargo install` recipe-runner-rs if missing; bootstraps rustup (and build-essential on apt systems) when absent
└── 10. ensure_mermaid_cli()      — best-effort: provision mmdc (npm @mermaid-js/mermaid-cli); warn-and-continue on failure
```

Phase 0 runs first, so a refusal changes nothing and comes before any prompt.

Phases 1b–3b never change or delete a file that a running recipe run may still
execute or read. Binaries and the bundle go into a new release directory, and
the links switch to it only after the release is complete. Old releases are
deleted only when no process uses them, and install never signals a process it
did not start. A run alive across an install completes. See
[Versioned Install Layout](../concepts/versioned-install-layout.md).

Phase 1 runs only when `--interactive` is passed **and** stdin is a TTY. If `--interactive` is set but no TTY is available, the wizard is skipped with a warning to stderr. Phase 7 applies wizard results (default tool, update-check preference) to the manifest and writes hooks to the selected settings.json scope.

Phase 10 runs **after** the version stamp, manifest, and Copilot-home staging, so an `mmdc` failure can never leave required install state unwritten. It is **optional and best-effort**: it attempts `npm install -g @mermaid-js/mermaid-cli` only when npm is available and `mmdc` is missing, and it always continues — a failed or skipped install emits a warning/info line and never fails the install. See [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md).

Phase 9b installs `recipe-runner-rs` with `cargo install` when it is not
already present. It is a hard failure when it cannot (install-completeness,
issue #527), but it runs after the uninstall manifest is written, so a failure
never leaves staged assets without an `amplihack uninstall` path (issue
#1491); the error says how to finish or clean up. On a fresh machine with no Rust toolchain it first runs the
official rustup installer (`rustup-init.sh -y --no-modify-path --profile
minimal`) into `$CARGO_HOME` (default `~/.cargo`); shell profiles are not
modified because amplihack finds `~/.cargo/bin` itself. If there is no `cc`
(rustc's linker) and `apt-get` is available, it installs
`build-essential` as root or through passwordless `sudo -n` (on a fresh VM it
waits up to about 5 minutes for first-boot `apt-daily`/`unattended-upgrades`:
the dpkg lock via `DPkg::Lock::Timeout`, and the apt lists lock by retrying
`apt-get update` while that lock is the failure; any other `update` error
falls through to the install with the existing package lists); it never
prompts for a password and instead fails with the command to run.
`recipe-runner-rs` is found in `$CARGO_HOME/bin` even when that directory is
not on PATH. Only an explicit `amplihack install` does this; the startup self-heal,
launch-time checks and the install that `amplihack update` runs afterwards
never download rustup or run apt.
Set `AMPLIHACK_NO_RUST_BOOTSTRAP=1` to disable both automatic installs.

### Environment Variables

These variables are read during install. All are optional; the installer works without any of them set.

| Variable | Effect |
|----------|--------|
| `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | Hook lookup override for callers using [binary resolution](./binary-resolution.md). Installation stages companions from the resolved release source, independently of this override. |
| `AMPLIHACK_HOME` | The first [framework source](./framework-source-selection.md) tried. It doesn't change where install stages, which is always `$HOME/.amplihack`: a different version is refused by plain `install`. |
| `AMPLIHACK_SKIP_AUTO_INSTALL` | Any non-empty value suppresses [implicit installation](../features/self-heal-asset-restage.md#what-the-bypass-skips), including self-heal and launch reconciliation. Compatibility assessment is read-only and uncached. Has no effect on explicit `install` or `update`. |
| `AMPLIHACK_NO_RUST_BOOTSTRAP` | When set to a non-empty value other than `0`, install never installs rustup or `build-essential` automatically; a missing cargo or C linker fails phase 9b with manual instructions. |
| `AMPLIHACK_SKIP_MMDC` | When set to any non-empty value, skips the best-effort [Mermaid CLI provisioning](../features/mermaid-cli-best-effort-install.md) step (no `mmdc`/`npm` probe, no `npm install -g @mermaid-js/mermaid-cli`). The install proceeds normally; this optional step never gates a successful install. |

### Version stamp

Every successful install writes the selected release version into
`~/.amplihack/.installed-version` (plain text, no trailing newline, atomic
tmp+rename). An implicit re-stage that kept an installed binary adds a second
line, `binaries: <version|unknown>`
([details](../features/self-heal-asset-restage.md#stamp-file)). This stamp is read on every subsequent launch
by the [self-heal check](../features/self-heal-asset-restage.md): if the
stamp is missing, unparseable or older than `crate::VERSION`, an implicit
re-stage runs before the requested command dispatches only if the executable
is an eligible installed release and the opt-out is inactive. Otherwise repair
is skipped without changing installation state; commands with usable assets
continue. A stamp newer than the
running binary is never re-staged over; a warning is printed instead
([details](../features/self-heal-asset-restage.md#never-an-implicit-downgrade-1526)).
Manual installs and
the self-heal path therefore converge on the same source of truth.

### Output

Successful install prints a phase-by-phase progress summary:

```
📦 Using framework source /home/alice/src/amplihack-rs (v1.2.99, from AMPLIHACK_HOME)
✓ Deployed amplihack → ~/.local/bin/amplihack
✓ Deployed amplihack-hooks → ~/.local/bin/amplihack-hooks
✓ Deployed amplihack-asset-resolver → ~/.local/bin/amplihack-asset-resolver
✓ Updated shell PATH profile block
✓ Refreshed amplifier-bundle
✓ Verified Rust amplihack resolves first
✓ Created runtime directories
✓ Backed up ~/.claude/settings.json → settings.json.backup.1741651200
✓ Registered 7 Claude Code hooks
✓ XPIA hooks directory found
   Manifest written to ~/.claude/install/amplihack-manifest.json
✅ Amplihack installation completed successfully!
```

After the success banner, two best-effort post-install steps run (each prints
one status line and never fails the install): Copilot-home staging and
[Mermaid CLI provisioning](../features/mermaid-cli-best-effort-install.md). For
example, on a host with npm and `mmdc` already present:

```
  ✅ Copilot home staged (~/.copilot/)
  ✓ mermaid CLI (mmdc) already installed; skipping
```

When npm is unavailable the mermaid step skips with an informational line
instead, and install still succeeds:

```
  ℹ npm not available; skipping mermaid CLI install (pr-guide will fall back to mermaid.ink)
```

If `~/.local/bin` is not first on `PATH`, install updates the managed profile
block:

```text
Updated shell PATH profile block:
  /home/alice/.bashrc
```

If a stale Python or uvx wrapper is safely neutralized:

```text
Quarantined stale amplihack wrapper:
  /home/alice/.local/share/uv/tools/amplihack/bin/amplihack
  -> /home/alice/.amplihack/quarantine/stale-wrappers/20260710T183440Z/...
```

If an unknown executable still shadows the Rust binary, install fails instead
of pretending the repair succeeded:

```text
Cannot select Rust amplihack because an unknown executable shadows it:
  /home/alice/bin/amplihack
```

See [Install/update PATH conflict reference](./install-update-path-conflicts.md).

---

## amplihack uninstall

Removes all files, directories, binaries, and hook registrations tracked by the install manifest. It runs under the install lock and keeps any release a running process still uses; see [uninstall in the versioned layout](../concepts/versioned-install-layout.md#uninstall).

### Options

None.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Uninstall completed (or nothing to remove) |
| `1` | A listed path resolved outside an allowed base directory (security check) |

### Uninstall Phases

```
amplihack uninstall
│
├── Read ~/.amplihack/.claude/install/amplihack-manifest.json
│   (if missing: warn and use hardcoded fallback list)
│
├── Phase 1 — Remove manifest files
│   └── validates each path is under ~/.amplihack/.claude/
│
├── Phase 2 — Remove manifest directories (deepest first)
│   └── validates each path is under ~/.amplihack/.claude/
│
├── Phase 3 — Remove manifest binaries
│   └── validates each path is under ~/.local/bin/; removes a link only if it is a symlink (a regular file is retired)
│
├── Phase 3b — Remove ~/.amplihack/amplifier-bundle link and unused releases
│   └── keeps and lists releases that running processes still use
│
└── Phase 4 — Remove hook registrations from ~/.claude/settings.json
    └── retains XPIA and non-amplihack entries
```

### What Uninstall Preserves

- `~/.claude/settings.json` (the file itself — only amplihack entries are removed)
- `~/.claude/settings.json.backup.*` files
- XPIA hook registrations
- Any other tool's hook registrations

---

## Internal API (for contributors)

The functions below are in `crates/amplihack-cli/src/commands/install/mod.rs`.

### `run_install(local: Option<PathBuf>, interactive: bool, force_refresh: bool) -> Result<()>`

Entry point called by the command dispatcher. When `interactive` is true and stdin is a TTY, runs the interactive wizard before proceeding. Canonicalizes and validates `--local` path when provided. The install path deploys user-level Rust binaries, neutralizes eligible shadowing stale wrappers, persists PATH precedence, refreshes the framework bundle, and then verifies Rust-first command resolution.

The `force_refresh` parameter controls framework source resolution:

| `force_refresh` | Behavior |
|-----------------|----------|
| `false` | **Default.** Tries the local sources in order, honoring `--local` when supplied, and then the release tag. Every candidate is validated before staging. |
| `true` | **Bypasses the installed bundle and the current directory as sources.** Uses `--local`, a same-version `AMPLIHACK_HOME` or the release tag, so `amplihack update` cannot re-stage stale assets already present at `~/.amplihack/amplifier-bundle/`. The source and staged destination are still validated for compatibility. |

**Priority rule:** When `local` is `Some(path)`, it takes precedence over `force_refresh` for source selection, but not for compatibility. The user-specified path must still pass framework bundle validation. This is correct by design: `--local` is an explicit source override that may change the version, not permission to stage stale smart-orchestrator assets.

**Call sites and their `force_refresh` values:**

| Caller | `force_refresh` | Rationale |
|--------|-----------------|-----------|
| `amplihack install` (command dispatcher) | `false` | Standalone install prefers compatible same-version local sources and refuses a mismatched `AMPLIHACK_HOME` or checkout |
| `amplihack update` (post-update closure) | `true` | Stages the bundle of the release just installed |
| Self-heal (startup version-stamp and compatibility check) | `false` | Re-runs install only for an eligible installed executable without the opt-out; skips mismatched sources rather than refusing. Ineligible and opted-out executions assess compatibility without caching or installation writes |
| `ensure_framework_installed()` (bootstrap) | `false` | Bootstrap applies executable eligibility and the opt-out before installation or reconciliation; permitted repair prefers compatible same-version local sources and skips mismatched ones |

### `validate_framework_bundle_compatibility(root: &Path) -> Result<()>`

Validates a candidate framework bundle before install accepts it as a source.
The validator accepts either a repository root containing `amplifier-bundle/` or
the `amplifier-bundle/` directory itself. It requires the composable
`smart-orchestrator.yaml` and the four companion recipes:
`smart-classify-route`, `smart-execute-routing`, `smart-reflect-loop`, and
`smart-validate-summarize`.

Stale monolithic smart-orchestrator recipes, Python/importlib orchestration,
current-use `orch_helper.py` references, and old `helper-path` orchestration
helper flows are rejected. `helper-path` itself remains valid and continues to
resolve to `amplifier-bundle/bin/multitask-orchestrator.sh`.

Those stale-marker checks are scoped to the current
`recipes/smart-orchestrator.yaml` behavior. Historical docs and tests may still
mention `orch_helper.py` or old helper-path behavior when describing past
failures.

Located in `crates/amplihack-cli/src/commands/install/bundle_compat.rs`.

### `validate_staged_framework_bundle(root: &Path) -> Result<()>`

Validates the installed `amplifier-bundle/` after staging. This is a hard
install failure so stale destination files cannot remain after an otherwise
successful install/update repair.

Located in `crates/amplihack-cli/src/commands/install/bundle_compat.rs`.

### `is_compatible_framework_bundle(root: &Path) -> bool`

Boolean helper for source discovery. Use it to reject optional stale candidates
while continuing to the next candidate. Final staging verification should use
`validate_staged_framework_bundle()` for actionable diagnostics.

Located in `crates/amplihack-cli/src/commands/install/bundle_compat.rs`.

### `run_uninstall() -> Result<()>`

Entry point for uninstall. Reads the manifest, then executes phases 1–4.

### `local_install_resolved(repo_root, wizard, reconcile_codex, mode, binaries) -> Result<()>`

Core install logic. Runs the install phases through manifest writing, using resolved release executables for staging and hook registration.

### `find_hooks_binary() -> Result<PathBuf>`

Locates the `amplihack-hooks` binary using a 5-step resolution:

1. `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` environment variable (must point to an existing executable)
2. Sibling of the currently running `amplihack` executable
3. `PATH` lookup via `which amplihack-hooks`
4. `~/.local/bin/amplihack-hooks`
5. `~/.cargo/bin/amplihack-hooks`

PATH runs at Step 3 so system-wide installs (e.g. a tarball to `/usr/local/bin`) survive the uninstall→reinstall cycle, since `amplihack uninstall` only removes the `~/.local/bin` copies.

Returns an actionable error if none of the five locations yield an executable.
Release publication uses separately resolved release executables. This lookup remains available to other callers.

### Release binary staging (internal)

`deploy_binaries_into()` stages supplied, validated release executable paths
with `0o755` permissions. It does not discover `current_exe()`, sibling builds
or companions on `PATH`. The installation coordinator resolves `amplihack`,
`amplihack-hooks` and the asset resolver where required by the selected release
before installation mutations. Its private resolved-binaries value retains
the release version, executable paths and any temporary extraction directory
until staging completes. This adds no public Rust API or configuration option.

Lower-level implicit deployment independently enforces the opt-out and
executable-origin restriction before writing. Callers cannot bypass those
checks by skipping startup self-heal.

After deployment, install analyses ordered `PATH` candidates for `amplihack` and
`amplihack-hooks`, quarantines eligible stale Python/uvx wrappers that shadow
Rust, persists `~/.local/bin` precedence, refreshes the framework bundle, and
then verifies the selected `amplihack` is the Rust binary. Unknown or
inaccessible shadowing candidates are reported and fail the repair if
Rust-first resolution cannot be guaranteed.

### `neutralize_stale_wrappers(report: &PathConflictReport) -> Result<NeutralizationReport>`

Moves positively identified shadowing stale Python/uvx `amplihack` wrappers into
`~/.amplihack/quarantine/stale-wrappers/<timestamp>/`. The function re-checks
basename, safe location, ownership, metadata, and symlink boundaries before
moving any file. It never mutates system-managed prefixes or unknown
executables.

Located in `crates/amplihack-cli/src/commands/install/stale_wrappers.rs`.

### `ensure_user_bin_precedence() -> Result<PathPrecedenceReport>`

Writes or updates the managed shell profile block that prepends
`$HOME/.local/bin` when it is not already first. The detected profile is
`~/.bashrc` for bash and `~/.zshrc` for zsh. Preserves unrelated profile
content and returns the file changed.

Located in `crates/amplihack-cli/src/commands/install/paths.rs`.

For macOS executable quarantine guidance, see the
[First-time install how-to](../howto/first-install.md#macos-sip-note).

### `ensure_settings_json(staging_dir: &Path, timestamp: u64, hooks_bin: &Path) -> Result<(bool, Vec<String>)>`

Reads or creates `~/.claude/settings.json`. Creates a timestamped backup and backup metadata JSON (both with `0o600` permissions). Calls `validate_hook_command_string()` on each command before writing. Calls `update_hook_paths()` for amplihack hooks and, when the XPIA tools directory exists, for XPIA hooks (using `XPIA_HOOK_SPECS` which route through the `amplihack-hooks` binary). Returns `(settings_existed, registered_event_names)`.

### `validate_hook_command_string(cmd: &str) -> Result<()>`

Validates that a hook command string does not contain shell metacharacters (`|&;$\`(){}<!>#~*\`). Called by `ensure_settings_json()` and `update_hook_paths()` before any write to `settings.json`. Returns an error with the offending string identified if validation fails.

### `update_hook_paths(settings, hook_system, specs, hooks_dir, hooks_bin)`

Iterates `specs` and calls `validate_hook_command_string()` on each command string before upserting its hook wrapper into `settings["hooks"][event]`. Uses `wrapper_matches()` for idempotency. Preserves order — `workflow-classification-reminder` always precedes `user-prompt-submit` in the `UserPromptSubmit` array.

All hook registrations use **binary subcommands** such as `"amplihack-hooks post-tool-use"`. Both amplihack and XPIA hooks route through the compiled `amplihack-hooks` binary — there are no Python or shell script hook files to deploy or verify.

### `remove_hook_registrations(settings) -> Result<()>`

Removes hook array entries whose command string contains `amplihack-hooks` or `tools/amplihack/`. Preserves all other entries.

### `maybe_run_wizard(interactive: bool) -> Result<Option<InteractiveConfig>>`

Checks whether the wizard should run (`interactive == true` and stdin is a TTY). If so, presents three `dialoguer::Select` prompts and returns an `InteractiveConfig`. If `interactive` is true but no TTY is available, prints a warning to stderr and returns `None`. If `interactive` is false, returns `None` immediately. Located in `crates/amplihack-cli/src/commands/install/interactive.rs`.

### `apply_config(config: &InteractiveConfig, manifest: &mut InstallManifest, settings_path: &Path) -> Result<()>`

Writes wizard results to the install manifest (`default_tool`, `update_check_preference` fields) and, for repo-local hook scope, to the repo-local `settings.json`. Located in `crates/amplihack-cli/src/commands/install/interactive.rs`.

### `ensure_mermaid_cli() -> Result<Outcome>`

Best-effort provisioning of the Mermaid CLI (`mmdc`, npm `@mermaid-js/mermaid-cli`). Invoked from `local_install()` after Copilot-home staging. Runs probe → optional `npm install -g @mermaid-js/mermaid-cli` → re-probe and prints one status line for the resolved `Outcome` (`AlreadyPresent`, `Installed`, `SkippedByEnv`, `SkippedNoNpm`, `Failed`). **Always returns `Ok`** — a failed or skipped install is encoded in the `Outcome` and warned via `tracing::warn!` + a stderr `⚠️` line, never propagated as an install error. Honors `AMPLIHACK_SKIP_MMDC`. Uses `std::process::Command` in argument-vector form (no shell) with a hardcoded package spec; never escalates privileges. Located in `crates/amplihack-cli/src/commands/install/mermaid_cli.rs`. See [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md).

## See Also

- [Versioned Install Layout](../concepts/versioned-install-layout.md) — releases, atomic link switch, in-use cleanup, the install lock and the no-signal rule
- [Framework Source Selection](./framework-source-selection.md) — which source install stages from, the manifest check and file modes
- [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md) — optional `mmdc` install for local mermaid rendering in `pr-guide`
- [Interactive Install](../howto/interactive-install.md) — guided setup wizard walkthrough
- [Repair stale amplihack wrappers and PATH conflicts](../howto/repair-install-update-path-conflicts.md) — quarantine shadowing stale Python/uvx wrappers and keep Rust `amplihack` first
- [Install/update PATH conflict reference](./install-update-path-conflicts.md) — target selection and resolver API
- [Hook Specifications](./hook-specifications.md) — the 7 hooks registered by amplihack install
- [Install Manifest](./install-manifest.md) — manifest schema
- [Binary Resolution](./binary-resolution.md) — find_hooks_binary lookup detail
- [Framework bundle compatibility](./framework-bundle-compatibility.md) — smart-orchestrator compatibility contract and stale bundle repair
