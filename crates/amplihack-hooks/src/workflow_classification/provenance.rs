//! Prompts that are not a person's own (issue #2533, point 1).
//!
//! The harness writes some prompts itself: task notifications, system
//! notices, messages from another session and loop ticks. An agent step that
//! the recipe runner launched has `AMPLIHACK_STEP_ID` set, even when the run
//! variable was not passed on to it. None of these is a person changing topic,
//! so `process` inserts no routing text for them and records the rule that
//! decided it.
//!
//! The decision is made in code, from the environment and from the start of
//! the prompt. Leading whitespace is ignored, prefixes are literal and
//! case-sensitive, and no regex is used. An envelope that follows other text
//! is a person quoting it, and that prompt is routed as before. See
//! `docs/reference/coordination-messages.md#gate-order`.

use super::rule;

/// The frame the harness puts in front of a notice.
const SYSTEM_NOTICE: &str = "[SYSTEM NOTIFICATION - NOT USER INPUT]";

/// The envelope of a background task's result.
const TASK_NOTIFICATION_TAG: &str = "<task-notification";

/// The envelopes of a message from another session.
const PEER_TAGS: [&str; 2] = ["<cross-session-message", "<agent-message"];

/// Header lines the harness can put in front of a peer envelope.
const PEER_HEADERS: [&str; 3] = [
    "Another Claude session sent a message:",
    "Another Claude session sent a message while you were working:",
    "A peer session sent a message while you were working:",
];

/// Loop sentinels, matched in case the hook sees one before it is expanded.
const LOOP_SENTINELS: [&str; 4] = [
    "<<autonomous-loop>>",
    "<<autonomous-loop-dynamic>>",
    "<<loop.md>>",
    "<<loop.md-dynamic>>",
];

/// The headers that start the text a loop sentinel expands to.
const LOOP_HEADERS: [&str; 3] = [
    "# Autonomous loop tick",
    "# Autonomous loop check",
    "# /loop tick",
];

/// The `last_rule` value of a prompt that is not a person's, or `None`.
///
/// The step id is checked first, then the prompt, in the order of the rules
/// in `docs/reference/coordination-messages.md`. The first match wins.
pub(super) fn skip_rule(prompt: &str) -> Option<&'static str> {
    if crate::recipe_step::step_id_set() {
        return Some(rule::AGENT_STEP_ID);
    }
    let p = prompt.trim_start();
    if p.starts_with(SYSTEM_NOTICE) || starts_with_tag(p, TASK_NOTIFICATION_TAG) {
        return Some(rule::HARNESS_NOTICE);
    }
    if is_peer_envelope(p)
        || PEER_HEADERS.iter().any(|header| {
            p.strip_prefix(header)
                .is_some_and(|rest| is_peer_envelope(rest.trim_start()))
        })
    {
        return Some(rule::PEER_MESSAGE);
    }
    if LOOP_SENTINELS
        .iter()
        .any(|sentinel| p.starts_with(sentinel))
        || LOOP_HEADERS
            .iter()
            .any(|header| starts_with_heading(p, header))
    {
        return Some(rule::SCHEDULED_PROMPT);
    }
    None
}

/// `text` starts with `tag`, and the next character is `>` or ASCII
/// whitespace, so `<task-notification-extra>` is not a task notification.
fn starts_with_tag(text: &str, tag: &str) -> bool {
    text.strip_prefix(tag)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|next| next == '>' || next.is_ascii_whitespace())
}

fn is_peer_envelope(text: &str) -> bool {
    PEER_TAGS.iter().any(|tag| starts_with_tag(text, tag))
}

/// `text` starts with `header`, followed by whitespace or the end of the text.
fn starts_with_heading(text: &str, header: &str) -> bool {
    text.strip_prefix(header)
        .is_some_and(|rest| rest.chars().next().is_none_or(char::is_whitespace))
}
