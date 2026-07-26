//! Outbound message chunking sized to Signal's per-message limit.
//!
//! Agent turns can be far larger than a single Signal message. Before posting a
//! (already-redacted) body to the group it is split into [`SIGNAL_MAX_BYTES`]
//! chunks that reassemble losslessly. This bound is **distinct** from the
//! inbound JSON-RPC frame bound (`transport::MAX_FRAME_BYTES`, 256 KiB): that
//! caps a single wire frame, whereas this caps what one Signal message can
//! usefully carry.

/// Maximum size, in bytes, of a single outbound Signal message body.
///
/// Deliberately conservative and well under any JSON-RPC frame bound so a large
/// agent turn is delivered as several readable messages rather than one giant
/// (or rejected) one.
pub const SIGNAL_MAX_BYTES: usize = 2000;

/// Split `body` into chunks each at most [`SIGNAL_MAX_BYTES`] bytes.
///
/// Never splits a UTF-8 code point (each chunk is valid UTF-8), and the chunks
/// reassemble to exactly `body` (`chunk(b).concat() == b`). A body already
/// within the cap is returned as a single chunk; an empty body yields no
/// chunks.
#[must_use]
pub fn chunk(body: &str) -> Vec<String> {
    if body.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::with_capacity(body.len().div_ceil(SIGNAL_MAX_BYTES));
    let mut start = 0;
    while start < body.len() {
        let mut end = (start + SIGNAL_MAX_BYTES).min(body.len());
        // Back off to the nearest char boundary so no multibyte code point is
        // cut. A single code point is at most 4 bytes, far below the cap, so
        // `end` can never fall back all the way to `start`.
        while end > start && !body.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(body[start..end].to_string());
        start = end;
    }
    chunks
}
