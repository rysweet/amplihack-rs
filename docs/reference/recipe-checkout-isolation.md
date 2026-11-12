---
title: Recipe Run and Auto-Drive Checkout Isolation
description: Where recipe runs and auto-drive reviews execute, what they copy from the caller, how they record Git, and how they release what they own.
doc_type: reference
last_updated: 2026-11-12
---

# Recipe run and auto-drive checkout isolation

`amplihack recipe run` never runs a step in the caller's checkout. Each real
run copies the caller's committed, staged and working state into a private
repository and a detached checkout, runs every step, agent and hook there,
records the Git commands they run, and removes the checkout when the run ends.
Auto-drive judges each pull request in a private checkout at the exact PR
head commit it recorded. This is issue #2096.

## Contents

- [What the run copies](#what-the-run-copies)
- [Where each kind of run executes](#where-each-kind-of-run-executes)
- [Paths and Git configuration](#paths-and-git-configuration)
- [Independent, nested and resumed runs](#independent-nested-and-resumed-runs)
- [Ownership and release](#ownership-and-release)
- [Required recording](#required-recording)
- [Private review generations](#private-review-generations)
- [Configuration](#configuration)
- [Rust components](#rust-components)
- [Failure behavior](#failure-behavior)
- [Platform support](#platform-support)
- [Tests](#tests)
- [Limits](#limits)

## What the run copies

Preparation finishes before the first step starts. The caller's checkout is
read only: the run does not stage, commit, switch branches, reset, pull or
change Git configuration there.

| Caller state | What the execution checkout gets |
| --- | --- |
| `HEAD` | The same commit, checked out detached. |
| Index | The staged version of every path, including additions and deletions not yet committed. |
| Working tree | The unstaged version of every path, kept separate from the staged version. |
| Untracked files | Every file `git ls-files --others --exclude-standard` lists. |
| Ignored files | Only the explicit recipe file when it is ignored. Other ignored files are not copied. |
| Symlinks | Kept when the target stays inside the repository. |
| File modes | Only `mode & 0o777`; set-user-ID and similar bits are dropped. |

Paths are handled as bytes, so non-UTF-8 names survive. Staged blobs are
looked up in one `git cat-file --batch-check` call. Its input is object IDs
only, and any output line that is not `<oid> <type>` or `<oid> missing` stops
preparation.

Preparation fails, naming the path, when it finds:

- a symlink that points outside the repository, or a path under a symlinked
  directory;
- a socket, FIFO or device file;
- merge conflicts, submodules or index flags it cannot reproduce;
- an unborn `HEAD` (make a first commit), a bare repository, or a malformed
  `.git`;
- caller changes between the two observations taken around the copy.

There is no fallback to committed content or to the caller's checkout.

## Where each kind of run executes

| Invocation | Steps run in |
| --- | --- |
| Inside a Git checkout | `<run dir>/execution-<id>/checkout`, at the same relative subdirectory as `-w`. |
| Outside any Git checkout | The requested directory, as given. Recording still applies. |
| Git missing, or `.git` unreadable or corrupt | Nowhere: preparation fails. |
| `--dry-run` | Nothing runs. The caller's directory is used for planning only. |

A dry run prepares no checkout, writes no `git-recording.json`, and creates no
run folder under the work root. It uses a temporary `amplihack-dry-*`
directory for scratch space and removes it on exit.

The runner gets `-C <exec dir>`. Steps, hooks and agents start there.

## Paths and Git configuration

Every Git call made during preparation and release uses a fixed argument list
and an environment with `GIT_DIR`, `GIT_COMMON_DIR`, `GIT_WORK_TREE`,
`GIT_INDEX_FILE`, object-directory overrides and `GIT_CONFIG_*` removed. Global
and system configuration, hooks, templates and clean/smudge filters are
disabled for those calls. Hooks that steps trigger, such as pre-commit and
pre-push, run normally inside the execution checkout.

The private repository is a `git clone --bare --no-local --no-hardlinks` of the
caller. It has no alternates pointing at the caller's object store, and the
caller's worktree list does not change.

Path inputs are mapped before launch:

- `-w` selects the source and the matching subdirectory of the checkout.
- A context value that is exactly an absolute path inside the caller's
  checkout, such as `-c repo_path=/home/ryan/src/amplihack-rs`, is rewritten to
  the matching path in the execution checkout.
- A context value that embeds a caller path inside longer text is refused:
  `context task embeds a caller-checkout path in opaque text; pass the path as
  a separate context value`.
- Paths outside the caller's checkout are passed through unchanged.

Durable task and PR worktrees are separate checkouts with their own ownership.
Commits a recipe means to keep go there, not into the execution checkout.

## Independent, nested and resumed runs

Every independent `amplihack recipe run`, including one started inside
another run, gets its own repository, checkout, index, recording and owner
record. An inherited `AMPLIHACK_RUN_CHECKOUT` is ignored with a message:

```text
amplihack: ignoring inherited AMPLIHACK_RUN_CHECKOUT; this independent invocation owns a fresh checkout
```

Recovery admission ([recipe recovery](recipe-recovery.md)) is keyed on the
caller's `--working-dir`, because that identifies the run. Execution happens
in a private `exec dir` that belongs to one attempt:

- A resumed run prepares a fresh checkout before its first step. It never
  reuses the contents of a checkout an earlier attempt removed.
- The recovery descriptor is written in the run folder. A descriptor path that
  resolves inside the `exec dir` is refused.
- A run with a recovery identity prepares each attempt's checkout at
  `<work root>/exec/<identity>/`, so that a relaunch presents the same
  repository path. The path is reused when the earlier attempt has finished
  and no durable worktrees from it remain. Then replay can reuse steps that
  depend on the checkout (part of
  [#2493](https://github.com/rysweet/amplihack-rs/issues/2493), which stays
  open: a replay while durable worktrees from the earlier attempt are still
  in use needs those worktrees moved over or reattached). A run without an
  identity always uses a random path.
- Each attempt creates the path new (mode `0700`) and writes `claim.json`
  there, naming its run folder. Release removes the path. When an earlier
  attempt left it behind (it was killed, or preparation failed after the
  claim), the next attempt reclaims it once, under a lock on `exec/`, only
  when all of these hold:
  - the path is a real directory, never a symlink, and still the same
    directory (device and inode) when it is moved aside to be removed;
  - the run named in `claim.json` has ended (its `run.json` says so, or names
    a process that is gone, or the run folder is gone). A directory with no
    claim is reclaimed only when it is empty;
  - no worktree registered in its `repository.git` lies outside it and still
    exists. In default-workflow, step 04 creates such worktrees.

  Otherwise the new attempt prints why the path is still held (the owner is
  still running, a durable worktree still uses it, or it is not a real
  directory) and uses a fresh random path; replay then re-runs the
  checkout-dependent steps. `amplihack cleanup` applies the same rules to
  every path under `exec/` (category `run-exec`).

Each attempt keeps its own recording. A later successful attempt does not
remove an earlier attempt's evidence.

## Ownership and release

Preparation writes `<run dir>/execution-<id>/owner.json` at mode `0600`:

```json
{
  "schema_version": 1,
  "owner": "5b0e2f0c-1d7a-4f8e-9a63-0c2e7d41b9aa",
  "repository": "/home/ryan/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/execution-Xq3TzA/repository.git",
  "checkout": "/home/ryan/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/execution-Xq3TzA/checkout",
  "identities": [[2049, 1835012], [2049, 1835013], [2049, 1835014]],
  "head": "60ecbe5736c2f1d0a9b8e7f6a5b4c3d2e1f0a9b8"
}
```

`identities` holds the device and inode of the storage directory, the private
repository and the checkout, in that order.

The run directories are created at `0700`, and `owner.json`,
`git-recording.json` and the recovery descriptor at `0600`, when they are
created rather than by a later `chmod`.

Release checks the manifest and the device and inode of every path again just
before deleting. It deletes only paths the manifest proves the run owns, never
follows symlinks, and never acts on a path given only as an argument. A
substituted or changed path is kept and release fails:

```text
execution checkout ownership identity changed at <path>; resources retained
```

When a durable worktree depends on the private repository, its metadata is
kept and listed in `durable-dependencies`.

At the end of a real run, release happens in this order:

1. `checkout.remove()` removes the execution checkout.
2. `git_record.finish()` publishes the Git command record.
3. The recovery admission is finished with an outcome judged from the result
   of the first two steps.
4. `finish_workspace` writes `run.json` and runs the cleanup sweep.

Each step runs even when an earlier one failed. Errors are joined with
`.context()`, so the final error names every failure. A failure before the
first step (in `GitRecord::begin`, `prepare_with_inputs`, or path mapping)
still finishes the recovery admission, with `Failure`, unless the run was
stopped by a signal. The next launch therefore does not resume a run that
never started.

Copied inputs, which may include `.env` files, live only in the `0700`
`exec dir` and go with it. Error messages name paths, never file contents or
environment values.

## Required recording

On Linux, a ptrace observer follows the runner's whole process tree across
`fork`, `vfork`, `clone` and `exec`. It records every successful launch of an
executable named `git` or `git-*`, except known read-only commands. This
covers absolute Git paths and children started with a cleared environment.

Each record has schema version `1` and `run_id`, `attempt_id`, `executable`,
`argv` (redacted), `cwd`, `git_dir`, `index_file`, `git_environment`, `time`,
`sid` and `pid`. The run publishes:

- `<run dir>/git-commands.jsonl`
- `<run dir>/diagnostics/git/commands.jsonl`
- `<run dir>/git-recording.json`, with scope `runner-descendants`

Failure to start the observer, losing it during the run, a storage or
attribution error, or a failed publication makes the run fail. An empty
journal means the observer ran and saw no Git commands that can write.
`GitRecord::dry_run()` is the only state with recording switched off, and only
a dry run reaches it. See the [Git command record](recipe-runner-logging.md#git-command-record).

## Private review generations

Each auto-drive owns a review generation for one repository, PR number and
recorded PR-head SHA. `autodrive_worktree_guard.sh private-review-dir` calls
`amplihack internal review-checkout`, which clones into a private repository
and checks out that SHA detached. A second drive on the same PR gets its own
generation. When the PR advances, `review-next` makes a new generation; an old
verdict never covers the new head.

The guard accepts a review dir only when all four checks pass:

1. The generation has an ownership manifest that this run wrote.
2. The identities recorded in it (repository, generation, owner, device and
   inode of each path) match what is on disk.
3. `HEAD` is detached at the recorded PR-source commit.
4. The working tree and index hold exactly that commit's files:
   `git status --porcelain=v1 -z --untracked-files=all` prints nothing.
   Ignored files, such as build output, are allowed.

`index-check`, which the rounds run after the judging step, applies check 4
again through `amplihack internal review-clean`, so an edit that is never
staged still stops the drive. `review-bind-record` runs all four before it
binds a verdict to the recorded commit.

Any other directory gives `REVIEW_DIR_CHANGED` or `REVIEW_DIR_UNSTAMPED`,
never a pass. Being outside the PR worktree, or being detached, grants
nothing on its own. The review checkout does not share the caller's Git
directory, so staging or committing in the main checkout does not change its
stamp.

| Material | Comes from |
| --- | --- |
| Files and diff being judged | The review generation's checkout, at the recorded SHA. |
| Reviewer recipes, skills, prompts and settings | The launch-side bundle and the generation's separate trusted launch directory. |
| `AGENTS.md`, `CLAUDE.md`, `.claude/` in the PR | Review data only. They are kept byte for byte and never loaded as instructions. |

Judging agents start in the launch directory and read the checkout by
explicit path and SHA. Every verdict records `reviewed_head_sha`.
`amplihack internal review-release` removes the generation, under the same
ownership rules as [execution release](#ownership-and-release). See
[Where the judging agents start](auto-drive-to-merge.md#where-the-judging-agents-start).

## Configuration

Isolation and recording cannot be switched off.

| Setting | Effect |
| --- | --- |
| `-w`, `--working-dir` | Source checkout and the relative directory steps start in. Also the run's recovery identity. |
| `-c repo_path=...` | Mapped into the execution checkout when it is a caller-checkout path. |
| `--context-file`, `--context-from-file` | Read before launch; their contents are context values and follow the same mapping rules. |
| `--dry-run` | Plans only. No checkout, no recording, no run folder. |
| `AMPLIHACK_WORK_ROOT` | Where run folders and execution storage go; see [cleanup settings](../howto/clean-up-after-recipe-runs.md#settings). |
| `RECIPE_RUNNER_RS_PATH` | Runner binary. It runs under the observer like any step. |
| `AMPLIHACK_RUN_CHECKOUT` | Set for steps. An inherited value is ignored and grants no ownership. |
| `AMPLIHACK_CLEANUP_ON_EXIT=0` | Skips the cleanup sweep only. Release and recording still run. |
| Caller `GIT_TRACE2_*` | Has no effect on the Git command record. |

## Rust components

These are internal modules, not a public API.

| Component | Responsibility |
| --- | --- |
| `run_isolation/input_snapshot.rs` | Captures `HEAD`, index and working tree separately; rejects unsafe inputs. |
| `commands/recipe/run/run_checkout.rs` | Finds the repository (fails closed), provisions the private checkout, maps paths, releases owned paths. |
| `commands/recipe/run/git_record.rs` | `GitRecord::begin` starts the Linux observer; `GitRecord::dry_run()` is the disabled state for dry runs; `finish` publishes. |
| `run_isolation/observer/linux.rs` | ptrace observer, built only with `cfg(target_os = "linux")`. |
| `run_isolation/review_generation.rs` | Review generations: provision, four-part validation (including a clean working tree), release. |
| `run_isolation/exec_storage.rs` | Claims `<work root>/exec/<identity>/` and decides when it may be reclaimed; used by preparation and by `amplihack cleanup`. |
| `commands/cleanup/run_dir.rs` | `RunWorkspace::begin_durable` for real runs, `RunWorkspace::dry_runtime` for dry runs; `record_attempt_checked` and `finish_checked` return errors. |
| `commands/recipe/run/execute.rs` | Order of preparation, launch and release; joins errors. |

Callers use `?` on every `Result` from recording, checkout, admission and
workspace code. None of them is discarded with `.ok()`, `let _ =` or
`unwrap_or_default`.

## Failure behavior

| Failure | Result |
| --- | --- |
| Unsafe input, unborn `HEAD`, bare or malformed repository | Preparation fails naming the path; no step runs; admission finished as `Failure`. |
| Observer unavailable or fails to start | The runner never starts. Without a writable work root: `cannot initialize required Git recording; provide a writable absolute AMPLIHACK_WORK_ROOT and retry`. |
| Observer lost, storage or attribution error | Controlled teardown, partial evidence kept, run fails. |
| Checkout identity changed at release | Resources kept, run fails. |
| Review dir fails a check | `REVIEW_DIR_CHANGED` or `REVIEW_DIR_UNSTAMPED`; no judging. |
| Review checkout has edited or untracked files | `review-validate` and `index-check` refuse it, naming the first changed path. |
| Stable execution path still held | The attempt prints the reason and uses a fresh random path. |
| Git recording trace folder cannot be removed | The error is returned: the folder may contain command lines. |
| Every step passed but release or publication failed | The run fails. |

Output formats keep their existing fields. A runner exit code of zero is not
enough for success: recording, release and publication must also succeed.

## Platform support

Real recipe runs need Linux, because the observer is Linux-only. On macOS a
real run stops before the runner starts:

```text
required recipe process observation is supported on Linux only; run this recipe on Linux
```

`--dry-run`, `recipe list`, `recipe show` and `recipe validate` work on every
platform, and the crate builds on macOS.

## Tests

The tests run the real CLI against temporary repositories, with temporary
`HOME`, `AMPLIHACK_HOME`, profile root and `TMPDIR`. They never read or write
the real `~/.claude`, `~/.amplihack` or profile root. Concurrency uses socket
handshakes and observer acknowledgements; there are no sleeps, timers or
elapsed-time checks. Test helpers fail on any I/O error or non-zero exit, so an
isolation test cannot pass on empty data.

| Test file | Covers |
| --- | --- |
| `crates/amplihack-cli/tests/issue_2096_checkout_units.rs` | Snapshot layers, byte paths, symlinks, special files, batch blob query. |
| `crates/amplihack-cli/tests/issue_2096_execution_isolation.rs` | Caller `HEAD` and index unchanged; dry run makes no checkout, recording or run folder; resumed run gets a fresh `exec dir`; unborn caller finishes admission as `Failure`. |
| `crates/amplihack-cli/tests/issue_2096_recording.rs` | Observer coverage and failure cases. |
| `crates/amplihack-cli/tests/issue_2096_real_hooks.rs` | Pre-commit and push hooks run in the execution checkout. |
| `crates/amplihack-git/tests/issue_2096_git_policy.rs` | Git environment and configuration policy. |
| `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` | Review generation checks, `REVIEW_DIR_CHANGED` and `REVIEW_DIR_UNSTAMPED`. |

```bash
cargo test -p amplihack-cli --test main issue_2096
cargo test -p amplihack-git --test issue_2096_git_policy
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
```

## Limits

- A private repository isolates Git state. It is not an OS sandbox: a step
  can still write any path its user can write.
- The observer sees Git launched in the runner's process tree. It does not see
  Git that preparation and release run themselves, unrelated host processes,
  or non-Git writes to an index.
- Evidence files are not tamper-proof against the same user.
- Ignored files other than the recipe file are not copied. A recipe that needs
  one must take its path as a context value outside the checkout.

## Related

- [Inspect a local-input snapshot](../tutorials/recipe-checkout-isolation.md)
- [Run a recipe](../howto/run-a-recipe.md#know-where-the-run-works)
- [Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md#what-happens-when-a-run-ends)
- [Recipe runner logging](recipe-runner-logging.md#git-command-record)
- [Auto-drive to merge](auto-drive-to-merge.md#where-the-judging-agents-start)
- [Recipe runner architecture](../concepts/recipe-runner-architecture.md#the-runs-own-checkout)
