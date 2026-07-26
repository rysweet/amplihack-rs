//! Control-phrase parsing for inbound bridge messages.
//!
//! Every accepted group message is first classified by [`parse_control`]
//! **before** it is treated as an agent prompt. Reserved control words
//! (`status`, `stop`, `kill`) are matched **only** as the entire message body
//! (after trimming). Any other text — including a sentence that merely *contains*
//! a reserved word — is a normal [`Control::Prompt`].
//!
//! This precedence is a safety property: `stop`/`kill` must always be able to
//! pre-empt an in-flight turn, but the operator must equally be able to *talk*
//! about stopping ("please stop the review") without accidentally killing the
//! bridge.

/// The classification of one inbound message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Report bridge state to the group (session id, turn, allowlist, queue).
    Status,
    /// Pre-empt: terminate the child agent, close the group, and exit.
    Stop,
    /// Ordinary text to be run as the next agent prompt (verbatim, trimmed).
    Prompt(String),
}

/// Classify one inbound message body.
///
/// Total function: never errors. Leading/trailing whitespace is ignored for the
/// control-word comparison and stripped from the resulting prompt.
#[must_use]
pub fn parse_control(body: &str) -> Control {
    let trimmed = body.trim();
    // Compare ASCII-case-insensitively without lowercasing the whole body: the
    // common `Prompt` case can be a multi-KB agent instruction, and
    // `eq_ignore_ascii_case` short-circuits on length so it never scans past the
    // few bytes of a reserved word.
    if trimmed.eq_ignore_ascii_case("status") {
        Control::Status
    } else if trimmed.eq_ignore_ascii_case("stop") || trimmed.eq_ignore_ascii_case("kill") {
        Control::Stop
    } else {
        Control::Prompt(trimmed.to_string())
    }
}
