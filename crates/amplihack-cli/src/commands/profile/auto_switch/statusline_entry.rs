//! Which `statusLine` commands are amplihack's own (issue #1745).
//!
//! `install`, `uninstall`, `profile auto-switch install` and the status-line
//! wrapper all ask this one question, so they cannot disagree about whose
//! command it is. The match is exact: a command that runs one of these and
//! does more, or only contains the same words, is the user's.

use super::MARKER;
use std::ffi::OsStr;
use std::path::Path;

/// The script older installs pointed the entry at, after `~` or `$HOME`.
const LEGACY_SCRIPT: &str = "/.amplihack/.claude/tools/statusline.sh";

/// Whether `command`, after trimming, is the `~/.claude` entry and nothing
/// else: a single-quoted absolute path to a binary named `amplihack` followed
/// by ` profile statusline`, or the bare `amplihack profile statusline`.
pub fn is_home_statusline(command: &str) -> bool {
    let c = command.trim();
    if c.strip_prefix("amplihack ") == Some(MARKER) {
        return true;
    }
    c.strip_suffix(MARKER)
        .and_then(|rest| rest.strip_suffix(' '))
        .and_then(sh_unquote)
        .is_some_and(|path| {
            path.starts_with('/') && Path::new(&path).file_name() == Some(OsStr::new("amplihack"))
        })
}

/// Whether `command` is an entry amplihack wrote: [`is_home_statusline`], or
/// the legacy `statusline.sh` path alone, quoted or not.
pub fn is_amplihack_statusline(command: &str) -> bool {
    is_home_statusline(command) || is_legacy_script(command.trim())
}

/// `word` as `sh` reads it, when it is single-quoted segments joined by
/// `\'` (the form `sh_quote` writes); `None` for anything else.
fn sh_unquote(word: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = word;
    loop {
        rest = rest.strip_prefix('\'')?;
        let end = rest.find('\'')?;
        out.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        if rest.is_empty() {
            return Some(out);
        }
        rest = rest.strip_prefix("\\'")?;
        out.push('\'');
        if rest.is_empty() {
            return Some(out);
        }
    }
}

/// The legacy script path and nothing else: `~`, `$HOME`, `${HOME}` or an
/// absolute path in front, the whole optionally in one pair of quotes.
fn is_legacy_script(c: &str) -> bool {
    let unquoted = ['\'', '"']
        .iter()
        .find_map(|q| c.strip_prefix(*q).and_then(|r| r.strip_suffix(*q)))
        .unwrap_or(c);
    let Some(prefix) = unquoted.strip_suffix(LEGACY_SCRIPT) else {
        return false;
    };
    matches!(prefix, "~" | "$HOME" | "${HOME}")
        || (prefix.starts_with('/')
            && !prefix
                .contains(|ch: char| ch.is_whitespace() || "'\"$`\\;|&<>(){}*?[]!#~".contains(ch)))
}

#[cfg(test)]
#[path = "tests_statusline_entry.rs"]
mod tests;
