---
title: Clean up after recipe runs
description: Locate, retain, remove, or archive run-owned files and recovery state.
doc_type: howto
last_updated: 2026-11-12
---

# Clean Up After Recipe Runs

Each real run removes its own execution checkout when it ends, after checking
the ownership manifest and the device and inode of every owned path. A path
that changed is kept and the run fails. Private repository metadata stays when
a durable worktree depends on it. See
[ownership and release](../reference/recipe-checkout-isolation.md#ownership-and-release).

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
- [Preserve or retire recovery state](#preserve-or-retire-recovery-state)
- [What happens when a run ends](#what-happens-when-a-run-ends)
- [When a run is eligible for cleanup](#when-a-run-is-eligible-for-cleanup)
- [When a target is removed](#when-a-target-is-removed)
- [Linked targets](#linked-targets)
- [Remove the target of a removed worktree](#remove-the-target-of-a-removed-worktree)
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
`amplihack recipe run` unless `AMPLIHACK_CLEANUP_ON_EXIT=0` is set. A
`recipe run --dry-run` still removes its temporary `amplihack-dry-*` folder
(`RunWorkspace::dry_runtime`) but never runs the sweep. A dry run creates no
`runs/` folder, so there is nothing else to remove.

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
| `AMPLIHACK_TARGET_LINK_ROOTS` | none | unset | Extra colon-separated absolute folders where linked targets may point. Not needed for links made by the host cargo wrapper. See [Linked targets](#linked-targets). |

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
│       ├── git-commands.jsonl    # index-writing git commands the run made (kept)
│       ├── git-trace2/           # raw git events; deleted once summarised
│       ├── recovery/             # private completions and required artifact copies
│       ├── procs/                # one record per running attempt, for recipe stop (Linux)
│       └── runtime/              # removed when the run ends
│           ├── checkout/         # the run's detached worktree (git runs only)
│           ├── tmp/              # TMPDIR, TMP and TEMP for every step
│           └── artifacts/        # AMPLIHACK_WORKFLOW_ARTIFACT_DIR
├── targets/                      # one cargo target per checkout, from git-hook-cargo.sh
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

Trusted delegated executions also attach to the enclosing
recovery tree.
Their completion records are published through the root runner's writer;
their own temporary folders are not recovery authority.

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

Run folders themselves are never removed. After cleanup a folder retains
`run.json`, `cleanup-record.json` and, while some recorded path remains,
`owned-paths`. These summary files take a few kilobytes. The recovery
feature will retain additional `recovery/` contents, which can be much larger.
Pruning old run folders is not part of this feature.

### Recording owned paths

Cleanup removes a worktree or target only if the run that created it recorded
it in `owned-paths`. The bundled helpers do this for you:

- `workflow_worktree_root.sh` and the recipes that create worktrees through it
  (`workflow-worktree.yaml`, `consensus-issue-worktree.yaml` and
  `autodrive_pr_worktree.sh`) call `amplihack_run_owned.sh record` after a new
  `git worktree add`.
- When the same helpers reuse an existing worktree, they call
  `amplihack_run_owned.sh adopt`. It records the worktree for the current run
  only if an earlier run already recorded it. The worktree is then listed by
  every run that used it, and cleanup keeps it while any of those runs is
  active. A worktree you made by hand is never recorded, even when a run
  reuses it.
- `scripts/git-hook-cargo.sh` records the target it uses, whether it built
  it first (`created`) or found it already there (`reused`), together with the
  checkout it builds.

A worktree line has three tab-separated fields, `kind`, `path` and `branch`.
A target line written by the current hook has five: `kind`, `path`, an empty
branch, the origin (`created` or `reused`) and the checkout path:

```text
worktree	/home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-2039	feat/issue-2039
target	/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d		created	/home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-2039
```

Cleanup also accepts the older forms: a line with two fields, or three with an
empty branch. Such a **legacy** target line comes from a hook older than
#2039, names a target with a `-run-<run id>` suffix, and is handled by the
rules in [When a run is eligible for cleanup](#when-a-run-is-eligible-for-cleanup).

Any other line is dropped when cleanup reads the file: four fields, six or
more, an origin other than `created` or `reused`, a relative checkout path, a
five-field line whose kind is not `target` or whose branch is not empty, and
any line with a control character. Cleanup never repairs a line. It keeps
valid lines byte for byte when it rewrites the file.

The five-field format and the conditions under which the hook writes nothing
are listed in
[Recording the target for a run](../reference/pre-commit-build-cache.md#recording-the-target-for-a-run).

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

None of them adds a fallback of its own. When the work root is set
explicitly but cannot be used, for example because `AMPLIHACK_WORK_ROOT` is a
relative path, `base` prints the reason and the step stops with
`cannot work out where worktrees go`. It does not create the worktree under
`<main checkout>/worktrees/`, the place you asked to avoid.

One exception remains. When the branch a run continues is checked out in the
caller's own checkout, `default-workflow` step 4 creates an alias worktree,
`<branch>--wt-<n>`, and that always goes under `<main checkout>/worktrees/`,
because the #1762 planner and `workflow_alias_reuse.sh` look for it there.
Alias worktrees are not recorded for the run, and cleanup does not remove
them. Remove them with `git worktree remove` when you no longer need them.

The default work root (`$HOME/.amplihack/work`) applies to run folders,
targets, clones and the archive whether or not the variable is set. Only
worktrees need the variable set explicitly.

## Preserve or retire recovery state

Durable recovery storage and `--fresh` are supported. Recovery retirement
remains a separate operation; existing cleanup categories do not retire recovery
history or its latest-attempt markers.

Keep `<run dir>/recovery/` when you need a future launch to recover after
a usage limit. The identity index under the work root selects only the
latest attempt. Keep that metadata as well; moving a run directory alone
does not establish a matching recovery identity.

The `run-tmp` category and normal end-of-run runtime removal exclude durable
recovery state. Required temporary artifacts are copied into private
recovery storage before completion is published, so runtime cleanup does
not erase replayable outputs. An output that refers to an unavailable,
uncopied artifact is invalidated on relaunch.

Use `--fresh` on the next `recipe run` to bypass history without deleting
it. Do not delete a latest marker to reach an older usage-limit run.
Retention must preserve a latest-attempt tombstone when detailed history is
retired, so a later failure or success continues to exclude older history.
Deleting the latest attempt's detail means future recovery falls back
fresh if that detail is needed.

**Retirement tooling is unresolved.** Existing `amplihack cleanup`
categories cover runtime files, worktrees, targets, and logs; they do not
provide a coordinated recovery-retirement operation. No supported command
or manual deletion procedure is specified for retiring sensitive recovery
records. Deleting directories or markers manually cannot meet the
concurrency contract. The planned `--fresh` flag only bypasses replay; it
does not remove records.

A supported retirement interface must coordinate with admission and
imports, exclude active owners and in-progress imports, and durably preserve
the latest-attempt sequence/tombstone before deleting detailed records.
Interrupted retirement must not expose older history as eligible. Its
selection/reporting contract must account for sensitive inherited copies
in later attempts: retiring an ancestor does not erase those copies.
Recovery records never authorize deleting external dependencies or
worktrees. The command, locking protocol, and handling of interrupted
retirement must be specified and implemented before publishing operational
retirement instructions.

Recovery copies can retain allowed prompts, typed context, outputs, and
artifact contents after their runtime originals disappear. They are
private sensitive data: directories are `0700`, regular files `0600`.
They are not included in a summary-only `run.json`, and log archiving does
not retire them. Retention policy must cover these durable copies as well
as logs, but enforcing that policy requires the unresolved retirement
interface. Private permissions do not provide encryption.

For launch behavior, see
[Resume a recipe after a usage limit](resume-recipe-after-usage-limit.md).
For publication and retention invariants, see
[private recovery storage](../reference/recipe-recovery.md#private-storage-and-publication).

## What happens when a run ends

When `amplihack recipe run` finishes, whether the run succeeded, failed or
panicked, the runner does these steps in order. Each step runs even when an
earlier one failed, and the final error names every failure.

1. Removes the execution checkout (`checkout.remove()`). It deletes only paths
   its `owner.json` proves the run owns and never follows symlinks. It does
   not change the caller's worktree list. A substituted path is kept and the
   run fails. When a durable worktree depends on the private repository, the
   repository stays and is listed in `durable-dependencies`.
2. Publishes the Git command record (`git_record.finish()`):
   `git-commands.jsonl`, `diagnostics/git/commands.jsonl` and
   `git-recording.json`, then removes `git-trace2/`. See
   [Git command record](../reference/recipe-runner-logging.md#git-command-record).
   A publication failure makes the run fail.
3. Finishes the recovery admission with an outcome judged from the run and
   the first two steps.
4. Finishes the workspace (`finish_workspace`):
   1. Removes `<run dir>/runtime/`, including everything steps wrote to
      `TMPDIR` and `artifacts/`. Durable `recovery/` state and copied
      artifacts remain outside that disposable folder.
   2. Finalises `run.json` with the end time, outcome and step summary.
   3. Unless `AMPLIHACK_CLEANUP_ON_EXIT=0` is set, sweeps the whole work root
      for run-owned items that are now eligible (the categories `run-tmp`,
      `run-worktree` and `run-target`), and removes them. This includes items
      from earlier runs, and from the current run if it is already eligible.
      When the sweep removes a worktree, that worktree's target goes with it in
      the same sweep, whether the run recorded it as `created` or `reused`; see
      [When a target is removed](#when-a-target-is-removed).

`AMPLIHACK_CLEANUP_ON_EXIT=0` skips only the sweep in step 4. Steps 1 to 3
always run.

A run that fails before its first step, for example on an unborn `HEAD`,
still finishes its recovery admission as `Failure`, so the next launch does
not try to resume it.

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
          "path": "/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d",
          "size_bytes": 4294967296
        }
      ],
      "skipped": []
    }
  ]
}
```

If nothing was removed, no record is written.

A real run starts its workspace with `RunWorkspace::begin_durable`. If the
work root cannot be used, for example because it is group-writable, the run
fails before any step starts, with an error that names the work root. Fix
its permissions (`chmod g-w,o-w`) or point `AMPLIHACK_WORK_ROOT` at a private
absolute directory. A real run never falls back to a temporary folder. A `--dry-run` uses
`RunWorkspace::dry_runtime`: a temporary `amplihack-dry-*` folder that is
removed on exit, with no run folder, checkout or recording.

Unverifiable historical records instead cause a logged fresh execution
after secure admission of the new attempt.

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

These rules decide worktrees, and targets recorded in the legacy format. A
run's legacy targets are removed only when every worktree the run still has is
eligible. If a recorded worktree could not be checked, the targets are kept.
When none of the run's worktrees is left, the targets follow the no-PR rule
above. Targets recorded in the five-field format follow
[When a target is removed](#when-a-target-is-removed) instead, and the keep
period does not apply to them.

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

## When a target is removed

A target recorded in the five-field format is removed only when one of three
events happens. There is no age rule: `--keep-hours` and
`AMPLIHACK_CLEANUP_KEEP_HOURS` do not delay or cause the removal, and a target
is never removed because it is old.

| Event | Which targets | Reason in the report |
| --- | --- | --- |
| **(a) Run end.** The sweep removes a worktree that this run recorded. | Targets this run recorded as `created` for that worktree's path. | `worktree removed by run cleanup` |
| **(b) Checkout gone.** The checkout path in the line no longer exists. | Any recorded target for that checkout, `created` or `reused`. | `checkout gone` |
| **(c) Worktree teardown.** A worktree is removed by `amplihack cleanup` (including the run-end sweep) or by a bundled recipe helper. | The target of that worktree path, unless rule (a) already covers it. This includes a `reused` target of the run's own worktree. See [Remove the target of a removed worktree](#remove-the-target-of-a-removed-worktree). | `worktree removed` |

In every other case the target is kept and the entry says why:

| Case | Reason |
| --- | --- |
| The run's worktree for that checkout is kept, for example because its PR is open. | `worktree kept` |
| The sweep does not include the `run-worktree` category (`--only run-target`), so no worktree is removed and rule (a) cannot apply. | `worktree kept: run-worktree not selected` |
| The checkout is not a worktree this run recorded, such as the main checkout or a worktree the run adopted. | `checkout not owned by this run` |
| The origin is `reused` and the checkout still exists, for example the main checkout. | `reused target; checkout still exists` |
| The checkout path could not be checked, for example `EACCES`. | `checkout state unknown: <error>` |
| Another active run records the same target. | the existing `recorded by active run <id>` reason |

So the target of your main checkout is never removed at the end of a run, even
when a run built in it first, because the main checkout is never a worktree
the sweep removes. It goes only if the main checkout folder itself
is deleted (event b).

Rule (a) needs the worktree to be gone. Cleanup removes worktrees before
targets, and just before removing a target under rule (a) it checks the
checkout path again. Anything other than "not found", including a worktree
that `git worktree remove` refused, keeps the target with the reason
`worktree kept`.

Every removal, whatever the event, also needs the checks in
[Safety rules](#safety-rules), including a free `.cargo-lock`.

Issue #2039 asked that a run's target be gone when the run ends. Cleanup
instead removes it when the run's worktree is removed (events a and c). While
the worktree is kept, for example an auto-drive PR worktree waiting for
review, its target stays, because the next commit in that worktree would
otherwise rebuild it from nothing (2-5 GB of build output). The target goes
in the same sweep that removes the worktree, or when the checkout folder is
deleted by any other means (event b).

Example: a run builds in a new worktree for `feat/issue-2039`, commits, and
its PR merges. At the next sweep the worktree passes its checks and is
removed, and its target goes with it in the same sweep:

```text
CATEGORY        ACTION                         SIZE     PATH
run-worktree    removed                        1.8 MiB  /home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-2039
                reason: PR #2071 merged 2026-11-08T14:02:11Z
run-target      removed                        4.0 GiB  /home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d
                reason: worktree removed by run cleanup
```

The report lists entries in the order they were processed, so a worktree
always comes before the target that went with it.

The same run also committed once in the main checkout and recorded that
target as `reused`. That target stays:

```text
run-target      skipped                        6.2 GiB  /home/ryan/.amplihack/work/targets/amplihack-precommit-target-0a1b2c3d4e5f
                reason: reused target; checkout still exists
```

## Linked targets

A target in `<work root>/targets` or the hook's cache folder can be a link to
a folder on another disk. Cleanup removes a linked target, first the
destination folder and then the link, only when every one of these holds,
both when the plan is made and again just before removal:

- the link is owned by you;
- the link points in one step at a real folder: not dangling, not at another
  link, with no link anywhere in the destination's path;
- the destination is a direct child of an allowed root (see below), not the
  root itself, or the link has the host cargo wrapper's layout (see
  [Links made by the host cargo wrapper](#links-made-by-the-host-cargo-wrapper));
- the destination is owned by you;
- the destination looks like a Cargo target: a `CACHEDIR.TAG` with Cargo's
  signature, a `.rustc_info.json`, or `<profile>/.fingerprint` or
  `<profile>/.cargo-lock`; links inside it are not followed;
- at removal, the link still points at the same destination, and the target
  has the same shape (folder, link or dangling link) it had when planned.

The allowed roots are `<work root>/targets`, the hook's XDG cache folder, and
the folders in `AMPLIHACK_TARGET_LINK_ROOTS`:

```bash
export AMPLIHACK_TARGET_LINK_ROOTS=/mnt/build/cargo-targets
amplihack cleanup --only run-target
```

### Links made by the host cargo wrapper

A host cargo wrapper, such as the one on candide, moves a new target off the
root disk and leaves a link in its place:

```text
~/.cache/amplihack/amplihack-precommit-target-0e047858ba2f
  -> /d0/ryan/cargo-targets/amplihack-amplihack-precommit-target-0e047858ba2f-f5e95e09b016
```

Cleanup follows such a link with no setting. The destination's name must be
exactly `<name of the link's folder>-<target name>-<12 lowercase hex>`, so it
carries the checkout's own hash, and the folder that holds the destination
must pass the same checks as an entry of `AMPLIHACK_TARGET_LINK_ROOTS`
(below). Every other rule in this list still applies. On candide, therefore,
`AMPLIHACK_TARGET_LINK_ROOTS` does not need to be set; it is only for links
with some other layout.

Use a folder that holds only build output for each entry of
`AMPLIHACK_TARGET_LINK_ROOTS`, such as `/mnt/build/cargo-targets`. An entry
such as `~/src` is accepted, because it is not your home folder or one of
its parents, but a link you own that points at a project's own `target/`
folder under it would then let cleanup remove that project's build output.

Entries of `AMPLIHACK_TARGET_LINK_ROOTS`, and the folder derived from a
wrapper link, that are
`/`, your home folder or one of its parents, relative, missing, owned by
someone else, or writable by others without the sticky bit are ignored with a
warning. The full rules are in
[Linked targets and AMPLIHACK_TARGET_LINK_ROOTS](../reference/pre-commit-build-cache.md#linked-targets-and-amplihack_target_link_roots).

When a check fails, the link and its destination are left alone and the
entry is skipped with one of these reasons:

| Reason | Meaning |
| --- | --- |
| `link not followed: outside allowed roots` | The destination is not a direct child of an allowed root, and its name is not the host cargo wrapper's layout, or its folder failed the root checks. Set `AMPLIHACK_TARGET_LINK_ROOTS`. |
| `link not followed: not a cargo target` | The destination does not look like a Cargo target. |
| `link not followed: link chain` | The link points at another link, or the destination's path contains one. |
| `link not followed: foreign owner` | The link or the destination belongs to another user. |
| `changed since plan` | The link now points elsewhere, or the target changed shape between plan and removal. |

A dangling link is not checked against these rules, since there is nothing
behind it to remove. When rule (a) or (b) applies to a recorded target that
is a dangling link, or `--worktree-target` finds one at the target path, the
link alone is removed with `remove_file`, after the ownership check. The
report gives the reason of the rule that applied, for example `checkout gone`.
A link that resolved when planned and is dangling at removal is skipped as
`changed since plan`.

Untagged targets (the `untagged-target` category) still skip every link,
with the reason `symbolic link; not followed`.

## Remove the target of a removed worktree

When `amplihack cleanup` removes a worktree, including during the run-end
sweep, it then removes that worktree's target by itself. The one exception is
a target for which the same sweep has already planned a rule (a) removal as
its own entry; the follow-on step skips that one to avoid handling it twice.
A `created` record alone is not enough: under `--only run-worktree`, or when
the rule (a) entry was refused at plan time (for example because its
`.cargo-lock` was held), there is no such entry, and this step checks the
target again after the worktree is gone. A target recorded as `reused` for
the run's own worktree, or not recorded at all, is also removed by this step
in the same sweep, with the reason `worktree removed`; it does not wait for
rule (b) at a later sweep.

Whatever happens to the target, it is listed in the report. A target this
step keeps is shown as `skipped` with its reason, in both text and JSON
output, and a warning is written to the log. A target is listed once: when
this step reports it, a plan-time entry for the same target is dropped. The
result never changes the worktree's own entry.

The bundled recipe helpers do the same after their own worktree removals, in
`workflow-worktree.yaml`, `workflow_worktree_sweep.sh` and
`workflow_runtime_artifacts.sh`. Each one calls:

```bash
command -v amplihack >/dev/null 2>&1 \
  && amplihack cleanup --apply --worktree-target "$wt" >/dev/null || true
```

The report on stdout is discarded, but errors and refusals still go to
stderr, so they appear in the step log. No helper deletes a target with `rm`.

When a helper runs inside an active run, that run usually records the
target itself, so the call skips it as recorded by another live run. This
is expected: the target is removed under rule (b) once the run has ended,
because its checkout is gone by then. If `amplihack` is not installed, the
target stays and is removed later under rule (b), if a run recorded it, or by
the hook's stale-cache reclaim.

You can call the option yourself after removing a worktree by hand:

```bash
git worktree remove ~/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-2039
amplihack cleanup --worktree-target ~/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-2039
```

```text
CATEGORY        ACTION                         SIZE     PATH
run-target      would-remove                   4.0 GiB  /home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d
                reason: worktree removed
```

Add `--apply` to remove it. The option computes the target name from the
checkout path, looks for it under `<work root>/targets` and the hook's cache
folder, and applies the same checks as the sweep: owned by you, Cargo-shaped,
lock free, the [linked target](#linked-targets) rules, and not recorded by any
active run. A dangling link at the target path is removed.

It refuses, writes one line to stderr and exits with code `1`, when the
checkout path:

| Case | Message |
| --- | --- |
| is relative | `amplihack cleanup: --worktree-target needs an absolute path: <path>` |
| still exists, including the main checkout or a dangling link at that path | `amplihack cleanup: --worktree-target: checkout still exists: <path>` |
| has a parent folder that cannot be resolved | `amplihack cleanup: --worktree-target: cannot resolve the parent of <path>` |
| cannot be examined, for example because a parent folder denies access | `amplihack cleanup: --worktree-target: cannot examine checkout <path>: <error>` |

Only "not found" counts as removed. Any other error leaves the checkout's
state unknown, and the target is kept.

The target name is computed from the checkout's parent folder, so that folder
must still exist. When a helper removes the folder that held the worktree as
well as the worktree, the call exits with `1` and the message
`cannot resolve the parent of <path>`, and the target stays. The bundled
helpers end the call in `|| true`, so this does not fail the step; the
message is in the step log. The target is then removed under rule (b), if a
run recorded it, or by the hook's stale-cache reclaim.

It does not take the cleanup lock, so it can run while a sweep is in
progress. It exits with `0` when nothing was found or the target was skipped,
and with `1` only if a removal failed.

`git worktree remove` run directly does not call it. Such a target stays
until a sweep finds its checkout gone, if a run recorded it, or until the
hook's TTL reclaim removes it.

## Preview and apply cleanup

A dry run lists every candidate and what would happen to it:

```bash
amplihack cleanup
```

```text
CATEGORY        ACTION                         SIZE     PATH
run-target      would-remove                   4.0 GiB  /home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d
                reason: PR #1820 merged 2026-10-31T08:02:11Z
run-worktree    would-remove                   1.8 MiB  /home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-1808
                reason: PR #1820 merged 2026-10-31T08:02:11Z
run-worktree    skipped                        612 KiB  /home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-1829
                reason: PR #1831 is open
recipe-log      would-archive                  88 KiB   /home/ryan/.amplihack/recipe-logs/0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f.log
                reason: modified 41 days ago
session-log     skipped                        2.1 MiB  /home/ryan/.claude/projects/-home-ryan-src-amplihack-rs/5c1f....jsonl
                reason: archived by `amplihack sessions archive`
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
      "path": "/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d",
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
| `run-tmp` | `<work root>/runs/<id>/runtime` (or `<work root>/r/<short id>`) left by a run that is no longer active, such as one that was killed. The keep period does not apply: the folder is removed as soon as the run is inactive and no running process (such as a child the killed run left behind) uses it. | remove |
| `run-worktree` | Worktrees recorded in a run's `owned-paths`, under `<work root>/worktrees/` or `<main checkout>/worktrees/`, once the run is eligible. | `git worktree remove`; the branch is kept |
| `run-target` | Targets recorded in a run's `owned-paths`: on the events in [When a target is removed](#when-a-target-is-removed), or for legacy lines once the run is eligible. Also the target removed by `--worktree-target`. A linked target loses both its destination and the link. | remove |
| `run-exec` | Folders directly under `<work root>/exec/`: the stable execution storage of runs with a recovery identity ([checkout isolation](../reference/recipe-checkout-isolation.md)). One is collected when the run named in its `claim.json` has ended and no worktree registered in its private repository still exists outside it; an empty folder with no claim, and a `.reclaim-*` folder an interrupted reclaim moved aside, are collected too. A symlink is never followed. The keep period does not apply. A relaunch with the same identity applies the same rules before it reuses the path. | remove |
| `untagged-target` | Folders directly under `<work root>/targets/`, and `amplihack-precommit-target-*` folders in the hook cache base, that no run recorded and that have not been modified within the keep period. | remove |
| `tmp-leftover` | Top-level `replay-*`, `release-*` and `amplihack-workflow-*` folders in the system temporary folder that you own, that are older than the keep period, that hold no `pid` marker naming a live process, and that no running process has as its working folder, holds a file open in, or has as its `TMPDIR`, `TMP` or `TEMP`. | remove |
| `recipe-log` | Files in `recipe.log_dir` older than `--log-days`. | archive |
| `session-log` | Files in `${CODEX_HOME:-~/.codex}/sessions`, including its nested date folders, older than `--log-days`. Claude sessions under `~/.claude/projects` are listed but skipped with ``archived by `amplihack sessions archive` ``: that command archives each whole session as one restorable unit, never touches a session a running `claude` still uses, and runs once a day from the login rotator. See [the sessions command](../reference/sessions-command.md). | archive |

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

- `scripts/git-hook-cargo.sh`, for the pre-commit and pre-push target. It
  follows the #1771 rules: it tags a target that already exists without a
  tag, leaves a missing target for cargo to create (cargo writes its own
  tag, with "created by cargo" on the second line), and never writes through
  a `CACHEDIR.TAG` that is a link, dangling or not;
- `scripts/install-pinned-recipe-runner.sh` and `scripts/dev-space.sh`, when
  they create a target folder;
- the Rust helper `create_tagged_target_dir`.

`git-hook-cargo.sh` chooses its folder like this, inside or outside a run:

| Condition | Folder |
| --- | --- |
| `AMPLIHACK_WORK_ROOT` usable | `$AMPLIHACK_WORK_ROOT/targets/amplihack-precommit-target-<hash>` |
| `AMPLIHACK_WORK_ROOT` not set, or not usable | `${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/amplihack-precommit-target-<hash>` |

`<hash>` is the first 12 hex digits of the SHA-256 of the checkout path.

The hook uses the work root only when it already exists as a real folder (not
a symbolic link) owned by you, is not group- or world-writable, and is neither
`/` nor your home folder. It never creates the work root, and it creates
`targets/` with mode `0700`. If it cannot record a target in `owned-paths`,
it prints `amplihack: warning: could not record cache ownership for <path>`
and the build goes ahead.

Each checkout has one target. Every run that commits in a checkout reuses
its target, so a second run in the same worktree does not build from
scratch. Inside a run, the script records the target in `owned-paths` as
`created` or `reused`, and cleanup removes it on the events in
[When a target is removed](#when-a-target-is-removed). Its stale-cache
reclaim recognises both the current names and the older
`-run-<run id>` names, and considers only folders you own.

The runner does not set `CARGO_TARGET_DIR`. A worktree's own `target/` stays
inside the worktree, Cargo tags it, and it is removed with the worktree.

To list the targets a run recorded, with their origin and checkout:

```bash
grep '^target' ~/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/owned-paths
# target	/home/ryan/.amplihack/work/targets/amplihack-precommit-target-5e6f7a8b9c0d		created	/home/ryan/.amplihack/work/worktrees/amplihack-rs-1a2b3c4d/feat/issue-2039
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
2. writes the archive to a `.partial` file with mode `0600` and flushes it to
   disk;
3. reads the `.partial` file back, decompresses it and compares the bytes
   with the original, then renames it to its final name;
4. appends a line to `MANIFEST.jsonl`;
5. checks that the original's size and modification time have not changed;
6. removes the original.

If any step fails, the original is kept and the entry is reported as failed.
No log is deleted unless its archive has been verified. An archive that fails
the read-back check is removed, so a retry does not leave extra files.

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
clones go to `./<repository name>` as before. Inside a run, the launcher
prints `amplihack: <url> is checked out at <folder>`, because the clone is no
longer in the current folder. Cleanup does not remove clones.

The folder name depends only on the URL, so a later run that checks out the
same repository reuses the clone. It fetches and moves the clone's branch to
the fetched tip, so the run never works on an old commit. The launcher adds
`/worktrees/` to the clone's `.git/info/exclude`, so worktrees that runs put
under `<clone>/worktrees/` do not count as changes. The launch is
refused, with a message naming the folder and changing nothing, when:

- the folder is not a git checkout of that URL;
- the clone has uncommitted changes, is not on a branch with an upstream,
  tracks a branch of a remote other than `origin`, or has commits its
  upstream does not have;
- the work root or `clones/` is a symbolic link, belongs to another user, or
  is group- or world-writable, or the clone belongs to another user.

Clones and fetches inside a run use only `https` and `ssh`; plain `git://` is
refused.

## Safety rules

Cleanup deletes only items inside the folders it manages, at the depth it
expects:

- `<work root>/runs/<id>/runtime` and `<work root>/r/<short id>`;
- recorded worktrees under `<work root>/worktrees/` or
  `<main checkout>/worktrees/`;
- folders directly under `<work root>/targets/`, and
  `amplihack-precommit-target-*` folders in the hook cache base;
- for a [linked target](#linked-targets), the destination folder directly
  inside an allowed root, and then the link;
- the `tmp-leftover` folders described above;
- log originals, after their archive is verified.

The `owned-paths` rules below apply to this cleanup command only. They are
separate from the release a recipe run does at its end: that release deletes
only paths its `owner.json` manifest proves the run created, with matching
identities. See
[ownership and release](../reference/recipe-checkout-isolation.md#ownership-and-release).

An entry in `owned-paths` is a request, not a permission. A recorded path
outside these folders is reported as skipped.

A worktree is removed only if all of these hold:

- it is a linked worktree, never the main checkout;
- it has no uncommitted changes and no unpushed commits (if its upstream
  branch is gone, for example deleted after the merge, every commit must be
  reachable from some remote-tracking branch);
- no running process has its working folder, an open file, or its
  `TMPDIR`, `TMP` or `TEMP` inside it;
- the run that recorded it is not active, and no other active run has
  recorded it (a run whose `run.json` cannot be read counts as active);
- `git worktree remove` succeeds without `--force`.

Git commands against a candidate run with hooks and fsmonitor disabled and
with the system git configuration ignored. A repository's own
`.git/config` can still name clean and smudge filters, and those can run
during `git status` and `git worktree remove`, and inside `gh`. Cleanup runs
as you, on worktrees of your own runs, so such a filter has no more rights
than whoever could already write that configuration.

Before each removal, cleanup resolves the path again and requires it to
resolve to the same place it did when the item was collected, and still to lie
inside the folders above. It then removes that resolved path, never the path a
step wrote. It also refuses symbolic links and repeats the liveness, Cargo
lock and cleanliness checks; if the run's `run.json` can no longer be read,
the item is skipped. An item that fails one of these checks is reported as
`skipped` with the reason. Symbolic links are not followed when counting
sizes. A run target that is a link is checked by the
[linked target](#linked-targets) rules instead of being refused outright.

Every target removal, whether by the sweep, after a worktree removal or by
`--worktree-target`, goes through one routine that:

1. takes an exclusive, non-blocking lock on `<target>/.cargo-lock` and on the
   `.cargo-lock` of each real subfolder, such as `debug/` and `release/`.
   Lock files are opened without following links and are never created.
   A `.cargo-lock` that is itself a link counts as held, because Cargo never
   makes one;
2. if any lock is held, skips the target with `in use: .cargo-lock is held`
   and deletes nothing. If a lock or a subfolder cannot be checked, it skips
   the target with a reason that names the problem and the path, and deletes
   nothing:

   | Reason | Meaning |
   | --- | --- |
   | `in use: .cargo-lock is held` | A build holds the lock. |
   | `linked lock: <path>` | The `.cargo-lock` is a link. Cargo never makes one. |
   | `permission denied: <path>` | The lock file or folder cannot be opened. |
   | `lock check failed: <path>: <error>` | Any other error while opening or locking. |

3. keeps every lock until the removal is finished, so a build cannot start in
   the target while it is being deleted;
4. removes the resolved target folder without following links inside it,
   then, for a linked target, the link;
5. logs each removal at `info` level with the path and shape, and a second
   line with the event (`a`, `b` or `c` from
   [When a target is removed](#when-a-target-is-removed)), the reason and the
   run id.

The locks are kept while `remove_dir_all` runs, so the `.cargo-lock` files
are deleted while cleanup still holds them. A build that opened one of them
just before that point finds its target folder gone and fails; nothing else
is removed. The next build in that checkout starts from an empty target.

A held lock never makes cleanup wait or retry. The target is skipped and
looked at again by the next sweep. A running process with a file open in the
target also keeps it.

Locks are checked one folder level deep only. A cross-compiled profile keeps
its lock at `<target>/<triple>/<profile>/.cargo-lock`, which is not locked;
such a build is protected by the running-process check instead.

An error of any kind while checking a target, such as a permission error, a
path in an unknown state, or a folder that cannot be listed, is a reason to
keep the target, never to remove it.

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
amplihack cleanup [--apply] [--format text|json] --worktree-target <checkout path>
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
| `--worktree-target <path>` | Hidden. Remove only the target of the removed worktree at `<path>`. Cannot be combined with `--only`, `--skip-logs`, `--keep-hours` or `--log-days`. See [Remove the target of a removed worktree](#remove-the-target-of-a-removed-worktree). |

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

**`git worktree list` shows a `prunable` entry under `runs/`.** A run was
killed with `SIGKILL`, so it could not remove its own checkout. The
end-of-run sweep deleted the folder later, but the worktree record is still in
the repository's `.git/worktrees/`. It does no harm. `amplihack cleanup` never
prunes records, so remove it from the repository yourself:

```bash
git -C ~/src/myapp worktree prune --dry-run --verbose
git -C ~/src/myapp worktree prune
```

Check the dry-run list first: `prune` also drops records of worktrees on a
disk that is not mounted.

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

**A linked target is skipped with `link not followed: outside allowed roots`.**
The link points at a disk that is not in `AMPLIHACK_TARGET_LINK_ROOTS`, and
the destination's name is not the host cargo wrapper's layout. Add the folder
that holds the destinations, for example
`export AMPLIHACK_TARGET_LINK_ROOTS=/mnt/build/cargo-targets`, and run cleanup
again. If cleanup prints a warning that it ignored the entry, check that the
folder exists, is owned by you, and is not writable by others. A wrapper
link with this reason means the folder holding its destination failed those
same checks.

**A target is skipped with `in use: .cargo-lock is held`.** A Cargo build is
using it. The next sweep looks at it again; nothing needs to be done.

**A target is skipped with `linked lock`, `permission denied` or
`lock check failed`.** Cleanup could not check a `.cargo-lock` at the path
shown. Fix the permissions, or remove the link if you made it, and run
cleanup again.

**A target stays after its worktree was removed.** Check how the worktree was
removed. `git worktree remove` run by hand does not remove the target; call
`amplihack cleanup --apply --worktree-target <path>` with the same path.

**A tool cannot create a socket in `TMPDIR`.** Check the length of
`$TMPDIR`. Runs switch to the shorter `<work root>/r/<short id>` layout when
the default path is over 80 bytes; a very long `AMPLIHACK_WORK_ROOT` can still
exceed the socket limit, so choose a shorter one.

## Related documentation

- [Hygiene Cleanup](../operations/hygiene-cleanup.md) - Opt-in cleanup of stale worktrees, detached Cargo targets and session artifacts outside recipe runs
- [Configure Workflow Runtime Isolation](configure-workflow-runtime-isolation.md) - Where workflow runtime output is written
- [Correlate Recipe Runs with Logs](correlate-recipe-runs.md) - Match a run id to its logs and processes
