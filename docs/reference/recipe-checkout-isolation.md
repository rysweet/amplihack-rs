---
title: Recipe Run and Auto-Drive Checkout Isolation
description: Where recipe runs and auto-drive reviews execute, what they copy from the caller, how they record Git, and how they release what they own.
doc_type: reference
last_updated: 2026-11-13
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
  - [The origin remote in the private repository](#the-origin-remote-in-the-private-repository)
- [Independent, nested and resumed runs](#independent-nested-and-resumed-runs)
- [Ownership and release](#ownership-and-release)
- [Required recording](#required-recording)
- [Private review generations](#private-review-generations)
  - [Superseded generations are retired](#superseded-generations-are-retired)
- [Configuration](#configuration)
- [Rust components](#rust-components)
- [Failure behavior](#failure-behavior)
- [Platform support](#platform-support)
- [Tests](#tests)
  - [Review commands in the loop test double](#review-commands-in-the-loop-test-double)
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

### The origin remote in the private repository

A bare clone keeps no remote-tracking refs and no fetch refspec. Recipes that
create a branch need both: `workflow-worktree` resolves its base from
`origin/HEAD` and `origin/<branch>`, and `workflow_delivery_inputs.sh` reads
the same refs. Preparation therefore rebuilds the caller's `origin` in the
private repository, after copying the caller's remote configuration:

| Caller has | Private repository gets |
| --- | --- |
| `remote.origin.url` | `remote.origin.fetch=+refs/heads/*:refs/remotes/origin/*`, the normal refspec. |
| `refs/remotes/origin/*` | The same refs and the objects they need, copied by one local fetch from the caller's top-level directory: `git fetch --no-tags --quiet --no-auto-maintenance -- <caller top> '+refs/remotes/origin/*:refs/remotes/origin/*' '^refs/remotes/origin/HEAD'`. |
| `refs/remotes/origin/HEAD` as a symbolic ref | The same symbolic ref, after checks (below). |
| `refs/remotes/origin/HEAD` as a plain ref, or as a symbolic ref outside `refs/remotes/origin/` | No `origin/HEAD`, and a warning in the second case. |
| No `origin` remote | No refspec and no `origin/*` refs. Preparation still succeeds. |
| `remote.origin.url` but no `origin/*` refs | The refspec only. Nothing is fetched over the network during preparation; the recipe's own `workflow_fetch_origin.sh` fills `origin/*` on its first fetch. |
| `remote.origin.url` but no `origin/HEAD` | No `origin/HEAD`. Preparation never invents a default branch; the recipes use their existing fallback. |

The seeding fetch reads only the caller's local repository. It runs with
`GIT_ALLOW_PROTOCOL=file`, takes the canonical absolute caller path after
`--`, and writes only under `refs/remotes/origin/`. Tags are not copied.
The negative refspec keeps `origin/HEAD` out of the fetch, so `origin/HEAD` is
written only by the validated `symbolic-ref` step. Automatic maintenance is
off for the fetch, so no detached repack is still writing to the run's
storage when the run releases it.

`origin/HEAD` is copied only when the caller's
`git symbolic-ref -q refs/remotes/origin/HEAD` exits `0` and its target:

- starts with `refs/remotes/origin/`;
- contains no whitespace or control characters;
- passes `git check-ref-format`;
- resolves in the private repository after the fetch
  (`git rev-parse --verify --quiet`).

Exit status `1` (not a symbolic ref, or absent) means the ref is skipped. Any
other status, including death by a signal, stops preparation.

Each of these Git calls uses the same restricted environment as the rest of
preparation. A failure stops preparation with the usual wording, for example:

```text
cannot seed origin refs in the private repository: <git stderr>; partial resources retained at <exec dir>
```

Remote URLs are never printed, because they can carry credentials. Error text
contains only git's trimmed standard error from the local fetch.

Steps see the result as an ordinary clone. Inside a run's checkout:

```bash
git config --get remote.origin.fetch
# +refs/heads/*:refs/remotes/origin/*
git symbolic-ref refs/remotes/origin/HEAD
# refs/remotes/origin/main
git fetch origin   # updates refs/remotes/origin/* from the real remote
```

`quick-change` and `auto-drive-light` take their worktree from
`workflow-worktree`, and `consensus-issue-worktree` and
`workflow-terminal-state` run the same `resolve_base_ref` logic, so all four
find their base the same way.

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

Off Linux there is no observer. The run still executes in its private
checkout, without command recording, and says so in `git-recording.json` and
`run.json` with `"coverage":"unavailable"`. See
[Platform support](#platform-support).

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
[Review generations](auto-drive-to-merge.md#review-generations).

### Superseded generations are retired

Each loop's generations live in one allocation directory,
`<state dir>/review-<random>/`:

| Path | Holds |
| --- | --- |
| `<allocation>/checkout`, then `<allocation>/checkout-<uuid>` for each later head | The detached review checkout. |
| `<checkout>.review-owner.json` beside each checkout | The generation's ownership manifest (`0600`). |
| `<allocation>/review-generation-<random>/` | Storage: `repository.git` and the trusted launch directory. |

The manifest sits beside the checkout, not inside the storage, so it survives
when the storage is removed.

When `review-next` finds that the PR head has moved, it provisions the next
generation, records it in the current generation's `children` and writes the
manifest. It then retires the generation it replaced:

1. It checks the device and inode of the old storage, repository and checkout
   again.
2. It runs `git worktree remove --force` on the old checkout.
3. It sets `"retired": true` in the old manifest and writes it atomically.
4. It removes the old storage, only when the path is a directory (not a
   symlink) whose device and inode still match the manifest.

`review-next` prints the new generation's checkout on success. If the
identity check or the worktree removal fails, the old generation stays as it
is (`retired` remains `false`). If only the storage removal fails, the record
is already `retired` and `review-release` removes the storage later when its
identity still matches. Either way one warning goes to stderr and
`review-next` still prints the new generation. The loop continues, and
`review-release` frees the old generation when the loop exits:

```text
amplihack: warning: could not retire superseded review generation <checkout>: <error>; it is released when the loop ends
```

A retired generation keeps its manifest as the record that links the chain,
so the loop's first generation stays the release anchor. Its rules:

- `review-validate`, `review-clean`, `review-launch`, `review-head`,
  `review-bind-record`, `review-next` and state registration refuse it:
  `review generation is superseded and retired`.
- `review-release` does not run `git worktree remove` for it. It removes the
  storage if any is left, but only when the identity still matches; a
  mismatch stops release with `review storage identity changed; retained`.
  It then removes the manifest and the allocation directory and releases each
  child, as for a live generation.
- Nothing sets `retired` back to `false`, and retirement never changes the
  owner, children or allocation fields.
- Every manifest, retired or not, must name a storage directory that sits in
  the same allocation directory as its checkout and whose name starts with
  `review-generation-`. Retirement skips only the on-disk identity checks for
  the removed checkout, repository and launch directory; the size bound,
  `O_NOFOLLOW` read, schema version, owner and path-shape checks still apply.

When every retirement succeeds, a loop keeps one review clone on disk between
rounds, not one per PR head it has judged.

Generations are not made with `git clone --reference --dissociate`. After
`--dissociate` repacks, the clone holds a full copy of the objects, so it saves
no disk compared with `--no-local`, and during the clone it reads the caller's
object store through alternates, which weakens the isolation this design
relies on. Retirement gives the disk saving instead.

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
| `commands/recipe/run/run_checkout.rs` | Finds the repository (fails closed), provisions the private checkout, rebuilds the `origin` refspec, `origin/*` and `origin/HEAD`, maps paths, releases owned paths. |
| `commands/recipe/run/git_record.rs` | `GitRecord::begin` delegates to `begin_on(run_dir, RecordingPlatform::current())`. `RecordingPlatform::Linux` starts the observer; `RecordingPlatform::Other` gives the unobserved `Unavailable` state. `GitRecord::dry_run()` is the disabled state for dry runs; `finish` publishes. |
| `run_isolation/observer/linux.rs` | ptrace observer, built only with `cfg(target_os = "linux")`. |
| `run_isolation/review_generation.rs` | Review generations: provision, four-part validation (including a clean working tree), retirement of superseded generations in `advance`, release. `read_live` refuses retired generations; plain `read` is used only by `release`, `release_state` and the child walk. |
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
| Observer fails to start on Linux | The runner never starts. Without a writable work root: `cannot initialize required Git recording; provide a writable absolute AMPLIHACK_WORK_ROOT and retry`. |
| Not Linux | The run proceeds unobserved and records `coverage: "unavailable"`. Without a writable work root it stops with the same message as above. |
| Seeding `origin/*` or copying `origin/HEAD` fails | Preparation fails with `partial resources retained at <exec dir>`; no step runs. |
| Observer lost, storage or attribution error | Controlled teardown, partial evidence kept, run fails. |
| Checkout identity changed at release | Resources kept, run fails. |
| Review dir fails a check | `REVIEW_DIR_CHANGED` or `REVIEW_DIR_UNSTAMPED`; no judging. |
| Review checkout has edited or untracked files | `review-validate` and `index-check` refuse it, naming the first changed path. |
| Retiring a superseded review generation fails | One warning; the new generation is used and the old one is freed by `review-release` at loop exit. |
| A command is given a retired review generation | Refused: `review generation is superseded and retired`. |
| Stable execution path still held | The attempt prints the reason and uses a fresh random path. |
| Git recording trace folder cannot be removed | The error is returned: the folder may contain command lines. |
| Every step passed but release or publication failed | The run fails. |

Output formats keep their existing fields. A runner exit code of zero is not
enough for success: recording, release and publication must also succeed.

## Platform support

Real recipe runs work on every platform. Checkout isolation is the same
everywhere; only command recording depends on Linux.

| Platform | Steps run in | Git command recording |
| --- | --- | --- |
| Linux | The private checkout | Required. The ptrace observer runs and its failure fails the run. |
| macOS and every other non-Linux target | The private checkout | Unavailable. No observer is started and the run is not failed for it. |

Off Linux, `GitRecord` enters its `Unavailable` state:

- It prints one notice to stderr:

  ```text
  amplihack: Git command recording is unavailable on this platform; the run continues in its private checkout without recording
  ```

- The runner's environment has `AMPLIHACK_OBSERVER_PID`, the observer's event
  variables and every caller trace variable (`GIT_TRACE2`, `GIT_TRACE2_EVENT`,
  `GIT_TRACE2_ENV_VARS`, `GIT_TRACE2_PERF`, `GIT_TRACE2_CONFIG_PARAMS`,
  `GIT_TRACE2_REDACT`) removed, so no step writes trace output anywhere.
- `finish` writes `<run dir>/git-recording.json` (`0600`) and `run.json`
  carries it in `git_recording`. No `git-commands.jsonl` is written. See
  [When recording is unavailable](recipe-runner-logging.md#when-recording-is-unavailable).
- A run dir is still required. Without a usable work root the run stops, as on
  Linux.

The platform is decided once, by `RecordingPlatform::current()`, from the
build target. There is no environment variable or flag that switches a Linux
run to the unobserved path.

`--dry-run`, `recipe list`, `recipe show` and `recipe validate` work on every
platform, and the crate builds on macOS. Auto-drive still refuses macOS (see
[#1646](https://github.com/rysweet/amplihack-rs/issues/1646)); that refusal is
separate from recording.

## Tests

The cargo tests run the real CLI against temporary repositories, with temporary
`HOME`, `AMPLIHACK_HOME`, profile root and `TMPDIR`. They never read or write
the real `~/.claude`, `~/.amplihack` or profile root. Concurrency uses socket
handshakes and observer acknowledgements; there are no sleeps, timers or
elapsed-time checks. Test helpers fail on any I/O error or non-zero exit, so an
isolation test cannot pass on empty data.

| Test file | Covers |
| --- | --- |
| `crates/amplihack-cli/tests/issue_2096_checkout_units.rs` | Snapshot layers, byte paths, symlinks, special files, batch blob query. |
| `crates/amplihack-cli/tests/issue_2096_execution_isolation.rs` | Caller `HEAD` and index unchanged; dry run makes no checkout, recording or run folder; resumed run gets a fresh `exec dir`; unborn caller finishes admission as `Failure`. |
| `crates/amplihack-cli/tests/issue_2096_private_base_ref.rs` | Base resolution inside a private checkout made by `RunCheckout::prepare`, whose caller has an origin: the `resolve_base_ref` bodies taken from `workflow-worktree.yaml`, `consensus-issue-worktree.yaml` and `workflow-terminal-state.yaml`, and `workflow_delivery_inputs.sh worktree main false "" ""`, all without a prior fetch; `git fetch origin` in the private repository moves `origin/main`; a caller without an origin gets no refspec; a foreign `origin/HEAD` target is refused. |
| `crates/amplihack-cli/tests/issue_2096_recording.rs` | Observer coverage and failure cases. |
| `crates/amplihack-cli/src/commands/recipe/run/tests_git_record.rs` | `begin_on(RecordingPlatform::Other)`: the run is unobserved, no observer or trace variables reach the runner, `git-recording.json` says `unavailable`, and a missing run dir still fails. `begin_on(RecordingPlatform::Linux)` still requires the observer. |
| `crates/amplihack-cli/tests/issue_2096_review_isolation.rs` | Review generations, including retirement: after `advance` the old checkout and storage are gone and the manifest says `retired`; commands on a retired generation are refused; `release` of the first generation removes everything, including storage a failed retirement left behind. |
| `crates/amplihack-cli/tests/issue_2096_real_hooks.rs` | Pre-commit and push hooks run in the execution checkout. |
| `crates/amplihack-git/tests/issue_2096_git_policy.rs` | Git environment and configuration policy. |
| `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` | Review generation checks, `REVIEW_DIR_CHANGED` and `REVIEW_DIR_UNSTAMPED`. |
| `tests/issue_1437_autodrive_loop.sh` | The #1437 loop contract with review generations in place; the default case (`NOT_READY`, then `MERGE_READY`, ending `DONE`) also checks that `review-release` runs exactly once and that no round starts in the PR worktree or the main checkout. Failure cases are listed below. |

```bash
cargo test -p amplihack-cli --test main issue_2096
cargo test -p amplihack-cli --lib git_record
cargo test -p amplihack-git --test issue_2096_git_policy
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
bash tests/issue_1437_autodrive_loop.sh
```

### Review commands in the loop test double

`tests/issue_1437_autodrive_loop.sh` runs `autodrive_loop.sh` against the
`amplihack` test double `tests/issue_1437_amplihack_stub.sh`, not the real
CLI. The Lint & Format job has no built binary, and the test's fixture
repository has no origin or PR head for a real `review-allocate` to clone.
The double accepts exactly these five commands, with exactly these argument
counts:

| Command | Does | Prints | Exit |
| --- | --- | --- | --- |
| `internal review-allocate REPO STATE` | `git -C REPO worktree add --detach $STUB_STATE/review-gen-1` | that path | 0 |
| `internal review-next REPO CHECKOUT` | Adds the next detached worktree, `$STUB_STATE/review-gen-<n+1>` | the new path | 0 |
| `internal review-launch CHECKOUT` | Nothing | `CHECKOUT` | 0 |
| `internal review-head CHECKOUT` | Reads `git rev-parse --verify HEAD` in `CHECKOUT` | the SHA | git's status |
| `internal review-release CHECKOUT` | `git worktree remove --force` on each `$STUB_STATE/review-gen-*` it created, by literal name | nothing | 0 |

Each review generation is therefore a detached Git worktree, separate from
both the fixture's PR worktree (`$WT`) and its main checkout (`$MAIN`). The
stub's `recipe run` branch logs `round-workdir <path>` for each round's
`--working-dir`. The default case asserts that no `review-launch` argument and
no `round-workdir` equals `$WT` or `$MAIN`.

Each call appends one line to `$STUB_STATE/review-calls`, so the existing
counts in `$STUB_STATE/calls` are unchanged. `review-release` also increments
`$STUB_STATE/review-release`. Any other `internal` command, or a wrong
argument count, falls through to `unhandled` and exit 3.

These switches drive the loop's failure paths:

| Switch | Effect |
| --- | --- |
| `STUB_REVIEW_ALLOCATE_RC` | `review-allocate` exits with this status and prints nothing. |
| `STUB_REVIEW_NEXT_RC` | `review-next` exits with this status and prints nothing. |
| `STUB_REVIEW_LAUNCH_RC` | `review-launch` exits with this status and prints nothing. |
| `STUB_REVIEW_HEAD_RC` | `review-head` exits with this status and prints nothing. |
| `STUB_REVIEW_RELEASE_RC` | `review-release` exits with this status without counting a release. |
| `STUB_ROUND_DIRTIES_REVIEW=1` | A round writes `stub-dirt.txt` into the `review_dir` it was given, so the next `index-check` fails. |

The cases that use them:

| Case | Asserts |
| --- | --- |
| `ALLOCATE-FAILS-*` | No round runs and nothing is released; output has `"loop_result":"REVIEW_COMMAND_FAILED"`; exit 1. |
| `REVIEW-NEXT-FAILS-*`, `REVIEW-LAUNCH-FAILS-*`, `REVIEW-HEAD-FAILS-*` | `"loop_result":"REVIEW_COMMAND_FAILED"`, with the failing command in its reason; exit 1. |
| `RELEASE-FAILS-RC`, `RELEASE-FAILS-ATTEMPTED`, `RELEASE-FAILS-DONE` | The release is attempted once; `"loop_result":"DONE"` is still printed; exit 1. |
| `RELEASE-FAILS-KEEPS-79` | A terminal refusal with a failing release still exits 79. |
| `RELEASE-FAILS-KEEPS-75` | An orphaned round with a failing release still exits 75. |
| `REVIEW-DIR-CHANGED-*` | stderr holds the generation's `HEAD` and the porcelain line `?? stub-dirt.txt` before `"loop_result":"REVIEW_DIR_CHANGED"`. |

None of the cases uses a timer or a sleep.

The double cannot detect faults in real review isolation. The
`issue_2096_*` cargo tests above cover that.

## Limits

- A private repository isolates Git state. It is not an OS sandbox: a step
  can still write any path its user can write.
- The observer sees Git launched in the runner's process tree. It does not see
  Git that preparation and release run themselves, unrelated host processes,
  or non-Git writes to an index.
- Evidence files are not tamper-proof against the same user.
- Ignored files other than the recipe file are not copied. A recipe that needs
  one must take its path as a context value outside the checkout.
- `origin/*` is seeded only from refs the caller already has. A caller with an
  origin URL but no `origin/*` refs gets an empty `origin/*` until the recipe
  fetches.
- A relative `remote.origin.url` in the caller (for example `../upstream.git`)
  is copied as written and resolves against the private repository, not the
  caller. Use an absolute path or URL for such remotes.
- Off Linux, no Git command evidence is produced. `coverage: "unavailable"`
  records that fact; no gate accepts it as evidence.

## Related

- [Inspect a local-input snapshot](../tutorials/recipe-checkout-isolation.md)
- [Run a recipe](../howto/run-a-recipe.md#know-where-the-run-works)
- [Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md#what-happens-when-a-run-ends)
- [Recipe runner logging](recipe-runner-logging.md#git-command-record)
- [Auto-drive to merge](auto-drive-to-merge.md#review-generations)
- [Recipe runner architecture](../concepts/recipe-runner-architecture.md#the-runs-own-checkout)
