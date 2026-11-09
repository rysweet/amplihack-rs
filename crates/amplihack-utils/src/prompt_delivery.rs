//! Subprocess prompt-delivery helper for amplihack-rs.
//!
//! Implements the delivery modes (`argv` / `tempfile` / `stdin` /
//! `file-reference`) plus an `auto` selector and an `AMPLIHACK_PROMPT_DELIVERY` env-var override.
//!
//! Tracking issues: Simard #1897 (the helper), amplihack-rs #2030 (prompt
//! text never in argv). Working reference (shell-mktemp pattern): Simard
//! #1878. Originating apostrophe-truncation bug: Simard #1871.
//!
//! ## Contract summary
//!
//! - **Never argv (#2030):** a prompt in argv is visible in
//!   `/proc/<pid>/cmdline`, so any `pkill -f` or `pgrep -f` that matches a
//!   phrase in it hits the agent. [`try_select_mode`] and [`deliver`] never
//!   return or build [`DeliveryMode::Argv`], whatever was requested.
//! - **Env var:** `AMPLIHACK_PROMPT_DELIVERY` ∈ `{auto, tempfile, stdin}`,
//!   case-insensitive. Unset / empty / unrecognised values all resolve to
//!   `auto` (unrecognised emits a `tracing::warn!`). `argv` was removed by
//!   #2030 (R1.2): the variable is inherited by every nested step, so it is
//!   now an unrecognised value. `file-reference` is not a request value; it
//!   is only ever selected by `auto` or by degrading.
//! - **Auto rule:** `stdin`, then `file-reference`, then `tempfile`, then a
//!   [`NoSafeChannel`] error. Prompt size does not affect the choice.
//! - **Explicit override:** wins over `auto`, and degrades only through
//!   channels that keep the prompt out of argv:
//!   `tempfile → stdin → file-reference`, `stdin → file-reference`.
//!   A [`PromptDelivery::Argv`] request (only reachable from code, never
//!   from the env var) is treated exactly as `auto`.
//! - **Lifecycle:** [`deliver`] returns a [`DeliveryHandle`] that owns the
//!   underlying [`tempfile::NamedTempFile`] (for `tempfile` and
//!   `file-reference` modes) or marks
//!   the child stdin as piped (for `stdin` mode). The handle MUST be kept
//!   alive until the child has been waited on; dropping it unlinks the
//!   temp file.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::{NamedTempFile, TempDir};

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

/// Caller-requested delivery mode. `Auto` defers to [`try_select_mode`].
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
///
/// `DeliveryCaps::default()` has no channels at all: every field is `false`
/// or `None`. A launcher that is not recognised gets the default, so it
/// receives [`NoSafeChannel`] instead of argv.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeliveryCaps {
    pub supports_argv: bool,
    pub supports_tempfile: bool,
    pub supports_stdin: bool,
    /// The binary can be told, in one short argument, to read its prompt from
    /// a file (see [`FILE_REFERENCE_TEMPLATE`]). Used for agent CLIs that have
    /// neither a prompt-file flag nor a stdin contract.
    pub supports_file_reference: bool,
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
            supports_file_reference: false,
            tempfile_flag: None,
        }
    }

    pub fn argv_and_tempfile(flag: &'static str) -> Self {
        Self {
            supports_argv: true,
            supports_tempfile: true,
            supports_stdin: false,
            supports_file_reference: false,
            tempfile_flag: Some(flag),
        }
    }

    pub fn all_modes(tempfile_flag: &'static str) -> Self {
        Self {
            supports_argv: true,
            supports_tempfile: true,
            supports_stdin: true,
            supports_file_reference: false,
            tempfile_flag: Some(tempfile_flag),
        }
    }

    /// Returns a copy with `supports_stdin` set.
    pub const fn with_stdin(self) -> Self {
        Self {
            supports_stdin: true,
            ..self
        }
    }

    /// Returns a copy with `supports_file_reference` set.
    pub const fn with_file_reference(self) -> Self {
        Self {
            supports_file_reference: true,
            ..self
        }
    }
}

/// Mode actually selected after considering caps + env.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMode {
    Argv,
    Tempfile,
    Stdin,
    /// The prompt is written to a 0600 temp file and the child receives one
    /// short argument, built from [`FILE_REFERENCE_TEMPLATE`], naming it.
    FileReference,
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
    // Order matters for drop semantics: fields drop in declaration order, so
    // `_tempfile` is unlinked before `_tempdir`, its private parent in
    // `FileReference` mode, is removed.
    _tempfile: Option<NamedTempFile>,
    _tempdir: Option<TempDir>,
}

impl DeliveryHandle {
    pub fn mode(&self) -> DeliveryMode {
        self.mode
    }

    /// Path to the temp file in `Tempfile` and `FileReference` modes. `None`
    /// otherwise.
    pub fn tempfile_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Env-var name that selects the delivery mode at runtime.
pub const ENV_VAR_NAME: &str = "AMPLIHACK_PROMPT_DELIVERY";

/// Fixed sentence used for file-reference delivery. `{path}` is the only
/// substitution, and the path of a `NamedTempFile` contains no newline, so
/// the argument stays well under 512 bytes.
pub const FILE_REFERENCE_TEMPLATE: &str = "Read and follow the complete instructions in the file {path}. Its contents are the task prompt.";

/// The text before `{path}` in [`FILE_REFERENCE_TEMPLATE`].
pub const FILE_REFERENCE_PREFIX: &str = "Read and follow the complete instructions in the file ";

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
        "tempfile" => PromptDelivery::Tempfile,
        "stdin" => PromptDelivery::Stdin,
        other => {
            tracing::warn!(
                env = ENV_VAR_NAME,
                value = other,
                "unrecognised prompt-delivery mode; falling back to auto. \
                 Valid values: auto, tempfile, stdin (case-insensitive)."
            );
            PromptDelivery::Auto
        }
    }
}

// ---------------------------------------------------------------------------
// NoSafeChannel
// ---------------------------------------------------------------------------

/// Fixed text of [`NoSafeChannel`]. It never contains prompt text.
const NO_SAFE_CHANNEL_MESSAGE: &str = "no channel keeps this prompt out of the command line: \
the agent CLI accepts none of stdin, a prompt file or a file reference";

/// Returned when no delivery channel keeps the prompt out of argv (#2030).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoSafeChannel {
    /// What the caller asked for.
    pub requested: PromptDelivery,
    /// What the binary offers.
    pub caps: DeliveryCaps,
}

impl std::fmt::Display for NoSafeChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(NO_SAFE_CHANNEL_MESSAGE)
    }
}

impl std::error::Error for NoSafeChannel {}

impl From<NoSafeChannel> for std::io::Error {
    fn from(err: NoSafeChannel) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, err)
    }
}

// ---------------------------------------------------------------------------
// try_select_mode
// ---------------------------------------------------------------------------

/// Pick the delivery mode for a spawn, or for a doctor report.
///
/// Never returns [`DeliveryMode::Argv`]. `Auto` tries `stdin`, then
/// `file-reference`, then `tempfile`. An explicit request degrades only
/// through channels that keep the prompt out of argv. A
/// [`PromptDelivery::Argv`] request is treated as `Auto` (#2030, R1.2).
/// When no channel fits, the answer is [`NoSafeChannel`]: reports show it as
/// "unavailable" and spawns fail. The size of a prompt never matters.
pub fn try_select_mode(
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> Result<DeliveryMode, NoSafeChannel> {
    let order: &[DeliveryMode] = match requested {
        // R1.2: an `Argv` request is not a delivery channel; it picks exactly
        // what `Auto` picks. `DeliveryMode::Argv` is never in any order list.
        PromptDelivery::Auto | PromptDelivery::Argv => &[
            DeliveryMode::Stdin,
            DeliveryMode::FileReference,
            DeliveryMode::Tempfile,
        ],
        PromptDelivery::Tempfile => &[
            DeliveryMode::Tempfile,
            DeliveryMode::Stdin,
            DeliveryMode::FileReference,
        ],
        // Stdin never degrades to tempfile: the two have different process
        // contracts (a bare flag reading stdin versus a path argument).
        PromptDelivery::Stdin => &[DeliveryMode::Stdin, DeliveryMode::FileReference],
    };
    let chosen = order.iter().copied().find(|m| supports(caps, *m));
    match chosen {
        Some(mode) => {
            if order[0] != mode && requested != PromptDelivery::Argv {
                tracing::warn!(
                    requested = ?requested,
                    selected = ?mode,
                    "requested prompt-delivery mode unsupported by binary; degrading"
                );
            }
            Ok(mode)
        }
        None => Err(NoSafeChannel {
            requested,
            caps: *caps,
        }),
    }
}

fn supports(caps: &DeliveryCaps, mode: DeliveryMode) -> bool {
    match mode {
        DeliveryMode::Argv => caps.supports_argv,
        DeliveryMode::Tempfile => caps.supports_tempfile,
        DeliveryMode::Stdin => caps.supports_stdin,
        DeliveryMode::FileReference => caps.supports_file_reference,
    }
}

// ---------------------------------------------------------------------------
// deliver
// ---------------------------------------------------------------------------

/// Apply prompt delivery to a [`Command`].
///
/// Mutates `cmd` as required by the selected mode. The prompt is never
/// appended to argv: [`try_select_mode`] never selects
/// [`Argv`](DeliveryMode::Argv), and a request for it is treated as `Auto`.
/// - [`Tempfile`](DeliveryMode::Tempfile): writes the prompt to a fresh
///   `tempfile::NamedTempFile` with mode `0600` (Unix), then appends
///   `caps.tempfile_flag` (when non-empty) followed by the path.
/// - [`Stdin`](DeliveryMode::Stdin): configures the child's stdin as piped.
///   The caller is responsible for writing the prompt bytes to
///   [`std::process::Child::stdin`] and then closing it before waiting
///   (see [`crate::payload_delivery::spawn_stdin_writer`]).
/// - [`FileReference`](DeliveryMode::FileReference): writes the prompt to a
///   0600 temp file and appends one argument built from
///   [`FILE_REFERENCE_TEMPLATE`].
///
/// Returns a [`DeliveryHandle`] whose lifetime guards the temp resources.
///
/// # Errors
///
/// Returns [`std::io::ErrorKind::InvalidInput`] (from [`NoSafeChannel`]) when
/// no channel keeps the prompt out of argv; `cmd` is left unchanged.
pub fn deliver(
    cmd: &mut Command,
    prompt: &str,
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> std::io::Result<DeliveryHandle> {
    // Issue #898: strip NUL bytes so a single stray NUL in agent/bash step
    // output cannot abort the whole workflow at child spawn. Sanitization
    // affects only the delivered bytes.
    let mode = try_select_mode(requested, caps)?;
    let prompt = sanitize_prompt_nul(prompt);
    let prompt = prompt.as_ref();
    match mode {
        // Unreachable through `try_select_mode`; fail closed rather than
        // ever building an argv delivery (R5).
        DeliveryMode::Argv => Err(NoSafeChannel {
            requested,
            caps: *caps,
        }
        .into()),
        DeliveryMode::Tempfile => {
            let named = write_private_tempfile(prompt)?;
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
                _tempdir: None,
            })
        }
        DeliveryMode::Stdin => {
            cmd.stdin(Stdio::piped());
            Ok(DeliveryHandle {
                mode,
                path: None,
                _tempfile: None,
                _tempdir: None,
            })
        }
        DeliveryMode::FileReference => {
            // The file sits alone in a private 0700 directory, so a caller
            // that grants the agent read access to the file's directory
            // (Copilot's `--add-dir`) exposes only this prompt, never the
            // shared temp root (issue #1570, security review).
            let dir = tempfile::Builder::new()
                .prefix("amplihack-prompt-")
                .tempdir()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
            }
            let named = write_private_tempfile_in(prompt, Some(dir.path()))?;
            let path = named.path().to_path_buf();
            let path_text = path.to_string_lossy();
            if path_text.contains('\n') {
                return Err(std::io::Error::other(
                    "prompt temp file path contains a newline",
                ));
            }
            cmd.arg(FILE_REFERENCE_TEMPLATE.replace("{path}", &path_text));
            Ok(DeliveryHandle {
                mode,
                path: Some(path),
                _tempfile: Some(named),
                _tempdir: Some(dir),
            })
        }
    }
}

/// Write `prompt` to a fresh `NamedTempFile` that only the owner can read.
fn write_private_tempfile(prompt: &str) -> std::io::Result<NamedTempFile> {
    write_private_tempfile_in(prompt, None)
}

/// [`write_private_tempfile`] in `dir`, or in the system temp root for `None`.
fn write_private_tempfile_in(prompt: &str, dir: Option<&Path>) -> std::io::Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("simard-prompt-");
    let mut named = match dir {
        Some(dir) => builder.tempfile_in(dir)?,
        None => builder.tempfile()?,
    };
    named.write_all(prompt.as_bytes())?;
    named.flush()?;

    // tempfile::Builder already creates with 0600 on Unix; re-assert it here
    // as a belt-and-braces guarantee against any future upstream behaviour
    // change.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(named.path(), perms)?;
    }
    Ok(named)
}
