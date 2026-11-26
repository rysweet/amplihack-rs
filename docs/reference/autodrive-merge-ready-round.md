---
title: Auto-drive Merge-ready Round (retired)
description: Retired page. The merge-ready round, merge and finalize steps are now turns of the goal loop.
last_updated: 2026-11-23
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Merge-ready Round, Merge and Hand-off (retired)

The merge-ready round, its parsed one-word outcome, `finalize`, the automatic
base merge and the hand-back to draft are retired. Readiness is now the agent's
judgement from a fresh reading each turn, with its evidence in the turn record.

- The loop, readings, actions and endings: [Auto-drive Goal Loop](./autodrive-goal-loop.md)
- Inputs and operator how-to: [Auto Drive To Merge Reference](./auto-drive-to-merge.md)

The recipes `autodrive-merge-round`, `autodrive-merge-evidence` and
`autodrive-finalize` remain for one release as aliases that each run one turn.
The hand-off recipe `autodrive-handoff` names the owner and the decision and no
longer converts the PR to draft.
