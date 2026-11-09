//! Issue #2030, decision D7: rewrite the arguments `amplihack claude` passes
//! through to `claude`, so a caller that puts prompt text on the wrapper's
//! argv does not put it on the agent's argv.
//!
//! | Caller passes                                   | Child receives |
//! | ----------------------------------------------- | -------------- |
//! | `-p <text>`, `--print <text>`, `-p=<text>`, `--print=<text>` | bare `-p`/`--print`, text on stdin |
//! | `-p -- <text>` (print mode, one positional after `--`) | bare `-p`, text on stdin, `--` dropped |
//! | bare `-p` with nothing after it              | unchanged |
//! | bare `-p` followed by a token starting with `-` | unchanged; Claude parses that token as an option. Use `-p -- <text>` for a prompt that starts with `-` |
//! | `--system-prompt[=]<text>`                      | `--system-prompt-file <per-session file>` |
//! | `--append-system-prompt[=]<text>`               | `--append-system-prompt-file <per-session file>` (counts as user-supplied) |
//! | `--system-prompt-file`, `--append-system-prompt-file` | unchanged |
//! | an interactive positional prompt and everything after it | unchanged (D9) |
//!
//! In print mode (a bare `-p`/`--print` anywhere before `--`), a positional
//! prompt is the print prompt and goes to stdin as well, whether it comes
//! before or after `--`. More than one positional prompt in print mode is an
//! error. Outside print mode, `--` and everything after it are passed through
//! unchanged (D9).
//!
//! Per-session files are 0600, inside one 0700 `TempDir` named
//! `amplihack-prompt-*`, created only when needed and removed when the
//! [`Rewritten`] is dropped. Error messages name flags only, never the text,
//! and `Debug` redacts the stdin payload.
//!
//! Option values are skipped by Claude's own Commander arity table
//! ([`crate::commands::profile::claude_option_end`]), so the value of
//! `--model opus` is never read as a positional prompt.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tempfile::TempDir;

const PRINT_FLAGS: &[&str] = &["-p", "--print"];
const SYSTEM_PROMPT: &str = "--system-prompt";
const SYSTEM_PROMPT_FILE: &str = "--system-prompt-file";
const APPEND_PROMPT: &str = "--append-system-prompt";
const APPEND_PROMPT_FILE: &str = "--append-system-prompt-file";

/// The rewritten passthrough arguments and what they need at spawn time.
///
/// Holds the per-session directory: keep it alive until the child has been
/// waited on.
pub(crate) struct Rewritten {
    /// The arguments to hand to `claude`. No prompt text.
    pub(crate) args: Vec<String>,
    /// Text to write to the child's stdin, when the caller gave `-p <text>`.
    pub(crate) stdin_payload: Option<String>,
    /// The caller supplied their own `--append-system-prompt*`, so
    /// amplihack's contract is not added.
    pub(crate) user_supplied_append: bool,
    /// The rewrite produced a `*-file` flag, which needs the version gate.
    pub(crate) used_file_flags: bool,
    /// The per-session 0700 directory, created on first use.
    _dir: Option<TempDir>,
}

impl std::fmt::Debug for Rewritten {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let payload = self
            .stdin_payload
            .as_ref()
            .map(|p| format!("Some(<{} bytes>)", p.len()))
            .unwrap_or_else(|| "None".to_string());
        f.debug_struct("Rewritten")
            .field("args", &self.args)
            .field("stdin_payload", &format_args!("{payload}"))
            .field("user_supplied_append", &self.user_supplied_append)
            .field("used_file_flags", &self.used_file_flags)
            .field("dir", &self._dir.as_ref().map(|d| d.path().to_path_buf()))
            .finish()
    }
}

impl Rewritten {
    /// Arguments unchanged, for binaries the rewrite does not apply to.
    pub(crate) fn unchanged(args: Vec<String>) -> Self {
        Self {
            args,
            stdin_payload: None,
            user_supplied_append: false,
            used_file_flags: false,
            _dir: None,
        }
    }

    /// Arguments for a non-Claude agent: an optional stdin payload and the
    /// per-session directory holding any pointer-sentence files.
    pub(super) fn from_parts(
        args: Vec<String>,
        stdin_payload: Option<String>,
        dir: Option<TempDir>,
    ) -> Self {
        Self {
            args,
            stdin_payload,
            user_supplied_append: false,
            used_file_flags: false,
            _dir: dir,
        }
    }
}

/// Which prompt-bearing flag a token is, and its inline `=value` if any.
enum Token<'a> {
    Print {
        flag: &'a str,
        inline: Option<&'a str>,
    },
    SystemPrompt(Option<&'a str>),
    AppendPrompt(Option<&'a str>),
    SystemPromptFile,
    AppendPromptFile,
    Other,
}

fn classify(arg: &str) -> Token<'_> {
    let (name, inline) = match arg.split_once('=') {
        Some((name, value)) if name.starts_with('-') => (name, Some(value)),
        _ => (arg, None),
    };
    if let Some(flag) = PRINT_FLAGS.iter().find(|f| **f == name) {
        return Token::Print { flag, inline };
    }
    match name {
        SYSTEM_PROMPT => Token::SystemPrompt(inline),
        APPEND_PROMPT => Token::AppendPrompt(inline),
        SYSTEM_PROMPT_FILE => Token::SystemPromptFile,
        APPEND_PROMPT_FILE => Token::AppendPromptFile,
        _ => Token::Other,
    }
}

struct Builder {
    out: Vec<String>,
    payload: Option<String>,
    dir: Option<TempDir>,
    system_text: bool,
    system_file: bool,
    append_text: bool,
    append_file: bool,
}

impl Builder {
    fn set_payload(&mut self, text: &str) -> Result<()> {
        if self.payload.is_some() {
            bail!("the prompt was given twice (-p/--print); pass it once");
        }
        self.payload = Some(text.to_string());
        Ok(())
    }

    /// Write `text` to `<dir>/<name>` (0600, `create_new`) and return the path.
    fn write_private(&mut self, name: &str, flag: &str, text: &str) -> Result<PathBuf> {
        write_private_file(&mut self.dir, name, flag, text)
    }
}

/// Write `text` to `<dir>/<name>` (0600, `create_new`, `O_NOFOLLOW`) and
/// return the path. `dir` is the per-session 0700 `amplihack-prompt-*`
/// directory, created on first use. Errors name `flag`, never the text.
pub(super) fn write_private_file(
    dir: &mut Option<TempDir>,
    name: &str,
    flag: &str,
    text: &str,
) -> Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if dir.is_none() {
        let created = tempfile::Builder::new()
            .prefix("amplihack-prompt-")
            .tempdir()
            .context("cannot create a private directory for the prompt files")?;
        std::fs::set_permissions(created.path(), std::fs::Permissions::from_mode(0o700))
            .context("cannot set mode 0700 on the private prompt directory")?;
        *dir = Some(created);
    }
    let dir = dir.as_ref().expect("created above");
    let path = dir.path().join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| {
            // The io::Error carries an errno only, never the prompt text.
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!("{flag} was given twice; pass it once")
            } else {
                anyhow::Error::new(error)
                    .context(format!("cannot create a private file for {flag}"))
            }
        })?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write the {flag} text to a private file"))?;
    Ok(path)
}

/// Rewrite `amplihack claude` passthrough arguments; see the module docs.
///
/// # Errors
///
/// - the prompt was given twice;
/// - `--system-prompt` and `--system-prompt-file` (or the append pair) were
///   both given;
/// - a private file could not be written.
pub(crate) fn rewrite_claude_passthrough(args: &[String]) -> Result<Rewritten> {
    let print_mode = args
        .iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| matches!(classify(a), Token::Print { .. }));
    let mut b = Builder {
        out: Vec::with_capacity(args.len()),
        payload: None,
        dir: None,
        system_text: false,
        system_file: false,
        append_text: false,
        append_file: false,
    };

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            if print_mode {
                // In print mode a positional after `--` is the print prompt.
                // Claude reads it from stdin, and `--` is dropped because
                // nothing follows it.
                match &args[i + 1..] {
                    [] => {}
                    [text] if b.payload.is_none() => b.set_payload(text)?,
                    _ => bail!("the prompt was given twice (-p/--print); pass it once"),
                }
            } else {
                // D9: outside print mode, everything after `--` belongs to
                // the caller.
                b.out.extend(args[i..].iter().cloned());
            }
            break;
        }
        match classify(arg) {
            Token::Print { flag, inline } => {
                b.out.push(flag.to_string());
                match inline {
                    Some(text) => b.set_payload(text)?,
                    None => {
                        if let Some(next) = args.get(i + 1)
                            && !next.starts_with('-')
                        {
                            b.set_payload(next)?;
                            i += 1;
                        }
                    }
                }
                i += 1;
            }
            Token::SystemPrompt(inline) => {
                let Some(text) = inline.or_else(|| args.get(i + 1).map(String::as_str)) else {
                    // No value: Claude reports the error itself.
                    b.out.push(arg.to_string());
                    i += 1;
                    continue;
                };
                let path = b.write_private("system-prompt.md", SYSTEM_PROMPT, text)?;
                b.system_text = true;
                b.out.push(SYSTEM_PROMPT_FILE.to_string());
                b.out.push(path.to_string_lossy().into_owned());
                i += if inline.is_some() { 1 } else { 2 };
            }
            Token::AppendPrompt(inline) => {
                let Some(text) = inline.or_else(|| args.get(i + 1).map(String::as_str)) else {
                    b.out.push(arg.to_string());
                    i += 1;
                    continue;
                };
                let path = b.write_private("append-system-prompt.md", APPEND_PROMPT, text)?;
                b.append_text = true;
                b.out.push(APPEND_PROMPT_FILE.to_string());
                b.out.push(path.to_string_lossy().into_owned());
                i += if inline.is_some() { 1 } else { 2 };
            }
            Token::SystemPromptFile | Token::AppendPromptFile => {
                if matches!(classify(arg), Token::SystemPromptFile) {
                    b.system_file = true;
                } else {
                    b.append_file = true;
                }
                let end = crate::commands::profile::claude_option_end(args, i);
                b.out.extend(args[i..end].iter().cloned());
                i = end;
            }
            Token::Other if arg.starts_with('-') && arg != "-" => {
                let end = crate::commands::profile::claude_option_end(args, i);
                b.out.extend(args[i..end].iter().cloned());
                i = end;
            }
            Token::Other => {
                if print_mode && b.payload.is_none() {
                    // In print mode a positional argument is the prompt.
                    b.set_payload(arg)?;
                    i += 1;
                } else if print_mode {
                    bail!("the prompt was given twice (-p/--print); pass it once");
                } else {
                    // D9: an interactive positional prompt and everything after
                    // it are passed through untouched.
                    b.out.extend(args[i..].iter().cloned());
                    break;
                }
            }
        }
    }

    if b.system_text && b.system_file {
        bail!("--system-prompt and --system-prompt-file were both given; pass one");
    }
    if b.append_text && b.append_file {
        bail!("--append-system-prompt and --append-system-prompt-file were both given; pass one");
    }
    Ok(Rewritten {
        args: b.out,
        stdin_payload: b.payload,
        user_supplied_append: b.append_text || b.append_file,
        used_file_flags: b.system_text || b.append_text,
        _dir: b.dir,
    })
}
