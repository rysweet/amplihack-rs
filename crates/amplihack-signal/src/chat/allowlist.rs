//! Re-export shim: the scoped Copilot tool allowlist now lives in the
//! agent-generic [`amplihack_turn`] crate (issue #910, PR-2).
//!
//! [`ToolAllowlist`] was relocated **behaviour-identical** — nothing about it
//! was Signal-specific. This shim keeps the original path
//! `amplihack_signal::chat::allowlist::ToolAllowlist` valid so existing callers
//! and the PR-1 characterization tests compile and pass unchanged.

pub use amplihack_turn::ToolAllowlist;
