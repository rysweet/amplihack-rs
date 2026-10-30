//! The `[coord]` marker for coordination traffic between sessions (issue #1742).
//!
//! A prompt that starts with [`COORDINATION_PREFIX`] gets [`COORDINATION_NOTE`]
//! instead of the routing prompt, unless its body is a work request: it starts
//! with a change verb and is not a report (see [`is_explicit_work_request`]).
//! See `docs/reference/coordination-messages.md`.

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
/// The whitespace after the verb keeps `update: CI passed` a note. A body that
/// matches is a work request only if [`VERB_LED_REPORT_PATTERN`] does not also
/// match it.
pub(super) const EXPLICIT_WORK_REQUEST_PATTERN: &str =
    r"(?i)^(build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s+\S";

/// A body that starts with a change verb used as a noun, in a report.
///
/// Applied only to a body that matches [`EXPLICIT_WORK_REQUEST_PATTERN`], so
/// its first word is a change verb. The body is a report, and gets the note,
/// when one of these holds:
///
/// - it opens with `update on` (`update on PR 57: merged`);
/// - the second word is `is`, `was`, `are`, `were`, `has` or `had`, followed
///   by whitespace or the end (`build is green on #1740`, `fix has landed`);
/// - the second word is an outcome (`passed`, `failed`, `landed`, `merged`,
///   `succeeded`, `shipped`, `finished`, `completed`, `started`, `worked`,
///   `done`) that ends the body or is followed by punctuation, a preposition
///   or a conjunction (`build failed on PR 12`, `fix landed.`).
///
/// An outcome word followed by a noun is an adjective in a request, so
/// `fix failed tests` and `update merged branches` stay work requests.
///
/// Errors fall towards routing. A report this pattern misses, such as
/// `fix for #12 landed`, gets the routing prompt. That prompt applies the
/// first rule that matches, and rule 4 (IMPLEMENT -> DEV) comes before the
/// rule-8 exception for result reports, so the agent will usually start
/// dev-orchestrator for it. The opposite mistake would give a code change the
/// note, which leaves the routing to the agent's reading of the note's last
/// sentence.
pub(super) const VERB_LED_REPORT_PATTERN: &str = r"(?i)^(?:update\s+on\b|\S+\s+(?:(?:is|was|are|were|has|had)(?:\s|$)|(?:passed|failed|landed|merged|succeeded|shipped|finished|completed|started|worked|done)(?:\s*$|\s*[.,;:!?]|\s+(?:on|in|for|at|to|into|with|after|again|and|but)\b)))";

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

/// Does a marked body ask for work? True when it starts with a change verb
/// and is not a report that starts with one.
///
/// See [`EXPLICIT_WORK_REQUEST_PATTERN`] for the verbs and
/// [`VERB_LED_REPORT_PATTERN`] for the reports and the trade-off.
pub(super) fn is_explicit_work_request(body: &str) -> bool {
    static WORK: OnceLock<Regex> = OnceLock::new();
    static REPORT: OnceLock<Regex> = OnceLock::new();
    let work = WORK.get_or_init(|| {
        Regex::new(EXPLICIT_WORK_REQUEST_PATTERN).expect("valid explicit work request regex")
    });
    let report = REPORT
        .get_or_init(|| Regex::new(VERB_LED_REPORT_PATTERN).expect("valid verb-led report regex"));
    work.is_match(body) && !report.is_match(body)
}
