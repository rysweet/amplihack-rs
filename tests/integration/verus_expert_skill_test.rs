//! Contract tests for the `verus-expert` formal-methods skill (Phase-1 of #4610).
//!
//! Tracks GitHub issue rysweet/Simard#4610. Phase-1 ships a reusable
//! `verus-expert` skill that MIRRORS the existing `tla-plus-expert` 3-file layout,
//! plus a durable Verus-vs-Lean4-vs-TLA+ applicability assessment for Rust. These
//! invariants regress silently (a renamed section, a drifted skill mirror, a
//! reintroduced dated phrase, an unregistered nav page, or a stray non-ASCII byte
//! that trips the CI invisible-char gate would each go unnoticed), so this test
//! pins them:
//!   - all four artifacts (bundle skill, agent, docs skill copy, assessment) exist,
//!   - the bundle and docs SKILL.md copies stay byte-for-byte identical,
//!   - skill + agent frontmatter match the tla-plus-expert convention and the
//!     agent id `amplihack:specialized:verus-expert` resolves to `verus-expert.md`,
//!   - the skill exposes the mandated sections + activation keywords,
//!   - the agent is grounded in the primary-source Verus vocabulary
//!     (verus!{}, spec/proof/exec, requires/ensures/invariant/decreases, ghost/
//!     tracked, int/nat, Z3/SMT, #[verifier::...], panic/overflow, auto-verus,
//!     "verifier is ground truth") and points to the assessment,
//!   - the durable assessment carries its decision framework, Simard targets,
//!     honest limits, phased recommendation, and primary-source citations, and
//!     stays durable (no point-in-time "as of today" framing),
//!   - none of the artifacts reintroduce the forbidden "Bridge" name, kuzu, or a
//!     Python code path, and all four are ASCII-clean,
//!   - both docs pages are registered in the mkdocs nav, and the agent is present
//!     in the known-agents registry so the skill loads.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const BUNDLE_SKILL: &str = "amplifier-bundle/skills/verus-expert/SKILL.md";
const DOCS_SKILL: &str = "docs/claude/skills/verus-expert/SKILL.md";
const AGENT: &str = "amplifier-bundle/agents/specialized/verus-expert.md";
const ASSESSMENT: &str = "docs/formal-methods/verus-vs-lean-vs-tla-for-rust.md";
const MKDOCS: &str = "mkdocs.yml";
const KNOWN_AGENTS: &str = "crates/amplihack-hooks/src/known_agents.rs";

const ARTIFACTS: &[&str] = &[BUNDLE_SKILL, DOCS_SKILL, AGENT, ASSESSMENT];

/// Extract the leading YAML frontmatter block delimited by the first two `---`
/// lines. Panics if the file has no frontmatter, which is itself a contract.
fn frontmatter(relative: &str) -> String {
    let body = read(relative);
    let mut lines = body.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "{relative} must open with a `---` YAML frontmatter fence"
    );
    let mut fm = String::new();
    for line in lines {
        if line.trim_end() == "---" {
            return fm;
        }
        fm.push_str(line);
        fm.push('\n');
    }
    panic!("{relative} frontmatter is not terminated by a closing `---`");
}

// ============================================================================
// Existence of the four Phase-1 artifacts
// ============================================================================

#[test]
fn all_four_artifacts_exist() {
    for f in ARTIFACTS {
        let path = workspace_root().join(f);
        assert!(
            path.exists(),
            "{f} must exist (Phase-1 of #4610 ships this artifact)"
        );
    }
}

// ============================================================================
// Skill mirror parity (bundle copy == docs copy, byte-for-byte)
// ============================================================================

#[test]
fn skill_mirror_is_byte_for_byte_identical() {
    let bundle = read(BUNDLE_SKILL);
    let docs = read(DOCS_SKILL);
    assert_eq!(
        bundle, docs,
        "{BUNDLE_SKILL} and {DOCS_SKILL} must be byte-for-byte identical; the docs \
         copy mirrors the bundle skill (Phase-1 of #4610)."
    );
}

// ============================================================================
// Skill frontmatter contract (mirrors tla-plus-expert convention)
// ============================================================================

#[test]
fn skill_frontmatter_matches_convention() {
    let fm = frontmatter(BUNDLE_SKILL);
    for needle in [
        "name: verus-expert",
        "version: 1.0.0",
        "description:",
        "activation_keywords:",
        "agent: amplihack:specialized:verus-expert",
    ] {
        assert!(
            fm.contains(needle),
            "{BUNDLE_SKILL} frontmatter must contain `{needle}` to mirror the \
             tla-plus-expert skill convention (#4610)."
        );
    }
}

/// The `agent:` id must resolve to the on-disk agent filename and the agent's own
/// declared `name`, or the skill will fail to load its expert.
#[test]
fn agent_id_resolves_to_agent_file_and_name() {
    let skill_fm = frontmatter(BUNDLE_SKILL);
    assert!(
        skill_fm.contains("agent: amplihack:specialized:verus-expert"),
        "{BUNDLE_SKILL} must reference agent id `amplihack:specialized:verus-expert`."
    );
    // Last segment of the id -> agents/specialized/<segment>.md
    assert!(
        workspace_root().join(AGENT).exists(),
        "the `amplihack:specialized:verus-expert` id must resolve to {AGENT}."
    );
    let agent_fm = frontmatter(AGENT);
    assert!(
        agent_fm.contains("name: verus-expert"),
        "{AGENT} frontmatter `name` must be `verus-expert` to match the skill's \
         agent id last segment (#4610)."
    );
}

#[test]
fn agent_frontmatter_matches_convention() {
    let fm = frontmatter(AGENT);
    for needle in [
        "name: verus-expert",
        "version: 1.0.0",
        "description:",
        "role:",
        "priority:",
        "model: inherit",
    ] {
        assert!(
            fm.contains(needle),
            "{AGENT} frontmatter must contain `{needle}` to mirror the \
             tla-plus-expert agent convention (#4610)."
        );
    }
}

// ============================================================================
// Skill sections + activation keywords
// ============================================================================

#[test]
fn skill_has_mandated_sections() {
    let body = read(BUNDLE_SKILL);
    for section in [
        "## Purpose",
        "## When This Skill Activates",
        "## How It Works",
        "## Integration",
        "## Usage Examples",
        "## Key Resources",
    ] {
        assert!(
            body.contains(section),
            "{BUNDLE_SKILL} must contain the `{section}` section (mirrors \
             tla-plus-expert; #4610)."
        );
    }
}

#[test]
fn skill_exposes_mandated_activation_keywords() {
    let body = read(BUNDLE_SKILL);
    for kw in [
        "Verus",
        "verus!",
        "formal verification",
        "deductive verification",
        "auto-verus",
        "loop invariant",
        "SMT",
        "Z3",
        "panic-free",
        "overflow-free",
        "ghost code",
        "spec function",
    ] {
        assert!(
            body.contains(kw),
            "{BUNDLE_SKILL} must list the `{kw}` activation keyword (#4610)."
        );
    }
}

// ============================================================================
// Agent competency grounding (primary-source Verus vocabulary)
// ============================================================================

#[test]
fn agent_is_grounded_in_verus_primitives() {
    let body = read(AGENT);
    // Core macro + modes + contract vocabulary.
    for needle in [
        "verus! {",
        "spec",
        "proof",
        "exec",
        "requires",
        "ensures",
        "invariant",
        "decreases",
        "ghost",
        "tracked",
        "int",
        "nat",
        "Z3",
        "SMT",
        "#[verifier::",
        "overflow",
        "panic",
    ] {
        assert!(
            body.contains(needle),
            "{AGENT} must be grounded in the Verus primitive `{needle}` \
             (evidence rule; #4610)."
        );
    }
}

#[test]
fn agent_covers_auto_verus_behind_verifier() {
    let body = read(AGENT);
    assert!(
        body.contains("auto-verus") || body.contains("AutoVerus"),
        "{AGENT} must cover auto-verus / AutoVerus LLM proof synthesis (#4610)."
    );
    assert!(
        body.contains("verifier is ground truth"),
        "{AGENT} must state the `verifier is ground truth` LLM guardrail (#4610)."
    );
}

#[test]
fn agent_points_to_assessment_for_tool_choice() {
    let body = read(AGENT);
    assert!(
        body.contains("formal-methods/verus-vs-lean-vs-tla-for-rust.md"),
        "{AGENT} must include a Verus-vs-Lean-vs-TLA+ decision note pointing to \
         the assessment doc (#4610)."
    );
}

// ============================================================================
// Durable assessment content contract
// ============================================================================

#[test]
fn assessment_has_decision_framework() {
    let body = read(ASSESSMENT);
    for needle in [
        "# Verus vs Lean 4 vs TLA+ for Rust",
        "complementary, not competitors",
        "## Use X when",
    ] {
        assert!(
            body.contains(needle),
            "{ASSESSMENT} must contain `{needle}` (decision framework; #4610)."
        );
    }
}

#[test]
fn assessment_covers_all_three_tools_cost_models() {
    let body = read(ASSESSMENT);
    for tool in ["Verus", "Lean 4", "TLA+"] {
        assert!(
            body.contains(tool),
            "{ASSESSMENT} must cover `{tool}` (three-way comparison; #4610)."
        );
    }
    assert!(
        body.contains("cost model") || body.contains("Cost model"),
        "{ASSESSMENT} must state each tool's cost model (#4610)."
    );
}

#[test]
fn assessment_names_concrete_simard_targets() {
    let body = read(ASSESSMENT);
    for target in [
        "cost-ledger",
        "OODA",
        "lbug",
        "epoch-fencing",
        "self-deploy gate",
    ] {
        assert!(
            body.contains(target),
            "{ASSESSMENT} must name the Verus-amenable Simard target `{target}` \
             (#4610 context)."
        );
    }
}

#[test]
fn assessment_states_honest_limits() {
    let body = read(ASSESSMENT);
    assert!(
        body.contains("cannot practically cover"),
        "{ASSESSMENT} must state what Verus cannot practically cover (#4610)."
    );
    for limit in ["async", "FFI", "legacy"] {
        assert!(
            body.contains(limit),
            "{ASSESSMENT} must name the honest limit `{limit}` (#4610)."
        );
    }
}

#[test]
fn assessment_gives_phased_recommendation_with_lean_as_research_track() {
    let body = read(ASSESSMENT);
    assert!(
        body.contains("Phase-2"),
        "{ASSESSMENT} must give a phased Phase-2 recommendation (#4610)."
    );
    assert!(
        body.contains("research track"),
        "{ASSESSMENT} must keep Lean 4 as a research track (#4610)."
    );
    assert!(
        body.contains("#4610"),
        "{ASSESSMENT} must reference issue #4610."
    );
}

/// Durable reference guidance must not be framed as a dated point-in-time report.
#[test]
fn assessment_stays_durable_no_dated_framing() {
    let body = read(ASSESSMENT).to_lowercase();
    for banned in [
        "as of today",
        "as of this writing",
        "at the time of writing",
    ] {
        assert!(
            !body.contains(banned),
            "{ASSESSMENT} must be durable reference guidance; remove dated framing \
             `{banned}` (#4610)."
        );
    }
}

// ============================================================================
// Primary-source citations (evidence rule)
// ============================================================================

#[test]
fn agent_and_assessment_cite_primary_sources() {
    let agent = read(AGENT);
    let assessment = read(ASSESSMENT);
    let sources = [
        "github.com/verus-lang/verus",
        "verus-lang.github.io/verus/guide",
        "github.com/microsoft/verus-proof-synthesis",
        "arxiv.org/abs/2409.13082", // AutoVerus
        "arxiv.org/abs/2512.18436", // VeruSAGE
        "arxiv.org/abs/2605.30106", // Rust -> Lean
        "arxiv.org/abs/2509.23130", // SysMoBench
        "github.com/leanprover/lean4",
    ];
    for src in sources {
        assert!(
            agent.contains(src) || assessment.contains(src),
            "primary source `{src}` must be cited in the agent or assessment \
             (hard evidence rule; #4610)."
        );
    }
}

// ============================================================================
// Constraint guards: no "Bridge", no kuzu, no Python code path
// ============================================================================

#[test]
fn artifacts_do_not_use_forbidden_names() {
    for f in ARTIFACTS {
        let body = read(f);
        assert!(
            !body.contains("Bridge"),
            "{f} must never name anything \"Bridge\" (hard constraint; #4610)."
        );
        assert!(
            !body.to_lowercase().contains("kuzu"),
            "{f} must not introduce kuzu (hard constraint; #4610)."
        );
        assert!(
            !body.contains("```python"),
            "{f} must not introduce a Python code path (hard constraint; #4610)."
        );
    }
}

// ============================================================================
// ASCII cleanliness (mirrors the CI invisible-char scan gate)
// ============================================================================

#[test]
fn artifacts_are_ascii_clean() {
    for f in ARTIFACTS {
        let body = read(f);
        for (idx, ch) in body.char_indices() {
            let c = ch as u32;
            let allowed = matches!(c, 0x09 | 0x0A | 0x0D) || (0x20..=0x7E).contains(&c);
            assert!(
                allowed,
                "{f} contains a non-ASCII/invisible char U+{c:04X} at byte {idx}; \
                 the CI invisible-char scan would reject it (#4610)."
            );
        }
    }
}

// ============================================================================
// Integration: mkdocs nav registration + known-agents registry (skill loads)
// ============================================================================

#[test]
fn mkdocs_registers_both_docs_pages() {
    let nav = read(MKDOCS);
    assert!(
        nav.contains("claude/skills/verus-expert/SKILL.md"),
        "{MKDOCS} must register the Verus Expert skill page under nav so \
         `mkdocs build --strict` passes (#4610)."
    );
    assert!(
        nav.contains("formal-methods/verus-vs-lean-vs-tla-for-rust.md"),
        "{MKDOCS} must register the formal-methods assessment page under nav so \
         `mkdocs build --strict` passes (#4610)."
    );
}

#[test]
fn known_agents_registry_lists_verus_expert() {
    let registry = read(KNOWN_AGENTS);
    assert!(
        registry.contains("\"verus-expert\""),
        "{KNOWN_AGENTS} must list `verus-expert` so the agent id resolves and the \
         skill loads (parity with tla-plus-expert; #4610)."
    );
}
