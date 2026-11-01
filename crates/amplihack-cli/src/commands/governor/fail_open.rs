//! Fail-open admissions (D11): why the governor admitted without deciding,
//! and the record of the last one kept in `state.json`. Re-exported from
//! [`super::model`].

use serde::{Deserialize, Serialize};

/// Why the governor admitted without deciding (D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailOpenCode {
    NoRoot,
    StateDir,
    LockTimeout,
    CorruptState,
    StateVersion,
    NoReadings,
    QueueFull,
}

impl FailOpenCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRoot => "no-root",
            Self::StateDir => "state-dir",
            Self::LockTimeout => "lock-timeout",
            Self::CorruptState => "corrupt-state",
            Self::StateVersion => "state-version",
            Self::NoReadings => "no-readings",
            Self::QueueFull => "queue-full",
        }
    }
}

/// A fault that admits. The message never holds a path, an account key or
/// task text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailOpen {
    pub code: FailOpenCode,
    pub message: String,
}

/// The longest fail-open message. It matches the 512-byte check in
/// `files.rs` that `state.json` passes when it is loaded again.
pub const MAX_FAIL_OPEN_MESSAGE: usize = 512;

impl FailOpen {
    /// The message is sanitised for the terminal and the log, and cut to
    /// [`MAX_FAIL_OPEN_MESSAGE`] bytes on a character boundary, so a saved
    /// record always passes validation.
    pub fn new(code: FailOpenCode, message: impl Into<String>) -> Self {
        let mut message = crate::user_config::safe::display_safe(&message.into());
        if message.len() > MAX_FAIL_OPEN_MESSAGE {
            let mut end = MAX_FAIL_OPEN_MESSAGE;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Self { code, message }
    }
}

/// The last fail-open admission, kept in `state.json` for `governor status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailOpenRecord {
    pub code: FailOpenCode,
    pub message: String,
    pub at: i64,
}
