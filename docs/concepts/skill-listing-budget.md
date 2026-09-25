# The Skill Listing Budget

Every skill amplihack stages contributes its `name` and `description` to a
listing that is resident in **every** Claude Code session, before the user
types anything. That listing has a size budget. When the budget is exceeded,
Claude Code truncates the tail of the list to bare names — and says nothing.

This document explains why amplihack keeps the listing small on purpose, why
the fix was to shorten descriptions rather than to ship fewer skills, and why
the enforced limit is an empirical number rather than a published constant.

## The failure this prevents

A skill is selected for a task by matching the task against the skill's
`description`. A skill whose description has been truncated away still appears
in the listing by name, still installs, still runs when invoked explicitly —
and can no longer be matched automatically. It is staged but unreachable.

Before this budget was enforced, amplihack shipped 130 skills whose combined
name + description text measured **42,835 characters** — 2,170 of `name` and
40,665 of `description`, counting each value trimmed. The staged tree measures
the same, because staging copies `SKILL.md` unchanged. The listing was cut
alphabetically after `knowledge-extractor`, at a cumulative 21,054 characters.
Everything from `lawyer-analyst` through `xlsx` — 67 of 130 skills, more than
half the bundle — reached the model with no description at all.

The casualties were not obscure. In a Rust repository, the skills that had lost
their descriptions included `pr-guide`, `reviewing-code`, `testing-code`,
`smart-test`, `qa-team`, `verus-expert`, `oxidizer-workflow`,
`tla-plus-expert`, `skill-builder`, `pre-commit-manager`,
`property-based-testing`, and `test-gap-analyzer`.

Nothing in Claude Code, in `amplihack install`, or in CI reported this. The
install succeeded. The skills were all present on disk. The defect was visible
only by counting characters.

## Why this is an install-completeness defect

`amplifier-bundle/context/PHILOSOPHY.md` states that install must fail loudly
when a required component cannot be staged. A skill that is staged but cannot
be reached by the model is the same class of defect as a skill that was never
copied: the user paid for it, the installer reported success, and the
capability is not there.

So the budget check lives where the other completeness checks live — in
`verify_install_completeness` — and fails install the same way they do. See
[Install Completeness Verification](../reference/install-completeness.md).

## Why shorten, not remove

Three directions were available: stage fewer skills, nest skills so only a
subset is listed, or shorten what each skill contributes. Shortening is the
only one that costs nothing.

Most of the length was template, not signal. The 23 `*-analyst` skills alone
accounted for 12,138 characters — 28.3% of the 42,835-character listing — and
nearly all of them restated the same four-part scaffold:

```yaml
description: |
  Analyzes events through anthropological lens using cultural analysis, ethnographic methods, kinship and social organization,
  symbolic systems, ritual and practice, and comparative ethnology. Provides insights on cultural meanings, social practices,
  symbolic structures, cultural change, and cross-cultural patterns.
  Use when: Cultural conflicts, identity issues, ritual significance, symbolic meanings, cultural change, cross-cultural comparison.
  Evaluates: Cultural systems, symbolic meanings, social practices, kinship structures, cultural adaptation, power-culture nexus.
```

Of those 574 characters, the part that decides whether the skill gets selected
is the trigger list. The taxonomy prose — "Provides insights on…",
"Evaluates:…" — describes the discipline, not the situation, and no task
description matches against it. Rewritten:

```yaml
description: Anthropology lens on culture, ritual, kinship, and symbols. Use for cultural conflict, identity, or cross-cultural work.
```

Same routing signal, 120 characters instead of 574. No skill removed, no
behaviour changed.

Applying that to all 130 skills brought the listing from **42,835** characters
to **15,801** — a 63% reduction, 4,199 characters of headroom under the enforced
limit — with every skill's triggers intact, no skill removed, and no behaviour
changed. The average description is now 105 characters and the longest is 128.

That number moves with every skill edit, so it is not restated elsewhere in the
docs; `scripts/check-skill-description-budget.sh` is the authority on what the
bundle actually measures, and `I-02` is what stops it drifting back.

## Why the descriptions are not simply truncated

The old template put the trigger clause **last**. Mechanically cutting each
description to its first 120 characters would have deleted precisely the part
that matters and kept the part that does not. Every rewrite is read, and takes
the form:

```
<What it does>. Use when <triggers>.
```

## Why 20,000 is an empirical number

The issue reports that Claude Code budgets this listing at roughly 1% of the
context window. One percent of a 1M-token window is about 10,000 tokens, or
roughly 40,000 characters — which does not reconcile with a truncation observed
at 21,054 characters (~5,300 tokens). Either the fraction differs from 1%, or
the budget scales with the window differently than the raw fraction suggests,
or both.

That question cannot be answered from inside this repository, and this project
does not pretend otherwise. What it does instead is work under the uncertainty
with headroom:

| Number | Value | Where it comes from |
| --- | --- | --- |
| Observed truncation point | ~21,054 chars | Measured on a clean install, 2026-09-23 |
| `SKILL_LISTING_BUDGET_CHARS` | 20,000 | The observed cut, less ~5% |
| Rewrite landing target | ≤ 18,000 | 2,000 further characters of growth room |

Two layers of slack, one constant to change. The Rust source comment on
`SKILL_LISTING_BUDGET_CHARS` records that it is measured rather than published,
gives the measurement and its date, and warns that a smaller context window may
impose a tighter budget. If a future measurement contradicts it, the fix is to
edit one number.

## What the guard does not measure

The check counts only the skills **amplihack staged**, under
`~/.amplihack/.claude/skills`. It never counts the user's own
`~/.claude/skills`.

The budget in Claude Code is genuinely shared between the two — a user with 200
personal skills will be truncated regardless of what amplihack does. But making
that a fatal install error would lock a user out of installing amplihack
because of files amplihack does not own. The check holds amplihack to its own
footprint and says so in the failure message.

## See Also

- [Skill Listing Budget Reference](../reference/skill-listing-budget.md) — the constant, the API, the exact failure messages, the tests.
- [Keep Skill Descriptions in Budget](../howto/keep-skill-descriptions-in-budget.md) — how to measure, add a skill, and fix an over-budget failure.
- [Install Completeness Verification](../reference/install-completeness.md) — the other checks that fail install loudly.
- [Frontmatter Standards](../../amplifier-bundle/context/FRONTMATTER_STANDARDS.md) — the `description` field contract.
- [Skill Frontmatter Type Guard](../testing/SKILL_FRONTMATTER_TYPE_GUARD.md) — the sibling guard on frontmatter types.
