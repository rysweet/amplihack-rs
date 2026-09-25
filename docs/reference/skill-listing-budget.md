# Skill Listing Budget Reference

`amplihack install` fails loudly when the skills it stages contribute more
`name` + `description` text than Claude Code will keep. Past that limit Claude
Code silently truncates the tail of its skill listing to bare names, and the
truncated skills can no longer be matched to a task.

- **Module**: `crates/amplihack-cli/src/skill_listing_budget.rs`
- **Install hook**: `verify_skill_description_budget` in `crates/amplihack-cli/src/commands/install/verification.rs`
- **Script**: `scripts/check-skill-description-budget.sh`
- **Integration test**: `tests/integration/skill_description_budget_test.rs` (cargo target `skill_description_budget`)
- **Issue**: [#1459](https://github.com/rysweet/amplihack-rs/issues/1459)
- **Rationale**: [The Skill Listing Budget](../concepts/skill-listing-budget.md)

## Numbers

| Quantity | Value |
| --- | --- |
| Bundled `SKILL.md` files | 130 |
| `SKILL_LISTING_BUDGET_CHARS` (enforced limit) | 20,000 |
| Rewrite landing target | ≤ 18,000 |
| Per-skill description target | ~120 characters |
| Observed Claude Code truncation point | ~21,054 characters (measured 2026-09-23) |
| Listing total before the #1459 rewrite | 42,835 (2,170 name + 40,665 description) |

The **current** total is deliberately not restated here, because it moves with
every skill edit. Get it from the script:

```bash
scripts/check-skill-description-budget.sh
```

Measured under the trimming rule below, the in-repo tree and a staged install
produce the **same** total: staging copies `SKILL.md` byte-for-byte and
preserves directory structure, so the two figures agree by construction. A
divergence means the staged tree is stale, not that the measurements differ.

## What counts toward the budget

Only two frontmatter fields, from files named exactly `SKILL.md`:

| Counted | Not counted |
| --- | --- |
| The `name` value | Any other frontmatter key (`metadata`, `allowed-tools`, `model`, `argument-hint`, …) |
| The `description` value | The Markdown body after the closing `---` |
| | Files that are not named `SKILL.md` |

Counting rules:

- **The parsed scalar is trimmed before counting.** YAML literal (`|`) and
  folded (`>`) block scalars carry a trailing newline that the author did not
  write, so an untrimmed count charges skills for their own formatting. This
  rule is load-bearing, not cosmetic: 73 of the 130 bundled descriptions are
  block scalars and 29 of those carry a trailing newline, so the untrimmed
  total is 42,864 rather than 42,835 and per-skill figures shift by one
  (`code-atlas` 776 rather than 775). Both the Rust walk and the shell script
  trim; `I-03` asserts they agree, and that assertion only holds because both
  sides make the same choice.
  Trimming does **not** become moot after the #1459 rewrite. The rewrite
  removes block scalars from `amplifier-bundle/skills`, but the tree the
  install guard measures is user-writable, and a user-authored `SKILL.md` may
  use any scalar form.
- **Bytes**, via `str::len()` on the trimmed value. For this ASCII corpus bytes
  equal characters. Bytes are used rather than Unicode scalars because the shell
  re-measurement cannot count scalars portably (`mawk` counts bytes, `gawk`
  counts characters), and a counter that disagrees with the number quoted in a
  PR is worse than one that is off by zero.
- Values count only when they parse as a YAML **string** scalar. A sequence or
  mapping counts 0 and is reported as malformed. Plain, quoted, literal (`|`)
  and folded (`>`) scalars all count normally.
- The walk **recurses**, so the seven nested skills under `collaboration/` (1),
  `development/` (2), `meta-cognitive/` (1), `quality/` (2) and `research/` (1)
  are included. Nesting is **preserved** when the bundle is staged — the staged
  tree has the same `quality/reviewing-code/SKILL.md` shape as the repository —
  so recursing is what makes the in-repo and staged totals identical. A
  non-recursive walk would miss the same seven skills in both trees and
  under-report by the same amount in each, which is exactly the kind of
  consistent-but-wrong number this check exists to prevent.
  (`amplifier-bundle/skills/common/` is **not** in that list. It holds shared
  assets — `README.md`, `dependencies.txt`, `ooxml/`, `verification/` — and no
  `SKILL.md`, so it contributes nothing.)
- Symlinked files and symlinked directories are skipped, not followed. The walk
  root itself may be a symlink.

## Library API

Add to a `Cargo.toml` that already depends on `amplihack-cli`; the module has no
dependencies beyond `std::fs` and the already-vendored `serde_yaml`.

```rust
use amplihack_cli::skill_listing_budget::{
    SKILL_LISTING_BUDGET_CHARS, SkillListingEntry, collect_skill_listing, over_budget_report,
};
use std::path::Path;

let entries = collect_skill_listing(Path::new("amplifier-bundle/skills"))?;
let total: usize = entries.iter().map(|e| e.chars).sum();

println!("{} skills, {total} characters", entries.len());
// Example output: 130 skills, 15801 characters - the exact total moves with
// every skill edit; only the `<= SKILL_LISTING_BUDGET_CHARS` relation is pinned.

match over_budget_report(&entries, SKILL_LISTING_BUDGET_CHARS) {
    None => println!("under budget"),
    Some(report) => eprintln!("{report}"),
}
// Output: under budget
```

### `SKILL_LISTING_BUDGET_CHARS`

```rust
pub const SKILL_LISTING_BUDGET_CHARS: usize = 20_000;
```

The enforced limit. **Empirical, not a published Claude Code constant**: on a
clean install on 2026-09-23 the listing was observed to truncate at a cumulative
21,054 characters, and this value takes ~5% off that. A smaller context window
may impose a tighter budget. The source comment carries the same warning, so
the next person to measure this does not mistake it for documented behavior.

### `SkillListingEntry`

```rust
pub struct SkillListingEntry {
    /// Sanitized skill label — the directory name, restricted to
    /// `[A-Za-z0-9._-]` and truncated to 64 characters.
    pub name: String,
    /// Bytes this skill contributes: trimmed `name` value + trimmed
    /// `description` value.
    pub chars: usize,
}
```

The label comes from the **directory name**, not the frontmatter `name` field —
a shorter trust chain — and is sanitized either way. An unsanitized label
containing ANSI escapes, `\r` or `\n` could rewrite the install transcript,
including forging the success line.

### `collect_skill_listing`

```rust
pub fn collect_skill_listing(root: &Path) -> anyhow::Result<Vec<SkillListingEntry>>;
```

Recursively walks `root` for `SKILL.md` files and returns one entry per file,
in descending `chars` order. Each field is parsed as YAML, **trimmed**, and
then measured with `str::len()` — see
[Counting rules](#what-counts-toward-the-budget); the trim is what makes the
Rust total match the script's.

| Condition | Behavior |
| --- | --- |
| `root` does not exist | `Ok(vec![])` |
| `name` or `description` is a block scalar (`\|`, `>`) | Counts normally, trailing newline trimmed off |
| `name` or `description` is empty or whitespace-only | Counts 0, entry still emitted |
| I/O error below an existing `root` | `Err` — the check fails closed rather than reporting a smaller passing total |
| File larger than 1 MiB | Skipped, counts 0 |
| Frontmatter block larger than 64 KiB | Skipped, counts 0 |
| Missing or unparseable frontmatter | Entry with `chars: 0`, reported as malformed |
| `name` or `description` is a sequence or mapping | That field counts 0, reported as malformed |
| Symlinked `SKILL.md` or symlinked directory | Skipped entirely |

Only the frontmatter block — from the leading `---` to the next `---` — is
handed to `serde_yaml`. The Markdown body never is. Both the size caps and the
body exclusion exist because a staged `SKILL.md` is user-writable: a YAML alias
or merge-key bomb in the body would otherwise turn install into an OOM.

The library and the script deliberately differ on block scalars: the library
**tolerates** them because the tree it measures at install time is user-authored
and may contain any scalar form, while the script **hard-fails** on them because
it only ever walks `amplifier-bundle/skills`, where the #1459 rewrite leaves
none. `I-03` compares totals on the repository corpus, where the two agree
because no block scalar is present for the script to reject.

The module contains no `unwrap`, no `expect`, no panicking index, and no
`#[allow]` escapes. A malformed staged file degrades to a report line; it never
aborts install with a backtrace.

### `over_budget_report`

```rust
pub fn over_budget_report(entries: &[SkillListingEntry], budget: usize) -> Option<String>;
```

Returns `None` when the entries total at most `budget` (including for an empty
slice), otherwise a multi-line report naming the offending skills largest-first
with the running total and the recovery action.

**The report contains skill labels and character counts only — never
description or body text.** Descriptions are attacker-influenceable strings
that would land verbatim in an agent's context, making an install error a
prompt-injection channel, and a user's own staged skill may hold private
content. The module documentation says so, so a later "helpful" change does not
add it back.

## Install behavior

`verify_skill_description_budget` runs inside `verify_install_completeness`,
**after** `verify_skill_count` and **before** `verify_staged_bundle`.

The budget check treats a missing staged skills root as an empty listing, which
passes. On its own that is a hole — an install that staged nothing would report
"under budget". It is not a hole in context, and the source comment states the
reason precisely:

- If the **source** bundle has no `skills/` directory, `verify_skill_count`
  returns `Ok(())` early without complaining. So does the budget check, and
  correctly: there were no skills to stage, so an empty listing is the right
  answer, not a missed failure.
- If the source bundle **does** have `skills/`, `verify_skill_count` compares
  the source and staged directory counts and pushes a `missing` entry when the
  staged tree is short — including when it is absent, which counts as 0. That
  failure is already recorded by the time the budget check runs.

In neither case can an empty staged listing pass silently. The ordering is what
lets the budget check stay simple enough to have no opinion about absence.

Note that `verify_skill_count` counts **immediate child directories**, while the
budget check counts `SKILL.md` **files** found recursively. The two numbers
differ by design (129 versus 130 for the current bundle: `common/` is a
directory with no `SKILL.md`, and the five category directories each hold one or
more nested skills). Neither is wrong; they measure different things.

It measures `<claude_dir>/skills` — the amplihack-staged tree — and nothing
else. It never reads `~/.claude/skills`; see
[the concepts doc](../concepts/skill-listing-budget.md#what-the-guard-does-not-measure)
for why.

Failures are pushed onto the same `missing` list as every other completeness
check, so install exits non-zero through the existing `bail!`. Example of a
stale staged tree — pre-rewrite descriptions still on disk, plus one skill the
user added themselves:

```text
install completeness verification failed for /home/you/.amplihack/.claude:
  - staged skill listing is over budget: 24118 of 20000 characters across 131 skills
    at /home/you/.amplihack/.claude/skills
      4842  my-local-experiment
       775  code-atlas
       662  signal-setup
       619  computer-scientist-analyst
       617  environmentalist-analyst
      (+126 more)
    Claude Code truncates the tail of this listing to bare names, so skills past
    the cut cannot be matched to a task.
    Fix: shorten the `description` frontmatter of the skills named above to about
    120 characters. If you did not author them, remove
    /home/you/.amplihack/.claude/skills and re-run `amplihack install`.
```

The recovery action is mandatory in the message. Making this check fatal turns
a cosmetic problem into a broken install; without a stated way out that is a
self-inflicted denial of service. The stale-staged-tree case is real:
`publish_skills` is deliberately non-fatal, so a partial publish can leave old
long descriptions staged against an already-rewritten bundle — a hard failure on
a tree the user did not author.

## Script

`scripts/check-skill-description-budget.sh` re-measures the repository corpus
without cargo, so the number can be checked in about a second and quoted in a
PR. It mirrors `scripts/check-brick-budget.sh` in structure.

```bash
scripts/check-skill-description-budget.sh
```

Example output when under budget — the total is illustrative and moves with
every skill edit; only the `<= 20000` relation is pinned:

```text
skill listing budget: 15801 / 20000 characters (130 skills, 4199 to spare)
```

Example output when over — these are the real pre-rewrite figures:

```text
skill listing budget EXCEEDED: 42835 / 20000 characters (130 skills, 22835 over)
largest first:
   775  code-atlas
   662  signal-setup
   619  computer-scientist-analyst
   617  environmentalist-analyst
   596  anthropologist-analyst
   ...
Shorten the `description` frontmatter of the skills above to about 120 chars.
```

| Exit code | Meaning |
| --- | --- |
| 0 | Total is at or under the limit |
| 1 | Total is over the limit, or the script could not do its job |

Behavior notes:

- The limit is **read from** `crates/amplihack-cli/src/skill_listing_budget.rs`
  with `sed`, not copied, so the script cannot drift from the rule it reports.
  The scraped value is validated against `^[0-9]+$` before any arithmetic. Bash
  `(( ))` evaluates its operands as arithmetic expressions, and arithmetic
  evaluation runs command substitution inside array subscripts — an unvalidated
  `x[$(...)]` in that variable would be code execution at pre-commit time.
  `check-brick-budget.sh` only checks its scraped limit for non-emptiness; this
  one checks the shape.
- If the limit cannot be read, or the walk finds no `SKILL.md` files, the script
  exits 1 rather than passing vacuously.
- It walks `amplifier-bundle/skills` — the real directory, not the repo-root
  `skills` symlink — with POSIX `find`, which does not descend symlinks without
  `-H`/`-L`. This matches the Rust walk so the two always report the same total.
- It **trims** each value before measuring, matching the Rust walk. This is a
  requirement, not an implementation detail: `I-03` asserts the two totals are
  equal, and that assertion is only meaningful if both sides made the same
  choice about trailing whitespace. A script that did not trim would report
  42,864 where the library reports 42,835 on the pre-rewrite corpus.
- It hard-fails on a block scalar (`|`, `>`) `description` rather than silently
  miscounting one. After the #1459 rewrite every description is a single-line
  scalar, so a block scalar means someone reintroduced the padded form. The
  Rust library deliberately does not share this rule — it measures a
  user-authored tree; see
  [`collect_skill_listing`](#collect_skill_listing).
- bash 3.2 compatible (`while read`, no `mapfile`) so it runs on stock macOS —
  see [#1423](https://github.com/rysweet/amplihack-rs/issues/1423).
- A newline in a filename is a hard error, not a silent field split.

### Pre-commit

The script is wired into `.pre-commit-config.yaml` next to `check-brick-budget`:

```yaml
      - id: check-skill-description-budget
        name: staged skill name+description under the listing budget (issue #1459)
        entry: scripts/check-skill-description-budget.sh
        language: system
        pass_filenames: false
        files: '(^amplifier-bundle/skills/.*/SKILL\.md$|^crates/amplihack-cli/src/skill_listing_budget\.rs$)'
```

It needs no network and no cargo, and exits 0 without writing anything when
under budget.

> `pre-commit run --all-files` on an untrusted branch already executes that
> branch's `language: system` scripts. This hook is one more of those; it does
> not change the trust model. Review fork PRs through CI, not through local
> pre-commit.

## Tests

Unit tests live in `#[cfg(test)]` inside `skill_listing_budget.rs`; integration
tests in `tests/integration/skill_description_budget_test.rs`, registered in
`bins/amplihack/Cargo.toml`:

```toml
# Issue #1459: skill name+description listing must stay under the Claude Code
# budget, or skills past the cut reach the model with no description and cannot
# be matched to a task.
[[test]]
name = "skill_description_budget"
path = "../../tests/integration/skill_description_budget_test.rs"
```

Integration tests under `tests/integration/` are not auto-discovered by Cargo in
this workspace. Without that block the file compiles but never runs.

| Test | Purpose |
| --- | --- |
| `U-01` | Over-budget slice produces a report naming offenders largest-first with the running total. |
| `U-02` | Under-budget slice, and an empty slice, both produce `None`. |
| `U-03` | Malformed, truncated, and oversized frontmatter yield a report line, never a panic. |
| `U-04` | (unix) A symlinked `SKILL.md` and a symlinked directory contribute 0 and do not hang. |
| `U-05` | A skill label containing `\x1b[2K\r` and `\n` is sanitized out of the report. |
| `U-06` | The offending description text is absent from the report. |
| `I-01` | Exactly 130 `SKILL.md` files are discovered in `amplifier-bundle/skills`. |
| `I-02` | The real bundle total is at or under `SKILL_LISTING_BUDGET_CHARS`; on failure it prints the full report and the headroom. |
| `I-03` | (unix) The script's total equals `collect_skill_listing`'s total. |

The failing path is exercised with synthetic entries rather than a committed
fixture tree of deliberately padded skills — a fixture like that is something a
future contributor would "fix".

Run them:

```bash
cargo test -p amplihack-cli skill_listing
cargo test -p amplihack --test skill_description_budget -- --nocapture
```

## Verifying the guard actually guards

Prove it fails when the bug returns, without committing anything:

```bash
# 1. Baseline: green.
scripts/check-skill-description-budget.sh

# 2. Blow the budget on one skill (uncommitted edit).
python3 - <<'PY'
import pathlib
p = pathlib.Path("amplifier-bundle/skills/code-atlas/SKILL.md")
s = p.read_text()
p.write_text(s.replace("description: ", "description: " + "padding. " * 500, 1))
PY

# 3. The script now FAILS, naming code-atlas first.
scripts/check-skill-description-budget.sh

# 4. Restore.
git checkout -- amplifier-bundle/skills/code-atlas/SKILL.md

# 5. Green again.
scripts/check-skill-description-budget.sh
```

## See Also

- [The Skill Listing Budget](../concepts/skill-listing-budget.md) — why the budget exists and why 20,000 is empirical.
- [Keep Skill Descriptions in Budget](../howto/keep-skill-descriptions-in-budget.md) — task-focused guide for skill authors.
- [Install Completeness Verification](install-completeness.md) — the sibling checks that fail install loudly.
- [Frontmatter Standards](../../amplifier-bundle/context/FRONTMATTER_STANDARDS.md) — the `description` field contract.
- [Skill Frontmatter Type Guard](../testing/SKILL_FRONTMATTER_TYPE_GUARD.md) — the guard on frontmatter field types.
- [Skills Catalog](../skills/SKILL_CATALOG.md) — the full list of bundled skills.
