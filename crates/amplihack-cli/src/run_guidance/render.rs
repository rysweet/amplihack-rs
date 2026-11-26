//! The operator-instructions block (issue #2535, D13) and its marked form
//! for `recipe guidance show` (issue #3129).

use super::{BLOCK_BEGIN, BLOCK_END, BLOCK_PREAMBLE, BLOCK_TITLE, Entry, STEP_PREAMBLE};
use std::fmt::Write as _;

/// The operator-instructions block for `entries` (empty string for none).
///
/// Entries from the operator come first, under the operator preamble.
/// Entries an agent step wrote ([`Entry::is_from_step`]) follow under their
/// own preamble, so a step that was misled into adding one cannot present
/// it as the operator's. Numbering runs on across both parts.
///
/// This is the block agent prompts receive; it is
/// [`render_block_marked`] with no mark.
pub(crate) fn render_block(entries: &[Entry]) -> String {
    render_block_marked(entries, None)
}

/// Whether the entry named `name` arrived after the entry named `since`.
///
/// Entry names sort in the order they were stamped, so this is a comparison
/// of names: an empty `since` sorts before every name and makes every entry
/// new. An entry stamped before `since` but published after it is not new
/// (the name-order limit in the module documentation).
pub(crate) fn is_new(name: &str, since: &str) -> bool {
    name > since
}

/// [`render_block`], with each entry's header line also carrying the
/// entry's file name, then ` NEW` when [`is_new`] says it arrived after
/// `since`. With `since` `None` the block is byte for byte the one agent
/// prompts receive.
pub(crate) fn render_block_marked(entries: &[Entry], since: Option<&str>) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let (from_steps, from_operator): (Vec<&Entry>, Vec<&Entry>) =
        entries.iter().partition(|e| e.is_from_step());
    let mut out = format!("{BLOCK_BEGIN}\n{BLOCK_TITLE}\n");
    let mut number = 0_usize;
    for (preamble, part) in [
        (BLOCK_PREAMBLE, &from_operator),
        (STEP_PREAMBLE, &from_steps),
    ] {
        if part.is_empty() {
            continue;
        }
        if number > 0 {
            out.push('\n');
        }
        out.push_str(preamble);
        out.push('\n');
        for e in part {
            number += 1;
            // Writing to a String cannot fail.
            let _ = write!(
                out,
                "\n[{number}] {} from {} ({})",
                e.timestamp,
                e.caller,
                e.source.label()
            );
            if let Some(since) = since {
                let _ = write!(out, " {}", e.name);
                if is_new(&e.name, since) {
                    out.push_str(" NEW");
                }
            }
            out.push('\n');
            out.push_str(&e.text);
            out.push('\n');
        }
    }
    out.push_str(BLOCK_END);
    out.push('\n');
    out
}

/// `prompt` with trailing line breaks removed, one blank line, then `block`.
pub(crate) fn append_block(prompt: &str, block: &str) -> String {
    format!("{}\n\n{block}", prompt.trim_end_matches(['\n', '\r']))
}
