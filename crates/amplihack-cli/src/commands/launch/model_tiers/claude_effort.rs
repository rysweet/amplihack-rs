//! Issue #2916: putting the tier's reasoning effort on Claude Code's own
//! command line.
//!
//! Claude Code 2.1.330 takes `--effort <level>`, so a recipe step on the
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
//! There is no version probe and no gate. `commands/agent/launch.rs` already
//! appends `--effort` to a Claude command line with no version check on that
//! path, so this adds no new class of failure; if an installed Claude is old
//! enough not to know the option, the refusal is Claude's own, in Claude's
//! own output, and an instance clears the cell's `effort` in its own
//! `~/.amplihack/model-tiers.toml`.
//!
//! Pure: it reads only the argv tail, like `codex_effort`.

use super::argv_scan::caller_set_flag;
use super::table::Effort;

/// Claude Code's own name for the option.
pub(crate) const FLAG: &str = "--effort";

/// What the injection did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Injection {
    /// The command line with the pair prepended.
    Added(Vec<String>),
    /// The command line already sets the effort; amplihack adds nothing.
    CallerChose,
}

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
