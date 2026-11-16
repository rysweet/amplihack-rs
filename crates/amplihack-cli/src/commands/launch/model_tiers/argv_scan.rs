//! Issue #1619: reading `--model` and `-c key=value` out of a command line
//! without mistaking an option's value for a flag.
//!
//! Resolving a tier means finding a `--model <word>` pair the recipe runner
//! wrote and replacing or deleting it. A scanner that does not walk arity
//! would read the value of `--output-last-message --model` as a flag and
//! delete the following argument, silently changing where Codex writes its
//! result. Whether a Codex option takes a value comes from the one option
//! table in `amplihack-launcher`, the same table the prompt rewrite and the
//! agent-step injection read.
//!
//! Arity walking is applied to Codex only. Claude Code and Copilot CLI have
//! their own, different tables, and amplihack has none for them: walking a
//! Claude command line with Codex's table would read `-p` (Claude's
//! `--print`, a flag) as Codex's `--profile` and skip the `--model` that
//! follows it, which is exactly the shape the recipe runner builds. For those
//! harnesses `--model` is recognised wherever it appears before `--`, which
//! is how `model_args` has always detected an explicit model.
//!
//! Pure: it reads the argv and a compiled-in table, nothing else.

use amplihack_launcher::prompt_delivery::codex_options;

/// One `--model` on a command line, in either spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelOccurrence {
    /// Index of `--model` or `--model=<value>`.
    pub(crate) flag_index: usize,
    /// Index of the separate value token, or `None` for `--model=<value>`.
    pub(crate) value_index: Option<usize>,
    /// The value as written.
    pub(crate) value: String,
}

impl ModelOccurrence {
    /// Whether the value was attached with `=`.
    pub(crate) fn attached(&self) -> bool {
        self.value_index.is_none()
    }
}

/// Where amplihack's own region of the command line ends: the index of the
/// first bare `--`, or `args.len()` when there is none.
///
/// Every scan here stops at that index, and so must every insertion. The two
/// are the same function so they cannot drift: everything past `--` is the
/// caller's, forwarded verbatim (D9), so a pair written there reaches the
/// harness as positional text while amplihack's line claims it was applied.
pub(crate) fn options_end(args: &[String]) -> usize {
    args.iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())
}

/// Every `--model` before the end-of-options marker.
///
/// A `--model` with no value is not an occurrence: amplihack has nothing to
/// replace it with, and Codex's own reader already refuses it by name.
pub(crate) fn find_model_options(args: &[String], arity_aware: bool) -> Vec<ModelOccurrence> {
    let mut out = Vec::new();
    let end = options_end(args);
    let mut i = 0;
    while i < end {
        let arg = args[i].as_str();
        if let Some(value) = arg.strip_prefix("--model=") {
            out.push(ModelOccurrence {
                flag_index: i,
                value_index: None,
                value: value.to_string(),
            });
            i += 1;
            continue;
        }
        if arg == "--model" {
            // `args.get(i + 1)`, not `end`: `--model --` is a `--model` with
            // no value, which is not an occurrence.
            if let Some(value) = args.get(i + 1).filter(|_| i + 1 < end) {
                out.push(ModelOccurrence {
                    flag_index: i,
                    value_index: Some(i + 1),
                    value: value.clone(),
                });
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        i += 1 + usize::from(arity_aware && consumes_next(arg));
    }
    out
}

/// Whether `-c`/`--config` already sets `key` before the end-of-options
/// marker, in any spelling Codex accepts.
pub(crate) fn caller_set_config_key(args: &[String], key: &str) -> bool {
    let end = options_end(args);
    let mut i = 0;
    while i < end {
        let arg = args[i].as_str();
        let value = if let Some((name, attached)) = arg.split_once('=').filter(|_| is_long(arg)) {
            (name == "--config").then_some(attached)
        } else if arg == "-c" || arg == "--config" {
            args.get(i + 1).filter(|_| i + 1 < end).map(String::as_str)
        } else if let Some(rest) = arg.strip_prefix("-c").filter(|_| short_with_value(arg)) {
            Some(rest.strip_prefix('=').unwrap_or(rest))
        } else {
            None
        };
        if let Some(value) = value
            && value
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            return true;
        }
        i += 1 + usize::from(consumes_next(arg));
    }
    false
}

/// `-cvalue` rather than the bare `-c`.
fn short_with_value(arg: &str) -> bool {
    arg.len() > 2 && !arg.starts_with("--")
}

fn is_long(arg: &str) -> bool {
    arg.starts_with("--")
}

/// Whether `arg` is an option that takes its value as the next token, so the
/// walk must step over that token.
///
/// `-` and a bare positional take nothing. `--name=value` and `-nvalue`
/// carry their value inside the token.
fn consumes_next(arg: &str) -> bool {
    if arg == "-" || !arg.starts_with('-') {
        return false;
    }
    if is_long(arg) {
        return !arg.contains('=') && codex_options::takes_value(arg);
    }
    arg.len() == 2 && codex_options::takes_value(arg)
}
