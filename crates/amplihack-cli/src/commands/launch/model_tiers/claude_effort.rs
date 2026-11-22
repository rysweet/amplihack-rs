//! Issue #2916: putting the tier's reasoning effort on Claude Code's own
//! command line.
//!
//! Claude Code 2.1.329 takes `--effort <level>`, so a recipe step on the
//! Claude harness can carry the tier's effort the way a Codex step already
//! carries it through `-c model_reasoning_effort=…`. Before this, a tier's
//! effort in the Claude column was recorded and never written, and the
//! owner's guidance that planning and design run at `xhigh` had no way to
//! reach a step.
//!
//! The pair is prepended rather than appended. Appended it would land after a
//! positional prompt, and `--effort` takes its value as the next token, so
//! the question of what that value is would depend on where the prompt sits.
//! Prepending keeps the pair inside amplihack's own option region, where
//! `model.force` already inserts, and every reader that walks argv steps over
//! an option and its value.
//!
//! The caller-set scan here is deliberately not arity-aware, and
//! `codex_options` must not be imported: walking a Claude command line with
//! Codex's table reads `-p` (Claude's `--print`, a flag) as Codex's
//! `--profile` and skips the token after it, which is the shape the recipe
//! runner builds. See `argv_scan::caller_set_flag`.
//!
//! An installed Claude Code too old to know the option would refuse the whole
//! launch, and the issue asks that the step then run without it. Once the
//! binary is resolved, the launch reads that binary's `--help` once and, when
//! the text does not list `--effort`, [`withdraw`] takes the pair back off and
//! a line says so. That is a fall-back, never a gate: a help text that could
//! not be read leaves the flag in place, and nothing here ends a launch.
//!
//! Pure: it reads only the argv tail and the help text it is handed.

use super::argv_scan::{Injection, caller_set_flag};
use super::table::Effort;

/// Claude Code's own name for the option.
pub(crate) const FLAG: &str = "--effort";

/// Add `--effort <effort>` to `args`.
///
/// `effort` is an [`Effort`], not a string, so only a compiled-in word can
/// become the value token. That is argv safety expressed in the type: a word
/// outside the Claude column's vocabulary is refused at the table, with a
/// `BadEffort` notice naming the file the operator wrote it in, and never
/// reaches this function. Whether the installed Claude Code accepts the word
/// it is handed is Claude's decision, and it says so in its own output.
pub(crate) fn inject(args: &[String], effort: Effort) -> Injection {
    if caller_set_flag(args, FLAG) {
        return Injection::CallerChose;
    }
    // Prepending shifts `extra_args.first()`, which `context.rs` reads to
    // recognise a non-session invocation such as `--version`. It cannot bite:
    // amplihack prepends only where a tier word was on the command line, and
    // `--version` and the help flags carry none. `model.force` already
    // inserts at this same position.
    let mut out = vec![FLAG.to_string(), effort.as_str().to_string()];
    out.extend_from_slice(args);
    Injection::Added(out)
}

/// Whether a Claude Code `--help` text lists the option.
pub(crate) fn help_lists_flag(help: &str) -> bool {
    help.split_whitespace()
        .map(|word| word.trim_end_matches(','))
        .any(|word| word == FLAG || word.starts_with("--effort="))
}

/// Take the `--effort <level>` pair [`inject`] added back off `args`.
///
/// Called only for a launch where amplihack added the pair, which it does
/// only when the command line carried none, so the first `--effort` is
/// amplihack's own.
pub(crate) fn withdraw(args: Vec<String>) -> Vec<String> {
    let Some(at) = args.iter().position(|a| a == FLAG) else {
        return args;
    };
    args.into_iter()
        .enumerate()
        .filter(|(i, _)| *i != at && *i != at + 1)
        .map(|(_, a)| a)
        .collect()
}
