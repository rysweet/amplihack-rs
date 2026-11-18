//! Issue #1619: putting the tier's reasoning effort on Codex's own command
//! line.
//!
//! This is the fix for the defect reported in the issue's 3 November comment.
//! An outer Codex session's `-c model_reasoning_effort=…` is a setting of
//! that session and is not inherited, and a recipe's
//! `-c reasoning_effort=…` is a context variable the recipe runner never
//! passes on. The setting has to be on the child's own command line or it
//! does not exist.
//!
//! The pair is prepended, so `exec` stays the first positional argument and
//! the agent-step sandbox splice still lands after it: the option tables give
//! `-c` an arity of one, so every reader that walks argv steps over the pair
//! as an option and its value.
//!
//! Pure: it reads only the argv tail, like `codex_agent_step`.

use super::argv_scan::caller_set_config_key;
use super::table::Effort;

/// Codex's own name for the setting.
pub(crate) const KEY: &str = "model_reasoning_effort";

/// What the injection did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Injection {
    /// The command line with the pair prepended.
    Added(Vec<String>),
    /// The command line already sets the effort; amplihack adds nothing.
    CallerChose,
}

/// Add `-c model_reasoning_effort=<effort>` to `args`.
///
/// `effort` is an [`Effort`], not a string, so only one of the five
/// allowlisted words can ever become the right-hand side of a `-c` token.
/// That is argv safety expressed in the type: a word outside the list is
/// refused at the table, with a `BadEffort` notice naming the file the
/// operator wrote it in, and never reaches this function. Whether the
/// installed Codex accepts the word it is handed is Codex's decision, and it
/// says so in its own output.
pub(crate) fn inject(args: &[String], effort: Effort) -> Injection {
    if caller_set_config_key(args, KEY) {
        return Injection::CallerChose;
    }
    let mut out = vec!["-c".to_string(), format!("{KEY}={}", effort.as_str())];
    out.extend_from_slice(args);
    Injection::Added(out)
}
