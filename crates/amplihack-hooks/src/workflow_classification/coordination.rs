//! The `[coord]` marker for coordination traffic between sessions (issue #1742).
//!
//! A prompt that starts with [`COORDINATION_PREFIX`] gets [`COORDINATION_NOTE`]
//! instead of the routing prompt, unless its body is a work request: it starts
//! with a change verb and is not a report (see [`is_explicit_work_request`]).
//! A work request is then decided as the same body without the marker would be,
//! throttle included; that part is in `process`. See
//! `docs/reference/coordination-messages.md`.

use regex::{Regex, RegexSet};
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
/// `please fix the parser` or `fix the tests that failed`, or one that the
/// throttle holds back, must still reach `default-workflow` through
/// dev-orchestrator: that is the downstream invariant in `routing_prompt.txt`.
pub const COORDINATION_NOTE: &str = "[coordination] This message starts with the [coord] marker \
     and is coordination traffic between sessions. Answer it or act on it directly, \
     without dev-orchestrator, smart-orchestrator or a recipe. \
     If it asks for a code change, use dev-orchestrator as usual.";

/// The change verbs: the routing prompt's IMPLEMENT signal (build, fix, add,
/// create, refactor, update, write) plus implement, develop, work on and
/// start. A test keeps the list in step with `routing_prompt.txt`.
///
/// Shared by [`EXPLICIT_WORK_REQUEST_PATTERN`] and
/// [`VERB_LED_REPORT_PATTERNS`], so the report patterns treat `work on` as one
/// verb and the two lists cannot drift apart.
macro_rules! change_verbs {
    () => {
        r"build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start"
    };
}

/// Outcome words: what a report says happened to the thing it names.
///
/// The list is closed. A report whose only statement of outcome is a word not
/// listed here, such as `fix #12 rolled back`, is still a work request; the
/// docs tell senders to start a report with its subject instead.
macro_rules! outcomes {
    () => {
        "passed|failed|landed|merged|succeeded|shipped|finished|completed|started|worked|done|\
         reverted|deployed|released|pushed|broke|broken|green|red"
    };
}

/// A marked body that starts with a change verb, then whitespace, then text.
///
/// The whitespace after the verb keeps `update: CI passed` a note. A body that
/// matches is a work request only if none of [`VERB_LED_REPORT_PATTERNS`]
/// also matches it.
pub(super) const EXPLICIT_WORK_REQUEST_PATTERN: &str =
    concat!(r"(?i)^(", change_verbs!(), r")\s+\S");

/// Reports that start with a change verb used as a noun.
///
/// Applied only to a body that matches [`EXPLICIT_WORK_REQUEST_PATTERN`], so
/// its first word is a change verb. The body is a report, and gets the note,
/// when any of these matches (one pattern each, in this order):
///
/// 1. It opens with `update on` (`update on PR 57: merged`).
/// 2. A later word is `is`, `was`, `are`, `were`, `has` or `had`, followed by
///    whitespace, punctuation or the end (`build is green`,
///    `build #1740 is green`, `fix for #12 has landed`).
/// 3. The word after the verb is an outcome word that ends the body or is
///    followed by punctuation, a preposition or a conjunction
///    (`build failed on PR 12`, `fix landed.`). An outcome word in that place
///    followed by a noun is an adjective in a request, so `fix failed tests`
///    and `update merged branches` stay work requests.
/// 4. An outcome word comes later than the word after the verb, followed by
///    whitespace, punctuation or the end. The report names its object before
///    its outcome: `fix #12 merged`, `build #1740 failed on main`,
///    `fix for #12 landed`, `build #1740 passed CI`.
///
/// Errors fall towards the note. A request that these patterns take for a
/// report, such as `fix the tests that failed`, `fix the failed tests` or
/// `fix the case where the list is empty`, gets the note, and the note's last
/// sentence sends a code change to dev-orchestrator. A report that they miss,
/// such as `fix #12 rolled back`, is decided as the same report without the
/// marker would be: inside the throttle window it gets the note, and on a new
/// topic it gets the routing prompt. That prompt applies the first rule that
/// matches, rule 4 (IMPLEMENT -> DEV) comes before the rule-8 exception for
/// result reports, and a DEV classification is told "Do not answer inline",
/// so the agent would usually start a recipe for a status line. The note is
/// the milder of the two mistakes.
pub(super) const VERB_LED_REPORT_PATTERNS: [&str; 4] = [
    r"(?i)^update\s+on\b",
    concat!(
        r"(?is)^(?:",
        change_verbs!(),
        r")\s.*\b(?:is|was|are|were|has|had)(?:[\s.,;:!?]|$)"
    ),
    concat!(
        r"(?i)^(?:",
        change_verbs!(),
        r")\s+(?:",
        outcomes!(),
        r")(?:\s*$|\s*[.,;:!?]|\s+(?:on|in|for|at|to|into|with|after|again|and|but)\b)"
    ),
    concat!(
        r"(?is)^(?:",
        change_verbs!(),
        r")\s+\S+\s.*\b(?:",
        outcomes!(),
        r")(?:[\s.,;:!?]|$)"
    ),
];

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
/// [`VERB_LED_REPORT_PATTERNS`] for the reports and the trade-off.
pub(super) fn is_explicit_work_request(body: &str) -> bool {
    static WORK: OnceLock<Regex> = OnceLock::new();
    static REPORT: OnceLock<RegexSet> = OnceLock::new();
    let work = WORK.get_or_init(|| {
        Regex::new(EXPLICIT_WORK_REQUEST_PATTERN).expect("valid explicit work request regex")
    });
    let report = REPORT.get_or_init(|| {
        RegexSet::new(VERB_LED_REPORT_PATTERNS).expect("valid verb-led report regexes")
    });
    work.is_match(body) && !report.is_match(body)
}
