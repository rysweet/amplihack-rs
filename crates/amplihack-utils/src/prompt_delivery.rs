//! Subprocess prompt-delivery helper for amplihack-rs.
//!
//! Implements the three delivery modes (`argv` / `tempfile` / `stdin`) plus
//! an `auto` selector and an `AMPLIHACK_PROMPT_DELIVERY` env-var override.
//!
//! Tracking issue: Simard #1897. Working reference (shell-mktemp pattern):
//! Simard #1878. Originating apostrophe-truncation bug: Simard #1871.
//!
//! ## Contract summary
//!
//! - **Env var:** `AMPLIHACK_PROMPT_DELIVERY` ∈ `{auto, argv, tempfile, stdin}`,
//!   case-insensitive. Unset / empty / unrecognised values all resolve to
//!   `auto` (unrecognised emits a `tracing::warn!`).
//! - **Auto rule:** prompts ≤ [`AUTO_TEMPFILE_THRESHOLD_BYTES`] stay on
//!   `argv` when the target binary supports it; otherwise the helper picks
//!   `tempfile`, then `stdin`, then falls back to `argv`.
//! - **Explicit override:** wins over `auto`. If the requested mode is not
//!   supported by the binary, the helper degrades deterministically through
//!   `tempfile → stdin → argv` and emits a `tracing::warn!`.
//! - **Lifecycle:** [`deliver`] returns a [`DeliveryHandle`] that owns the
//!   underlying [`tempfile::NamedTempFile`] (for `tempfile` mode) or marks
//!   the child stdin as piped (for `stdin` mode). The handle MUST be kept
//!   alive until the child has been waited on; dropping it unlinks the
//!   temp file.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::NamedTempFile;

/// Strip all NUL (`0x00`) bytes from `prompt`, preserving every other byte in
/// order.
///
/// A NUL byte cannot be passed through to a child process: argv elements are
/// C strings terminated by NUL, so [`std::process::Command::arg`] rejects
/// interior NULs and aborts the spawn (`nul byte found in provided data`).
/// Issue #898: a single stray NUL in agent/bash step output was killing the
/// entire recipe-runner workflow. Rather than reject the whole prompt, we
/// sanitize at this boundary so downstream steps continue.
///
/// Fast path: when the prompt contains no NUL byte the input is returned
/// unchanged as [`Cow::Borrowed`] (zero-copy). Only when a NUL is present do
/// we allocate an owned, filtered copy and emit a count-only
/// [`tracing::warn!`] (never the prompt content, which may hold secrets).
pub fn sanitize_prompt_nul(prompt: &str) -> Cow<'_, str> {
    if !prompt.as_bytes().contains(&0) {
        return Cow::Borrowed(prompt);
    }

    // Copy the runs between NULs in bulk (memcpy per chunk) into a single
    // pre-sized buffer, rather than decoding/re-encoding each char. `split`
    // drops the NUL separators, so `extend` appends only NUL-free slices.
    let mut sanitized = String::with_capacity(prompt.len());
    sanitized.extend(prompt.split('\0'));
    // Each stripped NUL is a single byte, so the byte-length delta is the count.
    let removed = prompt.len() - sanitized.len();
    tracing::warn!(
        removed_nul_bytes = removed,
        "stripped NUL byte(s) from prompt before subprocess delivery (issue #898)"
    );
    Cow::Owned(sanitized)
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Caller-requested delivery mode. `Auto` defers to [`select_mode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptDelivery {
    Auto,
    Argv,
    Tempfile,
    Stdin,
}

/// Per-binary capability descriptor.
///
/// Mirrors the additions made to
/// [`crate::flag_matrix::FlagSet`](`crates/amplihack-launcher/src/flag_matrix.rs`)
/// — kept here as a stand-alone struct so `amplihack-utils` does not depend
/// on `amplihack-launcher`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryCaps {
    pub supports_argv: bool,
    pub supports_tempfile: bool,
    pub supports_stdin: bool,
    /// CLI flag the binary uses to consume a prompt file (e.g.
    /// `--prompt-file`). `Some("")` means "append only the path with no
    /// preceding flag" (used by harness binaries like `cat`). `None` means
    /// the binary has no documented file-prompt flag — interpreted the same
    /// as `Some("")` if [`Tempfile`](DeliveryMode::Tempfile) is selected.
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

/// Mode actually selected after considering caps + env.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMode {
    Argv,
    Tempfile,
    Stdin,
}

/// RAII handle returned by [`deliver`].
///
/// Owns the temp resources required for the lifetime of the spawned child.
/// Dropping the handle unlinks any temp file. The caller MUST keep the
/// handle alive until [`std::process::Child::wait`] (or equivalent) has
/// returned — otherwise the child will see the temp file disappear.
#[derive(Debug)]
pub struct DeliveryHandle {
    mode: DeliveryMode,
    path: Option<PathBuf>,
    // Order matters for drop semantics: `_tempfile` is unlinked when the
    // struct is dropped, so the field is declared last to drop last.
    _tempfile: Option<NamedTempFile>,
}

impl DeliveryHandle {
    pub fn mode(&self) -> DeliveryMode {
        self.mode
    }

    /// Path to the temp file, if `mode == Tempfile`. `None` otherwise.
    pub fn tempfile_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Auto-promotion threshold (bytes). Prompts strictly larger than this
/// promote out of `argv` when the binary supports a longer-form channel.
/// Rationale: PTY canonical-mode line cap per Simard #1879.
pub const AUTO_TEMPFILE_THRESHOLD_BYTES: usize = 4096;

/// Env-var name that selects the delivery mode at runtime.
pub const ENV_VAR_NAME: &str = "AMPLIHACK_PROMPT_DELIVERY";

// ---------------------------------------------------------------------------
// from_env
// ---------------------------------------------------------------------------

/// Parse [`ENV_VAR_NAME`] from the process environment.
///
/// - Unset or empty → [`PromptDelivery::Auto`] (silent).
/// - Recognised value → matching variant (case-insensitive).
/// - Unrecognised → [`PromptDelivery::Auto`] + `tracing::warn!`.
pub fn from_env() -> PromptDelivery {
    let raw = match std::env::var(ENV_VAR_NAME) {
        Ok(v) => v,
        Err(_) => return PromptDelivery::Auto,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return PromptDelivery::Auto;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "auto" => PromptDelivery::Auto,
        "argv" => PromptDelivery::Argv,
        "tempfile" => PromptDelivery::Tempfile,
        "stdin" => PromptDelivery::Stdin,
        other => {
            tracing::warn!(
                env = ENV_VAR_NAME,
                value = other,
                "unrecognised prompt-delivery mode; falling back to auto. \
                 Valid values: auto, argv, tempfile, stdin (case-insensitive)."
            );
            PromptDelivery::Auto
        }
    }
}

// ---------------------------------------------------------------------------
// select_mode
// ---------------------------------------------------------------------------

/// Resolve the actual delivery mode for a request.
///
/// See module-level docs for the algorithm. Both the `auto` rule and the
/// explicit-override degradation chain prefer `tempfile → stdin → argv`
/// when their preferred mode is unsupported.
pub fn select_mode(
    requested: PromptDelivery,
    prompt_size: usize,
    caps: &DeliveryCaps,
) -> DeliveryMode {
    match requested {
        PromptDelivery::Auto => select_auto(prompt_size, caps),
        PromptDelivery::Argv => {
            if caps.supports_argv {
                DeliveryMode::Argv
            } else {
                tracing::warn!("argv mode unsupported by binary capabilities; degrading");
                degrade_chain(caps)
            }
        }
        PromptDelivery::Tempfile => {
            if caps.supports_tempfile {
                DeliveryMode::Tempfile
            } else {
                tracing::warn!("tempfile mode unsupported by binary capabilities; degrading");
                if caps.supports_stdin {
                    DeliveryMode::Stdin
                } else {
                    DeliveryMode::Argv
                }
            }
        }
        PromptDelivery::Stdin => {
            if caps.supports_stdin {
                DeliveryMode::Stdin
            } else {
                tracing::warn!("stdin mode unsupported by binary capabilities; degrading");
                DeliveryMode::Argv
            }
        }
    }
}

fn select_auto(prompt_size: usize, caps: &DeliveryCaps) -> DeliveryMode {
    if prompt_size <= AUTO_TEMPFILE_THRESHOLD_BYTES && caps.supports_argv {
        return DeliveryMode::Argv;
    }
    if caps.supports_tempfile {
        return DeliveryMode::Tempfile;
    }
    if caps.supports_stdin {
        return DeliveryMode::Stdin;
    }
    if caps.supports_argv {
        tracing::warn!(
            prompt_size,
            "no long-form delivery mode supported by binary; falling back to argv. \
             Large prompts may be truncated or corrupted by shell-escape limits."
        );
        DeliveryMode::Argv
    } else {
        tracing::warn!(
            prompt_size,
            "binary advertises no supported delivery modes; defaulting to argv"
        );
        DeliveryMode::Argv
    }
}

fn degrade_chain(caps: &DeliveryCaps) -> DeliveryMode {
    if caps.supports_tempfile {
        DeliveryMode::Tempfile
    } else if caps.supports_stdin {
        DeliveryMode::Stdin
    } else {
        DeliveryMode::Argv
    }
}

// ---------------------------------------------------------------------------
// deliver
// ---------------------------------------------------------------------------

/// Apply prompt delivery to a [`Command`].
///
/// Mutates `cmd` as required by the selected mode:
/// - [`Argv`](DeliveryMode::Argv): appends the prompt as a single argv element.
/// - [`Tempfile`](DeliveryMode::Tempfile): writes the prompt to a fresh
///   `tempfile::NamedTempFile` with mode `0600` (Unix), then appends
///   `caps.tempfile_flag` (when non-empty) followed by the path.
/// - [`Stdin`](DeliveryMode::Stdin): configures the child's stdin as piped.
///   The caller is responsible for writing the prompt bytes to
///   [`std::process::Child::stdin`] and then closing it before waiting.
///
/// Returns a [`DeliveryHandle`] whose lifetime guards the temp resources.
pub fn deliver(
    cmd: &mut Command,
    prompt: &str,
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> std::io::Result<DeliveryHandle> {
    // Issue #898: strip NUL bytes so a single stray NUL in agent/bash step
    // output cannot abort the whole workflow at child spawn. Mode selection
    // uses the *original* prompt length (not the sanitized length) to stay
    // consistent with stdin-path callers that compute select_mode on the raw
    // prompt. Sanitization affects only the delivered bytes.
    let mode = select_mode(requested, prompt.len(), caps);
    let prompt = sanitize_prompt_nul(prompt);
    let prompt = prompt.as_ref();
    match mode {
        DeliveryMode::Argv => {
            cmd.arg(prompt);
            Ok(DeliveryHandle {
                mode,
                path: None,
                _tempfile: None,
            })
        }
        DeliveryMode::Tempfile => {
            let mut named = tempfile::Builder::new()
                .prefix("simard-prompt-")
                .tempfile()?;
            named.write_all(prompt.as_bytes())?;

            // tempfile::Builder already creates with 0600 on Unix; re-assert
            // it here as a belt-and-braces guarantee against any future
            // upstream behaviour change.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(named.path(), perms)?;
            }

            let path = named.path().to_path_buf();
            if let Some(flag) = caps.tempfile_flag
                && !flag.is_empty()
            {
                cmd.arg(flag);
            }
            cmd.arg(&path);

            Ok(DeliveryHandle {
                mode,
                path: Some(path),
                _tempfile: Some(named),
            })
        }
        DeliveryMode::Stdin => {
            cmd.stdin(Stdio::piped());
            Ok(DeliveryHandle {
                mode,
                path: None,
                _tempfile: None,
            })
        }
    }
}
