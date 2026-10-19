# Bug Fix #860 — `pr-guide` skill missing from the Copilot CLI skills list

> **Issue:** [#860](https://github.com/rysweet/amplihack-rs/issues/860)

---

## Summary

The `pr-guide` skill stopped appearing in the Copilot CLI skills list even
though its definition still existed in the bundled skill corpus. Skill exposure
to Copilot CLI was gated by a hardcoded registry that did not include
`pr-guide`, so the skill was silently dropped from what Copilot could list and
invoke.

Copilot skill discovery is now **filesystem-driven**: a skill is listed if — and
only if — its `SKILL.md` exists under `amplifier-bundle/skills/<name>/` and is
staged into the Copilot home. No allow-list gates which skills are exposed. This
bug fix pins `pr-guide` with dedicated regression coverage so that the concrete
failure modes that make a skill vanish (removed, renamed, or malformed metadata)
fail loudly instead of silently.

## Root cause

`pr-guide` disappeared because a hardcoded skill registry —
`crates/amplihack-hooks/src/known_skills.rs` — enumerated which skills were
exposed to Copilot CLI and did not include `pr-guide`. The skill definition on
disk was correct; the registry was the single point of omission.

That registry was:

1. Patched to add missing entries (#821), then
2. **Removed entirely** (#865: _"the skills directory is the single source of
   truth"_), making Copilot skill discovery purely filesystem-driven.

After #865 the production code path is correct: any skill directory containing a
valid `SKILL.md` is staged and listed. What remained missing was a guard that
pins `pr-guide` specifically, so a future removal, rename, or metadata
regression could once again silently drop it from Copilot's list without any
test failing.

This was **not** a missing-file defect in the current tree. The `pr-guide`
directory, its `SKILL.md`, and supporting markdown are all present and valid.
The fix does **not** restore `known_skills.rs`, which is intentionally gone.

## Fix

Skill discovery for Copilot CLI is filesystem-driven in `stage_skills`:

```text
crates/amplihack-cli/src/copilot_setup/staging.rs
```

`stage_skills(source_skills, copilot_home)` walks every subdirectory of
`amplifier-bundle/skills/`, flattens each skill's markdown tree, and writes it to
`<copilot_home>/skills/<name>/`. There is no registry, allow-list, or manifest
that a skill must additionally appear in. A skill is listed by Copilot CLI when:

| Requirement | Detail |
| --- | --- |
| Directory exists | `amplifier-bundle/skills/<name>/` is a directory |
| `SKILL.md` present | `<name>/SKILL.md` exists at the directory root |
| Frontmatter at byte 0 | The file starts with a `---` YAML frontmatter block |
| `name` matches directory | Frontmatter `name:` equals `<name>` (kebab-case) |
| `description` is a string | Frontmatter `description:` is a scalar string |

`pr-guide` satisfies every requirement, so it is staged and listed like any
other skill.

## `pr-guide` skill

`pr-guide` generates an illustrated, plain-language walkthrough document for a
pull request — problem statement, approach overview, a step-by-step code tour
with mermaid diagrams and deep diff links, key decisions, and a testing summary.
It works with GitHub and Azure DevOps and deliberately skips trivial PRs.

Skill definition:

```text
amplifier-bundle/skills/pr-guide/
├── SKILL.md                    # frontmatter (name: pr-guide) + instructions
├── reference.md                # supporting reference material
└── tests/test_skill_structure.sh
```

`SKILL.md` frontmatter:

```yaml
---
name: pr-guide
description: Generates an illustrated, plain-language walkthrough document for a
  pull request — problem statement, approach overview, step-by-step code tour
  with mermaid diagrams, deep diff links, key decisions, and testing summary.
  Use when explaining, documenting, or summarizing a PR, creating a
  reviewer-friendly illustrated guide, or producing walkthrough notes at the end
  of default-workflow. Works with GitHub and Azure DevOps.
---
```

## Verifying the fix

After installing or updating, confirm the skill is staged into the Copilot home:

```sh
amplihack install
ls ~/.copilot/skills/pr-guide/SKILL.md
```

Then confirm Copilot CLI lists it:

```sh
copilot
> /skills
```

`pr-guide` appears in the skills list. Invoking it produces the illustrated PR
walkthrough document.

## Regression coverage

Two layers of tests pin `pr-guide` so it cannot silently vanish again.

**Staging unit tests** (`crates/amplihack-cli/src/copilot_setup/staging.rs`)
exercise the real `stage_skills` function against a `pr-guide`-shaped source:

| Test | Proves |
| --- | --- |
| `stage_skills_makes_pr_guide_discoverable_for_copilot` | `skills/pr-guide/SKILL.md` lands intact with valid `name: pr-guide` frontmatter and its supporting markdown, with no sibling skill dropped |
| `stage_skills_is_idempotent_for_pr_guide` | Re-staging leaves exactly one discoverable `SKILL.md` (no duplication or loss) |

**Integration invariants**
(`crates/amplihack-cli/tests/issue_860_pr_guide_present.rs`) are read-only and
fail loudly the moment the skill definition regresses:

| Case | Invariant |
| --- | --- |
| TC-860-01 | `amplifier-bundle/skills/pr-guide/` and its `SKILL.md` exist |
| TC-860-02 | Frontmatter block starts at byte 0 of `SKILL.md` |
| TC-860-03 | Frontmatter parses as a YAML mapping whose `name` and `description` are `serde_yaml::Value::String` scalars (guards against list/mapping metadata regressions — see [frontmatter type guard](../testing/SKILL_FRONTMATTER_TYPE_GUARD.md)) |
| TC-860-04 | Frontmatter `name` equals the directory name `pr-guide` |

These tests read only the bundled source files; they never write or stage. Any
removal, rename, or malformed-metadata change to `pr-guide` fails a test rather
than silently dropping the skill from Copilot's list.

Focused checks:

```sh
cargo test -p amplihack-cli --test cli_integration issue_860_pr_guide_present::
cargo test -p amplihack-cli --lib stage_skills
```

Broader checks:

```sh
cargo test -p amplihack-cli
cargo check --workspace
```

## Related

- [Staging API reference](../reference/staging-api.md)
- [Copilot installation implementation](../reference/copilot-installation-implementation.md)
- [Skill catalog](../skills/SKILL_CATALOG.md)
- [Skill frontmatter type guard](../testing/SKILL_FRONTMATTER_TYPE_GUARD.md)
