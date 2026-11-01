# Clean Up After Recipe Runs

Use this guide to find what a recipe run leaves on disk, to choose where it
goes, and to remove or archive it with `amplihack cleanup`.

Every `amplihack recipe run` now writes into one work root. At the end of each
run the runner removes the run's temporary folder and clears out earlier runs
whose pull request was merged or closed more than a day ago. The
`amplihack cleanup` command does the same on demand, and also archives old
logs and removes temporary folders left by killed runs.

## Contents

- [Quick start](#quick-start)
- [Settings](#settings)
- [Where a run writes](#where-a-run-writes)
- [What happens when a run ends](#what-happens-when-a-run-ends)
- [When a run is eligible for cleanup](#when-a-run-is-eligible-for-cleanup)
- [Preview and apply cleanup](#preview-and-apply-cleanup)
- [Cleanup categories](#cleanup-categories)
- [Build output: CACHEDIR.TAG and target names](#build-output-cachedirtag-and-target-names)
- [Archive and restore logs](#archive-and-restore-logs)
- [Move the work root to another disk](#move-the-work-root-to-another-disk)
- [Safety rules](#safety-rules)
- [Command reference](#command-reference)
- [Troubleshooting](#troubleshooting)
- [Related documentation](#related-documentation)

## Quick start

See what cleanup would do. A dry run writes nothing, not even the work root:

```bash
amplihack cleanup
```

Remove what is eligible and archive logs older than 30 days:

```bash
amplihack cleanup --apply
```

Remove only stale temporary folders, without touching logs:

```bash
amplihack cleanup --apply --only run-tmp --only tmp-leftover
```

No configuration is needed for the end-of-run cleanup. It runs after every
`amplihack recipe run` unless `AMPLIHACK_CLEANUP_ON_EXIT=0` is set.

## Settings

Each setting can be given as a command-line flag (where one exists), an
environment variable, or left at its default. A flag overrides the variable,
and the variable overrides the default. There are no `amplihack config` keys
for these settings.

| Variable | Flag | Default | Meaning |
| --- | --- | --- | --- |
| `AMPLIHACK_WORK_ROOT` | none | `$HOME/.amplihack/work` | Folder that holds run folders, targets, clones, the archive and, when set explicitly, worktrees. |
| `AMPLIHACK_CLEANUP_KEEP_HOURS` | `--keep-hours N` | `24` | How long run-owned worktrees and targets are kept after a run's PR closes or the run ends, and how old untagged targets and system temporary leftovers must be. Does not apply to `run-tmp`. Maximum `87600`. |
| `AMPLIHACK_LOG_ARCHIVE_DAYS` | `--log-days N` | `30` | Recipe logs and session logs whose modification time is older than this are archived. Maximum `36500`. |
| `AMPLIHACK_LOG_ARCHIVE_DIR` | none | `<work root>/archive` | Where log archives and `MANIFEST.jsonl` are written. |
| `AMPLIHACK_CLEANUP_ON_EXIT` | none | `1` | Set to `0` to skip the end-of-run sweep. The run's `runtime/` folder is still removed. |

Recipe logs stay in `recipe.log_dir` (default `~/.amplihack/recipe-logs`).
The work root does not change where they are written.

The work root must be an absolute path. It is refused if it is `/`, your home
folder, an ancestor of your home folder, a system folder such as `/usr` or
`/etc`, the system temporary folder, or the root of a git checkout. If the
folder already exists it must be owned by you and must not be writable by
group or others. New folders under the work root are created with mode `0700`.

## Where a run writes

Each `amplihack recipe run` invocation has a **run id**, a UUID created once
when the command starts. The run folder is named after it, and steps see it
as `AMPLIHACK_RUN_ID`.

The run id is not the same as the **attempt id**. The runner retries an
attempt that fails with a transient fault, such as a 529 response or a reset
connection, and gives each attempt its own UUID. The attempt id is what
`AMPLIHACK_RECIPE_RUN_ID` holds and what `run_id` in the JSON result of
`amplihack recipe run --format json` reports, as in earlier releases. A run
with no retry has one attempt; `run.json` lists every attempt id of the run
under `attempts[].correlation_id`.

The runner creates one folder per run:

```text
~/.amplihack/work/
├── runs/
│   └── 3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/
│       ├── run.json              # the run report (kept)
│       ├── owned-paths           # worktrees and targets this run created
│       ├── cleanup-record.json   # written only when cleanup removed something
│       └── runtime/              # removed when the run ends
│           ├── tmp/              # TMPDIR, TMP and TEMP for every step
│           └── artifacts/        # AMPLIHACK_WORKFLOW_ARTIFACT_DIR
├── targets/                      # cargo targets from git-hook-cargo.sh
├── clones/                       # repositories cloned by the launcher inside a run (never swept)
├── worktrees/                    # only when AMPLIHACK_WORK_ROOT is set
├── archive/                      # log archives and MANIFEST.jsonl
└── .cleanup.lock
```

Every step of the run sees these variables:

| Variable | Value |
| --- | --- |
| `AMPLIHACK_WORK_ROOT` | The resolved work root. |
| `AMPLIHACK_WORK_ROOT_EXPLICIT` | `1` or `0`; see below. The worktree helpers read this, so a work root that the runner exported by default does not move worktrees. |
| `AMPLIHACK_RUN_ID` | The run id. |
| `AMPLIHACK_RECIPE_RUN_ID` | The id of the current attempt (unchanged from earlier releases). |
| `AMPLIHACK_RUN_DIR` | `<work root>/runs/<run id>` |
| `AMPLIHACK_WORKFLOW_RUNTIME_DIR` | `<run dir>/runtime` |
| `AMPLIHACK_RUNTIME_ROOT` | `<run dir>/runtime` |
| `AMPLIHACK_WORKFLOW_ARTIFACT_DIR` | `<run dir>/runtime/artifacts` |
| `TMPDIR`, `TMP`, `TEMP` | `<run dir>/runtime/tmp` |

`AMPLIHACK_WORK_ROOT_EXPLICIT` is set as follows:

- If it is already set in the environment that starts the run, the runner
  passes it through unchanged. A nested run therefore keeps its parent's `0`,
  even though the parent exported `AMPLIHACK_WORK_ROOT`.
- Otherwise it is `1` when `AMPLIHACK_WORK_ROOT` is set in that environment,
  and `0` when it is not.

A sub-recipe started by a step gets its own run folder, its own run id and its
own temporary folder.

If `<run dir>/runtime/tmp` would be longer than 80 bytes, which can happen
with a long home path, the runtime folder is placed at
`<work root>/r/<first 12 hex digits of the run id>` instead. This leaves room
under the 108-byte limit on Unix socket paths for tools that create sockets in
`TMPDIR`.

### The run report

`run.json` is written when the run starts and finalised when it ends. It is
mode `0600`. It holds only status data, never environment variables, prompts
or step output:

```json
{
  "schema_version": 1,
  "run_id": "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3",
  "recipe": "default-workflow",
  "pid": 418822,
  "pid_start_ticks": 91833412,
  "cwd": "/home/ryan/src/amplihack-rs",
  "started_at": "2026-11-01T09:12:44Z",
  "ended_at": "2026-11-01T10:03:19Z",
  "outcome": "success",
  "attempts": [
    { "correlation_id": "b81d47e0-2a9c-4f13-8e65-7c0f19d3a2e4" },
    { "correlation_id": "e4a2c6f1-58b7-4d0e-9a31-6f2b8c7d0e95" }
  ],
  "steps": [{ "id": "step-01-prepare-workspace", "status": "completed" }],
  "log_path": "/home/ryan/.amplihack/recipe-logs/e4a2c6f1-58b7-4d0e-9a31-6f2b8c7d0e95.log"
}
```

This run had one transient retry, so it lists two attempt ids; each is an
independent UUID. `log_path` is the log of the last attempt.

`outcome` is `success`, `failure` or `interrupted`. A run that was killed
before it could finalise the report keeps `ended_at` empty and reads as
interrupted. For cleanup, such a run is dated by the modification time of
`run.json`, which the runner updates at the start of each attempt and each
step.

To find the run folder from the JSON result of `amplihack recipe run`, which
reports the attempt id, search the run reports for that attempt:

```bash
ATTEMPT_ID="$(jq -r '.run_id' result.json)"
WORK_ROOT="${AMPLIHACK_WORK_ROOT:-$HOME/.amplihack/work}"
grep -l "\"$ATTEMPT_ID\"" "$WORK_ROOT"/runs/*/run.json | xargs -r dirname
# /home/ryan/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3
```

Run folders themselves are never removed. After cleanup a folder holds only
`run.json`, `owned-paths` and `cleanup-record.json`, which together take a few
kilobytes. Pruning old run folders is not part of this feature.

### Recording owned paths

Cleanup removes a worktree or target only if the run that created it recorded
it in `owned-paths`. The bundled helpers do this for you:

- `workflow_worktree_root.sh` and the recipes that create worktrees through it
  (`workflow-worktree.yaml`, `consensus-issue-worktree.yaml` and
  `autodrive_pr_worktree.sh`) call `amplihack_run_owned.sh record` after a new
  `git worktree add`.
- `scripts/git-hook-cargo.sh` records the target it creates.

Each line has three tab-separated fields, `kind`, `path` and `branch`. The
branch field is empty for a target, so the helpers write a target line that
ends with a tab. A line with only two fields is also accepted and read as
having an empty branch, which is the form shown for the target below:

```text
worktree	/home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-1808	feat/issue-1808
target	/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b-run-3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3
```

A run with no recorded branch has no pull request to look up and follows the
no-PR rule in
[When a run is eligible for cleanup](#when-a-run-is-eligible-for-cleanup).

A custom step that creates a worktree can record it the same way:

```bash
"$AMPLIHACK_HOME/amplifier-bundle/tools/amplihack_run_owned.sh" record \
  worktree "$WORKTREE_PATH" "$BRANCH" \
  || echo "warning: could not record $WORKTREE_PATH for cleanup" >&2
```

A worktree created without recording it is never removed by cleanup. That is
the safe outcome; remove it yourself with `git worktree remove` when you no
longer need it.

### Worktree location

By default, worktrees stay where they were: `<main checkout>/worktrees/<branch>`.

When `AMPLIHACK_WORK_ROOT` is set in the environment that starts the run,
worktrees go to
`<work root>/worktrees/<repository name>-<8-character hash of the main checkout path>/<branch>`
instead. The hash keeps two checkouts with the same name apart. Inside a run
the helpers decide this from `AMPLIHACK_WORK_ROOT_EXPLICIT`, not from whether
`AMPLIHACK_WORK_ROOT` is set. To print the base folder that the helpers will
use:

```bash
"$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_worktree_root.sh" base
# with AMPLIHACK_WORK_ROOT_EXPLICIT=1:
# /home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d
# with AMPLIHACK_WORK_ROOT_EXPLICIT=0 or unset:
# /home/ryan/src/amplihack-rs/worktrees
```

`workflow_worktree_root.sh base` is the only source of this folder. These
helpers use it:

- `workflow-worktree.yaml` (used by `default-workflow`);
- `autodrive_pr_worktree.sh`;
- `consensus-issue-worktree.yaml`, unless the caller passes `worktree_dir`,
  which still wins;
- `workflow_branch_name.sh`, so that a re-run of a task that names its branch
  recognises its own worktree under the work root and reuses it.

The default work root (`$HOME/.amplihack/work`) applies to run folders,
targets, clones and the archive whether or not the variable is set. Only
worktrees need the variable set explicitly.

## What happens when a run ends

When `amplihack recipe run` finishes, whether the run succeeded, failed or
panicked, the runner:

1. Removes `<run dir>/runtime/`, including everything steps wrote to
   `TMPDIR` and `artifacts/`.
2. Finalises `run.json` with the end time, outcome and step summary.
3. Unless `AMPLIHACK_CLEANUP_ON_EXIT=0` is set, sweeps the whole work root for
   run-owned items that are now eligible (the categories `run-tmp`,
   `run-worktree` and `run-target`), and removes them. This includes items
   from earlier runs, and from the current run if it is already eligible.

The sweep is best effort. If another cleanup holds the lock, the sweep is
skipped. It stops after 120 seconds. Any problem is printed as a warning and
never changes the run's exit status. Untagged targets, log archiving and
scans of the system temporary folder are left to `amplihack cleanup`, so a
run never removes the pre-commit hook caches of your other checkouts.

When the sweep removes something from a run, it adds an entry to that run's
`cleanup-record.json`:

```json
{
  "run_id": "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3",
  "sweeps": [
    {
      "cleaned_at": "2026-11-02T11:20:05Z",
      "removed": [
        {
          "kind": "run-worktree",
          "path": "/home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-1808",
          "size_bytes": 1843200
        },
        {
          "kind": "run-target",
          "path": "/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b-run-3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3",
          "size_bytes": 4294967296
        }
      ],
      "skipped": []
    }
  ]
}
```

If nothing was removed, no record is written.

If the work root cannot be used, for example because it is group-writable,
the runner prints a warning and falls back to a private temporary folder for
`runtime/`, as earlier releases did. No run folder or cleanup happens for that
run.

## When a run is eligible for cleanup

Cleanup never touches an **active** run. A run is active when the process id
recorded in `run.json` is alive and its start time matches the recorded
`pid_start_ticks`. The start time check stops a reused process id from making
a dead run look alive. If `/proc` cannot be read, the run is treated as active.

An inactive run is **eligible** in either of these cases:

- Its pull request is merged or closed, and at least the keep period
  (`AMPLIHACK_CLEANUP_KEEP_HOURS`, default 24) has passed since `closedAt`.
- It has no pull request, or no recorded branch, and at least the keep period
  has passed since the later of `ended_at` and the modification time of
  `run.json`. A killed run with an empty `ended_at` is dated by the
  modification time of `run.json` alone.

The pull request is found with:

```bash
gh pr list --head=<branch> --state all --json number,state,closedAt,mergedAt
```

run inside the worktree itself, using the branch recorded for it in
`owned-paths`. Each worktree gets its own verdict, so a run with one merged
and one open pull request loses only the merged worktree. The lookup runs only
in a worktree that has already passed the containment and linked-worktree
checks, never in a folder named only by `run.json`, and `gh` is started with
the same git hardening as cleanup's own git commands. Each branch is looked up
once per sweep, and each lookup has a 30-second limit.

A run's targets are removed only when every worktree the run still has is
eligible. If a recorded worktree could not be checked, the targets are kept.
When none of the run's worktrees is left, the targets follow the no-PR rule
above.

A run is **skipped**, with a reason in the output, when:

- the keep period has not yet passed;
- its pull request is still open;
- `gh` is missing, fails, returns output that cannot be parsed, or takes longer
  than 30 seconds;
- `run.json` has a `schema_version` newer than this release understands;
- its `owned-paths` file exists but cannot be read (reported as
  `owned-paths unreadable: ...`). Untagged targets are then also skipped,
  since that run may own them.

For example, with the default keep period, a run whose PR closed one hour ago
is skipped with the reason `within keep period`, and a run whose PR closed 25
hours ago has its worktree and target removed.

## Preview and apply cleanup

A dry run lists every candidate and what would happen to it:

```bash
amplihack cleanup
```

```text
CATEGORY        ACTION                         SIZE     PATH
run-target      would-remove                   4.0 GiB  /home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b-run-3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3
                reason: PR #1820 merged 2026-10-31T08:02:11Z
run-worktree    would-remove                   1.8 MiB  /home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-1808
                reason: PR #1820 merged 2026-10-31T08:02:11Z
run-worktree    skipped                        612 KiB  /home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-1829
                reason: PR #1831 is open
recipe-log      would-archive                  88 KiB   /home/ryan/.amplihack/recipe-logs/0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f.log
                reason: modified 41 days ago
session-log     skipped                        2.1 MiB  /home/ryan/.claude/projects/-home-ryan-src-amplihack-rs/5c1f....jsonl
                reason: awaiting sessions archive (#1766)
```

The dry run reads only. It does not create the work root, the archive folder
or the lock file.

Apply the plan:

```bash
amplihack cleanup --apply
```

`--apply` takes an exclusive lock on `<work root>/.cleanup.lock` and checks
each item again just before removing it. Actions in the output change to
`removed` and `archived`. When an item is skipped, the action is always
`skipped` and the explanation is in `reason`.

Get machine-readable output:

```bash
amplihack cleanup --format json
```

```json
{
  "dry_run": true,
  "work_root": "/home/ryan/.amplihack/work",
  "entries": [
    {
      "category": "run-target",
      "path": "/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b-run-3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3",
      "size_bytes": 4294967296,
      "reason": "PR #1820 merged 2026-10-31T08:02:11Z",
      "action": "would-remove",
      "run_id": "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3"
    }
  ]
}
```

Each entry has the same six keys: `category`, `path`, `size_bytes` (an
integer, or `null` when the size could not be counted), `reason`, `action`
and `run_id` (the run id of the run folder the item belongs to, or `null`).
Entries are sorted by category, then by path, so
two runs over the same state give the same output.

Running `--apply` a second time reports no candidates and changes nothing on
disk.

Other examples:

```bash
# Keep run output for a week instead of a day
amplihack cleanup --apply --keep-hours 168

# Archive logs older than 7 days and do nothing else
amplihack cleanup --apply --only recipe-log --only session-log --log-days 7

# Remove run output but leave every log alone
amplihack cleanup --apply --skip-logs
```

## Cleanup categories

| Category | What it collects | Action |
| --- | --- | --- |
| `run-tmp` | `<work root>/runs/<id>/runtime` (or `<work root>/r/<short id>`) left by a run that is no longer active, such as one that was killed. The keep period does not apply: the folder is removed as soon as the run is inactive. | remove |
| `run-worktree` | Worktrees recorded in a run's `owned-paths`, under `<work root>/worktrees/` or `<main checkout>/worktrees/`, once the run is eligible. | `git worktree remove`; the branch is kept |
| `run-target` | Targets recorded in a run's `owned-paths`, once the run is eligible. | remove |
| `untagged-target` | Folders directly under `<work root>/targets/`, and `amplihack-precommit-target-*` folders in the hook cache base, that no run recorded and that have not been modified within the keep period. | remove |
| `tmp-leftover` | Top-level `replay-*`, `release-*` and `amplihack-workflow-*` folders in the system temporary folder that you own, that are older than the keep period, and that no running process has as its working folder or holds a file open in. | remove |
| `recipe-log` | Files in `recipe.log_dir` older than `--log-days`. | archive |
| `session-log` | Files in `${CODEX_HOME:-~/.codex}/sessions`, including its nested date folders, older than `--log-days`. Claude sessions under `~/.claude/projects` are listed but skipped with `awaiting sessions archive (#1766)`. | archive |

`amplihack-workflow-*` folders were the per-run temporary folders of earlier
releases. Runs now use `<run dir>/runtime` instead, so new ones no longer
appear. `replay-*` and `release-*` come from tools outside amplihack; cleanup
only collects them.

The `tmp-leftover` category is skipped entirely when cleanup runs as root, and
when the system temporary folder is neither owned by you nor sticky.

Clones under `<work root>/clones/` belong to no category. They are never
recorded in `owned-paths` and never swept; remove them by hand when you no
longer need them.

## Build output: CACHEDIR.TAG and target names

Every target folder that amplihack creates contains a `CACHEDIR.TAG` file, so
backup tools that honour the
[Cache Directory Tagging Specification](https://bford.info/cachedir/) skip it:

```text
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by amplihack.
# For information about cache directory tags see https://bford.info/cachedir/
```

The tag is written by:

- `scripts/git-hook-cargo.sh`, for the pre-commit and pre-push target;
- `scripts/install-pinned-recipe-runner.sh` and `scripts/dev-space.sh`, when
  they create a target folder;
- the Rust helper `create_tagged_target_dir`.

`git-hook-cargo.sh` chooses its folder like this:

| Condition | Folder |
| --- | --- |
| `AMPLIHACK_WORK_ROOT` usable, inside a run | `$AMPLIHACK_WORK_ROOT/targets/amplihack-precommit-target-<hash>-run-<run id>` |
| `AMPLIHACK_WORK_ROOT` usable, outside a run | `$AMPLIHACK_WORK_ROOT/targets/amplihack-precommit-target-<hash>` |
| `AMPLIHACK_WORK_ROOT` not set, or not usable | the XDG cache path used by earlier releases, with the same naming |

The hook uses the work root only when it already exists as a real folder (not
a symbolic link) owned by you, is not group- or world-writable, and is neither
`/` nor your home folder. It never creates the work root, and it creates
`targets/` with mode `0700`. If it cannot record a target in `owned-paths`,
it prints `amplihack: warning: could not record cache ownership for <path>`
and the build goes ahead.

The `-run-<run id>` suffix uses `AMPLIHACK_RUN_ID`, not the attempt id, and
is added only when that variable is set and matches `^[0-9a-f-]{1,64}$`, the
same pattern cleanup requires of every id it reads from a folder name. Inside a run, the script also records the
target in `owned-paths`. Its stale-cache reclaim recognises both name forms
and considers only folders you own.

The runner does not set `CARGO_TARGET_DIR`. A worktree's own `target/` stays
inside the worktree, Cargo tags it, and it is removed with the worktree.

To find the build output of a run, look for its id:

```bash
ls -d ~/.amplihack/work/targets/*-run-3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3
```

## Archive and restore logs

`amplihack cleanup --apply` archives each recipe log and Codex session log
older than `--log-days` into its own `.tar.gz` file under
`AMPLIHACK_LOG_ARCHIVE_DIR`, then removes the original. Archives keep the
log's path relative to its log folder:

| Log | Archive path |
| --- | --- |
| `<recipe.log_dir>/<name>` | `<archive dir>/recipe-logs/<name>.tar.gz` |
| `${CODEX_HOME:-~/.codex}/sessions/<relative path>` | `<archive dir>/codex-sessions/<relative path>.tar.gz` |

For example, `~/.codex/sessions/2026/09/12/rollout-2026-09-12T08-14-03-7c2e.jsonl`
is archived as
`~/.amplihack/work/archive/codex-sessions/2026/09/12/rollout-2026-09-12T08-14-03-7c2e.jsonl.tar.gz`.

For each file, cleanup:

1. computes the SHA-256 digest;
2. writes the archive to a `.partial` file with mode `0600`, flushes it to
   disk and renames it;
3. reads the archive back, decompresses it and compares the bytes with the
   original;
4. appends a line to `MANIFEST.jsonl`;
5. checks that the original's size and modification time have not changed;
6. removes the original.

If any step fails, the original is kept and the entry is reported as failed.
No log is deleted unless its archive has been verified.

A manifest line looks like this:

```json
{"original_path":"/home/ryan/.amplihack/recipe-logs/0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f.log","archive_path":"/home/ryan/.amplihack/work/archive/recipe-logs/0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f.log.tar.gz","size_bytes":90112,"digest":"sha256:4f1c...","archived_at":"2026-11-01T10:05:40Z"}
```

The archive folder has no `CACHEDIR.TAG`, so backup tools still include it.
It must not be inside a log folder, and no log folder may be inside it.

### Restore a log

Restore one archive to its original path:

```bash
amplihack cleanup restore \
  ~/.amplihack/work/archive/recipe-logs/0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f.log.tar.gz
```

Restore every file listed in a manifest:

```bash
amplihack cleanup restore ~/.amplihack/work/archive/MANIFEST.jsonl
```

Restore into another folder instead of the original paths:

```bash
amplihack cleanup restore ~/.amplihack/work/archive/MANIFEST.jsonl \
  --dest ~/restored-logs
```

The destination path always comes from a manifest line, never from the
archive. When given an archive, restore reads `MANIFEST.jsonl` in the archive
folder (`AMPLIHACK_LOG_ARCHIVE_DIR`, the root that holds `recipe-logs/` and
`codex-sessions/`) and uses the line whose `archive_path` is that archive. If
no line matches, the archive is not restored; the command reports
`no manifest entry for <archive>` and exits with code 1.

Restore never overwrites a file. It writes to a temporary file in the
destination folder and moves it into place with an operation that fails if
the destination exists, so a file created at the same moment is not replaced
either. If the destination already exists, the conflict is reported, the
other files are still restored, and the command exits with code 1. The
destination must lie under a log folder or under `--dest`. Restored files are
checked against the recorded digest and written with mode `0600`.

Restore refuses, and writes nothing, when:

- a folder between the log folder (or `--dest`) and the destination is a
  symbolic link, or resolves outside it;
- `--dest` or an archive path in the manifest contains `.` or `..`;
- the archive holds anything other than one regular file, or more bytes than
  the manifest records.

## Move the work root to another disk

To put run folders, targets, clones and worktrees on a data disk:

```bash
install -d -m 700 /data/amplihack-work
export AMPLIHACK_WORK_ROOT=/data/amplihack-work

amplihack recipe run default-workflow \
  -c task_description="Fix flaky login tests" \
  -c repo_path=.
```

With the variable set, nothing new is created under `~/.amplihack/work`.
Existing items under the old root are not moved; run
`AMPLIHACK_WORK_ROOT=$HOME/.amplihack/work amplihack cleanup --apply` to clear
them as they become eligible.

Inside a run, the launcher clones repositories into
`<work root>/clones/<repository name>-<8-character hash>`. Outside a run,
clones go where they did before. Cleanup does not remove clones. The folder
name depends only on the URL, so a later run that checks out the same
repository reuses the clone and fetches it. If the folder is not a git
checkout of that URL, the launch is refused with a message naming the folder.

## Safety rules

Cleanup deletes only items inside the folders it manages, at the depth it
expects:

- `<work root>/runs/<id>/runtime` and `<work root>/r/<short id>`;
- recorded worktrees under `<work root>/worktrees/` or
  `<main checkout>/worktrees/`;
- folders directly under `<work root>/targets/`, and
  `amplihack-precommit-target-*` folders in the hook cache base;
- the `tmp-leftover` folders described above;
- log originals, after their archive is verified.

An entry in `owned-paths` is a request, not a permission. A recorded path
outside these folders is reported as skipped.

A worktree is removed only if all of these hold:

- it is a linked worktree, never the main checkout;
- it has no uncommitted changes and no unpushed commits;
- no running process has its working folder inside it;
- the run that recorded it is not active;
- `git worktree remove` succeeds without `--force`.

Git commands against a candidate run with hooks and fsmonitor disabled and
with the system git configuration ignored, so a worktree's configuration
cannot run code during cleanup.

Before each removal, cleanup resolves the path again and requires it to
resolve to the same place it did when the item was collected, and still to lie
inside the folders above. It then removes that resolved path, never the path a
step wrote. It also refuses symbolic links and repeats the liveness, Cargo
lock and cleanliness checks; if the run's `run.json` can no longer be read,
the item is skipped. An item that fails one of these checks is reported as
`skipped` with the reason. Symbolic links are not followed when counting
sizes.

If a run's `cleanup-record.json` cannot be parsed, it is renamed to
`cleanup-record.json.corrupt-<time>`, a warning is printed, and a new record
is started. When every path in `owned-paths` has been removed, the file is
removed too.

Cleanup never touches other files in the work root, such as driver scripts or
task files you keep there, clones under `<work root>/clones/`, your home
folder, other runs' folders, or the main checkout. It never runs
`git worktree prune`, which would also drop the entries of other runs'
worktrees on missing or unmounted paths.

## Command reference

```text
amplihack cleanup [--apply] [--format text|json] [--keep-hours N]
                  [--log-days N] [--only <category>]... [--skip-logs]
amplihack cleanup restore <manifest-or-archive-path> [--dest <dir>]
```

| Option | Meaning |
| --- | --- |
| `--apply` | Remove and archive. Without it the command is a dry run. |
| `--format text\|json` | Output format. Default `text`. |
| `--keep-hours N` | Overrides `AMPLIHACK_CLEANUP_KEEP_HOURS`. |
| `--log-days N` | Overrides `AMPLIHACK_LOG_ARCHIVE_DAYS`. |
| `--only <category>` | Limit to one category. Repeat for several. Cannot be combined with `--skip-logs`. |
| `--skip-logs` | Leave `recipe-log` and `session-log` alone. |

Actions in the output are `would-remove`, `removed`, `would-archive`,
`archived`, `skipped` and `failed`. The explanation for each action is in
`reason`.

| Exit code | Meaning |
| --- | --- |
| `0` | Success, including when items were skipped. |
| `1` | At least one apply or restore action failed, or a fatal error occurred. The remaining items are still processed. Errors are printed as `amplihack cleanup: <message>`. |
| `2` | Bad usage, such as an unknown category or `--only` with `--skip-logs`. |

`amplihack hygiene cleanup` is unchanged. Its 48-hour minimum, categories and
output stay as described in [Hygiene Cleanup](../operations/hygiene-cleanup.md).

## Troubleshooting

**`work root ... is writable by group or others`.** An existing
`~/.amplihack/work` created with a permissive umask is refused. Fix it with:

```bash
chmod 700 ~/.amplihack/work
```

Until then, runs fall back to a private temporary folder and print a warning.

**A worktree is always skipped with `not recorded in owned-paths`.** The
worktree was created outside the bundled helpers, or by a release before this
feature. Record it with `amplihack_run_owned.sh record`, or remove it yourself.

**Everything is skipped with `gh failed`.** Check that `gh auth status` works
in the worktree's repository. Cleanup does not guess when it cannot read the
PR state.

**Skipped with `cleanup lock held`.** Another `amplihack cleanup --apply` or an
end-of-run sweep is running. Wait for it to finish and run the command again.

**A tool cannot create a socket in `TMPDIR`.** Check the length of
`$TMPDIR`. Runs switch to the shorter `<work root>/r/<short id>` layout when
the default path is over 80 bytes; a very long `AMPLIHACK_WORK_ROOT` can still
exceed the socket limit, so choose a shorter one.

## Related documentation

- [Hygiene Cleanup](../operations/hygiene-cleanup.md) - Opt-in cleanup of stale worktrees, detached Cargo targets and session artifacts outside recipe runs
- [Configure Workflow Runtime Isolation](configure-workflow-runtime-isolation.md) - Where workflow runtime output is written
- [Correlate Recipe Runs with Logs](correlate-recipe-runs.md) - Match a run id to its logs and processes
