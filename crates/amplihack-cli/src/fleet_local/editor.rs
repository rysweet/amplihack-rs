//! Multiline proposal editor buffer (`EditorState`).

use super::{FleetLocalError, EDITOR_MAX_BYTES_PER_LINE, EDITOR_MAX_LINES};

// ── EditorState ───────────────────────────────────────────────────────────────

/// Multiline proposal editor buffer.
///
/// Hard limits (SEC-09):
/// - [`EDITOR_MAX_BYTES_PER_LINE`] bytes per line (4 096).
/// - [`EDITOR_MAX_LINES`] lines total (200).
///
/// Control characters below `0x20` (except `\t` and `\n`) are silently
/// stripped before storage (SEC-08).
#[derive(Debug, Clone, Default)]
pub struct EditorState {
    /// Zero-based row of the text cursor.
    pub cursor_row: usize,
    /// Zero-based column of the text cursor (byte offset in the current line).
    pub cursor_col: usize,
    /// The editor buffer, one `String` per line.
    pub lines: Vec<String>,
}

impl EditorState {
    /// Create a new, empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the cursor up one row, clamping at row 0.
    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            // Clamp cursor_col to the length of the new current line.
            let line_len = self.lines.get(self.cursor_row).map_or(0, |l| l.len());
            if self.cursor_col > line_len {
                self.cursor_col = line_len;
            }
        }
    }

    /// Move the cursor down one row, clamping at the last line.
    pub fn move_down(&mut self) {
        let last = self.lines.len().saturating_sub(1);
        if self.cursor_row < last {
            self.cursor_row += 1;
            // Clamp cursor_col to the length of the new current line.
            let line_len = self.lines.get(self.cursor_row).map_or(0, |l| l.len());
            if self.cursor_col > line_len {
                self.cursor_col = line_len;
            }
        }
    }

    /// Move the cursor left one byte, wrapping to the end of the previous line.
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines.get(self.cursor_row).map_or(0, |l| l.len());
        }
    }

    /// Move the cursor right one byte, wrapping to the start of the next line.
    pub fn move_right(&mut self) {
        let line_len = self.lines.get(self.cursor_row).map_or(0, |l| l.len());
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Insert a character at the cursor position.
    ///
    /// - Strips control characters below `0x20` except `\t` (SEC-08).
    /// - Splits the line on `\n` insertion.
    /// - Silently drops input when the 200-line or 4 096-byte-per-line limit
    ///   would be exceeded (SEC-09).
    pub fn insert_char(&mut self, ch: char) {
        // SEC-08: strip control chars < 0x20 except \t and \n.
        if (ch as u32) < 0x20 && ch != '\t' && ch != '\n' {
            return;
        }

        if ch == '\n' {
            // Check 200-line limit before splitting.
            if self.lines.len() >= EDITOR_MAX_LINES {
                return;
            }
            // Ensure there is at least one line to split.
            if self.lines.is_empty() {
                self.lines.push(String::new());
                self.cursor_row = 0;
                self.cursor_col = 0;
            }
            let row = self.cursor_row.min(self.lines.len().saturating_sub(1));
            let col = self.cursor_col.min(self.lines[row].len());
            let tail = self.lines[row].split_off(col);
            self.lines.insert(row + 1, tail);
            self.cursor_row = row + 1;
            self.cursor_col = 0;
            return;
        }

        // Ensure there is at least one line.
        if self.lines.is_empty() {
            self.lines.push(String::new());
            self.cursor_row = 0;
            self.cursor_col = 0;
        }

        let row = self.cursor_row.min(self.lines.len().saturating_sub(1));

        // SEC-09: check 4096-byte-per-line limit.
        let char_len = ch.len_utf8();
        if self.lines[row].len() + char_len > EDITOR_MAX_BYTES_PER_LINE {
            return;
        }

        let col = self.cursor_col.min(self.lines[row].len());
        self.lines[row].insert(col, ch);
        self.cursor_col = col + char_len;
    }

    /// Apply an AI-suggested proposal text at the cursor position.
    ///
    /// - Validates `text` is valid UTF-8 via `String::from_utf8()` internally.
    /// - Returns `Err(FleetLocalError::InvalidUtf8)` if the text contains
    ///   invalid sequences.
    /// - Inserts each line of the proposal at the current cursor row.
    pub fn apply_proposal(&mut self, text: &str) -> Result<(), FleetLocalError> {
        // Validate UTF-8 by round-tripping through bytes (spec: String::from_utf8()).
        String::from_utf8(text.as_bytes().to_vec()).map_err(|_| FleetLocalError::InvalidUtf8)?;

        // Insert each character (handles newlines via insert_char logic).
        for ch in text.chars() {
            self.insert_char(ch);
        }
        Ok(())
    }

    /// Total number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The full buffer content as a single string (lines joined by `\n`).
    pub fn content(&self) -> String {
        self.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_default_is_empty() {
        let e = EditorState::default();
        assert_eq!(e.cursor_row, 0);
        assert_eq!(e.cursor_col, 0);
        assert!(e.lines.is_empty());
        assert_eq!(e.line_count(), 0);
        assert_eq!(e.content(), "");
    }

    #[test]
    fn editor_state_content_joins_lines_with_newline() {
        let e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec!["hello".to_string(), "world".to_string()],
        };
        assert_eq!(e.content(), "hello\nworld");
    }

    #[test]
    fn editor_state_move_up_from_zero_does_not_underflow() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec!["line0".to_string()],
        };
        e.move_up();
        assert_eq!(e.cursor_row, 0, "cursor must stay at row 0");
    }

    #[test]
    fn editor_state_move_down_clamps_at_last_line() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec!["line0".to_string(), "line1".to_string()],
        };
        e.move_down();
        assert_eq!(e.cursor_row, 1);
        e.move_down();
        assert_eq!(e.cursor_row, 1, "cursor must clamp at last line");
    }

    #[test]
    fn editor_state_insert_char_appends_to_line() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec![String::new()],
        };
        e.insert_char('h');
        e.insert_char('i');
        assert!(
            e.lines[0].contains('h') || e.lines[0].contains("hi"),
            "inserted chars must appear in the line"
        );
    }

    #[test]
    fn editor_state_enforces_4096_byte_per_line_limit() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec!["a".repeat(EDITOR_MAX_BYTES_PER_LINE)],
        };
        e.cursor_col = EDITOR_MAX_BYTES_PER_LINE;
        e.insert_char('x');

        assert!(
            e.lines[0].len() <= EDITOR_MAX_BYTES_PER_LINE,
            "line must not exceed {} bytes; got {}",
            EDITOR_MAX_BYTES_PER_LINE,
            e.lines[0].len()
        );
    }

    #[test]
    fn editor_state_enforces_200_line_limit_on_newline_insert() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: (0..EDITOR_MAX_LINES).map(|i| format!("line {i}")).collect(),
        };
        e.cursor_row = EDITOR_MAX_LINES - 1;
        e.cursor_col = 0;
        e.insert_char('\n');

        assert!(
            e.line_count() <= EDITOR_MAX_LINES,
            "editor must not exceed {} lines; got {}",
            EDITOR_MAX_LINES,
            e.line_count()
        );
    }

    #[test]
    fn editor_state_strips_control_chars_below_0x20_except_tab() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec![String::new()],
        };
        for &ctrl in &['\x01', '\x04', '\x1b'] {
            e.insert_char(ctrl);
        }
        assert!(
            e.lines[0].is_empty(),
            "control chars 0x01/0x04/0x1b must be stripped; line = {:?}",
            e.lines[0]
        );
    }

    #[test]
    fn editor_state_apply_proposal_with_valid_utf8_succeeds() {
        let mut e = EditorState {
            cursor_row: 0,
            cursor_col: 0,
            lines: vec![String::new()],
        };
        let result = e.apply_proposal("hello, world");
        assert!(
            result.is_ok(),
            "valid UTF-8 proposal must succeed; got {result:?}"
        );
    }
}
