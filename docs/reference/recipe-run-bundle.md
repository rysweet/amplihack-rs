---
title: Recipe run bundle selection and pinning
description: How amplihack recipe run chooses one amplifier-bundle, checks it, copies it into the run and hands it to every step.
doc_type: reference
last_updated: 2026-11-13
related_issues: [2528, 1551, 2340, 2537]
---

# Recipe run bundle selection and pinning

Each `amplihack recipe run` uses one `amplifier-bundle` for the recipe, its
sub-recipes, its agents and its tools. This page lists the selection rules,
trust checks, snapshot layout, environment, log fields and errors. For the
reasons behind the design, see
[One bundle per recipe run](../concepts/one-bundle-per-recipe-run.md). For
day-to-day tasks, see
[Run a recipe from a checkout's bundle](../howto/run-a-recipe-from-a-checkout-bundle.md).

## Contents

- [Choosing the bundle](#choosing-the-bundle)
- [Where each choice looks](#where-each-choice-looks)
- [Trust checks](#trust-checks)
- [The snapshot](#the-snapshot)
- [Resolving the recipe argument](#resolving-the-recipe-argument)
- [Runner arguments](#runner-arguments)
- [Step and runner environment](#step-and-runner-environment)
- [Nested runs](#nested-runs)
- [Bundle version](#bundle-version)
- [Log record and stderr line](#log-record-and-stderr-line)
- [Dry runs](#dry-runs)
- [Detached runs](#detached-runs)
- [Cleanup](#cleanup)
- [Auto-drive](#auto-drive)
- [Errors](#errors)
- [Rust interface](#rust-interface)
- [Known limits](#known-limits)

## Choosing the bundle

| Input | Values | Default |
|-------|--------|---------|
| `--bundle <CHOICE>` | `installed`, `checkout` | `installed` |
| `-c bundle=<CHOICE>` | `installed`, `checkout` (exact, lower case) | none |

- The flag and the context key mean the same thing. Use either.
- If both are given and differ, the run is refused (E1).
- Any other value for `-c bundle=` is refused (E2). The flag rejects other
  values through its argument parser.
- The `bundle` key is removed from the context before the context is exported
  to steps, so no step sees `$BUNDLE`.
- The key is lower case because context keys that start with `AMPLIHACK_` are
  reserved and refused.

`recipe list`, `recipe show` and `recipe validate` are not affected. They keep
the search path described in
[Recipe search path](./recipe-command.md#recipe-search-path).

## Where each choice looks

### `installed`

The candidates are tried in this order:

1. `AMPLIHACK_HOME` from the environment.
2. `paths.home` from the user configuration.
3. `$HOME/.amplihack`.

For each candidate, in order:

- If it is unset or does not exist, it is passed over.
- If its canonical path contains a `.git` entry (a directory or a file), it is
  a checkout. It is skipped, and the run prints one note to stderr:

  ```
  recipe bundle: skipping AMPLIHACK_HOME=/d0/src/amplihack-rs: it is a git checkout; pass --bundle checkout to use its bundle
  ```

  The note names the source of the candidate (`AMPLIHACK_HOME`, `paths.home`
  or `$HOME/.amplihack`).
- If it contains an `amplifier-bundle/` entry, it is the source home and the
  search stops.

The installed choice never walks up from the working directory, so a
checkout's bundle is used only with `--bundle checkout`, even when the run
starts inside the checkout and `AMPLIHACK_HOME` points at it.

If no candidate qualifies, the run fails with E4. E4 lists every candidate
tried, and marks each skipped checkout with `(git checkout, skipped)`.

### `checkout`

The source home is the nearest ancestor of the run's working directory
(`--working-dir`, otherwise the current directory) that contains both a `.git`
entry and an `amplifier-bundle/` entry. The `.git` entry may be a directory or
a file (a linked worktree); a `.git` file is never parsed.

If no ancestor qualifies, the run fails with E5.

Arbitrary bundle paths are not accepted. To use a bundle elsewhere, start the
run from inside the checkout that holds it, or set `AMPLIHACK_HOME` and use
the installed choice.

### Symlinked bundles

When `<home>/amplifier-bundle` is a symlink (the
[versioned install layout](../concepts/versioned-install-layout.md)), it is
resolved once, during selection. The rest of the run uses that canonical path.
Re-pointing the symlink later, for example by `amplihack update`, does not
change the bundle of a run that has already started.

## Trust checks

The same checks apply to both choices. They are the checks auto-drive uses
(#1551, #2340), and live in
`crates/amplihack-cli/src/commands/orch/autodrive/trusted.rs`.

| Check | Applies to | Rule |
|-------|------------|------|
| Release link | `<home>/amplifier-bundle` when it is a symlink | Target is inside `<home>/releases/`, the release has `.complete`, and the releases directory and release directory pass the directory check |
| Directory check (`trusted_directory`) | Canonical bundle root | Real directory, owned by the effective user, mode has no group or other write bit (`mode & 0o022 == 0`) |
| Per-entry check (`copy_bundle`) | Every file, directory and symlink in the bundle | Owned by the effective user, no group or other write bit |
| Symlinks inside the bundle | Every symlink entry | See [Symlinks inside the bundle](#symlinks-inside-the-bundle) |
| Entry type | Every entry | Regular file, directory or kept symlink; anything else (FIFO, socket, device) is refused |

A failed check ends the run before any step starts. There is no fallback to
another bundle.

Each entry is checked and copied in one pass. Files are opened with
`O_NOFOLLOW`, and the device and inode of the open handle must match the
metadata that was checked, so a file swapped between the check and the copy
is refused (E7).

Ancestors of the bundle root (for example `$HOME` or `/d0`) are not checked.
See [Known limits](#known-limits).

### Symlinks inside the bundle

| Link | Result |
|------|--------|
| Relative, target inside the bundle, target passes the per-entry check | Recreated in the snapshot with the same link text |
| Absolute target | Refused (E7) |
| Target outside the bundle, including a `../` escape or a sibling directory whose name shares a prefix | Refused (E7) |
| Dangling or cyclic | Refused (E7) |
| Link to a directory | Recreated as a link; the copy never descends through it |

Containment is tested on canonical paths, component by component, so
`amplifier-bundle-old/` does not count as inside `amplifier-bundle/`.

The two merge-guard names that auto-drive skips (`gh-merge-guard`,
`autodrive_merge_gate.sh`) are skipped here too.

## The snapshot

An executing run copies the selected bundle into its own workspace:

```
<run dir>/
└── runtime/                    # 0700, removed when the run ends
    └── bundle/                 # 0755; AMPLIHACK_HOME and AMPLIHACK_RUN_BUNDLE
        └── amplifier-bundle/   # copy of the selected bundle
            ├── recipes/
            ├── agents/
            ├── tools/
            └── ...
```

When `<run dir>/runtime/tmp` would be longer than 80 bytes, the runtime
folder is `<work root>/r/<first 12 hex digits of the run id>` instead, and the
snapshot is `<that>/bundle`. `<run dir>` is `<work root>/runs/<run id>`, described in
[Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md).

| Property | Value |
|----------|-------|
| Directory mode | `0755` |
| File mode, when the source file has any execute bit (`mode & 0o111 != 0`) | `0755` |
| Other file mode | `0644` |
| setuid, setgid, sticky bits | Dropped |
| Size | The size of the bundle, about 13 MB for a release |
| Copies per run tree | One, made by the outermost run |

The modes are set explicitly, so the snapshot passes the trust checks whatever
the process umask.

Order of operations in `recipe run`:

1. Merge `--bundle` and `-c bundle=` (E1, E2) and remove the `bundle` key
   from the context.
2. Check for an inherited snapshot ([Nested runs](#nested-runs)). If there is
   none, select the bundle and run the trust checks on the source (E3 to E7).
3. Resolve the recipe argument against the source's `recipes/` directory
   (E9) and parse the recipe.
4. Run the existing pre-flight checks and the capacity governor admission.
5. Apply the recursion-depth guard (#964).
6. Probe the runner binary.
7. Create the run workspace. A dry run creates only private scratch space.
8. Copy the bundle into `runtime/bundle/amplifier-bundle` and print the
   stderr line. A dry run walks and checks every entry without copying.
9. Point the recipe path at the same file inside the snapshot. A recipe given
   as a path outside the bundle keeps its own path.
10. For auto-drive, call `trusted::LaunchBundle::prepare(pinned.home())`.
11. Settle `AMPLIHACK_STATE_HOME` (E8).
12. Call `recovery::admit` with the canonical source recipe path, not the
    snapshot path, so a recipe keeps the same recovery identity from one run
    to the next (F5).
13. Set the [step and runner environment](#step-and-runner-environment) and
    start `recipe-runner-rs`.

A retry of the runner within the same `recipe run` reuses the snapshot.

## Resolving the recipe argument

| Argument form | Example | Recipe file loaded from | Sub-recipes, agents and tools from |
|---------------|---------|-------------------------|------------------------------------|
| Bare name | `default-workflow` | `<bundle>/recipes/default-workflow.yaml` only | The selected bundle |
| Path | `./my-recipe.yaml`, `recipes/x.yml`, `../a/b` | The given path | The selected bundle |

An argument is treated as a path when it contains `/`, `\`, `..` or a NUL
byte, or ends in `.yaml` or `.yml`. Everything else is a bare name.

For a bare name:

- Only the selected bundle's `recipes/` directory is searched. The
  repository root, `.claude/recipes/` and `~/.amplihack/.claude/recipes/` are
  not searched.
- The canonical result must lie inside that `recipes/` directory. A recipe
  that is a symlink to a file outside it is refused (E9).
- If the name is not found, the run fails with E9, which names the bundle and
  the choice.

For a path, the log record keeps the file path in `recipe_file`, alongside the
bundle that supplied the tools.

## Runner arguments

`recipe run` passes these to `recipe-runner-rs`:

| Argument | Value | Condition |
|----------|-------|-----------|
| `-R` | `<run dir>/runtime/bundle/amplifier-bundle/recipes` | Executing run |
| `-R` | `<source bundle>/recipes` | Dry run |
| `-R` | Parent directory of the recipe file | Only when the recipe was given as a path |
| `--agent-dir` | `<run dir>/runtime/bundle/amplifier-bundle/agents` | Executing run |
| `--agent-dir` | `<source bundle>/agents` | Dry run |
| `--no-default-search` | | Always |

A dry run has no snapshot, so it points the runner at the canonical source
bundle. See [Dry runs](#dry-runs).

With default search off, a sub-recipe or agent that is missing from the
selected bundle fails the step. It is never taken from another bundle on the
host.

Agent references in bundled recipes use the `amplihack:<group>:<name>` form,
which the snapshot's `agents/` directory resolves.

## Step and runner environment

An executing `recipe run` sets these on the runner process. The runner passes
them to every bash and agent step. A dry run sets none of them; see
[Dry runs](#dry-runs).

| Variable | Value | Purpose |
|----------|-------|---------|
| `AMPLIHACK_HOME` | `<run dir>/runtime/bundle` | Recipes and tools resolve `${AMPLIHACK_HOME}/amplifier-bundle/...` inside the snapshot |
| `AMPLIHACK_RUN_BUNDLE` | `<run dir>/runtime/bundle` | Marks the run's bundle for [nested runs](#nested-runs) |
| `AMPLIHACK_RUN_BUNDLE_CHOICE` | `installed` or `checkout` | The choice the outermost run made; a nested run compares its own request with it (E3) |
| `AMPLIHACK_STATE_HOME` | See [State home](#state-home) | Keeps hook, session and contract state out of the snapshot |

All four names are reserved. A `-c` key with any of these names is refused,
as are all `AMPLIHACK_*` keys.

The retired name `AMPLIHACK_BUNDLE_DIR` is not used.

### State home

The value of `AMPLIHACK_STATE_HOME` is settled as follows:

| Run | Value |
|-----|-------|
| Outermost run | `amplihack_types::paths::amplihack_state_root()`, evaluated before `AMPLIHACK_HOME` is changed. That reads an inherited `AMPLIHACK_STATE_HOME` first, if it is usable |
| Nested run that reuses the outer snapshot | The inherited `AMPLIHACK_STATE_HOME`, passed through unchanged. It is not computed again, because `AMPLIHACK_HOME` now names the snapshot |

A value is usable when it is an absolute path with no `..` component.
`amplihack_state_root()` and the launch contract reader
(`launch/contract_file.rs`) read `AMPLIHACK_STATE_HOME` first, and fall back
to their usual rules, without a message, when it is unset or not usable.
Commands outside `recipe run` do not set it and behave as before.

`recipe run` refuses the run (E8) in one case only: when the settled state
home lies under a run's `runtime/` folder. That happens, for example, when a
nested run finds the inherited value unset or not usable and falls back to a
root derived from the snapshot's `AMPLIHACK_HOME`.

### Tool lookups in bundled recipes

Bundled recipes look for tools under `${AMPLIHACK_HOME}/amplifier-bundle/`
first. In a pinned run that is the snapshot, so a recipe and the tools it
calls always come from the same bundle. For example, in
`workflow-design.yaml`:

```bash
if [ -n "${AMPLIHACK_HOME:-}" ]; then
  CONTEXT_HELPER="$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_context.sh"
else
  CONTEXT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_context.sh"
  # ... further fallbacks
fi
```

Later candidates are fallbacks for runs started without `recipe run`. A
pinned run never reaches them, because the snapshot is a full copy.

## Nested runs

A step that runs `amplihack recipe run` starts a nested run. It inherits the
outer run's environment.

| Condition | Result |
|-----------|--------|
| `AMPLIHACK_RUN_BUNDLE` is set, passes the marker checks below, and the nested run gives no `--bundle` or `-c bundle=` | Reuses the outer snapshot; no selection, no copy, no cleanup ownership |
| Marker passes and the nested run asks for a choice other than `AMPLIHACK_RUN_BUNDLE_CHOICE` | Refused (E3) |
| Marker passes and the nested run asks for the choice in `AMPLIHACK_RUN_BUNDLE_CHOICE` | Reuses the outer snapshot |
| Marker fails any check | Ignored with a note on stderr; the nested run selects and copies normally |

The marker checks, all on canonical paths:

1. `AMPLIHACK_RUN_BUNDLE` names an existing directory.
2. It equals `AMPLIHACK_HOME`.
3. It is a `bundle` folder inside a run's runtime folder: it ends in
   `runtime/bundle`, or in `r/<12 hex digits>/bundle` (the short runtime
   path used when `<run dir>/runtime/tmp` would be longer than 80 bytes).
4. The marker directory and its `amplifier-bundle/` pass the directory check.
5. `AMPLIHACK_RUN_BUNDLE_CHOICE` is exactly `installed` or `checkout`.

The marker is not required to lie under the runs directory. A step that can
set `AMPLIHACK_RUN_BUNDLE` can already set `AMPLIHACK_HOME`, so a further
location check would add no protection.

`AMPLIHACK_RUN_BUNDLE_CHOICE` decides only which request a nested run treats
as a conflict. Changing it in a step never changes which bundle a nested run
uses.

The outermost run writes `runtime/bundle/.bundle-record.json` beside the
snapshot, holding its `source`, `version` and `choice`. The record of a
nested run that reuses the snapshot takes `source` and `version` from that
file (at most 64 KiB is read, and the version is sanitised again), and sets
`inherited` to `true`. When the file is missing or unreadable, `source` is
the snapshot's `amplifier-bundle` and `version` is `unknown`.

When the marker fails a check and `AMPLIHACK_HOME` names the same directory,
that `AMPLIHACK_HOME` is another run's snapshot, not an install, so it is not
tried as an installed candidate; a second note says so.

Only the outermost run owns the snapshot and removes it.

## Bundle version

The version shown in the log record and on stderr is found as follows:

| Source | Version |
|--------|---------|
| The directory that contains the canonical bundle root has `.installed-by` with a `version=` line | That value |
| Checkout (`--bundle checkout`) with a `[workspace.package] version` in its root `Cargo.toml` | That value with `+checkout` appended, for example `1.2.151+checkout` |
| Anything else | `unknown` |

For a release symlink, the directory that contains the canonical bundle root
is the release root, so `releases/<version>/.installed-by` is read, not a
file in the home. For a plain directory it is the home itself.

Rules:

- At most 64 KiB of each file is read.
- The value must match `[0-9A-Za-z.+-]{1,64}`; otherwise it is `unknown`.
- No child process is started to find it.

## Log record and stderr line

The early and final records described in
[Recipe run correlation](./recipe-run-correlation.md) gain two fields,
`recipe_bundle` and `tool_bundle`. In a pinned run they are always equal.
Each is an object:

| Field | Type | Description |
|-------|------|-------------|
| `source` | string | Canonical source bundle path, before copying |
| `version` | string | See [Bundle version](#bundle-version) |
| `snapshot` | string | `<run dir>/runtime/bundle`; absent on a dry run |
| `choice` | string | `installed` or `checkout` |
| `inherited` | boolean | `true` when a nested run reused its parent's snapshot |
| `recipe_file` | string | The recipe file path, present only when the recipe was given as a path |

Example of an early record, formatted, with unrelated fields left out:

```json
{
  "schema_version": 1,
  "event": "early",
  "recipe_name": "default-workflow",
  "recipe_bundle": {
    "source": "/home/alice/.amplihack/releases/1.2.151/amplifier-bundle",
    "version": "1.2.151",
    "snapshot": "/home/alice/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/runtime/bundle",
    "choice": "installed",
    "inherited": false
  },
  "tool_bundle": {
    "source": "/home/alice/.amplihack/releases/1.2.151/amplifier-bundle",
    "version": "1.2.151",
    "snapshot": "/home/alice/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/runtime/bundle",
    "choice": "installed",
    "inherited": false
  }
}
```

The fields are additive. The record's schema version does not change.

At start, the run prints one line to stderr:

```
recipe bundle: /home/alice/.amplihack/releases/1.2.151/amplifier-bundle (1.2.151); tools: same bundle
```

A nested run that reuses its parent's snapshot prints the parent's source and
version followed by `(inherited)`.

Records, errors and the stderr line contain only paths, octal modes, the
sanitised version, the choice and booleans. No environment values are logged.

## Dry runs

`recipe run --dry-run` selects the bundle, runs every trust check and walks
every entry, so it reports the same E1 to E7 and E9 errors as a real run. It
makes no copy and creates no run workspace.

- The runner gets `-R <source bundle>/recipes` and
  `--agent-dir <source bundle>/agents`.
- `AMPLIHACK_HOME` is left as the run found it. `AMPLIHACK_RUN_BUNDLE`,
  `AMPLIHACK_RUN_BUNDLE_CHOICE` and `AMPLIHACK_STATE_HOME` are not set.
- The log record has no `snapshot` field, and the plan shows the source path.

## Detached runs

`recipe run --detach --bundle checkout` passes `--bundle checkout` to the run
in the tmux pane as two separate arguments after `--working-dir`. The pane's
run selects and copies at its own start, and its workspace owns the snapshot.
The detaching command makes no copy.

## Cleanup

The snapshot lives under `<run dir>/runtime/`, so it is removed with the rest
of `runtime/`:

| How the run ends | What removes the snapshot |
|------------------|---------------------------|
| Success | The run's `RuntimeGuard`, in the run's own exit path |
| Step failure or error | The run's `RuntimeGuard` |
| Panic (unwind) | The run's `RuntimeGuard`, on drop |
| `SIGKILL` | The end-of-run sweep of a later run, once no process uses the folder (`run-tmp` in [Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md)) |

No process, thread or timer is started for cleanup. There is no reaper
subcommand. When `recipe run` returns, no process it started for cleanup is
still running.

## Auto-drive

`trusted::LaunchBundle::prepare` is called with `pinned.home()`, the run's
snapshot directory, after the copy (step 11 of the order of operations),
instead of reading `AMPLIHACK_HOME` again. Its YAML rewrite and policy digest are
unchanged. `pin_bundle` accepts a plain directory (no release link) under the
same ownership and mode checks, which is the form a snapshot takes.

## Errors

| Code | When | Message |
|------|------|---------|
| E1 | Flag and context key differ | `--bundle installed conflicts with -c bundle=checkout; give one value` |
| E2 | Context key has another value | `-c bundle=Checkout is not valid; use installed or checkout` |
| E3 | Nested run asks for a different bundle | `this run is nested in a run pinned to the installed bundle at <snapshot>; --bundle checkout cannot change it` |
| E4 | No installed bundle | `no installed amplifier-bundle found; searched: <path> (git checkout, skipped), <path>, <path>. To use this checkout's bundle, pass --bundle checkout` |
| E5 | No checkout bundle | `--bundle checkout: no directory containing both .git and amplifier-bundle/ above <working dir>` |
| E6 | Ownership or mode check fails | `untrusted bundle entry <path>: mode 0664 (group or other writable); fix with chmod go-w <path>` or `untrusted bundle entry <path>: owned by uid 1001, not the current user` |
| E7 | Symlink refused, entry changed during copy, or unsupported file type | `untrusted bundle entry <path>: <reason>`, where the reason is `absolute symlink`, `symlink leaves the bundle`, `dangling symlink`, `symlink cycle`, `changed while being copied` or `not a regular file, directory or symlink` |
| E8 | Settled state home lies under a run's `runtime/` folder | `AMPLIHACK_STATE_HOME <path> lies under a run's runtime/ folder` |
| E9 | Bare recipe name not found or outside the bundle | `recipe 'default-workflow' not found in the installed bundle at <path>; to use this checkout's recipes, pass --bundle checkout` or `recipe '<name>' resolves outside <recipes dir>` |

Every error exits with status 1 before the runner starts. No step runs.

## Rust interface

The module is `crates/amplihack-cli/src/commands/recipe/run/bundle_pin.rs`.
It starts no process, thread or timer.

```rust
/// Which bundle a run uses.
pub(crate) enum BundleChoice { Installed, Checkout }

impl BundleChoice {
    /// Parses "installed" or "checkout"; anything else is an error (E2).
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self>;
    /// Merges the --bundle flag and the -c bundle key (E1, E2).
    /// Returns None when neither is given.
    pub(crate) fn from_inputs(flag: Option<&str>, key: Option<&str>)
        -> anyhow::Result<Option<Self>>;
    pub(crate) fn as_str(self) -> &'static str;
}

/// The checked source bundle, or an inherited snapshot.
pub(crate) struct Selection { /* source, choice, version, inherited */ }

impl Selection {
    /// <source>/amplifier-bundle/recipes
    pub(crate) fn recipes_dir(&self) -> &Path;
}

/// Reads the environment and calls select_from.
pub(crate) fn select(choice: Option<BundleChoice>, working_dir: &Path)
    -> anyhow::Result<Selection>;

/// Pure selection over explicit inputs; used by tests.
pub(crate) fn select_from(inputs: SelectInputs<'_>) -> anyhow::Result<Selection>;

/// The bundle a run executes with.
pub(crate) struct PinnedBundle { /* home, bundle, source, record */ }

impl PinnedBundle {
    /// <run dir>/runtime/bundle (AMPLIHACK_HOME for steps)
    pub(crate) fn home(&self) -> &Path;
    /// <run dir>/runtime/bundle/amplifier-bundle
    pub(crate) fn bundle(&self) -> &Path;
    /// Canonical source bundle path
    pub(crate) fn source(&self) -> &Path;
    /// Log record for recipe_bundle and tool_bundle
    pub(crate) fn record(&self) -> &BundleRecord;
}

/// Copies the selection into runtime/bundle, or reuses an inherited
/// snapshot. With dry_run, walks and checks every entry without copying.
pub(crate) fn pin(selection: &Selection, runtime_dir: &Path, dry_run: bool)
    -> anyhow::Result<PinnedBundle>;
```

In `trusted.rs`:

```rust
pub(crate) enum LinkPolicy {
    /// Skip every symlink (auto-drive's existing behaviour).
    Skip,
    /// Keep relative links whose targets stay inside the bundle.
    KeepInternal,
}

/// Checks and copies each entry in one pass.
pub(crate) fn copy_bundle(source: &Path, dest: &Path, links: LinkPolicy)
    -> anyhow::Result<()>;

/// Real directory, owned by the effective user, not group or other writable.
pub(crate) fn trusted_directory(path: &Path) -> anyhow::Result<()>;
```

`copy_tree` remains and calls `copy_bundle` with `LinkPolicy::Skip`, so
auto-drive copies the same entries as before. The execute-bit rule widens from
the owner bit to any execute bit; no file in a release has a group or other
execute bit without the owner bit, so auto-drive's copies keep the same modes.

In `crates/amplihack-cli/src/commands/recipe/resolve.rs`:

```rust
pub(crate) enum RecipeOrigin { Bundle, File(PathBuf) }

/// Bare names resolve only inside selection.recipes_dir();
/// path-like arguments load from the given path.
pub(crate) fn resolve_recipe_in_bundle(
    recipe: &str,
    selection: &Selection,
    working_dir: &Path,
) -> anyhow::Result<(PathBuf, RecipeOrigin)>;
```

## Known limits

- **Ancestor directories.** Directories above the bundle root are not
  checked. Once the snapshot exists, every read in the run goes through it;
  the source path is used afterwards only to identify the recipe for
  recovery.
- **Explicit recipe paths.** A recipe given as a path outside the bundle runs
  with the trusted bundle's tools, because the user named the file. The path
  is kept in `recipe_file` for audit.
- **Other users.** `runtime/` is `0700`, so steps that switch to another user
  cannot read the snapshot.
- **Group-writable installs.** An install made under umask `002` has
  group-writable files and is refused (E6) until fixed with
  `chmod -R go-w`.

## Related

- [One bundle per recipe run](../concepts/one-bundle-per-recipe-run.md): why runs pin a snapshot
- [Run a recipe from a checkout's bundle](../howto/run-a-recipe-from-a-checkout-bundle.md): task steps
- [amplihack recipe reference](./recipe-command.md#recipe-run): all `recipe run` flags
- [Recipe executor environment](./recipe-executor-environment.md): the full runner environment
- [Versioned install layout](../concepts/versioned-install-layout.md): releases and the bundle link
- [Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md): run directories and the end-of-run sweep
