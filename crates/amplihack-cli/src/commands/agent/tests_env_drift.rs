//! Drift guard for `env_record.rs`: no variable that `amplihack claude` sets
//! on the process it starts may be on the allowlist, or a resume would
//! replay a value the launcher meant to set itself. `AMPLIHACK_AGENT_BINARY`
//! is the one exception: the user may set it, and the launcher only exports
//! it through the agent-binary methods (design 4.6).
//!
//! API under test: `env_record::ALLOWLIST: &[&str]`.

use super::env_record::ALLOWLIST;
use std::path::Path;

/// Names passed as string literals to `.set(`, `.env(` or `.env_remove(` in
/// the env builder and the launch command, test files excluded.
fn launcher_set_names() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut names = Vec::new();
    for dir in ["env_builder", "commands/launch"] {
        for e in std::fs::read_dir(src.join(dir)).unwrap() {
            let p = e.unwrap().path();
            let file = p.file_name().unwrap().to_string_lossy().into_owned();
            if !file.ends_with(".rs") || file.starts_with("tests_") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for call in [".set(", ".env(", ".env_remove("] {
                for (at, _) in text.match_indices(call) {
                    let rest = text[at + call.len()..].trim_start();
                    let Some(lit) = rest.strip_prefix('"') else {
                        continue;
                    };
                    let name = &lit[..lit.find('"').unwrap_or(0)];
                    if !name.is_empty()
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit())
                    {
                        names.push(name.to_string());
                    }
                }
            }
        }
    }
    names
}

#[test]
fn no_variable_the_launcher_sets_is_recorded() {
    let set = launcher_set_names();
    assert!(
        set.iter().any(|n| n == "AMPLIHACK_SESSION_ID"),
        "the scan found nothing; did the launcher move? {set:?}"
    );
    let mut also: Vec<String> = amplihack_utils::agent_binary::SESSION_MARKERS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    also.extend(
        [
            "AMPLIHACK_DEPTH",
            "AMPLIHACK_SESSION_DEPTH",
            "AMPLIHACK_TREE_ID",
            "CLAUDE_CONFIG_DIR",
        ]
        .map(String::from),
    );
    let overlap: Vec<&String> = set
        .iter()
        .chain(&also)
        .filter(|n| ALLOWLIST.contains(&n.as_str()) && n.as_str() != "AMPLIHACK_AGENT_BINARY")
        .collect();
    assert!(
        overlap.is_empty(),
        "remove from ALLOWLIST in env_record.rs: {overlap:?}"
    );
}
