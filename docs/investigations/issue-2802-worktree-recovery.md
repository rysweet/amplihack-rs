# Issue #2802: private-run worktree recovery

## Investigation checkpoint (2026-11-18)

Read [issue #2802](https://github.com/rysweet/amplihack-rs/issues/2802) in full.
This branch starts from main at `f2ef1f914afda033b97876f437a549f17a85eed3`;
the preceding test checkpoints are `bfa9b778` and `4a3c5a16`.

Confirmed from the recipes before implementation:

- Build preflight uses `fast` since #2722; build step-03 uses `strong`.
  Light resolution is `strong`; merge and Crusty preflight are `fast`.
  Model tier is observable, but its causal effect is unproven. No comparable
  execution demonstrates a reason to change these tiers.
- All five resolution steps ask for the head branch's **existing** linked
  worktree. None explicitly instructs creation when registration is absent.
  Crusty's fetch/fast-forward instructions repair an existing behind checkout;
  they do not recover absence.
- Build preflight reports missing worktree/state as blockers. Step-03 reads
  the full `build_preflight`, but has no explicit instruction to supersede a
  recovered absence. Step-02 intentionally skips implementation for an
  existing OPEN PR; recovery must not change that condition.
- The unchanged Rust Worktree helper validates a supplied checkout, including
  branch and head, and refuses the detached launch checkout. It does not
  create a worktree or prove ownership by the captured run repository.
- The trusted `workflow_worktree_root.sh base` supports bare private-run
  repositories via `AMPLIHACK_RUN_CHECKOUT`. Its placement answer is distinct
  from Git common-directory ownership. Bare `repository.git/worktrees` holds
  Git metadata and cannot be used as the checkout base.

The issue reports both build preflight and strong step-03 failing to recover
on Calypso. This supports repairing the omitted action before considering a
tier change. The Calypso logs were not available here; the specific mechanism
by which an earlier absence influences step-03 remains a hypothesis.

## Recovery contract

For an independently observed OPEN PR, capture the run's checkout and
canonical Git common directory before inspecting candidates. Preserve the
entry point's PR selection restrictions. Reuse valid registrations in that
repository. If genuinely absent, fetch from an identity-associated PR source,
resolve placement through the trusted base helper, and add a separate linked
worktree on the observed branch at the observed full SHA. Preserve existing
history and occupied paths; never reset, force, prune, replace, or repurpose
the detached launch checkout. Dirty contents do not themselves prevent reuse.

Verify common-directory equality, registration, branch, full SHA, and the
existing Worktree helper before publishing `repo`. Initialize private state
through the existing helper. Successful recovery supersedes only historical
absence; retain unrelated blockers and the typed build handoff. Existing
OPEN PRs still skip implementation and complete build-phase resolution.

Only an actual missing-head fetch failure is `no-worktree`, with sanitized
Git diagnostic and exit status. Ownership, identity, creation, and validation
failures remain distinct in existing report fields. Crusty's existing behind,
ahead, and divergence reconciliation is preserved separately; exact observed
SHA is required for newly recovered worktrees. Non-OPEN PRs receive no
recovery mutations. No output schema, helper, guard, retry, timer, or timeout
change is needed for this instruction repair.

## Validation dependency and required evidence

The existing Rust harness executes the shipped steps through the supported
Codex adapter and pinned runner. It requires an explicitly provided
`ISSUE_2802_OPENAI_API_KEY`; operator credential files are never copied.
Its local fixture and helper checks are infrastructure evidence only.
The real-agent cases must prove recovery and downstream consumption, private
registration, exact identity, launch/operator/dirty preservation, historical
absence supersession, concrete fetch failure, occupied-path refusal, allowed
fork heads, and existing Crusty reconciliation. Until executed, neither RED
nor passing recovery reliability is established. Unavailable credentials do
not justify a deterministic recovery helper.

The Gadugi scenario and thin driver already exist at the test checkpoints.
They must never run on this host. CI runs their real-agent suite only in its
trusted main context, keeping provider credentials away from PR code. The PR
and merge-queue jobs explicitly record incomplete release evidence. A pending,
skipped, or prerequisite-failing scenario is not a passing result. Record the
actual run URL, head, job, scenario step, conclusion, and Git-state evidence
when available; do not imply the pre-merge trust restriction has been solved.
