//! Root-anchored exclusions for locations owned by the framework.
//! Configuration such as `.amplihack/project.toml` remains visible.
pub const EXCLUSIONS: &[&str] = &[
    "/index.scip",
    "/index.scip.backup",
    "/indexing.pid",
    "/.amplihack/runtime/",
    "/.amplihack/graph_db.wal",
    "/.amplihack/graph_db.lock",
    "/.amplihack/kuzu_db.wal",
    "/.amplihack/kuzu_db.lock",
    "/.claude/runtime/",
    "/worktrees/",
    "/.amplihack/graph_db",
    "/.amplihack/kuzu_db",
    "/.amplihack/indexes/",
    "/.amplihack/index.scip",
    "/.amplihack/index.scip.backup",
    "/.amplihack/indexing.pid",
    "/.amplihack/blarify.json",
    "/.amplihack/locks/",
    "/.amplihack/metrics/",
    "/.amplihack/classification_state/",
    "/.amplihack/session-state/",
];

/// Publication is stricter than collection: all framework state is prohibited.
pub fn prohibited(path: &str) -> bool {
    let mut parts = path.split('/');
    match parts.next() {
        Some(root) if root.eq_ignore_ascii_case(".amplihack") => true,
        Some(root) if root.eq_ignore_ascii_case(".claude") => parts
            .next()
            .is_some_and(|p| p.eq_ignore_ascii_case("runtime")),
        _ => false,
    }
}
