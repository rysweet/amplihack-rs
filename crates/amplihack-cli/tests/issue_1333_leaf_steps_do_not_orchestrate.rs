//! Issue #1333 / #1336 — a leaf agent step must not be told to orchestrate.
//!
//! A `default-workflow` run once spent 2h47m and produced zero commits. Every
//! design step shelled out to a full nested `smart-orchestrator`, each of those
//! calls was cut off at exactly `10m 0s` by the Copilot shell tool's default,
//! and one child hit the recursion ceiling at depth 4 of 3 and returned exit 79.
//!
//! None of that was a timeout bug or a depth-limit bug. The steps are declared
//! as `agent:` steps, so the shell-out was the agent's own choice — and it made
//! that choice because the repository root carried a tracked `AGENTS.md` holding
//! a verbatim copy of the routing prompt and the orchestrator skill body, both
//! phrased as mandatory instructions.
//!
//! Copilot CLI loads that file unconditionally as a custom instruction, and
//! every git worktree the workflow creates carries a copy. So every leaf agent —
//! including recipe steps already inside an orchestration — was instructed to
//! start another one. The provenance gate from #1328 could not help: it gates
//! the *hook*, and this arrived through a different channel entirely.
//!
//! The file was committed by accident in `d3341a78` and nothing writes it; the
//! writer was removed by #862, whose own regression test calls `AGENTS.md` "the
//! instruction channel Copilot re-ingests". This test is what would have caught
//! that commit.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/crates/amplihack-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Instruction text that turns a reader into an orchestrator. Any of these in a
/// file that every agent loads is a re-entry hazard, whatever the intent.
const ROUTING_MARKERS: &[(&str, &str)] = &[
    (
        "auto-intent-router",
        "the routing prompt's own source marker",
    ),
    (
        "auto-routed",
        "the literal token an agent is told to emit before orchestrating",
    ),
    (
        "AMPLIHACK_CONTEXT_START",
        "the generated-context marker; nothing writes it any more",
    ),
    (
        "recipe run smart-orchestrator",
        "the shell-out that turns a leaf step into a nested orchestration",
    ),
    (
        "Skill(skill=\"dev-orchestrator\")",
        "a direct instruction to invoke the orchestrator skill",
    ),
    (
        "invoke the dev-orchestrator skill",
        "the same instruction in prose",
    ),
    (
        "launching dev-orchestrator",
        "the phrase an agent is told to emit as it re-enters",
    ),
];

/// Files a CLI loads into every agent's context automatically, with no
/// provenance gate. Routing instructions must not live in any of them.
///
/// The list is deliberately wider than the one file that actually broke. That
/// file reached eleven checkouts on the affected host, including two
/// repositories that are not amplihack clones — it travelled by clone and by
/// worktree. Guarding only the instance that failed would leave the same class
/// open in every sibling channel. The rest are clean today, so this costs
/// nothing to keep green.
const UNGATED_INSTRUCTION_FILES: &[&str] = &[
    "AGENTS.md",
    ".github/agents/AGENTS.md",
    "CLAUDE.md",
    ".github/copilot-instructions.md",
    ".cursorrules",
    ".windsurfrules",
    "GEMINI.md",
];

/// Collapse runs of whitespace and fold case, so a reflowed paragraph or a
/// different quote style still matches.
fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push(' ');
        }
        in_space = false;
        out.extend(ch.to_lowercase());
    }
    out
}

#[test]
fn ungated_instruction_files_carry_no_routing_instructions() {
    let root = repo_root();
    let mut findings = Vec::new();

    for rel in UNGATED_INSTRUCTION_FILES {
        let path = root.join(rel);
        // Absent is the ideal state; only content can be wrong.
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Normalise before matching. An exact, case-sensitive search dies to a
        // line wrap or a quote swap, and the block this guard was written for
        // wrapped `recipe run smart-orchestrator` across lines in some
        // revisions. Collapse whitespace and fold case so reformatting the file
        // cannot silently disarm the check.
        let haystack = normalise(&body);
        for (marker, why) in ROUTING_MARKERS {
            if haystack.contains(&normalise(marker)) {
                findings.push(format!("  {rel}  contains {marker:?} — {why}"));
            }
        }
    }

    assert!(
        findings.is_empty(),
        "routing instructions found in a file every agent loads:\n{}\n\n\
         Every git worktree the workflow creates carries a copy of these files, \
         and Copilot CLI loads them unconditionally as custom instructions, so a \
         recipe step already inside an orchestration is told to start another one \
         (issues #1333, #1336). Routing must arrive through the \
         workflow-classification-reminder hook, which is gated on the recipe-run \
         provenance marker (#1328).",
        findings.join("\n")
    );
}

/// The guard above is only meaningful if it is actually looking at the
/// repository. A wrong `repo_root()` would make it pass vacuously forever.
#[test]
fn the_guard_is_pointed_at_a_real_repository() {
    let root = repo_root();
    assert!(
        root.join("Cargo.toml").is_file(),
        "repo_root() resolved to {} which has no Cargo.toml; the routing guard \
         would pass without inspecting anything",
        root.display()
    );
    assert!(
        root.join("amplifier-bundle").is_dir(),
        "repo_root() resolved to {} which has no amplifier-bundle",
        root.display()
    );
}
