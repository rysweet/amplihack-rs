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
//! | a value that is already a pointer sentence, and no guidance | unchanged |
//! | a `-p` pointer sentence, with guidance (the recipe runner's form, #3162) | `-p`/`--prompt` and a pointer to a private copy of the named file with the block after it |
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
use super::passthrough_rewrite::{PromptBlock, Rewritten, write_private_file};
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

/// The value of the first `-p`/`--prompt` in `args`, before any `--`.
fn copilot_prompt_value(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return None;
        }
        let Some((flag, inline)) = copilot_prompt_flag(arg) else {
            i += 1;
            continue;
        };
        let (text, consumed) = match inline {
            Some(text) => (Some(text), 1),
            None => (args.get(i + 1).map(String::as_str), 2),
        };
        if matches!(flag, "-p" | "--prompt") {
            return text;
        }
        i += consumed;
    }
    None
}

/// Whether `args` give `-p`/`--prompt` prompt text, which
/// [`rewrite_copilot_passthrough`] moves to a private file (#2535).
pub(crate) fn copilot_has_prompt_text(args: &[String]) -> bool {
    copilot_prompt_value(args).is_some_and(|t| !t.starts_with(FILE_REFERENCE_PREFIX))
}

/// Whether `args` give `-p`/`--prompt` a pointer sentence naming a prompt
/// file, the form the recipe runner starts every Copilot step with
/// (issue #3162).
pub(crate) fn copilot_has_prompt_pointer(args: &[String]) -> bool {
    copilot_prompt_value(args).is_some_and(|t| t.starts_with(FILE_REFERENCE_PREFIX))
}

/// The prompt file a pointer sentence names: the text between
/// [`FILE_REFERENCE_PREFIX`] and the rest of [`FILE_REFERENCE_TEMPLATE`].
fn pointer_path(pointer: &str) -> Option<&str> {
    let (_, suffix) = FILE_REFERENCE_TEMPLATE.split_once("{path}")?;
    pointer
        .strip_prefix(FILE_REFERENCE_PREFIX)?
        .strip_suffix(suffix)
        .filter(|path| path.starts_with('/'))
}

/// The prompt a caller's pointer sentence names, read in full. `None` when
/// the sentence names no absolute path, or the path is not a regular file
/// holding UTF-8 text. Copilot would read the same file; reading it here
/// gives Copilot nothing it could not already open.
fn read_pointed_prompt(pointer: &str) -> Option<String> {
    let path = std::path::Path::new(pointer_path(pointer)?);
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    std::fs::read_to_string(path).ok()
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
///
/// `prompt_suffix` (issue #2535) is the operator-instructions block. It is
/// added after one blank line to `-p`/`--prompt` text only, before the
/// private file is written; `-i` text never receives it, and it never
/// reaches argv.
///
/// A caller's own `-p` pointer sentence (issue #3162: the recipe runner
/// starts every Copilot step this way) is passed on unchanged when there is
/// no block. With a block, the file it names is read, and its text with the
/// block after it goes to a private file that a new pointer sentence names,
/// as for `-p` text. The caller's file is never written. When that file
/// cannot be read, or Copilot could open no private file (neither
/// `grant_dir` nor `path_access`), the caller's pointer is passed on
/// unchanged and [`Rewritten::prompt_block`] says the block was not added.
pub(crate) fn rewrite_copilot_passthrough(
    args: &[String],
    grant_dir: bool,
    path_access: bool,
    prompt_suffix: Option<&str>,
) -> Result<Rewritten> {
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 2);
    let mut dir: Option<TempDir> = None;
    let mut prompt_block = PromptBlock::Absent;
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
        let prompt_mode = matches!(flag, "-p" | "--prompt");
        if text.starts_with(FILE_REFERENCE_PREFIX) {
            // The caller's own pointer sentence. With a block, and a private
            // file Copilot can open, the prompt it names is copied with the
            // block after it; otherwise it is passed on unchanged.
            let block = prompt_suffix.filter(|_| prompt_mode && (grant_dir || path_access));
            let copied = block.map(|block| {
                read_pointed_prompt(text).and_then(|prompt| {
                    let with_block = crate::run_guidance::append_block(&prompt, block);
                    private_pointer(&mut dir, "copilot-prompt.md", flag, &with_block).ok()
                })
            });
            match copied {
                Some(Some(pointer)) => {
                    out.push(flag.to_string());
                    out.push(pointer);
                    prompt_block = PromptBlock::Added;
                }
                Some(None) => {
                    out.extend(args[i..i + consumed].iter().cloned());
                    prompt_block = PromptBlock::PromptFileNotCopied;
                }
                None => out.extend(args[i..i + consumed].iter().cloned()),
            }
        } else {
            // One file per flag kind: `-p` and `--prompt` share a name, so a
            // second prompt fails with "given twice" from `create_new`.
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
            let with_suffix;
            let text = match prompt_suffix {
                Some(block) if prompt_mode => {
                    with_suffix = crate::run_guidance::append_block(text, block);
                    prompt_block = PromptBlock::Added;
                    with_suffix.as_str()
                }
                _ => text,
            };
            out.push(flag.to_string());
            out.push(private_pointer(&mut dir, name, flag, text)?);
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
    let mut rewritten = Rewritten::from_parts(out, None, dir);
    rewritten.prompt_block = prompt_block;
    Ok(rewritten)
}

/// Writes `text` to the private file `name` and returns the pointer
/// sentence naming it.
fn private_pointer(
    dir: &mut Option<TempDir>,
    name: &str,
    flag: &str,
    text: &str,
) -> Result<String> {
    let path = write_private_file(dir, name, flag, text)?;
    let path_text = path.to_string_lossy();
    if path_text.contains('\n') || path_text.contains('\0') {
        bail!("the private file path for {flag} contains a newline");
    }
    Ok(FILE_REFERENCE_TEMPLATE.replace("{path}", &path_text))
}

/// Whether `args` give Copilot any prompt flag (`-p`/`--prompt` or
/// `-i`/`--interactive`) before `--`, with text or a file reference
/// (issue #3162). Without one, Copilot reads its prompt from a piped stdin.
pub(crate) fn copilot_has_prompt_flag(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| copilot_prompt_flag(a).is_some())
}
