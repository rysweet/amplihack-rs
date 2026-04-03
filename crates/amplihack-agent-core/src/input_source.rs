//! Input sources for the agentic loop.
//!
//! Ports Python `input_source.py` — provides the `InputSource` trait and
//! concrete implementations: `ListInputSource`, `StdinInputSource`, and
//! event extraction utilities.
//!
//! Azure-specific sources (Service Bus, Event Hubs) are left as trait
//! implementations for external crates to provide, keeping this crate
//! dependency-free of Azure SDKs.

use std::io::{self, BufRead};

// ---------------------------------------------------------------------------
// InputSource trait
// ---------------------------------------------------------------------------

/// Blocking input source for the OODA loop.
///
/// Implementations return the next input string on each call to [`next`],
/// or `None` when the source is exhausted.
pub trait InputSource: Send {
    /// Return the next input or `None` when exhausted.
    fn next(&mut self) -> Option<String>;

    /// Release any held resources.
    fn close(&mut self);
}

// ---------------------------------------------------------------------------
// ListInputSource
// ---------------------------------------------------------------------------

/// In-memory list of pre-loaded turns.
pub struct ListInputSource {
    turns: Vec<String>,
    position: usize,
    closed: bool,
}

impl ListInputSource {
    /// Create from a list of turns.
    pub fn new(turns: Vec<String>) -> Self {
        Self {
            turns,
            position: 0,
            closed: false,
        }
    }

    /// Total number of turns.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether there are no turns.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Number of unconsumed turns remaining.
    pub fn remaining(&self) -> usize {
        if self.position >= self.turns.len() {
            0
        } else {
            self.turns.len() - self.position
        }
    }
}

impl InputSource for ListInputSource {
    fn next(&mut self) -> Option<String> {
        if self.closed || self.position >= self.turns.len() {
            return None;
        }
        let turn = self.turns[self.position].clone();
        self.position += 1;
        Some(turn)
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

// ---------------------------------------------------------------------------
// StdinInputSource
// ---------------------------------------------------------------------------

/// Reads lines from stdin (or any `BufRead`).
pub struct StdinInputSource {
    prompt: String,
    eof_on_empty: bool,
    closed: bool,
    reader: Box<dyn BufRead + Send>,
}

impl StdinInputSource {
    /// Create reading from real stdin.
    pub fn new(prompt: &str, eof_on_empty: bool) -> Self {
        Self {
            prompt: prompt.to_string(),
            eof_on_empty,
            closed: false,
            reader: Box::new(io::BufReader::new(io::stdin())),
        }
    }

    /// Create reading from a custom `BufRead` (useful for testing).
    pub fn from_reader(prompt: &str, eof_on_empty: bool, reader: Box<dyn BufRead + Send>) -> Self {
        Self {
            prompt: prompt.to_string(),
            eof_on_empty,
            closed: false,
            reader,
        }
    }
}

impl InputSource for StdinInputSource {
    fn next(&mut self) -> Option<String> {
        if self.closed {
            return None;
        }

        if !self.prompt.is_empty() {
            eprint!("{}", self.prompt);
        }

        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None, // EOF
            Ok(_) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() && self.eof_on_empty {
                    None
                } else {
                    Some(trimmed)
                }
            }
            Err(_) => None,
        }
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ListInputSource ----

    #[test]
    fn list_source_returns_turns_in_order() {
        let mut src = ListInputSource::new(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(src.len(), 3);
        assert_eq!(src.remaining(), 3);
        assert_eq!(src.next(), Some("a".into()));
        assert_eq!(src.remaining(), 2);
        assert_eq!(src.next(), Some("b".into()));
        assert_eq!(src.next(), Some("c".into()));
        assert_eq!(src.next(), None);
        assert_eq!(src.remaining(), 0);
    }

    #[test]
    fn list_source_empty() {
        let mut src = ListInputSource::new(vec![]);
        assert!(src.is_empty());
        assert_eq!(src.next(), None);
    }

    #[test]
    fn list_source_close_stops_iteration() {
        let mut src = ListInputSource::new(vec!["a".into(), "b".into()]);
        assert_eq!(src.next(), Some("a".into()));
        src.close();
        assert_eq!(src.next(), None);
    }

    // ---- StdinInputSource (from_reader) ----

    #[test]
    fn stdin_source_reads_lines() {
        let data = b"hello\nworld\n";
        let reader = Box::new(io::BufReader::new(&data[..]));
        let mut src = StdinInputSource::from_reader("", false, reader);
        assert_eq!(src.next(), Some("hello".into()));
        assert_eq!(src.next(), Some("world".into()));
        assert_eq!(src.next(), None);
    }

    #[test]
    fn stdin_source_eof_on_empty() {
        let data = b"hello\n\n";
        let reader = Box::new(io::BufReader::new(&data[..]));
        let mut src = StdinInputSource::from_reader("", true, reader);
        assert_eq!(src.next(), Some("hello".into()));
        assert_eq!(src.next(), None);
    }

    #[test]
    fn stdin_source_close() {
        let data = b"hello\nworld\n";
        let reader = Box::new(io::BufReader::new(&data[..]));
        let mut src = StdinInputSource::from_reader("", false, reader);
        assert_eq!(src.next(), Some("hello".into()));
        src.close();
        assert_eq!(src.next(), None);
    }
}
