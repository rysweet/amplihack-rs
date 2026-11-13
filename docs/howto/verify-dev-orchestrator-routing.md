---
title: Verify dev-orchestrator routing
description: Exercise leaf routing, optional issues, tracking and resumption through isolated Rust fixtures.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Verify dev-orchestrator routing

> **[IMPLEMENTED - Validation Pending]** Feature acceptance is pending. The five
> `issue_2553_*` modules listed below are absent from the current tree and are
> not registered in `crates/amplihack-cli/tests/main.rs`. Their names describe
> required coverage, not tests that have run or passed.

Use this procedure after implementing and registering the Rust behavioural
fixtures. They must verify the full path from decomposition through configuration
to execution. Valid JSON and local links alone do not establish routing behaviour.

## Prerequisites

Run from the repository root with the pinned Rust toolchain and recipe runner
available. The fixtures must use temporary repositories, homes, profile roots,
configuration, runtime and temporary directories. They clear inherited agent
and issue-specific configuration and use private tmux sockets where needed.
They must never access the real `~/.claude`, `~/.amplihack`, `~/.codex` or profile root.

## Implement and register coverage first

Implement these proposed integration modules, then register them with `mod`
declarations and paths in [tests/main.rs](../../crates/amplihack-cli/tests/main.rs).
Registration is required because `cli_integration` uses that explicit entry point.

| Planned module (not yet registered) | Required behaviour |
| --- | --- |
| `issue_2553_plan_contract` | Full-plan validation, classification precedence, dependencies, optional issue parsing through `u64::MAX`, serialisation and stable identities |
| `issue_2553_operations_routing` | Administrative examples, mixed requests, tracked documentation edits, recipe coercion, per-leaf overrides and dirty initial repositories |
| `issue_2553_tracking_resolution` | Supplied or persisted issue reuse, explicit producer/output bindings, required creation, definite failure, ambiguous outcomes and no unsolicited creation |
| `issue_2553_dependency_execution` | Readiness, single launch, completion events, log EOF, typed results, hand-over validation, result persistence, blocked dependants and sequential fallback |
| `issue_2553_state_ownership` | Multiple issue-less leaves, resume, interrupted external mutations, legacy adaptation and cancellation/cleanup ownership |

Include reconciliation success, proven non-application with explicit retry,
unresolved and partial outcomes, stale revisions, duplicate resolution
submissions, ownership rejection and failed durable writes in the tracking and
state modules. Verify that resolving one action cannot replay earlier successful
actions or release dependants before the whole prerequisite succeeds.

## Run the registered behavioural fixtures

After implementation and registration, inventory the selected tests first:

```bash
cargo test --locked -p amplihack-cli --test cli_integration issue_2553_ -- --list
```

Expected result after registration: fully qualified test names appear under all
five modules. If no tests are listed or a required module is missing, stop:
acceptance remains pending. A successful command with zero selected tests is
not a passing feature check.

Then execute the fixtures:

```bash
cargo test --locked -p amplihack-cli --test cli_integration issue_2553_
```

Expected result after implementation: the selected applicable tests execute and
pass. Each execution fixture must record actual runner invocations, child
contexts, external-adapter calls and owned artefacts. Isolated Rust executables
stand in for agents and external tools; these tests must not create live GitHub
issues. This guide records no feature pass result before those fixtures exist.

## Check the acceptance scenarios

Use the fixture traces to check these outcomes:

1. Triage, issue creation and assignment, research answers, status and
   coordination reach `operations-direct` with one scoped task. There are no
   Development workflow, clone, worktree, branch or implementation-PR calls.
2. A mixed request executes each leaf once. The motivating programme-name
   request and cross-repository documentation edits send only file changes
   through Development; prerequisite results arrive with provenance.
3. An issue-less decomposition reaches its execution fixture without issue
   context. Multiple issue-less leaves have separate identity, state, logs,
   artefact directories and process tracking.
4. Missing, null and blank issue values, all recognised placeholders with case
   and whitespace variants, valid positive numbers and malformed values follow
   the [shared parser contract](../reference/dev-orchestrator-routing.md#optional-issue-parsing).
5. Required tracking creates a validated fixture issue before downstream execution.
   Persisted success is reused. Definite failure remains visible, and unknown
   outcomes do not trigger a second creation attempt. A producer-bound issue
   resolves only from its declared output with matching UUID and repository.
6. Sequential mode, depth limits, strict delivery, resume and fallback retain
   the same leaves, routes and dependencies. Completed Operations are skipped;
   uncertain external mutations require reconciliation.
   Supported reconciliation durably records the verified outcome; only an
   explicitly authorised, proven-safe retry permits a new attempt.
7. Failed prerequisite persistence or invalid completion evidence prevents
   dependant launch. Cleanup leaves the Operations repository intact and
   rejects ambiguous ownership.
8. Every Development result carries its effective workflow. A `quick-change`
   hand-over receives `default-workflow` validation in serial and parallel
   execution; parent context loss cannot skip the gates.

Fixture synchronisation must use process completion, events, readiness signals and
isolation. Sleeps, polling, interval probes, timed retries and wall-clock test
windows are not used. Source-text compliance scans do not satisfy these checks.

## Check interpretation separately

Run the [calibration requests](../reference/workflow-classifier.md#calibration-cases)
through the model-backed classifier in an isolated environment. Compare its
classifications, executable leaves, output destinations and dependencies with
the expected interpretation. Include the original motivating request.

Report calibration separately from deterministic fixture routing. A fixture
that supplies `Operations` as input proves the handling of that label; it
does not establish that the model recognised an administrative task in prose.

## Validate on macOS and Linux

After registration, run the applicable Rust tests on macOS and Linux. Preserve
platform gating for tests that actually require Linux facilities such as
`/proc`, pidfd ownership checks or FIFO pools. Do not classify an entire proposed
module as Linux-only without inspecting its implemented tests.

Record actual commands, exit statuses and executed/passed/failed counts in the
implementation PR and CI logs. Compare the macOS and Linux `-- --list`
inventories and inspect each difference's platform gate. List every Linux-only
omission by its exact fully qualified Rust test name, gate/reason and Linux CI
result or pending job. Also record ignored tests, runtime skips and unavailable
tools; listing a test does not prove its body ran. No exact omission inventory
can be claimed for the five proposed modules before they exist. Until both
inventories and actual results are available, platform validation is pending.

Run applicable formatting, Clippy and configured hooks without bypasses,
including `pre-commit run --all-files`. Do not disable hooks, force-push or
change versions. The implementation PR includes `Fixes #2553`; candide owns
the subsequent PR drive and merge.

## Diagnose failures

| Observation | Check |
| --- | --- |
| Operations starts `default-workflow` | Route coercion, fallback handling and bundle/binary agreement |
| A documentation edit runs directly | Action/destination classification and the leaf's explicit classification |
| Missing tracking stops execution | Shared issue parser and omitted context at every boundary |
| A sibling's issue leaks into a child | Inherited variable removal and per-leaf context construction |
| Issue creation repeats on resume | Stable identity, durable tracking attempt and reconciliation state |
| A dependant starts before durable success | Completion validation and persistence ordering |

See the [routing contract](../reference/dev-orchestrator-routing.md) and
[state reference](../reference/workstream-state.md) for the corresponding invariants.
