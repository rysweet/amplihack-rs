//! Phrases that turn a reader into an orchestrator (issues #1333, #1336).
//!
//! Test-only. The #1333 guard test keeps these phrases out of files every
//! agent loads, and the `run_guidance` tests check that the generated
//! operator-instructions preambles hold none of them. Operator text itself is
//! not filtered (PR #2567). The guard test includes this file with `#[path]`;
//! do not add a second copy.

/// Instruction text that turns a reader into an orchestrator. Any of these in a
/// file that every agent loads is a re-entry hazard, whatever the intent.
pub(crate) const ROUTING_MARKERS: &[(&str, &str)] = &[
    (
        "auto-intent-router",
        "the routing prompt's own source marker",
    ),
    (
        "auto-routed",
        "the literal token an agent is told to emit before orchestrating",
    ),
    (
        "AMPLIHACK_CONTEXT_START",
        "the generated-context marker; nothing writes it any more",
    ),
    (
        "recipe run smart-orchestrator",
        "the shell-out that turns a leaf step into a nested orchestration",
    ),
    (
        "Skill(skill=\"dev-orchestrator\")",
        "a direct instruction to invoke the orchestrator skill",
    ),
    (
        "invoke the dev-orchestrator skill",
        "the same instruction in prose",
    ),
    (
        "launching dev-orchestrator",
        "the phrase an agent is told to emit as it re-enters",
    ),
];
