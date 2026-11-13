---
title: Run Operations and mixed requests
description: Execute administrative work, repository edits and workstreams without tracking issues.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Run Operations and mixed requests

The native engine supports these Operations and mixed-plan configurations.
Each leaf retains its task, classification, dependencies and tracking intent.
Choose a separate private temporary root for each plan and reuse it for resume.
See the [routing boundary](../reference/dev-orchestrator-routing.md#implementation-boundary).

Use an installed amplihack session with an authorised execution context for
each target repository. Issue creation, assignment and comments require an
authenticated GitHub account with permission for the requested action.

## Request administrative work

State the action and its destination. Enter this prompt in the agent session:

```text
For rysweet/amplihack-rs, read issue #2553 and all its comments. Answer in
chat: which actions are Operations, which require repository edits, and
what happens when a workstream has no issue number?
```

Expected result: an Operations answer in the session, or one scoped
checklist agent. Reading the issue does not attach it as tracking. No
Development clone, worktree, branch or PR is created.

Triage, issue assignment, research write-ups outside the repository and
status reports take the same route. Name a concrete repository and assignee
when asking for an issue to be created or assigned.

## Combine research with a repository edit

Give the outputs separate destinations and state their dependency:

```text
For rysweet/amplihack-rs, research the Operations routing requirements in
issue #2553 and return the answers. Then use those answers to update
docs/operations-routing.md. Run both parts without creating tracking issues.
```

Expected result: an Operations leaf produces the answers. A dependent
Development leaf receives them and edits the repository document through
`default-workflow`. The top-level classification cannot bypass that
Development leaf.

For a cross-repository wording sweep, name each repository and the wording to
change. File edits become Development workstreams; research and coordination
remain Operations. An Operations-led plan preserves all of them.

## Run an issue-less workstream

Save this configuration as `operations-workstreams.json` outside the
repository, or in an existing local scratch directory:

```json
[
  {
    "key": "research-routing",
    "classification": "Operations",
    "description": "Research Operations routing",
    "task": "Read issue #2553 and its comments in rysweet/amplihack-rs. Return the Operations and Development boundaries in the result.",
    "issue": "TBD",
    "create_issue": false,
    "recipe": "operations-direct"
  }
]
```

Preview it, then execute from the intended execution directory:

```bash
amplihack multitask run operations-workstreams.json --dry-run
amplihack orch run operations-workstreams.json
```

The preview identifies `research-routing` and `operations-direct`. Execution
produces the research result without a tracking issue or Development
workspace. `TBD` becomes absence before numeric validation. The child context
omits `issue_number`, and no sibling or parent tracking issue is inherited.

Omitting `issue` has the same effect. Blank strings, JSON `null`, and the
[recognised placeholders](../reference/dev-orchestrator-routing.md#optional-tracking-issues)
also mean absence. Unknown strings and invalid numbers remain errors.

## Preserve a dependency in a mixed plan

Save this array as `mixed-workstreams.json`:

```json
[
  {
    "key": "research-routing",
    "classification": "Operations",
    "description": "Research routing requirements",
    "task": "Read issue #2553 in rysweet/amplihack-rs and return its routing requirements.",
    "create_issue": false,
    "recipe": "operations-direct"
  },
  {
    "key": "document-routing",
    "classification": "Development",
    "branch": "feat/document-operations-routing",
    "description": "Document routing requirements",
    "task": "Use the prerequisite result to update docs/operations-routing.md.",
    "depends_on": ["research-routing"],
    "create_issue": false,
    "recipe": "default-workflow"
  }
]
```

```bash
amplihack multitask run mixed-workstreams.json --dry-run
amplihack orch run mixed-workstreams.json
```

The preview preserves both routes. The edit starts only after successful
research, with the prerequisite result identified by `research-routing`.
Failed, partial or cancelled research blocks the edit and reports why.
Sequential execution preserves this dependency and both leaves.

## Supply or request tracking

Set `issue` to `2553`, `"002553"`, `"#2553"`, or
`"https://github.com/rysweet/amplihack-rs/issues/2553"` to attach that existing
issue. All four forms normalise to the integer `2553`. Use an existing issue
only when it actually tracks the work.

To require a new tracking issue, set `create_issue` to `true` and omit
`issue`. The orchestrator acquires and saves a valid issue before executing
the affected leaf. If `issue` is already present, it is reused. Merely
omitting an issue never requests one.

A key allocated before acquisition remains unchanged after attachment.
Preserve the saved plan and state when resuming; do not replace the key with
the newly acquired issue number.

## Recover and inspect safely

For a resumable run, keep a separate stable temporary root for each plan.
Use the same root on initial launch and resume; the canonical plan will be
`$TMPDIR/amplihack-workstreams/state/canonical-plan.json`. Follow the
[exact resume invocation and reconciliation procedure](../reference/workstream-state.md#resumption).
That file is not generated by the current engine.

The current `status --base-dir` command can inspect an existing artefact base.
Current recovery reruns the original, unchanged config with the same `TMPDIR`;
it does not provide canonical-plan recovery or guarantee completed actions are
skipped. See [current recovery limits](../reference/workstream-state.md#current-recovery-limits)
before rerunning work with external effects.

The cleanup preview is:

```bash
amplihack multitask cleanup operations-workstreams.json --dry-run
```

Current cleanup requires a merged PR for the configured branch, including
issue-less work. PR-independent Operations cleanup is pending; an empty
preview does not establish that Operations artefacts are eligible for removal.

## Diagnose a rejected plan

| Observation | Action |
| --- | --- |
| An Operations leaf selects a conflicting recipe | Operations always resolves to `operations-direct`; inspect the canonical plan and installed asset version |
| A documentation edit runs as Operations | Give the edit its own Development leaf |
| An issue value is rejected | Correct the named field or omit optional tracking; never invent a number |
| Duplicate keys or dependency errors | Correct the complete plan before rerunning; validation has launched no work |
| Explicit tracking acquisition fails | Resolve authentication or the service error; the affected leaf has not executed |
| Saved ownership or process observation cannot be verified | Preserve the artefacts and resolve the reported conflict; a PID alone is insufficient |

## Related documentation

- [Routing contract and configuration](../reference/dev-orchestrator-routing.md)
- [Execution schema](../reference/orch-run-command.md#field-reference)
- [Verify routing](verify-dev-orchestrator-routing.md)
