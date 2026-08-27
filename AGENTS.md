# Amplihack Agents

Agent instructions are delivered at runtime through the
`workflow-classification-reminder` hook, which is gated on recipe-run
provenance so it reaches a human's prompt and never an agent's own step.

Do not put agent guidance here. Every git worktree carries a copy of this file
and CLI agents load it unconditionally, so instructions written here reach
agents that are already inside an orchestration and tell them to start another
one — see issues #1333 and #1336, and the guard in
`crates/amplihack-cli/tests/issue_1333_leaf_steps_do_not_orchestrate.rs`.
