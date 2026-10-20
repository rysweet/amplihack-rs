//! Issue #1551: inside an auto-drive run, refuse the direct merge commands an
//! agent types into its Bash tool.
//!
//! The primary guard is the `gh` shim (`amplifier-bundle/tools/gh-merge-guard/gh`),
//! which the recipe runner puts first on `PATH` and re-asserts through
//! `BASH_ENV` after a login shell's profile has run. This check is a second,
//! deliberately small layer for the same ordinary forms, independent of which
//! `gh` `PATH` finds:
//!
//! * `gh … pr … merge` where `gh` (any path) is the program being run;
//! * `gh api <endpoint>` whose endpoint is `…/pulls/<n>/merge` or a
//!   `…/merges` endpoint.
//!
//! Running `autodrive_merge_gate.sh` is not one of them. The shim trusts only
//! a parent that is `/bin/bash -p <gate>` with a clean environment, and the
//! runner exports `BASH_ENV` into every agent shell, so a gate run from an
//! agent's shell has its merge refused by the shim. Clearing the environment
//! first to get past that is deliberate evasion, the stated non-goal; reading
//! shell options here to spot gate runs only produced false positives
//! (`bash -n`, `command -v`, `bash -c '… gate'`; crusty rounds 11–15).
//!
//! The program word is the first word of each command, after `VAR=value`
//! assignments and the wrappers in [`WRAPPERS`]. Nothing else is read: quoted
//! text, arguments of other programs, heredoc bodies and comments are data,
//! so `git commit -m '… gh pr merge …'`, `echo gh pr merge 38 > notes`,
//! `grep -rn 'gh pr merge' .` and `cat <<'EOF' … EOF` pass in any context.
//!
//! This is not a shell parser and does not try to be one. Code hidden from
//! the program word (`bash -c '…'`, a script file, `eval`, an alias, a
//! variable, `curl` with the token) is outside what this check reads. The
//! ordinary forms are covered by the shim; deliberate evasion with the same
//! token is a stated non-goal (docs/reference/auto-drive-to-merge.md, "What
//! this does not stop"), backstopped by branch protection and the `MERGED`
//! checks after each phase.
//!
//! Active only when `AMPLIHACK_MERGE_GUARD` is non-empty, which the recipe
//! runner sets for every auto-drive run and every agent CLI inside one.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Set by `amplihack recipe run` for auto-drive runs (see
/// `crates/amplihack-cli/src/commands/recipe/run/merge_guard.rs`).
pub(super) const MERGE_GUARD_ENV: &str = "AMPLIHACK_MERGE_GUARD";

/// Programs that run the command after them: `(name, options that take a
/// value, positional arguments before the command)`.
const WRAPPERS: &[(&str, &[&str], usize)] = &[
    (
        "env",
        &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"],
        0,
    ),
    ("exec", &["-a"], 0),
    ("command", &[], 0),
    ("nohup", &[], 0),
    ("time", &[], 0),
    ("nice", &["-n", "--adjustment"], 0),
    ("timeout", &["-s", "--signal", "-k", "--kill-after"], 1),
    (
        "sudo",
        &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U"],
        0,
    ),
];

/// Shell reserved words that can precede a command's program word.
const RESERVED: &[&str] = &[
    "!", "{", "}", "if", "then", "elif", "else", "while", "until", "do",
];

/// `gh api` options that take a value (so the value is not the endpoint).
const GH_API_VALUE_OPTS: &[&str] = &[
    "-X",
    "--method",
    "-f",
    "--raw-field",
    "-F",
    "--field",
    "-H",
    "--header",
    "--input",
    "-q",
    "--jq",
    "-t",
    "--template",
    "-p",
    "--preview",
    "--cache",
    "--hostname",
];

/// `gh` global options that take a value.
const GH_VALUE_OPTS: &[&str] = &["-R", "--repo"];

const BLOCK_MESSAGE: &str = "\
🚫 OPERATION BLOCKED - merge outside auto-drive's merge gate (amplihack #1551)\n\
\n\
This is an auto-drive run (AMPLIHACK_MERGE_GUARD is set) and the command\n\
runs {label}. Only autodrive_merge_gate.sh may merge: leave the PR open.\n\
Auto-drive's merge gate merges it after a Crusty CLEAN and the merge-ready loop,\n\
from its own recipe step; an agent's shell never runs the gate.\n\
Do not retry with another gh path, PATH, alias, API client or token.";

/// REST merge endpoints: `…/pulls/<n>/merge` and `repos/<o>/<r>/merges` (or
/// `repositories/<id>/merges`), any case, with an optional trailing slash,
/// query or fragment.
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
    let label = commands(command).iter().find_map(|c| merge_form(c))?;
    Some(serde_json::json!({
        "block": true,
        "message": BLOCK_MESSAGE.replace("{label}", label),
    }))
}

/// The label of the merge form `words` (one simple command) runs, if any.
fn merge_form(words: &[String]) -> Option<&'static str> {
    let rest = program(words)?;
    let (name, args) = rest.split_first()?;
    if basename(name) != "gh" {
        return None;
    }
    gh_merge_form(args)
}

/// The words from the program being run onward: reserved words, `VAR=value`
/// assignments and [`WRAPPERS`] (with their options) are skipped.
fn program(words: &[String]) -> Option<&[String]> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        if RESERVED.contains(&w) || is_assignment(w) {
            i += 1;
            continue;
        }
        let Some((_, value_opts, positionals)) =
            WRAPPERS.iter().find(|(name, _, _)| *name == basename(w))
        else {
            return Some(&words[i..]);
        };
        i += 1;
        let mut skip = *positionals;
        while i < words.len() {
            let w = words[i].as_str();
            if value_opts.contains(&w) {
                i += 2;
            } else if w.starts_with('-') || is_assignment(w) {
                i += 1;
            } else if skip > 0 {
                skip -= 1;
                i += 1;
            } else {
                break;
            }
        }
    }
    None
}

/// `gh`'s arguments: `pr merge`, or `api` with a merge endpoint.
fn gh_merge_form(args: &[String]) -> Option<&'static str> {
    let mut positional = operands(args, GH_VALUE_OPTS);
    match positional.next().map(|(_, w)| w.as_str()) {
        Some("pr") => (positional.next()?.1 == "merge").then_some("gh pr merge"),
        Some("api") => {
            let (at, _) = operands(args, GH_VALUE_OPTS).find(|(_, w)| w.as_str() == "api")?;
            let (_, endpoint) = operands(&args[at + 1..], GH_API_VALUE_OPTS).next()?;
            ENDPOINT_RE
                .is_match(endpoint)
                .then_some("gh api on a merge endpoint")
        }
        _ => None,
    }
}

/// The non-option words of `args` with their indexes, skipping the value of
/// each option in `value_opts`.
fn operands<'a>(
    args: &'a [String],
    value_opts: &'a [&'a str],
) -> impl Iterator<Item = (usize, &'a String)> + 'a {
    let mut skip_next = false;
    args.iter().enumerate().filter(move |(_, w)| {
        if std::mem::take(&mut skip_next) {
            return false;
        }
        if value_opts.contains(&w.as_str()) {
            skip_next = true;
            return false;
        }
        !w.starts_with('-')
    })
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Splits `text` into simple commands, each a list of unquoted words.
///
/// Commands end at `;`, `&`, `|`, `(`, `)` and newlines outside quotes.
/// Quotes and backslashes are removed from words. Comments are dropped, and
/// heredoc bodies are skipped: they are data, never a command's words.
/// `$(…)` and backticks stay inside the word they appear in, including a
/// `$(…)` inside double quotes, whose own quotes do not end the string.
fn commands(text: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;
    let end_word = |word: &mut String, in_word: &mut bool, out: &mut Vec<Vec<String>>| {
        if std::mem::take(in_word) {
            out.last_mut()
                .expect("non-empty")
                .push(std::mem::take(word));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
                        // `"$(…)"`: quotes inside the substitution are its
                        // own, not the end of this string (bash manual,
                        // Command Substitution).
                        let end = subst_end(&chars, i);
                        word.extend(&chars[i..=end]);
                        i = end + 1;
                        continue;
                    }
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '\\' => {
                in_word = true;
                if let Some(&next) = chars.get(i + 1) {
                    if next != '\n' {
                        word.push(next);
                    }
                    i += 1;
                }
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                in_word = true;
                let end = subst_end(&chars, i);
                word.extend(&chars[i..=end]);
                i = end;
            }
            '`' => {
                in_word = true;
                word.push(c);
                i += 1;
                while i < chars.len() && chars[i] != '`' {
                    word.push(chars[i]);
                    i += 1;
                }
                word.push('`');
            }
            '#' if !in_word => {
                while i + 1 < chars.len() && chars[i + 1] != '\n' {
                    i += 1;
                }
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) == Some(&'<') => {
                // A here-string: its word follows as an ordinary word.
                end_word(&mut word, &mut in_word, &mut out);
                i += 2;
            }
            '<' if chars.get(i + 1) == Some(&'<') => {
                // `<<WORD` / `<<-WORD`: remember the delimiter; the body is
                // skipped at the end of this line.
                end_word(&mut word, &mut in_word, &mut out);
                heredocs.push(heredoc_delim(&chars, &mut i));
                continue;
            }
            '\n' => {
                end_word(&mut word, &mut in_word, &mut out);
                out.push(Vec::new());
                skip_heredoc_bodies(&chars, &mut i, std::mem::take(&mut heredocs));
            }
            ';' | '&' | '|' | '(' | ')' => {
                end_word(&mut word, &mut in_word, &mut out);
                out.push(Vec::new());
            }
            c if c.is_whitespace() => end_word(&mut word, &mut in_word, &mut out),
            _ => {
                in_word = true;
                word.push(c);
            }
        }
        i += 1;
    }
    end_word(&mut word, &mut in_word, &mut out);
    out.retain(|c| !c.is_empty());
    out
}

/// The index of the `)` closing the `$(` at `start` (or the last index, if
/// it is unclosed). Everything inside is data to this check; the scan only
/// keeps quoted text and heredoc bodies from ending it early, so a `)` or `"`
/// in a commit message body does not close the substitution.
fn subst_end(chars: &[char], start: usize) -> usize {
    let mut depth = 0usize;
    let mut heredocs = Vec::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            '\\' => i += 1,
            '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    } else if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
                        i = subst_end(chars, i);
                    }
                    i += 1;
                }
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) == Some(&'<') => i += 2,
            '<' if chars.get(i + 1) == Some(&'<') => {
                heredocs.push(heredoc_delim(chars, &mut i));
                continue;
            }
            '\n' => skip_heredoc_bodies(chars, &mut i, std::mem::take(&mut heredocs)),
            _ => {}
        }
        i += 1;
    }
    chars.len() - 1
}

/// Reads the delimiter of the `<<WORD` / `<<-WORD` at `*i`, leaving `*i` just
/// past it. Returns the delimiter (quotes removed) and whether `-` strips tabs.
fn heredoc_delim(chars: &[char], i: &mut usize) -> (String, bool) {
    *i += 2;
    let strip_tabs = chars.get(*i) == Some(&'-');
    if strip_tabs {
        *i += 1;
    }
    while chars.get(*i).is_some_and(|c| *c == ' ' || *c == '\t') {
        *i += 1;
    }
    let mut delim = String::new();
    while let Some(&d) = chars.get(*i) {
        if d.is_whitespace() || ";&|()<>".contains(d) {
            break;
        }
        if !"'\"\\".contains(d) {
            delim.push(d);
        }
        *i += 1;
    }
    (delim, strip_tabs)
}

/// With `*i` at a newline, skips the bodies of `heredocs` in order, leaving
/// `*i` at the newline that ends the last delimiter line.
fn skip_heredoc_bodies(chars: &[char], i: &mut usize, heredocs: Vec<(String, bool)>) {
    for (delim, strip_tabs) in heredocs {
        loop {
            let start = *i + 1;
            if start >= chars.len() {
                break;
            }
            let stop = (start..chars.len())
                .find(|&j| chars[j] == '\n')
                .unwrap_or(chars.len());
            let line: String = chars[start..stop].iter().collect();
            *i = stop;
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                &line
            };
            if line == delim {
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/merge_guard_tests.rs"]
mod tests;
