//! Issue #2589 (SEC-V1/SEC-V2): the validated command prefix.
//!
//! A `Prefix` is the only thing the three renderings accept, and its one
//! constructor is [`TryFrom<&str>`]. Validation is an ASCII whitelist rather
//! than escaping, because each rendering has its own syntax and a blacklist
//! would have to anticipate all three: the colon and the asterisk end a
//! Claude `Bash(...)` pattern, the parentheses delimit a Copilot `shell(...)`
//! pattern, and the quote and the backslash are syntax in the Codex rules
//! file as well as in a shell. Everything outside the whitelist is refused,
//! so a renderer is never handed a string it would have to escape.

use std::fmt;

/// Longest accepted prefix, in bytes.
pub(crate) const MAX_BYTES: usize = 128;

/// Most words one prefix may hold. A command prefix is a command name and a
/// few flags; anything longer is a command line, not a prefix.
pub(crate) const MAX_TOKENS: usize = 8;

/// A command prefix that every rendering may use verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Prefix(String);

impl Prefix {
    /// The validated text, which holds whitelisted ASCII only.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a value is not a prefix. Each variant prints one plain sentence and
/// never quotes the value, which is attacker-chosen; the caller decides
/// whether to show it, and escapes it when it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrefixError {
    /// Empty, or longer than [`MAX_BYTES`].
    Length,
    /// More than [`MAX_TOKENS`] words.
    Words,
    /// A leading, trailing or repeated space.
    Separator,
    /// A character outside the whitelist.
    Character,
}

impl fmt::Display for PrefixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrefixError::Length => write!(f, "must be 1 to {MAX_BYTES} bytes long"),
            PrefixError::Words => write!(f, "must hold at most {MAX_TOKENS} words"),
            PrefixError::Separator => f.write_str(
                "must separate its words with exactly one space, and start and end with a word",
            ),
            PrefixError::Character => {
                f.write_str("must hold only ASCII letters, digits and the characters . _ - / @ = +")
            }
        }
    }
}

impl std::error::Error for PrefixError {}

/// The whitelist: everything else is refused, including every shell
/// metacharacter and every character that is syntax in a rendering.
fn is_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '@' | '=' | '+')
}

impl TryFrom<&str> for Prefix {
    type Error = PrefixError;

    fn try_from(raw: &str) -> Result<Self, Self::Error> {
        if raw.is_empty() || raw.len() > MAX_BYTES {
            return Err(PrefixError::Length);
        }
        let words: Vec<&str> = raw.split(' ').collect();
        if words.len() > MAX_TOKENS {
            return Err(PrefixError::Words);
        }
        if words.iter().any(|word| word.is_empty()) {
            return Err(PrefixError::Separator);
        }
        if !raw.chars().all(|c| c == ' ' || is_allowed(c)) {
            return Err(PrefixError::Character);
        }
        Ok(Self(raw.to_string()))
    }
}

impl TryFrom<&String> for Prefix {
    type Error = PrefixError;

    fn try_from(raw: &String) -> Result<Self, Self::Error> {
        Prefix::try_from(raw.as_str())
    }
}
