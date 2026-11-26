//! Finds the removal commands in a command line (#3261).
//!
//! The line is split into simple commands with the shared tokeniser
//! ([`shell::tokenise`]), which keeps quoted text inside its word and never
//! reads a heredoc body as commands. A simple command is a removal when one
//! of its words is `rm` followed by a recursive flag (a short option cluster
//! with `r` or `R` in it, or `--recursive`, before `--`), or is `rmdir`. The
//! word may be in any position, because `sudo`, `xargs`, `find -exec` and
//! many other programs run their arguments, and this module does not try to
//! tell which programs do. `git rm -r "$f"` is therefore read as a removal,
//! as it was before #3261. A quoted string is one word, so `echo 'rm -rf .'`
//! has no `rm` word and is not a removal.
//!
//! The body of each `$(…)` or backtick substitution the shell runs (outside
//! single quotes, comments and heredoc bodies) is decided in the same way.
//! This keeps the common `--body "$(cat <<'EOF' … EOF)"` form readable: its
//! heredoc body is text, not commands.
//!
//! # Whole-text reading
//!
//! Some text is run as commands in a way the tokeniser does not follow. For
//! that text the caller keeps the decision the check made before #3261, on
//! the whole string ([`Reading::WholeText`]):
//!
//! - the tokeniser cannot read the line (an unclosed quote or substitution,
//!   or an unterminated heredoc);
//! - a word is `eval`, or a shell in [`SHELLS`], which may run a `-c`
//!   argument, a heredoc or its standard input as a script;
//! - a heredoc with an unquoted delimiter has `$(` or a backtick in its body,
//!   which the shell runs before the program reads the body;
//! - substitutions are nested more than [`MAX_DEPTH`] deep.

use super::super::shell::{self, SimpleCommand, basename};
use super::super::tail_follow_guard::substitution_bodies;

/// Deepest substitution nesting read command by command.
const MAX_DEPTH: usize = 8;

/// Programs that read a script from an argument, a heredoc or standard input.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];

/// One removal command.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Removal {
    /// Every word and redirection target of the simple command, read for
    /// shell expansion.
    pub(super) words: Vec<String>,
    /// The `rm` or `rmdir` word and the words after it, read for the paths
    /// to be removed.
    pub(super) argv: Vec<String>,
}

/// How the caller is to decide on a command line.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reading {
    /// Decide on each of these removals by its own words.
    Commands(Vec<Removal>),
    /// Decide on the whole text, for one of the reasons in the module notes.
    WholeText,
}

/// Why `text` has to be read whole.
#[derive(Debug)]
struct ReadWhole;

/// How to decide on `text`.
pub(super) fn read(text: &str) -> Reading {
    let mut found = Vec::new();
    match collect(text, 0, &mut found) {
        Ok(()) => Reading::Commands(found),
        Err(ReadWhole) => Reading::WholeText,
    }
}

/// Adds the removals in `text` to `found`, or reports that the text has to
/// be read whole.
fn collect(text: &str, depth: usize, found: &mut Vec<Removal>) -> Result<(), ReadWhole> {
    // Substitutions nested too deeply to read.
    if depth > MAX_DEPTH {
        return Err(ReadWhole);
    }
    // An unclosed quote or substitution, or a heredoc with no closing
    // delimiter line.
    let parsed = shell::tokenise(text);
    if !parsed.complete {
        return Err(ReadWhole);
    }
    for command in &parsed.commands {
        if command
            .words
            .iter()
            .map(|w| basename(w))
            .any(|name| name == "eval" || SHELLS.contains(&name))
        {
            // `eval` or a shell may run quoted text or a heredoc as commands.
            return Err(ReadWhole);
        }
        // The shell runs a substitution in the body of a heredoc whose
        // delimiter is unquoted before the program reads the body.
        let substituted_heredoc = command
            .heredocs
            .iter()
            .zip(&command.heredoc_expands)
            .any(|(body, expands)| *expands && (body.contains("$(") || body.contains('`')));
        if substituted_heredoc {
            return Err(ReadWhole);
        }
        if let Some(removal) = removal(command) {
            found.push(removal);
        }
    }
    for body in substitution_bodies(text) {
        collect(&body, depth + 1, found)?;
    }
    Ok(())
}

/// The removal in `command`: from the first `rm` word with a recursive flag
/// after it, or the first `rmdir` word.
fn removal(command: &SimpleCommand) -> Option<Removal> {
    let words = &command.words;
    let at = (0..words.len()).find(|&i| match basename(&words[i]) {
        "rmdir" => true,
        "rm" => has_recursive_flag(&words[i + 1..]),
        _ => false,
    })?;
    Some(Removal {
        words: words.iter().chain(&command.redirects).cloned().collect(),
        argv: words[at..].to_vec(),
    })
}

/// Whether `rm`'s arguments ask for a recursive removal. Options are read
/// anywhere among the operands, as GNU `rm` reads them, until `--`.
fn has_recursive_flag(args: &[String]) -> bool {
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--recursive" {
            return true;
        }
        if arg.starts_with("--") {
            continue;
        }
        if let Some(cluster) = arg.strip_prefix('-')
            && cluster.contains(['r', 'R'])
        {
            return true;
        }
    }
    false
}
