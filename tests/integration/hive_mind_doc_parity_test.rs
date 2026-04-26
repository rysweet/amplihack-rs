//! TDD tests for issue #420 Doc Parity Batch E: hive_mind documentation port.
//!
//! Verifies that the four ported hive-mind docs:
//!   - exist at the kebab-case Diataxis paths,
//!   - are linked from `docs/index.md`,
//!   - reference only real `amplihack-hive` crate symbols (no fabricated APIs),
//!   - use `amplihack hive ...` CLI invocations (not `python -m amplihack...`),
//!   - reference `lbug` instead of `kuzu` for the graph backend.
//!
//! These tests pin the contract for the port and will catch regressions if
//! upstream re-syncs introduce drift between docs and the live Rust API.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path
}

fn read_doc(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

const PORTED_DOCS: &[&str] = &[
    "docs/tutorials/hive-mind-getting-started.md",
    "docs/tutorials/hive-mind-tutorial.md",
    "docs/concepts/hive-mind-design.md",
    "docs/concepts/hive-mind-eval.md",
];

// ═══════════════════════════════════════════════════════════════════════════
// Existence: each ported doc lives at the documented kebab-case path.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn all_four_ported_docs_exist() {
    for rel in PORTED_DOCS {
        let path = workspace_root().join(rel);
        assert!(
            path.is_file(),
            "Expected ported hive-mind doc at {}; missing.",
            path.display()
        );
    }
}

#[test]
fn ported_docs_are_non_trivial() {
    // Each ported doc should be substantive — not a placeholder stub.
    for rel in PORTED_DOCS {
        let body = read_doc(rel);
        assert!(
            body.lines().count() >= 50,
            "{rel} has fewer than 50 lines; looks like a stub, not a real port."
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Index linkage: docs/index.md must surface each ported doc in the matching
// Diataxis section (Tutorials for tutorials/, Concepts for concepts/).
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn index_links_all_ported_docs() {
    let index = read_doc("docs/index.md");
    for rel in PORTED_DOCS {
        // index.md lives at docs/index.md and links via ./tutorials/... or ./concepts/...
        let link_target = rel.trim_start_matches("docs/");
        assert!(
            index.contains(link_target),
            "docs/index.md does not link to {link_target}; Diataxis navigation is broken."
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Adaptation rule: NO `python -m amplihack...` invocations should remain.
// The Rust port replaces these with `amplihack hive ...` CLI calls or with
// crate API usage examples.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn no_python_amplihack_invocations_remain() {
    for rel in PORTED_DOCS {
        let body = read_doc(rel);
        for (lineno, line) in body.lines().enumerate() {
            // Allow markdown blockquote lines (prefix `>`) — those are explicit
            // attributions to upstream behavior, not instructions to the user.
            if line.trim_start().starts_with('>') {
                continue;
            }
            let lower = line.to_lowercase();
            assert!(
                !lower.contains("python -m amplihack"),
                "{rel}:{}: forbidden `python -m amplihack` invocation found:\n  {line}",
                lineno + 1
            );
            assert!(
                !lower.contains("uv run python"),
                "{rel}:{}: forbidden `uv run python` invocation found:\n  {line}",
                lineno + 1
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Adaptation rule: graph backend is `lbug`, not `kuzu`.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn no_kuzu_references_in_ported_docs() {
    for rel in PORTED_DOCS {
        let body = read_doc(rel);
        for (lineno, line) in body.lines().enumerate() {
            let lower = line.to_lowercase();
            assert!(
                !lower.contains("kuzu"),
                "{rel}:{}: ported docs must reference `lbug`, not `kuzu`:\n  {line}",
                lineno + 1
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Truthfulness: every Rust symbol named in the docs must be a real public
// export from the amplihack-hive crate. Catches fabricated APIs like
// `InMemoryHiveGraph::add_child`, `LearningAgent::builder().hive_store(...)`,
// `unified::UnifiedHiveMind`, `create_event_bus("redis")`, etc.
// ═══════════════════════════════════════════════════════════════════════════

/// Symbols that the docs DO reference and that MUST exist in the live crate.
/// If a doc removes a symbol, drop it from this list. If the crate removes
/// one, the symbol must also be scrubbed from the docs.
const REQUIRED_HIVE_SYMBOLS: &[&str] = &[
    "HiveMindOrchestrator",
    "DefaultPromotionPolicy",
    "PromotionPolicy",
    "HiveEvalConfig",
    "run_eval_with_responder",
    "EvalResponder",
    "FeedConfig",
    "LocalEventBus",
    "HiveCoordinator",
    "AgentNode",
    "BloomFilter",
];

fn lib_rs() -> String {
    read_doc("crates/amplihack-hive/src/lib.rs")
}

fn crate_source() -> String {
    let root = workspace_root().join("crates/amplihack-hive/src");
    let mut combined = String::new();
    fn walk(dir: &std::path::Path, out: &mut String) {
        for entry in fs::read_dir(dir).expect("read crate src dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && let Ok(text) = fs::read_to_string(&path)
            {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }
    walk(&root, &mut combined);
    combined
}

#[test]
fn all_referenced_symbols_exist_in_crate() {
    let lib = lib_rs();
    let src = crate_source();
    for sym in REQUIRED_HIVE_SYMBOLS {
        // Symbol must appear either in lib.rs re-exports OR as a definition
        // somewhere in the crate source (struct/fn/enum).
        assert!(
            lib.contains(sym) || src.contains(sym),
            "Doc references symbol `{sym}` which does NOT exist anywhere in \
             crates/amplihack-hive/src/. Either the doc is fabricated or the \
             API was renamed."
        );
    }
}

/// Symbols that MUST NOT appear in the ported docs because they were
/// fabricated by the original (pre-rewrite) port and have no counterpart
/// in the real `amplihack-hive` crate.
const FORBIDDEN_FABRICATED_SYMBOLS: &[&str] = &[
    "UnifiedHiveMind",
    "HiveMindAgent",
    "HiveMindConfig",
    "create_event_bus",
    "use_hierarchical",
    "hive_store(",
    "InMemoryHiveGraph",
];

#[test]
fn no_fabricated_symbols_in_ported_docs() {
    for rel in PORTED_DOCS {
        let body = read_doc(rel);
        for forbidden in FORBIDDEN_FABRICATED_SYMBOLS {
            assert!(
                !body.contains(forbidden),
                "{rel} references fabricated symbol `{forbidden}` which does \
                 not exist in the amplihack-hive crate."
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CLI surface honesty: the eval CLI flag set documented in
// concepts/hive-mind-eval.md must match the real arg-struct in
// `hive_haymaker.rs`. Fabricated flags like `--turns`, `--question-set`,
// `--mode`, `--agents` must NOT appear as documented `amplihack hive eval`
// flags.
// ═══════════════════════════════════════════════════════════════════════════

const FORBIDDEN_FABRICATED_FLAGS: &[&str] = &["--question-set", "--turns", "--output-dir"];

#[test]
fn no_fabricated_eval_cli_flags() {
    for rel in PORTED_DOCS {
        let body = read_doc(rel);
        for flag in FORBIDDEN_FABRICATED_FLAGS {
            // Allow the flag to appear inside an explicit "Planned" / "Not Yet"
            // / "fictional" / "removed" disclaimer block. We approximate that
            // by requiring the flag never appears on a line that also contains
            // `amplihack hive` (a documented invocation).
            for (lineno, line) in body.lines().enumerate() {
                if line.contains(flag) && line.contains("amplihack hive") {
                    panic!(
                        "{rel}:{}: line documents fabricated flag `{flag}` as a \
                         real `amplihack hive` invocation:\n  {line}",
                        lineno + 1
                    );
                }
            }
        }
    }
}
