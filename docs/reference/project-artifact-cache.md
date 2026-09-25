# Per-Project Artifact Cache — Reference

**Status: [PLANNED — Implementation Pending]** (issue #1476). This document
describes the intended layout. Code marked `[PLANNED]` does not exist yet; the
"Layout today" section describes what shipping code does right now. Remove the
`[PLANNED]` markers once the code is merged.

Code-index artifacts for a project — SCIP indexes, the `blarify.json` import
file, the code-graph database, the staleness marker, and the
background-indexing PID file — live in a per-project cache directory
**outside** the indexed checkout.

## Contents

- [Why: the failure mode](#why-the-failure-mode)
- [Layout today](#layout-today)
- [Planned layout](#planned-layout)
- [Resolution order](#resolution-order)
- [Directory permissions](#directory-permissions)
- [The `project` pointer file](#the-project-pointer-file)
- [SCIP indexer output paths](#scip-indexer-output-paths)
- [Migration from an in-repo layout](#migration-from-an-in-repo-layout)
- [Interaction with Artifact Guard](#interaction-with-artifact-guard)
- [What moving the artifacts changes for confidentiality](#what-moving-the-artifacts-changes-for-confidentiality)
- [Out of scope](#out-of-scope)
- [Verifying the invariant](#verifying-the-invariant)

## Why: the failure mode

Indexing writes into the repository it indexes. In `amplihack-rs` this is
invisible, because this repo's own `.gitignore` lists both offenders:

```sh
$ grep -n 'index.scip\|\.amplihack' .gitignore
30:index.scip
31:.amplihack/
33:observability/litellm/.amplihack-api-key*
```

No other repository has those entries. In `rysweet/amplihack-recipe-runner` a
routine `git add -A` therefore staged an 8&nbsp;MB `graph_db` binary along with
the rest of the generated tree. The user had asked amplihack to index their
code; amplihack answered by putting a build artifact into their commit.

The artifacts a single indexing run leaves in the indexed checkout today:

| Path (relative to the indexed project) | What it is | Typical size | Persists after a clean run? |
| --- | --- | --- | --- |
| `.amplihack/graph_db` | LadybugDB code-graph store (a single file) | 8–10 MB | Yes |
| `.amplihack/indexes/<language>.scip` | Per-language SCIP index | 1–50 MB | Yes |
| `.amplihack/blarify.json` | Import input for `index-code` | KB–MB | Yes |
| `.amplihack/blarify_stale` | Staleness marker, rewritten on every code edit | bytes | Yes |
| `.amplihack/indexing.pid` | Background-indexing lock | bytes | Yes, until the job exits |
| `.amplihack/kuzu_db` | Legacy code-graph store | 8–10 MB | Yes, where it already exists |
| `index.scip` | Raw indexer output, at the repo **root** | 1–50 MB | No — deleted or restored by `restore_root_index` |
| `.amplihack/index.scip.backup` | Backup of a pre-existing root `index.scip` | 1–50 MB | No — renamed back over the root file |

The root `index.scip` and its backup are transient by design, but "transient"
only holds for a run that finishes. An indexer that is interrupted, killed by
the 600-second timeout, or whose rename fails leaves the root `index.scip`
behind — and the background indexing mode means the run outliving the session
is the normal case, not the exotic one. The `.amplihack/` contents are not
transient at all: they are the point of indexing and they stay.

`.amplihack/blarify_stale` deserves separate attention because nothing in the
indexing pipeline writes it. The PostToolUse hook does, on the first code-file
edit of a session (`mark_blarify_stale_if_needed`,
`crates/amplihack-hooks/src/post_tool_use/validation.rs:103-114`), using a
project root it derives itself from `ProjectDirs::from_cwd()`. Relocating only
the indexing writes would leave this one re-creating `<root>/.amplihack/` on
the next edit, which is enough to fail the clean-repo test in
[Verifying the invariant](#verifying-the-invariant).

## Layout today

**Three** functions place these paths, all keyed on a project root — and the
third is the one most easily missed:

- `project_artifact_paths()` — `crates/amplihack-memory/src/cli_memory/types.rs:131`
  returns `artifact_dir = project_path.join(".amplihack")` and
  `root_index_scip = project_path.join("index.scip")`.
- `project_code_graph_paths()` — `crates/amplihack-memory/src/cli_memory/code_graph/paths.rs:23`
  returns `project_root/.amplihack/graph_db` (and the legacy `kuzu_db`).
- `GraphDbConfig::default()` — `crates/amplihack-memory/src/graph_db.rs:64`
  hardcodes a **cwd-relative** `PathBuf::from(".amplihack/kuzu_db")`. It takes
  no project root at all, so it writes wherever the process happens to be
  standing. Any caller that constructs a `GraphDbConfig` without an explicit
  `db_path` reintroduces the bug regardless of what the other two functions do.

`root_index_scip` is a write target, not a compatibility read:
`crates/amplihack-memory/src/cli_memory/scip_indexing/commands.rs:112` passes it
into `run_indexer_for_language`, which deletes it, runs the indexer with
`current_dir` set to the project, then renames whatever the indexer dropped at
the repo root into `.amplihack/indexes/<language>.scip`
(`scip_indexing/indexer.rs:12-114`). A backup/restore pair brackets the loop so
a user's own `index.scip` survives the run.

The decisive **production** write path is neither of these functions directly.
`EnvBuilder::with_project_graph_db()`
(`crates/amplihack-cli/src/env_builder/builder.rs:135`) joins
`.amplihack/graph_db` onto the project root and exports it as
`AMPLIHACK_GRAPH_DB_PATH`, which is the **highest-precedence** input to
`resolve_code_graph_db_path_for_project()` (`paths.rs:157-165`) — it is checked
before any project-derived path. Whatever the resolver would compute on its
own, the launcher's exported value wins.

## Planned layout

```
${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/projects/<slug>/
├── project                  # canonical project path, one line, no trailing junk
├── graph_db                 # code-graph store (single file)
├── kuzu_db                  # legacy store, only if migrated from one
├── indexes/
│   ├── python.scip
│   └── rust.scip
├── index.scip               # staging target for the current indexer run
├── blarify.json
├── blarify_stale
├── indexing.pid
└── .migration.lock          # fs4 advisory lock, migration only
```

`<slug>` is `<basename>-<sha256(canonical_project_path)[..16]>`, so it is both
readable and collision-resistant:

```
/home/user/src/myproject  ->  myproject-3f9c1ad7b2e40561
```

The hash is taken over the **canonicalised absolute** path, so a symlinked and
a real path to the same checkout share one cache entry, and two checkouts with
the same basename (`worktrees/feat-a/myproject` and `worktrees/feat-b/myproject`)
do not.

Two details of the hash are deliberate:

- **The hash input is the raw `OsStr` bytes of the canonical path**, not
  `to_string_lossy()`. Lossy conversion replaces every invalid UTF-8 byte with
  the same `U+FFFD`, so two distinct non-UTF-8 paths can hash identically and
  collide into one cache entry — one project silently reading and overwriting
  another's index. This is the one place the existing precedent is **not**
  copied: `consent_cache_path()`
  (`crates/amplihack-cli/src/commands/launch/blarify.rs:334`) does hash
  `to_string_lossy()`. It is intentionally left alone and intentionally not
  unified with `project_slug()` — changing it would invalidate every user's
  stored consent and re-prompt them, and a consent-file collision is a
  different and much smaller problem than an index collision.
- **Truncating to 16 hex digits (64 bits) is accepted.** These are local
  filesystem paths derived from paths the user already controls, not a
  security boundary; there is no adversary choosing project paths to force a
  collision. 64 bits also matches the existing fingerprint precedent in
  `crates/amplihack-hooks/src/issue_dedup.rs:24`, and a full 64-character
  directory name would make the cache unreadable for the humans who have to
  debug it.

Nothing is written under the indexed project. That is the whole point of the
change, and it is enforced by a test rather than by review
(see [Verifying the invariant](#verifying-the-invariant)).

### Planned API

```rust
// [PLANNED] crates/amplihack-memory/src/cli_memory/types.rs

/// Per-project artifact paths, all under a cache directory outside the project.
///
/// Deliberately **not** `#[non_exhaustive]`: every consumer is in this
/// workspace, and exhaustive struct literals plus exhaustive destructuring are
/// what make the compiler point at every call site when a field is added or
/// removed. That is the property this change depends on — a missed consumer is
/// an artifact written back into someone's repository, not a compile warning.
pub struct ProjectArtifactPaths {
    pub artifact_dir: PathBuf,
    pub indexes_dir: PathBuf,
    pub blarify_json: PathBuf,
    pub blarify_stale: PathBuf,
    pub index_scip: PathBuf,
    pub indexing_pid: PathBuf,
}

/// Resolve the artifact dir for `project_path`. Fails rather than falling back
/// to a project-relative path.
pub fn project_artifact_paths(project_path: &Path) -> Result<ProjectArtifactPaths> {
    todo!()
}
```

Three changes to the shape:

- **`blarify_stale` is added.** It is not an artifact path today — the
  PostToolUse hook builds it inline — and bringing it into the struct is what
  makes the compiler force that hook onto the new location.
- **`root_index_scip` and `index_scip_backup` are removed**, not relocated.
  They exist only to name the file an indexer drops in the repo root and the
  backup that protects a user's copy of it. Once indexers are given an explicit
  output path (below), both the drop and the rescue disappear. Their complete
  consumer set is `commands.rs:112,114`, `indexer.rs:12-20`, and
  `indexer.rs:187-202` — nothing else in the workspace reads either one. A
  field called `root_index_scip` pointed at a cache directory would be a name
  that lies.
- **The struct and function become `pub`**, not `pub(crate)`. The hooks and CLI
  crates both need them.

`project_artifact_paths()` returns `Result` because resolution can now fail
(no `HOME`, no usable `XDG_CACHE_HOME`, un-canonicalisable project path).
Returning a project-relative path on failure would reintroduce the bug on
exactly the systems where it is hardest to notice.

### Environment builder contract

`[PLANNED] EnvBuilder::with_project_artifact_dir(project_root)` replaces
`with_project_graph_db()` (which is deleted, or deprecated as a thin forward to
the new call). One invocation:

1. calls `ensure_artifact_root()` **once**, creating the directory chain with
   the permissions described below;
2. derives **both** `AMPLIHACK_ARTIFACT_DIR` and `AMPLIHACK_GRAPH_DB_PATH` from
   that single resolved root, so the two cannot disagree;
3. unsets `AMPLIHACK_KUZU_DB_PATH`, so only the neutral contract propagates.

Deriving both from one resolution is the point. Two independent resolutions —
one for the artifact dir, one for the graph DB — is exactly the shape that lets
the SCIP indexes land in the cache while the graph store lands somewhere else,
and `AMPLIHACK_GRAPH_DB_PATH` outranks every project-derived path in the
resolver, so a disagreement resolves in favour of the wrong one silently.

## Resolution order

| Order | Source | Result |
| --- | --- | --- |
| 1 | `AMPLIHACK_ARTIFACT_DIR` | Used directly as the artifact dir |
| 2 | `XDG_CACHE_HOME` | `$XDG_CACHE_HOME/amplihack/projects/<slug>` |
| 3 | `HOME` | `$HOME/.cache/amplihack/projects/<slug>` |
| 4 | none of the above | Error |

### Validation

Today's validator, `validate_graph_db_env_path()`
(`code_graph/paths.rs:123-144`), checks one variable's value: absolute, no `..`
component, not under `/proc`, `/sys`, or `/dev`. `[PLANNED]` it is generalised
to `validate_env_dir_path(var_name, path)` — the `var_name` is carried so the
error message names the variable the user actually set — and applied to **all
three** inputs above, not just the override. An attacker-controlled or merely
mistaken `XDG_CACHE_HOME` or `HOME` is the same class of problem as a bad
`AMPLIHACK_ARTIFACT_DIR`, and today neither is checked at all.

`AMPLIHACK_ARTIFACT_DIR` gets four additional rejections on top of the shared
checks, because it is used *directly* as the artifact dir rather than having
`amplihack/projects/<slug>/` appended:

| Rejected value | Why |
| --- | --- |
| `/` | Migration and cleanup would operate on the filesystem root |
| `$HOME` | Same, scoped to the user's entire home directory |
| `/tmp`, `/var/tmp` | World-writable; a pre-created `<slug>` directory owned by another user is a straightforward index-poisoning path |

The two failure modes are **asymmetric**, and the asymmetry is intentional:

- An invalid `XDG_CACHE_HOME` produces a `tracing::warn!` and **falls through
  to `HOME`**. The user very likely did not set it for amplihack's benefit, and
  a usable fallback exists.
- An invalid `AMPLIHACK_ARTIFACT_DIR` is an **error**. The user set it
  deliberately, for this tool; silently ignoring it and writing somewhere else
  is worse than stopping.

See [Environment Variables](environment-variables.md#amplihack_artifact_dir).

### Inheritance hazard

`AMPLIHACK_ARTIFACT_DIR` names one project's directory, and the environment
builder exports it to every child process. Agents in this repo run in git
worktrees (`AGENTS.md`), so a child launched for project B would otherwise
inherit project A's artifact dir and write B's index into A's cache.

The **primary** defence is (1); (2) is a backstop that does not always fire:

1. `with_project_artifact_dir(project_root)` re-resolves the directory for the
   project being launched and **overwrites** any inherited value — the same
   contract `with_project_graph_db` already documents for issue #250
   (`builder.rs:130-138`). Every launch that goes through the builder is
   correct by construction.
2. If `AMPLIHACK_ARTIFACT_DIR` names a directory whose `project` pointer file
   records a *different* canonical project path, the override is ignored with a
   `tracing::warn!` and resolution falls through to `XDG_CACHE_HOME`.

**A residual gap remains.** Check (2) cannot fire before a pointer file exists,
and the first thing an inherited-but-wrong directory tends to be is one that
has not been written yet — a fresh cache entry has no pointer, so the mismatch
is undetectable and the value is accepted. It closes the case where project A
has already been indexed, which is the common one, and leaves open the case
where a hand-set `AMPLIHACK_ARTIFACT_DIR` points at an empty directory. A
process launched outside the builder — a user running `amplihack index-scip`
directly in an inherited shell — is therefore still able to write project B's
index into a directory named for project A. Accepted, and recorded here rather
than papered over.

## Directory permissions

The cache holds a searchable index of the user's source code. On a shared host
the default `0o755` that `create_dir_all` produces under a typical `umask`
would make every indexed project world-readable.

- Every directory in the chain — `<cache>/amplihack`, `.../projects`,
  `.../projects/<slug>`, `.../indexes` — is created with
  `DirBuilder::mode(0o700)` under `cfg(unix)`.
- **`create_dir_all` does not re-tighten an existing directory.** A
  `~/.cache/amplihack` left at `0o755` by an earlier version, or created by
  something else entirely, keeps those bits forever. Resolution therefore
  inspects the directories amplihack owns and repairs loose modes, logging the
  repair.
- **amplihack never `chmod`s a directory it did not create.** `~/.cache` and
  `$HOME` belong to the user and to other tools; tightening them would break
  unrelated software. The repair rule above is scoped to `amplihack/` and
  below.
- After creating or repairing, the result is verified with `symlink_metadata`
  (not `metadata` — the distinction is the whole point), checking that the
  entry is a real directory rather than a symlink, that its `uid` matches the
  current user, and that its mode is `0o700`. A pre-existing entry that fails
  any of these is an error, not something to fix in place: it is the shape a
  planted symlink takes.
- The `project` pointer file is written with `create_new` and mode `0o600`, via
  a temporary name carrying a **random** suffix rather than the process id.
  `create_new` turns a pre-planted file into an error instead of a truncation,
  and a predictable temp name in a directory an attacker can reach is the
  classic symlink race.
- Pointer reads are **bounded to 4&nbsp;KiB** and validated before use. The
  file is one line of path; refusing to read more means a hostile or corrupt
  cache entry cannot be turned into an allocation.

### Consent gate

Code-graph indexing is already gated on explicit user consent. That gate is
unchanged and is not moved earlier. `ensure_artifact_root()` may create an
**empty** `0o700` directory before consent is resolved — an empty directory
discloses nothing beyond a path the user chose — but **no index, no
`blarify.json`, and no graph store is written before the existing consent check
passes**.

## The `project` pointer file

The artifact dir contains a `project` file holding the canonical project path,
written atomically when the dir is created. It exists because one path lookup
runs backwards: `project_root_for_blarify_input()`
(`code_graph/paths.rs:167`) recovers a project root by walking **two** parents
up from a `blarify.json` — `<project>/.amplihack/blarify.json` — and confirming
the guess against `project_artifact_paths()`. Under the slug layout that
confirmation can never match.

What happens on that failure differs between the two callers, and only one of
them is dangerous:

- `infer_code_graph_db_path_from_input()` (`paths.rs:172-177`) falls back to
  `default_code_graph_db_path()`, which resolves to **the current working
  directory**. `amplihack index-code <blarify.json>` would then create a graph
  database inside whichever repo the user happened to be standing in: the same
  bug, relocated.
- `code_graph_compatibility_notice_for_input()` (`paths.rs:109-121`) also
  retries against the cwd, but it is only producing an advisory notice and
  degrades to `Ok(None)` when it finds nothing. It is wrong, not harmful.

`[PLANNED]` the reverse lookup reads the pointer file instead. The new
signature is `Result<Option<PathBuf>>` — it returns an owned path because the
answer no longer borrows from the input, and `Result` because a missing or
unreadable pointer is now reported rather than swallowed. It walks **one**
parent hop (`<artifact_root>/blarify.json`), not two. Crucially it **errors on
a missing or unreadable pointer instead of falling back to the cwd**: that
silent cwd fallback is itself a latent instance of this failure mode and is
removed on this path.

## SCIP indexer output paths

Every artifact-relocation scheme depends on the indexer writing where it is
told. The flags are **not** uniform, and today no language arm passes one
(`scip_indexing/indexer.rs:120-183`). In-repo evidence confirms `--output` for
two of the seven:

```sh
$ grep -n -- '--output' crates/amplihack-blarify/src/code_refs/scip.rs
128:                "--output",
143:            .args(["index", "--output", "index.scip"])
```

Line 128 is `scip-python` and line 143 is `scip-typescript`, in
`amplihack-blarify`'s own indexer wrapper — a separate code path from
`scip_indexing`, and the only place in the workspace where an output path is
passed at all. Note that even there the destination is inside the project
(`generate_python_index` builds `root_path/index.scip`,
`crates/amplihack-blarify/src/code_refs/scip.rs:121`), so it confirms the flag
exists without demonstrating the fix. The flags for `scip-go`,
`rust-analyzer scip`, `scip-dotnet`, and `scip-clang` are unverified and must
each be confirmed against the installed tool, not assumed.

`[PLANNED]` A per-language `ScipOutput` table replaces the bare command
vectors, with a doc comment on each arm citing where its flag was confirmed,
and a three-tier ladder:

| Tier | Condition | Behaviour |
| --- | --- | --- |
| 1 | Indexer accepts an output flag | Pass `<artifact_dir>/indexes/<lang>.scip` directly |
| 2 | No output flag, but takes an explicit project root (e.g. `rust-analyzer scip <path>`) | Run with `current_dir` set to a tempdir **outside** the checkout, pass the project root explicitly, then move the result into the cache |
| 3 | Neither | Report the language through the existing `skipped_languages` channel with a named reason |

No language is *expected* to reach tier 3. The tier exists so that discovering
one during implementation cannot quietly reintroduce an in-repo write. Tier 2
matters because "write into the repo, then move it out" is not available to a
tool that infers its project root from `current_dir` — moving the cwd out of
the checkout changes what gets indexed. A silent degradation to an in-repo
write would keep stub-based tests green while the bug returned in production.

The `javascript` arm's temporary `tsconfig.json` stays where it is. That file
is an *input* to `scip-typescript` and must sit where the tool reads it, in the
project root (`indexer.rs:130-168`). It is already written and then removed by
an explicit cleanup closure, so it is not an artifact and not in scope.

### Detecting a live indexer during and after the move

The background-indexing lock moves from `<project>/.amplihack/indexing.pid` to
`<artifact_root>/indexing.pid`. The liveness check must read **both**.

Checking only the new location fails open on the single upgrade every existing
user performs: an unmigrated project has no `<artifact_root>/indexing.pid`,
because the artifact root did not exist when the job started. The check would
see no lock, conclude nothing is running, and start a second indexer over the
same tree while the first is still writing — or migrate the files out from
under it. The legacy path is read until migration has completed for that
project.

## Migration from an in-repo layout

`[PLANNED]` Migration runs from the **`session_start` hook**, positioned
between `migrate_global_hooks()` and `setup_blarify_indexing()`
(`crates/amplihack-hooks/src/session_start/mod.rs:90-100`).

**The ordering is load-bearing, not cosmetic.** `setup_blarify_indexing()`
decides whether to reindex by looking at what is present. If migration ran
later — or lazily, on first artifact-path resolution — the staleness check
would inspect a cache directory that is still empty, conclude the project has
never been indexed, and start a full rebuild against the 600-second timeout.
Not for one unlucky user: for **every existing user, on their first session
after upgrading**, while a perfectly good index sat in the repo waiting to be
moved. Migration therefore completes before anything asks whether an index
exists.

### Trigger

Migration is triggered by the **presence of a source artifact** in the
checkout, and runs at most once per project. It is explicitly **not** triggered
by the absence of a pointer file or an empty cache directory — a project that
has genuinely never been indexed has both, and there is nothing to migrate.

Concurrency is handled by an `fs4` advisory lock on
`<artifact_root>/.migration.lock`. A session that cannot take the lock
**returns a skipped reason and continues** — another session is already doing
the work, which is a normal outcome, not a failure. `Err` from the migration
entry point is reserved for one condition: **the artifact root cannot be
resolved at all.** Everything else — a conflicting destination, an unreadable
source, a tracked file, lock contention — is a per-item entry in the migration
report.

### What moves

The recognised names, all under `<project>/.amplihack/`:

`graph_db`, `kuzu_db`, `indexes/`, `blarify.json`, `blarify_stale`,
`indexing.pid`, `index.scip`, `index.scip.backup`

plus `<project>/index.scip` at the repo root. Everything else in `.amplihack/`
is left untouched, including `.amplihack/session-state/` and any file the user
put there. Migration deletes nothing it did not move; an emptied `.amplihack/`
is left in place rather than removed.

Each moved path is logged at `info`. Migration is idempotent: a destination
that already exists is left alone and the source is recorded as a conflict
rather than overwritten.

### Git-tracked artifacts are skipped, not moved

**Only untracked sources are migrated.** A source that Git already tracks — as
in the `amplihack-recipe-runner` incident that motivated this change — is
recorded in the report as `skipped: tracked` and left exactly where it is.

This is not politeness about the user's working tree. A committed file is
repository-supplied data: it arrived through the repo, it can have been
authored by anyone with commit access, and its contents are attacker-influenced
in the same way any other checked-in file is. Moving those bytes into
`~/.cache/amplihack/` promotes them into trusted, agent-readable,
user-permissioned storage that later reads treat as amplihack's own output. A
poisoned `blarify.json` committed to a repository would become the graph the
agent reasons over.

The user is told, and removing the file from the repository stays their
decision — `git rm --cached` plus a `.gitignore` entry, or nothing at all if
they committed it deliberately.

Size validation is unchanged and stays at the **new** location:
`validate_blarify_json_size()` runs against the migrated
`<artifact_root>/blarify.json` before it is read, exactly as it runs today
against the in-repo copy.

### Symlinks are never followed

Every stat during migration uses `symlink_metadata`, and every resolved
destination is checked for containment inside the artifact root after
canonicalisation. A symlink at `<project>/.amplihack/graph_db` pointing at
`~/.ssh/id_ed25519` must be moved as *a symlink* or refused — never followed
and copied. The same applies to the destination side: a symlink planted in the
cache directory must not redirect a write outside it.

### Cross-device moves preserve mtimes

`fs::rename` fails with `EXDEV` when `$HOME` and the checkout are on different
filesystems — a container bind-mount, a separate `/home` partition, an
NFS-mounted work tree. The fallback is **copy, verify, then remove the
source**, in that order, and it explicitly restores the source's modification
time on the destination with `File::set_modified`.

Without that last step every cross-device migration would stamp
`SystemTime::now()` on the copies, and the staleness comparison — which asks
whether the index is older than the source files — would conclude that a
months-old index is newer than the code it describes. The user would get stale
query results with no error, on the machines where the failure is hardest to
reproduce.

## Interaction with Artifact Guard

[Artifact Guard](../artifact-guard.md) is **strengthened** by this work, not
weakened. Its `build-artifact` rule already blocks `index.scip` at any depth
(`crates/amplihack-utils/src/artifact_guard.rs:993`), and that rule **stays**
even though the relocation should stop producing the file — it is a cheap
backstop against a regression or a third-party indexer run by hand.

The gap this change closes: the guard's default rules match
`.amplihack/session-state` (`artifact_guard.rs:954-955`) but **not**
`.amplihack/graph_db`, `.amplihack/kuzu_db`, or `.amplihack/indexes/`. The
8&nbsp;MB store in the motivating incident would not have been named as a
violation even by a full `--mode all` scan. `[PLANNED]` those three paths are
added as prohibited rules as part of this change, so that if the relocation
ever regresses, the result is a blocked commit rather than a staged binary.

## What moving the artifacts changes for confidentiality

Relocating the artifacts is a net improvement, but it is a **trade**, and the
trade should be written down where users read it rather than only in a pull
request description.

- **Permission model changes from repository-level to user-level.** In the
  repo, the index inherited the checkout's permissions, which on a shared
  project directory may be deliberately group-readable. In
  `~/.cache/amplihack/projects/`, it is `0o700` and readable only by the
  invoking user. For almost every user this is strictly tighter. For a
  deliberately shared checkout it is a behaviour change: collaborators who
  could read the index no longer can, and each will build their own.
- **The cache directory names every project the user has indexed.** A listing
  of `~/.cache/amplihack/projects/` shows the *basename* of every checkout —
  `acme-billing-rewrite-3f9c1ad7b2e40561` — to anything that can read the
  user's home directory. The hash hides the full path; the basename is
  deliberately in the clear so humans can debug the cache. If a project name is
  itself sensitive, `AMPLIHACK_ARTIFACT_DIR` puts the directory somewhere else.

## Out of scope

This change moves artifacts. It does not change:

- what is indexed, or which indexers run
- the code-graph schema (see [LadybugDB Code Schema](../memory/KUZU_CODE_SCHEMA.md))
- the memory database layout under `~/.amplihack/`
  (`memory_home_paths()`, `types.rs:109`)
- `.amplihack/session-state/`, which is workflow session state, not an index
  artifact
- the per-project `.claude/` staging directory
- `consent_cache_path()`, which keeps its own hashing scheme
  ([above](#planned-layout))

Two known limitations are accepted rather than solved here:

**Unbounded cache growth.** Nothing reaps orphaned entries. A deleted project
leaves its cache directory behind forever, and because every git worktree
canonicalises to a distinct path, every worktree is a distinct cache identity —
this repository's own workflow creates one per feature branch. Tens of
gigabytes in `~/.cache/amplihack/projects/` is a realistic steady state for a
heavy user. A follow-up `amplihack cache gc` — prune entries whose `project`
pointer names a path that no longer exists, plus an age bound — is the intended
fix and is tracked separately. Until then, deleting a `<slug>` directory by
hand is safe: the next run rebuilds it.

**Windows.** `0o700` is `cfg(unix)`; there is no equivalent call on Windows and
`HOME` is usually unset there. Windows resolution uses the same `home_dir()`
helper `memory_home_paths()` already relies on (`types.rs:109`), and the
security boundary is the per-user profile directory's own ACL — amplihack sets
no additional ACL. Documented as the accepted posture, not as an oversight.

## Verifying the invariant

`[PLANNED]` Two tests, in the crates that own the behaviour:

**`crates/amplihack-cli/tests/issue_1476_no_artifacts_in_checkout.rs`** — the
clean-repo property. Indexes a throwaway git repository in a tempdir and
asserts the repository is untouched:

```rust
// [PLANNED]
let repo = init_git_repo_with_one_source_file()?;
run_native_scip_indexing(Some(repo.path()), &[])?;

let status = git(&repo, &["status", "--porcelain"])?;
assert_eq!(status, "", "indexing dirtied the indexed repository");
assert!(!repo.path().join(".amplihack").exists());
assert!(!repo.path().join("index.scip").exists());
```

It asserts on `git status` rather than on a path list because the property that
matters is "the user's repository is unchanged", not "these six filenames are
absent". It fails on `main` — `git status --porcelain` reports `?? .amplihack/`
— and passes after the change.

The `index.scip` assertion is the weaker of the two on `main`: a run that
completes cleanly removes the root `index.scip` itself, so that line alone
would pass today. Keep it anyway — it is what catches a tier-2 or tier-3
regression in [SCIP indexer output paths](#scip-indexer-output-paths), where an
indexer writes to the repo root again and the move back out fails.

**`crates/amplihack-hooks/tests/issue_1476_migration_before_staleness.rs`** —
the ordering property. Sets up a project with a populated in-repo
`.amplihack/`, runs the `session_start` hook, and asserts that
`setup_blarify_indexing()` saw the migrated index and did **not** trigger a
rebuild. This is the test that would have caught the 600-second-rebuild-for-
everyone failure described under [Migration](#migration-from-an-in-repo-layout).

A third test — asserting that no resolution path ever returns a
cwd-relative or project-relative artifact path, including when `HOME` and
`XDG_CACHE_HOME` are both unset — is a **security regression test**. It reads
as redundant with the two above, because on a healthy system it exercises the
same outcome by a different route. It is not: it is the only test that pins the
"error rather than fall back into the repository" decision, and that decision is
the entire fix. Do not delete it as duplicative.

To check by hand in any repository:

```sh
cd /path/to/some/other/repo
amplihack index-scip
git status --porcelain     # must print nothing
ls "${XDG_CACHE_HOME:-$HOME/.cache}"/amplihack/projects/
```

## Related

- [Environment Variables](environment-variables.md#amplihack_artifact_dir) — `AMPLIHACK_ARTIFACT_DIR`
- [`amplihack index-scip` / `index-code`](memory-index-command.md) — command reference
- [How to Index a Project](../howto/index-a-project.md) — task guide
- [Artifact Guard](../artifact-guard.md) — the commit/publish gate
- [LadybugDB Code Graph](../concepts/kuzu-code-graph.md) — what the graph stores
