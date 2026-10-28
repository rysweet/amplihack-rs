//! Issue #1710 (D2): the identity of the session that is running amplihack.
//!
//! `recipe run --detach` records it as a new run's `parent`, and
//! `workstreams status` uses it to find the caller's own runs. The prefix
//! names the variable the value came from, so identities from different
//! sources never match each other.

/// Set to the detached run's tmux session name for everything the run starts.
pub(crate) const RUN_ID_ENV: &str = "AMPLIHACK_RUN_ID";

/// The variables consulted, in order, with the prefix each one gives.
const SOURCES: [(&str, &str); 4] = [
    (RUN_ID_ENV, "run"),
    ("AMPLIHACK_SESSION_ID", "session"),
    ("AMPLIHACK_TREE_ID", "tree"),
    ("CLAUDE_CODE_SESSION_ID", "claude"),
];

/// Longest value accepted from any of the variables.
const MAX_VALUE_BYTES: usize = 128;

/// `<source>:<value>` for the first usable variable, or `None`. A value is
/// usable when it is non-empty, at most 128 bytes and printable ASCII.
/// `get` reads one variable; this function never reads the environment.
pub(crate) fn caller_identity(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    SOURCES.iter().find_map(|(var, prefix)| {
        get(var)
            .filter(|v| usable(v))
            .map(|v| format!("{prefix}:{v}"))
    })
}

/// [`caller_identity`] over this process's environment.
pub(crate) fn caller_identity_from_env() -> Option<String> {
    caller_identity(|k| std::env::var(k).ok())
}

fn usable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && value.bytes().all(|b| (b' '..=b'~').contains(&b))
}
