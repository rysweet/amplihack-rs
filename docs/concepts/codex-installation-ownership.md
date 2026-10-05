---
title: Native Codex installation ownership
type: explanation
updated: 2026-10-05
---

# Native Codex installation ownership

Native installation changes a package, marketplace registration and command hooks
as one recoverable transaction. Separating responsibilities keeps ownership and
failure boundaries explicit while one lock protects the entire update.

## Responsibility boundaries

| Boundary | Responsibility |
| --- | --- |
| Packaging | Build the staged package from canonical source bytes, nested resources and generated command/persona skills; preserve executable wrapper modes without mutating live configuration. |
| Hook ownership | Plan reconciliation against prior ownership, preserve foreign definitions and reject collisions; never grant hook trust. |
| Native registration | Use the resolved Codex executable and selected `CODEX_HOME`; verify native inventory and exact package identity, propagating failures. |
| Transaction | Coordinate preparation, live application, verification and ownership commit under the held lock. |
| Recovery | Validate and replay the pending journal, restore state and retain evidence when recovery fails. |

These are private installer modules, with at most 300 lines per source module
including colocated tests. Install orchestration delegates stages instead of
containing packaging, hook reconciliation and rollback inline. Canonical skill
sources and `AGENTS.md` remain independent of generated native packaging.

## Transaction and commit

The lock covers recovery, ownership/inventory validation, staging, planning,
application and commit. Preparation snapshots original configuration bytes and
file absence, and writes a durable journal before changing live state. Application
registers the package and reconciles hooks. Verification checks native inventory
and actual hooks before publishing the ownership ledger last.

## Durability barriers

Before publishing ownership, the installer synchronizes managed package resources
and affected directories. Publication synchronizes the ledger file, renames it
into place and synchronizes its parent directory. A visible ledger alone does
not authorize deletion of rollback evidence.

Committed recovery repeats the validated synchronization barriers before removing
the previous package and pending journal, then synchronizes cleanup directory
changes. A failed barrier before cleanup retains the journal and backup. A failure
after cleanup is reported with the cleanup state; it does not imply the deleted
backup is still available. Recovery does not blindly restore obsolete ownership
after commit. Errors preserve both the primary failure and any recovery or cleanup
failure. These checked storage operations depend on filesystem synchronization
support; they do not guarantee survival of every storage or hardware failure.

## Recovery and foreign state

Before replacement, installation rejects foreign identity collisions, unsafe paths
and conflicting hook ownership. Recovery validates journal version, transaction
identity and home/root scope before acting. Raw optional-byte snapshots preserve
comments, formatting and original file absence during rollback. Interrupted
updates with valid pending-journal schema **2** can be replayed under the same lock
on the next install or uninstall. The ownership ledger uses schema **1**; it is a
separate record, not a recovery-journal version.

Unsupported journal schemas, including older journals, are rejected before
recovery mutates live state. There is no automatic historical JSON restoration.
The pending record and available previous package remain as evidence for manual
reconciliation. Current schema 2 snapshots preserve exact configuration bytes and
original file absence.

Failed recovery never reports success. It retains available evidence for diagnosis
and another recovery attempt: `~/.amplihack/codex/pending.json` if not yet removed,
and the previous package if not yet deleted or restored. Failures after committed
cleanup starts may leave no backup. Modified
owned files and unknown ownership versions require reconciliation rather than
unsafe deletion. Manual reconciliation requires comparing the retained journal,
available previous package, ledger, native registration and current configuration in the
selected `CODEX_HOME`. Preserve those records before making changes; a historical
snapshot must not overwrite later foreign edits. Do not delete the journal merely
to bypass validation or change its schema number to force replay.
Uninstall removes only the owned identity and package, preserving
foreign plugins, shared marketplaces, authentication and user configuration.

## Launch reconciliation

Codex tool availability is resolved before framework preparation. The same selected
executable handles native registration and launch. Existing framework staging does
not skip reconciliation when the client arrives later. Auto-install opt-out and
noninteractive/subprocess restrictions remain effective. Claude/Copilot retain
their existing preparation order and discovery behavior.

Installation and launch do not choose a model, grant hook trust, or add implicit
permissions or directory access. See [Codex installation and usage](../howto/install-codex-plugin.md)
and [runner delivery validation](../reference/recipe-runner-validation.md).
