//! Finds the removal commands a command line runs (#3261).
//!
//! The line is split into simple commands with the shared tokeniser
//! ([`shell::tokenise`]), which keeps quoted text inside its word and never
//! reads a heredoc body as commands. A simple command is a removal when the
//! program it runs is `rm` with a recursive flag (a short option cluster
//! holding `r` or `R`, or `--recursive`, before `--`), or `rmdir`. The
//! program is the word [`shell::program`] finds after reserved words,
//! assignments and the [`shell::WRAPPERS`] (`sudo`, `env`, `command`,
//! `timeout` and others), so `git rm -r "$f"` and `echo rm -rf $X` are not
//! removals: `rm` is an argument there, not the program.
//!
//! Two kinds of program run another command from their arguments, and only
//! those are looked into:
//!
//! - a [`LAUNCHERS`] program (`xargs`, `ionice` and others): the first `rm`,
//!   `rmdir`, `eval` or shell word among its arguments is the command it
//!   runs. These programs have options that take values, and the values
//!   cannot be told apart from the command without a table for each one, so
//!   the scan errs towards finding a removal;
//! - `find`: the words after each `-exec`, `-execdir`, `-ok` or `-okdir`, up
//!   to its `;` or `+`, are read as a command of their own.
//!
//! # Text the shell runs as a command
//!
//! The check this module replaced matched the whole string with a regex, so
//! it caught `bash -c 'rm -rf .'`, `eval "rm -rf ."`, `echo $(rm -rf .)` and
//! `cat <<EOF` with `$(rm -rf .)` in the body. Deciding on simple commands
//! alone would let all four through, because the removal is inside a quoted
//! word or a heredoc body. To keep refusing them, the text the shell itself
//! runs as a command is tokenised again, up to [`MAX_DEPTH`]:
//!
//! - the body of each `$(…)` or backtick substitution outside single quotes,
//!   `$'…'`, comments and heredoc bodies ([`shell::substitution_bodies`]);
//! - the `-c` operand of `sh`, `bash`, `zsh`, `dash` and `ksh`, or, with no
//!   `-c`, the heredoc bodies such a shell reads as its script
//!   ([`shell::shell_scripts`], shared with the tail-follow guard);
//! - the arguments of `eval`, joined by spaces;
//! - the substitutions inside a heredoc whose delimiter is unquoted
//!   ([`shell::heredoc_substitution_bodies`], the same scanner as
//!   [`shell::substitution_bodies`] under bash's heredoc quoting rules).
//!
//! No new parser is added for this: each piece of text is read again with
//! the same tokeniser and the same scanners the other command guards use.
//!
//! When the text cannot be read with certainty (an incomplete parse, input
//! over [`MAX_INPUT`], nesting at [`MAX_DEPTH`] or an exhausted
//! [`MAX_REPARSES`] budget), [`removal_commands`] returns `None` and the
//! caller decides on the whole text, as the check did before. The limits
//! bound the work on hostile input; reaching one never allows a command the
//! old check refused.

use super::super::shell::{self, SHELLS, SimpleCommand, basename};

/// Text at this depth or deeper is not read; the top level is depth 0.
const MAX_DEPTH: usize = 4;

/// Larger input is decided on the whole text.
const MAX_INPUT: usize = 64 * 1024;

/// Most re-parses of embedded scripts for one command line.
const MAX_REPARSES: usize = 256;

/// Programs that run a command given in their arguments, after options whose
/// values this module does not tell apart from the command. [`shell::WRAPPERS`]
/// covers the ones whose options are known, such as `sudo` and `env`.
const LAUNCHERS: &[&str] = &[
    "xargs", "ionice", "setsid", "stdbuf", "chrt", "taskset", "flock", "doas", "parallel",
];

/// `find` actions whose following words, up to `;` or `+`, are a command.
const FIND_ACTIONS: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];

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
            let Some(bodies) = shell::heredoc_substitution_bodies(body) else {
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
    for run in commands_run(&command.words) {
        let name = basename(&run[0]);
        let rest = &run[1..];
        let read = match name {
            "eval" => reparse(&rest.join(" "), depth, budget, found),
            "rm" | "rmdir" => {
                if name == "rmdir" || has_recursive_flag(rest) {
                    found.push(Removal {
                        words: command
                            .words
                            .iter()
                            .chain(&command.redirects)
                            .cloned()
                            .collect(),
                        argv: run.to_vec(),
                    });
                }
                true
            }
            _ => shell::shell_scripts(rest, &command.heredocs)
                .into_iter()
                .all(|script| reparse(script, depth, budget, found)),
        };
        if !read {
            return false;
        }
    }
    true
}

/// Whether a program named `name` is one this module looks at: a removal,
/// `eval` or a shell whose script it reads.
fn is_target(name: &str) -> bool {
    matches!(name, "rm" | "rmdir" | "eval") || SHELLS.contains(&name)
}

/// The commands `words` runs whose program [`is_target`], each from its
/// program word onward: the command's own program, the command a
/// [`LAUNCHERS`] program runs, and the commands of `find`'s
/// [`FIND_ACTIONS`]. An argument named `rm` of any other program, as in
/// `git rm -r` or `echo rm -rf`, is not a command.
fn commands_run(words: &[String]) -> Vec<&[String]> {
    let Some(run) = shell::program(words) else {
        return Vec::new();
    };
    let Some(first) = run.first() else {
        return Vec::new();
    };
    let name = basename(first);
    if is_target(name) {
        return vec![run];
    }
    if LAUNCHERS.contains(&name) {
        return run
            .iter()
            .skip(1)
            .position(|w| is_target(basename(w)))
            .map(|at| vec![&run[at + 1..]])
            .unwrap_or_default();
    }
    if name != "find" {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < run.len() {
        if !FIND_ACTIONS.contains(&run[i].as_str()) {
            i += 1;
            continue;
        }
        let tail = &run[i + 1..];
        let end = tail
            .iter()
            .position(|w| w == ";" || w == "+")
            .unwrap_or(tail.len());
        out.extend(commands_run(&tail[..end]));
        i += 1 + end;
    }
    out
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
