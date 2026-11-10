---
title: Native Codex installation ownership
description: Understand native package ownership, rollback, stale journals and recovery barriers.
type: explanation
updated: 2026-11-10
---

# Native Codex installation ownership

Native installation changes a package, marketplace registration and command hooks
as one recoverable transaction. Separating responsibilities keeps ownership and
failure boundaries explicit while one lock protects the entire update.

## Contents

- [Responsibility boundaries](#responsibility-boundaries)
- [Transaction and commit](#transaction-and-commit)
- [Trusted home boundaries](#trusted-home-boundaries)
- [Durability barriers](#durability-barriers)
- [Recovery and foreign state](#recovery-and-foreign-state)
- [Automatic stale-journal recovery](#automatic-stale-journal-recovery)
- [Journal archival and explicit reset](#journal-archival-and-explicit-reset)
- [Launch reconciliation](#launch-reconciliation)

## Responsibility boundaries

| Boundary | Responsibility |
| --- | --- |
| Packaging | Build the staged package from canonical source bytes, nested resources and generated command/persona skills; preserve executable wrapper modes without mutating live configuration. |
| Hook ownership | Plan reconciliation against prior ownership, preserve foreign definitions and reject collisions; never grant hook trust. |
| Native registration | Use the resolved Codex executable and selected `CODEX_HOME`; verify native inventory and exact package identity, propagating failures. |
| Transaction | Coordinate preparation, live application, verification and ownership commit under the held lock. |
| Recovery | Classify prior journals at installer startup, archive proven stale evidence, and otherwise validate and replay strict recovery. |

These are private installer modules, with at most 300 lines per source module
including colocated tests. Install orchestration delegates stages instead of
containing packaging, hook reconciliation and rollback inline. Canonical skill
sources and `AGENTS.md` remain independent of generated native packaging.

## Transaction and commit

The lock covers recovery, ownership/inventory validation, staging, planning,
application and commit. Preparation snapshots original configuration bytes and
file absence, and writes a durable journal before changing live state. Application
registers the package and reconciles hooks. Verification checks native inventory
and actual hooks before publishing the ownership ledger last. Supported completion
evidence binds transaction identity and a checked generation to SHA-256 snapshots
of validated configuration, hooks and marketplace bytes, the package digest, and
ownership declarations and native registrations. Absence is distinct from an
empty file. Before ownership publication, the exact completed ledger bytes are
durably recorded in the pending journal's `expected.ledger` snapshot. Completion
proof therefore describes transaction-validated state rather than unfamiliar
bytes found after an interruption. Unchanged-install fast paths do not manufacture
a new completion generation.

Before that commit, rollback restores the original package and nonconfiguration
snapshots, then re-registers the original installed identity. Native registration
can enable a disabled plugin, so final byte-exact configuration restoration
happens after registration. A previously installed but disabled plugin remains
installed and disabled after successful rollback. Original configuration absence
is restored as absence. Ownership and concurrent-change checks surround native
operations and the final restoration; an accepted intermediate enabled state
does not establish successful rollback.
Successful rollback still reports the original upgrade failure.

## Trusted home boundaries

The installer keeps the caller's `HOME` and `CODEX_HOME` path spelling
for native commands and journal scope. A stable, pre-existing symlink above a regular home
directory is supported during installation, rollback and committed cleanup.
`HOME` must be a regular directory. Existing `CODEX_HOME` and its immediate parent
must also be regular directories; a directly symlinked home or immediate
Codex-home parent is outside this contract. An alias above `HOME` is permitted.

Before native probes, lock creation or filesystem mutation, the installer
captures one boundary context from caller-selected paths. Each anchor records
its lexical absolute path, canonical location and stable directory identity.
Relative caller paths resolve against a working directory captured once.
On Unix, device and inode identify an anchor; directory change time is unsuitable
because ordinary child creation changes it. Install, uninstall and their internal
recovery share this scope without recapturing changed anchors.

| Selected path | Pre-existing anchor |
| --- | --- |
| `$HOME/.amplihack/codex` and default `$HOME/.codex` | Regular `HOME`; explicitly selecting the default Codex path keeps this policy. |
| Explicit `CODEX_HOME` with existing parent | Regular immediate parent; an existing selected home must also be regular. |
| Explicit `CODEX_HOME` with missing parents | Nearest existing ancestor of its parent, captured before creation; it must be regular. |
| Independently invoked recovery with an explicit staging root outside `HOME` | Captured regular volume entry of the staging path; synchronization excludes the filesystem root. |

A symlink at the candidate anchor is rejected; the installer does not skip it
to choose another ancestor. Stable aliases above a regular anchor may resolve
normally. Canonical resolution is limited to the anchors; it does not establish
ownership of a package, configuration file or backup. Journal contents cannot
select a different anchor or staging root.

Beneath each anchor, relative paths cannot escape the boundary. Directory checks
run top-down without following links and stop at a missing parent. Missing
descendants can be created and are checked again afterward. Managed directory
components and configuration leaves cannot be symlinks or special files.
Allowed relative and dangling links inside package resources remain recorded
entries; their targets are never traversed for synchronization or cleanup.
Native operations, mutations, synchronization and journal retirement recheck the
same captured anchor mapping, directory identity and affected resource types.
The staging directory must match the directory held under the exclusive lock.
An unexpected change never refreshes the captured authority. Retargeting an alias
or replacing an anchor during the operation stops recovery, preserving evidence that has not
already been removed. Exact journal, configuration and package ownership guards
remain required even when the anchor is unchanged.

These checks detect changes within an operation. A new recovery invocation
captures its caller-authorized anchors again; the schema-2 journal does not store
a historical anchor identity. Path preflights do not form a fully
descriptor-relative transaction or eliminate every race with another process
running as the same user.

## Durability barriers

Before publishing ownership, the installer synchronizes managed package resources
and affected directories. Publication synchronizes the ledger file, renames it
into place and synchronizes its parent directory. A visible ledger alone does
not authorize deletion of rollback evidence.

Rollback has its own prerequisites before retiring the pending journal. It
synchronizes restored regular-file contents and package-tree contents, directory
entries for restored or removed resources, both source and destination parents
of a package rename, and publication of newly created ancestors. Restoring an
originally absent resource synchronizes its surviving parent directory. Checked
paths and ownership guards prevent following foreign symlink targets to satisfy
these barriers.

Directory barriers run deepest first through the validated canonical anchors,
including publication of transaction-created ancestors. They exclude alias
entries above those anchors, unrelated external directory contents and
package-link targets. Synchronizing an anchor directory persists its affected
immediate entries without recursively synchronizing unrelated user contents.

The restored package tree is synchronized once per recovery attempt; retries
repeat the prerequisites even when no new restoration is needed.

A prerequisite synchronization failure returns an error and retains a valid,
retryable `pending.json`, even if some resources have already been restored.
Retry repeats validation and synchronization. After the prerequisites succeed,
recovery verifies exact original snapshots, original package identity or absence,
backup absence and unchanged raw journal bytes before deleting the journal.
It then synchronizes the journal's containing directory. If this final synchronization
fails after unlink, recovery reports the error; the journal has already been
removed and retention is not promised.

Committed recovery repeats the validated synchronization barriers before removing
the previous package and pending journal. Before the first backup unlink it
atomically publishes and synchronizes a complete `backup_cleanup` inventory in
the schema 2 journal. The inventory has its own schema version 1 and binds the
transaction, selected home and original package digest to every relative path,
entry type, file SHA-256 and symlink target, including directories. Paths must be
unique and stay within the backup; unsupported entry types are rejected. The
record contains `schema_version`, `transaction`, `codex_home`, `original_digest`
and `entries`. Each entry contains `path` and `kind`; `kind.type` is `Directory`,
`File` (with `sha256`) or `Symlink` (with `target`). The empty path denotes the
backup root. An installation without a previous package records no entries.

Cleanup validates complete contents at authorization and completion. On Unix it
captures inode, type, length, modification-time and change-time stamps for every
live package entry and shared resource, including the journal, then repeats a
full preflight before accepting those stamps. Before each backup unlink it checks
all captured stamps and the backup entry's exact type, file hash or symlink
target, including directory ancestors. Full content hashing therefore has a
fixed number of passes; per-unlink metadata checks still scale with the live
entry count. Other platforms retain full preflights at every unlink.
It removes checked entries without following symlinks,
removes directories from children to parents and synchronizes each removal.
Retries tolerate missing recorded entries, but reject changed or unrecorded
survivors. Backup removal is synchronized before journal removal; journal removal
has a separate checked directory barrier. A failed barrier before cleanup retains
the journal and backup. A failure
after cleanup is reported with the cleanup state; it does not imply the deleted
backup is still available. Recovery does not blindly restore obsolete ownership
after commit. Errors preserve both the primary failure and any recovery or cleanup
failure. These checked storage operations depend on filesystem synchronization
support; they do not guarantee survival of every storage or hardware failure.
Fault injection establishes barrier ordering, error reporting and retry behavior.
Crash consequences inferred from that ordering are not empirical power-loss proof.

## Recovery and foreign state

Before replacement, installation rejects foreign identity collisions, unsafe paths
and conflicting hook ownership. Automatic archival does not adopt those resources.
Recovery validates journal version, transaction
identity and home/root scope before acting. Raw optional-byte snapshots preserve
comments, formatting and original file absence during rollback. Interrupted
updates with valid pending-journal schema **2** can be replayed under the same lock
on the next install or uninstall. The ownership ledger uses schema **1**; it is a
separate record, not a recovery-journal version.

With neither a pending journal nor an incomplete archival intent, recovery
returns without native calls or synchronization.

Unsupported journal schemas, including older journals, are rejected by automatic
classification and strict recovery before either mutates live state. Explicit
metadata reset can preserve such a journal without parsing it. There is no
automatic historical JSON restoration.
The pending record and available previous package remain as evidence for manual
reconciliation. Current schema 2 snapshots preserve exact configuration bytes and
original file absence. An intact schema 2 backup can acquire a cleanup inventory.
A historical partially deleted backup without that durable inventory requires
manual reconciliation; recovery cannot safely infer which removals were authorized.

Failed recovery never reports success. It retains available evidence for diagnosis
and another recovery attempt: `~/.amplihack/codex/pending.json` if not yet removed,
and the previous package if not yet deleted or restored. Failures after committed
cleanup starts may leave no backup. Modified
owned files and unknown ownership versions require reconciliation rather than
unsafe deletion. Manual reconciliation requires comparing the retained journal,
available previous package, ledger, native registration and current configuration in the
selected `CODEX_HOME`. Preserve those records before making changes; a historical
snapshot must not overwrite later foreign edits. Do not delete the journal merely
to bypass validation or change its schema number to force replay. Explicit
reset preserves the journal when abandoning recovery is intentional; subsequent
reconciliation still enforces ownership.
Uninstall removes only the owned identity and package, preserving
foreign plugins, shared marketplaces, authentication and user configuration.

## Automatic stale-journal recovery

The installer examines a pre-existing `pending.json` under the package lock before
normal reconciliation. It first resolves any incomplete archival intent, then
validates journal structure, supported version, selected scope and path safety.
Only this initial gate classifies stale journals. Rollback, commit-failure
recovery and committed cleanup for the current attempt retain their strict
checks. Uninstall recovery does not use stale-journal classification.

There are two reasons to archive automatically:

| Classification | Required evidence |
| --- | --- |
| Orphaned | Both recorded Codex targets, `config.toml` and `hooks.json`, are absent, and at least one was present in every permitted original, expected and intermediate state. |
| Superseded | Existing ownership and transaction evidence independently proves a coherent later completed reconciliation, and both current targets differ from every permitted state of the abandoned transaction. |

The orphan rule allows recovery after deletion of a Codex configuration tree.
Absence already permitted by the journal does not prove abandonment. If each
missing target has a permitted absent state, recovery retains protection. A single
missing target with surviving transaction state also retains protection.
Dangling symlinks, directories, unreadable files and unsafe ancestors are errors,
not absence. Missing marketplace or ownership files alone never prove orphaning.

Supersession requires consistent base evidence in the abandoned journal and a
supported, independently verified completion proof for a later generation. The
current transaction must differ from both the abandoned transaction and its
recorded base transaction. Exact snapshots, selected home, package digest,
ownership declarations and native installation registrations must agree with
that completed reconciliation. Conflicting backups, uninstall state or cleanup
transitions prevent supersession. SHA-256 equality binds bytes within the trusted
metadata boundary; hashing current files alone does not establish ownership.

Newer modification times or a higher generation alone are insufficient. A
foreign edit, a mixture of recorded and unfamiliar states, or either target
still matching an abandoned permitted state cannot establish supersession.
Missing, incomplete or unsupported completion evidence leaves strict recovery
in place. Optional versioned `recovery_evidence` in pending journals and completion
fields in ownership records preserve compatibility with journal schema **2** and
ownership schema **1**. A journal without these extensions can meet the orphan
rule if its valid snapshots prove it, but cannot establish supersession.

All other journals go through strict recovery. A state that can be recovered
safely is replayed; a genuine in-flight mismatch retains the existing refusal,
including `foreign Codex config changed; reconcile manually; recovery record retained`.

## Journal archival and explicit reset

Automatic archival and `amplihack codex --reset-recovery` share one guarded
operation. Recovery metadata is always under `$HOME/.amplihack/codex`, independent
of `CODEX_HOME`. Reset validates caller `HOME` and metadata paths, takes the package
lock and resolves incomplete archival intent. It reads raw regular-file bytes
without parsing journal contents or inspecting Codex targets. This permits explicit
reset of malformed or unsupported journals while retaining path and ownership
boundaries. Unsafe metadata paths remain errors.

Archival renames `pending.json` to `pending.json.stale-<UTC stamp>` in the same
directory, preserving exact bytes and restrictive permissions. The timestamp is
captured once in `YYYYMMDDTHHMMSSZ` form; a collision adds `.1`, `.2` and so on.
Every existing destination entry, including a dangling symlink or directory,
counts as occupied. A no-replace rename prevents overwriting an archive, without
waiting for the clock to advance. Recovery never overwrites or deletes archives.

Before movement, a durable archival intent records the validated source identity,
content hash and destination basename. Its destination must stay inside the
captured metadata directory. The operation rechecks source identity and bytes,
publishes without replacement and synchronizes storage before reporting success.
A retry resumes only when retained evidence matches the intent. Missing,
changed or conflicting evidence stops reconciliation. If rename or a later
durability barrier fails, the intent and available journal or archive keep
recovery protection; absence of `pending.json` alone cannot bypass it. Failure
identifies the affected path, filesystem cause and corrective action.
Filesystems without the required no-replace rename or synchronization support
cannot complete archival; reconciliation remains blocked.

After durable completion, exactly one concise recovery status line names the
preserved destination, with control characters in paths sanitized. Both automatic
and explicit archival use `amplihack: preserved Codex recovery journal: ` followed
by that path. With nothing to archive or resume, reset reports
`amplihack: no pending Codex recovery journal` and continues normal launch.

Reset is consumed immediately after successful CLI parsing, before client
resolution, startup self-healing or reconciliation. It precedes native, auto-mode
and Docker argument construction; Codex never receives the consumed option. The
explicit `--` separator still starts native arguments. The reset operation moves
metadata only: configuration, packages, profiles and ownership records remain
subject to normal reconciliation afterward. Archiving grants no ownership of the
former targets and cannot authorize replacement of foreign packages, hooks,
marketplace entries or leftover backups. Usage is in
[Recover an interrupted installation](../howto/install-codex-plugin.md#recover-an-interrupted-installation).

## Launch reconciliation

When requested, metadata reset runs first. Codex tool availability is then resolved
before framework preparation. The same selected
executable handles native registration and launch. Existing framework staging does
not skip reconciliation when the client arrives later. Auto-install opt-out and
noninteractive/subprocess restrictions remain effective. Claude/Copilot retain
their existing preparation order and discovery behavior.

Before registration, the installer probes the native plugin inventory with a longer time limit than other native calls. If the probe fails and Codex is the selected provider, a separate capability check decides whether the failure is reported as missing plugin support or as the probe's own failure. When another provider is selected, the check does not run and the optional plugin is skipped with a warning. See the [native plugin probe reference](../reference/codex-native-plugin-probe.md).

Installation and launch do not choose a model, grant hook trust, or add implicit
permissions or directory access. See [Codex installation and usage](../howto/install-codex-plugin.md)
and [runner delivery validation](../reference/recipe-runner-validation.md).
