//! What a supervised launch's Claude arguments say (issue #1618): whether
//! they ask for print mode, and whether they carry the prompt. Claude parses
//! its options with Commander, so this follows Commander's rules over the
//! option table from `claude --help`:
//!
//! - `-p`/`--print` is a boolean flag; the prompt is a positional argument
//!   that can come anywhere. `--print=x` is rejected by Claude, not a prompt.
//! - An option that takes a value skips it: the next argument for `<value>`,
//!   every following non-option argument for `<values...>`, and the next
//!   non-option argument for `[value]`. `--opt=value` and `-nvalue` carry it
//!   inline.
//! - Everything after `--` is positional, so a prompt that starts with `-`
//!   is only reachable there.
//!
//! The tables cover `claude --help` plus the hidden options Claude accepts
//! but does not list; those were found by passing each candidate with no
//! value and getting `error: option '--x <value>' argument missing`.
//!
//! An option missing from every table fails safe: it is read as taking an
//! optional value, so a following non-option word counts as its value, not
//! as the prompt. Claude rejects truly unknown options, so a missing entry
//! only ever hides a real option of a newer Claude. Reading too few prompts
//! is the safe direction: `prompt` only matters when stdin is a pipe, and
//! there `prompt: false` means "no wait, no rerun" (the old behaviour),
//! while a wrongly found prompt would mean a long wait and then a rerun with
//! no prompt. The ignored test `the_tables_cover_claude_help` (run with
//! `--ignored` where `claude` is installed) reports drift.

/// Long options that take exactly one value (`<value>`).
pub(super) const REQUIRED: &[&str] = &[
    "agent",
    "agent-color",
    "agent-id",
    "agent-name",
    "agents",
    "append-system-prompt",
    "append-system-prompt-file",
    "autocompact",
    "debug-file",
    "effort",
    "environment",
    "fallback-model",
    "input-format",
    "json-schema",
    "max-budget-usd",
    "max-thinking-tokens",
    "max-turns",
    "model",
    "name",
    "output-format",
    "parent-session-id",
    "permission-mode",
    "permission-prompt-tool",
    "permission-prompts",
    "plugin-dir",
    "plugin-url",
    "prefill",
    "remote-control-session-name-prefix",
    "resume-session-at",
    "rewind-files",
    "sdk-url",
    "session-id",
    "setting-sources",
    "settings",
    "system-prompt",
    "system-prompt-file",
    "system-prompt-snapshot",
    "team-name",
    "teammate-mode",
    "thinking",
    "workload",
];

/// Long options that take one or more values (`<values...>`).
pub(super) const VARIADIC: &[&str] = &[
    "add-dir",
    "allowedTools",
    "allowed-tools",
    "betas",
    "channels",
    "disallowedTools",
    "disallowed-tools",
    "file",
    "mcp-config",
    "tools",
];

/// Long options with an optional value (`[value]`).
pub(super) const OPTIONAL: &[&str] = &[
    "cloud",
    "debug",
    "from-pr",
    "prompt-suggestions",
    "remote-control",
    "resume",
    "teleport",
    "worktree",
];

/// Long boolean flags (no value). Anything in no table is read as
/// `Optional`; see the module comment.
pub(super) const FLAGS: &[&str] = &[
    "allow-dangerously-skip-permissions",
    "ax-screen-reader",
    "background",
    "bare",
    "bg",
    "brief",
    "chrome",
    "continue",
    "dangerously-skip-permissions",
    "desktop",
    "disable-slash-commands",
    "exclude-dynamic-system-prompt-sections",
    "fork-session",
    "forward-subagent-text",
    "help",
    "ide",
    "include-hook-events",
    "include-partial-messages",
    "no-chrome",
    "no-session-persistence",
    "print",
    "replay-user-messages",
    "restricted",
    "safe-mode",
    "strict-mcp-config",
    "tmux",
    "verbose",
    "version",
];

/// Short options: `-c`, `-h`, `-p` and `-v` are flags; `-n` takes a value;
/// `-d`, `-r` and `-w` an optional one. An unknown letter fails safe as
/// `Optional`, like an unknown long option.
fn short_kind(c: char) -> Kind {
    match c {
        'c' | 'h' | 'p' | 'v' => Kind::Flag,
        'n' => Kind::Required,
        _ => Kind::Optional,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Flag,
    Required,
    Variadic,
    Optional,
}

fn long_kind(name: &str) -> Kind {
    if REQUIRED.contains(&name) {
        Kind::Required
    } else if VARIADIC.contains(&name) {
        Kind::Variadic
    } else if OPTIONAL.contains(&name) {
        Kind::Optional
    } else if FLAGS.contains(&name) {
        Kind::Flag
    } else {
        // Unknown: fail safe (see the module comment).
        Kind::Optional
    }
}

/// What the arguments ask for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ClaudeArgs {
    /// `-p` or `--print` is set.
    pub(crate) print: bool,
    /// A non-empty positional argument (the prompt) is present. May be
    /// `false` for a prompt that follows an option in no table, which is
    /// the safe side (see the module comment). Only an unknown option with
    /// two or more values (`<values...>`) can still leave a value counted.
    pub(crate) prompt: bool,
}

/// Reads Claude's arguments as Commander would.
pub(crate) fn parse<S: AsRef<str>>(args: &[S]) -> ClaudeArgs {
    let mut out = ClaudeArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_ref();
        i += 1;
        if arg == "--" {
            out.prompt |= args[i..].iter().any(|a| !a.as_ref().is_empty());
            break;
        }
        let kind = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, false),
            };
            out.print |= name == "print" && !inline;
            if inline { Kind::Flag } else { long_kind(name) }
        } else if let Some(shorts) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            short_cluster(shorts, &mut out)
        } else {
            out.prompt |= !arg.is_empty() && arg != "-";
            Kind::Flag
        };
        i = skip_values(args, i, kind);
    }
    out
}

/// A cluster such as `-p`, `-pc` or `-nNAME`: sets `print` for `p`, and
/// returns what the last option still needs from the following arguments.
fn short_cluster(shorts: &str, out: &mut ClaudeArgs) -> Kind {
    for (at, c) in shorts.char_indices() {
        let kind = short_kind(c);
        if kind != Kind::Flag {
            // The rest of the cluster, if any, is this option's value.
            let rest = &shorts[at + c.len_utf8()..];
            return if rest.is_empty() { kind } else { Kind::Flag };
        }
        out.print |= c == 'p';
    }
    Kind::Flag
}

/// The index after the values an option of `kind` takes from `args[i..]`.
fn skip_values<S: AsRef<str>>(args: &[S], i: usize, kind: Kind) -> usize {
    let is_value = |a: &S| !a.as_ref().starts_with('-');
    match kind {
        Kind::Flag => i,
        Kind::Required => (i + 1).min(args.len()),
        Kind::Optional => i + usize::from(args.get(i).is_some_and(is_value)),
        Kind::Variadic => i + args[i..].iter().take_while(|a| is_value(a)).count(),
    }
}
