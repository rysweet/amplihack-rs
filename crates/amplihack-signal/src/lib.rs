//! Feature-gated per-session Signal channel for amplihack.
//!
//! This crate is compiled into default builds (so its pure logic — config
//! parsing, wire-format helpers, the trust-boundary gate, and the file-backed
//! inbox — is always testable), but all async TCP I/O against a `signal-cli`
//! JSON-RPC daemon is gated behind the `signal` cargo feature so default builds
//! pull in no async runtime and no signal transport dependency.
//!
//! Module map:
//! - [`config`]   — env > file > explicit-error resolver (fail-loud, never a
//!   silent default). Empty allowlist parses successfully to the empty set =
//!   fail-closed.
//! - [`transport`] — pure `build_*_request` / `parse_incoming` wire helpers
//!   (no I/O, dual inbound envelope shapes) plus the gated tokio client.
//! - [`gating`]   — infallible trust boundary: allowlist AND primary-device
//!   AND groupId-match AND TTL echo-suppression.
//! - [`session_channel`] — file-backed [`session_channel::Inbox`] +
//!   injection-context formatting + the gated per-session `SignalSession`.

pub mod config;
pub mod gating;
pub mod session_channel;
pub mod transport;

pub use config::{ConfigError, GroupMode, SignalConfig};
pub use gating::{Gate, GateDecision, RejectReason};
pub use session_channel::{Inbox, InboxEntry, InboxError, format_injection_context};
pub use transport::{IncomingMessage, ParseError, parse_incoming};
