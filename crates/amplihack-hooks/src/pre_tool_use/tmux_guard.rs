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
//! refers to `$TMUX`, and a socket path ending in `default` (or in
//! `default}`, as in `"${S:-/tmp/tmux-1000/default}"`) are not scope. Nor is
//! a value that refers to a variable assigned, anywhere in the command, a
//! value that is not scope: `S=$TMUX; tmux -S "$S" …`,
//! `S=/tmp/tmux-$(id -u)/default; tmux -S "$S" …` and
//! `L=default; tmux -L "$L" …` are denied. Nor is a value holding a `$(…)`
//! or backtick substitution that runs tmux, or a variable assigned one
//! (which the rule above already covers): inside a pane
//! `tmux display -p '#{socket_path}'` prints the user's own socket, so
//! `tmux -S "$(tmux display -p '#{socket_path}')" kill-server` is denied. A
//! substitution "runs tmux" when its body has a `tmux` token, so a sandbox
//! made by `mktemp -d /tmp/tmux-qa.XXXXXX` is still scope.
//!
//! tmux commands that take another tmux command as an argument (`if-shell`,
//! `run-shell -C`, `bind`, `set-hook`, …) are covered by one rule: in an
//! unscoped invocation, any argument after the subcommand word that contains
//! a kill command counts as a kill (`tmux if-shell true kill-server`,
//! `tmux set -g command-alias[0] zz=kill-server`).
//!
//! tmux is found three ways, using the shared tokeniser in [`super::shell`]:
//!
//! 1. as the program word of a simple command, including a program word that
//!    is an expansion naming tmux (`"$(command -v tmux)" $c` reads like
//!    `tmux $c`);
//! 2. as a later word of another command (`xargs tmux …`, `ssh host tmux …`);
//! 3. inside a word that holds both a `tmux` and a `kill-s` token
//!    (`bash -c '…'`, `ssh host '…'`, `python -c "…"`, `$(…)`, backticks),
//!    parsed again up to [`MAX_DEPTH`] levels. The shell has not yet removed
//!    the word's own quoting, so its tokens are also read with that quoting
//!    undone ([`any_reading`]: `t''mux`, `t\mux` and `$'\x74mux'` read as
//!    `tmux`), and a token whose last path component is `tmux` or ends in
//!    `-tmux` counts as `tmux` ([`is_tmux_token`]), so a parameter
//!    expansion's fallback (`${TMUX_BIN:-tmux}`, `${T-tmux}`) counts. A word
//!    with a `tmux` token and an expansion but no `kill-s` token that would
//!    parse back into the very text being scanned (a lone
//!    `${TMUX_BIN:-tmux}`) is not parsed again, since layers 1 and 2 have
//!    read it; one with a `kill-s` token still is, and reaching
//!    [`MAX_DEPTH`] denies it. The word is cleared when the
//!    parse finds at least one tmux invocation, none of them kills, and
//!    either every one is scoped or no run of tokens anywhere in the word
//!    would kill (`kill-server`, or `kill-session` with `-a`). So
//!    `bash -c "tmux kill-session -t amp-x"` is allowed, as it is unwrapped,
//!    while `python -c 'subprocess.run(["tmux", "kill-server"])'`, whose
//!    tmux call the shell tokeniser cannot see, is still denied. A
//!    substitution's output may be run as well (`bash -c "$(echo tmux
//!    kill-server)"`, `x=$(…); $x`), so a body that mentions `tmux` and
//!    `kill-s` must also not print them through a data-only command, unless
//!    the substitution is an argument of a data-only command, whose output
//!    stays data.
//!
//! Redirections never count as tmux arguments (`tmux 2>/dev/null
//! kill-server` is denied), and `$'…'` escapes are decoded first
//! (`tmux $'\x6bill-server'` is denied); both are done by the tokeniser.
//! Words are split on whitespace and shell punctuation, and read a second
//! time with the one-letter printf escapes (`\n`, `\t`, …) as separators, so
//! `printf 'ls\nkill-server\n' | tmux -C attach` is denied, and a third time
//! with quoting undone, as described for layer 3.
//!
//! An unscoped tmux, or a program word that is an expansion, counts as
//! reading a kill from stdin, as a `tmux -C` client does, only when an
//! earlier stage of its pipeline has a word with a
//! `kill-s` token, or a heredoc in the pipeline mentions one. Stdin flows left
//! to right, so in `tmux ls -F '#S' | grep '^amp-' | xargs -r -n1 tmux
//! kill-session -t` the `tmux ls` reads nothing the `xargs` stage holds, and
//! the targeted kill is allowed. A pipeline broken across lines after `|` is
//! one pipeline; the tokeniser keeps its stages together.
//!
//! Layers 2 and 3 exempt data-only commands such as
//! `git commit -m "… tmux kill-server …"`. `git` is data-only only with a
//! subcommand in [`GIT_DATA_SUBCOMMANDS`] (`commit`, `log`, …), none of which
//! runs shell text given as an argument. Any other git command is judged like
//! any other program, so `git fetch --upload-pack 'tmux kill-server #' r` is
//! denied. A git subcommand in [`GIT_COMMAND_RUNNERS`], which runs its
//! arguments or stores shell text for a later git command on the same line
//! (`config` and `clone -c` for `!` aliases and `core.pager`), makes the
//! whole line an evaluator; so do `gh alias` and an environment assignment
//! before `git` or `gh` (`GIT_EXTERNAL_DIFF=… git diff`).
//!
//! The `-c`/`--context` values of `amplihack recipe run` are data too:
//! amplihack hands them to the recipe as text, exactly as it hands over a
//! `--context-from-file` file, which the guard never reads. So a task
//! description that mentions the guard can be launched inline. As with the
//! other data-only commands, a `$(` or backtick in the value, or an
//! evaluator anywhere on the line, removes the exemption. This ties the
//! guard to `recipe run`'s options: if it gains another way to pass context
//! inline, text passed that way is judged as shell text, and a task
//! description that mentions the guard is denied, until
//! [`is_context_value`] learns the new form.
//!
//! Heredoc bodies are parsed when a shell evaluator appears anywhere on the
//! line, so `cat > k.sh <<EOF … EOF` followed by `bash k.sh` is denied.
//! Uncertain input fails closed: unbalanced quotes, a heredoc whose
//! delimiter line never appears, a redirection with no target, or hitting [`MAX_DEPTH`] or [`MAX_REPARSES`] with a candidate
//! left deny, as does a simple command with more than
//! [`MAX_TMUX_PER_COMMAND`] tmux words. So does an unscoped tmux whose
//! subcommand word holds a `$` or
//! a backtick (`c=kill-server; tmux $c`, `tmux $(echo kill-server)`), since
//! the expansion may name `kill-server`; expansions in option values
//! (`tmux new -s "$name"`) and in scoped invocations are allowed.
//!
//! A program word that is an expansion ([`unknown_program`]: `$T`,
//! `"$(command -v tmux)"`, `` `echo tmux` ``, `/usr/bin/$T`) may be tmux,
//! and may split into several words, so its scope cannot be read. It is
//! treated as an unscoped tmux whose arguments are unknown: any later word
//! with a `kill-s` token denies, and so does a `kill-s` token on its stdin
//! by the stdin rule below, so `echo kill-server | "$(command -v tmux)" -C
//! attach` and `${TMUX_BIN:-tmux} -C attach <<EOF` with a `kill-server`
//! body are denied.
//!
//! This is a guard rail against agent mistakes, not a sandbox. It does not
//! cover:
//!
//! - shell aliases or functions, script files (`bash script.sh` for a file
//!   written earlier), or commands assembled at run time from pieces;
//! - a tmux named by an expansion after the program word, as in
//!   `xargs "$T" kill-server`, `xargs ${T:-tmux} kill-server` or
//!   `ssh vm1 "$T" kill-server`: layer 2 starts an invocation only at a word
//!   whose last path component is `tmux`. The wrappers the tokeniser sees
//!   through (`env`, `sudo`, `nohup`, `command`, `exec`, `nice`, `timeout`,
//!   `time`) leave the expansion as the program word, so
//!   `sudo "$T" kill-server` is denied;
//! - `tmux source-file` (or `source`) reading its commands from a file,
//!   stdin, a heredoc or a here-string, unless a word with a `kill-s` token
//!   sits in an earlier stage of the unscoped tmux's pipeline, or in a
//!   heredoc of that pipeline;
//! - a `tmux -C` client fed by a subshell, a `{ …; }` group or a process
//!   substitution (`(echo kill-server) | tmux -C attach`, `tmux -C attach <
//!   <(echo kill-server)`), which the tokeniser does not read as earlier
//!   stages of the client's pipeline;
//! - a `tmux -C` client inside a compound command (`if`, `while`, `until`,
//!   `for`, `case`, `{ …; }`) that receives a pipe or heredoc, as in
//!   `echo kill-server | if true; then tmux -C attach; fi` or
//!   `printf kill-server | { true; tmux -C attach; }`: the tokeniser ends a
//!   stage at the first `;` or `)`, so the client starts a new pipeline and
//!   reads no earlier stage;
//! - an `-S` path that is a symlink to the default socket, which only a
//!   file system lookup could see;
//! - a variable copied from `$TMUX`, from tmux's output or from a default
//!   socket path other than by assignment (`read`, `printf -v`,
//!   `declare -n`, a `for` loop), or in an earlier tool call;
//! - a `default` name assembled from pieces, or given only as a parameter
//!   expansion's fallback other than at the end of the value
//!   (`-S "/tmp/tmux-1000/${N}ault"`, `-L "${L:-default}"`);
//! - an `-S` value that is a glob or a file read at run time (`-S
//!   /tmp/tmux-*/*`, `-S "$(cat sock.txt)"`);
//! - a command glued to a short option with no separator, as in
//!   `git grep -O'tmux kill-server'`, where `-Otmux` is one token;
//! - shell text that git runs from a hook file or from configuration set
//!   outside the command line;
//! - gh subcommands other than `gh alias` that run commands (`gh codespace
//!   ssh`, `gh extension exec`): gh is otherwise data-only;
//! - input over [`MAX_INPUT`] bytes, which gets only a plain text check for
//!   `tmux` and `kill-s` with `'`, `"` and `\` removed, not the full parse:
//!   `t''mux` and `t\mux` are caught, but a word spelt through an escape
//!   (`$'\x74mux'`) or assembled from pieces gets past it;
//! - named-fd redirections (`{fd}>file`), whose `{fd}` is read as a word;
//! - an arithmetic shift (`(( x = 1 << 2 ))`, `$[1<<2]`) followed, on a
//!   later line, by a line equal to its right operand: the tokeniser reads
//!   every unquoted `<<` as a heredoc, so the lines between are its body.
//!   Without such a line the heredoc never ends, the parse is incomplete and
//!   the text test applies;
//! - a shell tool whose name is not in [`SHELL_TOOLS`], such as one a new
//!   harness release adds;
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
//! `$(tmux kill-server)` in a commit message is denied as well, and so are
//! `x=$(echo "notes on tmux kill-server")`, an unscoped
//! `tmux send-keys 'tmux kill-server'`, and a hand-written
//! `tmux new-session -d "$(amplihack agent-binary --shell) amplihack recipe
//! run … -c task_description='… tmux kill-server …'"`, whose task text tmux
//! runs inside a shell command.
//! [`BLOCK_MESSAGE`] names the way out for text: pass it in a file
//! (`git commit -F`, `--body-file`, `--context-from-file`) or run the
//! command in its own tool call.
//!
//! Three more false positives come from reading tmux where the text alone
//! cannot rule it out:
//!
//! - a `kill-s` word piped or heredoc-fed to any program word that is an
//!   expansion, since it may be a `tmux -C` client: `echo kill-server |
//!   $PAGER` and `grep kill-session log | "$PAGER"` are denied, and so is
//!   `$T -S "$SB/t.sock" -C attach` fed a kill, whose scope the guard cannot
//!   trust while the program word may split into several words;
//! - a token ending in `-tmux` beside a `kill-s` token in an embedded script
//!   that holds no tmux invocation, such as `bash -c 'ls ~/qa-tmux; echo
//!   kill-server'` or a `--no-tmux` flag, since the token may be the
//!   fallback of `${T-tmux}`;
//! - a socket file named `tmux` followed by a kill: layer 2 reads the `-S`
//!   value of `env -u TMUX tmux -S "$SB/tmux" kill-server` as a second,
//!   unscoped tmux. Name the socket `tmux.sock`, as [`BLOCK_MESSAGE`] does.
//!
//! A socket file named `default` is refused even when it belongs to a
//! sandbox (`tmux -S "$SB/tmuxsock/tmux-1000/default" kill-server`, the
//! server `TMUX_TMPDIR=$SB/tmuxsock` starts), because from the text alone it
//! cannot be told apart from the user's. [`BLOCK_MESSAGE`] says so, and names
//! the way out: name a private server something else, and end one already
//! running on a `default` path with `tmux -S <path> kill-session -t <name>`
//! for each session. A variable whose value ends in `default` is not scope
//! anywhere in an `-S` value, so `D=/srv/default; tmux -S "$D/qa.sock"
//! kill-server` is denied as well.
//!
//! The guard always runs and reads no environment variables, so nothing in
//! the environment can turn it off; `guard_source_reads_no_environment`
//! checks the source of this module and of the tokeniser. It never panics
//! on any input: the hook fails open on Claude Code and Copilot, so a panic
//! would allow the command.
//! For the same reason a harness that times out the hook, for example on a
//! cold start, lets the command run. The guard's own cost grows linearly
//! with the input: at most [`MAX_TMUX_PER_COMMAND`] tmux invocations are
//! read per simple command, each reading the rest of that command once;
//! embedded scripts are parsed again at most [`MAX_REPARSES`] times and
//! [`MAX_DEPTH`] levels deep; and input over [`MAX_INPUT`] gets only the
//! text test. `large_inputs_shaped_against_the_cost_bound_finish_quickly`
//! runs the slowest inputs found, each just under [`MAX_INPUT`]; the slowest
//! takes about 0.3 s in a release build.

use super::shell::{self, SimpleCommand, basename, program};
use serde_json::Value;
use std::collections::HashSet;
use tracing::warn;

/// The deny reason. The command itself is never echoed. It also covers the
/// false-positive case, a command that only mentions the words as text, so
/// an agent is told the supported way out rather than guessing at one.
pub(super) const BLOCK_MESSAGE: &str = "Blocked: this command looks like a tmux kill-server or kill-session -a with no -S or -L, which acts on the tmux server named in $TMUX (normally the user's own server, with all of their sessions). TMUX_TMPDIR alone does not redirect tmux while $TMUX is set. Use `tmux -S <private socket> …`, or `env -u TMUX tmux -S <private socket> …`, and only for a server you started on that socket; a socket path taken from $TMUX or from tmux itself (`#{socket_path}`) is the user's server. A socket file named `default`, or `-L default`, also counts as the user's server, even under a sandbox directory: give a private server another name (`-S \"$SB/tmux.sock\"`), and end one already running on a `default` path with `tmux -S <path> kill-session -t <name>` for each of its sessions, since by default a server exits when its last session ends. If the command only mentions these words as text, such as a commit message or a task description, pass the text in a file (`git commit -F`, `--body-file`, `--context-from-file`) or run that command in its own tool call.";

/// Tool names the guard checks, compared case-insensitively: Claude Code's
/// `Bash`, `PowerShell` and `Monitor`, Copilot's `bash` and `powershell`, and
/// Codex's shell tools in case a host skips the Codex normalisation.
///
/// Claude Code's `Monitor` takes a `command` that is a shell command or
/// script, run in the same shell environment as `Bash`, so it is a shell
/// tool; its other form, a WebSocket `ws` with no `command`, is not checked.
/// In Claude Code 2.1.314 the tools that run code and take a `command` are
/// `Bash`, `PowerShell` and `Monitor`. Check this list against each new
/// Claude Code release: a shell tool missing from it skips the guard.
pub(super) const SHELL_TOOLS: &[&str] = &[
    "bash",
    "powershell",
    "monitor",
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

/// The only git subcommands whose arguments are data. None of them runs shell
/// text given as an argument. A git command with any other subcommand is not
/// data-only (`fetch`, `pull`, `push`, `ls-remote` and `archive` take
/// `--upload-pack`, `--receive-pack` or `--exec`; `grep -O` runs a pager).
/// This list fails closed: a subcommand missing from it only loses the
/// exemption.
const GIT_DATA_SUBCOMMANDS: &[&str] = &[
    "add", "commit", "diff", "log", "notes", "show", "stash", "status", "tag",
];

/// git subcommands that run their arguments as a command, or store shell text
/// for a later git command on the same line to run (`rebase -x`,
/// `bisect run`, `submodule foreach`, `difftool --extcmd`,
/// `config alias.x '!…'`, `clone -c core.pager=…`, …). Any of them makes the
/// whole line an evaluator. Shell text in a single argument of any other
/// non-data subcommand is already judged as a script, so this list is not
/// meant to grow with every option that runs a program.
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

/// Most tmux invocations read in one simple command; a command with more is
/// denied. Each invocation reads the words after it, so without a cap the
/// cost would grow with the square of the number of `tmux` words.
const MAX_TMUX_PER_COMMAND: usize = 64;

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

/// The fallback for input the tokeniser cannot read with confidence: the
/// text, with every `'`, `"` and `\` removed, contains both `tmux` and
/// `kill-s`. Removing them undoes quoting such as `t''mux` and `t\mux`; it
/// cannot hide a `tmux` or `kill-s` that is already there, since neither
/// holds those characters.
fn text_test(text: &str) -> bool {
    let lower: String = text
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
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
    /// A data-only command's words mention both `tmux` and `kill-s`, so its
    /// output may be the text `tmux kill-server`.
    data_mention: bool,
}

/// State shared by every level of one check.
struct Ctx {
    /// Re-parses left before the check fails closed.
    budget: usize,
    /// `TMUX` and every variable assigned, in the text seen so far, a value
    /// that is not scope: one taken from `TMUX` or tmux's output, directly
    /// or through another tainted variable, one naming the default socket,
    /// or an empty one. An `-S` or `-L` value that refers to one of these is
    /// not scope.
    tainted: HashSet<String>,
}

/// Adds to `tainted` each variable assigned, in any word of `cmds`, a value
/// that would not be scope if written as the `-S` or `-L` value itself
/// ([`scope_value_ok`]): one that refers to a tainted variable, substitutes
/// tmux's output (`sock=$(tmux display -p '#{socket_path}')`), names the
/// default socket (`S=/tmp/tmux-$(id -u)/default`, `L=default`) or is empty.
/// One pass in text order, which is the order the shell assigns in, keeps
/// the cost linear. Assignment position is not checked, and a value that
/// refers to a tainted variable anywhere is not scope: over-tainting only
/// denies more, so `D=/srv/default; tmux -S "$D/qa.sock" kill-server` is
/// denied too.
fn taint_assignments(cmds: &[SimpleCommand], tainted: &mut HashSet<String>) {
    for word in cmds.iter().flat_map(|c| &c.words) {
        if !shell::is_assignment(word) {
            continue;
        }
        let Some((name, value)) = word.split_once('=') else {
            continue;
        };
        if !scope_value_ok(value, tainted) {
            tainted.insert(name.to_string());
        }
    }
}

/// Whether `value` holds a `$(…)` or backtick substitution whose body has a
/// `tmux` token ([`is_tmux_token`]). Inside a pane,
/// `tmux display -p '#{socket_path}'` prints the socket of the user's own
/// server, so such a value is not scope.
fn substitutes_tmux_output(value: &str) -> bool {
    (value.contains("$(") || value.contains('`'))
        && substitutions(value)
            .0
            .iter()
            .any(|body| any_reading(body, |t| t.iter().any(|w| is_tmux_token(w))))
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
    let facts = line_facts(cmds);
    let evaluator_present = facts.evaluator;
    let mut out = Scan::default();
    for (n, cmd) in cmds.iter().enumerate() {
        let words = &cmd.words;
        let start = program_start(words);
        let prog = words.get(start);
        let prog_name = prog.map(|p| basename(p).to_ascii_lowercase());
        let args = words.get(start + 1..).unwrap_or_default();
        let git = prog_name.as_deref() == Some("git");
        let data_only = !evaluator_present
            && prog_name
                .as_deref()
                .is_some_and(|n| DATA_PROGRAMS.contains(&n))
            && (!git || git_subcommand(args).is_some_and(|s| GIT_DATA_SUBCOMMANDS.contains(&s)));
        // `amplihack recipe run`: only its `-c`/`--context` values are data.
        let recipe_run =
            !evaluator_present && prog_name.as_deref() == Some("amplihack") && is_recipe_run(args);
        let context_value = |k: usize| recipe_run && is_context_value(words, k);
        out.data_mention |= (data_only || recipe_run) && has_candidate(&words.join(" "));

        // A program word that is an expansion could be an unscoped tmux
        // (`"$(command -v tmux)"`, `${TMUX_BIN:-tmux}`), and could split into
        // several words, so its scope cannot be read. Its own body is judged
        // by layer 3; what counts here is a later word holding a `kill-s`
        // token, or one on its stdin, which a `tmux -C` client would run.
        let fed_kill_s = facts.fed_kill_s.get(n).copied().unwrap_or(true);
        if prog.is_some_and(|p| unknown_program(p))
            && (fed_kill_s || words.iter().skip(start + 1).any(|w| mentions_kill_s(w)))
        {
            out.dangerous = true;
        }

        // Layers 1 and 2: every word naming tmux, from the program word on,
        // starts an invocation, and so does a program word that is an
        // expansion naming tmux, so `"$(command -v tmux)" $c` reads like
        // `tmux $c`. Words before the program word are assignments and
        // wrapper options (the `TMUX` in `env -u TMUX`).
        let mut invocations = 0usize;
        for (k, word) in words.iter().enumerate().skip(start) {
            let names_tmux =
                is_tmux(word) || (k == start && unknown_program(word) && expands_near_tmux(word));
            if !names_tmux || (k != start && (data_only || context_value(k))) {
                continue;
            }
            out.tmux_seen = true;
            // Each invocation reads the rest of its command, so the count
            // is capped to keep the check linear in the input.
            invocations += 1;
            if invocations > MAX_TMUX_PER_COMMAND {
                out.dangerous = true;
                break;
            }
            let inv = invocation(words.get(k + 1..).unwrap_or_default(), &ctx.tainted);
            if !inv.scoped {
                out.unscoped = true;
                // An unscoped `tmux -C` runs each line of its stdin.
                out.dangerous |= fed_kill_s;
            }
            out.dangerous |= inv.dangerous;
        }

        // Layer 3: words that hold a script mentioning tmux and kill-s.
        for (k, word) in words.iter().enumerate() {
            let data = (data_only && !(git && git_runs_value(words, k))) || context_value(k);
            let exempt = data && !word.contains("$(") && !word.contains('`');
            if !has_candidate(word) {
                if !exempt && expands_near_tmux(word) {
                    out.dangerous |= expansion_dangerous(word, text, depth, ctx);
                }
                continue;
            }
            let verdict = judge_word(word, data, depth, ctx);
            out.tmux_seen |= verdict.tmux_seen;
            out.data_mention |= verdict.data_mention;
            if !verdict.substitutions_ok || (!verdict.plain_ok && !exempt) {
                out.dangerous = true;
            }
        }

        // A git subcommand that runs its arguments: judge them together too,
        // so `git bisect run tmux kill-server` reads as one command.
        if git && git_runs_commands(args) {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
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
    data_mention: bool,
}

/// Re-parses a word that holds a `tmux` and a `kill-s` token.
///
/// Substitutions run as shell code wherever they appear, so each body is
/// judged on what it runs. Unless the word is an argument to a data-only
/// command (`data`), a body's output may be run too, so the body is also
/// cleared only when it is [`cleared`] and no data-only command in it prints
/// the tokens. The rest of the word is allowed only if it is [`cleared`]; a
/// word whose tokens are split across the pieces is uncertain and fails.
fn judge_word(word: &str, data: bool, depth: usize, ctx: &mut Ctx) -> WordVerdict {
    let (bodies, rest) = substitutions(word);
    let mut verdict = WordVerdict {
        substitutions_ok: true,
        plain_ok: true,
        tmux_seen: false,
        data_mention: false,
    };
    let mut any_candidate = false;
    for body in &bodies {
        if !has_candidate(body) {
            continue;
        }
        any_candidate = true;
        let inner = reparse(body, depth, ctx);
        verdict.substitutions_ok &=
            !inner.dangerous && (data || (cleared(&inner, body) && !inner.data_mention));
        verdict.tmux_seen |= inner.tmux_seen;
        verdict.data_mention |= inner.data_mention;
    }
    if has_candidate(&rest) {
        let inner = reparse(&rest, depth, ctx);
        verdict.plain_ok = cleared(&inner, &rest);
        verdict.tmux_seen |= inner.tmux_seen;
        verdict.data_mention |= inner.data_mention;
    } else if !any_candidate {
        verdict.plain_ok = false;
    }
    verdict
}

/// Whether an embedded script that mentions `tmux` and `kill-s`, scanned as
/// `inner`, is harmless: it holds at least one tmux invocation, none of them
/// kills, and either every one is scoped or no run of tokens anywhere in
/// `text` would kill. The token test keeps a tmux call that the shell
/// tokeniser cannot see as one, such as the argv list in
/// `python -c 'subprocess.run(["tmux", "kill-server"])'`, from passing as
/// harmless. So `bash -c "tmux kill-session -t amp-x"` gets the same verdict
/// as `tmux kill-session -t amp-x`.
fn cleared(inner: &Scan, text: &str) -> bool {
    inner.tmux_seen && !inner.dangerous && (!inner.unscoped || !text_kills(text))
}

/// Re-parses a word that holds a `tmux` token and an expansion but no
/// `kill-s` token, such as the script in `bash -c 'tmux $c'`. Each
/// substitution body and the rest of the word are judged on their own, so
/// `x=$(tmux ls)` stays allowed.
///
/// A piece that is the whole of `scanned`, the text whose scan found the
/// word, is skipped: it is one word that parses back into that same scan,
/// whose layers 1 and 2 have already read it. Without this a lone word such
/// as `${TMUX_BIN:-tmux}` or the `-S "$SB/tmux"` value would be parsed again
/// until [`MAX_DEPTH`] and denied.
fn expansion_dangerous(word: &str, scanned: &str, depth: usize, ctx: &mut Ctx) -> bool {
    let (bodies, rest) = substitutions(word);
    let mut dangerous = false;
    for text in bodies.iter().chain(std::iter::once(&rest)) {
        if text.trim() != scanned.trim() && needs_reparse(text) {
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

/// Whether `text` holds both a `tmux` token ([`is_tmux_token`]) and a
/// `kill-s` token, separated by whitespace or shell punctuation, in any of
/// the readings of [`any_reading`].
fn has_candidate(text: &str) -> bool {
    any_reading(text, |t| {
        t.iter().any(|w| is_tmux_token(w)) && t.iter().any(|w| starts_kill_s(basename(w)))
    })
}

/// Whether `text` holds a `tmux` token ([`is_tmux_token`]) and a `$` or
/// backtick that could expand to its subcommand.
fn expands_near_tmux(text: &str) -> bool {
    is_expansion(text) && any_reading(text, |t| t.iter().any(|w| is_tmux_token(w)))
}

/// Whether `word` holds a token starting with `kill-s`, wherever it sits in
/// the word: `printf 'ls\nkill-server'` and `echo ' kill-server'` each
/// print a line that a `tmux -C` client would run.
fn mentions_kill_s(word: &str) -> bool {
    any_reading(word, |t| t.iter().any(|w| starts_kill_s(w)))
}

/// Whether some run of tokens in `text` would kill: `kill-server` or a
/// prefix of it, or `kill-session` with `-a` in the tokens after it.
fn text_kills(text: &str) -> bool {
    any_reading(text, kills_in)
}

/// The words of `text` split on whitespace and shell punctuation.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || "_.-/".contains(c)))
        .filter(|t| !t.is_empty())
}

/// Whether `test` holds for the [`tokens`] of any of three readings of
/// `text`. Each reading only adds tokens to test, so a word that one reading
/// finds is never hidden by another.
///
/// 1. The text as written. A backslash separates tokens and the letter after
///    it is kept, as a shell keeps it (`tmux \kill-server`).
/// 2. When the text has a backslash: each one-letter printf escape (`\n`,
///    `\t`, …) read as a separator, as `printf` and `echo -e` read it
///    (`ls\nkill-server`).
/// 3. When the text has a quote or a backslash: the quoting a shell removes
///    from a word undone. Each `$'…'` is decoded, then every `'`, `"` and
///    `\` is dropped, so `t''mux`, `t\mux`, `kill-se""rver` and
///    `$'\x74mux'` read as `tmux` and `kill-server` in an embedded script,
///    where the shell tokeniser has not yet run.
fn any_reading(text: &str, test: impl Fn(&[&str]) -> bool) -> bool {
    if test(&tokens(text).collect::<Vec<_>>()) {
        return true;
    }
    if text.contains('\\') && test(&tokens(&printf_split(text)).collect::<Vec<_>>()) {
        return true;
    }
    text.contains(['\'', '"', '\\']) && test(&tokens(&unquoted(text)).collect::<Vec<_>>())
}

/// `text` with each one-letter printf escape replaced by a space.
fn printf_split(text: &str) -> String {
    let mut split = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            split.push(c);
            continue;
        }
        match chars.next() {
            Some(e) if "abefnrtv".contains(e) => split.push(' '),
            Some(e) => {
                split.push(c);
                split.push(e);
            }
            None => split.push(c),
        }
    }
    split
}

/// `text` with each `$'…'` decoded as bash decodes it and every other `'`,
/// `"` and `\` removed: the third reading of [`any_reading`].
fn unquoted(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        if c == '$'
            && chars.get(i + 1) == Some(&'\'')
            && let Some(close) = shell::ansi_c_close(&chars, i + 1)
        {
            out.push_str(&shell::ansi_c_decode(
                chars.get(i + 2..close).unwrap_or_default(),
            ));
            i = close.saturating_add(1);
            continue;
        }
        if !matches!(c, '\'' | '"' | '\\') {
            out.push(c);
        }
        i += 1;
    }
    out
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

/// Whether a token cut from text that a shell has not yet read names tmux:
/// its last path component is `tmux` or ends in `-tmux`. [`tokens`] keeps a
/// `-` inside a token, so a parameter expansion's fallback stays joined to
/// its operator: `${TMUX_BIN:-tmux}` gives `-tmux`, and `${T-tmux}` gives
/// `T-tmux`. The cost is that a token such as `qa-tmux` or `--no-tmux`
/// counts as well.
fn is_tmux_token(token: &str) -> bool {
    let last = basename(token);
    is_tmux(last)
        || last
            .rsplit_once('-')
            .is_some_and(|(_, tail)| tail.eq_ignore_ascii_case("tmux"))
}

/// Whether a program word is an expansion, so the program it runs is not
/// known here: the word, or its last path component, starts with `$` or a
/// backtick (`$T`, `"$(command -v tmux)"`, `` `echo tmux` ``,
/// `/usr/bin/$T`).
fn unknown_program(word: &str) -> bool {
    [word, basename(word)]
        .iter()
        .any(|w| w.starts_with('$') || w.starts_with('`'))
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

/// git's subcommand: the first word after git's global options.
fn git_subcommand(args: &[String]) -> Option<&str> {
    let mut i = 0usize;
    while let Some(word) = args.get(i) {
        if GIT_VALUE_OPTS.contains(&word.as_str()) {
            i = i.saturating_add(2);
        } else if word.starts_with('-') {
            i += 1;
        } else {
            return Some(word);
        }
    }
    None
}

/// Whether the arguments after `git` name one of [`GIT_COMMAND_RUNNERS`]
/// as the subcommand.
fn git_runs_commands(args: &[String]) -> bool {
    git_subcommand(args).is_some_and(|s| GIT_COMMAND_RUNNERS.contains(&s))
}

/// Whether the arguments after `amplihack` start with `recipe run`.
fn is_recipe_run(args: &[String]) -> bool {
    args.first().is_some_and(|w| w == "recipe") && args.get(1).is_some_and(|w| w == "run")
}

/// Whether word `k` of an `amplihack recipe run` command is a context value:
/// `-c KEY=VALUE`, `-cKEY=VALUE`, `--context KEY=VALUE` or
/// `--context=KEY=VALUE`.
fn is_context_value(words: &[String], k: usize) -> bool {
    let Some(word) = words.get(k) else {
        return false;
    };
    let joined = word.starts_with("--context=")
        || (word.starts_with("-c") && !word.starts_with("--") && word.len() > 2);
    joined
        || k.checked_sub(1)
            .and_then(|p| words.get(p))
            .is_some_and(|prev| prev == "-c" || prev == "--context")
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

/// What a command line holds that the per-command checks in [`scan`] need,
/// gathered in two passes over the commands so those checks stay linear in
/// the number of commands.
struct LineFacts {
    /// Some command on the line is a shell evaluator.
    evaluator: bool,
    /// For each command, in order: whether its stdin may carry a `kill-s`
    /// token, so that an unscoped `tmux -C` there would run it.
    fed_kill_s: Vec<bool>,
}

/// Stdin flows left to right through a pipeline (`a | b` connects a's
/// output to b's input), so a command is fed a `kill-s` token only when an
/// earlier stage of its pipeline has a word holding one. A `kill-s` word in
/// the same or a later stage is not input to it: in
/// `tmux ls | xargs -n1 tmux kill-session -t`, the `tmux ls` reads nothing
/// the `xargs` stage prints. A heredoc body that mentions `kill-s` counts
/// for every stage of its pipeline, whichever stage receives it.
fn line_facts(cmds: &[SimpleCommand]) -> LineFacts {
    let mut evaluator = false;
    let mut heredoc_kill_s: HashSet<usize> = HashSet::new();
    for cmd in cmds {
        evaluator |= is_evaluator(cmd);
        if cmd
            .heredocs
            .iter()
            .any(|b| b.to_ascii_lowercase().contains("kill-s"))
        {
            heredoc_kill_s.insert(cmd.pipeline);
        }
    }
    // The tokeniser numbers pipelines in text order and keeps each
    // pipeline's stages together, so one running flag per pipeline is
    // enough: (pipeline, an earlier stage has a `kill-s` word).
    let mut upstream: Option<(usize, bool)> = None;
    let fed_kill_s = cmds
        .iter()
        .map(|cmd| {
            let earlier = upstream.is_some_and(|(p, seen)| p == cmd.pipeline && seen);
            upstream = Some((cmd.pipeline, earlier || has_kill_s_word(cmd)));
            earlier || heredoc_kill_s.contains(&cmd.pipeline)
        })
        .collect();
    LineFacts {
        evaluator,
        fed_kill_s,
    }
}

fn has_kill_s_word(cmd: &SimpleCommand) -> bool {
    cmd.words.iter().any(|w| mentions_kill_s(w))
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
    // Any later argument may itself be a tmux command (`if-shell true
    // kill-server`, `bind k kill-server`, `run -C 'kill-session -a'`).
    let nested = || text_kills(&rest.get(1..).unwrap_or_default().join(" "));
    let dangerous = !global.scoped
        && if global.uncertain {
            loose_dangerous(rest)
        } else {
            sequence_dangerous(rest) || nested()
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
/// `default` (as a name or a socket file), not a reference to `$TMUX` or to
/// a variable assigned a value that is not scope ([`taint_assignments`]),
/// and not itself a substitution of tmux's output
/// (`"$(tmux display -p '#{socket_path}')"`). Trailing `/` and `}` are
/// ignored when the final component is read, so the default path given as a
/// parameter expansion's fallback (`"${S:-/tmp/tmux-1000/default}"`) is not
/// scope either.
fn scope_value_ok(value: &str, tainted: &HashSet<String>) -> bool {
    !value.is_empty()
        && !refers_to_any(value, tainted)
        && !substitutes_tmux_output(value)
        && !basename(value.trim_end_matches(['/', '}'])).eq_ignore_ascii_case("default")
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
    rest.iter().any(|w| is_expansion(w)) || text_kills(&rest.join(" "))
}

/// Whether any token is `kill-server`, or `kill-session` with `-a` in the
/// tokens after it.
///
/// A `kill-session` token inside the options already read for an earlier
/// one can only be that one's `-t` value, and its own options are the rest
/// of those options, so it is skipped. That reads `kill-session -t
/// kill-session -t …` once, not once per token.
fn kills_in(tokens: &[&str]) -> bool {
    let mut read_to = 0usize;
    for (j, token) in tokens.iter().enumerate() {
        if kills_server(token) {
            return true;
        }
        if j >= read_to && is_kill_session(token) {
            let (all, taken) = session_flags(tokens.get(j + 1..).unwrap_or_default());
            if all {
                return true;
            }
            read_to = j.saturating_add(1).saturating_add(taken);
        }
    }
    false
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
    session_flags(flags).0
}

/// [`kills_all_sessions`], and how many words of `flags` the options took.
fn session_flags(flags: &[&str]) -> (bool, usize) {
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
                _ => return (true, i),
            }
        }
        i = i.saturating_add(if takes_next { 2 } else { 1 });
    }
    (all, i.min(flags.len()))
}

#[cfg(test)]
#[path = "tests/tmux_guard_tests.rs"]
mod tests;
