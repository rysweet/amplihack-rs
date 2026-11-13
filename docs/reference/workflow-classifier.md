---
title: Workflow classifier
description: Action-based classification for Operations, repository edits and mixed requests.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workflow classifier

> **[IMPLEMENTED - Validation Pending]** These issue #2553 classification and
> leaf-plan rules describe the intended feature. Calibration and deterministic
> fixture coverage must be implemented and run before acceptance.

The classifier identifies the required action and destination of each result.
The routing hook, deterministic helpers and dev-orchestrator share these
boundaries. The [routing contract](dev-orchestrator-routing.md) defines the
canonical labels and executable plan.

## Classification boundaries

| Action | Canonical orchestration classification |
| --- | --- |
| Issue triage, creation or assignment | `Operations` |
| Research answers in chat or issues, status reports, coordination | `Operations` |
| Edits to tracked code, documentation, configuration or recipes | `Development` |
| Investigation into how an existing implementation works | `Investigation` |
| Ordinary explanation question | `Q&A` |
| Structured decision-making | `Consensus` |

`Operations` includes project management. `Ops`, `PM`, `Project Management`
and `project-management` are accepted aliases at classification boundaries;
normalised plans use `Operations`.

The lower-level classifier retains compatible workflow variants such as
`Default`, `Ops` and `Verification`. `Default` represents the Development
workflow family; `Ops` maps to canonical Operations. Existing Investigation
and Verification behaviour remains in force. These internal names do not
introduce alternative public labels for a normalised leaf plan.

## Action and destination take priority

Generic verbs such as “write”, “create”, “add” and “build” are not sufficient
Development evidence. Nor are requirement counts, complexity, a repository
name or a file mentioned as research material.

A requested tracked-file change is Development evidence, including a
documentation-only wording change. An administrative action stays Operations
even when it has a long checklist. Reading code to answer a question does not
by itself request a code change.

Operational phrases in a feature request do not change the requested action.
“Implement a disk-cleanup loop in the CLI” requests repository changes and is
Development. “Run disk cleanup on a staging host” requests an operation.

## Mixed-request handling

A request combining administration and repository edits is decomposed into
explicitly classified executable leaves. Each leaf retains its own route.
Dependencies preserve required ordering, including cross-repository work.
The original mixed request is not also executed by an Operations agent.

Per-leaf classification fields take precedence in this order:
`classification`, `task_type`, `type`. Top-level classification is a compatible
homogeneous fallback. New mixed plans require explicit leaf classifications.

## Calibration cases

| Request | Expected interpretation |
| --- | --- |
| “Triage these issues” | Operations |
| “Open an issue for X and assign Y” | Operations; X and Y are supplied task and owner details |
| “Write up the answers to these questions” | Operations when the answers belong in chat or an issue |
| “Write those answers into docs/operations-routing.md” | Development |
| “Rename a term across the docs of two repositories” | Development leaves for the repository edits |
| “Create an issue, then change a tracked document using it as tracking” | Explicit tracking resolution followed by Development; no duplicate issue-creation leaf |
| “Read a routing module and report its behaviour in chat” | Research answer; no tracked-file change |
| “Remove a programme name from code and docs, answer research questions and open a cross-check issue” | Mixed Development and Operations leaves with relevant dependencies |

Fixture-supplied classifications exercise deterministic routing. They do not
prove model interpretation quality. Calibration evaluates the original prose
separately, including output destination, decomposition, dependencies and
the absence of duplicated administrative actions.

## Source and related documentation

- [Classifier implementation](../../crates/amplihack-workflows/src/classifier.rs)
- [Operations routing explained](../operations-routing.md)
- [Run Operations and mixed requests](../howto/run-operations-and-mixed-work.md)
- [Verify routing behaviour](../howto/verify-dev-orchestrator-routing.md)
- [Recipe execution flow](../concepts/recipe-execution-flow.md)
