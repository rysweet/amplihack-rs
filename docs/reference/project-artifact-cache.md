# Per-Project Artifact Cache — Reference

Code-index artifacts for a project — SCIP indexes, the `blarify.json` import
file, the code-graph store, the staleness marker, and the background-indexing
PID file — live in a per-project cache directory **outside** the indexed
checkout. Indexing a repository leaves that repository byte-for-byte unchanged.

## Contents

- [Why the artifacts are not in your repository](#why-the-artifacts-are-not-in-your-repository)
- [Cache layout](#cache-layout)
- [Resolution order](#resolution-order)
- [Directory permissions](#directory-permissions)
- [The `project` pointer file](#the-project-pointer-file)
- [SCIP indexer output paths](#scip-indexer-output-paths)
- [Migration out of an in-repo layout](#migration-out-of-an-in-repo-layout)
- [The staleness marker](#the-staleness-marker)
- [API](#api)
- [Interaction with Artifact Guard](#interaction-with-artifact-guard)
- [What the relocation changes for confidentiality](#what-the-relocation-changes-for-confidentiality)
- [Cache growth and reclaiming space](#cache-growth-and-reclaiming-space)
- [Out of scope](#out-of-scope)
- [Verifying the invariant](#verifying-the-invariant)
- [Related](#related)

## Why the artifacts are not in your repository

Indexing used to write into the repository it indexed. Inside `amplihack-rs`
that is invisible, because this repository's own `.gitignore` lists both
offenders:

```sh
$ grep -n 'index.scip\|\.amplihack' .gitignore
30:index.scip
31:.amplihack/
33:observability/litellm/.amplihack-api-key*
```

No other repository has those entries. In `rysweet/amplihack-recipe-runner` a
routine `git add -A` therefore staged an 8&nbsp;MB `graph_db` binary along with
the rest of the generated tree. The user had asked amplihack to index their
code; amplihack answered by putting a build artifact into their commit. The
asymmetry is the whole reason the bug survived: from inside `amplihack-rs` the
artifacts were always being written into the checkout, and a local ignore rule
hid it.

A single indexing run produces these files. Every one of them now lands in the
cache directory:

| Artifact | What it is | Typical size |
| --- | --- | --- |
| `graph_db` | LadybugDB code-graph store (a single file) | 8–10 MB |
| `kuzu_db` | Legacy code-graph store, where one was migrated | 8–10 MB |
| `indexes/<language>.scip` | Per-language SCIP index | 1–50 MB |
| `index.scip` | Single-file index: where a migrated repo-root `index.scip` lands, and the last-resort path staleness reports on | 1–50 MB |
| `blarify.json` | Import input for `amplihack index-code` | KB–MB |
| `blarify_stale` | Staleness marker, written on the first code edit of a session | bytes |
| `indexing.pid` | Background-indexing lock | bytes |

Nothing on that list is transient. They are the point of indexing and they
persist between runs — which is why "clean up afterwards" was never an adequate
answer, and why the destination had to move instead.

## Cache layout

```
${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/projects/<slug>/
├── project                  # canonical project path, one line
├── graph_db                 # code-graph store
├── kuzu_db                  # legacy store, only if migrated from one
├── indexes/
│   ├── python.scip
│   └── rust.scip
├── index.scip               # migrated repo-root index; staleness fallback
├── blarify.json
├── blarify_stale
├── indexing.pid
└── .migration.lock          # fs4 advisory lock, migration only
```

`<slug>` is `<sanitised basename>-<sha256(canonical project path)[..16]>`, so it
is both readable and collision-resistant:

```
/home/user/src/myproject  ->  myproject-3f9c1ad7b2e40561
```

The hash covers the **canonicalised absolute** path, so a symlink to a checkout
and the checkout itself share one cache entry, while two checkouts with the same
basename — `worktrees/feat-a/myproject` and `worktrees/feat-b/myproject` — do
not. A project path that cannot be canonicalised (it does not exist yet) is
absolutised instead, which is still never cwd-relative.

Two details of the hash are deliberate:

- **The hash input is the raw `OsStr` bytes of the canonical path**, not
  `to_string_lossy()`. Lossy conversion maps every invalid UTF-8 byte to the
  same `U+FFFD`, so two distinct non-UTF-8 paths can hash identically and
  collide into one cache entry — one project silently reading and overwriting
  another's index. This is the one place the existing precedent is **not**
  copied: `consent_cache_path()`
  (`crates/amplihack-cli/src/commands/launch/blarify.rs`) does hash
  `to_string_lossy()`. It is intentionally left alone and intentionally not
  unified with `project_slug()`: changing it would invalidate every user's
  stored consent and re-prompt them, and a consent-file collision is a much
  smaller problem than an index collision.
- **Truncating to 16 hex digits (64 bits) is accepted.** These are local
  filesystem paths derived from paths the user already controls, not a security
  boundary; nobody is choosing project paths to force a collision. 64 bits
  matches the existing fingerprint precedent in
  `crates/amplihack-hooks/src/issue_dedup.rs`, and a 64-character directory name
  would make the cache unreadable for the humans who have to debug it.

## Resolution order

| Order | Source | Result |
| --- | --- | --- |
| 1 | `AMPLIHACK_ARTIFACT_DIR` | Used directly as the artifact dir |
| 2 | `XDG_CACHE_HOME` | `$XDG_CACHE_HOME/amplihack/projects/<slug>` |
| 3 | `HOME` | `$HOME/.cache/amplihack/projects/<slug>` |
| 4 | none of the above | Error |

Row 4 is the fix. Resolution **fails** rather than returning a project-relative
or cwd-relative path, because a fallback into the repository would reintroduce
the bug on exactly the systems where it is hardest to notice.

### Validation

`validate_env_dir_path(var_name, path)` is applied to all three inputs, not just
the override: a mistaken `XDG_CACHE_HOME` or `HOME` is the same class of problem
as a bad `AMPLIHACK_ARTIFACT_DIR`. The `var_name` is carried so the rejection
names the variable the user actually set rather than the subsystem that read it.
Shared checks: absolute, no `..` component, not under `/proc`, `/sys`, or
`/dev`.

`AMPLIHACK_ARTIFACT_DIR` gets four further rejections, because it is used
*directly* as the artifact dir rather than having `amplihack/projects/<slug>/`
appended:

| Rejected value | Why |
| --- | --- |
| `/` | Migration and permission repair would be scoped to the filesystem root |
| `$HOME` | Same, scoped to the user's entire home directory |
| `/tmp`, `/var/tmp` | World-writable; a pre-created `<slug>` directory owned by another user is a straightforward index-poisoning path |

A directory the user supplies is also checked for **loose permissions**.
amplihack does not re-`chmod` a directory it did not create, so an override
keeps whatever mode it has — group or other bits produce a warning naming the
mode, and a **world-writable** override is refused outright. Silently accepting
`0o777` would hand any local user a searchable index of the user's source.

The two failure modes are **asymmetric**, and the asymmetry is intentional:

- An invalid `XDG_CACHE_HOME` produces a `tracing::warn!` and **falls through
  to `HOME`**. The user very likely did not set it for amplihack's benefit, and
  a usable fallback exists.
- An invalid `AMPLIHACK_ARTIFACT_DIR` is an **error**. The user set it
  deliberately, for this tool; ignoring it and writing somewhere else is worse
  than stopping.

See [Environment Variables](environment-variables.md#amplihack_artifact_dir).

### Inheritance hazard

`AMPLIHACK_ARTIFACT_DIR` names one project's directory, and the environment
builder exports it to every child process. Agents in this repository run in git
worktrees, so a child launched for project B would otherwise inherit project A's
artifact dir and write B's index into A's cache.

The **primary** defence is (1); (2) is a backstop that does not always fire:

1. `EnvBuilder::with_project_artifact_dir(project_root)` re-resolves the
   directory for the project being launched and **overwrites** any inherited
   value — the same contract `with_project_graph_db` carried for issue #250.
   Every launch that goes through the builder is correct by construction.
2. If `AMPLIHACK_ARTIFACT_DIR` names a directory whose `project` pointer file
   records a *different* canonical project path, the override is ignored with a
   `tracing::warn!` and resolution falls through to `XDG_CACHE_HOME`.

**A residual gap remains.** Check (2) cannot fire before a pointer file exists,
and a fresh cache entry has no pointer — so a hand-set `AMPLIHACK_ARTIFACT_DIR`
naming an empty directory is accepted. It closes the case where project A has
already been indexed, which is the common one. A process launched outside the
builder — a user running `amplihack index-scip` directly in an inherited shell —
can still write project B's index into a directory named for project A.
Accepted, and recorded here rather than papered over.

## Directory permissions

The cache holds a searchable index of the user's source code. On a shared host
the `0o755` that `create_dir_all` produces under a typical `umask` would make
every indexed project world-readable.

- Every directory amplihack creates — `<cache>/amplihack`, `.../projects`,
  `.../projects/<slug>`, `.../indexes` — is created with
  `DirBuilder::mode(0o700)` under `cfg(unix)`, at create time. There is never a
  umask-width window followed by a `chmod`.
- **`create_dir_all` does not re-tighten an existing directory.** A
  `~/.cache/amplihack` left at `0o755` by an earlier version keeps those bits
  forever, so resolution inspects the directories amplihack owns and repairs
  loose modes, logging the repair. Permissions are only ever narrowed.
- **amplihack never `chmod`s a directory it did not create.** `~/.cache` and
  `$HOME` belong to the user and to other tools; tightening them would break
  unrelated software. The repair rule is scoped to `amplihack/` and below.
- After creating or repairing, the result is verified with `symlink_metadata`
  (not `metadata` — the distinction is the point): a real directory, not a
  symlink; `uid` matching the current user; mode `0o700`. A pre-existing entry
  that fails any of these is an error, not something to fix in place. It is the
  shape a planted symlink takes.
- The `project` pointer file is written with `create_new` and mode `0o600`, via
  a temporary name carrying a **random** suffix rather than the process id.
  `create_new` turns a pre-planted file into an error instead of a truncation,
  and a predictable temp name in a directory an attacker can reach is the
  classic symlink race.
- Pointer reads are **bounded to 4&nbsp;KiB** and validated before use. The file
  is one line of path; refusing to read more means a hostile or corrupt cache
  entry cannot be turned into an allocation.

### Consent gate

Code-graph indexing is gated on explicit user consent. That gate is unchanged
and is not moved earlier. `ensure_artifact_root()` may create an **empty**
`0o700` directory before consent is resolved — an empty directory discloses
nothing beyond a path the user chose — but **no index, no `blarify.json`, and no
graph store is written before the existing consent check passes**. Migration is
not a second un-gated write path either: it moves only artifacts the user
already consented to producing.

## The `project` pointer file

The artifact dir contains a `project` file holding the canonical project path,
written atomically when the dir is created. It exists because one path lookup
runs backwards: `project_root_for_blarify_input()` used to recover a project root
by walking **two** parents up from `<project>/.amplihack/blarify.json`. Under the
slug layout that arithmetic can never work — the cache directory name is a hash,
not a repository.

The reverse lookup reads the pointer instead: one parent hop from
`<artifact_root>/blarify.json`, then `project_for_artifact_dir()`. It returns
`Result<Option<PathBuf>>` — an owned path, because the answer no longer borrows
from the input, and `Result` because a missing or unreadable pointer is reported
rather than swallowed.

That last part matters more than it looks. The old failure path fell back to
**the current working directory**, so `amplihack index-code <blarify.json>` with
an unrecognised input path created a graph database inside whichever repository
the user happened to be standing in: the same bug, relocated. The pointer file
is what makes the correct answer available, and the cwd fallback is gone.

## SCIP indexer output paths

Every artifact-relocation scheme depends on the indexer writing where it is
told, and the flags are not uniform. Each language arm names its destination
explicitly; none relies on the convention that an indexer drops `index.scip`
into the current directory.

| Language | Invocation | Tier |
| --- | --- | --- |
| `python` | `scip-python index --output <out>` | 1 |
| `typescript`, `javascript` | `scip-typescript index --output <out>` | 1 |
| `go` | `scip-go --output <out>` | 1 |
| `csharp` | `scip-dotnet index --output <out>` | 1 |
| `cpp` | `scip-clang --index-output-path <out>` | 1 |
| `rust` | `rust-analyzer scip <project>`, run from a staging dir beside `<out>` | 2 |
| anything else | skipped with a named reason | 3 |

`<out>` is always `<artifact_dir>/indexes/<language>.scip`. The three tiers:

| Tier | Condition | Behaviour |
| --- | --- | --- |
| 1 | The tool takes an output path | Pass it |
| 2 | No output flag, but the project root is an argument | Run with `current_dir` set to a staging directory beside the final artifact — already outside the checkout — and move the result into place |
| 3 | Neither | Report the language through the existing `skipped_languages` channel with a named reason, and **do not** index |

Tier 2 exists because "write into the repo, then move it out" is not available
to a tool that infers its project root from `current_dir`: moving the cwd out of
the checkout would change what gets indexed. `rust-analyzer scip` takes the root
as an argument, so the working directory is free to move instead.

Tier 3 is `ScipOutput::Unsupported { reason }`, and it is a real constructed
value rather than a placeholder: an unknown language resolves to it and is
skipped loudly. That is the structural guarantee — discovering a tool without an
output flag cannot quietly reintroduce an in-repo write while a stub-based test
stays green. Empty argv is unrepresentable at the call site (`split_first`), so
a plan that names no binary cannot panic.

No indexer arm writes `<artifact_root>/index.scip`. Tier 2 stages inside
`plan.working_dir` and lands on `indexes/<language>.scip` like every other tier.
The bare `index.scip` field exists for two other readers: it is the destination
migration gives a legacy repo-root `index.scip`, and it is the last-resort value
`staleness_detector::resolve_index_artifact` reports when neither `blarify.json`
nor any `indexes/*.scip` is present — which is how "missing" is phrased to the
user rather than a path anything indexes into.

The `javascript` arm's temporary `tsconfig.json` stays in the project root. That
file is an *input* to `scip-typescript` and must sit where the tool reads it. It
is created with `create_new(true)` — the `O_EXCL` create *is* the
don't-clobber check, with no exists-then-write window — and removed by a `Drop`
guard, so an interrupted run, a timeout, or a panic cleans up as reliably as a
successful one. The guard re-checks with `symlink_metadata` and refuses to
remove anything that is not the regular file it created.

### Detecting a live indexer during and after the move

The background-indexing lock lives at `<artifact_root>/indexing.pid`. The
liveness check reads **both** that path and the legacy
`<project>/.amplihack/indexing.pid`.

Checking only the new location would fail open on the single upgrade every
existing user performs: an unmigrated project has no
`<artifact_root>/indexing.pid`, because the artifact root did not exist when the
job started. The check would see no lock, conclude nothing is running, and start
a second indexer over the same tree while the first was still writing — or
migrate the files out from under it.

## Migration out of an in-repo layout

Migration runs from the **`session_start` hook**, positioned after
`migrate_global_hooks()` and before `setup_blarify_indexing()`.

**The ordering is load-bearing, not cosmetic.** `setup_blarify_indexing()`
decides whether to reindex by looking at what is present. If migration ran later
— or lazily, on first artifact-path resolution — the staleness check would
inspect a cache directory that is still empty, conclude the project has never
been indexed, and start a full rebuild against the 600-second timeout. Not for
one unlucky user: for **every existing user, on their first session after
upgrading**, while a perfectly good index sat in the repository waiting to be
moved.

The `session_start` hook is the **only** call site. `amplihack index-scip` and
`amplihack index-code` do not migrate before they inspect anything, so a project
whose artifacts were produced entirely outside a session — by CI, or by direct
CLI invocations that never launched a tool — keeps them in the checkout until its
next session.

That is a **known gap, recorded rather than closed.** Every user who upgrades by
launching amplihack is already covered on their first session, and each extra
call site is another place that can fail in front of an index read for a case
that is already handled a moment later. A user who needs it sooner can start one
session in the project, or delete the in-repo artifacts and reindex — the reindex
writes to the cache either way. Both are spelled out in
[Index a project](../howto/index-a-project.md#what-happens-to-an-index-you-already-have).

The entry point stands down cheaply when there is nothing to move:
`collect_sources()` runs one `fs::symlink_metadata` per row of
`RECOGNISED_ARTIFACTS` — nine stats on a clean project — and returns an empty
list before any cache-root resolution, lock acquisition, or PID read. Nine stats
per session start is not a cost worth optimising, and short-circuiting on a
subset would make the trigger depend on table order. It is `symlink_metadata`
rather than `exists()` throughout, for the reason in
[Symlinks are never followed](#symlinks-are-never-followed): the scan's result is
what the move then acts on, so a symlink has to stay visible as a symlink
instead of being resolved away — and the file type is re-checked at the point of
use regardless, so nothing here is an `exists()`-then-act.

### Trigger and stand-down

Migration is triggered by the **presence of a source artifact** in the checkout.
It is explicitly *not* triggered by a missing pointer file or an empty cache
directory — a project that has genuinely never been indexed has both, and there
is nothing to move.

Concurrency is handled by an `fs4` advisory lock on
`<artifact_root>/.migration.lock`. Both indexing PID files are re-read **after**
the lock is held, not only before: a check before the lock leaves a window in
which an indexer starts and has its `graph_db` moved out from under it.

`Err` from `migrate_in_repo_artifacts()` is reserved for one condition: **the
artifact root cannot be resolved at all.** Everything else is a value in the
report, because a migration that fails session start invites users to work
around it by deleting their cache:

| Report field | Meaning |
| --- | --- |
| `moved` | Source and destination of each artifact that left the checkout |
| `skipped` | `DestinationExists`, `Tracked`, or `Symlink` — the user decides |
| `failed` | Per-artifact error string; the artifact stayed where it was |
| `skipped_reason` | `IndexerRunning` or `LockUnavailable` — whole-migration stand-down, both normal outcomes |

`describe_migration(&report)` renders the report for the session-start context
block, and returns `None` when nothing happened, so a project that never had
in-repo artifacts gains no notice. It interpolates only the fixed names it
recognises plus the project path — never a repository-supplied filename, which
would make the notice a prompt-injection channel into session start.

### What moves

| Source, relative to the project | Destination, relative to the artifact root |
| --- | --- |
| `index.scip` | `index.scip` |
| `.amplihack/graph_db` | `graph_db` |
| `.amplihack/kuzu_db` | `kuzu_db` |
| `.amplihack/indexes/` | `indexes/` |
| `.amplihack/blarify.json` | `blarify.json` |
| `.amplihack/blarify_stale` | `blarify_stale` |
| `.amplihack/indexing.pid` | `indexing.pid` |
| `.amplihack/index.scip` | `index.scip.superseded` |
| `.amplihack/index.scip.backup` | `index.scip.backup` |

The ordering of that table is load-bearing in one place: the repo-root
`index.scip` is the file an indexer actually wrote, so it claims
`<root>/index.scip`. A `.amplihack/index.scip` — a staging path amplihack only
ever read — is preserved under a distinct name rather than being left in the
checkout or deleted.

Everything else in `.amplihack/` is left untouched, including
`.amplihack/session-state/` and any file the user put there. **Migration deletes
nothing it did not move.** An emptied `.amplihack/` is removed with `fs::remove_dir`,
which fails on a non-empty directory and on a symlink — exactly the wanted
behaviour: anything a human or another subsystem left there keeps the directory
alive. No `remove_dir_all` is ever aimed at a path inside a checkout.

Each moved path is logged at `info`. Migration is idempotent: a destination that
already exists is left alone and the source is recorded as `DestinationExists`
rather than overwritten.

### Git-tracked artifacts are skipped, not moved

**Only untracked sources migrate.** A source that Git already tracks — as in the
`amplihack-recipe-runner` incident that motivated this change — is recorded as
`skipped: Tracked` and left exactly where it is.

This is not politeness about the working tree. A committed file is
repository-supplied data: it arrived through the repository, it can have been
authored by anyone with commit access, and its contents are attacker-influenced
like any other checked-in file. Moving those bytes into `~/.cache/amplihack/`
would promote them into trusted, agent-readable, user-permissioned storage that
later reads treat as amplihack's own output. A poisoned `blarify.json` committed
to a repository would become the graph the agent reasons over.

The control therefore **fails closed**. The tracked-path set is an
`Option<BTreeSet<String>>`: if `git ls-files` fails, is refused, times out, or
exits non-zero, the answer is `None` and **nothing moves** — every source is
reported as failed with the reason. An empty set means Git answered and tracks
none of them, which is different from Git not answering, and the two are not
collapsed. The `git` call itself runs under a timeout, because session start
must not hang on a wedged `git`.

The user is told, and removing the file from the repository stays their
decision: `git rm -r --cached .amplihack index.scip`, plus `.gitignore` entries —
or nothing at all, if they committed it deliberately.

Size validation is unchanged and runs at the **new** location:
`validate_blarify_json_size()` checks `<artifact_root>/blarify.json` before it is
read, exactly as it checked the in-repo copy.

### Symlinks are never followed

Every stat during migration uses `symlink_metadata`, and the check is repeated
at the point of use rather than only during the initial scan. A symlink at
`<project>/.amplihack/graph_db` pointing at `~/.ssh/id_ed25519` is skipped as
`Symlink` — never followed and copied. The directory copy re-checks each entry's
file type as it walks, so a symlink planted after the scan cannot be read
through. Every resolved destination is checked for containment inside the
artifact root after canonicalisation, so a symlink planted in the cache cannot
redirect a write outside it.

### Cross-device moves preserve mtimes

`fs::rename` fails with `EXDEV` when `$HOME` and the checkout are on different
filesystems — a container bind-mount, a separate `/home` partition, an
NFS-mounted work tree. The fallback is **copy, verify, then remove the source**,
in that order, and it restores the source's modification time on the destination
with `File::set_modified`.

Without that last step every cross-device migration would stamp
`SystemTime::now()` on the copies, and the staleness comparison — which asks
whether the index is older than the source files — would conclude that a
months-old index is newer than the code it describes. The user would get stale
query results with no error, on the machines where the failure is hardest to
reproduce.

## The staleness marker

`blarify_stale` is not written by the indexing pipeline. The PostToolUse hook
writes it on the first code-file edit of a session
(`mark_blarify_stale_if_needed`,
`crates/amplihack-hooks/src/post_tool_use/validation.rs`), deriving the project
root itself.

It goes through `ensure_artifact_root()` like everything else, which preserves
the pointer-file invariant for a cache entry the hook may be the first to touch.
Relocating only the indexing writes would leave this one re-creating
`<project>/.amplihack/` on the next edit — a fix that undoes itself on the next
tool call. `blarify_stale` is a field of `ProjectArtifactPaths` for that reason:
the compiler, not review, is what keeps the hook pointed at the cache.

The marker write runs after **every** tool call, so it is deliberately quiet:
`symlink_metadata` and `create_new` rather than a blind `fs::write`, and every
error becomes a `tracing::warn!`. A failure to record staleness never fails a
tool call.

`blarify.json` is read from the same place it is written. The session-start
import path resolves it through `project_artifact_paths()`, so an existing index
is imported rather than silently degrading into a full rebuild because the old
in-repo path no longer exists.

## API

`crates/amplihack-memory/src/cli_memory/` — re-exported from
`amplihack_memory::cli_memory`.

```rust
use amplihack_memory::cli_memory::{
    ProjectArtifactPaths, ensure_artifact_root, migrate_in_repo_artifacts,
    project_artifact_paths,
};
use std::path::Path;

let project = Path::new("/home/user/src/myproject");

// Resolve every artifact path for a project. Never falls back into the repo.
let paths: ProjectArtifactPaths = project_artifact_paths(project)?;
assert!(paths.blarify_json.starts_with("/home/user/.cache/amplihack/projects/"));

// Resolve *and* create the directory chain at 0o700.
let init = ensure_artifact_root(project)?;
println!("{} (created: {})", init.root.display(), init.created);

// Move any pre-existing in-repo artifacts in. Idempotent, safe to call often.
let report = migrate_in_repo_artifacts(project)?;
for moved in &report.moved {
    println!("moved {} -> {}", moved.from.display(), moved.to.display());
}
```

| Item | Signature | Notes |
| --- | --- | --- |
| `ProjectArtifactPaths` | struct with `artifact_dir`, `indexes_dir`, `blarify_json`, `index_scip`, `indexing_pid`, `blarify_stale` | Not `#[non_exhaustive]` — see below |
| `project_artifact_paths` | `(&Path) -> Result<ProjectArtifactPaths>` | Resolves only; creates nothing |
| `project_artifact_root` | `(&Path) -> Result<PathBuf>` | The directory alone, uncreated |
| `ensure_artifact_root` | `(&Path) -> Result<ArtifactRootInit>` | Creates the chain at `0o700`, repairs loose modes, returns `{ root, created }` |
| `project_slug` | `(&Path) -> String` | `<basename>-<sha256(canonical path)[..16]>` |
| `project_for_artifact_dir` | `(&Path) -> Result<Option<PathBuf>>` | Reads the `project` pointer; `Ok(None)` means no pointer yet |
| `validate_env_dir_path` | `(&str, &Path) -> Result<()>` | `var_name` is carried into the error message |
| `migrate_in_repo_artifacts` | `(&Path) -> Result<MigrationReport>` | `Err` only when the artifact root cannot be resolved |
| `describe_migration` | `(&MigrationReport) -> Option<String>` | `None` when nothing happened |

`ProjectArtifactPaths` is deliberately **not** `#[non_exhaustive]`. Every
consumer is in this workspace, and exhaustive struct literals plus exhaustive
destructuring are what make the compiler point at every call site when a field
is added or removed. A missed consumer here is an artifact written back into
someone's repository, not a compile warning. Keep it that way.

`root_index_scip` and `index_scip_backup` are **absent** rather than relocated.
They named the file an indexer dropped in the repo root and the backup that
rescued the user's own copy of it. Now that every indexer is given an explicit
output path, both the drop and the rescue are gone. A field called
`root_index_scip` pointing at a cache directory would be a name that lies.

### Environment builder contract

`EnvBuilder::with_project_artifact_dir(project_root)` replaces
`with_project_graph_db()`. One invocation:

1. calls `ensure_artifact_root()` **once**;
2. derives **both** `AMPLIHACK_ARTIFACT_DIR` and `AMPLIHACK_GRAPH_DB_PATH` from
   that single resolved root, so the two cannot disagree;
3. unsets `AMPLIHACK_KUZU_DB_PATH`, so only the neutral contract propagates.

Deriving both from one resolution is the point. Two independent resolutions is
exactly the shape that lets the SCIP indexes land in the cache while the graph
store lands somewhere else — and `AMPLIHACK_GRAPH_DB_PATH` outranks every
project-derived path in the resolver, so a disagreement resolves silently in
favour of the wrong one.

## Interaction with Artifact Guard

[Artifact Guard](../artifact-guard.md) is **strengthened** by this work, not
weakened. Two different files carry that name, and only one of them is touched:

- `crates/amplihack-cli/src/commands/hygiene/artifact_guard.rs` — the hygiene
  command. **Unchanged**, byte-for-byte.
- `crates/amplihack-utils/src/artifact_guard.rs` — the path detectors. **Widened,
  additively.**

The `build-artifact` rule already blocked `index.scip` at any depth, and that
rule **stays** even though the relocation should stop producing the file: it is
a cheap backstop against a regression, or against a third-party indexer someone
runs by hand.

The gap this change closes: the default rules matched `.amplihack/session-state`
but nothing else under `.amplihack/`, so the 8&nbsp;MB store from the motivating
incident would not have been named as a violation even by a full `--mode all`
scan. `graph_db`, `kuzu_db`, and `indexes/` under `.amplihack/` are now
classified as build artifacts, including the nested
`packages/app/.amplihack/graph_db` form a submodule or monorepo package
produces — one directory deeper is the same incident.

The widening is narrow on purpose. It is scoped to the `.amplihack/` prefix, so
a repository's own directory named `indexes` is not suddenly prohibited, and
`.amplihack/` is not blanket-prohibited either. No check was removed, no
allowlist widened, no early return added. The detectors stay pure string logic
with no dependency on `amplihack-memory`, so the guard says nothing at all about
artifacts that live in the cache — which is correct: they are not in a
repository.

## What the relocation changes for confidentiality

Relocating the artifacts is a net improvement, but it is a **trade**, and the
trade belongs where users read it.

- **The permission model changes from repository-level to user-level.** In the
  repository, the index inherited the checkout's permissions, which on a shared
  project directory may be deliberately group-readable. In
  `~/.cache/amplihack/projects/`, it is `0o700` and readable only by the invoking
  user. For almost every user this is strictly tighter. For a deliberately shared
  checkout it is a behaviour change: collaborators who could read the index no
  longer can, and each will build their own.
- **The cache directory names every project the user has indexed.** A listing of
  `~/.cache/amplihack/projects/` shows the *basename* of every checkout —
  `acme-billing-rewrite-3f9c1ad7b2e40561` — to anything that can read the user's
  home directory. The hash hides the full path; the basename is deliberately in
  the clear so humans can debug the cache. If a project name is itself
  sensitive, `AMPLIHACK_ARTIFACT_DIR` puts the directory somewhere else.
- **The cache is a secondary copy of the source, including any secret committed
  to it.** `0o700` plus mode repair is the whole control. Cache *contents* are
  never written to logs, to the session-start context block, or to any report —
  paths only.

Three properties are accepted rather than solved here:

- Running `git` inside a checkout honours that repository's local config. This
  predates the change and is not widened by it.
- On a case-insensitive filesystem, `.Amplihack/Graph_db` bypasses the guard
  predicates. That is a uniform pre-existing property of `artifact_guard.rs`,
  not specific to these rules.
- `augmented_path()` prepends `$HOME/.local/bin`, `$HOME/.dotnet/tools`, and
  `$HOME/go/bin` — user-writable, and normal for these toolchains. It notably
  does **not** add a repo-local `node_modules/.bin`, so indexing a hostile
  repository does not execute repository-supplied binaries. Keep it that way.

## Cache growth and reclaiming space

Nothing reaps orphaned entries. A deleted project leaves its cache directory
behind, and because every git worktree canonicalises to a distinct path, every
worktree is a distinct cache identity — this repository's own workflow creates
one per feature branch. Tens of gigabytes in `~/.cache/amplihack/projects/` is a
realistic steady state for a heavy user.

Deleting a `<slug>` directory by hand is safe; the next run rebuilds it:

```sh
du -sh "${XDG_CACHE_HOME:-$HOME/.cache}"/amplihack/projects/* | sort -h | tail
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}"/amplihack/projects/myproject-3f9c1ad7b2e40561
```

To find out which checkout an entry belongs to, read its pointer file:

```sh
cat "${XDG_CACHE_HOME:-$HOME/.cache}"/amplihack/projects/myproject-3f9c1ad7b2e40561/project
```

An `amplihack cache gc` — prune entries whose `project` pointer names a path that
no longer exists, plus an age bound — is the intended fix and is tracked
separately.

## Out of scope

This change moves artifacts. It does not change:

- what is indexed, or which indexers run
- the code-graph schema (see [LadybugDB Code Schema](../memory/KUZU_CODE_SCHEMA.md))
- the memory database layout under `~/.amplihack/` (`memory_home_paths()`)
- `.amplihack/session-state/`, which is workflow session state, not an index
  artifact
- the per-project `.claude/` staging directory
- `consent_cache_path()`, which keeps its own hashing scheme
  ([above](#cache-layout))
- `AgentConfig.storage_path` (`crates/amplihack-agent-core/src/models.rs`), whose
  default is `.amplihack/agents`. It is agent storage rather than an index
  artifact, so it is out of scope here — and it is a project-relative default of
  the same shape, worth revisiting on its own terms.

`GraphDbConfig::default()` carries a `.amplihack/kuzu_db` literal that reads like
the same bug. It is unreachable as a path: `GraphDbConnector::new` overwrites
`db_path`, and the only other constructor is a test asserting unrelated fields.
The literal is documented in place rather than changed, because changing an
unreachable default is a change with no observable behaviour and a real chance of
breaking the test that reads it.

**Windows.** `0o700` is `cfg(unix)`; there is no equivalent call on Windows and
`HOME` is usually unset there. Windows resolution uses the same `home_dir()`
helper `memory_home_paths()` relies on, and the security boundary is the per-user
profile directory's own ACL — amplihack sets no additional ACL. This is the
accepted posture, not an oversight.

## Verifying the invariant

Three tests hold the property, in the crates that own the behaviour.

**`crates/amplihack-cli/tests/issue_1476_no_artifacts_in_checkout.rs`** — the
clean-repo property. It indexes a throwaway git repository in a tempdir, with
stub `scip-*` binaries on `PATH` that exit non-zero unless they are given an
explicit output path, and asserts the repository is untouched:

```rust
let repo = init_git_repo_with_one_source_file()?;
run_native_scip_indexing(Some(repo.path()), &[])?;

// The headline assertion: the user's repository is unchanged.
assert_eq!(git(&repo, &["status", "--porcelain"])?, "");

// And the ignored-but-present case `git status` cannot see.
for entry in walk_skipping_git_and_symlinks(repo.path()) {
    assert_ne!(entry.file_name(), ".amplihack");
    assert_ne!(entry.file_name(), "index.scip");
}
```

`git status --porcelain` being byte-empty is the property that matters — "the
user's repository is unchanged", not "these six filenames are absent". The
recursive walk catches what porcelain cannot: a repository whose `.gitignore`
hides `.amplihack/`, which is precisely the configuration `amplihack-rs` itself
has and the configuration that hid this bug for as long as it did. The walk uses
`entry.file_type()` and never descends into a symlinked directory, because a
symlink cycle in the safety net is an infinite loop in the safety net.

**`crates/amplihack-hooks/src/session_start/tests_artifact_migration.rs`** — the
ordering property. It populates an in-repo `.amplihack/`, runs the `session_start`
hook, and asserts the indexing status came back complete: migration ran before
anything asked whether an index existed. This is the test that catches the
600-second-rebuild-for-every-user failure.

**`artifact_root_errors_when_home_and_xdg_are_both_unset` in
`crates/amplihack-memory/src/cli_memory/artifact_root_tests.rs`** — the
resolution property: no resolution path returns a cwd-relative or
project-relative artifact path, including when `HOME` and `XDG_CACHE_HOME` are
both unset. It reads as redundant with the two above, because on a healthy system
it reaches the same outcome by a different route. It is not: it is the only test
pinning the "error rather than fall back into the repository" decision, and that
decision is the entire fix. Do not delete it as duplicative.

To check by hand in any repository:

```sh
cd /path/to/some/other/repo
amplihack index-scip
git status --porcelain     # must print nothing
git status --porcelain --ignored | grep -E '\.amplihack|index\.scip'   # also nothing
ls "${XDG_CACHE_HOME:-$HOME/.cache}"/amplihack/projects/
```

## Related

- [Environment Variables](environment-variables.md#amplihack_artifact_dir) — `AMPLIHACK_ARTIFACT_DIR`
- [`amplihack index-scip` / `index-code`](memory-index-command.md) — command reference
- [How to Index a Project](../howto/index-a-project.md) — task guide
- [Artifact Guard](../artifact-guard.md) — the commit/publish gate
- [LadybugDB Code Graph](../concepts/kuzu-code-graph.md) — what the graph stores
