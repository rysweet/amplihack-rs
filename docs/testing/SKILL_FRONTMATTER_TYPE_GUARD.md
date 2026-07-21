# Skill Frontmatter Type Guard

A regression test that **always** catches Copilot CLI skill-frontmatter TYPE
bugs before they ship. It scans every bundled `SKILL.md` and fails loudly if a
frontmatter field that Copilot CLI requires to be a **string scalar** is
encoded as a YAML list (sequence) or map (mapping).

- **Test file**: `tests/integration/skill_frontmatter_type_test.rs`
- **Cargo target**: `skill_frontmatter_type` (registered in `bins/amplihack/Cargo.toml`)
- **Issue**: [#890](https://github.com/rysweet/amplihack-rs/issues/890)
- **Related standard**: [Frontmatter Standards](../../amplifier-bundle/context/FRONTMATTER_STANDARDS.md)

## The Bug Class

Copilot CLI parses skill frontmatter with a strict schema. Several fields must
be a **single string value**, not a YAML collection. When a scalar field is
written as a list, Copilot CLI refuses to load the skill:

```text
✖ /home/azureuser/.copilot/skills/merge-ready/SKILL.md: argument-hint must be a string
```

The root cause is a common YAML footgun. Square brackets create a **list**, not
a string:

```yaml
# ✗ WRONG — YAML parses this as a one-element list: ["pr-number"]
argument-hint: [pr-number]

# ✓ CORRECT — quotes make it a string scalar: "[pr-number]"
argument-hint: "[pr-number]"
```

Both lines look almost identical to a human, but Copilot CLI treats the first
as a type error and drops the entire skill. This shipped once (fixed in commit
`e0abfb4` for the `merge-ready` skill) and the guard test exists so it can
**never** ship again.

### Fields Guarded

The guard validates exactly the fields Copilot CLI requires to be string
scalars:

| Field           | Required | Rule                                             |
| --------------- | -------- | ------------------------------------------------ |
| `name`          | Always   | Must be present and a string scalar.             |
| `description`   | Always   | Must be present and a string scalar.             |
| `argument-hint` | When present | If the key exists, its value must be a string scalar. |

Fields that legitimately accept lists or maps — such as `allowed-tools`,
`metadata`, or `hooks` — are intentionally **not** checked, to avoid false
positives. The guard targets the string-scalar contract only.

### Scope and Limitations

The guarded set is deliberately narrow: `name` and `description` are the only
**required** skill fields (per [Frontmatter Standards](../../amplifier-bundle/context/FRONTMATTER_STANDARDS.md)),
and `argument-hint` is the one field with a **demonstrated** production failure
(commit `e0abfb4`). The guard does **not** yet validate the other scalar-typed
optional fields Copilot CLI would also reject if mistyped — for example
`model`, `license`, and `compatibility` (expected string scalars) or
`user-invocable` and `disable-model-invocation` (expected booleans). None of
those currently appear mistyped in the bundle, so they are out of scope for the
initial guard. Extending coverage to them is straightforward future work: add
the field name and its expected YAML type to the guard's field/type table. Until
then, "always catches" applies to the three guarded fields across **every**
`SKILL.md`, not to the full skill schema.

**Valid** values (all parse as a YAML `String` scalar):

```yaml
name: merge-ready
description: Plain scalar description.        # plain
description: "A quoted description."          # double-quoted
description: |                                # literal block scalar
  A multi-line description that YAML
  still parses as a single String.
description: >                                # folded block scalar
  Also a String after folding.
argument-hint: "[pr-number]"                  # double-quoted
argument-hint: '[pr-number]'                  # single-quoted
```

Block (`|`) and folded (`>`) scalars are the common form for `description` in
this bundle (71 of 122 skills use one) and all parse to `serde_yaml::Value::String`,
so the guard accepts them. Only sequences and mappings fail.

**Violations** (parse as a sequence or mapping):

```yaml
argument-hint: [pr-number]       # sequence → FAILS
name: {value: merge-ready}       # mapping  → FAILS
```

## Running the Guard

Run the guard on its own:

```bash
cargo test -p amplihack --test skill_frontmatter_type -- --nocapture
```

Run it as part of the full suite (CI runs this on every PR):

```bash
cargo test -p amplihack
```

## Failure Output

On violation the test fails loudly, aggregating **every** offending file into a
single report that names the relative path, the field, and the YAML type that
was found:

```text
---- tc_type_02_string_scalar_fields_are_strings stdout ----
SKILL.md frontmatter TYPE violations (1 found) — every `name`, `description`,
and `argument-hint` must be a string scalar, NOT a list/sequence or mapping:
  amplifier-bundle/skills/merge-ready/SKILL.md → `argument-hint` must be a string, found sequence/list (Copilot CLI rejects non-string scalar fields)
```

The message tells you exactly what to fix: locate the named field in the named
file and wrap the value in quotes so YAML parses it as a string.

### Malformed or Missing Frontmatter

The guard also treats these cases as violations rather than panicking or
silently skipping — a malformed file reports cleanly alongside type violations:

| Condition                                        | Reported as |
| ------------------------------------------------ | ----------- |
| No frontmatter block (`---` fence not at byte 0) | Violation: frontmatter missing. |
| Frontmatter present but not a YAML mapping        | Violation: frontmatter is not a mapping. |
| Frontmatter fails to parse as YAML                | Violation: parse error (file + message). |
| Required `name` or `description` key absent       | Violation: required field missing. |

> **Note on frontmatter detection.** The guard reuses the sibling guard's
> `extract_frontmatter` helper to slice the raw `---`…`---` block, then parses
> that slice with `serde_yaml` (already a `[dev-dependencies]` entry) into a
> `Value::Mapping` for type inspection — the `name` guard's line-string parsing
> cannot distinguish a `String` from a `Sequence`, so YAML parsing is required.
> `extract_frontmatter` requires the opening `---` fence at byte 0 (offset 0);
> a leading BOM or blank line makes the frontmatter appear *absent* and is
> reported as a missing-frontmatter violation — matching the byte-0 entrypoint
> rule already enforced by the `name` guard.

## Test Cases

The guard is organized into four test cases:

| Test         | Purpose |
| ------------ | ------- |
| `TC-TYPE-01` | **Corpus sanity.** Asserts the skill walk found a non-empty corpus (`len() > 100`), so a broken filesystem scan can never silently pass. |
| `TC-TYPE-02` | **Primary guard.** For every `amplifier-bundle/skills/**/SKILL.md`, parses the frontmatter slice with `serde_yaml` into a `Value::Mapping` and asserts `name`, `description`, and (when present) `argument-hint` are each a `Value::String` — accepting plain, quoted, and block/folded forms. Aggregates violations and fails with a path + field + found-type report. |
| `TC-TYPE-03` | **Copilot rule.** Codifies the rule with inline literals: `argument-hint: [pr-number]` parses as a `Sequence` (must fail) and `argument-hint: "[pr-number]"` parses as a `String` (must pass). |
| `TC-TYPE-04` | **`merge-ready` regression pin.** Asserts `merge-ready/SKILL.md` `argument-hint` is the string `"[pr-number]"`, pinning the `e0abfb4` fix so it can never regress. |

> The only other skill that ships an `argument-hint` in its frontmatter,
> `statler-waldorf`, is **not** separately pinned — it is covered by the blanket
> `TC-TYPE-02` scan. Only `merge-ready` gets a dedicated pin because it is the
> field that actually broke in production.

## Configuration

Integration tests under `tests/integration/` are **not** auto-discovered by
Cargo in this workspace; each is registered explicitly. The guard is wired up
in `bins/amplihack/Cargo.toml`:

```toml
# Issue #890: Skill frontmatter TYPE validation.
# Ensures Copilot-CLI string-scalar fields (name, description, argument-hint)
# are never encoded as a YAML list/map. Regression guard for the
# merge-ready `argument-hint` type fix (commit e0abfb4).
[[test]]
name = "skill_frontmatter_type"
path = "../../tests/integration/skill_frontmatter_type_test.rs"
```

Without this block the test file compiles but never runs. If you add or move
the test, keep this registration in sync — verify with an explicit
`--test skill_frontmatter_type` run.

## Adding a New Skill

When you add a new skill, the guard runs automatically in CI. To stay green:

1. Write `name` and `description` as plain string scalars.
2. If your skill takes an argument, quote the hint: `argument-hint: "[value]"`.
3. Run `cargo test -p amplihack --test skill_frontmatter_type` locally before
   pushing.

If the guard fails, read the aggregated report, open the named `SKILL.md`, and
quote the named field.

## Verifying the Guard Actually Guards

The guard is designed to fail when the bug is reintroduced. You can prove this
locally without committing anything:

```bash
# 1. Baseline: the guard passes on the current tree.
cargo test -p amplihack --test skill_frontmatter_type

# 2. Reintroduce the bug (uncommitted edit).
sed -i 's/argument-hint: "\[pr-number\]"/argument-hint: [pr-number]/' \
  amplifier-bundle/skills/merge-ready/SKILL.md

# 3. The guard now FAILS, naming the file and field:
#    amplifier-bundle/skills/merge-ready/SKILL.md → `argument-hint` must be a string, found sequence/list
cargo test -p amplihack --test skill_frontmatter_type

# 4. Restore the correct source.
git checkout -- amplifier-bundle/skills/merge-ready/SKILL.md

# 5. Green again.
cargo test -p amplihack --test skill_frontmatter_type
```

## Audit Baseline

All 122 bundled `SKILL.md` files were inspected when the guard was introduced:

- Only `merge-ready` and `statler-waldorf` carry an `argument-hint` in their
  frontmatter, and both are already quoted string scalars. (`skill-builder`
  mentions `argument-hint` only in prose, not frontmatter, so it is irrelevant.)
- Every `name` and `description` value parses as a YAML `String`. `name` is
  always a plain scalar; `description` is a mix of plain, quoted, and
  block/folded (`|`, `>`) scalars — 71 of 122 use a block scalar — all of which
  are valid strings and pass the guard.
- **No source fixes were required** beyond the pre-existing `merge-ready` fix
  (`e0abfb4`). The deliverable is the guard test itself plus its Cargo
  registration.

## See Also

- [Frontmatter Standards](../../amplifier-bundle/context/FRONTMATTER_STANDARDS.md) — full field reference for skills, commands, workflows, and agents.
- [Skills Catalog](../skills/SKILL_CATALOG.md) — complete list of bundled skills.
- `tests/integration/skill_frontmatter_name_test.rs` — the sibling guard for skill `name` formatting (Issue #592), whose conventions this guard mirrors.
