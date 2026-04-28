# Investigation: Disposition of Upstream Gherkin v2 Experiment Findings (#434)

**Date**: 2026-04-28
**Scope**: Investigation only — no code changes
**Disposition**: Closed as not-planned

---

## Summary

Issue #434 asked whether to port the upstream `gherkin_v2_experiment_findings.md`
document into amplihack-rs, or remove the `"gherkin-expert"` string from
`known_skills.rs` if the feature is unused.

**Conclusion: CLOSE AS NOT-PLANNED — no parity gap exists.**

The gherkin-expert capability already ships in amplihack-rs through the
amplifier-bundle. The upstream document is an informational experiment log, not a
feature specification. There is nothing to port and nothing to remove.

---

## 1. What the Issue Claimed

Issue #434 (tracked from the #420 documentation parity audit) reported:

- Upstream carries `docs/gherkin_v2_experiment_findings.md`
- The only amplihack-rs hit for "gherkin" is a string in `known_skills.rs`
- No agent, recipe, or workflow uses gherkin v2 today

The issue suggested either porting the experiment as an eval scenario or
removing the string from `known_skills.rs`.

## 2. What the Codebase Actually Contains

A broader search reveals the gherkin-expert is a live, functional skill:

| File | Purpose |
|---|---|
| `amplifier-bundle/skills/gherkin-expert/SKILL.md` | Full skill definition with triggers, usage, and examples |
| `amplifier-bundle/agents/specialized/gherkin-expert.md` | Agent definition for BDD/Gherkin test generation |
| `crates/amplihack-hooks/src/known_skills.rs` | Valid allowlist entry referencing the bundle skill |
| `docs/howto/use_gherkin_expert.md` | User-facing how-to guide |
| `docs/guides/formal-specifications-as-prompt-language.md` | Related guide referencing gherkin patterns |

The initial grep (`crates/*/src/`) only searched Rust source files, missing the
bundle and documentation directories where the skill lives.

## 3. Why No Action Is Needed

### The upstream document is informational, not a feature spec

`gherkin_v2_experiment_findings.md` records the results of a past experiment. It
describes what was tried and what was learned. It is not a specification for a
feature that needs implementing.

### The gherkin-expert capability already exists

The amplifier-bundle ships a complete gherkin-expert skill (SKILL.md) and agent
(gherkin-expert.md). The `known_skills.rs` entry correctly references this
bundle skill. Removing it would break skill discovery.

### No parity gap

The parity audit flagged this because the grep was too narrow. The capability
exists — it is bundled, documented, and registered.

## 4. Decision

| Aspect | Decision |
|---|---|
| Port upstream document? | No — it is an experiment log, not a feature spec |
| Remove `known_skills.rs` entry? | No — it references a valid, live skill |
| Create eval scenario? | No — skill already functions; eval is orthogonal |
| Issue disposition | Closed as not-planned |

## 5. Risks Considered

- **False positive from narrow grep**: The original issue was filed based on a
  grep limited to `crates/*/src/`. A repo-wide search immediately shows the
  skill exists. Future parity audits should search the full repository including
  `amplifier-bundle/`.

- **Removing a valid entry**: Had the `known_skills.rs` entry been removed, the
  gherkin-expert skill would no longer be discoverable by the hooks system. This
  would be a regression, not cleanup.
