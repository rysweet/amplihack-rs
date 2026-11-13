---
title: Workflow classifier reference
description: Requested actions, result destinations and classification boundaries.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workflow classifier reference

`WorkflowClassifier::classify` classifies a request by its intended action.
The classification does not grant additional permissions or determine whether
tracking is required.

## Workflow types

| Rust workflow type | Recipe selection | Purpose |
| --- | --- | --- |
| `Default` | `default-workflow`, with eligible quick-change selection at orchestration | Repository-file edits: features, fixes, refactoring, documentation, configuration and recipes |
| `Investigation` | `investigation-workflow` | Understanding or investigating a system |
| `Ops` | No top-level recipe required | Administration, project management, research reports, coordination and status requests |
| `QAndA` | No top-level recipe required | Direct questions |

`Ops` corresponds to the canonical plan label `Operations`. A decomposed
Operations leaf executes `operations-direct`; top-level Operations can execute
directly or through one checklist agent. Explicit Q&A leaves use the existing
`qa-workflow` route. Consensus selection retains its established rules.

## Classification priority

Requested repository edits take precedence over administrative wording.
Administrative requests without repository edits receive Operations handling.
Other requests retain the existing Investigation, Q&A and Development
classification rules. Empty input returns `Default` with zero confidence.

The classifier returns one classification for the whole request. Decomposition
assigns classifications to executable leaves in mixed work. A top-level label
never authorises dropping a leaf or bypassing its route.

## Operations classification

Operations covers issue triage, issue creation and assignment, research
write-ups, cross-repository coordination and status requests. Research is
classified by its destination: a report or issue comment is Operations;
committing the report into repository files is Development.

Mentioning a source file, test, documentation page or PR does not request a
repository edit. Numbered requirements, a long checklist and an initially
dirty working tree do not establish Development intent.

| Request | Required handling |
| --- | --- |
| Read issue #2553 and its comments; answer the questions in chat | Operations |
| Triage these issues | Operations |
| Open an issue about the routing module and assign an owner | Operations |
| Create an issue about `src/router.rs` | Operations |
| Research these issues and write up the answers in an issue comment | Operations |
| Report test coverage in the status issue | Operations |
| Coordinate reviews across repositories | Operations |
| Write those answers into `docs/operations-routing.md` | Development |
| Triage these issues and update `docs/operations-routing.md` with the findings | Mixed: Operations triage, Development edit |
| Remove a programme name from code and docs across repositories, research questions and open a cross-check issue | Operations-led mixed plan; Development only for repository edits |

## Development near neighbours

Administrative subjects do not hide explicit edits:

| Request | Required handling |
| --- | --- |
| Fix the status bar rendering | Development |
| Refactor the issue-assignment helper | Development |
| Add an agentic disk-cleanup loop | Development |
| Rename a term across documentation files in two repositories | Development leaves for both repositories |
| Write a research report into a tracked documentation file | Development |
| Open an issue about `src/router.rs` and update that file | Mixed: Operations issue creation, Development file edit |
| Explain how `src/router.rs` works in chat | Operations; the path names research material |

An ambiguous mention of documentation does not establish an edit. The
requested output and destination determine the boundary. See
[Operations routing explained](../operations-routing.md).

## Execution contract

The native engine enforces the following boundary. See the
[implementation reference](dev-orchestrator-routing.md#implementation-boundary).

Classification precedes recipe selection and Development setup. Pure
Operations performs no Development clone, worktree, branch or PR preparation,
even with a conflicting custom recipe. Each mixed-plan leaf retains its task,
dependencies and tracking intent through dispatch.

The native classifier, hook guidance, decomposition prompts and installed
recipes must follow these same boundaries. Required execution evidence includes completed
leaf results and recorded actions; registration or prompt-wording checks
alone do not establish correct routing.

## Related

- [Dev-orchestrator routing contract](dev-orchestrator-routing.md)
- [Run Operations and mixed requests](../howto/run-operations-and-mixed-work.md)
- [Coordination messages](coordination-messages.md)
- [Recipe execution flow](../concepts/recipe-execution-flow.md)
