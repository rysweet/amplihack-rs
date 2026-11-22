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
//! launch, and the issue asks that the step then run without it. The launch
//! already holds the binary's `--version` output, read when the binary was
//! resolved, so [`version_accepts_effort`] decides from that and nothing is
//! spawned for it. When the version is older than [`MIN_EFFORT_VERSION`], or
//! cannot be read, [`withdraw`] takes the pair back off and a line says so.
//! That is a fall-back, never a gate: nothing here ends a launch, and the
//! direction of doubt is to withhold the flag, because a step without its
//! effort still runs and a step with an option Claude Code refuses does not.
//!
//! Pure: it reads only the argv tail and the version text it is handed.

use super::argv_scan::{Injection, caller_set_flag};
use super::table::Effort;
use crate::commands::launch::contract_file::parse_claude_version;

/// Claude Code's own name for the option.
pub(crate) const FLAG: &str = "--effort";

/// The oldest Claude Code seen to accept `--effort`.
///
/// 2.1.329 lists `--effort <level> (low, medium, high, xhigh, max)` in its
/// `--help`. The first release that accepted the option is not known, so an
/// install between that release and this one loses only the effort; the step
/// still runs.
pub(crate) const MIN_EFFORT_VERSION: (u32, u32, u32) = (2, 1, 329);

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

/// Whether the Claude Code that reported `version` accepts `--effort`.
///
/// `version` is the binary's `--version` output as amplihack already read it.
/// A missing, unparseable or overflowing version answers `false`, so the flag
/// is withheld whenever the answer is not known.
pub(crate) fn version_accepts_effort(version: Option<&str>) -> bool {
    version
        .and_then(parse_claude_version)
        .is_some_and(|found| found >= MIN_EFFORT_VERSION)
}

/// Take the `--effort <level>` pair [`inject`] added back off `args`.
///
/// [`inject`] prepends the pair, so it is the first two tokens. The removal
/// is by that position, never by searching for the flag: a caller's own text
/// can equal `--effort` (an option's value, or prompt text after `--`), and
/// that text must reach the harness untouched. When the first two tokens are
/// not a pair amplihack could have written, `args` comes back unchanged.
pub(crate) fn withdraw(args: Vec<String>) -> Vec<String> {
    let ours = args.first().is_some_and(|a| a == FLAG)
        && args.get(1).and_then(|value| Effort::parse(value)).is_some();
    if !ours {
        return args;
    }
    args.into_iter().skip(2).collect()
}
