---
title: Verify dev-orchestrator routing
description: Inspect helper output and define required behavioural coverage for native routing.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Verify dev-orchestrator routing

Use helper output to inspect the normalised plan, then run isolated behavioural
checks to establish execution. Classification, context construction and prompt
wording alone do not prove completion or correct setup.

## Prerequisites

Use the intended amplihack binary and its matching installed recipe bundle.
For regression tests, work from a checkout with the Rust toolchain installed.
The required execution fixtures must provide private profiles, state
directories and repositories, without production GitHub issues or the real
user profile. The Rust execution fixture and injected tracking adapter run with private
profiles and record observable service actions and child completion.

## Inspect an Operations-led mixed plan

Save the following as `routing-decomposition.json` in a private scratch
directory:

```json
{
  "task_type": "Operations",
  "workstreams": [
    {
      "key": "research-routing",
      "classification": "Project Management",
      "description": "Research routing requirements",
      "task": "Read issue #2553 in rysweet/amplihack-rs and return the routing requirements.",
      "issue": "TBD",
      "create_issue": false,
      "recipe": "consensus-workflow"
    },
    {
      "key": "document-routing",
      "classification": "Development",
      "description": "Document routing requirements",
      "branch": "feat/document-operations-routing",
      "task": "Use the research result to update docs/operations-routing.md.",
      "depends_on": ["research-routing"],
      "create_issue": false,
      "recipe": "investigation-workflow"
    }
  ]
}
```

Run the helper to inspect output; it prints the generated array's path. Do
not launch this mixed plan to test dependency handling in the current engine:

```bash
WS_FILE=$(amplihack orch helper build-workstreams-config < routing-decomposition.json)
jq '[.[] | {key, classification, recipe}]' "$WS_FILE"
jq '[.[] | has("issue")]' "$WS_FILE"
amplihack multitask run "$WS_FILE" --dry-run
```

**Expected result:** `research-routing` is Operations on `operations-direct`;
`document-routing` is Development on `default-workflow`. The conflicting
Operations recipe is replaced. Both issue-presence checks are false, and the
second leaf retains its dependency on `research-routing`.

The helper preserves leaf keys, classifications, tasks, dependencies and tracking
intent. Dry-run validates the complete plan without launching an agent, cloning
or contacting tracking services. See
[Run Operations and mixed requests](run-operations-and-mixed-work.md) for execution.

## Check Development workflow selection

```bash
printf '%s' '{"recommended_workflow":"quick-change","workflow_reason":"comment typo"}' \
  | amplihack orch helper select-workflow --task-type Development --workstream-count 1
```

Expected stdout is `quick-change`. With `--workstream-count 2`, stdout is
`default-workflow`. The reason is logged on stderr. Leaf routing also requires Operations to retain `operations-direct`
under either Development selection; this helper check does not prove it. See
[Quick-change precedence](../reference/quick-change-recipe.md#precedence) for
existing-branch and override rules.

## Run existing focused checks

Run the focused Rust checks:

```bash
cargo test -p amplihack-workflows classifier
cargo test -p amplihack-cli --test cli_integration issue_2553_
cargo test -p amplihack-cli multitask
```

The issue-specific integration modules include `issue_2553_execution`,
`issue_2553_operations_routing` and `issue_2553_plan_contract`. Execution tests
call the production entry point with private child environments and Rust
agent/service fixtures. They record actual completion, Git and GitHub actions,
dependency results, cancellation, tracking acquisition and resumption. The
installation test executes the installed Operations recipe through the pinned
runner. No production issue is created or assigned by these fixtures.

Record actual command results, skipped checks and platform limits in CI logs
or the PR. No runtime pass or completed execution is asserted by this guide.
JSON parsing, link checks, shell syntax and whitespace validation establish
document structure only.

## Execution coverage

Build isolated checks through production normalisation, preparation and
dispatch paths, with a Rust fixture agent and an injected tracking adapter.
Exercise installed assets and record observable actions and completion;
a fixture-only router cannot establish the production contract.

Required acceptance criteria:

- Pure Operations completes despite a conflicting recipe, without
  Development clone, worktree, branch or PR setup.
- Mixed plans, including Operations-led plans, complete every required leaf
  in dependency order and transfer prerequisite results by immutable key.
- Omitted issues and every supported placeholder reach execution; valid
  positive issue values remain available to tracking consumers.
- Explicit acquisition succeeds before launch, failure prevents affected
  execution, and uncertain acquisition is reconciled without duplicate issues.
- Attaching tracking preserves identities and paths across resume; completed
  leaves are not repeated.
- Cancellation and unsuccessful prerequisites block affected dependants;
  process failure overrides successful agent prose.
- Invalid plans perform no external actions. Unsafe ownership prevents reuse,
  signalling or deletion, and Operations cleanup preserves the repository.

Each child must have a private home, application profile, state directory and
repository fixture. Adapters must reject unexpected external commands. Completion
must use process exits, channels or readiness signals, with no sleeps, interval
probes, timed retries, polling or wall-clock test windows.

Where process observation or FIFO coverage requires Linux, run those checks
in Linux CI once implemented. On macOS, report unavailable Linux checks
separately in the PR; local results do not establish CI coverage.

## Diagnose a mismatch

| Observation | Action |
| --- | --- |
| Operations keeps a custom recipe | Check the canonical plan and installed assets; Operations overrides every conflicting recipe |
| Repository edits disappear from an Operations-led plan | Inspect per-leaf classifications, dependencies and dispatch; the top-level label cannot suppress them |
| `TBD` fails numeric validation | Check the shared optional-issue parser at the failing boundary |
| Tracking is created without `create_issue: true` | Check leaf tracking intent and inherited context; absence is not a creation request |
| Dry-run passes but no result completes | Add the required production-path execution fixture; helper output does not demonstrate execution |
| Resume renames a leaf after issue acquisition | Restore the persisted key and dependency references; tracking must not determine resumed identity |

## Related

- [Routing contract](../reference/dev-orchestrator-routing.md)
- [Workstream state and recovery](../reference/workstream-state.md)
- [Workflow classifier](../reference/workflow-classifier.md)
- [Run Operations and mixed requests](run-operations-and-mixed-work.md)
