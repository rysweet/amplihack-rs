---
title: Verify a PR with Auto-drive Without Merging
description: Complete production readiness and leave an open PR for its steward.
last_updated: 2026-11-12
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Verify a PR with Auto-drive Without Merging

Use `merge=handoff` to run the normal build, reviews, fixes and all 13 readiness
criteria, then leave the PR open and ready for its steward. `no_merge` controls
only the build phase; it does not prevent the outer drive from merging.

## Select the PR and mode

Use the full composer with authenticated GitHub access and the repository's
required test tools. Supply the actual PR number; it takes precedence over
branch discovery. Preserve local work and use the PR's linked worktree.

```bash
amplihack recipe run auto-drive-to-merge \
  -c repo_path=. \
  -c pr_number=2555 \
  -c merge=handoff
```

After readiness passes, expect an open, non-draft PR, exactly one confirmed
comment saying “This pull request is ready for its steward.” and a durable
`handed_off` outcome. Handoff never requests or schedules a merge. An enabled
GitHub auto-merge request blocks handoff; clear it before running again.

For a repository default, commit this file on the PR's **target base branch**:

```toml
# .amplihack/auto-drive.toml
merge = "handoff"
```

Then omit `-c merge`, or pass an empty value. Explicit non-empty input wins over
committed base policy; otherwise an absent file or key defaults to `merge`.
Only lower-case `merge` and `handoff` are valid after surrounding whitespace
is trimmed. Invalid applicable configuration and failures to read the
base's committed tree stop completion. PR-branch, worktree and global settings
do not supply policy. `merge_method` remains independent.

To select the protected merge path explicitly:

```bash
amplihack recipe run auto-drive-to-merge -c pr_number=2555 -c merge=merge
```

## Resume incomplete work

Correct the named blocker and run again with `merge=handoff`, unless the
committed base policy already selects it. Every invocation captures fresh
authority. Stored or recovered context cannot grant a drive action.

Pass the same absolute state directory to retain progress and consumed budgets:

```bash
amplihack recipe run auto-drive-to-merge \
  -c pr_number=2555 \
  -c merge=handoff \
  -c autodrive_state_dir="$HOME/.amplihack/state/auto-drive/pr-2555" \
  -c autodrive_budget_loop_rounds=20
```

The state path must be absolute, without `..` or control characters. Do not
change its permissions to make historical evidence trusted. A changed head
requires fresh readiness; a changed base also requires fresh policy resolution.

A lost comment-creation response or failed state write can leave a public
comment without completed handoff. Resume with fresh readiness: reconciliation
checks every page and reuses the authenticated author's exact existing comment.
Unreadable pages, duplicate matches, failed ready conversion and persistence
failures remain visible. A previous comment alone never proves readiness.
No completion failure falls through to merging.

## Inspect the result

Readiness evidence stays in private drive state, without being appended to
the PR body. The change description must still be useful and read back during
readiness. Check the validated PR/head/base, all criterion evidence, confirmed
comment identity and the matching durable completion and outcome snapshots.
A helper's exit status or a public comment alone is insufficient.

`autodrive_dry_run=true` separately suppresses merging and follow-up creation;
it cannot publish successful handoff. Missing required follow-ups still block
readiness. Direct and light entrypoints remain restricted through escalation.

Controlled Rust tests validate production actions with isolated hosting
responses. They do not establish live agent recovery or the complete OS/runtime
matrix. Report local passes, observed CI results and named unavailable tests
separately in the delivery PR.

See [workflow options](../reference/auto-drive-to-merge.md),
[evidence and outcomes](../reference/auto-drive-evidence.md), and
[QA configuration](../reference/auto-drive-qa.md).
