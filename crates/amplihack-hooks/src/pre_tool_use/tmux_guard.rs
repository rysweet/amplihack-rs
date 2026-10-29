//! Issue #1733: deny a shell command that would kill a whole tmux server, or
//! every session on it, unless the tmux invocation names a private socket.
//!
//! When `$TMUX` is set, as it is inside any tmux pane, tmux uses the socket
//! named in `$TMUX` whenever no `-S` or `-L` is given, and ignores
//! `TMUX_TMPDIR`. An agent in a pane that ran
//! `TMUX_TMPDIR=$SB/tmuxsock tmux kill-server` killed the user's own server
//! and every session on it.
//!
//! Denied: `kill-server` (and its prefixes `kill-s` … `kill-serve`) and
//! `kill-session -a` (and the prefixes of `kill-session`), anywhere in a tmux
//! command sequence, unless `-S <path>` or `-L <name>` appears among tmux's
//! global options. `-L default`, an empty `-S`/`-L` value, a value that
//! refers to `$TMUX` or to a variable assigned from it anywhere in the
//! command (`S=$TMUX; tmux -S "$S" …`), and a socket path ending in
//! `default` are not scope.
//!
//! tmux is found three ways, using the shared tokeniser in [`super::shell`]:
//!
//! 1. as the program word of a simple command;
//! 2. as a later word of another command (`xargs tmux …`, `ssh host tmux …`);
//! 3. inside a word that holds both a `tmux` and a `kill-s` token
//!    (`bash -c '…'`, `python -c "…"`, `$(…)`, backticks), parsed again up
//!    to [`MAX_DEPTH`] levels.
//!
//! Redirections never count as tmux arguments (`tmux 2>/dev/null
//! kill-server` is denied), and `$'…'` escapes are decoded first
//! (`tmux $'\x6bill-server'` is denied); both are done by the tokeniser.
//!
//! Layers 2 and 3 exempt data-only commands such as
//! `git commit -m "… tmux kill-server …"`. A git or gh command counts as an
//! evaluator instead when it can run or store shell text: a git subcommand in
//! [`GIT_COMMAND_RUNNERS`] (`config` and `clone` included, for `!` aliases,
//! `core.pager` and `--upload-pack`), `gh alias`, or an environment
//! assignment before the program (`GIT_EXTERNAL_DIFF=… git diff`).
//! Heredoc bodies are parsed when a shell evaluator appears anywhere on the
//! line, so `cat > k.sh <<EOF … EOF` followed by `bash k.sh` is denied.
//! Uncertain input fails closed: unbalanced quotes, a redirection with no
//! target, or hitting [`MAX_DEPTH`] or [`MAX_REPARSES`] with a candidate
//! left deny. So does an unscoped tmux whose subcommand word holds a `$` or
//! a backtick (`c=kill-server; tmux $c`, `tmux $(echo kill-server)`), since
//! the expansion may name `kill-server`; expansions in option values
//! (`tmux new -s "$name"`) and in scoped invocations are allowed.
//!
//! This is a guard rail against agent mistakes, not a sandbox. It does not
//! cover:
//!
//! - shell aliases or functions, script files (`bash script.sh` for a file
//!   written earlier), or commands assembled at run time from pieces;
//! - `tmux source-file` (or `source`) reading its commands from a file,
//!   stdin, a heredoc or a here-string, unless a `kill-s` word sits in the
//!   same pipeline as the unscoped tmux;
//! - an `-S` path that is a symlink to the default socket, which only a
//!   file system lookup could see;
//! - a variable copied from `$TMUX` other than by assignment (`read`,
//!   `printf -v`, `declare -n`), or in an earlier tool call;
//! - input over [`MAX_INPUT`] bytes, which gets only a plain text check for
//!   `tmux` and `kill-s` (so quoting tricks such as `t''mux` get past it),
//!   not the full parse;
//! - named-fd redirections (`{fd}>file`), whose `{fd}` is read as a word;
//! - `tmux kill-session` without `-a`, `kill-window`, `kill-pane`, and
//!   signals sent by other programs (`pkill tmux`, `killall tmux`);
//! - shell text stored by an earlier tool call and run by a later one, such
//!   as a git alias or pager set in one call and used in the next.
//!
//! It also errs toward false positives: a [`SHELL_EVALUATORS`] word anywhere
//! on the line (`tmux`, `find`, `docker`, `kubectl`, …) removes the data-only
//! exemption for the whole line, so `tmux ls; git commit -m "… tmux
//! kill-server …"` is denied. Agents should put such commands in a separate
//! tool call. Quoting is removed before the check, so a single-quoted
//! `$(tmux kill-server)` in a commit message is denied as well.
//!
//! The guard always runs and reads no environment variables, so nothing in
//! the environment can turn it off. It never panics on any input: the hook
//! fails open on Claude Code and Copilot, so a panic would allow the command.
//! For the same reason a harness that times out the hook, for example on a
//! cold start, lets the command run; the guard's own cost is bounded by
//! [`MAX_INPUT`] and [`MAX_REPARSES`].

use super::shell::{self, SimpleCommand, basename, program};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tracing::warn;

/// The deny reason. The command itself is never echoed.
pub(super) const BLOCK_MESSAGE: &str = "Blocked: this tmux command would act on the tmux server named in $TMUX (normally the user's own server, with all of their sessions), because no -S or -L was given. TMUX_TMPDIR alone does not redirect tmux while $TMUX is set. Use `tmux -S <private socket> …`, or `env -u TMUX tmux -S <private socket> …`, and only for a server you started on that socket.";

/// Tool names the guard checks, compared case-insensitively: Claude Code's
/// `Bash` and `PowerShell`, Copilot's `bash` and `powershell`, and Codex's
/// shell tools in case a host skips the Codex normalisation.
pub(super) const SHELL_TOOLS: &[&str] = &[
    "bash",
    "powershell",
    "shell",
    "exec_command",
    "shell_command",
];

/// Programs whose arguments are data: a `tmux … kill-server` in them is text,
/// as long as no stage of the command line evaluates text as code.
const DATA_PROGRAMS: &[&str] = &[
    "git", "gh", "echo", "printf", "grep", "egrep", "fgrep", "rg", "cat", "head", "tail", "wc",
    "jq", "test", "[", "true", ":",
];

/// Programs that run text, or another command, as code.
const SHELL_EVALUATORS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "fish", "eval", "source", ".", "xargs", "nohup", "exec",
    "parallel", "watch", "script", "su", "sudo", "ssh", "docker", "kubectl", "find", "tmux",
    "setsid", "python", "python3", "node", "perl", "ruby",
];

/// git subcommands that run, or store for a later git command to run, shell
/// text given as arguments (`rebase -x`, `bisect run`, `submodule foreach`,
/// `difftool --extcmd`, `config alias.x '!…'`, `clone --upload-pack`, …).
/// Any of them makes a `git` command an evaluator, whatever its options.
const GIT_COMMAND_RUNNERS: &[&str] = &[
    "config",
    "clone",
    "rebase",
    "bisect",
    "submodule",
    "difftool",
    "mergetool",
    "filter-branch",
    "for-each-repo",
];

/// git global options that take a separate value.
const GIT_VALUE_OPTS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--super-prefix",
];

/// tmux global flags that take no value (tmux 3.x usage line: `-2CDlNuVv`).
/// Requirement D3 also lists `-d` and `-h`; tmux has no such global flags,
/// so they are left out on purpose and end parsing as unknown (a deny).
const GLOBAL_FLAGS: &str = "2CDlNuVv";

/// tmux global options that take a value.
const GLOBAL_VALUE_OPTS: &str = "cfLST";

/// Deepest re-parse of an embedded script.
const MAX_DEPTH: usize = 4;

/// Larger input gets only the text test.
const MAX_INPUT: usize = 64 * 1024;

/// Most re-parses of embedded scripts for one command.
const MAX_REPARSES: usize = 256;

/// Whether the guard checks a tool named `tool_name`.
pub(super) fn is_shell_tool(tool_name: &str) -> bool {
    SHELL_TOOLS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(tool_name))
}

/// The deny response for a shell tool's input, or `None`.
///
/// Reads `command` and `cmd`. A string is checked as a command line; an array
/// of strings (the Codex argv form) is shell-quoted and joined first. An array
/// holding anything other than strings has its strings joined and gets the
/// text test. Any other shape is not a command.
pub(super) fn check_tool_input(tool_input: &Value) -> Option<Value> {
    ["command", "cmd"]
        .iter()
        .find_map(|key| match tool_input.get(key) {
            Some(Value::String(text)) => check(text),
            Some(Value::Array(items)) if !items.is_empty() => {
                let strings: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
                if strings.len() == items.len() {
                    check(&join_argv(&strings))
                } else {
                    text_test(&strings.join(" ")).then(deny_response)
                }
            }
            _ => None,
        })
}

/// The deny response for the command line `command`, or `None`.
pub(super) fn check(command: &str) -> Option<Value> {
    is_dangerous(command).then(deny_response)
}

fn deny_response() -> Value {
    warn!("tmux guard denied an unscoped tmux kill-server or kill-session -a (#1733)");
    serde_json::json!({
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

fn is_dangerous(text: &str) -> bool {
    if text.len() > MAX_INPUT {
        return text_test(text);
    }
    let mut ctx = Ctx {
        budget: MAX_REPARSES,
        tainted: HashSet::from(["TMUX".to_string()]),
    };
    scan(text, 0, &mut ctx).dangerous
}

/// The fallback for input the tokeniser cannot read with confidence.
fn text_test(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("tmux") && lower.contains("kill-s")
}

/// Shell-quotes each argument and joins them with spaces.
fn join_argv(args: &[&str]) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    args.iter()
        .map(|a| {
            if !a.is_empty() && a.chars().all(safe) {
                (*a).to_string()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a scan of one piece of text found.
#[derive(Debug, Default, Clone, Copy)]
struct Scan {
    /// A tmux invocation kills a server or every session unscoped, or the
    /// text was too uncertain to clear.
    dangerous: bool,
    /// At least one tmux invocation was found.
    tmux_seen: bool,
    /// At least one tmux invocation had no valid `-S`/`-L`.
    unscoped: bool,
}

/// State shared by every level of one check.
struct Ctx {
    /// Re-parses left before the check fails closed.
    budget: usize,
    /// `TMUX` and every variable assigned from it, directly or through
    /// another tainted variable, in the text seen so far. An `-S` or `-L`
    /// value that refers to one of these is not scope.
    tainted: HashSet<String>,
}

/// Adds to `tainted` each variable assigned, in any word of `cmds`, a value
/// that refers to a tainted variable. One pass in text order, which is the
/// order the shell assigns in, keeps the cost linear. Assignment position is
/// not checked: over-tainting only denies more.
fn taint_assignments(cmds: &[SimpleCommand], tainted: &mut HashSet<String>) {
    for word in cmds.iter().flat_map(|c| &c.words) {
        if !shell::is_assignment(word) {
            continue;
        }
        let Some((name, value)) = word.split_once('=') else {
            continue;
        };
        if refers_to_any(value, tainted) {
            tainted.insert(name.to_string());
        }
    }
}

fn scan(text: &str, depth: usize, ctx: &mut Ctx) -> Scan {
    let parsed = shell::tokenise(text);
    if !parsed.complete {
        return Scan {
            dangerous: text_test(text),
            ..Scan::default()
        };
    }
    let cmds = &parsed.commands;
    taint_assignments(cmds, &mut ctx.tainted);
    let pipelines = pipeline_facts(cmds);
    let evaluator_present = pipelines.values().any(|f| f.evaluator);
    let mut out = Scan::default();
    for cmd in cmds {
        let words = &cmd.words;
        let start = program_start(words);
        let prog = words.get(start);
        let prog_name = prog.map(|p| basename(p).to_ascii_lowercase());
        let data_only = !evaluator_present
            && prog_name
                .as_deref()
                .is_some_and(|n| DATA_PROGRAMS.contains(&n));

        // A program word that is a variable or substitution could be tmux.
        if prog.is_some_and(|p| p.starts_with('$') || p.starts_with('`'))
            && words.iter().skip(start).any(|w| starts_kill_s(w))
        {
            out.dangerous = true;
        }

        // Layers 1 and 2: every word naming tmux, from the program word on,
        // starts an invocation. Words before it are assignments and wrapper
        // options (the `TMUX` in `env -u TMUX`).
        for (k, word) in words.iter().enumerate().skip(start) {
            if !is_tmux(word) || (data_only && k != start) {
                continue;
            }
            let inv = invocation(words.get(k + 1..).unwrap_or_default(), &ctx.tainted);
            out.tmux_seen = true;
            if !inv.scoped {
                out.unscoped = true;
                if pipeline_mentions_kill_s(&pipelines, cmd) {
                    out.dangerous = true;
                }
            }
            out.dangerous |= inv.dangerous;
        }

        // Layer 3: words that hold a script mentioning tmux and kill-s.
        let git = prog_name.as_deref() == Some("git");
        for (k, word) in words.iter().enumerate() {
            let exempt = data_only
                && !word.contains("$(")
                && !word.contains('`')
                && !(git && git_runs_value(words, k));
            if !has_candidate(word) {
                if !exempt && expands_near_tmux(word) {
                    out.dangerous |= expansion_dangerous(word, depth, ctx);
                }
                continue;
            }
            let verdict = judge_word(word, depth, ctx);
            out.tmux_seen |= verdict.tmux_seen;
            if !verdict.substitutions_ok || (!verdict.plain_ok && !exempt) {
                out.dangerous = true;
            }
        }

        // A git subcommand that runs its arguments: judge them together too,
        // so `git bisect run tmux kill-server` reads as one command.
        if git && git_runs_commands(words.get(start + 1..).unwrap_or_default()) {
            let args: Vec<&str> = words
                .get(start + 1..)
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect();
            let joined = join_argv(&args);
            if needs_reparse(&joined) {
                let inner = reparse(&joined, depth, ctx);
                out.dangerous |= inner.dangerous;
                out.tmux_seen |= inner.tmux_seen;
            }
        }

        // Heredoc bodies: code when a shell evaluator appears anywhere on
        // the line, since the body may be written to a file and run later.
        for (n, body) in cmd.heredocs.iter().enumerate() {
            let expands = cmd.heredoc_expands.get(n).copied().unwrap_or(true);
            if evaluator_present {
                if needs_reparse(body) {
                    let inner = reparse(body, depth, ctx);
                    out.dangerous |= inner.dangerous;
                    out.tmux_seen |= inner.tmux_seen;
                    out.unscoped |= inner.unscoped;
                }
            } else if expands {
                // An unquoted delimiter: the shell runs `$(…)` in the body.
                for sub in substitutions(body).0 {
                    if needs_reparse(&sub) {
                        let inner = reparse(&sub, depth, ctx);
                        out.dangerous |= inner.dangerous;
                        out.tmux_seen |= inner.tmux_seen;
                    }
                }
            }
        }
    }
    out
}

/// What layer 3 found in one word.
struct WordVerdict {
    /// Every `$(…)` or backtick substitution in the word is harmless.
    substitutions_ok: bool,
    /// The rest of the word, read as a script, holds only scoped tmux.
    plain_ok: bool,
    tmux_seen: bool,
}

/// Re-parses a word that holds a `tmux` and a `kill-s` token.
///
/// Substitutions run as shell code wherever they appear, so each body is
/// judged on what it runs. The rest of the word is allowed only if it holds
/// at least one tmux invocation and all of them are scoped; a word whose
/// tokens are split across the pieces is uncertain and fails.
fn judge_word(word: &str, depth: usize, ctx: &mut Ctx) -> WordVerdict {
    let (bodies, rest) = substitutions(word);
    let mut verdict = WordVerdict {
        substitutions_ok: true,
        plain_ok: true,
        tmux_seen: false,
    };
    let mut any_candidate = false;
    for body in &bodies {
        if !has_candidate(body) {
            continue;
        }
        any_candidate = true;
        let inner = reparse(body, depth, ctx);
        verdict.substitutions_ok &= !inner.dangerous;
        verdict.tmux_seen |= inner.tmux_seen;
    }
    if has_candidate(&rest) {
        let inner = reparse(&rest, depth, ctx);
        verdict.plain_ok = inner.tmux_seen && !inner.unscoped && !inner.dangerous;
        verdict.tmux_seen |= inner.tmux_seen;
    } else if !any_candidate {
        verdict.plain_ok = false;
    }
    verdict
}

/// Re-parses a word that holds a `tmux` token and an expansion but no
/// `kill-s` token, such as the script in `bash -c 'tmux $c'`. Each
/// substitution body and the rest of the word are judged on their own, so
/// `x=$(tmux ls)` stays allowed.
fn expansion_dangerous(word: &str, depth: usize, ctx: &mut Ctx) -> bool {
    let (bodies, rest) = substitutions(word);
    let mut dangerous = false;
    for text in bodies.iter().chain(std::iter::once(&rest)) {
        if needs_reparse(text) {
            dangerous |= reparse(text, depth, ctx).dangerous;
        }
    }
    dangerous
}

/// One level deeper, within the depth and re-parse limits. Hitting either
/// limit with a candidate left is dangerous.
fn reparse(text: &str, depth: usize, ctx: &mut Ctx) -> Scan {
    if depth >= MAX_DEPTH || ctx.budget == 0 {
        return Scan {
            dangerous: true,
            ..Scan::default()
        };
    }
    ctx.budget -= 1;
    scan(text, depth + 1, ctx)
}

/// Stands in for a substitution in the text around it. It is an expansion,
/// so a tmux subcommand made by a substitution stays uncertain.
const PLACEHOLDER: &str = " $_ ";

/// The bodies of the top-level `$(…)` and backtick substitutions in `text`,
/// and the text with each replaced by [`PLACEHOLDER`].
fn substitutions(text: &str) -> (Vec<String>, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut bodies = Vec::new();
    let mut rest = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == '$' && chars.get(i + 1) == Some(&'(') {
            let close = shell::subst_close(&chars, i);
            let end = close.unwrap_or(chars.len());
            bodies.push(chars.get(i + 2..end).unwrap_or_default().iter().collect());
            rest.push_str(PLACEHOLDER);
            i = end.saturating_add(1);
        } else if c == '`' {
            let end = (i + 1..chars.len())
                .find(|&j| chars.get(j) == Some(&'`'))
                .unwrap_or(chars.len());
            bodies.push(chars.get(i + 1..end).unwrap_or_default().iter().collect());
            rest.push_str(PLACEHOLDER);
            i = end.saturating_add(1);
        } else {
            rest.push(c);
            i += 1;
        }
    }
    (bodies, rest)
}

/// Whether `text` holds both a `tmux` token and a `kill-s` token, separated
/// by whitespace or shell punctuation.
fn has_candidate(text: &str) -> bool {
    let mut tmux = false;
    let mut kill = false;
    for token in text.split(|c: char| !(c.is_alphanumeric() || "_.-/".contains(c))) {
        let name = basename(token);
        tmux |= name.eq_ignore_ascii_case("tmux");
        kill |= starts_kill_s(name);
        if tmux && kill {
            return true;
        }
    }
    false
}

/// Whether `text` holds a `tmux` token and a `$` or backtick that could
/// expand to its subcommand.
fn expands_near_tmux(text: &str) -> bool {
    is_expansion(text)
        && text
            .split(|c: char| !(c.is_alphanumeric() || "_.-/".contains(c)))
            .any(is_tmux)
}

/// Whether `text` needs a re-parse: it holds a tmux candidate or a tmux
/// token beside an expansion.
fn needs_reparse(text: &str) -> bool {
    has_candidate(text) || expands_near_tmux(text)
}

/// Whether a word holds an expansion, so its value is unknown here.
fn is_expansion(word: &str) -> bool {
    word.contains('$') || word.contains('`')
}

fn starts_kill_s(word: &str) -> bool {
    word.get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("kill-s"))
}

fn is_tmux(word: &str) -> bool {
    basename(word).eq_ignore_ascii_case("tmux")
}

/// The index of the program word in `words` (or `words.len()` if none).
fn program_start(words: &[String]) -> usize {
    words.len() - program(words).map_or(0, <[String]>::len)
}

/// Whether the command runs text, or another command, as code.
fn is_evaluator(cmd: &SimpleCommand) -> bool {
    let words = &cmd.words;
    let start = program_start(words);
    let named = words
        .iter()
        .take(start.saturating_add(1))
        .any(|w| SHELL_EVALUATORS.contains(&basename(w).to_ascii_lowercase().as_str()));
    let env_split = words.first().is_some_and(|w| basename(w) == "env")
        && words
            .iter()
            .any(|w| w.starts_with("-S") || w.starts_with("--split-string"));
    let prog = words.get(start).map(|w| basename(w));
    let git_runner =
        prog == Some("git") && git_runs_commands(words.get(start + 1..).unwrap_or_default());
    let gh_alias = prog == Some("gh") && words.get(start + 1).is_some_and(|w| w == "alias");
    // `GIT_EXTERNAL_DIFF=…`, `GIT_SSH_COMMAND=…` and the like run their value.
    let assigned = matches!(prog, Some("git" | "gh"))
        && words.iter().take(start).any(|w| shell::is_assignment(w));
    named || env_split || git_runner || gh_alias || assigned
}

/// Whether the arguments after `git` name one of [`GIT_COMMAND_RUNNERS`]
/// as the subcommand: the first word after git's global options.
fn git_runs_commands(args: &[String]) -> bool {
    let mut i = 0usize;
    while let Some(word) = args.get(i) {
        if GIT_VALUE_OPTS.contains(&word.as_str()) {
            i = i.saturating_add(2);
        } else if word.starts_with('-') {
            i += 1;
        } else {
            return GIT_COMMAND_RUNNERS.contains(&word.as_str());
        }
    }
    false
}

/// Whether word `k` of a `git` command is a `-c` value (which can define a
/// `!` alias or a shell-run setting) or contains `=!`.
fn git_runs_value(words: &[String], k: usize) -> bool {
    let Some(word) = words.get(k) else {
        return false;
    };
    word.contains("=!")
        || (word.starts_with("-c") && word.len() > 2)
        || k.checked_sub(1)
            .and_then(|p| words.get(p))
            .is_some_and(|prev| prev == "-c")
}

/// What one pipeline holds, gathered in a single pass so the per-command
/// checks in [`scan`] stay linear in the number of commands.
#[derive(Debug, Default, Clone, Copy)]
struct PipelineFacts {
    /// A command in the pipeline is a shell evaluator.
    evaluator: bool,
    /// How many commands in the pipeline have a word starting with `kill-s`.
    kill_s_commands: usize,
    /// A heredoc body in the pipeline mentions `kill-s`.
    heredoc_kill_s: bool,
}

fn pipeline_facts(cmds: &[SimpleCommand]) -> HashMap<usize, PipelineFacts> {
    let mut facts: HashMap<usize, PipelineFacts> = HashMap::new();
    for cmd in cmds {
        let f = facts.entry(cmd.pipeline).or_default();
        f.evaluator |= is_evaluator(cmd);
        f.kill_s_commands += usize::from(has_kill_s_word(cmd));
        f.heredoc_kill_s |= cmd
            .heredocs
            .iter()
            .any(|b| b.to_ascii_lowercase().contains("kill-s"));
    }
    facts
}

fn has_kill_s_word(cmd: &SimpleCommand) -> bool {
    cmd.words.iter().any(|w| starts_kill_s(w))
}

/// Whether a command other than `cmd` in its pipeline has a word starting
/// with `kill-s`, or a heredoc body in the pipeline mentions it: an unscoped
/// `tmux -C` reads its commands from stdin.
fn pipeline_mentions_kill_s(
    pipelines: &HashMap<usize, PipelineFacts>,
    cmd: &SimpleCommand,
) -> bool {
    pipelines
        .get(&cmd.pipeline)
        .is_some_and(|f| f.heredoc_kill_s || f.kill_s_commands > usize::from(has_kill_s_word(cmd)))
}

/// One tmux invocation's verdict.
struct Invocation {
    scoped: bool,
    dangerous: bool,
}

/// Checks the arguments after a `tmux` word.
fn invocation(args: &[String], tainted: &HashSet<String>) -> Invocation {
    let global = global_scope(args, tainted);
    let rest = args.get(global.rest..).unwrap_or_default();
    let dangerous = !global.scoped
        && if global.uncertain {
            loose_dangerous(rest)
        } else {
            sequence_dangerous(rest)
        };
    Invocation {
        scoped: global.scoped,
        dangerous,
    }
}

/// tmux's global options: where the first subcommand starts, and whether a
/// valid `-S` or `-L` was given.
struct Global {
    scoped: bool,
    rest: usize,
    /// An unknown option ended parsing, so the subcommand's position is a
    /// guess.
    uncertain: bool,
}

fn global_scope(args: &[String], tainted: &HashSet<String>) -> Global {
    let mut scoped = false;
    let mut bad_scope = false;
    let mut i = 0usize;
    while let Some(word) = args.get(i) {
        if word == "--" {
            i += 1;
            break;
        }
        let Some(flags) = word.strip_prefix('-') else {
            break;
        };
        if flags.is_empty() {
            break;
        }
        let mut takes_next = false;
        for (pos, flag) in flags.char_indices() {
            if GLOBAL_FLAGS.contains(flag) {
                continue;
            }
            if !GLOBAL_VALUE_OPTS.contains(flag) {
                return Global {
                    scoped: false,
                    rest: i,
                    uncertain: true,
                };
            }
            let joined = flags.get(pos + flag.len_utf8()..).unwrap_or_default();
            let value = if joined.is_empty() {
                takes_next = true;
                args.get(i + 1).map(String::as_str)
            } else {
                Some(joined)
            };
            if flag == 'S' || flag == 'L' {
                if value.is_some_and(|v| scope_value_ok(v, tainted)) {
                    scoped = true;
                } else {
                    bad_scope = true;
                }
            }
            break;
        }
        i = i.saturating_add(if takes_next { 2 } else { 1 });
    }
    Global {
        scoped: scoped && !bad_scope,
        rest: i.min(args.len()),
        uncertain: false,
    }
}

/// Whether an `-S` or `-L` value names a private server: not empty, not
/// `default` (as a name or a socket file), and not a reference to `$TMUX` or
/// to a variable assigned from it.
fn scope_value_ok(value: &str, tainted: &HashSet<String>) -> bool {
    !value.is_empty()
        && !refers_to_any(value, tainted)
        && !basename(value.trim_end_matches('/')).eq_ignore_ascii_case("default")
}

/// Whether `value` expands a variable in `names`: `$NAME` or `${NAME…}`,
/// where `NAME` is the longest run of letters, digits and `_` after the `$`
/// or `${`. So `$TMUX`, `${TMUX}` and `${TMUX%%,*}` refer to `TMUX`, and
/// `$TMUX_TMPDIR` does not.
fn refers_to_any(value: &str, names: &HashSet<String>) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        let after = rest.get(at + 1..).unwrap_or_default();
        let body = after.strip_prefix('{').unwrap_or(after);
        let end = body
            .find(|c: char| c != '_' && !c.is_ascii_alphanumeric())
            .unwrap_or(body.len());
        if body.get(..end).is_some_and(|name| names.contains(name)) {
            return true;
        }
        rest = after;
    }
    false
}

/// Whether a tmux command sequence (split on `;` words and words ending in
/// `;`) runs `kill-server` or `kill-session -a`, or has a subcommand word
/// that is an expansion and so may be either.
fn sequence_dangerous(rest: &[String]) -> bool {
    let mut current: Vec<&str> = Vec::new();
    let mut commands: Vec<Vec<&str>> = Vec::new();
    for word in rest {
        if let Some(head) = word.strip_suffix(';') {
            if !head.is_empty() {
                current.push(head);
            }
            commands.push(std::mem::take(&mut current));
        } else {
            current.push(word);
        }
    }
    commands.push(current);
    commands.iter().any(|cmd| match cmd.split_first() {
        Some((name, flags)) => {
            is_expansion(name)
                || kills_server(name)
                || (is_kill_session(name) && kills_all_sessions(flags))
        }
        None => false,
    })
}

/// After an unknown global option the subcommand could start anywhere, so
/// any word that would kill, or any expansion, counts.
fn loose_dangerous(rest: &[String]) -> bool {
    rest.iter().enumerate().any(|(j, word)| {
        let word = word.trim_end_matches(';');
        is_expansion(word)
            || kills_server(word)
            || (is_kill_session(word) && {
                let flags: Vec<&str> = rest
                    .get(j + 1..)
                    .unwrap_or_default()
                    .iter()
                    .map(String::as_str)
                    .collect();
                kills_all_sessions(&flags)
            })
    })
}

/// `kill-server` or a prefix of it from `kill-s` on (`kill-s` and `kill-se`
/// are ambiguous with `kill-session`, and tmux may resolve them either way).
fn kills_server(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.len() >= "kill-s".len() && "kill-server".starts_with(&name)
}

/// `kill-session` or an unambiguous prefix of it (`kill-ses` on).
fn is_kill_session(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.len() >= "kill-ses".len() && "kill-session".starts_with(&name)
}

/// Whether `kill-session`'s flags set `-a`, read getopt-style with spec
/// `aCt:`. An unknown flag is uncertain, so it counts as `-a`.
fn kills_all_sessions(flags: &[&str]) -> bool {
    let mut all = false;
    let mut i = 0usize;
    while let Some(word) = flags.get(i) {
        if *word == "--" {
            break;
        }
        let Some(opts) = word.strip_prefix('-') else {
            break;
        };
        if opts.is_empty() {
            break;
        }
        let mut takes_next = false;
        for (pos, flag) in opts.char_indices() {
            match flag {
                'a' => all = true,
                'C' => {}
                't' => {
                    takes_next = opts
                        .get(pos + flag.len_utf8()..)
                        .unwrap_or_default()
                        .is_empty();
                    break;
                }
                _ => return true,
            }
        }
        i = i.saturating_add(if takes_next { 2 } else { 1 });
    }
    all
}

#[cfg(test)]
#[path = "tests/tmux_guard_tests.rs"]
mod tests;
