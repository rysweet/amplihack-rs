# Amplihack Agents

This repository's agent instructions are delivered at runtime, not from this
file. Do not add agent guidance here.

An earlier revision carried a generated context block: a verbatim copy of the
routing prompt plus the body of the `dev-orchestrator` skill, both phrased as
mandatory instructions. It was committed by accident in `d3341a78` ("test:
harden PR 712 asset mapping refactor"), and no code writes it — the writer was
removed by issue #862, which names this file "the instruction channel Copilot
re-ingests". The artifact outlived its writer.

Because this file sits at the repository root and is tracked, every git
worktree the workflow creates carried a copy, and Copilot CLI loads it
unconditionally as a custom instruction. Every leaf agent step — including
recipe steps already running inside an orchestration — was therefore told to
invoke the orchestrator skill and shell out to a nested workflow run. That is
what made design steps spawn whole nested orchestrations, and it is the root
cause of issues #1333 and #1336.

Routing now reaches an agent through one channel only: the
`workflow-classification-reminder` hook, which is gated on the recipe-run
provenance marker (issue #1328) so it reaches a human's prompt and never an
agent-authored recipe step.

`crates/amplihack-cli/tests/issue_1333_leaf_steps_do_not_orchestrate.rs` keeps
this file free of routing instructions. Put provenance-gated guidance in the
hook, not here.
