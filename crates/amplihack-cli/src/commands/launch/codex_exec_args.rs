//! Issue #2030: find the prompt positional of a non-interactive Codex
//! command line: `codex exec` (and its `review`, `resume` and `fork`
//! subcommands) or the top-level `codex review`.
//!
//! Codex options do not follow one rule. Whether an option takes a value,
//! and how many, depends on the subcommand: `--commit <SHA>` exists only
//! under `exec review`, and `-i/--image` takes every following word under
//! plain `exec` but exactly one under `exec resume` and `exec fork`. So each
//! scope has its own table, copied from codex-cli 0.166.0's help. The tables
//! are in `amplihack-launcher/src/prompt_delivery/codex_options.rs`, the one
//! copy every Codex argument reader in amplihack uses:
//!
//! | Scope | Help text |
//! | --- | --- |
//! | before `exec` or `review` | `fixtures/codex-root-help-0.166.0.txt` |
//! | top-level `review` | `fixtures/codex-review-help-0.166.0.txt` |
//! | `exec` | `fixtures/codex-exec-help-0.166.0.txt` |
//! | `exec review` | `fixtures/codex-exec-review-help-0.166.0.txt` |
//! | `exec resume` | `fixtures/codex-exec-resume-help-0.166.0.txt` |
//! | `exec fork` | `fixtures/codex-exec-fork-help-0.166.0.txt` |
//!
//! `tests_codex_exec_args.rs` parses every fixture and fails when a table
//! and its help text disagree. A new Codex release is added by saving its
//! help next to these and updating the tables until that test passes.
//!
//! The parser follows clap as Codex uses it (checked against 0.166.0):
//! `--opt=value` and `-ovalue` carry their value; a variadic option stops at
//! the next token that starts with `-`, including a lone `-`; an attached
//! value closes the occurrence; after `--` every token is positional.
//!
//! At the top level only the fixed subcommand names `exec`, `e` and `review`
//! are recognised. Every other first positional (a prompt for interactive
//! Codex, `resume`, `fork`, `login`, ...) ends the parse with `Ok(None)`:
//! those start interactive sessions or take no prompt, and are listed as
//! exceptions in `docs/reference/agent-prompt-channels.md`.
//!
//! An option missing from the table, written without `=` and followed by a
//! word, is refused: amplihack cannot tell whether that word is the option's
//! value or the prompt, and guessing wrong either rewrites the caller's
//! argument or leaves the prompt on argv.

use anyhow::{Result, bail};

// The tables live in `amplihack-launcher` so this parser, the launcher's
// own `codex_args` classifier and `codex_agent_step` read one copy.
use Arity::{Flag, Many, One};
pub(crate) use amplihack_launcher::prompt_delivery::codex_options::{
    Arity, EXEC, FORK, Opt, RESUME, REVIEW, ROOT, TOP_REVIEW,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Root,
    Exec,
    Review,
    /// Top-level `codex review`.
    TopReview,
    Resume,
    Fork,
}

impl Scope {
    fn table(self) -> &'static [Opt] {
        match self {
            Scope::Root => ROOT,
            Scope::Exec => EXEC,
            Scope::Review => REVIEW,
            Scope::TopReview => TOP_REVIEW,
            Scope::Resume => RESUME,
            Scope::Fork => FORK,
        }
    }
}

/// A `codex exec` or top-level `codex review` command line, read the way
/// Codex reads it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ExecLine {
    pub(crate) scope: Scope,
    /// Indices of the positionals after the subcommand name.
    pub(crate) positionals: Vec<usize>,
    /// `exec resume --last`.
    pub(crate) last: bool,
    /// The most values one variadic `--image` took.
    pub(crate) widest_image: usize,
}

/// Read `args` (what follows `codex`). `Ok(None)`: neither `codex exec` nor
/// top-level `codex review`, or `codex exec help`, so there is no prompt to
/// move.
///
/// # Errors
///
/// An option is missing its value, or an unknown option is followed by a
/// word (see the module docs). Messages name the option, never a value.
pub(crate) fn parse_exec_line(args: &[String]) -> Result<Option<ExecLine>> {
    let mut line = ExecLine {
        scope: Scope::Root,
        positionals: Vec::new(),
        last: false,
        widest_image: 0,
    };
    let mut after_delimiter = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if after_delimiter || arg == "-" || !arg.starts_with('-') {
            match line.scope {
                // A first root positional other than `exec` or `review` is
                // interactive Codex or another subcommand: a documented
                // exception.
                Scope::Root if after_delimiter => return Ok(None),
                Scope::Root => {
                    line.scope = match arg {
                        "exec" | "e" => Scope::Exec,
                        "review" => Scope::TopReview,
                        _ => return Ok(None),
                    };
                }
                Scope::Exec if !after_delimiter && line.positionals.is_empty() => {
                    line.scope = match arg {
                        "review" => Scope::Review,
                        "resume" => Scope::Resume,
                        "fork" => Scope::Fork,
                        "help" => return Ok(None),
                        _ => {
                            line.positionals.push(i);
                            Scope::Exec
                        }
                    };
                }
                _ => line.positionals.push(i),
            }
            i += 1;
            continue;
        }
        if arg == "--" {
            after_delimiter = true;
            i += 1;
            continue;
        }
        let table = line.scope.table();
        let (arity, attached, name) = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, false),
            };
            let found = table.iter().find(|o| o.0 == name).map(|o| o.2);
            (found, inline, &arg[..2 + name.len()])
        } else {
            // A short cluster: flags, then at most one option that takes a
            // value, which owns the rest of the token when there is any.
            let mut found = Some(Flag);
            let mut attached = false;
            let mut name = arg;
            for (k, c) in arg[1..].char_indices() {
                let opt = table.iter().find(|o| o.1 == Some(c)).map(|o| o.2);
                name = &arg[..1 + k + c.len_utf8()];
                match opt {
                    Some(Flag) => continue,
                    Some(a) => {
                        found = Some(a);
                        attached = 1 + k + c.len_utf8() < arg.len();
                    }
                    None => {
                        found = None;
                        attached = 1 + k + c.len_utf8() < arg.len();
                    }
                }
                break;
            }
            (found, attached, name)
        };
        if line.scope == Scope::Resume && arg == "--last" {
            line.last = true;
        }
        match arity {
            Some(Flag) => {}
            Some(_) if attached => {
                if arity == Some(Many) {
                    line.widest_image = line.widest_image.max(1);
                }
            }
            Some(One) => {
                i += 1;
                if i == args.len() {
                    bail!("Codex option {name} needs a value");
                }
            }
            Some(Many) => {
                let start = i;
                while i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                }
                if i == start {
                    bail!("Codex option {name} needs a value");
                }
                line.widest_image = line.widest_image.max(i - start);
            }
            None => {
                let ambiguous =
                    !attached && args.get(i + 1).is_some_and(|next| !next.starts_with('-'));
                // Before the subcommand, a word after an unknown option
                // matters only when `exec` or `review` could follow it.
                if ambiguous
                    && line.scope == Scope::Root
                    && !args[i + 1..]
                        .iter()
                        .any(|a| matches!(a.as_str(), "exec" | "e" | "review"))
                {
                    return Ok(None);
                }
                if ambiguous {
                    bail!(
                        "amplihack does not know whether Codex option {name} takes a value, so it cannot tell which word is the prompt; write it as {name}=<value>, or pass `-` and send the prompt on stdin"
                    );
                }
            }
        }
        i += 1;
    }
    Ok((line.scope != Scope::Root).then_some(line))
}
