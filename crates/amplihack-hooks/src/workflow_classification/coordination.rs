//! The `[coord]` marker for coordination traffic between sessions (issue #1742).
//!
//! A prompt that starts with [`COORDINATION_PREFIX`] gets [`COORDINATION_NOTE`]
//! instead of the routing prompt, unless its body starts with a change verb
//! (see [`is_explicit_work_request`]). See
//! `docs/reference/coordination-messages.md`.

use regex::Regex;
use std::sync::OnceLock;

/// Marker that a prompt is coordination traffic between sessions (issue #1742).
///
/// Read only by [`strip_coordination_prefix`]: it must be the first non-blank
/// text of the prompt, compared ASCII case-insensitively.
pub const COORDINATION_PREFIX: &str = "[coord]";

/// Context injected for a marked prompt in place of the routing prompt.
///
/// A constant, so no part of the message body is ever reflected into the
/// hook output.
///
/// The note keeps recipes away from replies, reports and single commands, but
/// not from code changes. A marked request for a code change that
/// [`is_explicit_work_request`] does not recognise, such as
/// `please fix the parser`, must still reach `default-workflow` through
/// dev-orchestrator: that is the downstream invariant in `routing_prompt.txt`.
pub const COORDINATION_NOTE: &str = "[coordination] This message starts with the [coord] marker \
     and is coordination traffic between sessions. Answer it or act on it directly, \
     without dev-orchestrator, smart-orchestrator or a recipe. \
     If it asks for a code change, use dev-orchestrator as usual.";

/// A marked body that starts with a change verb, then whitespace, then text.
///
/// The verbs are the routing prompt's IMPLEMENT signal (build, fix, add,
/// create, refactor, update, write) plus implement, develop, work on and
/// start. A test keeps the list in step with `routing_prompt.txt`.
///
/// The whitespace after the verb keeps `update: CI passed` a note. A report
/// that happens to start with a verb, such as `fix landed on PR 12`, is
/// routed. That costs one classification by the routing prompt, whose rule 8
/// then treats a result report as Q&A; the opposite mistake would let a code
/// change bypass the workflow.
pub(super) const EXPLICIT_WORK_REQUEST_PATTERN: &str =
    r"(?i)^(build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s+\S";

/// Return the body of a prompt that starts with [`COORDINATION_PREFIX`].
///
/// Leading whitespace is ignored and the marker is compared ASCII
/// case-insensitively. The body is the rest of the prompt with leading
/// whitespace removed, and may be empty. A marker joined to the next word
/// (`[coord]status`) still matches; one later in the prompt never does.
pub(super) fn strip_coordination_prefix(prompt: &str) -> Option<&str> {
    let trimmed = prompt.trim_start();
    let len = COORDINATION_PREFIX.len();
    // `get` returns None when `len` is not a char boundary, so a multibyte
    // character at the marker position cannot panic.
    let head = trimmed.get(..len)?;
    if !head.eq_ignore_ascii_case(COORDINATION_PREFIX) {
        return None;
    }
    Some(trimmed[len..].trim_start())
}

/// Does a marked body ask for work? True when it starts with a change verb.
///
/// See [`EXPLICIT_WORK_REQUEST_PATTERN`] for the verbs and the trade-off.
pub(super) fn is_explicit_work_request(body: &str) -> bool {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| {
            Regex::new(EXPLICIT_WORK_REQUEST_PATTERN).expect("valid explicit work request regex")
        })
        .is_match(body)
}
