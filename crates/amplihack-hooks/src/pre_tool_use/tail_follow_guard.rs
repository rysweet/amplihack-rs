//! Issue #1794: deny `tail` with a follow option on a device that never ends.
//!
//! Agents that are told not to call `sleep` reach for `tail -f /dev/null`, or
//! `timeout N tail -f /dev/null`, as a way to wait. The follow never ends on
//! its own, and when the tool call is cancelled or times out the `tail` is
//! left running as an orphan. This guard refuses those forms in every shell
//! tool and names the supported ways to wait instead: `amplihack wait
//! --seconds N`, or running the work in the background and waiting on its
//! PID (`cmd & pid=$!; wait $pid`).
//!
//! # What is denied
//!
//! A simple command whose program word is `tail` (after the wrappers that
//! [`shell::program`] strips, and the launch wrappers `setsid`, `stdbuf`,
//! `ionice`, `chrt`, `taskset` and `unbuffer`, which this module strips
//! itself) with a follow option (`-f`, `-F`, `--follow`,
//! `--follow=…`, a shortened form such as `--f`, `--fo` or `--foll=name`, or
//! `f`/`F` in a short cluster before an option that takes a value) and an
//! operand that names one of [`DEVICES`] once repeated `/` and `/./` are
//! collapsed. Options are read anywhere among the operands, as GNU and uutils
//! `tail` read them, until `--`.
//!
//! The `-c` operand of `sh`, `bash`, `zsh`, `dash` and `ksh` is parsed again,
//! and so is each heredoc body fed to one of those shells when there is no
//! `-c`. A heredoc fed to any other program (`cat > Dockerfile <<EOF`) is
//! data and is not read. The body of each command substitution the shell
//! runs, `$(…)` or backticks, bare or in double quotes, is parsed again too,
//! so `x=$(timeout 5 tail -f /dev/null)` is denied; one inside single quotes,
//! `$'…'`, a comment or a heredoc body is not read.
//!
//! # Failing open
//!
//! Unlike the tmux guard, which protects the user's tmux server and fails
//! closed, this guard allows whenever it cannot be sure: input over
//! [`MAX_INPUT`], a parse that is not complete, nesting at [`MAX_DEPTH`], an
//! exhausted re-parse budget ([`MAX_REPARSES`]) and more than [`MAX_UNWRAP`]
//! wrappers in a row. A miss leaves an orphaned `tail`; a false deny would
//! block a legitimate command.
//!
//! # Known limits
//!
//! Device paths given by expansion (`$DEV`, `$(…)`, globs), paths with `..`,
//! symlinks to a device, `xargs`, `find -exec`, `eval`, `env -S`, scripts run
//! from files, multi-call wrappers such as `busybox tail`, launch wrappers
//! other than those named above (`flock`, `systemd-run`, `cgexec`,
//! `firejail` and others), a script given
//! to a shell as a here-string (`bash <<< '…'`) or through a pipe
//! (`echo '…' | bash`), a substitution inside an expanding heredoc fed to a
//! program that is not a shell, and nesting deeper than three shells are not
//! matched. The last few would need data-flow tracking, and this guard is
//! not a security boundary.
//!
//! The guard reads no environment, file or clock, and nothing turns it off.

use super::shell::{self, SHELLS, SimpleCommand, basename, substitution_bodies};
use serde_json::Value;
use tracing::warn;

/// The deny message. It never contains the command.
const BLOCK_MESSAGE: &str = "Blocked: `tail` following a device that never ends \
(such as `tail -f /dev/null`) must not be used as a way to wait. The tail outlives \
the tool call and is left running as an orphan process (#1794). To wait for a fixed \
time, run `amplihack wait --seconds N`. To wait for work to finish, run it in the \
background and wait on its PID: `cmd & pid=$!; wait $pid`. Do not use \
`tail -f /dev/null` or `timeout N tail -f /dev/null` as a sleep.";

/// Devices a follow never finishes on, after [`normalise`].
const DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/full",
    "/dev/stdin",
    "/dev/tty",
    "/dev/fd/0",
    "/proc/self/fd/0",
];

/// `tail` long options that take a value as the next word or after `=`.
const TAIL_LONG_VALUE_OPTS: &[&str] = &[
    "lines",
    "bytes",
    "pid",
    "sleep-interval",
    "max-unchanged-stats",
];

/// `tail` short options that take a value: the rest of the cluster, or the
/// next word when the cluster ends with them.
const TAIL_SHORT_VALUE_OPTS: &[char] = &['n', 'c', 's'];

/// Text at this depth or deeper is not scanned; the top level is depth 0.
const MAX_DEPTH: usize = 4;

/// Larger input is allowed without being read.
const MAX_INPUT: usize = 64 * 1024;

/// Most re-parses of embedded scripts for one command line.
const MAX_REPARSES: usize = 256;

/// Most launch-wrapper / `function` / [`shell::WRAPPERS`] rounds in one
/// command.
const MAX_UNWRAP: usize = 8;

/// The deny response for a shell tool's input, or `None`.
///
/// Reads `command`, then `cmd`. A string is checked as a command line. An
/// array of strings (the Codex argv form) is shell-quoted and joined first;
/// an array holding anything else is allowed. Any other shape is not a
/// command.
pub(super) fn check_tool_input(tool_input: &Value) -> Option<Value> {
    ["command", "cmd"]
        .iter()
        .find_map(|key| match tool_input.get(key) {
            Some(Value::String(text)) => check(text),
            Some(Value::Array(items)) if !items.is_empty() => {
                let strings: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
                if strings.len() != items.len() {
                    return None;
                }
                // `check` applies MAX_INPUT to the joined string as well.
                check(&shell::join_argv(&strings))
            }
            _ => None,
        })
}

/// The deny response for the command line `command`, or `None`.
pub(super) fn check(command: &str) -> Option<Value> {
    if command.len() > MAX_INPUT {
        return None;
    }
    let mut budget = MAX_REPARSES;
    scan(command, 0, &mut budget).then(deny_response)
}

fn deny_response() -> Value {
    warn!("tail-follow guard denied tail following a never-ending device (#1794)");
    super::deny(BLOCK_MESSAGE)
}

/// Whether `text`, read at `depth`, runs a `tail` that follows a device.
fn scan(text: &str, depth: usize, budget: &mut usize) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    let parsed = shell::tokenise(text);
    if !parsed.complete {
        return false;
    }
    parsed
        .commands
        .iter()
        .any(|command| scan_command(command, depth, budget))
        || substitution_bodies(text)
            .iter()
            .any(|body| reparse(body, depth, budget))
}

fn scan_command(command: &SimpleCommand, depth: usize, budget: &mut usize) -> bool {
    let mut rest: &[String] = &command.words;
    for _ in 0..=MAX_UNWRAP {
        let Some(words) = shell::program(rest) else {
            return false;
        };
        let Some((first, args)) = words.split_first() else {
            return false;
        };
        let name = basename(first);
        match name {
            "setsid" => rest = skip_setsid_options(args),
            "stdbuf" => rest = skip_stdbuf_options(args),
            "ionice" => rest = skip_ionice_options(args),
            "chrt" => rest = skip_chrt_options(args),
            "taskset" => rest = skip_taskset_options(args),
            "unbuffer" => rest = skip_unbuffer_options(args),
            // `function f { …; }`: the body follows the name. The shared
            // tokeniser does not treat `function` as a reserved word.
            "function" => rest = args.get(1..).unwrap_or_default(),
            "tail" => return tail_follows_device(args),
            _ if SHELLS.contains(&name) => {
                return scan_shell(args, &command.heredocs, depth, budget);
            }
            _ => return false,
        }
    }
    // More wrappers than MAX_UNWRAP: fail open.
    false
}

/// The words after `setsid`'s options (`-w`, `-f`, `-c` and long forms).
fn skip_setsid_options(args: &[String]) -> &[String] {
    let skip = args.iter().take_while(|w| w.starts_with('-')).count();
    args.get(skip..).unwrap_or_default()
}

/// The words after `stdbuf`'s options. `-i`, `-o` and `-e` take their value
/// joined (`-oL`) or as the next word (`-o L`); so do `--input`, `--output`
/// and `--error`, joined with `=`.
fn skip_stdbuf_options(args: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = args.get(i) {
        if !w.starts_with('-') {
            break;
        }
        let separate_value = matches!(
            w.as_str(),
            "-i" | "-o" | "-e" | "--input" | "--output" | "--error"
        );
        i += if separate_value { 2 } else { 1 };
    }
    args.get(i..).unwrap_or_default()
}

/// The words after `ionice`'s options. `-c`, `-n`, `-p`, `-P` and `-u`
/// take their value joined (`-c3`) or as the next word (`-c 3`); the long
/// forms take it after `=` or as the next word. With `-p`, `-P` or `-u`,
/// `ionice` acts on running processes and starts no command, so nothing is
/// returned.
fn skip_ionice_options(args: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = args.get(i) {
        if !w.starts_with('-') || w == "-" {
            break;
        }
        i += 1;
        if w == "--" {
            break;
        }
        let (name, joined) = match w.strip_prefix("--") {
            Some(long) => match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, false),
            },
            None => {
                let mut chars = w.chars().skip(1);
                let Some(c) = chars.next() else { break };
                let joined = chars.next().is_some();
                match c {
                    'c' => ("class", joined),
                    'n' => ("classdata", joined),
                    'p' => ("pid", joined),
                    'P' => ("pgid", joined),
                    'u' => ("uid", joined),
                    _ => ("", joined),
                }
            }
        };
        if matches!(name, "pid" | "pgid" | "uid") {
            return &[];
        }
        if !joined && matches!(name, "class" | "classdata") {
            i += 1;
        }
    }
    args.get(i..).unwrap_or_default()
}

/// The words after `chrt`'s options and its priority. `-T`, `-P` and `-D`
/// (and `--sched-runtime`, `--sched-period`, `--sched-deadline`) take a
/// value; the policy flags do not. A priority that is all digits is skipped,
/// since util-linux lets it be left out for some policies. With `-p` or
/// `--pid`, `chrt` acts on a running process and starts no command, so
/// nothing is returned.
fn skip_chrt_options(args: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = args.get(i) {
        if !w.starts_with('-') || w == "-" {
            break;
        }
        i += 1;
        if w == "--" {
            break;
        }
        if w == "--pid" || (!w.starts_with("--") && w.contains('p')) {
            return &[];
        }
        let separate_value = matches!(
            w.as_str(),
            "-T" | "-P" | "-D" | "--sched-runtime" | "--sched-period" | "--sched-deadline"
        );
        if separate_value {
            i += 1;
        }
    }
    if args
        .get(i)
        .is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()))
    {
        i += 1;
    }
    args.get(i..).unwrap_or_default()
}

/// The words after `taskset`'s options and its CPU mask or list. No option
/// takes a value; the mask (or the list, with `-c`) is the first word that
/// is not an option. With `-p` or `--pid`, `taskset` acts on a running
/// process and starts no command, so nothing is returned.
fn skip_taskset_options(args: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = args.get(i) {
        if !w.starts_with('-') || w == "-" {
            break;
        }
        i += 1;
        if w == "--" {
            break;
        }
        if w == "--pid" || (!w.starts_with("--") && w.contains('p')) {
            return &[];
        }
    }
    // The mask or CPU list.
    args.get(i + 1..).unwrap_or_default()
}

/// The words after `unbuffer`'s one option, `-p`.
fn skip_unbuffer_options(args: &[String]) -> &[String] {
    let skip = args.iter().take_while(|w| w.as_str() == "-p").count();
    args.get(skip..).unwrap_or_default()
}

/// Scans the `-c` operand of a shell, or the heredoc bodies it reads as its
/// script when there is no `-c`.
fn scan_shell(args: &[String], heredocs: &[String], depth: usize, budget: &mut usize) -> bool {
    shell::shell_scripts(args, heredocs)
        .into_iter()
        .any(|script| reparse(script, depth, budget))
}

/// Scans `script` one level deeper, spending one unit of `budget`.
fn reparse(script: &str, depth: usize, budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    scan(script, depth + 1, budget)
}

/// Whether `tail`'s arguments `args` follow (`-f`, `-F`, `--follow…`) and
/// name a device in [`DEVICES`].
fn tail_follows_device(args: &[String]) -> bool {
    let mut follow = false;
    let mut device = false;
    let mut options_done = false;
    let mut i = 0;
    while let Some(w) = args.get(i) {
        i += 1;
        if options_done {
            device |= is_device(w);
            continue;
        }
        if w == "--" {
            options_done = true;
            continue;
        }
        if let Some(long) = w.strip_prefix("--") {
            let (name, joined) = match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, false),
            };
            // `tail` accepts any unique prefix of a long option, and
            // `follow` is the only one starting with `f`.
            if !name.is_empty() && "follow".starts_with(name) {
                follow = true;
            } else if !joined && TAIL_LONG_VALUE_OPTS.contains(&name) {
                i += 1;
            }
            continue;
        }
        if let Some(cluster) = w.strip_prefix('-')
            && !cluster.is_empty()
        {
            let (cluster_follows, value_next) = short_cluster(cluster);
            follow |= cluster_follows;
            if value_next {
                i += 1;
            }
            continue;
        }
        // An operand: a file, `-` (standard input) or `+N…`.
        device |= is_device(w);
    }
    follow && device
}

/// Reads the short option cluster `cluster` (the word without its `-`).
/// Returns whether it holds a follow flag and whether the next word is the
/// value of its last option.
fn short_cluster(cluster: &str) -> (bool, bool) {
    if cluster.starts_with(|c: char| c.is_ascii_digit()) {
        // Old-style `-5f`, `-10lf`: a count, a unit, then `f` to follow.
        let follow = cluster
            .chars()
            .skip_while(char::is_ascii_digit)
            .any(|c| c == 'f');
        return (follow, false);
    }
    let mut chars = cluster.chars();
    let mut follow = false;
    while let Some(c) = chars.next() {
        if c == 'f' || c == 'F' {
            follow = true;
        } else if TAIL_SHORT_VALUE_OPTS.contains(&c) {
            // The rest of the cluster is the value, or the next word is.
            return (follow, chars.as_str().is_empty());
        }
    }
    (follow, false)
}

/// Whether `word` names one of [`DEVICES`] once repeated `/` and `/./` are
/// collapsed. No filesystem access: `..` and symlinks are not resolved.
fn is_device(word: &str) -> bool {
    normalise(word).is_some_and(|path| DEVICES.contains(&path.as_str()))
}

/// `word` with empty and `.` segments removed, or `None` when it is not an
/// absolute path.
fn normalise(word: &str) -> Option<String> {
    if !word.starts_with('/') {
        return None;
    }
    let mut path = String::with_capacity(word.len());
    for segment in word.split('/').filter(|s| !s.is_empty() && *s != ".") {
        path.push('/');
        path.push_str(segment);
    }
    Some(path)
}

#[cfg(test)]
#[path = "tests/tail_follow_guard_tests.rs"]
mod tests;
