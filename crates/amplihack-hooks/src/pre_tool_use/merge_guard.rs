//! Issue #1551, crusty review of PR #1644: refuse a merge typed into an
//! agent's Bash tool inside an auto-drive run, whatever `gh` PATH resolves.
//!
//! The `gh` shim (`amplifier-bundle/tools/gh-merge-guard/gh`) is enforced by
//! PATH order alone. Agent shells start from the user's profile, and a profile
//! that prepends `~/.local/bin`, `~/.npm-global/bin` or Homebrew's directory
//! puts a real `gh` ahead of the shim. This check reads the command text
//! instead, so it holds in that case too. The two layers overlap on purpose:
//! the hook sees what the agent typed, the shim sees what actually runs
//! (aliases, extensions, scripts the agent wrote to a file).
//!
//! Active only when `AMPLIHACK_MERGE_GUARD` is non-empty, which the recipe
//! runner sets for every auto-drive run and every agent CLI inside one. The
//! merge gate itself runs from a recipe bash step, never through an agent's
//! Bash tool, so this hook never sees a legitimate run of it. An agent naming
//! the gate in its shell tool is therefore refused outright (crusty round 2):
//! the gate trusts its Crusty and QA record files, which an agent can write.
//!
//! When the command also runs an interpreter (a shell, Python, Perl, Ruby,
//! Node, awk, …), quoted text is code that will run, not prose, so every word
//! is read as a command too and scanned for endpoints and mutations. That
//! catches `echo 'gh pr merge 38' | bash`, `bash -s <<<'…'` and
//! `python3 -c "os.system('gh pr merge 38')"`, while `git commit -m '… gh pr
//! merge …'` with no interpreter in the command still passes. Variable
//! indirection (`G=gh; $G pr merge`) and script files are not readable from
//! the command text and are left to the shim and the `MERGED` checks.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Set by `amplihack recipe run` for auto-drive runs (see
/// `crates/amplihack-cli/src/commands/recipe/run/merge_guard.rs`).
pub(super) const MERGE_GUARD_ENV: &str = "AMPLIHACK_MERGE_GUARD";

/// The merge gate's file name. Only auto-drive's own recipe step runs it.
const GATE_NAME: &str = "autodrive_merge_gate.sh";

/// How deep `bash -c '…'`, `eval …` and `$(…)` bodies are followed.
const MAX_NESTING: usize = 8;

const BLOCK_MESSAGE: &str = "\
🚫 OPERATION BLOCKED - merge outside auto-drive's merge gate (amplihack #1551)\n\
\n\
This is an auto-drive run (AMPLIHACK_MERGE_GUARD is set) and the command\n\
contains {label}. Only autodrive_merge_gate.sh may merge: leave the PR open.\n\
Auto-drive's merge gate merges it after a Crusty CLEAN and the merge-ready loop,\n\
from its own recipe step; an agent's shell never runs the gate.\n\
\n\
This check reads the command itself, so it applies whichever gh your shell's\n\
PATH finds. Do not retry with another gh path, PATH, alias, API client or token.";

/// GraphQL mutations that merge, as whole names in any case (same list as
/// the shim's `MUTATION_RE`).
static MUTATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^a-z0-9_])(?:mergepullrequest|enablepullrequestautomerge|enqueuepullrequest|mergebranch)(?:[^a-z0-9_]|$)",
    )
    .expect("static regex")
});

/// REST merge endpoints in a path or URL: `…/pulls/<n>/merge` and
/// `repos/<o>/<r>/merges` (or `repositories/<id>/merges`), any case, with an
/// optional trailing slash, query or fragment.
static ENDPOINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|/)(?:pulls/[^/?#\s]+/merge|(?:repos/[^/?#\s]+/[^/?#\s]+|repositories/[^/?#\s]+)/merges)/?(?:$|[?#])",
    )
    .expect("static regex")
});

/// Whether `value` (the inherited [`MERGE_GUARD_ENV`]) switches the check on:
/// any non-empty value does, matching the recipe runner.
pub(super) fn guard_active(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// The block response for a Bash `command` that merges, or `None`.
pub(super) fn check(command: &str) -> Option<Value> {
    let label = merge_form(command, 0, false)?;
    Some(serde_json::json!({
        "block": true,
        "message": BLOCK_MESSAGE.replace("{label}", label),
    }))
}

/// The label of the first merge form in `command`, following `bash -c`,
/// `eval` and command substitutions up to [`MAX_NESTING`] levels.
///
/// `scripted` is set once any level of the command runs an interpreter; from
/// then on every word is also read as code (see the module doc).
fn merge_form(command: &str, depth: usize, scripted: bool) -> Option<&'static str> {
    if depth > MAX_NESTING {
        return None;
    }
    let (segments, nested) = split(command);
    let scripted = scripted
        || segments
            .iter()
            .flatten()
            .any(|w| is_interpreter(basename(w)));
    for nested_cmd in &nested {
        if let Some(label) = merge_form(nested_cmd, depth + 1, scripted) {
            return Some(label);
        }
    }
    for words in &segments {
        if let Some(label) = segment_merge_form(words, depth, scripted) {
            return Some(label);
        }
    }
    None
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Programs that run code given as text: a `-c`/`-e` argument, a here-string
/// or a pipe on stdin.
fn is_interpreter(name: &str) -> bool {
    matches!(
        name,
        "bash"
            | "sh"
            | "zsh"
            | "dash"
            | "ksh"
            | "mksh"
            | "fish"
            | "busybox"
            | "perl"
            | "ruby"
            | "node"
            | "nodejs"
            | "deno"
            | "bun"
            | "php"
            | "lua"
            | "tclsh"
            | "awk"
            | "gawk"
            | "mawk"
            | "nawk"
            | "osascript"
    ) || name.starts_with("python")
        || name.starts_with("pypy")
}

fn segment_merge_form(words: &[String], depth: usize, scripted: bool) -> Option<&'static str> {
    for (i, word) in words.iter().enumerate() {
        let rest = &words[i + 1..];
        if basename(word) == GATE_NAME {
            return Some("a run of the merge gate autodrive_merge_gate.sh by an agent");
        }
        if scripted && let Some(label) = code_merge_form(word, depth) {
            return Some(label);
        }
        let found = match basename(word) {
            "gh" => gh_merge_form(rest),
            // HTTP clients: an endpoint or mutation anywhere in the segment.
            "curl" | "wget" | "http" | "https" | "xh" => {
                words.iter().find_map(|w| raw_merge_form(w))
            }
            "bash" | "sh" | "zsh" | "dash" | "ksh" => {
                shell_c_body(rest).and_then(|body| merge_form(body, depth + 1, scripted))
            }
            "eval" => merge_form(&rest.join(" "), depth + 1, scripted),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// A word of a command that runs an interpreter, read as code: argument
/// lists (`['gh', 'pr', 'merge']`, `"gh","pr"`) become words, and string
/// literals keep their closing punctuation off the endpoint.
fn code_merge_form(word: &str, depth: usize) -> Option<&'static str> {
    let code: String = word
        .chars()
        .map(|c| {
            if matches!(c, ',' | '[' | ']' | '{' | '}') {
                ' '
            } else {
                c
            }
        })
        .collect();
    if code.chars().any(char::is_whitespace)
        && let Some(label) = merge_form(&code, depth + 1, true)
    {
        return Some(label);
    }
    raw_merge_form(code.trim_matches(|c: char| !(c.is_alphanumeric() || c == '/')))
        .or_else(|| raw_merge_form(word))
}

/// The script of `bash -c '<script>'` (also `-lc`, `-ic`, `-ec`, …).
fn shell_c_body(args: &[String]) -> Option<&str> {
    let pos = args
        .iter()
        .position(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains('c'))?;
    args.get(pos + 1).map(String::as_str)
}

fn raw_merge_form(word: &str) -> Option<&'static str> {
    if MUTATION_RE.is_match(word) {
        Some("a GraphQL merge mutation")
    } else if ENDPOINT_RE.is_match(word) {
        Some("a REST merge endpoint")
    } else {
        None
    }
}

/// `args` follow a word whose basename is `gh`.
fn gh_merge_form(args: &[String]) -> Option<&'static str> {
    let rest = skip_flags(args);
    let (cmd, rest) = rest.split_first()?;
    match cmd.as_str() {
        "pr" => {
            let rest = skip_flags(rest);
            (rest.first().map(String::as_str) == Some("merge")).then_some("gh pr merge")
        }
        "api" => {
            if let Some(label) = rest.iter().find_map(|w| raw_merge_form(w)) {
                return Some(label);
            }
            // A GraphQL body read from a file or stdin cannot be inspected.
            let graphql = rest
                .iter()
                .any(|w| w.to_ascii_lowercase().contains("graphql"));
            let unreadable = rest
                .iter()
                .any(|w| w == "--input" || w.starts_with("--input=") || w.contains("=@"));
            (graphql && unreadable).then_some("a GraphQL query read from a file or stdin")
        }
        _ => None,
    }
}

/// Drops leading flags; `-R`, `--repo` and `--hostname` take a value.
fn skip_flags(mut args: &[String]) -> &[String] {
    while let Some((first, rest)) = args.split_first() {
        match first.as_str() {
            "-R" | "--repo" | "--hostname" => args = rest.get(1..).unwrap_or(&[]),
            f if f.starts_with('-') => args = rest,
            _ => break,
        }
    }
    args
}

/// Splits `command` into simple commands (word lists) at unquoted `;`, `&`,
/// `|`, newlines, parentheses and redirections, honouring quotes and
/// backslashes. Bodies of `$(…)` and backticks, quoted or not, are returned
/// separately to be checked as commands of their own.
///
/// This is a reader for the commands agents write, not a shell: anything it
/// misreads still meets the `gh` shim, and branch protection after that.
fn split(command: &str) -> (Vec<Vec<String>>, Vec<String>) {
    let chars: Vec<char> = command.chars().collect();
    let mut segments = Vec::new();
    let mut nested = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut i = 0;

    fn end_word(words: &mut Vec<String>, word: &mut String, in_word: &mut bool) {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                if let Some(&next) = chars.get(i + 1)
                    && next != '\n'
                {
                    word.push(next);
                    in_word = true;
                }
                i += 2;
            }
            '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
                in_word = true;
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    match chars[i] {
                        '\\' if i + 1 < chars.len() => {
                            word.push(chars[i + 1]);
                            i += 2;
                        }
                        '$' if chars.get(i + 1) == Some(&'(') => {
                            let (body, end) = substitution(&chars, i + 2);
                            nested.push(body);
                            i = end;
                        }
                        '`' => {
                            let (body, end) = backtick(&chars, i + 1);
                            nested.push(body);
                            i = end;
                        }
                        other => {
                            word.push(other);
                            i += 1;
                        }
                    }
                }
                i += 1;
                in_word = true;
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                let (body, end) = substitution(&chars, i + 2);
                nested.push(body);
                in_word = true;
                i = end;
            }
            '`' => {
                let (body, end) = backtick(&chars, i + 1);
                nested.push(body);
                in_word = true;
                i = end;
            }
            ';' | '&' | '|' | '\n' | '(' | ')' => {
                end_word(&mut words, &mut word, &mut in_word);
                if !words.is_empty() {
                    segments.push(std::mem::take(&mut words));
                }
                i += 1;
            }
            '<' | '>' => {
                end_word(&mut words, &mut word, &mut in_word);
                i += 1;
            }
            c if c.is_whitespace() => {
                end_word(&mut words, &mut word, &mut in_word);
                i += 1;
            }
            other => {
                word.push(other);
                in_word = true;
                i += 1;
            }
        }
    }
    end_word(&mut words, &mut word, &mut in_word);
    if !words.is_empty() {
        segments.push(words);
    }
    (segments, nested)
}

/// The body of a `$(` starting at `start`, up to its matching `)`, and the
/// index after it. Quotes inside are skipped so their parentheses do not
/// count; an unterminated body runs to the end.
fn substitution(chars: &[char], start: usize) -> (String, usize) {
    let mut depth = 1usize;
    let mut i = start;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => i += 1,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return (chars[start..i].iter().collect(), i + 1);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    (
        chars[start.min(chars.len())..].iter().collect(),
        chars.len(),
    )
}

/// The body of a backtick substitution starting at `start`.
fn backtick(chars: &[char], start: usize) -> (String, usize) {
    let end = chars[start.min(chars.len())..]
        .iter()
        .position(|&c| c == '`')
        .map_or(chars.len(), |p| start + p);
    (chars[start.min(chars.len())..end].iter().collect(), end + 1)
}

#[cfg(test)]
#[path = "tests/merge_guard_tests.rs"]
mod tests;
