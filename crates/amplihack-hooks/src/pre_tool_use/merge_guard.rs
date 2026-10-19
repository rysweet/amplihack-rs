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
//! Bash tool, so this hook never sees a legitimate run of it. An agent
//! *running* the gate is therefore refused (crusty round 2): the gate trusts
//! its Crusty and QA record files, which an agent can write. Only a run is
//! refused (crusty round 3): the gate as the command word, after `VAR=…` and
//! wrappers such as `env`, `exec`, `xargs`, `timeout`, `nohup` or `sudo`, or
//! as the script operand of a shell or of `source`/`.`. Naming it as an
//! argument, as `cat`, `git show HEAD:…`, `git diff -- …`, `git add`, `grep`
//! and `rg` do, passes.
//!
//! When the command also runs an interpreter (a shell, Python, Perl, Ruby,
//! Node, awk, …) in command position, quoted text is code that will run, not
//! prose, so every word is read as a command too and scanned for endpoints,
//! mutations and the gate's name. That catches `echo 'gh pr merge 38' | bash`,
//! `bash -s <<<'…'`, `python3 -c "os.system('gh pr merge 38')"` and
//! `cat <gate> | bash`, while `git commit -m '… gh pr merge …'` and `rg 'gh pr
//! merge' --type sh`, where no interpreter is the program being run, still
//! pass. Variable indirection (`G=gh; $G pr merge`) and script files are not
//! readable from the command text and are left to the shim and the `MERGED`
//! checks.
//!
//! A heredoc body (`<<WORD`, `<<-WORD`, `<<'WORD'`) is code unless every
//! program that can receive it is a known data consumer (crusty round 5):
//! `cat`, `tee`, `git` (without an `alias.` setting), `gh` (except `gh
//! api`), `jq`, `wc`, `paste` and the other text tools in
//! [`reads_heredoc_as_data`], with nothing in the command that runs code.
//! So `git commit -F - <<'EOF'`, `git commit -m "$(cat <<'EOF' … EOF)"` and
//! `gh pr create --body "$(cat <<'EOF' … EOF)"` pass whatever prose the body
//! holds (crusty round 4), while `bash <<'EOF'`, `cat <<'EOF' | bash`,
//! `source /dev/stdin <<'EOF'`, `eval "$(cat <<'EOF' … EOF)"`, `parallel`,
//! `at`, `ssh` and any program not on the list read it as commands. The rule
//! is deny by default: a program added to a toolchain later is code until it
//! is listed. The check is per command (every simple command at that level,
//! and every command whose arguments a `$(…)` feeds), not per pipeline, so
//! prose sent to `git` in the same tool call as `cargo test` is read as code;
//! the refusal says to split the call. The `$(…)`/backtick substitutions of
//! an unquoted body, which bash runs, are always checked as commands.

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
PATH finds. Do not retry with another gh path, PATH, alias, API client or token.\n\
\n\
If the match is prose (a commit message, PR body or doc) and you meant no merge:\n\
the command also runs an interpreter, or a heredoc reaches a program not known\n\
to treat it as data, so its text was read as code. Write the prose in a separate\n\
tool call whose only programs are git, gh, cat or tee (git commit -F - <<'EOF'\n\
… EOF passes); keep the prose as it is.";

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
    let label = merge_form(command, 0, false, true)?;
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
/// `heredoc_data` is cleared once any level runs a program that is not a
/// known data consumer; from then on heredoc bodies are read as code.
fn merge_form(
    command: &str,
    depth: usize,
    scripted: bool,
    heredoc_data: bool,
) -> Option<&'static str> {
    if depth > MAX_NESTING {
        return None;
    }
    let Split {
        segments,
        nested,
        heredocs,
    } = split(command);
    let scripted = scripted
        || segments
            .iter()
            .flat_map(|words| command_words(words))
            .any(|i| is_interpreter(basename(&i.1)));
    // Deny by default: a body is data only when every program here (and in
    // the command whose arguments this one's output becomes) is a known
    // data consumer and nothing runs code.
    let heredoc_data =
        heredoc_data && !scripted && segments.iter().all(|w| reads_heredoc_as_data(w));
    for nested_cmd in &nested {
        if let Some(label) = merge_form(nested_cmd, depth + 1, scripted, heredoc_data) {
            return Some(label);
        }
    }
    if !heredoc_data {
        for body in &heredocs {
            if let Some(label) = merge_form(body, depth + 1, true, false) {
                return Some(label);
            }
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
/// or a pipe on stdin. `eval`, `source` and `.` run their input as shell code
/// too (crusty round 5).
fn is_interpreter(name: &str) -> bool {
    matches!(
        name,
        "eval"
            | "source"
            | "."
            | "bash"
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

/// Whether the simple command `words` treats a heredoc on its stdin, or the
/// output of a `$(…)` in its arguments, as data: a bare assignment, or every
/// program it runs is on this allowlist. Anything else (an unknown program,
/// `parallel`, `at`, `ssh`, `script`, `expect`, a wrapper with no command
/// left to find, as in `env -S '…'`) is code. `sed` and `awk` are left off:
/// GNU sed's `e` command and any awk script run shell commands.
fn reads_heredoc_as_data(words: &[String]) -> bool {
    if words.iter().all(|w| is_assignment(w)) {
        return true;
    }
    let programs = command_words(words);
    !programs.is_empty()
        && programs.iter().all(|(i, word)| {
            let args = &words[i + 1..];
            match basename(word) {
                // `gh api --input -` sends the body as a request.
                "gh" => skip_flags(args).first().map(String::as_str) != Some("api"),
                // A `!` alias runs a shell command.
                "git" => !args.iter().any(|a| a.contains("alias.")),
                name => matches!(
                    name,
                    "cat"
                        | "tee"
                        | "jq"
                        | "wc"
                        | "head"
                        | "tail"
                        | "grep"
                        | "rg"
                        | "diff"
                        | "cmp"
                        | "sort"
                        | "uniq"
                        | "tr"
                        | "cut"
                        | "paste"
                        | "fold"
                        | "fmt"
                        | "nl"
                        | "base64"
                        | "sha256sum"
                        | "md5sum"
                        | "echo"
                        | "printf"
                        | "cd"
                        | "mkdir"
                        | "ls"
                        | "pwd"
                        | "true"
                        | "false"
                ),
            }
        })
}

/// The words of `words` that run as programs, with their indices: the first
/// word after `VAR=…` assignments and wrappers (`env`, `command`, `exec`,
/// `xargs`, `timeout`, `nohup`, `sudo`, …, each with its flags and operands),
/// and the same after each `find -exec`/`-execdir`/`-ok`/`-okdir`.
fn command_words(words: &[String]) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut starts = vec![0];
    starts.extend(
        words
            .iter()
            .enumerate()
            .filter(|(_, w)| matches!(w.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
            .map(|(i, _)| i + 1),
    );
    for start in starts {
        if let Some(i) = command_index(words, start) {
            found.push((i, words[i].clone()));
        }
    }
    found
}

/// The index of the program word of the simple command starting at `start`.
fn command_index(words: &[String], start: usize) -> Option<usize> {
    let mut i = start;
    loop {
        let word = words.get(i)?;
        if is_assignment(word) {
            i += 1;
            continue;
        }
        let Some((value_flags, operands)) = wrapper(basename(word)) else {
            return Some(i);
        };
        i += 1;
        while let Some(arg) = words.get(i) {
            if arg == "--" {
                i += 1;
                break;
            }
            if arg.len() > 1 && arg.starts_with('-') {
                i += if value_flags.contains(&arg.as_str()) {
                    2
                } else {
                    1
                };
            } else if is_assignment(arg) {
                i += 1;
            } else {
                break;
            }
        }
        i += operands;
    }
}

/// `NAME=value`, a shell assignment or an `env` operand.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Programs that run the command after them: the flags that take a separate
/// value, and how many operands come before the command.
fn wrapper(name: &str) -> Option<(&'static [&'static str], usize)> {
    Some(match name {
        "env" => (
            &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"],
            0,
        ),
        "command" | "builtin" | "nohup" | "setsid" | "unbuffer" => (&[], 0),
        "exec" => (&["-a"], 0),
        "xargs" => (
            &[
                "-I",
                "-L",
                "-n",
                "-P",
                "-s",
                "-d",
                "-E",
                "-a",
                "--arg-file",
                "--delimiter",
            ],
            0,
        ),
        "timeout" => (&["-s", "--signal", "-k", "--kill-after"], 1),
        "sudo" | "doas" => (
            &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U", "-T"],
            0,
        ),
        "nice" => (&["-n", "--adjustment"], 0),
        "ionice" => (&["-c", "-n", "-p"], 0),
        "stdbuf" => (&["-i", "-o", "-e"], 0),
        "time" => (&["-f", "-o", "--format", "--output"], 0),
        "flock" => (&["-w", "--timeout", "-E", "--conflict-exit-code"], 1),
        "chrt" | "taskset" => (&[], 1),
        _ => return None,
    })
}

/// Whether the simple command `words` runs the merge gate: as its program,
/// or as the script of a shell or of `source`/`.`.
fn runs_gate(words: &[String]) -> bool {
    command_words(words).into_iter().any(|(i, word)| {
        let name = basename(&word);
        if name == GATE_NAME {
            return true;
        }
        if !matches!(
            name,
            "bash" | "sh" | "zsh" | "dash" | "ksh" | "mksh" | "source" | "."
        ) {
            return false;
        }
        let mut args = words[i + 1..].iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                return args.next().is_some_and(|a| basename(a) == GATE_NAME);
            }
            if arg.len() > 1 && (arg.starts_with('-') || arg.starts_with('+')) {
                if !arg.starts_with("--") && arg[1..].contains('c') {
                    // `bash -c '<script>'`: the script is checked as a command.
                    return false;
                }
                if matches!(arg.as_str(), "-o" | "+o" | "-O" | "+O") {
                    args.next();
                }
                continue;
            }
            return basename(arg) == GATE_NAME;
        }
        false
    })
}

const GATE_LABEL: &str = "a run of the merge gate autodrive_merge_gate.sh by an agent";

fn segment_merge_form(words: &[String], depth: usize, scripted: bool) -> Option<&'static str> {
    if runs_gate(words) {
        return Some(GATE_LABEL);
    }
    for (i, word) in words.iter().enumerate() {
        let rest = &words[i + 1..];
        // With an interpreter running, the gate named anywhere may be code
        // that runs it (`cat <gate> | bash`, `subprocess.run(['<gate>'])`).
        if scripted && basename(word) == GATE_NAME {
            return Some(GATE_LABEL);
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
                shell_c_body(rest).and_then(|body| merge_form(body, depth + 1, scripted, false))
            }
            "eval" => merge_form(&rest.join(" "), depth + 1, scripted, false),
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
        && let Some(label) = merge_form(&code, depth + 1, true, false)
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

/// What [`split`] reads from a command.
struct Split {
    /// Simple commands, as word lists.
    segments: Vec<Vec<String>>,
    /// Bodies of `$(…)` and backticks, to be checked as commands.
    nested: Vec<String>,
    /// Heredoc bodies: code unless every program in the command is a known
    /// data consumer (see [`reads_heredoc_as_data`]).
    heredocs: Vec<String>,
}

/// A heredoc redirection waiting for the end of its line.
struct Heredoc {
    delimiter: String,
    /// `<<-`: leading tabs are stripped before matching the delimiter.
    strip_tabs: bool,
    /// An unquoted delimiter: bash expands `$(…)` and backticks in the body.
    expands: bool,
}

/// The heredoc redirection whose `<<` starts at `i`, and the index after its
/// delimiter word. `None` for a here-string (`<<<`), and for a delimiter
/// that does not start with a letter, `_`, a quote or a backslash, so the
/// shift in `$((1<<2))` is not read as one.
fn heredoc_at(chars: &[char], i: usize) -> Option<(Heredoc, usize)> {
    if chars.get(i + 1) != Some(&'<')
        || chars.get(i + 2) == Some(&'<')
        || (i > 0 && chars[i - 1] == '<')
    {
        return None;
    }
    let mut j = i + 2;
    let strip_tabs = chars.get(j) == Some(&'-');
    if strip_tabs {
        j += 1;
    }
    while matches!(chars.get(j), Some(' ' | '\t')) {
        j += 1;
    }
    let first = *chars.get(j)?;
    if !(first.is_ascii_alphabetic() || matches!(first, '_' | '\'' | '"' | '\\')) {
        return None;
    }
    let mut delimiter = String::new();
    let mut quoted = false;
    while let Some(&c) = chars.get(j) {
        match c {
            '\'' | '"' => {
                quoted = true;
                j += 1;
                while let Some(&d) = chars.get(j) {
                    j += 1;
                    if d == c {
                        break;
                    }
                    delimiter.push(d);
                }
            }
            '\\' => {
                quoted = true;
                if let Some(&d) = chars.get(j + 1) {
                    delimiter.push(d);
                }
                j += 2;
            }
            c if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')') => {
                break;
            }
            c => {
                delimiter.push(c);
                j += 1;
            }
        }
    }
    (!delimiter.is_empty()).then_some((
        Heredoc {
            delimiter,
            strip_tabs,
            expands: !quoted,
        },
        j,
    ))
}

/// The body of `doc` starting at `start` (just after the newline that ends
/// the redirection's line), and the index after its delimiter line. `None`
/// when no line closes it: the text is then read as commands, as before.
fn heredoc_body(chars: &[char], start: usize, doc: &Heredoc) -> Option<(String, usize)> {
    let mut i = start;
    let mut body = String::new();
    while i < chars.len() {
        let end = chars[i..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(chars.len(), |p| i + p);
        let line: String = chars[i..end].iter().collect();
        let compared = if doc.strip_tabs {
            line.trim_start_matches('\t')
        } else {
            line.as_str()
        };
        if compared == doc.delimiter {
            return Some((body, (end + 1).min(chars.len())));
        }
        body.push_str(&line);
        body.push('\n');
        i = end + 1;
    }
    None
}

/// Consumes the bodies of `pending` heredocs starting at `start`, and
/// returns the index after the last one closed.
fn take_heredocs(
    chars: &[char],
    start: usize,
    pending: &mut Vec<Heredoc>,
    mut on_body: impl FnMut(String, &Heredoc),
) -> usize {
    let mut i = start;
    for doc in pending.drain(..) {
        if let Some((body, end)) = heredoc_body(chars, i, &doc) {
            on_body(body, &doc);
            i = end;
        }
    }
    i
}

/// The `$(…)` and backtick bodies of an unquoted heredoc body, which bash
/// runs while reading it. Quotes are literal there, so they hide nothing.
fn heredoc_substitutions(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '$' if chars.get(i + 1) == Some(&'(') => {
                let (inner, end) = substitution(&chars, i + 2);
                found.push(inner);
                i = end;
            }
            '`' => {
                let (inner, end) = backtick(&chars, i + 1);
                found.push(inner);
                i = end;
            }
            _ => i += 1,
        }
    }
    found
}

/// Splits `command` into simple commands (word lists) at unquoted `;`, `&`,
/// `|`, newlines, parentheses and redirections, honouring quotes and
/// backslashes. Bodies of `$(…)` and backticks, quoted or not, are returned
/// separately to be checked as commands of their own. Heredoc bodies are
/// returned separately too; [`merge_form`] reads them as data only when
/// every program that can receive them is a known data consumer.
///
/// This is a reader for the commands agents write, not a shell: anything it
/// misreads still meets the `gh` shim, and branch protection after that.
fn split(command: &str) -> Split {
    let chars: Vec<char> = command.chars().collect();
    let mut segments = Vec::new();
    let mut nested = Vec::new();
    let mut heredocs = Vec::new();
    let mut pending: Vec<Heredoc> = Vec::new();
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
                if c == '\n' && !pending.is_empty() {
                    i = take_heredocs(&chars, i, &mut pending, |body, doc| {
                        if doc.expands {
                            nested.extend(heredoc_substitutions(&body));
                        }
                        heredocs.push(body);
                    });
                }
            }
            '<' if let Some((doc, end)) = heredoc_at(&chars, i) => {
                end_word(&mut words, &mut word, &mut in_word);
                pending.push(doc);
                i = end;
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
    Split {
        segments,
        nested,
        heredocs,
    }
}

/// The body of a `$(` starting at `start`, up to its matching `)`, and the
/// index after it. Quotes inside are skipped so their parentheses do not
/// count, and so are heredoc bodies, whose apostrophes and parentheses are
/// prose (`$(cat <<'EOF' … don't (ever) … EOF)`); `$((…))` arithmetic has no
/// heredocs. An unterminated body runs to the end.
fn substitution(chars: &[char], start: usize) -> (String, usize) {
    let mut depth = 1usize;
    let mut i = start;
    let mut quote: Option<char> = None;
    let mut pending: Vec<Heredoc> = Vec::new();
    let arithmetic = chars.get(start) == Some(&'(');
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '<' if !arithmetic && let Some((doc, end)) = heredoc_at(chars, i) => {
                    pending.push(doc);
                    i = end;
                    continue;
                }
                '\n' if !pending.is_empty() => {
                    i = take_heredocs(chars, i + 1, &mut pending, |_, _| {});
                    continue;
                }
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
