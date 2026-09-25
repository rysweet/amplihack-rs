# Keep Skill Descriptions in Budget

Every bundled skill contributes its `name` and `description` to a listing that
Claude Code loads into every session. Past a limit, Claude Code truncates that
listing to bare names and the affected skills stop being selectable — silently.
`amplihack install`, a pre-commit hook, and a CI test all fail loudly instead.

This guide covers measuring the listing, writing a description that fits, and
fixing each of the three ways the budget can fail on you.

For why the limit exists and where the number came from, see
[The Skill Listing Budget](../concepts/skill-listing-budget.md). For the API,
the constant, and the test list, see
[Skill Listing Budget Reference](../reference/skill-listing-budget.md).

## Measure the listing

From the repository root:

```bash
scripts/check-skill-description-budget.sh
```

```text
skill listing budget: 15801 / 20000 characters (130 skills, 4199 to spare)
```

Your numbers will differ — the total moves with every skill edit, and this
example is illustrative. What matters is the exit code: **0** means you are
under the limit, **1** prints the offenders largest-first.

To get the same number from Rust — for example inside a test — use
`collect_skill_listing`; see the
[library API](../reference/skill-listing-budget.md#library-api). Both counters
trim each value before measuring, which is why they agree; see
[Counting rules](../reference/skill-listing-budget.md#what-counts-toward-the-budget).

## Write a description that fits

Target **about 120 characters**, in this shape:

```
<What it does>. Use when <triggers>.
```

The `Use when` clause is the part that decides whether the skill gets selected,
so it is never the part you cut. A skill selected for a task is matched against
its description; taxonomy prose that describes the discipline rather than the
situation costs characters and matches nothing.

Write it as a **single-line YAML scalar**. At ~120 characters a block scalar
(`|` or `>`) buys nothing, makes the diff harder to check mechanically, and the
budget script rejects it outright. Quote the value only when it contains a `:`
or starts with a YAML indicator character.

```yaml
---
name: smart-test
description: Runs the tests a change actually affects instead of the whole suite. Use when iterating on a fix or a failing test.
---
```

Things that do not belong in a description, because these strings are injected
into an agent's context by construction:

- Imperatives aimed at the agent ("always run…", "skip confirmation", "ignore
  previous instructions")
- URLs, file paths, shell commands
- Anything credential-shaped

## Shorten an over-long description

Take the longest offender the script named and cut the template, not the
triggers. Before — 574 characters:

```yaml
description: |
  Analyzes events through anthropological lens using cultural analysis, ethnographic methods, kinship and social organization,
  symbolic systems, ritual and practice, and comparative ethnology. Provides insights on cultural meanings, social practices,
  symbolic structures, cultural change, and cross-cultural patterns.
  Use when: Cultural conflicts, identity issues, ritual significance, symbolic meanings, cultural change, cross-cultural comparison.
  Evaluates: Cultural systems, symbolic meanings, social practices, kinship structures, cultural adaptation, power-culture nexus.
```

After — 120 characters:

```yaml
description: Anthropology lens on culture, ritual, kinship, and symbols. Use for cultural conflict, identity, or cross-cultural work.
```

The triggers survive; "Provides insights on…" and "Evaluates:…" do not.

Change **only** the `description` value. Do not touch `name` — it is referenced
from recipes, tests, and the catalog — any other frontmatter key, or one byte of
the body. Re-run the script, then read the diff:

```bash
git diff -- amplifier-bundle/skills/anthropologist-analyst/SKILL.md
```

Every changed line must sit inside the frontmatter `description` value. If the
diff touches `name`, any other key, or anything after the closing `---`, undo
that part before committing.

## Fix a failing pre-commit hook

```text
skill listing budget EXCEEDED: 20418 / 20000 characters (131 skills, 418 over)
largest first:
   775  code-atlas
   662  signal-setup
   ...
```

You added a skill, or lengthened a description, and the bundle went over.
Shorten descriptions until the script exits 0 — starting with whatever it lists
first, which is usually not the skill you just touched.

Do **not** raise `SKILL_LISTING_BUDGET_CHARS` to make the message go away. The
limit is set ~5% below a measured truncation point; raising it does not give
Claude Code more room, it just stops telling you that skills have gone dark.

## Fix a failing install

```text
install completeness verification failed for /home/you/.amplihack/.claude:
  - staged skill listing is over budget: 24118 of 20000 characters across 131 skills
    at /home/you/.amplihack/.claude/skills
```

The check measures only the skills amplihack staged under
`~/.amplihack/.claude/skills`. Two causes:

**A stale staged tree.** Skill publishing is deliberately non-fatal, so a
partial publish can leave the old long descriptions staged against an
already-rewritten bundle. Clear it and reinstall:

```bash
rm -rf ~/.amplihack/.claude/skills
amplihack install
```

**A skill you added to the staged tree yourself.** The failure message names it
in the largest-first list. Shorten its `description`, or move it to
`~/.claude/skills`, which this check does not measure.

## Check your own skills against the shared budget

The budget in Claude Code is shared between amplihack's staged skills and your
personal `~/.claude/skills`. amplihack only polices its own footprint — if your
own skills are crowding the listing, nothing in amplihack will tell you. Measure
them the same way:

```bash
python3 - ~/.claude/skills <<'PY'
import os, re, sys, yaml
root = os.path.expanduser(sys.argv[1] if len(sys.argv) > 1 else "~/.claude/skills")
rows = []
for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
    dirnames[:] = [d for d in dirnames if not os.path.islink(os.path.join(dirpath, d))]
    if "SKILL.md" not in filenames:
        continue
    text = open(os.path.join(dirpath, "SKILL.md"), encoding="utf-8").read()
    match = re.match(r"^---\n(.*?)\n---", text, re.S)
    data = yaml.safe_load(match.group(1)) if match else {}
    fields = [data.get(k, "") for k in ("name", "description")] if isinstance(data, dict) else []
    # .strip() matches the counting rule amplihack's own guard uses: block
    # scalars carry a trailing newline the author did not write.
    chars = sum(len(v.strip()) for v in fields if isinstance(v, str))
    rows.append((chars, os.path.basename(dirpath)))
rows.sort(reverse=True)
print(f"{sum(c for c, _ in rows)} characters across {len(rows)} skills in {root}")
for chars, name in rows[:10]:
    print(f"{chars:6}  {name}")
PY
```

```text
47940 characters across 138 skills in /home/you/.claude/skills
   981  docs
   964  pptx
   952  xlsx
   938  docx
   ...
```

That directory holds first-party Claude Code skills and anything you wrote, as
well as amplihack's published copies. Shortening the descriptions of your own
skills is the only lever you have over the non-amplihack part of the total.

## Add a new bundled skill

1. Write `name` and `description` as single-line string scalars, description at
   ~120 characters in the `<What it does>. Use when <triggers>.` form.
2. Run `scripts/check-skill-description-budget.sh`. If you pushed the bundle
   over, shorten the largest existing descriptions it names.
3. Run the frontmatter type guard, which checks a different property of the same
   two fields:
   ```bash
   cargo test -p amplihack --test skill_frontmatter_type
   ```
4. Run the budget test:
   ```bash
   cargo test -p amplihack --test skill_description_budget -- --nocapture
   ```

`I-01` pins the bundled `SKILL.md` count, so adding or removing a skill is a
deliberate edit to that test, never an accident.

## See Also

- [The Skill Listing Budget](../concepts/skill-listing-budget.md) — why the budget exists.
- [Skill Listing Budget Reference](../reference/skill-listing-budget.md) — constant, API, failure messages, tests.
- [Install Completeness Verification](../reference/install-completeness.md) — the other checks that fail install.
- [Frontmatter Standards](../../amplifier-bundle/context/FRONTMATTER_STANDARDS.md) — the full frontmatter field reference.
