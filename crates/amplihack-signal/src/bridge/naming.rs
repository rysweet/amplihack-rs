//! Deterministic Signal group naming for the `/signal` bridge.
//!
//! A bridge group is named `amplihack-<host>[-<tmux>]-<slug(topic)>` so an
//! operator can tell at a glance which host, tmux session, and topic a group
//! drives. Naming is a **pure function** — the same inputs always yield the
//! same name — which keeps it unit-testable and free of hidden state.

/// Maximum length of a slugified topic segment.
///
/// Signal imposes a limit on group names; the topic is the only unbounded
/// segment (host and tmux tokens are short), so the cap is applied to it. The
/// cap never leaves a dangling `-` (see [`slug`]).
pub const MAX_TOPIC_SLUG: usize = 40;

/// Slugify arbitrary free text into a lowercase, `-`-delimited token.
///
/// Rules (all applied):
/// - lowercase,
/// - every run of non-alphanumeric characters collapses to a single `-`,
/// - leading/trailing `-` are trimmed,
/// - the result is capped to [`MAX_TOPIC_SLUG`] bytes without leaving a
///   trailing `-`.
///
/// Slugs are ASCII by construction (only `[a-z0-9-]`), so the byte cap is also
/// a char cap and never splits a code point.
#[must_use]
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TOPIC_SLUG));
    let mut prev_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    // Trim to the cap, then strip any leading/trailing '-' (a trailing '-' can
    // appear either from the input's edges or from truncation mid-separator).
    let trimmed_len = out.len().min(MAX_TOPIC_SLUG);
    out.truncate(trimmed_len);
    out.trim_matches('-').to_string()
}

/// Build the deterministic bridge group name from its parts.
///
/// `tmux` is included only when present and non-empty (a bridge started outside
/// tmux omits that segment). All parts are slugified so an odd hostname or
/// session name cannot produce an invalid group name.
#[must_use]
pub fn group_name(host: &str, tmux: Option<&str>, topic: &str) -> String {
    let mut parts = vec!["amplihack".to_string(), slug(host)];
    if let Some(session) = tmux {
        let s = slug(session);
        if !s.is_empty() {
            parts.push(s);
        }
    }
    parts.push(slug(topic));
    parts.retain(|p| !p.is_empty());
    parts.join("-")
}
