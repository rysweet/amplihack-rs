---
title: Workflow runtime isolation
description: How workflows keep runtime state and unrelated staged work out of task commits.
doc_type: explanation
last_updated: 2026-11-12
---

# Workflow runtime isolation

**Acceptance remains incomplete.** The candidate implementation retains the
[runner and historical-evidence gates](../reference/workflow-checkpoint-publication.md#regression-and-runner-acceptance).
See the [current restrictions](../reference/workflow-checkpoint-publication.md#implementation-gaps).

Amplihack keeps its runtime state out of source inventories and task commits,
including in repositories without a `.gitignore`. Launch does not require changes
to tracked ignore rules, source manifests or the user's global Git configuration.

## Separate runtime state from source

Launcher context, session records and their locks live outside every registered
checkout, beneath external Amplihack state or the run's own directory. Writers,
agent resolution, session readers and nesting detection use the same locations.
Checkout-local launcher records left by older releases do not select an agent.

State has two scopes. Launcher context belongs to a particular checkout; power
steering state shared by linked worktrees belongs to the clone. Moving storage
outside the checkout preserves that sharing. Independent clones remain separate.

Graph databases and some generated indexes remain checkout-local. Before their
first write, Amplihack registers precise exclusions in the clone's common Git
`info/exclude`. Linked worktrees share that file. The exclusions work when `HOME`
and `XDG_CONFIG_HOME` are unset, so a repository's source-inventory hook sees the
same source files before and after launch.

Exclusions cover owned runtime paths rather than the entire `.amplihack/` or
`.claude/` tree. Project configuration, agents and skills remain discoverable.

## Protect the index independently

Ignore rules affect untracked discovery. Already tracked and force-staged files
need a separate boundary: the publication guard rejects every indexed path
beneath `.amplihack/` or `.claude/runtime/`, including unchanged tracked entries,
deletions and rename endpoints. Configuration in `.amplihack/` remains visible
but cannot enter a workflow commit under this policy.

Default, consensus and applicable autodrive roles disable runner automatic
staging. Checkpoints stage exact filenames agreed by the requirements and
reviewed design. A changed-file inventory identifies candidates without granting
permission to commit them.

The real index can hold unrelated user work, including a staged blob that differs
from the working file. Checkpoints commit through a private index and preserve
protected entries, bytes and Git metadata. Unsupported preservation or overlap
causes refusal before committing. Normal hooks run, and the resulting index is
validated before commit creation. A hook cannot silently extend task authority.

## Validate what the destination receives

Every pushed ref is checked against the actual destination's advertisement.
For a new branch, all advertised remote commit tips contribute to the boundary.
Local tracking refs and `FETCH_HEAD` cannot narrow it.

Each outgoing commit needs task authority, even if a later commit reverts its
change. Advertised imports do not become task changes, but merge resolution
changes still need review. Incomplete history, failed fetches and unsupported
updates stop publication. Unauthorised outgoing commits return status `78`.

## Validate before creating runtime state

Workflow preparation requires an existing non-bare checkout, committed history
and the workflow's remote and tool prerequisites. It refuses a non-repository,
including a parent containing nested checkouts, without initialising Git.
`auto_init_repo=true` alone does not authorise repository creation.

Cleanup uses run ownership and preserves tracked source, the user's index and
other runs' worktrees. It does not replace first-write preparation or publication
validation.

## Related documentation

- [Configure workflow runtime isolation](../howto/configure-workflow-runtime-isolation.md)
- [Runtime paths, sharing and preparation API](../reference/workflow-runtime-artifacts.md)
- [Checkpoint and publication contracts](../reference/workflow-checkpoint-publication.md)
- [Runtime isolation tutorial](../tutorials/workflow-runtime-isolation.md)
- [Artifact Guard](../artifact-guard.md)
