//! What counts as a `claude` process (A5): the comm or the basename of
//! argv[0] is a Claude binary, or a `node` process runs Claude's script.
//! Wider matching can only make a conversation look running, which is the
//! safe direction. Re-exported by `procfs`.

/// The names that identify Claude Code, injectable for tests (R4.3).
#[derive(Debug, Clone, Copy)]
pub struct ClaudeNames {
    pub binaries: &'static [&'static str],
    pub node_scripts: &'static [&'static str],
}

pub const CLAUDE: ClaudeNames = ClaudeNames {
    binaries: &["claude"],
    node_scripts: &["@anthropic-ai/claude-code/cli.js"],
};

pub fn is_claude(comm: &str, argv: &[String]) -> bool {
    is_claude_with(&CLAUDE, comm, argv)
}

/// A5: the comm or the basename of argv[0] is a Claude binary, or a `node`
/// process runs a Claude script.
pub fn is_claude_with(names: &ClaudeNames, comm: &str, argv: &[String]) -> bool {
    let base = argv.first().map(|a| a.rsplit('/').next().unwrap_or(a));
    if names.binaries.contains(&comm) || base.is_some_and(|b| names.binaries.contains(&b)) {
        return true;
    }
    if comm != "node" && base != Some("node") {
        return false;
    }
    let script = argv.iter().skip(1).find(|a| !a.starts_with('-'));
    script.is_some_and(|s| {
        names
            .node_scripts
            .iter()
            .any(|n| s == n || s.ends_with(&format!("/{n}")))
    })
}
