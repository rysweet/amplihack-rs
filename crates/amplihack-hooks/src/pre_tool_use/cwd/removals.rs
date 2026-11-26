//! Finds the removal commands a command line runs (#3261).
//!
//! The line is split into simple commands with the shared tokeniser
//! ([`shell::tokenise`]), which keeps quoted text inside its word and never
//! reads a heredoc body as commands. A simple command is a removal when it
//! has a word whose basename is `rm` with a recursive flag (a short option
//! cluster holding `r` or `R`, or `--recursive`, before `--`), or `rmdir`.
//! Looking for that word anywhere in the command, not only at its program
//! position, finds it after `sudo`, `xargs`, `ionice`, `find -exec` and other
//! launchers.
//!
//! Text the shell runs as a command is read again, up to [`MAX_DEPTH`]:
//!
//! - the body of each `$(…)` or backtick substitution outside single quotes,
//!   `$'…'`, comments and heredoc bodies;
//! - the `-c` operand of `sh`, `bash`, `zsh`, `dash` and `ksh`, or, with no
//!   `-c`, the heredoc bodies such a shell reads as its script;
//! - the arguments of `eval`, joined by spaces;
//! - the substitutions inside a heredoc whose delimiter is unquoted, which
//!   the shell expands before the program reads the body.
//!
//! When the text cannot be read with certainty (an incomplete parse, input
//! over [`MAX_INPUT`], nesting at [`MAX_DEPTH`] or an exhausted
//! [`MAX_REPARSES`] budget), [`removal_commands`] returns `None` and the
//! caller decides on the whole text, as the check did before.

use super::super::shell::{self, SHELLS, SimpleCommand, basename};

/// Text at this depth or deeper is not read; the top level is depth 0.
const MAX_DEPTH: usize = 4;

/// Larger input is decided on the whole text.
const MAX_INPUT: usize = 64 * 1024;

/// Most re-parses of embedded scripts for one command line.
const MAX_REPARSES: usize = 256;

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

/// The removal commands `text` runs, or `None` when the text cannot be read
/// with certainty.
pub(super) fn removal_commands(text: &str) -> Option<Vec<Removal>> {
    if text.len() > MAX_INPUT {
        return None;
    }
    let mut budget = MAX_REPARSES;
    let mut found = Vec::new();
    collect(text, 0, &mut budget, &mut found).then_some(found)
}

/// Adds the removals in `text` to `found`; false when the text cannot be
/// read with certainty.
fn collect(text: &str, depth: usize, budget: &mut usize, found: &mut Vec<Removal>) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    let parsed = shell::tokenise(text);
    if !parsed.complete {
        return false;
    }
    for command in &parsed.commands {
        if !collect_command(command, depth, budget, found) {
            return false;
        }
        let expanded = command
            .heredocs
            .iter()
            .zip(&command.heredoc_expands)
            .filter(|(_, expands)| **expands);
        for (body, _) in expanded {
            let Some(bodies) = heredoc_substitutions(body) else {
                return false;
            };
            if !bodies.iter().all(|b| reparse(b, depth, budget, found)) {
                return false;
            }
        }
    }
    shell::substitution_bodies(text)
        .iter()
        .all(|body| reparse(body, depth, budget, found))
}

/// Reads `script` one level deeper, spending one unit of `budget`.
fn reparse(script: &str, depth: usize, budget: &mut usize, found: &mut Vec<Removal>) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    collect(script, depth + 1, budget, found)
}

fn collect_command(
    command: &SimpleCommand,
    depth: usize,
    budget: &mut usize,
    found: &mut Vec<Removal>,
) -> bool {
    let words = &command.words;
    let Some(at) = words.iter().position(|w| {
        let name = basename(w);
        matches!(name, "rm" | "rmdir" | "eval") || SHELLS.contains(&name)
    }) else {
        return true;
    };
    let name = basename(&words[at]);
    let rest = &words[at + 1..];
    match name {
        "eval" => reparse(&rest.join(" "), depth, budget, found),
        "rm" | "rmdir" => {
            let mut argv = &words[at..];
            let is_find = shell::program(words)
                .and_then(|p| p.first())
                .is_some_and(|p| basename(p) == "find");
            if is_find && let Some(end) = argv.iter().position(|w| w == ";" || w == "+") {
                argv = &argv[..end];
            }
            if name == "rm" && !has_recursive_flag(&argv[1..]) {
                return true;
            }
            found.push(Removal {
                words: words.iter().chain(&command.redirects).cloned().collect(),
                argv: argv.to_vec(),
            });
            true
        }
        _ => shell::shell_scripts(rest, &command.heredocs)
            .into_iter()
            .all(|script| reparse(script, depth, budget, found)),
    }
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

/// The bodies of the `$(…)` and backtick substitutions in the body of a
/// heredoc whose delimiter is unquoted, or `None` when one is unclosed.
/// Quotes in such a body are ordinary characters; a backslash escapes the
/// next character.
fn heredoc_substitutions(body: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = body.chars().collect();
    let mut bodies = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '$' if chars.get(i + 1) == Some(&'(') => {
                let end = shell::subst_close(&chars, i)?;
                bodies.push(chars[i + 2..end].iter().collect());
                i = end;
            }
            '`' => {
                let mut end = i + 1;
                while end < chars.len() && chars[end] != '`' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                if end >= chars.len() {
                    return None;
                }
                bodies.push(chars[i + 1..end].iter().collect());
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    Some(bodies)
}
