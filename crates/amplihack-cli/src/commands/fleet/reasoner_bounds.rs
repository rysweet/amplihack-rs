//! Issue #1570: bounds on the untrusted session text that the fleet reasoner
//! puts into its prompt. Contract: `docs/reference/large-payload-delivery.md`,
//! "Fleet reasoner bounds".

use amplihack_utils::payload_delivery::{
    truncate_head_bytes, truncate_tail_bytes, truncate_tail_with_marker,
};

/// Lines of tmux scrollback requested by `capture-pane -p -S -<n>`.
pub(super) const REASONER_CAPTURE_LINES: usize = 2000;
/// Bytes of scrollback kept in the prompt (the tail of the capture).
pub(super) const REASONER_SCROLLBACK_MAX_BYTES: usize = 48 * 1024;
/// Most recent transcript messages kept in the prompt.
pub(super) const REASONER_TRANSCRIPT_MAX_MESSAGES: usize = 200;
/// Bytes kept from the head of each transcript message.
pub(super) const REASONER_TRANSCRIPT_MESSAGE_MAX_BYTES: usize = 4 * 1024;
/// Bytes kept from the tail of the joined transcript.
pub(super) const REASONER_TRANSCRIPT_MAX_BYTES: usize = 64 * 1024;

/// The tail of `capture`, at most [`REASONER_SCROLLBACK_MAX_BYTES`] bytes.
pub(super) fn bounded_scrollback(capture: &str) -> String {
    truncate_tail_with_marker(capture, REASONER_SCROLLBACK_MAX_BYTES).into_owned()
}

/// The most recent [`REASONER_TRANSCRIPT_MAX_MESSAGES`] lines of `transcript`,
/// each cut to its head, with the joined result cut to its tail.
pub(super) fn bounded_transcript(transcript: &str) -> String {
    // Walk from the end so only the kept lines are ever collected, however
    // long the transcript is.
    let mut recent = transcript
        .lines()
        .rev()
        .take(REASONER_TRANSCRIPT_MAX_MESSAGES)
        .map(|message| truncate_head_bytes(message, REASONER_TRANSCRIPT_MESSAGE_MAX_BYTES))
        .collect::<Vec<_>>();
    recent.reverse();
    let joined = recent.join("\n");
    truncate_tail_bytes(&joined, REASONER_TRANSCRIPT_MAX_BYTES).into_owned()
}
