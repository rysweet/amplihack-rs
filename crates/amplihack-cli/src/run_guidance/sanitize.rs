//! The one escaping pass for every line the guidance commands print
//! (issue #2783, CWE-150).
//!
//! `recipe guide`, `recipe guidance` and the launcher's guidance warnings
//! all print paths, and a path can carry a name chosen by whoever created
//! a folder. Every line they print goes through [`sanitize_line`], so no
//! name can add a line or a terminal sequence to the output.

/// Characters that never reach a terminal as themselves: control characters,
/// the bidirectional and zero-width marks, and the line and paragraph
/// separators, which would end the one line a message is meant to be.
pub(crate) fn needs_escape(c: char) -> bool {
    crate::commands::agent::validate::unsafe_char(c) || matches!(c, '\u{2028}' | '\u{2029}')
}

/// `c` written as `\u{..}`.
pub(crate) fn escaped(c: char) -> String {
    format!("\\u{{{:04x}}}", c as u32)
}

/// The last pass over a line before it is printed, on stdout as well as
/// stderr: every character [`needs_escape`] names is written as `\u{..}`.
pub(crate) fn sanitize_line(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for c in message.chars() {
        if needs_escape(c) {
            out.push_str(&escaped(c));
        } else {
            out.push(c);
        }
    }
    out
}
