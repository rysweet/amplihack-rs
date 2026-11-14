---
title: Recipe run bundle selection and pinning
description: How amplihack recipe run chooses one amplifier-bundle, checks it, pins a content-keyed copy under the work root and hands it to every step.
doc_type: reference
last_updated: 2026-11-13
related_issues: [2528, 1551, 2340, 2537, 1741]
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
is refused (E7). Any other failure to open a file, such as a file with mode
`0000`, is reported as `cannot open <path>` with the system error.

Ancestors of the bundle root (for example `$HOME` or `/d0`) are not checked.
See [Known limits](#known-limits).

### Symlinks inside the bundle

| Link | Result |
|------|--------|
| Relative, target inside the bundle, target passes the per-entry check | Recreated in the snapshot with the same link text |
| Absolute target | Refused (E7) |
| Target outside the bundle, including a `../` escape or a sibling directory whose name shares a prefix | Refused (E7) |
| Link text that climbs above the bundle root when read as text, even if it resolves back inside (for example surplus `..` steps followed by the bundle's own absolute path) | Refused (E7) |
| Dangling, with relative link text that stays inside the bundle | Left out of the snapshot, as `amplihack install` leaves it out of the installed bundle. The repository's own bundle has two such links, `skills/outside-in-testing/scripts` and `skills/outside-in-testing/tests` |
| Cyclic | Refused (E7) |
| Link to a directory | Recreated as a link; the copy never descends through it |

Containment is tested twice. The canonical target must lie inside the
bundle, component by component, so `amplifier-bundle-old/` does not count as
inside `amplifier-bundle/`. The link text itself, read from the link's folder
without following any symlink, must also never climb above the bundle root.
The text is copied verbatim, so text that only resolves inside the source
bundle would, in the snapshot, point back at the source.

The two merge-guard names that auto-drive skips (`gh-merge-guard`,
`autodrive_merge_gate.sh`) are skipped here too.

## The snapshot

An executing run uses a checked, read-only copy of the selected bundle. The
copy lives under the work root, in a folder named by a digest of the bundle's
contents, so every launch of the same bundle gets the same path:

```
<work root>/                         # 0700
├── runs/<run id>/                   # the run's own folder (unchanged)
└── bundle-pins/                     # 0700, checked at every launch
    ├── .lock                        # 0600; held while a launch pins or sweeps
    ├── .tmp-<uuid>/                 # a copy in progress
    ├── <key>/                       # 0555; AMPLIHACK_HOME and AMPLIHACK_RUN_BUNDLE
    │   ├── pin.json                 # 0444; source, version and choice
    │   └── amplifier-bundle/        # read-only copy of the selected bundle
    │       ├── recipes/
    │       ├── agents/
    │       ├── tools/
    │       └── ...
    └── <key>.use                    # 0600; shared lock held by each run using <key>
```

`<work root>` is the folder described in
[Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md)
(`AMPLIHACK_WORK_ROOT`, by default `$HOME/.amplihack/work`). A dry run has no
work root and no snapshot; see [Dry runs](#dry-runs).

### The key

`<key>` is the first 24 lower-case hex digits of a SHA-256 digest over these
parts, in this order:

| Part | Value |
|------|-------|
| Format tag | `v1` |
| Choice | `installed` or `checkout` |
| Source | Canonical source bundle path |
| Version | See [Bundle version](#bundle-version) |
| Tree digest | The 32-byte tree digest of the fresh copy, described below |

Each part is preceded by its length in bytes as an 8-byte big-endian integer,
so no two different sets of parts give the same input.

Because the choice and the source are part of the key, a snapshot of the
installed bundle never serves a checkout run, and the reverse. Because the
tree digest is part of the key, a bundle whose contents change gets a new key
and a new path.

### The tree digest

The tree digest is a SHA-256 digest of the copy, computed after
`copy_bundle` has checked and copied every entry. It covers only the
`amplifier-bundle/` folder (`.tmp-<uuid>/amplifier-bundle` for a fresh copy,
`<key>/amplifier-bundle` for an existing snapshot), and paths are relative to
that folder. `pin.json` and the `<key>/` folder itself are not part of it, so
writing or reading `pin.json` never changes the digest. The walk:

- visits entries in byte order of their names, depth first;
- never follows a symlink, and opens files with `O_NOFOLLOW`;
- feeds the hasher, for each entry, the relative path (length-prefixed), a
  type tag (`d`, `f` or `l`) and the mode with the write bits cleared
  (`mode & 0o7555`, as a 4-byte big-endian integer);
- then, for a file, its length as an 8-byte big-endian integer followed by
  its bytes, streamed; for a symlink, the link text, length-prefixed;
- fails on any other type of entry.

The write bits are left out so that a fresh copy and a read-only snapshot of
the same bundle give the same digest. `copy_bundle` sets every directory and
executable file of a fresh copy to `0755` and every other file to `0644`; the
snapshot holds `0555` and `0444`. Masked with `0o7555`, both give `0555` and
`0444`. A mask of `0o7777` would make every existing snapshot differ from a
fresh copy, so step 6 would replace or refuse (E11) every reuse. The match
therefore depends on `copy_bundle` keeping this normalisation: if it ever
copies source modes through, the mask must change with it.

### Modes

| Property | Value |
|----------|-------|
| `bundle-pins/` | `0700`, owned by the effective user; checked with `trusted_directory` at every launch |
| `<key>/` | `0555`, so no step can add or change a file in a snapshot that concurrent runs share |
| Directories under `amplifier-bundle/` | `0555` |
| Files with any execute bit in the source (`mode & 0o111 != 0`) | `0555` |
| Other files | `0444` |
| `pin.json` | `0444` |
| `.lock`, `<key>.use` | `0600`, regular files owned by the effective user, opened with `O_NOFOLLOW` and `O_CLOEXEC` |
| setuid, setgid, sticky bits | Dropped |
| Size | The size of the bundle, about 13 MB for a release |

The modes are set explicitly, so the snapshot passes the trust checks whatever
the process umask. The whole tree sits under the `0700` work root, so other
users cannot read it.

### How a launch pins the bundle

A launch that does not reuse an inherited snapshot pins the bundle as follows.
Every lock is a blocking or non-blocking `flock`. No timer or retry is
involved.

1. Check `bundle-pins/` with `trusted_directory`: a real directory, owned by
   the effective user, no group or other write bit. It is created `0700` if
   missing and checked even when it already exists.
2. Try to take `.lock` exclusively without blocking. If another run holds
   it (to pin, to release its lease or to sweep), print one line to stderr
   and then wait for it:

   ```
   recipe bundle: waiting for /home/alice/.amplihack/work/bundle-pins/.lock (another run holds the bundle snapshot lock)
   ```

3. Remove any `.tmp-<uuid>` folder left by a launch that was killed.
4. Check and copy the source into `.tmp-<uuid>/amplifier-bundle` with
   `copy_bundle`. `<uuid>` is a random UUID v4 in lower-case hyphenated form,
   generated by this launch; it is not the run id. This runs every [trust check](#trust-checks) on the source
   (E6, E7) at every launch, whether or not a snapshot already exists.
5. Compute the tree digest of the copy and the key, and write `pin.json`.
6. Compare with `<key>/`:

   | State of `<key>/` | Action |
   |-------------------|--------|
   | Missing | Make the whole copy read-only, its top folder and `pin.json` included, and rename `.tmp-<uuid>` to `<key>` |
   | Present, holds only `amplifier-bundle/` and `pin.json`, and its tree digest equals the copy's | Reuse it: clear any write bit anywhere in `<key>/` again, then remove the copy |
   | Present, anything else (differs) and no other run holds `<key>.use` | Restore the owner write bit on the real directories of the old `<key>/` (symlinks are never followed or changed), remove it, make the copy read-only, rename the copy to `<key>` and print `recipe bundle: replacing <path>: its contents differ from <source> (<version>)` |
   | Present, differs, another run holds `<key>.use` | Refuse the run (E11) |

   The whole `<key>/` folder is checked before it is reused. It must hold
   exactly a real `amplifier-bundle/` directory and a regular `pin.json`,
   neither of them a symlink. Steps read files directly under
   `AMPLIHACK_HOME`, such as `config` or `.claude/`, so a snapshot holding
   anything else counts as changed, even when the bundle's digest matches.
   The digest of the existing `amplifier-bundle/` is always computed again
   from its files. The contents of `pin.json` are never used to decide. A
   `<key>/` that cannot be read or digested counts as changed.

   The digest leaves out write bits, so a snapshot made writable since it
   was placed still matches. Reuse therefore makes the whole `<key>/`
   read-only again. Removal, by a sweep or by the last run's release,
   restores the owner write bit on the real directories first.

   Whether another run holds `<key>.use` is tested by opening it without
   creating it and taking a non-blocking exclusive `flock`. If the lock is
   granted, no run holds it; if it would block, another run does. A missing
   `<key>.use` counts as not held. The probe lock is kept while the old
   folder is removed and the copy renamed, then released. `.lock` is held
   throughout, so no launch or nested run can take a lease in between.
7. Open `<key>.use`, creating it `0600` if missing, and take a shared lock on
   it. The run keeps this lock until it ends; see [Cleanup](#cleanup).
8. Sweep the other keys, testing each `<key>.use` with the same
   non-blocking exclusive `flock` as step 6:

   | State | Action |
   |-------|--------|
   | `<key>/` present, `<key>.use` not held or missing | Restore the owner write bit on its real directories, remove `<key>/`, then remove `<key>.use` if present |
   | `<key>/` present, `<key>.use` held | Leave both |
   | `<key>/` missing, `<key>.use` not held | Remove `<key>.use`; it was left by a run whose folder is already gone |
   | `<key>/` missing, `<key>.use` held | Leave it; the run holding it removes it when it ends |

   The sweep never touches this launch's own `<key>` and `<key>.use`.
9. Release `.lock`.

No child process, network call or terminal read happens while `.lock` is
held. Any `flock` error other than "would block" (for example `ENOLCK` or
`EOPNOTSUPP`) refuses the run (E12), so a run never continues without its
lock. The runner and its steps do not inherit the lock descriptors.

The sweep and the start-up clean-up act only on names that match these
patterns, and never follow a symlink. A symlink with one of these names is
unlinked; its target is left alone. Any other name is left alone.

| Pattern | Meaning |
|---------|---------|
| `^[0-9a-f]{24}$` | A snapshot |
| `^[0-9a-f]{24}\.use$` | A snapshot's lease file |
| `^\.tmp-<uuid v4>$` | A copy in progress |
| `.lock` | The launch lock |

### Sharing between runs

Runs of the same bundle share one snapshot, each holding a shared lock on its
`<key>.use`. This is safe because the snapshot is read-only and its contents
are fixed by its key. Runs of different bundles, including the runs before
and after an `amplihack update`, get different keys and different snapshots,
so an upgrade never changes the bundle of a run that has already started.

### Order of operations in `recipe run`

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
8. Pin the bundle as described in
   [How a launch pins the bundle](#how-a-launch-pins-the-bundle) and print
   the stderr line. A dry run walks and checks every entry without copying.
9. Point the recipe path at the same file inside the snapshot. A recipe given
   as a path outside the bundle keeps its own path. A bundle recipe that is
   not inside the selected source is an error, never a fallback to the
   source. Read the recipe's name again from the file that will run; if it
   differs from the name read in step 3, refuse the run (E10).
10. For auto-drive, call `trusted::LaunchBundle::prepare(pinned.home())`.
11. Settle `AMPLIHACK_STATE_HOME` (E8).
12. Call `recovery::admit` with the canonical source recipe path, not the
    snapshot path, so a recipe keeps the same recovery identity from one run
    to the next (F5).
13. Set the [step and runner environment](#step-and-runner-environment) and
    start `recipe-runner-rs`.
14. When the run ends, finish the run workspace, then release the snapshot
    lease ([Cleanup](#cleanup)).

A retry of the runner within the same `recipe run` reuses the snapshot.

### Resumed runs

The runner's recovery store (#1741) records, for each run, settings that
include `AMPLIHACK_HOME`, the `-R` and `--agent-dir` paths and the canonical
paths of the sub-recipes it loaded. A later launch resumes a stopped run only
when these match.

A relaunch with an unchanged bundle computes the same key, so it gets the
same snapshot path and the same runner arguments, and the run resumes. The
snapshot is rebuilt, or reused after its digest is checked, before the runner
starts, so every sub-recipe file that the recovery store names exists again.

A relaunch after the bundle has changed gets a new key. The recovery store
then reports the input as changed and the run starts again from the first
step. This is deliberate: a resumed run never mixes results from one bundle
with steps from another.

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
| `-R` | `<work root>/bundle-pins/<key>/amplifier-bundle/recipes` | Executing run |
| `-R` | `<source bundle>/recipes` | Dry run |
| `-R` | `<launch copy>/amplifier-bundle/recipes` | Auto-drive run (see [Auto-drive](#auto-drive)) |
| `-R` | Parent directory of the recipe file | Only when the recipe was given as a path, and not for auto-drive |
| `--agent-dir` | `<work root>/bundle-pins/<key>/amplifier-bundle/agents` | Executing run |
| `--agent-dir` | `<source bundle>/agents` | Dry run |
| `--agent-dir` | `<launch copy>/amplifier-bundle/agents` | Auto-drive run |
| `--no-default-search` | | Always, auto-drive included |

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
| `AMPLIHACK_HOME` | `<work root>/bundle-pins/<key>`; for auto-drive, the launch copy | Recipes and tools resolve `${AMPLIHACK_HOME}/amplifier-bundle/...` inside the snapshot or the copy made from it |
| `AMPLIHACK_RUN_BUNDLE` | `<work root>/bundle-pins/<key>`, auto-drive included | Marks the run's bundle for [nested runs](#nested-runs) |
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

`recipe run` refuses the run (E8) when the settled state home lies under the
run's `runtime/` folder or anywhere under `<work root>/bundle-pins/`,
including under another run's snapshot. That happens, for example, when a
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
| `AMPLIHACK_RUN_BUNDLE` is set, passes the marker checks below, and the nested run gives no `--bundle` or `-c bundle=` | Reuses the outer snapshot and takes a shared lease on it; no selection, no copy, no removal |
| Marker passes and the nested run asks for a choice other than `AMPLIHACK_RUN_BUNDLE_CHOICE` | Refused (E3) |
| Marker passes and the nested run asks for the choice in `AMPLIHACK_RUN_BUNDLE_CHOICE` | Reuses the outer snapshot |
| Marker passes, but `<key>.use` or `bundle-pins/.lock` is missing | Refused (E13) |
| Marker fails any check | Ignored with a note on stderr; the nested run selects and pins normally |

The marker checks, all on canonical paths:

1. `AMPLIHACK_RUN_BUNDLE` names an existing directory.
2. It equals `AMPLIHACK_HOME`, or `AMPLIHACK_HOME` is an auto-drive launch
   copy made from it: a directory that passes the directory check and whose
   `bundle-source` file names the marker.
3. Its parent is `<work root>/bundle-pins` of this run's own work root, and
   its name is a key: exactly 24 lower-case hex digits
   (`^[0-9a-f]{24}$`). A nested run inherits `AMPLIHACK_WORK_ROOT`, so its
   work root is its parent's.
4. The marker directory and its `amplifier-bundle/` pass the directory check.
5. `AMPLIHACK_RUN_BUNDLE_CHOICE` is exactly `installed` or `checkout`.

Check 3 ties the marker to the folder whose lock files the nested run is
about to open. A marker under some other `bundle-pins` folder, for example
one under a different work root, fails the check.

When the marker passes, the nested run takes the existing `.lock` and opens
the existing `<key>.use`, creating neither, takes a shared lock on
`<key>.use` and releases `.lock`. If either file is missing, the outer run has ended or the folder was
tampered with, and the nested run is refused (E13) rather than run from a
snapshot that no lease protects. A nested run never copies, removes or
changes the mode of anything under `bundle-pins/`.

`AMPLIHACK_RUN_BUNDLE_CHOICE` decides only which request a nested run treats
as a conflict. Changing it in a step never changes which bundle a nested run
uses.

The launch that creates a snapshot writes `<key>/pin.json`, holding its
`source`, `version` and `choice`. The record of a nested run that reuses the
snapshot takes `source` and `version` from that file and sets `inherited` to
`true`. The file is read without following a symlink, at most 64 KiB is read,
and the version is sanitised again. It is used only for the record and the
stderr line, never to decide anything. When the file is missing, unreadable
or malformed, `source` is the snapshot's `amplifier-bundle`, `version` is
`unknown`, and a note on stderr names the file and the reason.

When the marker fails a check and `AMPLIHACK_HOME` names the same directory,
that `AMPLIHACK_HOME` is another run's snapshot, not an install, so it is not
tried as an installed candidate; a second note says so.

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
| `snapshot` | string | `<work root>/bundle-pins/<key>`; absent on a dry run |
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
    "snapshot": "/home/alice/.amplihack/work/bundle-pins/9c41e07a2b5d83f6a0e1c4d7",
    "choice": "installed",
    "inherited": false
  },
  "tool_bundle": {
    "source": "/home/alice/.amplihack/releases/1.2.151/amplifier-bundle",
    "version": "1.2.151",
    "snapshot": "/home/alice/.amplihack/work/bundle-pins/9c41e07a2b5d83f6a0e1c4d7",
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
run selects and pins at its own start, and holds its own lease on the
snapshot. The detaching command makes no copy.

## Cleanup

Each run that uses a snapshot holds a shared lock on `<key>.use` through a
lease. The lease is released after the run workspace is finished, on every
exit path. The last run to release a snapshot removes it.

When a lease created by a launch (not a nested run) is released:

1. Take `.lock` exclusively.
2. Close the run's own shared lock on `<key>.use`.
3. Try to take `<key>.use` exclusively without blocking.
   - If that succeeds, no other run uses the snapshot. Restore the owner
     write bit on the real directories of the tree (symlinks are never
     followed or changed), remove `<key>/` and remove `<key>.use`.
   - If it would block, another run still uses the snapshot. Leave it.
4. Release `.lock`.

If the removal fails, the run prints one line naming the path and continues
to exit as it would have:

```
recipe bundle: could not remove /home/alice/.amplihack/work/bundle-pins/9c41e07a2b5d83f6a0e1c4d7; a later launch removes it
```

A nested run's lease only releases its shared lock. It never removes the
snapshot.

| How the run ends | What removes the snapshot |
|------------------|---------------------------|
| Success | The run's lease, if it is the last user |
| Step failure, error or early return with `?` | The run's lease, if it is the last user |
| Panic (unwind) | The run's lease, on drop, if it is the last user |
| Other runs of the same bundle still running | The last of them to end |
| `SIGKILL` | The sweep of the next launch on the same work root (step 8 of [How a launch pins the bundle](#how-a-launch-pins-the-bundle)); a relaunch of the same bundle instead reuses the folder after checking its digest |

The kernel releases a killed run's locks, so a folder left by `SIGKILL` is
never mistaken for one in use.

No process, thread or timer is started for cleanup. There is no reaper
subcommand. When `recipe run` returns, no process it started for cleanup is
still running.

## Auto-drive

`trusted::LaunchBundle::prepare` is called with `pinned.home()`, the run's
snapshot directory, after the pin (step 10 of the order of operations),
instead of reading `AMPLIHACK_HOME` again. Its YAML rewrite and policy digest are
unchanged. `pin_bundle` accepts a plain directory (no release link) under the
same ownership and mode checks, which is the form a snapshot takes.

An auto-drive run uses the launch copy the way any other run uses the
snapshot:

- The runner gets `-R <launch copy>/amplifier-bundle/recipes`,
  `--agent-dir <launch copy>/amplifier-bundle/agents` and
  `--no-default-search`.
- Steps get `AMPLIHACK_HOME=<launch copy>`, as before, and the other three
  variables as in any executing run. `AMPLIHACK_RUN_BUNDLE` names the
  snapshot, not the launch copy, because the copy is not under
  `bundle-pins/`.
- The launch copy holds `bundle-source` (mode `0600`), naming the canonical
  home it was copied from. Marker check 2 accepts the launch copy through
  that file, so a nested `recipe run` in an auto-drive step reuses the
  snapshot and the outer run's choice instead of copying the launch copy as
  an installed bundle.

## Errors

| Code | When | Message |
|------|------|---------|
| E1 | Flag and context key differ | `--bundle installed conflicts with -c bundle=checkout; give one value` |
| E2 | Context key has another value | `-c bundle=Checkout is not valid; use installed or checkout` |
| E3 | Nested run asks for a different bundle | `this run is nested in a run pinned to the installed bundle at <snapshot>; --bundle checkout cannot change it` |
| E4 | No installed bundle | `no installed amplifier-bundle found; searched: <path> (git checkout, skipped), <path>, <path>. To use this checkout's bundle, pass --bundle checkout` |
| E5 | No checkout bundle | `--bundle checkout: no directory containing both .git and amplifier-bundle/ above <working dir>` |
| E6 | Ownership or mode check fails | `untrusted bundle entry <path>: mode 0664 (group or other writable); fix with chmod go-w <path>` or `untrusted bundle entry <path>: owned by uid 1001, not the current user` |
| E7 | Symlink refused, entry changed during copy, or unsupported file type | `untrusted bundle entry <path>: <reason>`, where the reason is `absolute symlink`, `symlink leaves the bundle`, `symlink cycle`, `changed while being copied` or `not a regular file, directory or symlink` |
| E8 | Settled state home lies under the run's `runtime/` folder or under `<work root>/bundle-pins/`, compared after resolving symlinks (a path not yet created resolves its nearest existing ancestor) | `AMPLIHACK_STATE_HOME <path> lies under a run's runtime/ folder` or `AMPLIHACK_STATE_HOME <path> lies under the bundle snapshots in <work root>/bundle-pins` |
| E9 | Bare recipe name not found or outside the bundle | `recipe 'default-workflow' not found in the installed bundle at <path>; to use this checkout's recipes, pass --bundle checkout` or `recipe '<name>' resolves outside <recipes dir>` |
| E10 | The recipe copied into the snapshot has a different name from the one read before the pin | `recipe changed while the bundle was being pinned: <path> names "<a>", but "<b>" was read before the pin` |
| E11 | An existing snapshot with the same key has different contents and another run holds its `<key>.use` (a non-blocking exclusive `flock` on it would block; a missing `<key>.use` never gives E11) | `bundle snapshot <path> is in use by another run and its contents differ from <source>; wait for that run to end` |
| E12 | `bundle-pins/` fails the directory check, a lock file is not a regular file owned by the effective user, or a lock call fails for any reason other than "would block" | `untrusted bundle snapshot directory <path>: <reason>` or `cannot lock <path>: <system error>` |
| E13 | A nested run's marker passes its checks but `<key>.use` or `bundle-pins/.lock` does not exist | `inherited bundle snapshot <path> has no lease file <path>.use; the run that pinned it has ended` or `inherited bundle snapshot <path> has no lock file <path>/.lock; the run that pinned it has ended` |

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

/// The bundle a run executes with. Owns the run's lease on the snapshot;
/// dropping it releases the lease and, for the last user, removes the
/// snapshot (see Cleanup). Keep it alive until the run workspace is finished.
pub(crate) struct PinnedBundle { /* home, bundle, source, record, lease */ }

impl PinnedBundle {
    /// <work root>/bundle-pins/<key> (AMPLIHACK_HOME for steps)
    pub(crate) fn home(&self) -> &Path;
    /// <work root>/bundle-pins/<key>/amplifier-bundle
    pub(crate) fn bundle(&self) -> &Path;
    /// Canonical source bundle path
    pub(crate) fn source(&self) -> &Path;
    /// Log record for recipe_bundle and tool_bundle
    pub(crate) fn record(&self) -> &BundleRecord;
}

/// Pins the selection under <work_root>/bundle-pins/<key>, reusing a
/// snapshot with the same contents, or takes a lease on an inherited
/// snapshot. With dry_run, walks and checks every entry without copying;
/// work_root is then unused.
pub(crate) fn pin(selection: &Selection, work_root: &Path, dry_run: bool)
    -> anyhow::Result<PinnedBundle>;

/// True only for <work root>/bundle-pins/<24 lower-case hex digits>.
pub(crate) fn is_snapshot_path(path: &Path, work_root: &Path) -> bool;

/// E8: refuses a state home under runtime_dir or under
/// <work root>/bundle-pins/.
pub(crate) fn check_state_home(
    state_home: &Path,
    pinned: &PinnedBundle,
    runtime_dir: Option<&Path>,
) -> anyhow::Result<()>;
```

`tree_digest` (the walk described in [The tree digest](#the-tree-digest))
and `PinLease` (the shared lock on `<key>.use`) are private to the module.
Lock calls use `fs4::FileExt`, called by its full path because the standard
library's `File` has methods with the same names.

In `crates/amplihack-cli/src/commands/cleanup/run_dir.rs`:

```rust
impl RunWorkspace {
    /// The work root that holds runs/ and bundle-pins/.
    /// None for the scratch space of a dry run.
    pub(crate) fn work_root(&self) -> Option<&Path>;
}
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
- **Other users.** The work root and `bundle-pins/` are `0700`, so steps that
  switch to another user cannot read the snapshot.
- **Group-writable installs.** An install made under umask `002` has
  group-writable files and is refused (E6) until fixed with
  `chmod -R go-w`.
- **A stopped launch holds `.lock`.** A launch that is stopped (for example
  by `SIGSTOP` or `SIGTTIN`) while it holds `bundle-pins/.lock` makes every
  later launch on the same work root wait. The waiting launch names the lock
  file on stderr. There is no timeout; resume or end the stopped process.
- **Concurrent launches wait for each other.** Launches on one work root take
  `.lock` in turn, each for the time of one copy and two tree digests of the
  bundle.
- **A changed bundle restarts a stopped run.** A resume after the bundle has
  changed, for example after `amplihack update`, gets a new snapshot path, so
  the run starts again from its first step.
- **Disk use after `SIGKILL`.** A snapshot left by a killed run stays on disk
  until the next launch on the same work root sweeps it.
- **Auto-drive resume.** `trusted::LaunchBundle::prepare` makes a new launch
  copy for each auto-drive run, so an auto-drive run's runner paths still
  differ from one launch to the next, and a stopped auto-drive run does not
  resume from where it stopped.

## Related

- [One bundle per recipe run](../concepts/one-bundle-per-recipe-run.md): why runs pin a snapshot
- [Run a recipe from a checkout's bundle](../howto/run-a-recipe-from-a-checkout-bundle.md): task steps
- [amplihack recipe reference](./recipe-command.md#recipe-run): all `recipe run` flags
- [Recipe executor environment](./recipe-executor-environment.md): the full runner environment
- [Versioned install layout](../concepts/versioned-install-layout.md): releases and the bundle link
- [Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md): the work root, run directories and the end-of-run sweep
