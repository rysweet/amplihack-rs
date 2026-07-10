# `pr-guide` Skill Missing from Copilot CLI - Troubleshooting

> [Home](../index.md) > [Troubleshooting](README.md) > pr-guide skill missing

## Problem

The `pr-guide` skill was previously available in the Copilot CLI skills list but
no longer appears. It is absent from the skills listing and cannot be invoked,
even though other skills work as expected.

This is tracked as issue #860. The same failure mode can affect **any** bundled
skill, not just `pr-guide`.

## Cause

Copilot CLI skill staging is **filesystem-driven**. During install/update,
`stage_skills` (in `crates/amplihack-cli/src/copilot_setup/staging.rs`) simply
iterates the directories under `amplifier-bundle/skills/`:

```rust
for entry in fs::read_dir(source_skills)? {
    // ... copies each skill directory into ~/.copilot/skills/
}
```

A skill is listed in Copilot CLI **if and only if** its bundle directory exists
on disk at stage time. There is no separate manifest — the directory *is* the
source of truth for staging.

The skill also has a second source of truth: the compile-time registry
`AMPLIHACK_SKILLS` in `crates/amplihack-hooks/src/known_skills.rs`, which hook
and classification code use to recognise skill names.

The root cause of #860 was a **stale-tree checkout** that predated the commit
adding `pr-guide`. On that tree, `pr-guide` was absent from **both** sources of
truth at once:

1. `amplifier-bundle/skills/pr-guide/` did not exist → staging never copied it
   into `~/.copilot/skills/`, so it vanished from the Copilot CLI listing.
2. `"pr-guide"` was missing from `AMPLIHACK_SKILLS` → hooks did not recognise it.

Because both sides were removed together, a naive set-equality check between the
two would still have passed (empty ⊆ empty), which is why the drop went silent.

## Solution

Restore `pr-guide` in **both** sources of truth (already present on `main`):

1. Bundle directory `amplifier-bundle/skills/pr-guide/` with a valid
   `SKILL.md` whose frontmatter is `name: pr-guide`.
2. The `"pr-guide"` entry in `AMPLIHACK_SKILLS`
   (`crates/amplihack-hooks/src/known_skills.rs`), kept in sorted order (the
   registry is queried with `binary_search`), with `skill_count()` matching the
   on-disk bundle count.

If you are on a stale branch that is missing the skill, do a **surgical**
restore of only these paths rather than merging or rebasing an old `main`:

```bash
git checkout origin/main -- amplifier-bundle/skills/pr-guide
git checkout origin/main -- crates/amplihack-hooks/src/known_skills.rs
```

Then re-run `amplihack install` (or `amplihack update`) to re-stage skills.

## Regression Guards

Two layers prevent a skill from silently disappearing again:

**1. Registry ↔ bundle set-equality** — the existing
`registry_matches_bundled_skill_frontmatter_names` unit test in
`crates/amplihack-hooks/src/known_skills.rs` asserts the set of bundled
`SKILL.md` frontmatter names equals the `AMPLIHACK_SKILLS` registry, catching
any *one-sided* drift (a bundled skill missing from the registry, or a registry
entry with no matching bundled `SKILL.md`).

**2. `pr-guide` two-sided pin** — a guard test added in
`tests/integration/skill_frontmatter_name_test.rs` (test binary
`skill_frontmatter_name`):

| Test | Guards against |
| --- | --- |
| `tc_skill_13_pr_guide_pinned_in_registry_and_bundle` | Wholesale two-sided removal of `pr-guide` (the exact #860 failure) — a skill dropped from *both* the bundle and the registry at once. |

The set-equality check in layer 1 stays green under a two-sided removal (both
sets lose the same element), so TC-SKILL-13 pins `pr-guide` concretely on each
side to turn the suite red for that case.

## Verifying the Fix

Confirm the skill is present in both sources of truth and staged for Copilot:

```bash
# 1. Bundle directory exists with valid frontmatter
cat amplifier-bundle/skills/pr-guide/SKILL.md | head -5   # expect: name: pr-guide

# 2. Registry entry present
grep '"pr-guide"' crates/amplihack-hooks/src/known_skills.rs

# 3. Regression guards pass
cargo test -p amplihack --test skill_frontmatter_name

# 4. After install/update, the skill is staged for Copilot CLI
ls ~/.copilot/skills/pr-guide/SKILL.md
```

All four checks should succeed. If step 4 is missing the file, re-run
`amplihack install` and confirm the bundle directory from step 1 exists at
stage time.
