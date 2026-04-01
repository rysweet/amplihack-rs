//! OSC escape sequence stripping for terminal output.

/// Strip OSC escape sequences from terminal output before TUI rendering.
///
/// Both termination forms must be stripped (SEC-06):
/// - `\x1b]...\x07`  (BEL-terminated)
/// - `\x1b]...\x1b\` (ST-terminated)
///
/// Stripping only one form would leave an injection vector.
pub fn strip_osc_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut result = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        // Check for ESC ] (OSC prefix: 0x1b 0x5d).
        if i + 1 < len && bytes[i] == 0x1b && bytes[i + 1] == b']' {
            // Scan forward to find the terminator.
            let start = i;
            i += 2; // skip ESC ]
            let mut found = false;
            while i < len {
                if bytes[i] == 0x07 {
                    // BEL terminator.
                    i += 1;
                    found = true;
                    break;
                } else if i + 1 < len && bytes[i] == 0x1b && bytes[i + 1] == b'\\' {
                    // ST terminator (ESC \).
                    i += 2;
                    found = true;
                    break;
                }
                i += 1;
            }
            if !found {
                // No terminator found — emit the raw bytes (not a complete OSC).
                result.extend_from_slice(&bytes[start..i]);
            }
            // If found, the OSC sequence is simply dropped.
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    // SAFETY: input is valid UTF-8 and we only strip complete OSC sequences
    // (which are ASCII bytes), so the remaining bytes are still valid UTF-8.
    String::from_utf8(result).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_osc_removes_bel_terminated_sequence() {
        let input = "before\x1b]0;window title\x07after";
        let result = strip_osc_sequences(input);
        assert_eq!(result, "beforeafter", "BEL-terminated OSC must be stripped");
    }

    #[test]
    fn strip_osc_removes_st_terminated_sequence() {
        let input = "before\x1b]0;window title\x1b\\after";
        let result = strip_osc_sequences(input);
        assert_eq!(result, "beforeafter", "ST-terminated OSC must be stripped");
    }

    #[test]
    fn strip_osc_handles_empty_string() {
        assert_eq!(strip_osc_sequences(""), "");
    }

    #[test]
    fn strip_osc_preserves_non_osc_ansi_codes() {
        let input = "\x1b[1mbold\x1b[0m";
        let result = strip_osc_sequences(input);
        assert_eq!(result, input, "non-OSC ANSI codes must be preserved");
    }

    #[test]
    fn strip_osc_preserves_plain_text() {
        let input = "plain text without any escape sequences";
        assert_eq!(strip_osc_sequences(input), input);
    }

    #[test]
    fn strip_osc_handles_multiple_sequences_in_input() {
        let input = "\x1b]0;title1\x07text\x1b]0;title2\x1b\\end";
        let result = strip_osc_sequences(input);
        assert_eq!(result, "textend");
    }

    #[test]
    fn strip_osc_must_strip_both_forms_independently() {
        let input = "\x1b]0;t1\x07mid\x1b]0;t2\x1b\\end";
        let result = strip_osc_sequences(input);
        assert!(
            !result.contains("\x1b]"),
            "residual OSC prefix found: {result:?}"
        );
        assert!(
            result.contains("mid"),
            "content between sequences must survive"
        );
        assert!(result.contains("end"), "trailing content must survive");
    }
}
