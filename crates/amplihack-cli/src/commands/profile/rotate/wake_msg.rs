//! Issue #2909: the fixed lines about the rotator's event wake. A file of
//! its own because `log.rs` is at its line budget, following the
//! `fresh_msg`/`overflow_msg` split. See "Rotate wake" in
//! docs/reference/profile-command.md.
//!
//! Every line is fixed text with, at most, a cause word from [`cause`] in
//! it. No value a ledger writer chose ever reaches a line here, and no log
//! line anywhere names a stored block reason, so a process able to write
//! the ledger cannot choose text in the rotate log.

/// Why the watch loop stopped waiting. [`Reason::line`] is what it logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The blocks recorded in `ledger.json` differ from those the last
    /// cycle began with: a step or hook recorded a limit, `profile overflow
    /// clear` removed one, or the last cycle recorded one itself.
    Ledger,
    /// The earliest recorded reset fell due, so a blocked login is back.
    Reset,
    /// `--interval` passed with neither of the above.
    Interval,
}

impl Reason {
    pub fn line(self) -> &'static str {
        match self {
            Self::Ledger => "wake: ledger changed",
            Self::Reset => "wake: reset due",
            Self::Interval => "wake: interval",
        }
    }
}

/// No watch could be made: not Linux, inotify refused, or the stop pipe
/// could not be set up. The rotator runs on with `--interval` as its only
/// wake; nothing ends. `wake: unavailable (<cause>), using --interval`.
pub fn wake_unavailable(e: &std::io::Error) -> String {
    format!("wake: unavailable ({}), using --interval", cause(e))
}

/// The wait failed after it had been working. As above: `--interval` only.
/// `wake: lost (<cause>), using --interval`.
pub fn wake_failed(e: &std::io::Error) -> String {
    format!("wake: lost ({}), using --interval", cause(e))
}

/// The watch was lost or its queue overflowed, and a new one replaced it.
pub const WAKE_REARMED: &str = "wake: watch rebuilt";

/// Fixed words for why the intake could not be made or kept, so the line
/// is evidence an operator can act on and never echoes an OS message. The
/// two inotify limits are named, because they are the likely causes on a
/// busy host and the generic words for them ("disk full", "I/O error")
/// would mislead.
pub fn cause(e: &std::io::Error) -> &'static str {
    match (e.kind(), e.raw_os_error()) {
        (std::io::ErrorKind::Unsupported, _) => "not supported here",
        (_, Some(libc::ENOSPC)) => "inotify watch limit reached",
        (_, Some(libc::EMFILE)) => "descriptor or inotify instance limit reached",
        (kind, _) => crate::commands::sessions::kind_word(kind),
    }
}
