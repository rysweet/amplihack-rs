//! `amplihack-signal`: a feature-gated, per-session Signal messaging channel.
//!
//! The **entire crate is compiled only under the `signal` cargo feature**
//! (default **OFF**). With the feature off, this lib is empty: no modules, no
//! `tokio` net stack, zero runtime cost.
//!
//! # Layout (a "brick" with a pure core + gated I/O shell)
//!
//! - [`config`] — env-first loader (`env > TOML > error`, no silent defaults).
//! - [`transport`] — pure wire helpers ([`transport::build_send_request`],
//!   [`transport::parse_incoming`]) plus the `tokio` TCP JSON-RPC client.
//! - [`gating`] — fail-closed inbound decision (allowlist + device + group +
//!   echo suppression).
//! - [`session_channel`] — [`session_channel::SignalSession`] and the
//!   file-backed [`session_channel::Inbox`].
//!
//! Trust model: inbound Signal text is **data, never commands**. It is only
//! ever surfaced to the agent as `additionalContext`; it is never executed.

#[cfg(feature = "signal")]
pub mod config;
#[cfg(feature = "signal")]
pub mod gating;
#[cfg(feature = "signal")]
pub mod session_channel;
#[cfg(feature = "signal")]
pub mod transport;
