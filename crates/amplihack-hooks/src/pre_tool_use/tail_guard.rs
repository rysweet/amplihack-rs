//! Issue #1794: deny `tail` in follow mode on a device that never ends.
//!
//! Agents waited with `tail -f /dev/null`, which outlives the agent's shell
//! and is left orphaned. This guard denies a shell tool call when a command
//! the shell would run resolves to `tail` with a follow flag and an operand
//! in [`DEVICES`], and points the agent to `amplihack wait`.
//!
//! Only words the shell would run as a program are checked: heredoc bodies,
//! redirect targets, comments and the arguments of other programs (`echo`,
//! `docker run`, `bash -c`) are data. The guard is a usability aid, not a
//! security boundary, and it has no setting to turn it off.
//!
//! TODO(#1735): switch to `shell::tokenise`/`program` and the shared deny
//! helper once #1735 merges; the wire output stays the same. Carry the
//! wrapper option tables over only if that module needs them itself.
//!
//! Every function here is total: input that cannot be parsed, such as an
//! unbalanced quote, is "not matched", in line with the hook's fail-open
//! policy. A panic would be caught by the hook runner and lose the deny along
//! with the later checks, hence the lints below.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use serde_json::{Value, json};

/// The fixed reason text. It never includes the command, so a secret in an
/// environment assignment is never echoed back.
pub(super) const BLOCK_MESSAGE: &str = "`tail -f` on /dev/null (or another device that never ends) \
leaves an orphaned process. Use `amplihack wait --seconds N`, or run the work in the \
background and use `amplihack wait --until pid-exited:<PID> --deadline <SECS>`.";

/// Tool names whose input is a shell command, compared case-insensitively:
/// Claude Code `Bash`, `Monitor` and `PowerShell`, Copilot `bash` and the
/// Codex shell tools.
const SHELL_TOOLS: &[&str] = &[
    "bash",
    "monitor",
    "shell",
    "exec_command",
    "shell_command",
    "powershell",
];

/// Devices that never reach end of file, so `tail -f` on them never ends.
const DEVICES: &[&str] = &["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"];

/// Reserved words that may precede the program word of a command.
const RESERVED_WORDS: &[&str] = &[
    "if", "then", "elif", "else", "do", "while", "until", "!", "{", "time",
];

/// Whether `tool_name` is a shell tool this guard checks.
pub(super) fn is_shell_tool(tool_name: &str) -> bool {
    SHELL_TOOLS
        .iter()
        .any(|name| name.eq_ignore_ascii_case(tool_name))
}

/// Check a shell tool's `tool_input`. The command is read from `command`
/// and `cmd`, each either a string or an argv array of strings; an array is
/// joined with shell quoting, so an argv word is never split again.
///
/// Returns `Some` only for a deny.
pub(super) fn check_tool_input(tool_input: &Value) -> Option<Value> {
    ["command", "cmd"]
        .iter()
        .filter_map(|key| tool_input.get(key))
        .find_map(|command| match command {
            Value::String(command) => check(command),
            Value::Array(argv) => {
                let words: Option<Vec<&str>> = argv.iter().map(Value::as_str).collect();
                words
                    .filter(|words| !words.is_empty())
                    .and_then(|words| check(&shell_words::join(words)))
            }
            _ => None,
        })
}

/// Check one shell command string. Returns `Some` only for a deny.
pub(super) fn check(command: &str) -> Option<Value> {
    let commands = tokenise(command)?;
    commands
        .iter()
        .any(|words| {
            program_index(words).is_some_and(|program| is_follow_on_device(words, program))
        })
        .then(deny_response)
}

/// The deny shape shared with the other PreToolUse guards (#1735, #1773):
/// the bare `block` fields for older hosts, and `permissionDecision` both at
/// the top level and in `hookSpecificOutput`, which Claude Code enforces.
/// Under the Codex protocol `codex_output()` keeps `hookSpecificOutput` only.
fn deny_response() -> Value {
    json!({
        "block": true,
        "message": BLOCK_MESSAGE,
        "decision": "block",
        "reason": BLOCK_MESSAGE,
        "permissionDecision": "deny",
        "permissionDecisionReason": BLOCK_MESSAGE,
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": BLOCK_MESSAGE,
        },
    })
}

// ------------------------------------------------------------- tokeniser

/// Split `command` into the word lists of the simple commands the shell
/// would run. Separators are `;`, `&`, `|`, newline, `(` and `)` (so `&&`
/// and `||` too). Quotes and backslashes are removed; heredoc bodies,
/// here-strings, redirect targets, file-descriptor numbers and comments are
/// dropped.
///
/// Returns `None` for an unbalanced quote. Each character is read once, so
/// the time is linear in the input.
fn tokenise(command: &str) -> Option<Vec<Vec<String>>> {
    Tokeniser {
        chars: command.chars().collect(),
        pos: 0,
        commands: Vec::new(),
        words: Vec::new(),
        heredocs: Vec::new(),
    }
    .run()
}

/// A heredoc whose body starts after the next unquoted newline.
struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
}

struct Tokeniser {
    chars: Vec<char>,
    pos: usize,
    commands: Vec<Vec<String>>,
    words: Vec<String>,
    heredocs: Vec<Heredoc>,
}

impl Tokeniser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos.checked_add(offset)?).copied()
    }

    fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    fn run(mut self) -> Option<Vec<Vec<String>>> {
        loop {
            self.skip_blanks();
            let Some(c) = self.peek() else { break };
            match c {
                '\n' => {
                    self.bump();
                    self.end_command();
                    self.skip_heredoc_bodies();
                }
                '#' => self.skip_comment(),
                '&' if self.peek_at(1) == Some('>') => {
                    // `&>` and `&>>`: both streams to a file.
                    self.bump();
                    self.redirect()?;
                }
                ';' | '&' | '|' | '(' | ')' => {
                    self.bump();
                    self.end_command();
                }
                '<' | '>' => self.redirect()?,
                _ => {
                    let word = self.word()?;
                    // `2>file`: an unquoted number right before a redirect is
                    // a file descriptor, not an argument.
                    let fd = word.unquoted_digits && matches!(self.peek(), Some('<' | '>'));
                    if !fd {
                        self.words.push(word.text);
                    }
                }
            }
        }
        self.end_command();
        Some(self.commands)
    }

    fn skip_blanks(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.bump();
        }
    }

    fn skip_comment(&mut self) {
        while self.peek().is_some_and(|c| c != '\n') {
            self.bump();
        }
    }

    fn end_command(&mut self) {
        if !self.words.is_empty() {
            self.commands.push(std::mem::take(&mut self.words));
        }
    }

    /// A redirect operator at `pos` (`<` or `>`): consume it and drop its
    /// target. `<<` and `<<-` register a heredoc; `<<<` drops a here-string.
    fn redirect(&mut self) -> Option<()> {
        if self.peek() == Some('<') && self.peek_at(1) == Some('<') {
            if self.peek_at(2) == Some('<') {
                self.pos = self.pos.saturating_add(3);
                self.target()?;
                return Some(());
            }
            self.pos = self.pos.saturating_add(2);
            let strip_tabs = self.peek() == Some('-');
            if strip_tabs {
                self.bump();
            }
            if let Some(delimiter) = self.target()? {
                self.heredocs.push(Heredoc {
                    delimiter,
                    strip_tabs,
                });
            }
            return Some(());
        }
        // `<`, `>`, then at most one of `>&|<` for `>>`, `>&`, `>|`, `<&`, `<>`.
        self.bump();
        if matches!(self.peek(), Some('>' | '&' | '|' | '<')) {
            self.bump();
        }
        self.target()?;
        Some(())
    }

    /// Read the word after a redirect operator, which is never a program
    /// word. `None` means an unbalanced quote; `Some(None)` means no word.
    fn target(&mut self) -> Option<Option<String>> {
        self.skip_blanks();
        match self.peek() {
            Some(c) if !is_metachar(c) => Some(Some(self.word()?.text)),
            _ => Some(None),
        }
    }

    /// Read one word, removing quotes and backslashes. Stops at an unquoted
    /// blank or metacharacter. `None` means an unbalanced quote.
    fn word(&mut self) -> Option<Word> {
        let mut text = String::new();
        let mut unquoted_digits = true;
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' => break,
                c if is_metachar(c) => break,
                '\'' => {
                    unquoted_digits = false;
                    self.bump();
                    loop {
                        match self.peek()? {
                            '\'' => break,
                            c => text.push(c),
                        }
                        self.bump();
                    }
                    self.bump();
                }
                '"' => {
                    unquoted_digits = false;
                    self.bump();
                    loop {
                        match self.peek()? {
                            '"' => break,
                            '\\' => match self.peek_at(1) {
                                Some(next @ ('"' | '\\' | '$' | '`')) => {
                                    text.push(next);
                                    self.bump();
                                }
                                Some('\n') => self.bump(),
                                _ => text.push('\\'),
                            },
                            '$' if self.peek_at(1) == Some('(') => {
                                self.substitution(&mut text)?;
                                continue;
                            }
                            c => text.push(c),
                        }
                        self.bump();
                    }
                    self.bump();
                }
                '\\' => {
                    unquoted_digits = false;
                    self.bump();
                    match self.peek() {
                        // Line continuation.
                        Some('\n') => self.bump(),
                        Some(c) => {
                            text.push(c);
                            self.bump();
                        }
                        None => {}
                    }
                }
                c => {
                    unquoted_digits &= c.is_ascii_digit();
                    text.push(c);
                    self.bump();
                }
            }
        }
        unquoted_digits &= !text.is_empty();
        Some(Word {
            text,
            unquoted_digits,
        })
    }

    /// Copy a `$(…)` met inside double quotes into `text`, through the
    /// matching `)`, so that a `"` inside it cannot end the outer quote. The
    /// contents are data and are not split into commands. Nested `$(` inside
    /// quotes within it is not tracked. `None` means the input ends first.
    fn substitution(&mut self, text: &mut String) -> Option<()> {
        text.push_str("$(");
        self.bump();
        self.bump();
        let mut depth = 1_usize;
        while depth > 0 {
            let c = self.peek()?;
            text.push(c);
            self.bump();
            match c {
                '\\' => {
                    text.push(self.peek()?);
                    self.bump();
                }
                '\'' | '"' => loop {
                    let q = self.peek()?;
                    text.push(q);
                    self.bump();
                    if q == c {
                        break;
                    }
                    if c == '"' && q == '\\' {
                        text.push(self.peek()?);
                        self.bump();
                    }
                },
                '(' => depth = depth.saturating_add(1),
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        Some(())
    }

    /// After an unquoted newline, drop the bodies of the heredocs opened on
    /// the line before, in order. An unterminated body runs to the end.
    fn skip_heredoc_bodies(&mut self) {
        for heredoc in std::mem::take(&mut self.heredocs) {
            while self.peek().is_some() {
                let start = self.pos;
                self.skip_comment();
                let line = self.chars.get(start..self.pos).unwrap_or_default();
                let line = if heredoc.strip_tabs {
                    let tabs = line.iter().take_while(|&&c| c == '\t').count();
                    line.get(tabs..).unwrap_or_default()
                } else {
                    line
                };
                let ends_body = line.iter().copied().eq(heredoc.delimiter.chars());
                if self.peek() == Some('\n') {
                    self.bump();
                }
                if ends_body {
                    break;
                }
            }
        }
    }
}

struct Word {
    text: String,
    /// Made only of digits, with no quoting: a candidate file descriptor.
    unquoted_digits: bool,
}

fn is_metachar(c: char) -> bool {
    matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>' | '\n')
}

// ---------------------------------------------------- program resolution

/// The index of the program word in `words`: past `VAR=value` assignments,
/// reserved words and the wrappers that run their arguments as a command.
fn program_index(words: &[String]) -> Option<usize> {
    let mut i = 0usize;
    loop {
        let word = words.get(i)?.as_str();
        if is_assignment(word) || RESERVED_WORDS.contains(&word) {
            i = i.checked_add(1)?;
            continue;
        }
        let next = i.checked_add(1)?;
        i = match basename(word) {
            "env" => skip_env(words, next),
            "nohup" => next,
            "exec" => skip_options(words, next, &['a'], &[]),
            "command" | "setsid" => skip_options(words, next, &[], &[]),
            "sudo" => skip_options(
                words,
                next,
                &['C', 'D', 'g', 'p', 'R', 'r', 't', 'T', 'U', 'u'],
                &[
                    "--close-from",
                    "--chdir",
                    "--group",
                    "--prompt",
                    "--chroot",
                    "--role",
                    "--type",
                    "--command-timeout",
                    "--other-user",
                    "--user",
                ],
            ),
            "nice" => skip_options(words, next, &['n'], &["--adjustment"]),
            "stdbuf" => skip_options(
                words,
                next,
                &['i', 'o', 'e'],
                &["--input", "--output", "--error"],
            ),
            // `timeout [options] DURATION COMMAND`: skip the duration too.
            "timeout" => skip_options(words, next, &['s', 'k'], &["--signal", "--kill-after"])
                .checked_add(1)?,
            _ => return Some(i),
        };
    }
}

/// `env [options] [NAME=value]... COMMAND`.
fn skip_env(words: &[String], start: usize) -> usize {
    let mut i = skip_options(
        words,
        start,
        &['u', 'C', 'S'],
        &["--unset", "--chdir", "--split-string"],
    );
    while words.get(i).is_some_and(|word| is_assignment(word)) {
        i = i.saturating_add(1);
    }
    i
}

/// Skip the options of a wrapper starting at `start`, up to and including
/// `--`. Short options in `short_values` and long options in `long_values`
/// take a value, either attached (`-n5`, `--signal=KILL`) or as the next
/// word. Returns the index of the first word that is not an option.
fn skip_options(
    words: &[String],
    start: usize,
    short_values: &[char],
    long_values: &[&str],
) -> usize {
    let mut i = start;
    while let Some(word) = words.get(i) {
        i = i.saturating_add(1);
        if word == "--" {
            break;
        }
        if let Some(long) = word.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or_default();
            if !long.contains('=') && long_values.iter().any(|v| v.get(2..) == Some(name)) {
                i = i.saturating_add(1);
            }
            continue;
        }
        let Some(cluster) = word.strip_prefix('-').filter(|c| !c.is_empty()) else {
            // Not an option: this is the first word after the options.
            return i.saturating_sub(1);
        };
        let mut chars = cluster.chars();
        while let Some(c) = chars.next() {
            if short_values.contains(&c) {
                if chars.as_str().is_empty() {
                    i = i.saturating_add(1);
                }
                break;
            }
        }
    }
    i
}

/// `NAME=value` with a valid shell variable name.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

// ------------------------------------------------------- tail arguments

/// Whether the command at `program` is `tail` with a follow flag and an
/// operand in [`DEVICES`].
fn is_follow_on_device(words: &[String], program: usize) -> bool {
    if words.get(program).map(|word| basename(word)) != Some("tail") {
        return false;
    }
    let mut follow = false;
    let mut device = false;
    let mut operands_only = false;
    let mut args = words.iter().skip(program.saturating_add(1));
    while let Some(word) = args.next() {
        let word = word.as_str();
        if operands_only || word == "-" || !word.starts_with('-') {
            device |= DEVICES.contains(&word);
        } else if word == "--" {
            operands_only = true;
        } else if let Some(long) = word.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, false),
            };
            match name {
                "follow" => follow = true,
                "lines" | "bytes" | "sleep-interval" | "pid" | "max-unchanged-stats" if !value => {
                    args.next();
                }
                _ => {}
            }
        } else {
            // A short cluster, read left to right: `n`, `c` and `s` take
            // the rest of the cluster, or the next word, as their value.
            let mut chars = word.chars().skip(1);
            while let Some(c) = chars.next() {
                match c {
                    'f' | 'F' => follow = true,
                    'n' | 'c' | 's' => {
                        if chars.next().is_none() {
                            args.next();
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    follow && device
}

#[cfg(test)]
#[path = "tests/tail_guard_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;
