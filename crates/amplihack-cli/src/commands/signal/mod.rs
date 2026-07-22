//! `amplihack signal` — first-class Signal onboarding + fleet distribution (#921).
//!
//! The subcommand is **always** registered in clap (see
//! `commands/mod.rs`). Its implementation lives behind the `signal` cargo
//! feature; a build without the feature returns a clean, non-zero
//! "rebuild with `--features signal`" error rather than silently doing nothing.
//!
//! Layout (pure, unit-tested cores + a thin gated runtime shell):
//! - [`error`] — [`error::SignalOpError`] → stable exit-code taxonomy.
//! - [`validate`] — boundary validation (VM / RG / account / loopback endpoint).
//! - [`render`] — pure device-link-URI → terminal QR rendering.
//! - [`config_writer`] — generates the `signal-config.toml` the channel consumes.
//! - [`distribute`] — resumable per-VM rollout state model + planning.
//! - [`setup`] — 3-probe idempotency planning.
//! - [`seams`] — injectable [`seams::VmLister`] (real impl shells to `az`).
//! - `fsutil` — shared `0600` file writer for the secrets-adjacent config/state.
//! - `run` — the runtime orchestration (signal-cli, daemon, azlin), gated.

#[cfg(feature = "signal")]
pub mod config_writer;
#[cfg(feature = "signal")]
pub mod distribute;
#[cfg(feature = "signal")]
pub mod error;
#[cfg(feature = "signal")]
mod fsutil;
#[cfg(feature = "signal")]
pub mod render;
#[cfg(feature = "signal")]
mod run;
#[cfg(feature = "signal")]
pub mod seams;
#[cfg(feature = "signal")]
pub mod setup;
#[cfg(feature = "signal")]
pub mod validate;

use crate::SignalCommands;
use anyhow::Result;

/// Dispatch a `signal` subcommand (feature build).
#[cfg(feature = "signal")]
pub fn dispatch(command: SignalCommands) -> Result<()> {
    let outcome = match command {
        SignalCommands::Setup(args) => run::run_setup(args),
        SignalCommands::Distribute(args) => run::run_distribute(args),
    };
    match outcome {
        Ok(()) => Ok(()),
        Err(err) => {
            // Surface the actionable message, then exit with the stable code
            // from the taxonomy so tooling can branch on outcomes.
            eprintln!("error: {err}");
            Err(crate::command_error::exit_error(err.exit_code()))
        }
    }
}

/// Dispatch a `signal` subcommand when the `signal` feature is **disabled**.
///
/// No silent no-op: emit clear guidance and exit with the "unsupported" code
/// (3), matching [`error::SignalOpError::Unsupported`].
#[cfg(not(feature = "signal"))]
pub fn dispatch(_command: SignalCommands) -> Result<()> {
    eprintln!(
        "error: the `signal` subcommand requires a build with the `signal` feature.\n\
         Rebuild with:\n    cargo build --release --features signal\n\
         (released amplihack binaries ship with it enabled)."
    );
    Err(crate::command_error::exit_error(3))
}
