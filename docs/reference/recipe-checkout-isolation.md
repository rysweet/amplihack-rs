---
title: Recipe Run and Auto-Drive Checkout Isolation
description: Where recipe runs and auto-drive reviews execute, what they copy from the caller, and how they release what they own.
doc_type: reference
last_updated: 2026-11-17
---

# Recipe run and auto-drive checkout isolation

`amplihack recipe run` never runs a step in the caller's checkout. Each real
run copies the caller's committed, staged and working state into a private
repository and a detached checkout, runs every step, agent and hook there, and
removes the checkout when the run ends. Auto-drive judges each pull request in
a private checkout at the exact PR head commit it recorded. This is issue
#2096.

The private clone is the whole of the isolation. A run does not trace, journal
or attribute the Git commands it makes, and it needs no `ptrace` privilege on
any platform.

## Contents

- [What the run copies](#what-the-run-copies)
- [Where each kind of run executes](#where-each-kind-of-run-executes)
- [Paths and Git configuration](#paths-and-git-configuration)
  - [The origin remote in the private repository](#the-origin-remote-in-the-private-repository)
- [Independent, nested and resumed runs](#independent-nested-and-resumed-runs)
- [Ownership and release](#ownership-and-release)
- [Git commands a run makes](#git-commands-a-run-makes)
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
| Outside any Git checkout | The requested directory, as given. |
| Git missing, or `.git` unreadable or corrupt | Nowhere: preparation fails. |
| `--dry-run` | Nothing runs. The caller's directory is used for planning only. |

A dry run prepares no checkout and creates no run folder under the work root. It uses a temporary `amplihack-dry-*`
directory for scratch space and removes it on exit.

The runner gets `-C <exec dir>`. Steps, hooks and agents start there.

## Paths and Git configuration

Every Git call made during preparation and release uses a fixed argument list
and an environment with `GIT_DIR`, `GIT_COMMON_DIR`, `GIT_WORK_TREE`,
`GIT_INDEX_FILE`, object-directory overrides and `GIT_CONFIG_*` removed. Global
and system configuration, hooks, templates and clean/smudge filters are
disabled for those calls. The Git calls that provision, validate, retire and
release a review generation are built the same way.

That blanking covers amplihack's own commands only. The recipe runner, and
every step, hook and agent below it, read the user's own global and system Git
configuration, so a credential helper, `safe.directory` entry, alias or filter
kept in `~/.gitconfig` or `/etc/gitconfig` applies inside a run as it does
outside one.

A `user.name` and `user.email` in those files are read inside a run too, and
reach a `git commit` a step makes on its own account. They do not decide the
author of an amplihack-owned workflow commit: the commit-producing recipes
resolve an explicit identity before calling `git commit` and export
`GIT_AUTHOR_*` and `GIT_COMMITTER_*` for that step, and the repo-local branch
of that resolution reads `git config --local` only. See
[Configure workflow commit identity](../howto/configure-workflow-commit-identity.md).

The runner's environment still has the repository-selecting variables and the
inline configuration variables removed: every name in
`amplihack_git::REPOSITORY_ENV_VARS`, which is `GIT_DIR`, `GIT_WORK_TREE`,
`GIT_INDEX_FILE`, `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`,
`GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_NAMESPACE`, `GIT_CEILING_DIRECTORIES`,
`GIT_QUARANTINE_PATH`, `GIT_PREFIX`, `GIT_INDEX_VERSION` and
`GIT_DISCOVERY_ACROSS_FILESYSTEM`, together with `GIT_CONFIG`,
`GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS`, every `GIT_CONFIG_KEY_<n>` and
`GIT_CONFIG_VALUE_<n>`, `GIT_EXEC_PATH` and `GIT_TEMPLATE_DIR`. No inherited
variable can therefore send a step's Git commands back to the caller's
checkout.

The configuration half is narrower, and the run does not hold it.
`GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` are outside both lists and are not
removed, so an inherited value of either names the file a step's Git reads in
place of `~/.gitconfig` or `/etc/gitconfig`, and Git runs the `!command` forms
of `alias.*` and `credential.helper`, and a `core.hooksPath` hook, that it
finds there; an inherited `GIT_CONFIG_NOSYSTEM` likewise still suppresses
`/etc/gitconfig`. The environment is not a boundary a run holds in any case:
`GIT_SSH_COMMAND`, `GIT_ASKPASS`, `HOME` and `PATH` are inherited too, and each
of those also names something a step's Git will execute. What the removals
above protect is the checkout rather than the configuration; a `core.worktree`
reached through a file named that way does not move a step out of its private
checkout, because `GIT_DIR` is gone and discovery starts from the execution
directory. Hooks that steps trigger, such
as pre-commit and pre-push, run normally inside the execution checkout.

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

Preparation needs Git 2.29 or later: the `^refs/remotes/origin/HEAD` negative
refspec and `--no-auto-maintenance` were both added in 2.29. With an older Git
the fetch fails and preparation stops; it does not fall back to a fetch that
would copy `origin/HEAD`. The worktree sweep already needs Git 2.31
(`rev-parse --path-format=absolute`).

Only one of the two cases in the table above prints a warning. A plain
`origin/HEAD` in the caller is dropped silently: a plain ref is never a valid
default-branch pointer, and the recipes already handle a missing
`origin/HEAD`. A symbolic `origin/HEAD` whose target
is outside `refs/remotes/origin/` gets a warning, because only the caller's
own configuration can produce it and the caller should fix it.

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

The private repository is bare, so `workflow_worktree_root.sh root` gives the
folder beside it (the run's storage folder, `<work root>/exec/<hash>`) as the
main root, and that folder is not a repository. The worktree helpers therefore
ask Git through the checkout, which shares the same refs, configuration and
worktree list: `workflow_worktree_root.sh base` is given the checkout,
`assert-not-nested` lists worktrees from the current repository when the main
root is not one, and `workflow_caller_branch_alias.sh` runs its Git commands in
the caller path.

A `step-04-setup-worktree` from before #2290, for example one found first in
the caller's own `amplifier-bundle/`, passes the `root` answer back to the
helper (`base "$MAIN_REPO_PATH"`). The helper accepts that: when the folder it
is given is not a work tree, but it is exactly the `root` answer for the
checkout named by `AMPLIHACK_RUN_CHECKOUT`, `root` prints that folder as its
own root and `base` resolves as it would from the checkout. Both print the
same path under `<work root>/worktrees`:

```sh
# In a step of a new-branch run started with
#   amplihack recipe run smart-orchestrator -w ~/src/amplihack-rs \
#     -c repo_path=. -c issue_number=2677 -c force_single_workstream=true
h=amplifier-bundle/tools/workflow_worktree_root.sh
main=$("$h" root .)          # <work root>/exec/<hash>
"$h" base .                  # <work root>/worktrees/amplihack-rs-<hash8>
"$h" base "$main"            # the same path
```

Any other folder that is not a work tree still fails with
`'<path>' is not inside a git work tree.`, whether or not
`AMPLIHACK_RUN_CHECKOUT` is set.

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
another run, gets its own repository, checkout, index and owner record. An inherited `AMPLIHACK_RUN_CHECKOUT` is ignored with a message:

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

The run directories are created at `0700`, and `owner.json` and the recovery
descriptor at `0600`, when they are created rather than by a later `chmod`.

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
2. The recovery admission is finished with an outcome judged from that result.
3. `finish_workspace` writes `run.json` and runs the cleanup sweep.

Each step runs even when an earlier one failed. Errors are joined with
`.context()`, so the final error names every failure. A failure before the
first step (in `prepare_with_inputs`, or in path mapping) still finishes the
recovery admission, with `Failure`, unless the run was stopped by a signal. The next launch therefore does not resume a run that
never started.

Copied inputs, which may include `.env` files, live only in the `0700`
`exec dir` and go with it. Error messages name paths, never file contents or
environment values.

## Git commands a run makes

Nothing traces, journals or attributes the Git commands a run's runner, steps,
agents and hooks launch. A run folder holds no `git-commands.jsonl`, no
`git-trace2/` directory, no `git-recording.json` and no `diagnostics/git/`
directory, and `run.json` carries no `git_recording` field.

A run therefore starts no tracer and needs no `ptrace` privilege. It works the
same on Linux, on macOS, in a container that denies `ptrace`, under a debugger,
and inside a step of another recipe run.

What keeps a run out of the caller's checkout is the private clone described
above: every step starts in `<run dir>/execution-<id>/checkout`, the caller's
repository is read only during preparation, and a Git command a step makes acts
on the private repository because that is the only checkout it is given. A
command that runs outside any repository, such as `git clone` or
`git ls-remote`, is ordinary work and has no effect on the run.

Caller `GIT_TRACE2*` variables are cleared from the runner's environment, so
a trace target set outside a run cannot collect the run's Git traffic; see
[Caller Git trace variables are cleared](recipe-runner-logging.md#caller-git-trace-variables-are-cleared)
in the logging reference.

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

1. It checks the device and inode of the old storage, its `repository.git`,
   its checkout and its launch directory against the pairs the manifest
   recorded for them. A review generation manifest records four pairs, in
   this order: the storage directory, `repository.git`, the checkout and
   `launch`. All four are checked here, before anything is removed, so every
   resource the storage removal in step 4 will require has been checked
   while the checkout is still intact. Were `launch` left out, a generation
   whose launch directory had changed would lose its checkout in step 2 and
   then be refused in step 4, leaking the storage and reporting a changed
   identity for a tree step 1 had just accepted. `review-next` also reads
   the generation as live before it provisions the successor, and reading a
   live generation compares all four pairs, so on that path a changed launch
   directory stops the call before retirement starts;
   `advance_destroys_nothing_when_the_recorded_launch_directory_is_replaced`
   pins that. The check in step 1 covers the window between that read and
   the worktree removal, and refuses with the same message the read gives,
   `review generation resource identity changed; retained for recovery`.
2. It runs `git worktree remove --force` on the old checkout.
3. It sets `"retired": true` in the old manifest and writes it atomically.
4. It removes the old storage, only when the path still exists, is a
   directory and not a symlink, carries the device and inode the manifest
   recorded for the storage directory, and still holds `repository.git` and
   `launch` at the device and inode recorded for each of those two. An absent
   storage path counts as already freed. Anything else keeps the directory
   and fails with `review storage identity changed; retained: <path>`,
   including a recorded child that is absent, is a symlink, is no longer a
   directory, or cannot be read. Every path is examined without following a
   symlink, and being a symlink is itself a refusal, so a link that points at
   the recorded directory does not pass.

Storage removal compares three recorded pairs, not just the storage
directory's own, because one pair is a weak answer to "is this still the
directory I made?". `fs::remove_dir_all` frees an inode, and the filesystem
may hand that same inode straight back to the next directory created at that
path, so a directory that has nothing to do with the generation can carry the
single pair the manifest recorded. A directory made after the removal does not
also hold `repository.git` and `launch` at their recorded inodes, so the
three-pair check separates the two cases without depending on whether the
filesystem reused anything. The three are required together: storage whose
own pair and `repository.git` both match, but whose `launch` is gone, is
refused like any other mismatch, because a tree that is only partly the
recorded one is not the recorded one. That case is
`retired_release_refuses_storage_whose_launch_child_is_gone`, which is
`retired_release_frees_storage_the_removal_never_reached` with `launch`
removed and nothing else changed, so the storage directory and
`repository.git` still carry the identities the manifest recorded for them;
release refuses it and leaves `repository.git` on disk.

The paths compared are not read from the manifest as free text, and the
removal does not rely on its caller having checked them. Loading a manifest
pins `repository.git` and `launch` to `<storage>/repository.git` and
`<storage>/launch`, confines the storage directory to the checkout's own
allocation directory, requires its name to begin with `review-generation-`,
and refuses the manifest otherwise. The removal then repeats that work on the
record it is handed: it derives the storage path from the record rather than
accepting one as an argument, and re-checks the containment, the name and both
child paths itself before it removes anything.

The repetition is not redundant. Retirement and release are not the only
sources of a generation: provisioning builds one in memory and writes it out
without ever reading it back through the manifest loader, so a removal that
trusted its caller would be checked on one path into it and unchecked on the
other. Deriving and re-checking at removal time makes the guarantee a property
of the removal rather than of how the record happened to arrive.

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
  storage if any is left, under the same three-pair check as step 4 of
  retirement: a retired record skips the four-way identity check on read that
  a live record gets, so those three pairs are the only pairs that authorise
  the storage removal. A mismatch, or a recorded child that is gone, stops
  release with `review storage identity changed; retained`. Other recorded
  pairs are still compared for other purposes on the same call: each child's
  own manifest is read on the recursive walk, and the allocation parent's
  recorded pair is checked before its directory is removed.
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

Isolation cannot be switched off.

| Setting | Effect |
| --- | --- |
| `-w`, `--working-dir` | Source checkout and the relative directory steps start in. Also the run's recovery identity. |
| `-c repo_path=...` | Mapped into the execution checkout when it is a caller-checkout path. |
| `--context-file`, `--context-from-file` | Read before launch; their contents are context values and follow the same mapping rules. |
| `--dry-run` | Plans only. No checkout and no run folder. |
| `AMPLIHACK_WORK_ROOT` | Where run folders and execution storage go; see [cleanup settings](../howto/clean-up-after-recipe-runs.md#settings). |
| `RECIPE_RUNNER_RS_PATH` | Runner binary. It starts in the private checkout like any step. |
| `AMPLIHACK_RUN_CHECKOUT` | Set for steps. An inherited value is ignored and grants no ownership. |
| `AMPLIHACK_CLEANUP_ON_EXIT=0` | Skips the cleanup sweep only. Release still runs. |
| Caller `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_NOSYSTEM` | Inherited by the runner and by steps. Before #2756 the run pinned all three, which is what broke the user's credential helper. They are now left alone, so a caller that sets one chooses the configuration a step's Git reads; see [paths and Git configuration](#paths-and-git-configuration). They do not move a step out of its private checkout. |
| Caller `GIT_TRACE2_*` | Cleared from the runner's environment, so they reach neither the runner nor the Git processes a run starts. amplihack sets none of them and reads none of them. |
| The user's `~/.gitconfig` and `/etc/gitconfig` | Read by the runner and by every step, hook and agent, as by any other Git command that user runs. amplihack itself writes neither file, and its own preparation, release and review-generation commands do not read them. Nothing stops a step from writing one: a `git config --global` in a step now edits the user's real `~/.gitconfig`, where between #2290 and #2756 it was redirected to `/dev/null`. That is not a new privilege, since a step runs as the user and could already append to the file directly, but it is a change of behaviour; no shipped recipe does it. A `user.name` there does not attribute an amplihack-owned workflow commit; see [Configure workflow commit identity](../howto/configure-workflow-commit-identity.md). |

## Rust components

These are internal modules, not a public API.

| Component | Responsibility |
| --- | --- |
| `run_isolation/input_snapshot.rs` | Captures `HEAD`, index and working tree separately; rejects unsafe inputs. |
| `commands/recipe/run/run_checkout.rs` | Finds the repository (fails closed), provisions the private checkout, rebuilds the `origin` refspec, `origin/*` and `origin/HEAD`, maps paths, releases owned paths. |
| `run_isolation/review_generation.rs` | Review generations: provision, four-part validation (including a clean working tree), retirement of superseded generations in `advance`, release. `read_live` refuses retired generations; plain `read` is used only by `release`, `release_state` and the child walk. |
| `run_isolation/exec_storage.rs` | Claims `<work root>/exec/<identity>/` and decides when it may be reclaimed; used by preparation and by `amplihack cleanup`. |
| `commands/cleanup/run_dir.rs` | `RunWorkspace::begin_durable` for real runs, `RunWorkspace::dry_runtime` for dry runs; `record_attempt_checked` and `finish_checked` return errors. |
| `commands/recipe/run/execute.rs` | Order of preparation, launch and release; joins errors. |

Callers use `?` on every `Result` from checkout, admission and workspace
code. None of them is discarded with `.ok()`, `let _ =` or
`unwrap_or_default`.

## Failure behavior

| Failure | Result |
| --- | --- |
| Unsafe input, unborn `HEAD`, bare or malformed repository | Preparation fails naming the path; no step runs; admission finished as `Failure`. |
| Runner cannot start (missing binary or folder, or another spawn error) | The run fails with `failed to spawn recipe-runner-rs` and the spawn error number. |
| Seeding `origin/*` (for example an `origin/*` ref whose objects are missing from the caller's repository) or copying `origin/HEAD` fails | Preparation fails with `cannot seed origin refs` and `partial resources retained at <exec dir>`; no step runs. |
| Checkout identity changed at release | Resources kept, run fails. |
| Review dir fails a check | `REVIEW_DIR_CHANGED` or `REVIEW_DIR_UNSTAMPED`; no judging. |
| Review checkout has edited or untracked files | `review-validate` and `index-check` refuse it, naming the first changed path. |
| Retiring a superseded review generation fails | One warning; the new generation is used and the old one is freed by `review-release` at loop exit. |
| A command is given a retired review generation | Refused: `review generation is superseded and retired`. |
| A live review generation's recorded resource is a different directory: its storage directory, `repository.git`, its checkout or `launch` no longer carries the device and inode the manifest recorded for it | The command that read it is refused with `review generation resource identity changed; retained for recovery`. Nothing is removed, the record stays live with `"retired": false`, and the checkout and storage are left in place. `review-next` stops here rather than retiring the generation it would have replaced. A recorded path that is absent, is a symlink or is no longer a directory is refused earlier, by `review resource is not an owned directory: <path>` or the underlying I/O error. |
| A storage path is no longer the recorded directory: a symlink, a non-directory, a different directory, or a directory missing one of the two children the manifest recorded inside it | Nothing is deleted. `review-next` warns and carries on; `review-release` fails with `review storage identity changed; retained` and the path. |
| Stable execution path still held | The attempt prints the reason and uses a fresh random path. |
| Every step passed but release failed | The run fails. |

Output formats keep their existing fields. The run's status follows the
runner's own exit; a zero exit still fails the run when release fails.

## Platform support

Real recipe runs work on every platform, and checkout isolation is the same on
each. Nothing in a run depends on the build target or on a kernel facility.

| Platform | Steps run in |
| --- | --- |
| Linux | The private checkout |
| macOS and every other target | The private checkout |

A run needs no `ptrace` privilege, so it works where `ptrace` is denied: a
container with `CAP_SYS_PTRACE` dropped or a `seccomp` filter that blocks the
call, a host with Yama `ptrace_scope` at `2` or `3`, and a step of another
recipe run.

`--dry-run`, `recipe list`, `recipe show` and `recipe validate` work on every
platform, and the crate builds on macOS. Auto-drive is still unavailable on
macOS (see
[#1646](https://github.com/rysweet/amplihack-rs/issues/1646)), which is a
separate matter from where a run executes.

## Tests

The cargo tests run the real CLI against temporary repositories, with temporary
`HOME`, `AMPLIHACK_HOME`, profile root and `TMPDIR`. They never read or write
the real `~/.claude`, `~/.amplihack` or profile root. Concurrency uses socket
handshakes, readiness files and child exits; there are no sleeps, timers or
elapsed-time checks. Test helpers fail on any I/O error or non-zero exit, so an
isolation test cannot pass on empty data.

| Test file | Covers |
| --- | --- |
| `crates/amplihack-cli/tests/issue_2096_checkout_units.rs` | Snapshot layers, byte paths, symlinks, special files, batch blob query. |
| `crates/amplihack-cli/tests/issue_2096_execution_isolation.rs` | Caller `HEAD` and index unchanged; dry run makes no checkout and no run folder; resumed run gets a fresh `exec dir`; unborn caller finishes admission as `Failure`. |
| `crates/amplihack-cli/tests/issue_2756_user_git_config.rs` | A run whose temporary `HOME` holds a `.gitconfig` with a sentinel `user.name` and a URL-scoped credential helper. The runner is a stub shell script the test writes itself and wires in through `RECIPE_RUNNER_RS_PATH`, so what the test pins is the environment the CLI hands the runner command and what Git does in it: the sentinel `user.name` is the one reported, the helper script is called, and `git credential fill` returns its user name and password with its marker file written. That the steps below the runner see the same configuration follows from the runner passing its own environment down, which this test does not itself exercise. The same run started with `GIT_DIR` and `GIT_INDEX_FILE` pointing at a second repository works in its private checkout and leaves that repository's `HEAD`, index and working tree byte-identical. |
| `crates/amplihack-cli/tests/issue_2096_private_base_ref.rs` | Base resolution inside a private checkout made by `RunCheckout::prepare`, whose caller has an origin: the `resolve_base_ref` bodies taken from `workflow-worktree.yaml`, `consensus-issue-worktree.yaml` and `workflow-terminal-state.yaml`, and `workflow_delivery_inputs.sh worktree main false "" ""`, all without a prior fetch; `git fetch origin` in the private repository moves `origin/main`; a caller without an origin gets no refspec; a foreign `origin/HEAD` target is refused while preparation still succeeds; a plain caller `origin/HEAD` is not copied. |
| `crates/amplihack-cli/tests/issue_2738_no_ptrace.rs` (Linux) | A real run's runner and every process in its tree report `TracerPid: 0` in `/proc/<pid>/status` while the run is in flight, and a run whose release succeeds reports the runner's own exit status. |
| `crates/amplihack-cli/tests/issue_2738_git_outside_repo.rs` | A `git clone` and a `git ls-remote` against a tempfile bare repository, run outside any repository while a run is in flight, both succeed and leave the run's outcome unchanged. |
| `crates/amplihack-cli/tests/issue_2738_drive_shaped_layout.rs` | A clone holding a `worktrees/pr-N` worktree runs to a terminal outcome, and `git -C <main root> worktree list` still shows the worktree. |
| `crates/amplihack-cli/tests/issue_2738_private_clone.rs` | A step that writes to the index works in a private clone whose path differs from the caller's checkout; the caller's index and working tree are unchanged. |
| `crates/amplihack-cli/tests/issue_2677_private_main_root.rs`, module `private_main_root` of `issue_2096_execution_isolation` | A new-branch run launched from the caller's main checkout with `-c repo_path=.` and an issue number: `workflow_worktree_root.sh root` names the run's storage folder, `base` resolves from it to the same worktree path as from the checkout, and a folder that is not a work tree and not that answer still fails. |
| `crates/amplihack-cli/tests/issue_2096_review_isolation.rs` | Review generations and both call sites of the three-pair storage check. `advance_retires_superseded_generation` covers the retirement call site: after `advance` the old checkout and storage are gone and the manifest says `retired`. `release_frees_storage_left_by_failed_retirement` covers the release call site, where a retirement that did not reach the removal leaves storage behind. `advance_destroys_nothing_when_the_recorded_launch_directory_is_replaced` covers the fourth recorded pair on the retirement path: a generation whose recorded `launch` directory was replaced keeps its checkout, its storage and `retired: false`, because the read that precedes retirement compares all four pairs and refuses first. `retired_generation_is_refused` covers the commands that refuse a retired generation, and `release_root_after_retirement_removes_everything` the release of the whole chain. On the release path, `retired_release_refuses_symlinked_storage` and `retired_release_refuses_substituted_storage` cover the storage directory's own identity, and five tests cover the recorded children: storage whose recorded children are absent, whose recorded children were replaced by a tree of the same shape, whose recorded child is a symlink, and whose `launch` alone is gone while the other two pairs still match (`retired_release_refuses_storage_whose_launch_child_is_gone`), are each refused with nothing deleted, while `retired_release_frees_storage_the_removal_never_reached` holds the opposite side, that a whole recorded tree a failed retirement left behind is still freed. The substitution tests do not wait for the kernel to reuse an inode: they give the substitute the recorded top-level pair outright, which is the most inode reuse can hand it. |
| `crates/amplihack-cli/tests/issue_2096_real_hooks.rs` | Pre-commit and push hooks run in the execution checkout. |
| `crates/amplihack-git/tests/issue_2096_git_policy.rs` | Git environment and configuration policy. |
| `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` | Review generation checks, `REVIEW_DIR_CHANGED` and `REVIEW_DIR_UNSTAMPED`. |
| `tests/issue_1437_autodrive_loop.sh` | The #1437 loop contract with review generations in place; the default case (`NOT_READY`, then `MERGE_READY`, ending `DONE`) also checks that `review-release` runs exactly once and that no round starts in the PR worktree or the main checkout. Failure cases are listed below. |

Every file under `crates/amplihack-cli/tests/` is a module of one test
binary. The crate sets `autotests = false`, so a test file is compiled only
when a `#[path = "..."] mod ...;` line names it: the `issue_2738_*.rs` files
and `issue_2756_user_git_config.rs` are wired into
`crates/amplihack-cli/tests/main.rs`, and the `issue_2096_*` files into
`tests/issue_2096_execution_isolation.rs`. A file left out of both is silently
never built. Select them by module path within the one `cli_integration`
target:

```bash
cargo test -p amplihack-cli --test cli_integration issue_2096_execution_isolation::
cargo test -p amplihack-cli --test cli_integration issue_2738_
cargo test -p amplihack-cli --test cli_integration issue_2756_user_git_config::
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
- The runner's command sets no `GIT_TERMINAL_PROMPT`, so Git inside a run
  behaves as it does in the user's shell when no helper supplies a credential:
  it opens the terminal and asks. A detached run has nobody to answer, so an
  https push or fetch from a step can stop there instead of failing, which is
  the symptom described under
  [a shell step hangs waiting for input](../howto/troubleshoot-recipe-execution.md#shell-step-hangs-waiting-for-input).
  Configure a credential helper that can answer without a terminal. Setting
  the variable for the run is deferred.
- No record is kept of the Git commands a run makes. A run is isolated by the
  clone it works in, not by an account of what it did, so there is nothing to
  read afterwards to find out which commands ran.
- Identity checks resolve a path by name, and so does the delete that
  follows. A path swapped between the last check and the delete is not caught.
  The window is narrow, because the recursive delete walks the tree with
  `openat` and `unlinkat` and follows no symlink below the top, but it is
  there. Closing it needs the directory to be opened once, with `O_DIRECTORY`
  and `O_NOFOLLOW`, and every later operation done through that descriptor.
  This too is residual risk on
  [#2701](https://github.com/rysweet/amplihack-rs/issues/2701).
- Storage whose removal stopped halfway is kept, not finished. If a delete
  removed `repository.git` or `launch` and then failed, the recorded child is
  gone, so every later attempt reports `review storage identity changed;
  retained` and leaves the rest on disk for a person to look at and remove.
  The alternative, treating a missing child as permission to delete, is the
  case this check exists to refuse.
- The allocation directory that holds a generation's storage is not itself
  checked for owner, or for group and world writability, as the review state
  directory is. A user who can write that directory can put something at the
  storage path, and can create and delete directories there to groom inode
  reuse. What they have no practical way to do is make three specific recorded
  pairs line up at once, so the expected outcome is a refusal rather than a
  deletion. Extending the state directory's owner and mode check to the
  allocation directory would close the gap. It is recorded as residual risk on
  [#2701](https://github.com/rysweet/amplihack-rs/issues/2701), the issue the
  three-pair check answers.
- Ignored files other than the recipe file are not copied. A recipe that needs
  one must take its path as a context value outside the checkout.
- `origin/*` is seeded only from refs the caller already has. A caller with an
  origin URL but no `origin/*` refs gets an empty `origin/*` until the recipe
  fetches.
- A relative `remote.origin.url` in the caller (for example `../upstream.git`)
  is copied as written and resolves against the private repository, not the
  caller. Use an absolute path or URL for such remotes.

## Related

- [Inspect a local-input snapshot](../tutorials/recipe-checkout-isolation.md)
- [Run a recipe](../howto/run-a-recipe.md#know-where-the-run-works)
- [Clean up after recipe runs](../howto/clean-up-after-recipe-runs.md#what-happens-when-a-run-ends)
- [Recipe runner logging](recipe-runner-logging.md#git-commands-a-run-makes)
- [Auto-drive to merge](auto-drive-to-merge.md#review-generations)
- [Recipe runner architecture](../concepts/recipe-runner-architecture.md#the-runs-own-checkout)
