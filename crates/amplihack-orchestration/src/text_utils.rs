//! Small string helpers shared across patterns.
//!
//! `truncate_at_char_boundary` provides UTF-8-safe slicing — naive byte
//! slicing like `&s[..3000]` panics if the cut lands inside a multi-byte
//! codepoint, which is a real risk when truncating arbitrary model output.

/// Return the longest prefix of `s` that fits within `max_bytes` and ends
/// on a UTF-8 character boundary.
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_full_when_short() {
        assert_eq!(truncate_at_char_boundary("hello", 100), "hello");
    }

    #[test]
    fn snaps_back_from_mid_codepoint() {
        // "é" is 2 bytes (0xC3 0xA9). Cutting at byte 1 lands mid-char.
        let s = "aé";
        assert_eq!(s.len(), 3);
        assert_eq!(truncate_at_char_boundary(s, 2), "a");
        assert_eq!(truncate_at_char_boundary(s, 3), "aé");
    }

    #[test]
    fn handles_zero() {
        assert_eq!(truncate_at_char_boundary("abc", 0), "");
    }

    #[test]
    fn handles_emoji() {
        // "🎉" is 4 bytes. Any cut between 1..=3 must snap back to 0.
        let s = "🎉🎉";
        for n in 1..=3 {
            assert_eq!(truncate_at_char_boundary(s, n), "");
        }
        assert_eq!(truncate_at_char_boundary(s, 4), "🎉");
    }
}
