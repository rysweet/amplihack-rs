//! Re-export shim: the Copilot turn-driver primitives now live in the
//! agent-generic [`amplihack_turn`] crate (issue #910, PR-2).
//!
//! [`build_turn_argv`], [`TurnRunner`], [`SerialTurnDriver`],
//! [`CopilotTurnRunner`], and [`PreemptSlot`] were relocated
//! **behaviour-identical** — they were already agent-generic (nothing
//! Signal-specific). This shim keeps their original paths under
//! `amplihack_signal::chat::turn::*` valid so existing callers (the CLI chat
//! subcommand) and the PR-1 characterization tests compile and pass unchanged.

pub use amplihack_turn::{
    CopilotTurnRunner, PreemptSlot, SerialTurnDriver, TurnRunner, build_turn_argv,
};
