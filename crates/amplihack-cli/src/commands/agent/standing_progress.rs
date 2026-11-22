//! Whether one run of a standing session did any work, read from its
//! conversation's transcript after the exit (issue #2912).
//!
//! The evidence is a reply written by the model during the run, and nothing
//! else. The size of the transcript is not evidence, because Claude Code
//! writes to it without doing a turn. Observed with Claude Code 2.1.330 on
//! 2026-11-22, `claude --continue` resuming an existing conversation under
//! the same id (fixtures in `testdata/issue_2912/`, figures in
//! docs/howto/keep-sessions-running.md):
//!
//! - two starts that reached the prompt, did no turn and were then ended
//!   with SIGKILL to the session's process each appended a
//!   `bridge-session` and a `cost-state` line, 839 bytes; a third start
//!   with no turn appended the `bridge-session` line alone, 267 bytes;
//! - a turn whose API request failed (an unknown model) appended the
//!   prompt as a `user` line, an `assistant` line with
//!   `"model":"<synthetic>"` and `"isApiErrorMessage":true`, and request
//!   records, 179,772 bytes in all;
//! - a turn the model answered appended 18,019 bytes, among them one
//!   `assistant` line naming the model;
//! - a `--continue` that reported no conversation to continue exited 1 and
//!   appended nothing;
//! - `--continue --fork-session`, which resumes under a new id, wrote no
//!   file when ended before a turn, and after one turn wrote a new
//!   transcript holding the earlier turns with the timestamps they were
//!   first written with.
//!
//! The first two grow the transcript on every start, so a start-up crash
//! loop measured by size would never be braked. A model reply dated after
//! the launch cannot come from a crash, a failed request or a copy of
//! earlier history.

use serde::Deserialize;
use std::io::BufRead;
use std::time::{SystemTime, UNIX_EPOCH};

/// The model Claude Code names on an assistant line it wrote itself, such as
/// an API error, rather than one the model returned.
const SYNTHETIC: &str = "<synthetic>";

/// Whether `transcript` holds a reply the model wrote at or after `since`.
///
/// A reply is a top-level `"type":"assistant"` line whose `message.model`
/// is a model, not [`SYNTHETIC`], that is not marked `isApiErrorMessage`,
/// and whose `timestamp` parses as RFC 3339. Every other line, and a line
/// that is not JSON, is not a reply. A read error ends the scan with what
/// was found up to it.
///
/// The transcript is written by Claude Code on this host and `since` is
/// read from the same clock just before the launch, so only a clock set
/// back during the run can hide a reply. That errs towards the brake, which
/// is loud and leaves the session to the supervisor, never towards a loop.
pub fn replied_since(mut transcript: impl BufRead, since: SystemTime) -> bool {
    let Some(since) = millis(since) else {
        return false;
    };
    let mut line = Vec::new();
    loop {
        line.clear();
        match transcript.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) if model_reply_at(&line).is_some_and(|at| at >= since) => return true,
            Ok(_) => {}
        }
    }
}

/// One transcript line, only the fields [`model_reply_at`] reads.
#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "isApiErrorMessage")]
    api_error: Option<bool>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
}

/// When the model wrote this line, in Unix milliseconds, if it is a reply.
fn model_reply_at(line: &[u8]) -> Option<i64> {
    // Most lines are not assistant lines and some are large request
    // records, so only a line that names one is parsed.
    if !line.windows(b"assistant".len()).any(|w| w == b"assistant") {
        return None;
    }
    let line: Line = serde_json::from_slice(line).ok()?;
    let model = line.message?.model?;
    let reply = line.kind.as_deref() == Some("assistant")
        && line.api_error != Some(true)
        && !model.is_empty()
        && model != SYNTHETIC;
    if !reply {
        return None;
    }
    let at = chrono::DateTime::parse_from_rfc3339(line.timestamp.as_deref()?).ok()?;
    Some(at.timestamp_millis())
}

/// `t` in Unix milliseconds; `None` before the epoch or past `i64`.
fn millis(t: SystemTime) -> Option<i64> {
    i64::try_from(t.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}
