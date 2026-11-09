//! Issue #2030: rewrite the arguments `amplihack codex` and `amplihack copilot`
//! pass through to the agent CLI, so a caller that puts prompt text on the
//! wrapper's argv does not put it on the agent's argv. The Claude rewrite is
//! in [`super::passthrough_rewrite`].
//!
//! ## Codex
//!
//! | Caller passes | Child receives |
//! | --- | --- |
//! | `exec <text>` (also `e`, `exec review <text>`) | `exec -`, text on stdin |
//! | `exec resume <session> <text>`, `exec resume --last <text>` | the same with `-`, text on stdin |
//! | `exec fork <session> <text>` | `exec fork <session> -`, text on stdin |
//! | `exec -`, or `exec` with no prompt | unchanged |
//! | `exec` with two positional prompts | error; Codex accepts one |
//! | `exec help ...` | unchanged |
//! | interactive `codex <text>`, any other subcommand | unchanged (documented exception) |
//!
//! The `-` takes the place of the text, so the argument order is otherwise
//! unchanged. Codex reads `-` as "read the prompt from stdin" for `exec`,
//! `exec resume`, `exec fork` and `exec review` (codex-cli 0.166.0 help).
//!
//! Interactive Codex is left alone: the first positional of a bare `codex`
//! is either a prompt or one of Codex's subcommands, and that list changes
//! between releases, so reading it as a prompt could break a subcommand.
//!
//! ## Copilot
//!
//! | Caller passes | Child receives |
//! | --- | --- |
//! | `-p <text>`, `--prompt <text>`, `--prompt=<text>`, `-p<text>` | `-p`/`--prompt` and the pointer sentence |
//! | `-i <text>`, `--interactive <text>`, `--interactive=<text>`, `-i<text>` | `-i`/`--interactive` and the pointer sentence |
//! | a value that is already a pointer sentence | unchanged |
//! | the same flag twice | error |
//!
//! Copilot reads no prompt from stdin that amplihack can rely on, so the text
//! goes to a 0600 file in the per-session 0700 directory and Copilot gets
//! [`FILE_REFERENCE_TEMPLATE`] naming it. `--add-dir <that directory>` is
//! added in front, except under the external LiteLLM gateway, whose launch
//! policy refuses `--add-dir`.

use super::passthrough_rewrite::{Rewritten, write_private_file};
use amplihack_launcher::prompt_delivery::{LONG_OPTIONS_WITH_VALUE, SHORT_OPTIONS_WITH_VALUE};
use amplihack_utils::prompt_delivery::{FILE_REFERENCE_PREFIX, FILE_REFERENCE_TEMPLATE};
use anyhow::{Result, bail};
use tempfile::TempDir;

/// Indices of the positional arguments of a Codex argv, skipping option
/// values with the launcher's arity tables. `--image`/`-i` takes every
/// following non-option token, as clap does, except under `exec resume`,
/// where it takes one.
fn codex_positionals(args: &[String]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut after_delimiter = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if after_delimiter || arg == "-" || !arg.starts_with('-') {
            out.push(i);
        } else if arg == "--" {
            after_delimiter = true;
        } else if arg == "--image" || arg == "-i" {
            let exec_resume = matches!(
                out.as_slice(),
                [cmd, sub, ..] if matches!(args[*cmd].as_str(), "exec" | "e") && args[*sub] == "resume"
            );
            i += 1;
            if !exec_resume {
                while i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                }
            }
        } else if arg.starts_with("--") {
            if !arg.contains('=') && LONG_OPTIONS_WITH_VALUE.contains(&arg) {
                i += 1;
            }
        } else if arg.len() == 2 && SHORT_OPTIONS_WITH_VALUE.contains(&arg) {
            i += 1;
        }
        i += 1;
    }
    out
}

/// Rewrite `amplihack codex` passthrough arguments; see the module docs.
///
/// # Errors
///
/// `exec` was given more than one positional prompt. The message names no
/// argument text.
pub(crate) fn rewrite_codex_passthrough(args: &[String]) -> Result<Rewritten> {
    let unchanged = || Ok(Rewritten::unchanged(args.to_vec()));
    let positions = codex_positionals(args);
    let Some((&command, rest)) = positions.split_first() else {
        return unchanged();
    };
    if !matches!(args[command].as_str(), "exec" | "e") {
        return unchanged();
    }
    let last = args
        .iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == "--last");
    let prompts: &[usize] = match rest.first().map(|&i| args[i].as_str()) {
        Some("help") => return unchanged(),
        Some("review") => &rest[1..],
        Some("resume") if last => &rest[1..],
        Some("resume") | Some("fork") => rest.get(2..).unwrap_or(&[]),
        _ => rest,
    };
    match prompts {
        [] => unchanged(),
        [only] if args[*only] == "-" => unchanged(),
        [only] => {
            let mut out = args.to_vec();
            let text = std::mem::replace(&mut out[*only], "-".to_string());
            Ok(Rewritten::from_parts(out, Some(text), None))
        }
        _ => bail!(
            "codex exec was given more than one positional prompt; pass one, or pass `-` and send the prompt on stdin"
        ),
    }
}

/// Which Copilot prompt flag a token is, with its value when attached.
fn copilot_prompt_flag(arg: &str) -> Option<(&'static str, Option<&str>)> {
    for (short, long) in [("-p", "--prompt"), ("-i", "--interactive")] {
        if arg == short || arg == long {
            return Some((if arg == short { short } else { long }, None));
        }
        if let Some(value) = arg.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
            return Some((long, Some(value)));
        }
        // Commander reads `-p<text>` as `-p` with the value `<text>`.
        if let Some(value) = arg.strip_prefix(short).filter(|v| !v.is_empty())
            && !arg.starts_with("--")
        {
            return Some((short, Some(value)));
        }
    }
    None
}

/// Rewrite `amplihack copilot` passthrough arguments; see the module docs.
///
/// `grant_dir` adds `--add-dir <per-session directory>` when a pointer file
/// was written. Pass `false` under the external LiteLLM gateway.
///
/// # Errors
///
/// - the same prompt flag was given twice;
/// - a private file could not be written, or its path holds a newline.
pub(crate) fn rewrite_copilot_passthrough(args: &[String], grant_dir: bool) -> Result<Rewritten> {
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 2);
    let mut dir: Option<TempDir> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            out.extend(args[i..].iter().cloned());
            break;
        }
        let Some((flag, inline)) = copilot_prompt_flag(arg) else {
            out.push(arg.to_string());
            i += 1;
            continue;
        };
        let (text, consumed) = match inline {
            Some(text) => (text, 1),
            None => match args.get(i + 1) {
                Some(next) => (next.as_str(), 2),
                // No value: Copilot reports the error itself.
                None => {
                    out.push(arg.to_string());
                    break;
                }
            },
        };
        if text.starts_with(FILE_REFERENCE_PREFIX) {
            out.extend(args[i..i + consumed].iter().cloned());
        } else {
            // One file per flag kind: `-p` and `--prompt` share a name, so a
            // second prompt fails with "given twice" from `create_new`.
            let name = if matches!(flag, "-p" | "--prompt") {
                "copilot-prompt.md"
            } else {
                "copilot-interactive.md"
            };
            let path = write_private_file(&mut dir, name, flag, text)?;
            let path_text = path.to_string_lossy();
            if path_text.contains('\n') || path_text.contains('\0') {
                bail!("the private file path for {flag} contains a newline");
            }
            out.push(flag.to_string());
            out.push(FILE_REFERENCE_TEMPLATE.replace("{path}", &path_text));
        }
        i += consumed;
    }
    if grant_dir && let Some(dir) = dir.as_ref() {
        out.splice(
            0..0,
            [
                "--add-dir".to_string(),
                dir.path().to_string_lossy().into_owned(),
            ],
        );
    }
    Ok(Rewritten::from_parts(out, None, dir))
}
