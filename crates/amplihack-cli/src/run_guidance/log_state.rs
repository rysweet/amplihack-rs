//! What a guidance folder's `guidance.log` says about its entries (issues
//! #3162 and #3161): which entries a step has read, and which launches since
//! the last read could not take the guidance.
//!
//! Two records matter here, both written by the launcher:
//!
//! - `guidance_injected`, whose `entry_names` lists the entries of this
//!   folder that went into an agent step's prompt. A record without the
//!   field (written by every launcher up to 1.2.219) proves nothing about
//!   any particular entry, so it marks none as read;
//! - `guidance_dropped`, written when a launch had guidance but no prompt it
//!   could follow. Its fields come from closed tables ([`DropReason`] and the
//!   launcher's launch-form labels), plus the run id and a sanitised agent
//!   name.
//!
//! The reader never fails: a missing log is an empty state, a line that does
//! not parse or names an event it does not know is skipped, and a log that
//! cannot be opened safely is one warning line plus [`LogState::log_error`],
//! so that a reader reports the state as unknown rather than empty. Every value read back goes
//! through [`sanitize_line`] before it is printed.

use super::sanitize::sanitize_line;
use super::{GUIDANCE_LOG, warn};
use std::collections::HashSet;
use std::path::Path;

/// The most bytes of `guidance.log` read. A log past it is read up to it,
/// which keeps the oldest records; the reader says so once.
pub(crate) const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// Why a launch that had guidance did not deliver it. Each has a stable code
/// that readers match on, and a fixed sentence for people.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropReason {
    /// The launch reads its prompt from stdin and stdin gave no text: a
    /// terminal, `/dev/null` or an empty file (known at launch), or a pipe
    /// or file that gave nothing but white space before the step exited
    /// (known by reading it).
    NoPromptOnStdin,
    /// The launch form has no prompt that guidance can follow.
    LaunchFormUnsupported,
    /// The block was due after the caller's stdin, and was not written.
    TrailerNotWritten,
    /// The step's prompt is in a file its `-p` pointer names (the recipe
    /// runner's Copilot form), and that file could not be read or its
    /// private copy written.
    PromptFileNotCopied,
}

impl DropReason {
    /// The stable identifier written to `guidance.log` and printed.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NoPromptOnStdin => "no_prompt_on_stdin",
            Self::LaunchFormUnsupported => "launch_form_unsupported",
            Self::TrailerNotWritten => "trailer_not_written",
            Self::PromptFileNotCopied => "prompt_file_not_copied",
        }
    }

    /// One sentence, with no value in it.
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::NoPromptOnStdin => {
                "the step reads its prompt from standard input, and standard input gave no text"
            }
            Self::LaunchFormUnsupported => {
                "this launch form has no prompt that operator guidance can follow"
            }
            Self::TrailerNotWritten => {
                "the step ended before the guidance after its prompt was written"
            }
            Self::PromptFileNotCopied => {
                "the step's prompt file could not be read or copied, so the guidance could not follow its prompt"
            }
        }
    }
}

/// What the launcher can say about the step that dropped guidance: the
/// recipe run id it was started under, the tool, a fixed launch-form label
/// and the agent name the arguments give, if any. Never argv text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StepLabel {
    pub step_run_id: String,
    pub tool: String,
    pub launch_form: String,
    pub agent: Option<String>,
}

/// One `guidance_dropped` record, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drop {
    pub ts: String,
    pub step_run_id: String,
    pub tool: String,
    pub launch_form: String,
    pub agent: Option<String>,
    pub reason: String,
    pub detail: String,
}

/// What one folder's log says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LogState {
    /// Names of entries a `guidance_injected` record lists.
    pub read: HashSet<String>,
    /// `guidance_dropped` records written after the last
    /// `guidance_injected` record, oldest first.
    pub drops_since_injection: Vec<Drop>,
    /// Why the log could not be read, when it exists but was refused or
    /// failed. `read` and `drops_since_injection` are then empty because
    /// nothing is known, not because nothing happened, and readers must say
    /// so.
    pub log_error: Option<String>,
}

fn text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(sanitize_line)
        .unwrap_or_default()
}

/// The last [`MAX_LOG_BYTES`] of `log`, starting at a whole line, through one
/// handle opened without following a link and checked by its own metadata.
/// `Ok(None)` when there is no log. The newest records are the ones that say
/// whether an entry has been read since, so a long log is read from its end.
fn read_tail(log: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        // O_NONBLOCK: a FIFO planted here must not hang the reader; the
        // regular-file check below then refuses it.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(log)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err("it is a symbolic link".to_string());
        }
        Err(e) => return Err(format!("cannot open it: {e}")),
    };
    let meta = file
        .metadata()
        .map_err(|e| format!("cannot check it: {e}"))?;
    if let Some(reason) = super::fs_safety::untrusted_reason(&meta, false) {
        return Err(reason);
    }
    let skip = meta.len().saturating_sub(MAX_LOG_BYTES);
    if skip > 0 {
        file.seek(SeekFrom::Start(skip))
            .map_err(|e| format!("cannot read it: {e}"))?;
    }
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read it: {e}"))?;
    if skip > 0 {
        // The first line may be cut; start after it.
        let start = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| i + 1);
        bytes.drain(..start);
    }
    Ok(Some(bytes))
}

/// Reads `guidance.log` beside `guidance_dir`. Never fails; see the module
/// documentation. A log over [`MAX_LOG_BYTES`] is read from its last
/// [`MAX_LOG_BYTES`], so an entry whose only `guidance_injected` record is
/// older than that reads as not yet read.
pub(crate) fn read_log_state(guidance_dir: &Path) -> LogState {
    let mut state = LogState::default();
    let Some(parent) = guidance_dir.parent() else {
        return state;
    };
    let log = parent.join(GUIDANCE_LOG);
    let bytes = match read_tail(&log) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return state,
        Err(reason) => {
            warn(&format!(
                "amplihack: ignoring operator guidance log {}: {reason}",
                log.display()
            ));
            state.log_error = Some(sanitize_line(&reason));
            return state;
        }
    };
    let body = String::from_utf8_lossy(&bytes);
    for line in body.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("event").and_then(serde_json::Value::as_str) {
            Some("guidance_injected") => {
                state.drops_since_injection.clear();
                if let Some(names) = value
                    .get("entry_names")
                    .and_then(serde_json::Value::as_array)
                {
                    state.read.extend(
                        names
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string),
                    );
                }
            }
            Some("guidance_dropped") => state.drops_since_injection.push(Drop {
                ts: text(&value, "ts"),
                step_run_id: text(&value, "step_run_id"),
                tool: text(&value, "tool"),
                launch_form: text(&value, "launch_form"),
                agent: value
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .filter(|a| !a.is_empty())
                    .map(sanitize_line),
                reason: text(&value, "reason"),
                detail: text(&value, "detail"),
            }),
            _ => {}
        }
    }
    state
}

/// The one line that reports a drop to a sender:
/// `dropped by TOOL (LAUNCH_FORM)[ agent A] in run ID: CODE: DETAIL`.
pub(crate) fn render_drop_line(drop: &Drop) -> String {
    let agent = drop
        .agent
        .as_deref()
        .map(|a| format!(" agent {a}"))
        .unwrap_or_default();
    sanitize_line(&format!(
        "dropped by {} ({}){agent} in run {}: {}: {}",
        drop.tool, drop.launch_form, drop.step_run_id, drop.reason, drop.detail
    ))
}
