//! The SessionEnd marker contract of `amplihack agent` (issue #1760, R6.3,
//! S7). The `session-stop-event` hook writes `~/.amplihack/agents/ended/<id>`
//! when a session is exited on purpose, and the supervisor reads it. Both
//! take these definitions from here, so a change to one side cannot leave the
//! other matching nothing.

/// The SessionEnd reasons that mean the user ended the session. A lost
/// terminal sends `other`; `clear` and `resume` keep the session running.
/// Claude Code has no `exit` reason.
pub const CLEAN_REASONS: &[&str] = &["prompt_input_exit", "logout"];

/// `^[A-Za-z0-9][A-Za-z0-9-]{0,63}$`: a conversation id, and so a marker's
/// file name. A leading `-` would make the id a Claude flag after `--resume`.
pub fn is_conversation_id(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && s.len() <= 64
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clean_reasons() {
        assert_eq!(CLEAN_REASONS, ["prompt_input_exit", "logout"]);
    }

    #[test]
    fn the_conversation_id_rule() {
        let max = "a".repeat(64);
        for ok in ["0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b", "a", "Z9-", &max] {
            assert!(is_conversation_id(ok), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in ["", "-a", "a_b", "a.b", "a b", "a/b", "é", "a\n", &long] {
            assert!(!is_conversation_id(bad), "{bad:?}");
        }
    }
}
