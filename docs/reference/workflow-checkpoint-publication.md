---
title: Workflow checkpoint and publication reference
description: Exact task authority, index preservation and destination-based outgoing-history validation.
doc_type: reference
last_updated: 2026-11-12
---

# Workflow checkpoint and publication reference

Default, consensus and applicable autodrive routes use bounded checkpoints and
a shared publication boundary during implementation, review feedback and cleanup.
This reference describes the candidate implementation. Acceptance evidence is recorded
separately in the delivery PR, following
[issue #1554 and its comments](https://github.com/rysweet/amplihack-rs/issues/1554).

**Acceptance remains incomplete.** The runner dependency and historical evidence
requirements below remain gates; the source changes do not establish merge readiness.

## Contents

- [Implementation gaps](#implementation-gaps)
- [Command interface](#command-interface)
- [Established task scope](#established-task-scope)
- [Recipe staging policy](#recipe-staging-policy)
- [Checkpoint transactions](#checkpoint-transactions)
- [Independent runtime guard](#independent-runtime-guard)
- [Destination and outgoing history](#destination-and-outgoing-history)
- [Merge handling](#merge-handling)
- [Fetch and push outcomes](#fetch-and-push-outcomes)
- [Structured results](#structured-results)
- [Internal API boundaries](#internal-api-boundaries)
- [Regression and runner acceptance](#regression-and-runner-acceptance)

## Implementation gaps

The CLI now emits the versioned result below and separates policy refusal (`78`)
from operational failure (`2`). Publication recipes use `complete-publication`
to keep preparation, checkpointing and pushing in one result. Hook output goes
to stderr. A created commit remains in the result when reconciliation fails;
callers must not repeat that commit automatically.

Checkpoints support Git index versions 2 and 3 with ordinary entries and the
derived `TREE` cache extension. Other formats, symlinked indexes and extensions
whose entry metadata cannot be proved unchanged are refused before commit creation. A preservation rehearsal compares every unaffected entry's
raw entry bytes, including blob ID, mode, stage, flags and stat fields before hooks run, and reconciliation
checks them again. Unsupported metadata combinations fail without replacing the
original index.

URL rewrite chains that would reinterpret an expanded destination are refused.
Normal source hooks and credential configuration remain active. Implicit tag
following and submodule publication are disabled so they cannot add unchecked
updates. These restrictions do not lift the
[runner acceptance hold](#regression-and-runner-acceptance) or recover missing
historical evidence.

## Command interface

Commands run in an existing checkout. `--repo` defaults to the current directory.
Checkpoint messages use `--message` or `-m`.

| Command prefix: `amplihack hygiene workflow` | Operation |
| --- | --- |
| `validate-workspace --repo .` | Check workspace identity and workflow prerequisites without initialising Git. |
| `prepare --repo .` | Register common-Git exclusions before checkout-local runtime writes. |
| `stage --repo .` | Stage only candidates covered by validated task authority. |
| `candidate-tree --repo .` | Print an authorised repair-candidate tree ID using a disposable private index. |
| `checkpoint --repo . --message "workflow source checkpoint"` | Commit authorised candidates through normal hooks while preserving protected entries. |
| `complete-publication --repo . --message "workflow source checkpoint"` | Prepare, checkpoint and publish with one structured result. |
| `publish --repo .` | Validate and push every explicit update to its actual destination. |

Workflow context supplies authority. These commands do not infer permission from
the current index. Missing prerequisites, authority or bounded helpers stop the
operation before staging. Preparation alone is not publication validation.

## Established task scope

Authority comes from requirements and the reviewed design. Both authority
records use schema version `1`, reject unknown authority fields and identify the
same non-empty task. Validation binds the scope to the canonical common Git
directory and checkout. Matching identities establish consistency; they do not
prove that a record came from a trusted review.

The exact filename intersection of requirements `permitted_source_paths` and
design `files_to_change` determines permitted source paths. New files and tests
need authority in both records. A missing or empty intersection, unsupported
version or identity mismatch prevents staging and publication.

Recipe context uses `final_requirements` and `design_spec` as primary inputs.
Compatibility inputs `requirements` and `design_consensus` apply only when their
primary input is absent. Malformed or inconsistent primary authority fails;
it cannot fall back to a more permissive compatibility record.

Paths are repository-relative filenames. Validation rejects absolute paths,
traversal, Git metadata, directory-wide authority, prohibited runtime paths and
unsupported encodings. Literal Git pathspecs preserve valid filename
metacharacters without treating them as patterns. Inventories, pre-existing
staged entries, hook output, tracking refs and `FETCH_HEAD` cannot add authority.
Scope extensions return through requirements and design review.

## Recipe staging policy

Applicable agent roles explicitly set `auto_stage: false` in default and
consensus phases and reachable autodrive recipes. This includes review step
`02` and concern-fix step `04` in `autodrive-crusty-round`.

Checkpoints receive explicit candidates within validated scope. They do not use
`git add -A`, blanket `git add -u` or runner automatic staging. Missing bounded
helpers fail before staging. The policy uses the runner's supported opt-out;
it does not change generic runner semantics.

## Checkpoint transactions

| Stage | Contract |
| --- | --- |
| Validate | Confirm checkout and task binding, exact candidates and supported index state. |
| Protect | Lock and snapshot the real index; identify pre-existing user-staged work. |
| Prepare | Stage authorised candidates into a trusted private index based on `HEAD`. |
| Commit | Run normal commit and index-change hooks; freeze and validate the tree against transaction-owned authority, then create that exact commit object and advance the branch with normal reference hooks. |
| Reconcile | Apply committed candidate entries to a copy of the original index, verify protected entries and install it atomically. |

Protected entries retain their staged bytes, object IDs, modes, stages, flags,
stat data and other Git entry metadata exactly, including staged deletions.
Working-tree bytes can differ from the staged blob. Candidate overlap, unmerged
entries, unsupported index formats or extensions, or an inability to prove
preservation cause refusal before committing. Automatic unstaging is prohibited.

Git subprocesses clear inherited repository, index, configuration and history
overrides, then supply trusted values explicitly. The transaction selects its
private index deliberately; inherited `GIT_INDEX_FILE` is not private-index
authority. Credential handling and normal hooks remain available.

The original validated scope remains in transaction-owned memory. `scope.json`
is an inspection copy and cannot grant authority after a hook edits it.
Validation runs after relevant hook mutations and before commit creation.
Hook-added unauthorised paths cannot enter the commit. If that boundary cannot
be established, the automatic commit is refused. Unexpected `HEAD` changes are
also errors; the index lock alone does not protect branch refs. Commit creation
uses the validated tree object, never a later read of the private index. The
reference update compares the expected old commit and checks that reference
hooks have not changed the frozen private index. Commit-message hooks, configured
message cleanup and signing, reference callbacks and post-commit notifications
remain active. A post-commit hook cannot change the already selected tree.

Repair snapshots use `candidate-tree` with a disposable private index. Retry and
push-fix checkpoints receive the same authorised candidates without first staging
them in the real index. The separate `stage` operation still stages explicitly;
it is not checkpoint preparation.

If commit creation succeeds but reconciliation fails, the original real index
is preserved. The result includes the commit ID and a failing reconciliation
outcome. It does not repeat the commit automatically or consume protected entries
to repair the index.

## Independent runtime guard

`amplihack hygiene artifact-guard --repo . --mode pre-publish` inspects the complete
validated index and staged differences. It rejects all paths beneath
`.amplihack/` and `.claude/runtime/`, including unchanged tracked entries,
force-staged files, deletions and both rename endpoints. Existing worktree
protections also apply.

Strict runtime rejection precedes ignore rules, baseline exemptions, source
manifests and allowlists. Launcher context, session records and locks have no
exception. The guard is read-only and preserves unrelated staged entries.

CLI scans validate the actual checkout index. Internal checkpoints pass an opaque
checkout-bound private-index handle. Arbitrary paths and inherited
`GIT_INDEX_FILE` cannot select a trusted index. Guard violations return `1`;
configuration or inspection failures return `2`. Outgoing-authority refusal is
a separate publication result with status `78`.

## Destination and outgoing history

Publication resolves actual push URLs, which can differ from fetch URLs, and
freezes every proposed object ID and destination ref. The default destination
is `refs/heads/<current-task-branch>`, independent of an inherited base upstream.
Explicit push refspecs retain the same outgoing-history checks. Task publication
refuses `main`, `master`, the configured default branch and the destination's
advertised default branch. It updates upstream tracking only after successful
publication. All destinations and refs are validated before any push. Implicit
tag following is disabled.

The basis is the destination's own complete successful advertisement, obtained
from pre-push input or one `ls-remote` for that destination. Local tracking refs
and `FETCH_HEAD` never narrow validation. For each proposed tip:

```text
outgoing = reachable(proposed tip) - reachable(all advertised remote commit tips)
```

Advertised tags contribute commit tips after peeling. For a new remote ref,
every advertised commit tip still contributes; a missing branch does not make
all local history outgoing. Complete history is acquired in disposable
repositories without trusting local replacements, grafts or shallow boundaries.

Every outgoing commit is checked individually. Root commits are compared with
an empty tree. Ordinary commits need exact path authority, including intermediate
unauthorised changes later reverted. An aggregate final diff is insufficient.

There is no import receipt, `WF_CHECKPOINT_FETCH_RECEIPT` or receipt-based race
policy. A destination change that prevents a normal push produces a transport
refusal. A new explicit attempt acquires a fresh advertisement and revalidates.

## Merge handling

Advertised remote commits are imports. Every outgoing merge still receives its
own check: outgoing parents are validated, and supported two-parent automatic
merges are reconstructed in isolation to identify resolution changes.
Resolutions need exact task authority.

Advertised imports with authorised resolutions are permitted. Unsupported merge
shapes, unauthorised resolutions, incomplete history or reconstruction failures
are refused. First-parent differences alone do not establish merge authority.

## Fetch and push outcomes

A fetch succeeds only after successful command completion and verification of
complete usable objects. Failed or partial fetches stop publication. Process
completion, readiness signals and isolation provide synchronisation; there are
no timers, interval probes, timed retries or GitHub polling.

Pushes use immutable validated object IDs, explicit branch destinations, normal
hooks and ordinary updates. Forced updates, deletion updates, wildcards,
matching refspecs and unsupported forms are refused. Cleanup does not bypass
this boundary to delete a remote ref. Local cleanup remains ownership-based.

| Outcome | Reporting |
| --- | --- |
| Every requested update pushed or already current | `pushed` is `true`. |
| An outgoing commit lacks authority | Exit `78`; identify destination, ref, commit and path. |
| Advertisement, fetch, history, hook or transport failure | Fail and stop dependent PR creation. |
| A later destination fails after an earlier push | Report each completed and failed update; `pushed` is `false`. No rollback is claimed. |

Diagnostics and structured destinations redact credentials. No failure is
recovered with hook bypasses or force-pushes.

## Structured results

A publication step with `parse_json: true` emits one versioned JSON object on
stdout. Diagnostics and hook output go to stderr. Preparation and checkpoint
output must not concatenate extra documents into the publication result.
Asset bootstrap is skipped centrally for `hygiene workflow` operations, which
work directly on the checkout and do not require installed framework assets.

The result contract uses version `1` and includes these fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | integer | Result schema version; unsupported versions fail. |
| `committed` | boolean | This attempt created a checkpoint commit. |
| `commit_id` | string or null | Created commit, including one followed by reconciliation failure. |
| `reconciled` | boolean | Reconciliation completed or was unnecessary; false for a reconciliation failure. |
| `pushed` | boolean | Every requested update succeeded or was already current. |
| `push_reason` | string | `success`, `already_current`, `not_attempted`, `policy_rejected`, `failed` or `partial`. |
| `updates` | array | Per-destination/ref outcomes, with redacted destination, ref, frozen object ID and outcome. |
| `errors` | array of strings | Failures identifying the operation and relevant ref, commit or path. |

Each update outcome is `pushed`, `already_current`, `not_attempted` or `failed`.
Unknown result versions and outcomes are errors. Operational failures retain
non-zero exit status; a JSON object alone never converts them into success.
A no-op checkpoint reports `committed=false`; an already-current push reports
`pushed=true` and `push_reason=already_current`.

PR creation consumes the parsed push outcome through the existing condition:

```text
commit_result.pushed == 'true'
```

Actual recipe parsing verifies Boolean rendering for this condition. A successful
checkpoint alone cannot satisfy it. All downstream consumers receive their
required fields on every outcome, including partial publication.

## Internal API boundaries

These modules define internal contracts rather than a user-editable permission
file. Validated scope, path and private-index constructors remain private.
Workspace binding is rechecked at mutation boundaries.

| Module | Responsibility |
| --- | --- |
| `amplihack_git::task_scope` | Validate versioned authority, identities and exact filename intersection. |
| `amplihack_git::checkpoint_transaction` | Protect the index, stage candidates, validate hook mutations and reconcile. |
| `amplihack_git::isolated_history` | Acquire and inspect complete destination history in disposable repositories. |
| `amplihack_git::publication_validation` | Resolve destinations, freeze updates, validate every ref and perform checked pushes. |
| `amplihack_utils::artifact_guard` | Independently enforce runtime prohibitions against the selected validated index. |

See [runtime preparation interfaces](workflow-runtime-artifacts.md#first-write-preparation-api)
for checkout validation and runtime placement.

## Regression and runner acceptance

Acceptance requires actual recipe settings in disposable repositories without
ignore rules. Removing each applicable `auto_stage: false` makes its corresponding
regression fail, including both `autodrive-crusty-round` roles.

The original nine publication, review-feedback and cleanup combinations must be
recovered and exercised with provenance. A reconstructed matrix is diagnostic;
if the original packet cannot be recovered, the PR records that acceptance gap
explicitly. Historical design approvals do not establish implementation acceptance.

Additional controls cover every pushed ref, new task branches, forged tracking
refs and `FETCH_HEAD`, advertised remote merges, unauthorised resolutions,
unauthorised intermediate commits, distinct fetch/push destinations, and failed
or partial fetches. The advertisement and every-ref rules require independent
review by someone other than their original proposer.

Runner pin `90286bc50b84ded773795d2c8301b689f680db0b` remains under the runner
[#201 ownership hold](https://github.com/rysweet/amplihack-recipe-runner/issues/201).
The authorised sequence is prerequisite merges, runner correction, release,
authorised pin adoption, then the original **25 native template-based removal
controls**. Materialised-directory diagnostics and reconstructed controls cannot
replace native acceptance. This documentation grants no runner change authority.

New review regressions cover fresh worktree creation with inherited base tracking,
hook edits to both scope and index, approved hook formatting, runtime parent
symlink substitution for both trackers, nested launches, repair checkpoints with
protected staged work, and publication from a fresh profile without the bootstrap
opt-out. These are new evidence, not recovered historical acceptance controls.

New macOS follow-up evidence (2026-11-12), separate from the missing historical
packet and runner-held acceptance:

| Check | Result |
| --- | --- |
| `cargo test --locked -p amplihack-git --no-fail-fast` | 31 passed. |
| CLI integration filter `issue_1554` | 79 passed; one runner-held diagnostic ignored. The 24 runtime and three launch controls also passed after the runtime creation fix. |
| Launcher library | 276 passed; seven tracker controls also rerun after the runtime creation fix. |
| CLI `session_tracker::` and `session_log::` | Six passed in each group, including concurrent append/compaction and parent substitution. |
| Utility routing readers and launcher context | 45 and six passed, respectively. |
| Workflow bootstrap exclusion | One passed with no installer calls or output. |
| Library Clippy for Git, utilities and launcher (`-D warnings`) | Passed. |
| `pre-commit run --all-files` | File checks, artifact guard, recipe budget, Git subprocess checks and formatting passed; Clippy failed in unmodified macOS code. |

The Clippy failures include an unused `RESUME_MSG` import in
`bins/amplihack-hooks/tests/limit_resume_binary.rs` and existing unused/dead code
in CLI profile, governor and parent-watch modules. An exploratory CLI library
`session_` filter also reported failures in the unmodified
`switch_uses_injected_login_identity_instead_of_the_session_account` and
`a_dead_holder_does_not_hold_capacity_forever` tests. Its tracker contention
failure was corrected and the focused tracker/compaction groups passed afterward.
These remaining repository check failures do not establish merge readiness.

All fixtures isolate profiles before lookup and never use real user profile
roots. Launch acceptance unsets `HOME` and `XDG_CONFIG_HOME`, verifies unchanged
untracked inventory and runs a repository-owned inventory hook without manifest
changes. Compatible checks run on macOS. Linux CI covers
`guard_rejects_a_runtime_filename_that_apfs_cannot_represent` and applicable
`/proc`, pidfd and FIFO-pool cases; the PR records exact deferred test identifiers.

Delivery evidence separates executed checks, deferred platform checks, missing
historical evidence and dependency holds. An unresolved hold permits a clearly
labelled draft PR, but blocks acceptance and merge-readiness claims. The delivery
PR contains `Fixes #1554`, remains unmerged and leaves subsequent driving to candide.
