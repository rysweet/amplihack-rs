//! Stop hook: lock mode, power steering, and reflection.
//!
//! The stop hook decides whether to block session exit. It implements:
//! - Lock mode: if `.lock_active` exists, block with continuation prompt
//! - Safety valve: after N lock iterations, auto-approve
//! - Power steering: check for incomplete work
//! - Reflection: optional SDK bridge for session reflection

pub mod lock;
pub mod power_steering;
pub mod reflection;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;

/// Default continuation prompt when lock mode is active.
const DEFAULT_CONTINUATION_PROMPT: &str =
    "Continue working on the current task. Do not stop until the task is complete.";

pub struct StopHook;

impl Hook for StopHook {
    fn name(&self) -> &'static str {
        "stop"
    }

    fn hook_event_name(&self) -> Option<&'static str> {
        Some("Stop")
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path) = match input {
            HookInput::Stop {
                session_id,
                transcript_path,
                ..
            } => (session_id, transcript_path),
            _ => return Ok(approve()),
        };

        let session_id = session_id.unwrap_or_else(get_session_id);
        let dirs = ProjectDirs::from_cwd();

        // Check lock mode.
        if lock::is_lock_active(&dirs) {
            return lock::handle_lock_mode(&dirs, &session_id);
        }

        // Check power steering (if enabled).
        if power_steering::should_run(&dirs)
            && let Some(block) =
                power_steering::check(&dirs, &session_id, transcript_path.as_deref())?
        {
            return Ok(block);
        }

        // Run reflection (if enabled).
        if reflection::should_run(&dirs)
            && let Some(block) =
                reflection::run_reflection(&dirs, &session_id, transcript_path.as_deref())?
        {
            return Ok(block);
        }

        // NOTE: the Signal channel is intentionally NOT torn down here. This
        // hook fires at the end of *every* assistant turn (Claude Code `Stop`,
        // Copilot `agentStop`), not at session end, so tearing the channel down
        // here would kill the per-session Signal group after the first turn.
        // Teardown lives in `SessionStopHook` (the genuine session-end event).
        //
        // Full-conversation mirroring (assistant side): relay this turn's final
        // assistant message to the session's Signal group. No-op unless the
        // channel is configured.
        #[cfg(feature = "signal")]
        if let Some(assistant_text) = transcript_path
            .as_deref()
            .and_then(last_assistant_message_from_transcript)
        {
            crate::signal_integration::relay_outbound(Some(&session_id), &assistant_text);
        }

        Ok(approve())
    }
}

/// Cap on how many trailing bytes we read from a transcript when locating the
/// last assistant message for outbound mirroring. Bounds memory for
/// pathologically large (or unbounded) files.
#[cfg(feature = "signal")]
const OUTBOUND_TRANSCRIPT_READ_CAP: u64 = 8 * 1024 * 1024;

/// Best-effort extraction of the **last** assistant message text from a
/// transcript JSONL file, for outbound mirroring. Tolerant of the several
/// transcript shapes across hosts (Copilot `assistant.message`, Claude
/// `role: assistant`, nested `message.role`). Returns `None` on any failure so
/// mirroring never blocks or fails session exit.
#[cfg(feature = "signal")]
fn last_assistant_message_from_transcript(transcript_path: &std::path::Path) -> Option<String> {
    let contents = read_transcript_tail_bounded(transcript_path)?;
    contents.lines().rev().find_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let entry = serde_json::from_str::<Value>(line).ok()?;
        extract_assistant_text_from_entry(&entry)
    })
}

/// Bounded, non-blocking read of a transcript for outbound mirroring.
///
/// Only **regular files** are read. A non-regular path — a FIFO, socket,
/// character/block device, or a `/proc/self/fd/*` handle onto this process's
/// own pipe (a symlink/`fd` transcript path is enough) — would otherwise make a
/// plain `read_to_string` block indefinitely and wedge session exit, or stream
/// unbounded data. Reading is capped to the trailing
/// [`OUTBOUND_TRANSCRIPT_READ_CAP`] bytes so the last assistant message is still
/// found without unbounded allocation. Returns `None` on any error so mirroring
/// never blocks or fails session exit.
///
/// The file is opened first and then classified via `fstat` on the **opened
/// descriptor** (not a separate path-based `metadata()` call), which closes the
/// TOCTOU window where a regular file could be swapped for a FIFO between the
/// check and the open. On Unix the open uses `O_NONBLOCK`, so opening a
/// FIFO/socket/device returns immediately instead of blocking; `O_NONBLOCK` is
/// ignored for reads on regular files, so real transcripts read normally.
#[cfg(feature = "signal")]
fn read_transcript_tail_bounded(transcript_path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = open_transcript_nonblocking(transcript_path)?;
    // fstat on the opened fd — never blocks, and reflects the actual object we
    // hold open (no path re-resolution, so no TOCTOU with the open above).
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }

    let len = metadata.len();
    if len > OUTBOUND_TRANSCRIPT_READ_CAP {
        file.seek(SeekFrom::Start(len - OUTBOUND_TRANSCRIPT_READ_CAP))
            .ok()?;
    }
    let mut buf = Vec::new();
    file.take(OUTBOUND_TRANSCRIPT_READ_CAP)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Open a transcript for reading without ever blocking on a non-regular path.
///
/// On Unix, `O_NONBLOCK` makes opening a FIFO with no writer (or a slow device)
/// return immediately rather than hang; the caller then rejects any non-regular
/// descriptor via `fstat`. Regular files ignore `O_NONBLOCK` for reads.
#[cfg(all(feature = "signal", unix))]
fn open_transcript_nonblocking(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()
}

/// Non-Unix fallback: the FIFO/proc-fd hang classes do not apply the same way,
/// and the caller still rejects non-regular descriptors via `fstat`.
#[cfg(all(feature = "signal", not(unix)))]
fn open_transcript_nonblocking(path: &std::path::Path) -> Option<std::fs::File> {
    std::fs::File::open(path).ok()
}

/// Pull assistant text out of a single transcript entry across host shapes.
#[cfg(feature = "signal")]
fn extract_assistant_text_from_entry(entry: &Value) -> Option<String> {
    let object = entry.as_object()?;

    if let Some(message) = object.get("message").and_then(Value::as_object)
        && message.get("role").and_then(Value::as_str) == Some("assistant")
    {
        return extract_entry_text(message.get("content").unwrap_or(&Value::Null));
    }

    if object.get("role").and_then(Value::as_str) == Some("assistant") {
        return extract_entry_text(
            object
                .get("content")
                .or_else(|| object.get("text"))
                .unwrap_or(&Value::Null),
        );
    }

    if object.get("type").and_then(Value::as_str) == Some("assistant.message") {
        return extract_entry_text(
            object
                .get("data")
                .and_then(|value| value.get("content"))
                .unwrap_or(&Value::Null),
        );
    }

    if object.get("type").and_then(Value::as_str) == Some("assistant") {
        return extract_entry_text(
            object
                .get("content")
                .or_else(|| object.get("message").and_then(|value| value.get("content")))
                .unwrap_or(&Value::Null),
        );
    }

    None
}

/// Flatten a transcript `content` value (string | array of parts | object)
/// into a single text string.
#[cfg(feature = "signal")]
fn extract_entry_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.to_string()),
        Value::String(_) => None,
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(extract_entry_text)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("value"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| map.get("content").and_then(extract_entry_text)),
        _ => None,
    }
}

/// Approve (allow session to exit).
fn approve() -> Value {
    serde_json::json!({"decision": "approve"})
}

/// Get the current session ID from env or generate one.
fn get_session_id() -> String {
    if let Ok(id) = std::env::var("CLAUDE_SESSION_ID") {
        return id;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("session-{}", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_has_correct_format() {
        let result = approve();
        assert_eq!(result["decision"], "approve");
    }

    #[test]
    fn handles_unknown_events() {
        let hook = StopHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert_eq!(result["decision"], "approve");
    }

    // Regression tests for the outbound-mirroring transcript reader. A
    // non-regular transcript path (FIFO/socket/`/proc/self/fd/*` pipe) must
    // never be read, otherwise mirroring blocks session exit indefinitely.
    #[cfg(feature = "signal")]
    mod transcript_read {
        use super::super::{OUTBOUND_TRANSCRIPT_READ_CAP, read_transcript_tail_bounded};
        use std::fs;
        use std::io::Write;
        use std::path::PathBuf;

        fn unique_dir(tag: &str) -> PathBuf {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "amplihack-stop-transcript-{tag}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        #[test]
        fn reads_regular_file() {
            let dir = unique_dir("regular");
            let path = dir.join("t.jsonl");
            fs::write(&path, "{\"role\":\"assistant\",\"content\":\"hi\"}\n").unwrap();
            let contents = read_transcript_tail_bounded(&path).expect("regular file read");
            assert!(contents.contains("hi"));
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn non_regular_path_returns_none_without_blocking() {
            // A directory is a simple, portable non-regular file. The guard
            // that rejects it is the same guard that rejects a FIFO/pipe
            // (e.g. `/proc/self/fd/1`) whose read would block forever.
            let dir = unique_dir("dir");
            assert!(read_transcript_tail_bounded(&dir).is_none());
            let _ = fs::remove_dir_all(&dir);
        }

        // A real FIFO with no writer: a plain blocking open (or read) would hang
        // session exit forever. The `O_NONBLOCK`-open + `fstat`-classify path
        // must return `None` promptly. We run it on a worker thread and require
        // it to finish well within a generous bound to prove it never blocks.
        #[cfg(unix)]
        #[test]
        fn real_fifo_with_no_writer_returns_none_without_blocking() {
            use std::ffi::CString;
            use std::sync::mpsc;
            use std::time::Duration;

            let dir = unique_dir("fifo");
            let path = dir.join("t.jsonl");
            let c_path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
            // 0o600 FIFO; if mkfifo is unsupported the assert below is skipped.
            let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
            assert_eq!(rc, 0, "mkfifo failed (errno {})", unsafe {
                *libc::__errno_location()
            });

            let (tx, rx) = mpsc::channel();
            let probe_path = path.clone();
            let handle = std::thread::spawn(move || {
                let r = read_transcript_tail_bounded(&probe_path);
                let _ = tx.send(r.is_none());
            });
            let got_none = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("reading a FIFO transcript must not block session exit");
            assert!(got_none, "a FIFO transcript must be rejected (None)");
            handle.join().unwrap();
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn missing_path_returns_none() {
            let dir = unique_dir("missing");
            let path = dir.join("does-not-exist.jsonl");
            assert!(read_transcript_tail_bounded(&path).is_none());
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn oversized_file_reads_bounded_tail_and_finds_last_message() {
            let dir = unique_dir("oversized");
            let path = dir.join("big.jsonl");
            let mut file = fs::File::create(&path).unwrap();
            // Write filler beyond the cap, then the real last message at the end.
            let filler = "x".repeat(1024 * 1024);
            let mut written: u64 = 0;
            while written <= OUTBOUND_TRANSCRIPT_READ_CAP {
                writeln!(file, "{filler}").unwrap();
                written += filler.len() as u64 + 1;
            }
            writeln!(file, "{{\"role\":\"assistant\",\"content\":\"tail-msg\"}}").unwrap();
            file.flush().unwrap();

            let contents = read_transcript_tail_bounded(&path).expect("bounded tail read");
            assert!(
                (contents.len() as u64) <= OUTBOUND_TRANSCRIPT_READ_CAP,
                "read must be bounded by the cap"
            );
            assert!(
                contents.contains("tail-msg"),
                "tail read must still contain the last assistant message"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
