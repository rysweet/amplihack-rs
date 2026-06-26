# amplihack install / uninstall — Command Reference

## Synopsis

```
amplihack install [--interactive] [--local <PATH>] [--verbose]
amplihack uninstall
```

## amplihack install

Bootstraps the amplihack environment on the current machine. On first run, it performs full setup: locates the bundled framework source (or falls back to network download), deploys native binaries, stages framework assets, and registers Claude Code hooks. Subsequent runs are idempotent — they update existing registrations in place without duplication.

Since issue #254, framework assets are bundled in the amplihack-rs source tree. The installer resolves the framework source in this order: (1) `AMPLIHACK_HOME`, (2) current-working-directory walk-up, (3) executable-path walk-up, (4) compile-time workspace root, (5) `~/.amplihack`, (6) network download from upstream (legacy fallback).

Since issue #675, the post-update installer bypasses local source selection and always downloads a fresh bundle from the network. This prevents stale `amplifier-bundle/` assets at `~/.amplihack/` from being re-staged after a binary update.

Since issue #734, every local framework source candidate is also checked for smart-orchestrator compatibility before it can be selected. Standalone `amplihack install` still uses the resolver order above, but stale or incompatible local bundles are skipped instead of being reused. See [Framework bundle compatibility](./framework-bundle-compatibility.md).

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
| `--local <PATH>` | `PathBuf` | absent | Install from a specific local directory instead of using the bundled source. The path must exist, be a directory, contain a framework root marker, and pass framework bundle compatibility validation. Without `--local`, the installer uses compatible bundled framework assets from the amplihack-rs source tree or falls back to download. |
| `--verbose` | `bool` | `false` | Accepted for diagnostic scripts. The install command already emits phase-level diagnostics by default. |
| `--force-refresh` | `bool` | `false` | **Hidden.** Forces a fresh network download of `amplifier-bundle/` assets, bypassing local source resolution. Used internally by `amplihack update` when spawning the new binary as a post-update install subprocess. Not shown in `--help` output. See [Post-Update Install — Re-exec New Binary](../features/update-reexec-new-binary.md). |

The `--interactive` and `--local` flags compose: `--interactive` controls configuration preferences while `--local` controls the framework source path.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Install completed successfully |
| `1` | `amplihack-hooks` binary not found after 5-step search |
| `1` | `--local` path does not exist or is not a directory |
| `1` | `--local` path does not contain a framework root marker |
| `1` | selected framework bundle is stale or incompatible |
| `1` | Framework archive download or extraction failed (non-local mode only) |

Note: A Node.js version below v24 does **not** cause a non-zero exit from `amplihack install`. It produces a warning at the Copilot plugin step. By contrast, `amplihack copilot` exits with code 1 if Node.js < v24. See [Node.js Version Checking](./node-version-checking.md).

### Install Phases

```
amplihack install [--interactive]
│
├── 0. maybe_run_wizard()         — if --interactive, prompt for tool/scope/update prefs (skipped otherwise)
├── 1. Bundled source, --local path, OR GitHub fallback — obtain compatible framework source
├── 2. deploy_binaries()          — copy amplihack + amplihack-hooks (+ asset resolver when present) to ~/.local/bin
├── 2b. analyze PATH conflicts    — warn if stale earlier PATH entries shadow ~/.local/bin
├── 3. copy framework assets      — validate source, stage mapped framework assets, validate staged bundle
├── 4. create_runtime_dirs()      — create runtime/ subdirs with 0o755 permissions
├── 5. ensure_settings_json()     — backup settings.json, register hooks, set permissions
├── 6. verify_framework_assets()  — confirm required staged framework assets exist
├── 7. apply_config()             — if wizard ran, write preferences to manifest and settings
├── 8. write_manifest()           — write amplihack-manifest.json for uninstall
└── 9. ensure_mermaid_cli()       — best-effort: provision mmdc (npm @mermaid-js/mermaid-cli); warn-and-continue on failure
```

Phase 0 runs only when `--interactive` is passed **and** stdin is a TTY. If `--interactive` is set but no TTY is available, the wizard is skipped with a warning to stderr. Phase 7 applies wizard results (default tool, update-check preference) to the manifest and writes hooks to the selected settings.json scope.

Phase 9 runs **after** the version stamp, manifest, and Copilot-home staging, so an `mmdc` failure can never leave required install state unwritten. It is **optional and best-effort**: it attempts `npm install -g @mermaid-js/mermaid-cli` only when npm is available and `mmdc` is missing, and it always continues — a failed or skipped install emits a warning/info line and never fails the install. See [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md).

### Environment Variables

These variables are read during install. All are optional; the installer works without any of them set.

| Variable | Effect |
|----------|--------|
| `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | Override the path used for `amplihack-hooks`. Useful in tests and CI. If set but the path does not exist, resolution falls through to Step 2. See [Binary Resolution](./binary-resolution.md). |
| `AMPLIHACK_HOME` | Override `~/.amplihack` staging root (default: `$HOME/.amplihack`). |
| `AMPLIHACK_SKIP_AUTO_INSTALL` | When set to any non-empty value, suppresses the startup-time [self-heal check](../features/self-heal-asset-restage.md) that would otherwise re-run install when `~/.amplihack/.installed-version` is missing or stale. Has no effect on an explicit `amplihack install` invocation. |
| `AMPLIHACK_SKIP_MMDC` | When set to any non-empty value, skips the best-effort [Mermaid CLI provisioning](../features/mermaid-cli-best-effort-install.md) step (no `mmdc`/`npm` probe, no `npm install -g @mermaid-js/mermaid-cli`). The install proceeds normally; this optional step never gates a successful install. |

### Version stamp

Every successful install writes the binary version into
`~/.amplihack/.installed-version` (single-line plain text, no trailing
newline, atomic tmp+rename). This stamp is read on every subsequent launch
by the [self-heal check](../features/self-heal-asset-restage.md): if the
stamp is missing or differs from `crate::VERSION`, install is run
automatically before the requested command dispatches. Manual installs and
the self-heal path therefore converge on the same source of truth.

### Output

Successful install prints a phase-by-phase progress summary:

```
✓ Using bundled framework assets from /path/to/amplihack-rs
✓ Deployed amplihack → ~/.local/bin/amplihack
✓ Deployed amplihack-hooks → ~/.local/bin/amplihack-hooks
✓ Deployed amplihack-asset-resolver → ~/.local/bin/amplihack-asset-resolver
✓ Staged framework assets (47 files, 12 directories)
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

If `~/.local/bin` is not in `$PATH`, an advisory is printed (install still succeeds):

```
⚠️  ~/.local/bin is not in $PATH
    Add: export PATH="$HOME/.local/bin:$PATH"
```

If another candidate appears earlier on `$PATH`, install prints an advisory
after deploying the user-local binaries:

```text
⚠️  PATH conflict: /usr/local/bin/amplihack appears before /home/alice/.local/bin/amplihack
    Your shell may continue to run the stale system binary.
    Fix: export PATH="$HOME/.local/bin:$PATH" or remove the stale system copy with sudo.
```

This warning does not mean install failed. It means the shell may still resolve
`amplihack` or `amplihack-hooks` to an older candidate until `PATH` is repaired.
See [Install/update PATH conflict reference](./install-update-path-conflicts.md).

---

## amplihack uninstall

Removes all files, directories, binaries, and hook registrations tracked by the install manifest.

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
│   └── validates each path is under ~/.local/bin/
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

Entry point called by the command dispatcher. When `interactive` is true and stdin is a TTY, runs the interactive wizard before proceeding. Canonicalizes and validates `--local` path when provided.

The `force_refresh` parameter controls framework source resolution:

| `force_refresh` | Behavior |
|-----------------|----------|
| `false` | **Default.** Resolves a compatible bundled framework root by checking `AMPLIHACK_HOME`, walking up from the current working directory, walking up from the executable path, checking the compile-time workspace root, then checking `~/.amplihack`. Incompatible local candidates are skipped. Falls back to network download only when no compatible local source is found. This is the behavior for standalone `amplihack install` and install runs triggered by self-heal. |
| `true` | **Skips local bundle resolution entirely.** Goes directly to `download_and_extract_framework_repo()` to fetch a fresh `amplifier-bundle/` from the upstream archive (`REPO_ARCHIVE_URL`). Used by the post-update installer to ensure that `amplihack update` always refreshes stale framework assets rather than re-staging the old bundle that was already present at `~/.amplihack/amplifier-bundle/`. The downloaded source and staged destination are still validated for compatibility. |

**Priority rule:** When `local` is `Some(path)`, it takes precedence over `force_refresh` for source selection, but not for compatibility. The user-specified path must still pass framework bundle validation. This is correct by design: `--local` is an explicit source override, not permission to stage stale smart-orchestrator assets.

**Call sites and their `force_refresh` values:**

| Caller | `force_refresh` | Rationale |
|--------|-----------------|-----------|
| `amplihack install` (command dispatcher) | `false` | Standalone install prefers compatible local sources |
| `amplihack update` (post-update closure) | `true` | **Root cause fix for issue #675** — forces fresh download |
| Self-heal (startup version-stamp check) | `false` | Re-runs install when the version stamp is stale; it benefits from install compatibility validation but does not perform a separate startup compatibility scan |
| `ensure_framework_installed()` (bootstrap) | `false` | Bootstrap prefers compatible local sources |

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

Boolean helper for local source discovery. Use it to skip optional stale
candidates such as `AMPLIHACK_HOME` while continuing to the next candidate or
network fallback. Final staging verification should use
`validate_staged_framework_bundle()` for actionable diagnostics.

Located in `crates/amplihack-cli/src/commands/install/bundle_compat.rs`.

### `run_uninstall() -> Result<()>`

Entry point for uninstall. Reads the manifest, then executes phases 1–4.

### `local_install(repo_root: &Path) -> Result<()>`

Core install logic. Runs all install phases through manifest writing. Calls `find_hooks_binary()` to locate `amplihack-hooks` before wiring hooks.

### `find_hooks_binary() -> Result<PathBuf>`

Locates the `amplihack-hooks` binary using a 5-step resolution:

1. `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` environment variable (must point to an existing executable)
2. Sibling of the currently running `amplihack` executable
3. `PATH` lookup via `which amplihack-hooks`
4. `~/.local/bin/amplihack-hooks`
5. `~/.cargo/bin/amplihack-hooks`

PATH runs at Step 3 so system-wide installs (e.g. a tarball to `/usr/local/bin`) survive the uninstall→reinstall cycle, since `amplihack uninstall` only removes the `~/.local/bin` copies.

Returns an actionable error if none of the five locations yield an executable.

### `deploy_binaries() -> Result<Vec<PathBuf>>`

Copies `amplihack` (current executable) and `amplihack-hooks` (resolved by `find_hooks_binary`) to `~/.local/bin` with `0o755` permissions. When a sibling `amplihack-asset-resolver` binary is present, it is deployed too so launched tools and recipe runs can resolve bundle assets without falling back to Python. Returns the list of deployed paths for inclusion in the manifest.

After deployment, install analyzes ordered `PATH` candidates for `amplihack` and
`amplihack-hooks`. It warns when a system-managed candidate such as
`/usr/local/bin/amplihack` shadows the freshly deployed `~/.local/bin` binary or
when the two binaries resolve from different install roots. The analysis is
side-effect free; install never removes or rewrites system-managed binaries.

> **macOS note:** On macOS with System Integrity Protection (SIP) active, copying the running executable to `~/.local/bin` may produce a quarantined binary. See the [First-time install how-to](../howto/first-install.md#macos-sip-note) for the resolution step.

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

- [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md) — optional `mmdc` install for local mermaid rendering in `pr-guide`

- [Interactive Install](../howto/interactive-install.md) — guided setup wizard walkthrough
- [Repair install/update PATH conflicts](../howto/repair-install-update-path-conflicts.md) — repair stale system binaries that shadow `~/.local/bin`
- [Install/update PATH conflict reference](./install-update-path-conflicts.md) — target selection and resolver API
- [Hook Specifications](./hook-specifications.md) — the 7 hooks registered by amplihack install
- [Install Manifest](./install-manifest.md) — manifest schema
- [Binary Resolution](./binary-resolution.md) — find_hooks_binary lookup detail
- [Framework bundle compatibility](./framework-bundle-compatibility.md) — smart-orchestrator compatibility contract and stale bundle repair
