---
title: Crusty Completion Contract (retired)
description: Retired page. A Crusty round is now one review action of the goal loop and gates nothing.
last_updated: 2026-11-23
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Crusty Completion Contract (retired)

The completion-first dispatch, the `reviewed_head_sha == head_sha` rule and the
no-progress rule of the Crusty loop are retired. A Crusty round
(`autodrive-crusty-round`) is now the `review` action of the
[goal loop](./autodrive-goal-loop.md#actions). Its verdict is evidence in the
next turn's reading, listed with the head it was given at. Whether a review is
converging is the agent's judgement, and nothing stops a run for it.

The `autodrive-crusty-loop` recipe remains for one release as an alias that runs
one review action.
