//! Output format: the heading, deduplication, score and size limits.

use super::*;

#[test]
fn relevant_memory_is_injected_with_its_computed_score() {
    let result = format_agent_memory_context(
        "/analyze why cargo test fails in CI",
        &agents(&["analyzer"]),
        &[memory("Always run cargo test before pushing to CI")],
    )
    .expect("relevant memory is injected");
    assert!(result.contains("## Relevant Memory (agents: analyzer)"));
    assert!(result.contains("Always run cargo test before pushing to CI"));
    // `analyze` invokes the agent, `ci` is too short and `always` /
    // `before` are stop words, leaving {cargo, test, fails} vs {run,
    // cargo, test, pushing}: 2 shared / sqrt(3 * 4).
    let expected = 2.0 / 12f64.sqrt();
    assert!(result.contains(&format!("(relevance: {expected:.2})")));
    assert!(!result.contains("relevance: 0.00"));
}

#[test]
fn irrelevant_memory_injects_nothing() {
    let result = format_agent_memory_context(
        "/analyze the auth middleware",
        &agents(&["analyzer"]),
        &[memory("user: reply with just: pong\nassistant: pong")],
    );
    assert_eq!(result, None);
}

#[test]
fn no_memories_injects_nothing() {
    assert_eq!(
        format_agent_memory_context("/analyze auth", &agents(&["analyzer"]), &[]),
        None
    );
}

#[test]
fn each_memory_is_printed_once_whatever_the_agent_count() {
    let result = format_agent_memory_context(
        "review the builder output for cargo fmt failures",
        &agents(&["builder", "reviewer", "tester"]),
        &[
            memory("cargo fmt failures come from the builder output"),
            memory("cargo fmt failures come from the builder output"),
        ],
    )
    .expect("relevant memory is injected");
    assert_eq!(result.matches("## Relevant Memory").count(), 1);
    assert_eq!(
        result
            .matches("cargo fmt failures come from the builder output")
            .count(),
        1
    );
    assert!(result.contains("builder, reviewer, tester"));
}

/// Session-stop stores one `Agent <name>: <summary>` copy per agent.
#[test]
fn per_agent_prefixed_copies_are_printed_once_without_prefix() {
    let body = "cargo fmt failures come from the builder output";
    let memories = ["builder", "reviewer", "tester", "general"]
        .map(|agent| memory(&format!("Agent {agent}: {body}")));
    let result = format_agent_memory_context(
        "why does cargo fmt report failures",
        &agents(&["builder"]),
        &memories,
    )
    .expect("relevant memory is injected");
    assert_eq!(result.matches(body).count(), 1);
    assert!(!result.contains("Agent "));
}

/// Memories that share their first [`MAX_MEMORY_CHARS`] characters but
/// differ later are distinct, and each keeps its own code context.
#[test]
fn memories_differing_past_the_cut_are_both_kept() {
    let shared = format!("cargo fmt failures {}", "the detail ".repeat(40));
    let result = format_agent_memory_context(
        "the cargo fmt failures detail",
        &agents(&["builder"]),
        &[
            PromptContextMemory {
                content: format!("{shared} first ending"),
                code_context: Some("ctx-one".to_string()),
            },
            PromptContextMemory {
                content: format!("{shared} second ending"),
                code_context: Some("ctx-two".to_string()),
            },
        ],
    )
    .expect("relevant memories are injected");
    assert_eq!(
        result.lines().filter(|line| line.starts_with("- ")).count(),
        2
    );
    assert!(result.contains("ctx-one"));
    assert!(result.contains("ctx-two"));
}

/// Copies that differ only in whitespace print the same line, so they
/// are one memory.
#[test]
fn whitespace_variant_copies_are_printed_once() {
    let result = format_agent_memory_context(
        "why does cargo fmt report failures in the builder",
        &agents(&["analyzer"]),
        &[
            memory("Agent a: cargo fmt failures\nfrom the builder"),
            memory("Agent b: cargo fmt failures from the builder"),
            memory("cargo  fmt failures   from the builder "),
        ],
    )
    .expect("relevant memory is injected");
    assert_eq!(
        result
            .matches("- cargo fmt failures from the builder")
            .count(),
        1
    );
}

#[test]
fn empty_agent_list_has_a_plain_heading() {
    let result = format_agent_memory_context("cargo fmt", &[], &[memory("the cargo fmt")]).unwrap();
    assert!(result.contains("## Relevant Memory\n"));
    assert!(!result.contains("agents:"));
}

#[test]
fn agent_prefix_is_stripped_only_when_it_is_one() {
    assert_eq!(strip_agent_prefix("Agent builder: body"), "body");
    assert_eq!(
        strip_agent_prefix("Agent smith said: hello"),
        "Agent smith said: hello"
    );
    assert_eq!(strip_agent_prefix("plain memory"), "plain memory");
}

#[test]
fn injected_memory_is_bounded() {
    let long = format!("cargo test ci {}", "the detail ".repeat(250));
    let result = format_agent_memory_context(
        "the cargo test ci detail",
        &agents(&["tester"]),
        &[memory(&long)],
    )
    .expect("relevant memory is injected");
    let entry = result
        .lines()
        .find(|line| line.starts_with("- "))
        .expect("memory entry");
    assert!(entry.contains('…'));
    assert!(entry.chars().count() < MAX_MEMORY_CHARS + 40);
}

#[test]
fn at_most_max_memories_are_injected_most_relevant_first() {
    let mut memories = (0..MAX_INJECTED_MEMORIES + 3)
        .map(|index| memory(&format!("the cargo fmt note{index} extra{index}")))
        .collect::<Vec<_>>();
    memories.push(memory("the cargo fmt"));
    let result =
        format_agent_memory_context("cargo fmt", &agents(&["builder"]), &memories).unwrap();
    let entries = result
        .lines()
        .filter(|line| line.starts_with("- "))
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), MAX_INJECTED_MEMORIES);
    assert!(entries[0].starts_with("- the cargo fmt (relevance: 1.00)"));
}

#[test]
fn format_with_code_context() {
    let result = format_agent_memory_context(
        "important fact about main",
        &agents(&["builder"]),
        &[PromptContextMemory {
            content: "Important fact about main".to_string(),
            code_context: Some("fn main() {}".to_string()),
        }],
    )
    .unwrap();
    assert!(result.contains("fn main() {}"));
}

#[test]
fn format_empty_code_context_skipped() {
    let result = format_agent_memory_context(
        "cargo fact",
        &agents(&["builder"]),
        &[PromptContextMemory {
            content: "The cargo fact".to_string(),
            code_context: Some("  ".to_string()),
        }],
    )
    .unwrap();
    assert!(!result.contains("  \n"));
    assert!(!result.ends_with("  "));
}
