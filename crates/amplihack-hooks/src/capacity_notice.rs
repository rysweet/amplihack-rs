//! The no-capacity notice for SessionStart and UserPromptSubmit: one bounded
//! read of `<root>/.auto-switch.d/capacity.json` (through
//! `amplihack_utils::claude_capacity`), shown once per state change per
//! session. Never an error, never a subprocess, never the network.
//!
//! Both outputs are built only from constant text, the state, the threshold
//! and the parsed reset time. The file's `over` and `writer` strings never
//! reach the user or the model.

use amplihack_utils::claude_capacity::{self as cap, dedupe};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// What a hook adds when capacity is low or exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    /// The user-visible line (top-level `systemMessage`); `None` in a nested
    /// recipe session, whose output nobody reads.
    pub(crate) system_message: Option<String>,
    /// The model context, appended to `additionalContext`.
    pub(crate) context: String,
}

/// The notice to show now for `session_id`, or `None`. `nested` is
/// `is_nested_recipe_session()`.
pub(crate) fn notice(session_id: Option<&str>, nested: bool) -> Option<Notice> {
    // catch_unwind works only because the workspace builds with
    // `panic = "unwind"` (Cargo.toml): a bug here must never fail a prompt.
    catch_unwind(AssertUnwindSafe(|| build(session_id, nested)))
        .ok()
        .flatten()
}

fn build(session_id: Option<&str>, nested: bool) -> Option<Notice> {
    let root = cap::resolve_root()?;
    let (state, file) = cap::current_now(&root)?;
    // Called for `Ok` too, so the session's record is cleared and a later
    // drop shows the notice again.
    if !dedupe::should_show(&root, session_id, state) {
        return None;
    }
    let line = cap::format_line(state, file.threshold, file.earliest_reset)?;
    let context = format!(
        "Claude account capacity is {} across all profiles (next reset {}). \
         Avoid launching large parallel workflows, multi-agent fan-out or long \
         recipe runs; prefer small sequential steps.",
        state.as_str(),
        cap::format_reset(file.earliest_reset)
    );
    Some(Notice {
        system_message: (!nested).then(|| format!("{line} — run '{}'", cap::INSPECT)),
        context,
    })
}

#[cfg(all(test, unix))]
#[path = "capacity_notice_tests.rs"]
pub(crate) mod tests;
