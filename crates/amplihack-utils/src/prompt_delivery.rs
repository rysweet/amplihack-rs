//! Subprocess prompt-delivery helper — STUB for the TDD red phase.
//!
//! This module is intentionally incomplete. It establishes the public API
//! surface that the implementation must satisfy and lets the test suite at
//! `tests/prompt_delivery.rs` *compile* while still *failing* — the canonical
//! TDD "tests first" red state.
//!
//! Implementation tracks Simard issue #1897 and the follow-up amplihack-rs
//! issue linked from there. See the design note on Simard #1897 for the
//! full contract, including:
//! - env var: `AMPLIHACK_PROMPT_DELIVERY` (`auto|argv|tempfile|stdin`)
//! - auto-promotion threshold at 4 KiB
//! - deterministic degradation order on unsupported modes
//! - RAII lifecycle for temp files
//!
//! DO NOT MERGE without replacing every `unimplemented_stub!` with a real
//! implementation and removing this header note.

use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Requested delivery mode. `Auto` defers to [`select_mode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptDelivery {
    Auto,
    Argv,
    Tempfile,
    Stdin,
}

/// Per-binary capability descriptor — mirrors the additions proposed for
/// `crates/amplihack-launcher/src/flag_matrix.rs::FlagSet`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryCaps {
    pub supports_argv: bool,
    pub supports_tempfile: bool,
    pub supports_stdin: bool,
    /// CLI flag the binary uses to consume a prompt file (e.g. `--prompt-file`).
    /// `None` means the binary has no documented file-prompt flag.
    pub tempfile_flag: Option<&'static str>,
}

impl DeliveryCaps {
    pub fn argv_only() -> Self {
        Self {
            supports_argv: true,
            supports_tempfile: false,
            supports_stdin: false,
            tempfile_flag: None,
        }
    }

    pub fn argv_and_tempfile(flag: &'static str) -> Self {
        Self {
            supports_argv: true,
            supports_tempfile: true,
            supports_stdin: false,
            tempfile_flag: Some(flag),
        }
    }

    pub fn all_modes(tempfile_flag: &'static str) -> Self {
        Self {
            supports_argv: true,
            supports_tempfile: true,
            supports_stdin: true,
            tempfile_flag: Some(tempfile_flag),
        }
    }
}

/// The mode actually selected after considering caps + env.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMode {
    Argv,
    Tempfile,
    Stdin,
}

/// RAII handle returned by [`deliver`]. Owns temp resources for the lifetime
/// of the spawned child. Drop unlinks any temp file and joins any stdin
/// writer task.
#[derive(Debug)]
pub struct DeliveryHandle {
    mode: DeliveryMode,
    // Real implementation will hold an `Option<tempfile::NamedTempFile>` here
    // and (for stdin mode) a join handle or a writer thread.
    #[allow(dead_code)]
    _opaque: (),
}

impl DeliveryHandle {
    pub fn mode(&self) -> DeliveryMode {
        self.mode
    }

    /// Path to the temp file, if `mode == Tempfile`. `None` otherwise.
    pub fn tempfile_path(&self) -> Option<&Path> {
        // Stub returns None unconditionally — real impl returns the path.
        None
    }
}

impl Drop for DeliveryHandle {
    fn drop(&mut self) {
        // Empty stub Drop: the real impl unlinks the temp file (RAII via
        // tempfile::NamedTempFile) and tears down any stdin writer task.
    }
}

// ---------------------------------------------------------------------------
// Public API — STUBS
// ---------------------------------------------------------------------------

/// The auto-promotion threshold (bytes). Prompts at-or-below this size stay
/// on argv when the binary supports it. See PTY canonical-mode line cap
/// rationale in Simard #1879.
pub const AUTO_TEMPFILE_THRESHOLD_BYTES: usize = 4096;

/// Env-var name that selects the delivery mode at runtime.
pub const ENV_VAR_NAME: &str = "AMPLIHACK_PROMPT_DELIVERY";

/// Parse the [`ENV_VAR_NAME`] environment variable.
///
/// - Unset, empty, or unrecognised values → `Auto` (with a `tracing::warn!`
///   for unrecognised values; quiet for unset/empty).
/// - Case-insensitive: `TempFile`, `TEMPFILE`, and `tempfile` all map to
///   `Tempfile`.
pub fn from_env() -> PromptDelivery {
    // STUB: always returns Auto, ignoring the env. Tests will fail.
    PromptDelivery::Auto
}

/// Resolve the actual delivery mode given a requested mode, the prompt size,
/// and the binary's capabilities.
///
/// Algorithm (see design note for the prose version):
/// 1. If `requested != Auto`: honour it when supported, else degrade
///    deterministically through `Tempfile → Stdin → Argv` and `tracing::warn!`.
/// 2. If `requested == Auto`:
///    a. `prompt_size <= AUTO_TEMPFILE_THRESHOLD_BYTES` + `supports_argv` → `Argv`.
///    b. else `supports_tempfile` → `Tempfile`.
///    c. else `supports_stdin` → `Stdin`.
///    d. else `Argv` + warn.
pub fn select_mode(
    _requested: PromptDelivery,
    _prompt_size: usize,
    _caps: &DeliveryCaps,
) -> DeliveryMode {
    // STUB: always returns Argv — wrong for large prompts and explicit overrides.
    DeliveryMode::Argv
}

/// Apply prompt delivery to a `Command`.
///
/// Mutates `cmd` to either append the prompt as an argv element, append a
/// path to a temp file (using `caps.tempfile_flag`), or configure piped
/// stdin and stage the prompt to be written on spawn. Returns a
/// [`DeliveryHandle`] that the caller MUST keep alive until the child has
/// been waited on.
pub fn deliver(
    _cmd: &mut Command,
    _prompt: &str,
    _requested: PromptDelivery,
    _caps: &DeliveryCaps,
) -> std::io::Result<DeliveryHandle> {
    // STUB: returns an Argv handle but does NOT mutate the command. Tests fail.
    Ok(DeliveryHandle {
        mode: DeliveryMode::Argv,
        _opaque: (),
    })
}
