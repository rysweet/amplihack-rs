//! Codex option tables, one per scope, copied from codex-cli 0.166.0's help.
//!
//! This is the only copy in amplihack. `amplihack-cli`'s `codex_exec_args`
//! (the prompt rewrite), this crate's `codex_args` classifier and
//! `amplihack-cli`'s `codex_agent_step` (agent-step flag injection) all read
//! these tables. `amplihack-cli/src/commands/launch/tests_codex_exec_args.rs`
//! checks each table against the saved help in
//! `amplihack-cli/src/commands/launch/fixtures/`; a new Codex release is
//! added by saving its help there and updating the tables here until that
//! test passes.

use Arity::{Flag, Many, One};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Arity {
    Flag,
    One,
    Many,
}

/// `(long name without --, short letter, arity, hidden)`. A hidden option is
/// accepted by Codex but not listed in its help (`--yolo`).
pub type Opt = (&'static str, Option<char>, Arity, bool);

pub const ROOT: &[Opt] = &[
    ("config", Some('c'), One, false),
    ("enable", None, One, false),
    ("disable", None, One, false),
    ("remote", None, One, false),
    ("remote-auth-token-env", None, One, false),
    ("strict-config", None, Flag, false),
    ("image", Some('i'), Many, false),
    ("model", Some('m'), One, false),
    ("oss", None, Flag, false),
    ("local-provider", None, One, false),
    ("profile", Some('p'), One, false),
    ("sandbox", Some('s'), One, false),
    ("approve-for-me", None, Flag, false),
    (
        "dangerously-bypass-approvals-and-sandbox",
        None,
        Flag,
        false,
    ),
    ("dangerously-bypass-hook-trust", None, Flag, false),
    ("cd", Some('C'), One, false),
    ("worktree", None, Flag, false),
    ("add-dir", None, One, false),
    ("ask-for-approval", Some('a'), One, false),
    ("search", None, Flag, false),
    ("no-alt-screen", None, Flag, false),
    ("no-daemon", None, Flag, false),
    ("help", Some('h'), Flag, false),
    ("version", Some('V'), Flag, false),
    ("yolo", None, Flag, true),
];

pub const EXEC: &[Opt] = &[
    ("config", Some('c'), One, false),
    ("enable", None, One, false),
    ("disable", None, One, false),
    ("strict-config", None, Flag, false),
    ("image", Some('i'), Many, false),
    ("model", Some('m'), One, false),
    ("oss", None, Flag, false),
    ("local-provider", None, One, false),
    ("profile", Some('p'), One, false),
    ("sandbox", Some('s'), One, false),
    ("approve-for-me", None, Flag, false),
    (
        "dangerously-bypass-approvals-and-sandbox",
        None,
        Flag,
        false,
    ),
    ("dangerously-bypass-hook-trust", None, Flag, false),
    ("cd", Some('C'), One, false),
    ("worktree", None, Flag, false),
    ("add-dir", None, One, false),
    ("thread-source", None, One, false),
    ("cyber-access-program", None, One, false),
    ("skip-git-repo-check", None, Flag, false),
    ("ephemeral", None, Flag, false),
    ("ignore-user-config", None, Flag, false),
    ("ignore-rules", None, Flag, false),
    ("output-schema", None, One, false),
    ("color", None, One, false),
    ("json", None, Flag, false),
    ("output-last-message", Some('o'), One, false),
    ("help", Some('h'), Flag, false),
    ("version", Some('V'), Flag, false),
    ("yolo", None, Flag, true),
];

pub const REVIEW: &[Opt] = &[
    ("config", Some('c'), One, false),
    ("uncommitted", None, Flag, false),
    ("base", None, One, false),
    ("enable", None, One, false),
    ("commit", None, One, false),
    ("disable", None, One, false),
    ("strict-config", None, Flag, false),
    ("title", None, One, false),
    ("model", Some('m'), One, false),
    (
        "dangerously-bypass-approvals-and-sandbox",
        None,
        Flag,
        false,
    ),
    ("dangerously-bypass-hook-trust", None, Flag, false),
    ("worktree", None, Flag, false),
    ("thread-source", None, One, false),
    ("cyber-access-program", None, One, false),
    ("skip-git-repo-check", None, Flag, false),
    ("ephemeral", None, Flag, false),
    ("ignore-user-config", None, Flag, false),
    ("ignore-rules", None, Flag, false),
    ("output-schema", None, One, false),
    ("json", None, Flag, false),
    ("output-last-message", Some('o'), One, false),
    ("help", Some('h'), Flag, false),
    ("yolo", None, Flag, true),
];

/// Top-level `codex review`, a non-interactive review that reads `-` from
/// stdin like `exec review`, but with a shorter option list.
pub const TOP_REVIEW: &[Opt] = &[
    ("config", Some('c'), One, false),
    ("strict-config", None, Flag, false),
    ("enable", None, One, false),
    ("uncommitted", None, Flag, false),
    ("base", None, One, false),
    ("disable", None, One, false),
    ("commit", None, One, false),
    ("title", None, One, false),
    ("help", Some('h'), Flag, false),
];

pub const RESUME: &[Opt] = &[
    ("config", Some('c'), One, false),
    ("last", None, Flag, false),
    ("all", None, Flag, false),
    ("enable", None, One, false),
    ("disable", None, One, false),
    ("image", Some('i'), One, false),
    ("strict-config", None, Flag, false),
    ("model", Some('m'), One, false),
    (
        "dangerously-bypass-approvals-and-sandbox",
        None,
        Flag,
        false,
    ),
    ("dangerously-bypass-hook-trust", None, Flag, false),
    ("worktree", None, Flag, false),
    ("thread-source", None, One, false),
    ("cyber-access-program", None, One, false),
    ("skip-git-repo-check", None, Flag, false),
    ("ephemeral", None, Flag, false),
    ("ignore-user-config", None, Flag, false),
    ("ignore-rules", None, Flag, false),
    ("output-schema", None, One, false),
    ("json", None, Flag, false),
    ("output-last-message", Some('o'), One, false),
    ("help", Some('h'), Flag, false),
    ("yolo", None, Flag, true),
];

pub const FORK: &[Opt] = &[
    ("config", Some('c'), One, false),
    ("image", Some('i'), One, false),
    ("enable", None, One, false),
    ("disable", None, One, false),
    ("strict-config", None, Flag, false),
    ("model", Some('m'), One, false),
    (
        "dangerously-bypass-approvals-and-sandbox",
        None,
        Flag,
        false,
    ),
    ("dangerously-bypass-hook-trust", None, Flag, false),
    ("worktree", None, Flag, false),
    ("thread-source", None, One, false),
    ("cyber-access-program", None, One, false),
    ("skip-git-repo-check", None, Flag, false),
    ("ephemeral", None, Flag, false),
    ("ignore-user-config", None, Flag, false),
    ("ignore-rules", None, Flag, false),
    ("output-schema", None, One, false),
    ("json", None, Flag, false),
    ("output-last-message", Some('o'), One, false),
    ("help", Some('h'), Flag, false),
    ("yolo", None, Flag, true),
];

/// The scopes a command line reaches before its prompt: options before the
/// subcommand, `exec`, `exec resume` and `exec fork`. The value tests below
/// ask whether an option takes a value in any of them.
const PROMPT_SCOPES: [&[Opt]; 4] = [ROOT, EXEC, RESUME, FORK];

fn find(name: &str) -> Option<Arity> {
    if let Some(long) = name.strip_prefix("--") {
        PROMPT_SCOPES
            .iter()
            .flat_map(|t| t.iter())
            .filter(|(n, _, _, _)| *n == long)
            .map(|(_, _, a, _)| *a)
            .max()
    } else {
        let mut chars = name.strip_prefix('-')?.chars();
        let letter = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        PROMPT_SCOPES
            .iter()
            .flat_map(|t| t.iter())
            .filter(|(_, s, _, _)| *s == Some(letter))
            .map(|(_, _, a, _)| *a)
            .max()
    }
}

/// `--name` or `-x` takes a value (one or more) in some prompt scope.
pub fn takes_value(name: &str) -> bool {
    matches!(find(name), Some(One | Many))
}

/// `--name` or `-x` is a flag (takes no value) in every prompt scope that
/// lists it.
pub fn is_flag(name: &str) -> bool {
    find(name) == Some(Flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_lookup_reads_the_scope_tables() {
        for name in [
            "--model",
            "-m",
            "--config",
            "-c",
            "--image",
            "-i",
            "--add-dir",
            "--cd",
            "-C",
            "--output-last-message",
            "-o",
            "--thread-source",
            "--cyber-access-program",
            "--remote",
            "-a",
            "-p",
            "-s",
        ] {
            assert!(takes_value(name), "{name}");
        }
        for name in [
            "--json",
            "--last",
            "--yolo",
            "-h",
            "-V",
            "--skip-git-repo-check",
        ] {
            assert!(!takes_value(name) && is_flag(name), "{name}");
        }
        // `exec review` options are outside the prompt scopes.
        for name in ["--commit", "--base", "--unknown", "-x", "-mx", "-"] {
            assert!(!takes_value(name) && !is_flag(name), "{name}");
        }
    }
}
