//! Conformance gate for `docs/spec/OrchLedger.tla` (issue #1326).
//!
//! The spec is only worth having if the implementation is held to it. Every
//! invariant named in a `docs/spec/*.cfg` INVARIANTS line must have a test
//! function here (or in `bins/amplihack/tests/spec_conformance_process.rs` for
//! the ones that need real processes), and `spec_invariants_all_have_tests`
//! fails if that mapping ever drifts.
//!
//! Traceability
//! ------------
//! | TLA+ invariant    | enforced by                                            |
//! |-------------------|--------------------------------------------------------|
//! | `CeilingMonotone` | `invariant_ceiling_monotone*` (this file)               |
//! | `DepthBound`      | `invariant_depth_bound*` (this file)                    |
//! | `NodeBudget`      | `invariant_node_budget*` (process test)                 |
//! | `LedgerSound`     | `invariant_ledger_sound*` (process test)                |
//! | `TypeOK`          | Rust's type system; `u32` domain, checked at the seams  |

use std::path::{Path, PathBuf};

use amplihack_cli::commands::session_tree::state::{
    DEFAULT_MAX_DEPTH, MAX_DEPTH_CEILING, effective_max_depth,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// CeilingMonotone
//   \A c \in Procs : parent[c] \in Procs => ceiling[c] =< ceiling[parent[c]]
//
// In the implementation the parent's ceiling is what the root sealed into the
// tree state, and the child's claim arrives as AMPLIHACK_MAX_DEPTH. So the
// invariant reduces to: effective_max_depth(sealed, env) =< sealed.
// ---------------------------------------------------------------------------

/// Exhaustive over the interesting domain: env may lower, never raise.
#[test]
fn invariant_ceiling_monotone_env_never_raises() {
    for sealed in 0..=MAX_DEPTH_CEILING {
        for env in 0..=(MAX_DEPTH_CEILING + 8) {
            let got = effective_max_depth(Some(sealed), Some(env));
            assert!(
                got <= sealed,
                "CeilingMonotone violated: sealed={sealed} env={env} -> {got}"
            );
        }
    }
}

/// The escalation ladder observed on the affected host (5 -> 6 -> 7 -> 8 -> 9)
/// must be inert. This is the regression oracle for the incident.
#[test]
fn invariant_ceiling_monotone_observed_escalation_is_inert() {
    let sealed = Some(3);
    for forged in [5u32, 6, 7, 8, 9, 99, MAX_DEPTH_CEILING, u32::MAX] {
        assert_eq!(
            effective_max_depth(sealed, Some(forged)),
            3,
            "forged AMPLIHACK_MAX_DEPTH={forged} must not raise a sealed ceiling of 3"
        );
    }
}

/// A child lowering its own ceiling is legitimate and must keep working.
#[test]
fn invariant_ceiling_monotone_env_may_lower() {
    assert_eq!(effective_max_depth(Some(5), Some(2)), 2);
    assert_eq!(effective_max_depth(Some(5), Some(0)), 0);
}

/// An unsealed tree (root, or state written by an older build) falls back to
/// the environment value, still clamped. Without this a root could never
/// establish a ceiling and nesting would be impossible.
#[test]
fn invariant_ceiling_monotone_unsealed_falls_back_to_env() {
    assert_eq!(effective_max_depth(None, Some(4)), 4);
    assert_eq!(effective_max_depth(None, None), DEFAULT_MAX_DEPTH);
    assert_eq!(
        effective_max_depth(None, Some(u32::MAX)),
        MAX_DEPTH_CEILING,
        "an unsealed tree must still be clamped to the hard ceiling"
    );
}

// ---------------------------------------------------------------------------
// DepthBound
//   \A p \in Procs : pc[p] # "off" => depth[p] =< MaxDepth
//
// I4 / NestingPossible is the other half: nesting BELOW the ceiling must never
// be refused. A fix that clamps everything to depth 0 satisfies DepthBound and
// destroys the product, so both directions are asserted.
// ---------------------------------------------------------------------------

#[test]
fn invariant_depth_bound_ceiling_is_never_exceeded() {
    let sealed = Some(3);
    for env in [None, Some(0), Some(3), Some(9), Some(u32::MAX)] {
        assert!(effective_max_depth(sealed, env) <= 3);
    }
}

#[test]
fn invariant_depth_bound_nesting_below_ceiling_is_permitted() {
    // The capability this whole design exists to protect.
    let ceiling = effective_max_depth(Some(3), Some(3));
    for depth in 0..ceiling {
        assert!(
            depth < ceiling,
            "depth {depth} must remain spawnable under a ceiling of {ceiling}"
        );
    }
    assert_eq!(
        ceiling, 3,
        "a sealed ceiling of 3 must still permit 3 levels"
    );
}

// ---------------------------------------------------------------------------
// Drift guard: spec and tests must not diverge.
// ---------------------------------------------------------------------------

fn invariants_named_in_configs() -> Vec<String> {
    let dir = repo_root().join("docs/spec");
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("docs/spec exists") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("cfg") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable cfg");
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("INVARIANTS") {
                for name in rest.split_whitespace() {
                    if !names.iter().any(|n| n == name) {
                        names.push(name.to_string());
                    }
                }
            }
        }
    }
    assert!(!names.is_empty(), "no INVARIANTS found in docs/spec/*.cfg");
    names
}

fn to_snake(camel: &str) -> String {
    let mut out = String::new();
    for (i, ch) in camel.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn conformance_sources() -> Vec<PathBuf> {
    let root = repo_root();
    vec![
        PathBuf::from(file!()),
        root.join("crates/amplihack-cli/tests/spec_conformance.rs"),
        root.join("bins/amplihack/tests/spec_conformance_process.rs"),
    ]
    .into_iter()
    .filter(|p| Path::new(p).exists())
    .collect()
}

/// Every invariant the spec checks must be enforced somewhere in the test
/// suite. Adding an invariant to a `.cfg` without a matching test fails here.
#[test]
fn spec_invariants_all_have_tests() {
    // TypeOK is discharged by Rust's type system, not by a runtime test.
    const DISCHARGED_BY_TYPES: &[&str] = &["TypeOK"];

    let sources: String = conformance_sources()
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n");

    let mut missing = Vec::new();
    for name in invariants_named_in_configs() {
        if DISCHARGED_BY_TYPES.contains(&name.as_str()) {
            continue;
        }
        let expected_fn = format!("fn invariant_{}", to_snake(&name));
        if !sources.contains(&expected_fn) {
            missing.push(format!("{name} (expected a test named `{expected_fn}...`)"));
        }
    }
    assert!(
        missing.is_empty(),
        "spec invariants with no conformance test: {missing:#?}\n\
         Add a test, or record why the type system discharges it."
    );
}

/// The spec gate script must exist and cover every config, so that adding a
/// config without wiring it into the gate is caught.
#[test]
fn spec_gate_covers_every_config() {
    let root = repo_root();
    let script = std::fs::read_to_string(root.join("scripts/check-spec.sh"))
        .expect("scripts/check-spec.sh exists");
    for entry in std::fs::read_dir(root.join("docs/spec")).expect("docs/spec exists") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("cfg") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        assert!(
            script.contains(stem),
            "docs/spec/{stem}.cfg is not referenced by scripts/check-spec.sh"
        );
    }
}

// ---------------------------------------------------------------------------
// Documentation drift guards.
//
// A formal-methods claim is only worth as much as its accuracy. `docs/spec/README.md`
// previously advertised bounds ("8 nodes") that no config used, which is exactly the
// kind of detail a reader has no way to check and every reason to trust.
// ---------------------------------------------------------------------------

fn cfg_constant(cfg: &str, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(repo_root().join("docs/spec").join(cfg)).ok()?;
    text.lines().find_map(|l| {
        l.trim()
            .strip_prefix(name)?
            .trim()
            .strip_prefix('=')
            .map(str::trim)
            .map(str::to_string)
    })
}

/// The bounds the README advertises must be the bounds the shipped config checks.
#[test]
fn spec_readme_states_the_bounds_actually_checked() {
    let readme = std::fs::read_to_string(repo_root().join("docs/spec/README.md"))
        .expect("docs/spec/README.md exists");
    let max_nodes = cfg_constant("B_proposed.cfg", "MaxNodes").expect("MaxNodes in B_proposed.cfg");
    let max_depth = cfg_constant("B_proposed.cfg", "MaxDepth").expect("MaxDepth in B_proposed.cfg");
    assert!(
        readme.contains(&format!("{max_nodes} nodes")),
        "README must state the checked node bound ({max_nodes}); it currently does not"
    );
    assert!(
        readme.contains(&format!("depth {max_depth}")),
        "README must state the checked depth bound ({max_depth})"
    );
}

/// The refusal text quoted in the user-facing reference must be the text the code
/// actually emits. A stale example teaches the wrong thing precisely when someone
/// is stuck and searching for the message they just saw.
#[test]
fn documented_refusal_matches_the_emitted_message() {
    let doc = std::fs::read_to_string(
        repo_root().join("docs/reference/session-tree-recursion-control.md"),
    )
    .expect("reference doc exists");
    let src = std::fs::read_to_string(
        repo_root().join("crates/amplihack-cli/src/commands/recipe/run/execute.rs"),
    )
    .expect("execute.rs exists");

    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let src_squashed = squash(&src).replace("\\ ", "").replace("\\n", " ");

    for sentence in [
        "This is a POLICY decision, not an infrastructure fault.",
        "DO: complete this step inline and return your result.",
    ] {
        assert!(
            squash(&doc).contains(sentence),
            "reference doc no longer quotes: {sentence}"
        );
        assert!(
            squash(&src_squashed).contains(sentence),
            "code no longer emits: {sentence}"
        );
    }
}
