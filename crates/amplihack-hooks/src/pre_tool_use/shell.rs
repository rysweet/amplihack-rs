//! The small shell tokeniser shared by the PreToolUse command guards: the
//! auto-drive merge guard (#1551) and the tmux server guard (#1733).
//!
//! This is not a shell parser and does not try to be one. It splits a command
//! line into simple commands of unquoted words, finds each command's program
//! word, and keeps the heredoc bodies each command receives so a guard can
//! decide whether they are data or code.

/// Programs that run the command after them: `(name, options that take a
/// value, positional arguments before the command)`.
pub(super) const WRAPPERS: &[(&str, &[&str], usize)] = &[
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
pub(super) const RESERVED: &[&str] = &[
    "!", "{", "}", "if", "then", "elif", "else", "while", "until", "do",
];

/// One simple command: its unquoted words, the heredoc bodies it receives and
/// the pipeline it belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SimpleCommand {
    pub(super) words: Vec<String>,
    /// Bodies of the heredocs whose `<<` appeared in this command, in order.
    pub(super) heredocs: Vec<String>,
    /// For each body in `heredocs`, whether its delimiter was unquoted, so
    /// the shell expands `$(…)` and backticks inside it.
    pub(super) heredoc_expands: Vec<bool>,
    /// Commands joined by a lone `|` (or `|&`) share an index; every other
    /// separator starts a new one.
    pub(super) pipeline: usize,
}

/// The result of tokenising a command line.
#[derive(Debug, Default)]
pub(super) struct Parsed {
    pub(super) commands: Vec<SimpleCommand>,
    /// False when the text has an unclosed quote, `$'…'`, `$(…)` or
    /// backtick, or a `<<` with no delimiter: the words are a guess.
    pub(super) complete: bool,
}

/// The words from the program being run onward: reserved words, `VAR=value`
/// assignments and [`WRAPPERS`] (with their options) are skipped.
pub(super) fn program(words: &[String]) -> Option<&[String]> {
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

pub(super) fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

pub(super) fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Splits `text` into simple commands, each a list of unquoted words.
///
/// Commands end at `;`, `&`, `|`, `(`, `)` and newlines outside quotes.
/// Quotes and backslashes are removed from words. Comments are dropped, and
/// heredoc bodies are skipped: they are data, never a command's words.
/// `$(…)` and backticks stay inside the word they appear in, including a
/// `$(…)` inside double quotes, whose own quotes do not end the string.
pub(super) fn commands(text: &str) -> Vec<Vec<String>> {
    parse(text)
        .into_iter()
        .map(|c| c.words)
        .filter(|w| !w.is_empty())
        .collect()
}

/// [`commands`] with each command's heredoc bodies and pipeline index.
pub(super) fn parse(text: &str) -> Vec<SimpleCommand> {
    tokenise(text).commands
}

/// [`parse`], plus whether the text was well formed.
pub(super) fn tokenise(text: &str) -> Parsed {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![SimpleCommand::default()];
    let mut pipeline = 0usize;
    let mut complete = true;
    let mut word = String::new();
    let mut in_word = false;
    // Pending heredocs on this line: (delimiter, strip tabs, expands, owner).
    let mut heredocs: Vec<(String, bool, bool, usize)> = Vec::new();
    let mut i = 0;
    fn end_word(word: &mut String, in_word: &mut bool, out: &mut [SimpleCommand]) {
        if std::mem::take(in_word)
            && let Some(last) = out.last_mut()
        {
            last.words.push(std::mem::take(word));
        }
    }
    let new_command = |out: &mut Vec<SimpleCommand>, pipeline: usize| {
        out.push(SimpleCommand {
            pipeline,
            ..SimpleCommand::default()
        });
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
                if i >= chars.len() {
                    complete = false;
                }
            }
            '$' if chars.get(i + 1) == Some(&'\'') => {
                // `$'…'` (ANSI-C quoting): `\'` inside is an escaped quote,
                // not the end of the string (bash manual, ANSI-C Quoting).
                in_word = true;
                let close = ansi_c_close(&chars, i + 1);
                complete &= close.is_some();
                let end = close.unwrap_or(chars.len().saturating_sub(1));
                let mut j = i + 2;
                while j < end {
                    if chars[j] == '\\' && j + 1 < end {
                        j += 1;
                    }
                    word.push(chars[j]);
                    j += 1;
                }
                i = end;
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
                        // `"$(…)"`: quotes inside the substitution are its
                        // own, not the end of this string (bash manual,
                        // Command Substitution).
                        let close = subst_close(&chars, i);
                        complete &= close.is_some();
                        let end = close.unwrap_or(chars.len() - 1);
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
                if i >= chars.len() {
                    complete = false;
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
                let close = subst_close(&chars, i);
                complete &= close.is_some();
                let end = close.unwrap_or(chars.len() - 1);
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
                if i >= chars.len() {
                    complete = false;
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
                // `<<WORD` / `<<-WORD`: remember the delimiter and the
                // command it belongs to; the body is read at the end of
                // this line.
                end_word(&mut word, &mut in_word, &mut out);
                let expands = !delim_quoted(&chars, i);
                let (delim, strip_tabs) = heredoc_delim(&chars, &mut i);
                if delim.is_empty() {
                    complete = false;
                }
                heredocs.push((delim, strip_tabs, expands, out.len() - 1));
                continue;
            }
            '\n' => {
                end_word(&mut word, &mut in_word, &mut out);
                pipeline += 1;
                new_command(&mut out, pipeline);
                let pending = std::mem::take(&mut heredocs);
                let specs = pending.iter().map(|(d, s, _, _)| (d.clone(), *s)).collect();
                let bodies = read_heredoc_bodies(&chars, &mut i, specs);
                for ((_, _, expands, owner), body) in pending.into_iter().zip(bodies) {
                    if let Some(cmd) = out.get_mut(owner) {
                        cmd.heredocs.push(body);
                        cmd.heredoc_expands.push(expands);
                    }
                }
            }
            '|' if chars.get(i + 1) != Some(&'|') => {
                // A lone `|` (or `|&`) joins the next command to this
                // pipeline.
                end_word(&mut word, &mut in_word, &mut out);
                if chars.get(i + 1) == Some(&'&') {
                    i += 1;
                }
                new_command(&mut out, pipeline);
            }
            '|' => {
                end_word(&mut word, &mut in_word, &mut out);
                i += 1;
                pipeline += 1;
                new_command(&mut out, pipeline);
            }
            ';' | '&' | '(' | ')' => {
                end_word(&mut word, &mut in_word, &mut out);
                pipeline += 1;
                new_command(&mut out, pipeline);
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
    // A heredoc whose line never ended has no body.
    for (_, _, expands, owner) in heredocs {
        if let Some(cmd) = out.get_mut(owner) {
            cmd.heredocs.push(String::new());
            cmd.heredoc_expands.push(expands);
        }
    }
    out.retain(|c| !c.words.is_empty() || !c.heredocs.is_empty());
    Parsed {
        commands: out,
        complete,
    }
}

/// The index of the `)` closing the `$(` at `start`, or `None` when it is
/// unclosed. Everything inside is data to the tokeniser; the scan only keeps
/// quoted text and heredoc bodies from ending it early, so a `)` or `"` in a
/// commit message body does not close the substitution.
///
/// Iterative, with an explicit stack of open substitutions and double-quoted
/// strings, so deeply nested input cannot overflow the thread's stack.
pub(super) fn subst_close(chars: &[char], start: usize) -> Option<usize> {
    enum Frame {
        Subst {
            depth: usize,
            heredocs: Vec<(String, bool)>,
        },
        Quote,
    }
    let mut stack = vec![Frame::Subst {
        depth: 0,
        heredocs: Vec::new(),
    }];
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        match stack.last_mut() {
            None => return None,
            Some(Frame::Quote) => match c {
                '"' => {
                    stack.pop();
                }
                '\\' => i += 1,
                '$' if chars.get(i + 1) == Some(&'(') => stack.push(Frame::Subst {
                    depth: 0,
                    heredocs: Vec::new(),
                }),
                _ => {}
            },
            Some(Frame::Subst { depth, heredocs }) => match c {
                '(' => *depth += 1,
                ')' => {
                    *depth = depth.saturating_sub(1);
                    if *depth == 0 {
                        stack.pop();
                        if stack.is_empty() {
                            return Some(i);
                        }
                    }
                }
                '\\' => i += 1,
                '$' if chars.get(i + 1) == Some(&'\'') => {
                    i = ansi_c_close(chars, i + 1)?;
                }
                '\'' => {
                    i += 1;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                }
                '"' => stack.push(Frame::Quote),
                '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) == Some(&'<') => i += 2,
                '<' if chars.get(i + 1) == Some(&'<') => {
                    heredocs.push(heredoc_delim(chars, &mut i));
                    continue;
                }
                '\n' => skip_heredoc_bodies(chars, &mut i, std::mem::take(heredocs)),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// The index of the `'` closing the `$'…'` string whose opening `'` is at
/// `open`, or `None` when it is unclosed. A backslash escapes the next
/// character, so `\'` does not close it.
pub(super) fn ansi_c_close(chars: &[char], open: usize) -> Option<usize> {
    let mut i = open + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '\'' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether the delimiter of the `<<` at `at` is quoted or escaped, which
/// stops the shell expanding the body.
fn delim_quoted(chars: &[char], at: usize) -> bool {
    let mut i = at + 2;
    if chars.get(i) == Some(&'-') {
        i += 1;
    }
    while chars.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
        i += 1;
    }
    chars
        .get(i..)
        .unwrap_or_default()
        .iter()
        .take_while(|d| !d.is_whitespace() && !";&|()<>".contains(**d))
        .any(|d| "'\"\\".contains(*d))
}

/// Reads the delimiter of the `<<WORD` / `<<-WORD` at `*i`, leaving `*i` just
/// past it. Returns the delimiter (quotes removed) and whether `-` strips tabs.
pub(super) fn heredoc_delim(chars: &[char], i: &mut usize) -> (String, bool) {
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
pub(super) fn skip_heredoc_bodies(chars: &[char], i: &mut usize, heredocs: Vec<(String, bool)>) {
    read_heredoc_bodies(chars, i, heredocs);
}

/// [`skip_heredoc_bodies`], returning each body (lines joined by `\n`, with
/// leading tabs stripped for `<<-`). An unterminated body runs to the end.
pub(super) fn read_heredoc_bodies(
    chars: &[char],
    i: &mut usize,
    heredocs: Vec<(String, bool)>,
) -> Vec<String> {
    let mut bodies = Vec::with_capacity(heredocs.len());
    for (delim, strip_tabs) in heredocs {
        let mut body: Vec<String> = Vec::new();
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
                line.trim_start_matches('\t').to_string()
            } else {
                line
            };
            if line == delim {
                break;
            }
            body.push(line);
        }
        bodies.push(body.join("\n"));
    }
    bodies
}

#[cfg(test)]
#[path = "tests/shell_tests.rs"]
mod tests;
