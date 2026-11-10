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
//! | top-level `review <text>`, `review --base main <text>` | `review -`, `review --base main -`, text on stdin |
//! | `exec resume <session> <text>`, `exec resume --last <text>` | the same with `-`, text on stdin |
//! | `exec fork <session> <text>` | `exec fork <session> -`, text on stdin |
//! | `exec -`, or `exec` with no prompt | unchanged |
//! | `exec` with two positional prompts | error; Codex accepts one |
//! | `exec --image a b` (several values in one `--image`) and no prompt | error; one value may be the prompt |
//! | `exec` with an option amplihack does not know, followed by a word | error; that word may be the prompt |
//! | `exec help ...` | unchanged |
//! | interactive `codex <text>`, top-level `resume <session> <text>`, top-level `fork <session> <text>`, any other subcommand | unchanged (documented exception) |
//!
//! Option values are never read as the prompt: `exec review --commit <sha>`
//! is unchanged, and `exec review --base main <text>` becomes
//! `exec review --base main -`. Which options take a value differs by
//! subcommand, so [`super::codex_exec_args`] keeps one table per subcommand,
//! checked against the saved Codex help text.
//!
//! The `-` takes the place of the text, so the argument order is otherwise
//! unchanged. Codex reads `-` as "read the prompt from stdin" for `exec`,
//! `exec resume`, `exec fork`, `exec review` and top-level `review`
//! (codex-cli 0.166.0 help).
//!
//! Interactive Codex is left alone: the first positional of a bare `codex`
//! is either a prompt or one of Codex's subcommands, and that list changes
//! between releases, so reading it as a prompt could break a subcommand.
//! Top-level `codex resume` and `codex fork` start interactive sessions with
//! a terminal UI that owns stdin, so they have no stdin prompt channel and
//! keep their prompt on argv as well.
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
//! policy refuses `--add-dir`. There Copilot can open the file only with
//! `--allow-all-paths` (also implied by `--allow-all` and `--yolo`), which
//! amplihack adds through `--allow-all` unless the caller passed an
//! `--allow-all-*` flag or set `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1`. A `-p`
//! launch under the gateway without that access is refused before anything
//! is written: it would run with a prompt Copilot cannot open.

use super::codex_exec_args::{Scope, parse_exec_line};
use super::passthrough_rewrite::{Rewritten, write_private_file};
use amplihack_utils::prompt_delivery::{FILE_REFERENCE_PREFIX, FILE_REFERENCE_TEMPLATE};
use anyhow::{Result, bail};
use tempfile::TempDir;

/// Rewrite `amplihack codex` passthrough arguments; see the module docs.
///
/// # Errors
///
/// - `exec` or top-level `review` was given more than one positional prompt;
/// - a variadic `exec --image` took more than one value and no prompt was
///   found, so one of those values may be the prompt;
/// - [`parse_exec_line`] refused the command line.
///
/// No message names argument text.
pub(crate) fn rewrite_codex_passthrough(args: &[String]) -> Result<Rewritten> {
    let unchanged = || Ok(Rewritten::unchanged(args.to_vec()));
    let Some(line) = parse_exec_line(args)? else {
        return unchanged();
    };
    let prompts: &[usize] = match line.scope {
        Scope::Resume if !line.last => line.positionals.get(1..).unwrap_or(&[]),
        Scope::Fork => line.positionals.get(1..).unwrap_or(&[]),
        _ => &line.positionals,
    };
    match prompts {
        [] if line.widest_image > 1 => bail!(
            "codex exec --image took {} values and no prompt was found, so one of them may be the prompt; give each image its own -i, or put `--` before the prompt",
            line.widest_image
        ),
        [] => unchanged(),
        [only] if args[*only] == "-" => unchanged(),
        [only] => {
            let mut out = args.to_vec();
            let text = std::mem::replace(&mut out[*only], "-".to_string());
            Ok(Rewritten::from_parts(out, Some(text), None))
        }
        _ => bail!(
            "codex exec or codex review was given more than one positional prompt; pass one, or pass `-` and send the prompt on stdin"
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
/// `path_access`: Copilot will run with `--allow-all-paths` (or
/// `--allow-all`/`--yolo`, given by the caller or added by amplihack). Without
/// `--add-dir` it is the only way a `-p` run can open the pointer file, which
/// is outside the workspace.
///
/// # Errors
///
/// - `-p`/`--prompt` text with neither `grant_dir` nor `path_access`: Copilot
///   could not open the file and no user is there to approve the read;
/// - the same prompt flag was given twice;
/// - a private file could not be written, or its path holds a newline.
pub(crate) fn rewrite_copilot_passthrough(
    args: &[String],
    grant_dir: bool,
    path_access: bool,
) -> Result<Rewritten> {
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
            let prompt_mode = matches!(flag, "-p" | "--prompt");
            // `-i` is interactive: Copilot asks the user before reading the
            // file. A `-p` run has no one to ask.
            if prompt_mode && !grant_dir && !path_access {
                bail!(
                    "amplihack copilot {flag}: under the external LiteLLM gateway Copilot gets no --add-dir, so it can read the private prompt file only with path access, and this launch has none (an --allow-all-* flag without --allow-all-paths, or AMPLIHACK_COPILOT_NO_ALLOW_ALL=1); add --allow-all-paths"
                );
            }
            let name = if prompt_mode {
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
