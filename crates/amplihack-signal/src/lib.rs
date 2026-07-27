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
//! - [`signal_channel`] — [`signal_channel::SignalChannel`], the
//!   [`amplihack_turn::Channel`] that runs `amplihack signal chat` on the
//!   crate-generic turn loop (fail-closed inbound gate, bounded turn queue,
//!   control phrases, fail-closed outbound membership re-check).
//! - [`chat`] — the `/signal` topic chat core (deterministic group naming,
//!   control-phrase parsing, scoped tool allowlist, outbound redaction +
//!   Signal-sized chunking, fail-closed membership verification, and the
//!   serialized Copilot turn driver).
//!
//! Trust model: inbound Signal text is **data, never commands**. It is only
//! ever surfaced to the agent as `additionalContext`; it is never executed.

#[cfg(feature = "signal")]
pub mod chat;
#[cfg(feature = "signal")]
pub mod config;
/// Test-only, in-process loopback fake of the signal-cli daemon.
///
/// Not part of the stable public API: it exists solely so cross-crate
/// integration tests (in `amplihack-signal/tests` and `amplihack-cli/tests`)
/// can exercise the real transport hermetically. It must stay `pub` for those
/// external test crates to reach it, but is `#[doc(hidden)]` so it is not
/// advertised as library surface.
#[cfg(feature = "signal")]
#[doc(hidden)]
pub mod fake_endpoint;
#[cfg(feature = "signal")]
pub mod gating;
#[cfg(feature = "signal")]
pub mod signal_channel;
#[cfg(feature = "signal")]
pub mod transport;
