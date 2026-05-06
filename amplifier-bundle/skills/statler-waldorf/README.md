# Statler & Waldorf Skill

*"We're not mad. We're just… unsurprised."*

Acerbic dual-voice analysis of meetings, transcripts, plans, and strategy documents.
Channels Statler & Waldorf from the Muppets — world-weary hecklers who mock process
and pretension, never people.

## Quick Start

Invoke with any of:
- `statler-waldorf` or `balcony-truth` (by name)
- "roast this meeting"
- "heckle" / "balcony review"

## When to Use It

**After a meeting that felt unproductive** — paste the transcript. It'll tell you
*why* it was unproductive using named failure archetypes, not just that it was.

**Before merging a strategy doc or RFC** — feed it the document. It treats plans
as "playbills for shows that haven't opened yet" and will find the missing verbs,
owners, and constraints.

**At the end of a long agent session** — it catches the loops, blind spots, and
local optimization traps that accumulate over hours of incremental work.

**When a PR description feels too polished** — the "Puppet Show" and "Hollow
Victory Lap" archetypes are built for this.

**When you suspect a decision was already made before the meeting** —
"Pre-Decision Theater" is one of its 14 archetypes.

### When NOT to Use It

- HR, legal, or grief-related meetings
- When you need a *fix*, not a *diagnosis* (use `crusty-old-engineer` instead)
- When the audience can't take a joke — though the severity dial goes down to
  "Matinee" for sensitive rooms

### Relationship to `crusty-old-engineer`

COE gives you engineering judgment directly. Statler & Waldorf wraps the same
depth of judgment in theatrical comedy — useful when the truth needs sugar, or
when you want a review format people will actually read and share.

## What It Reviews

| Input | Treatment |
|---|---|
| Meeting transcript | Stage performance |
| Copilot/agent session | One-act play between human and machine |
| Strategy document | Playbill for an unproduced show |
| Planning artifact | Stage directions for uncast actors |
| PR description | Curtain call |
| Chat thread | Improv comedy with no director |

## Output Structure (Five Acts)

1. **The Playbill** — Forensic summary (no snark yet)
2. **The Archetype** — Pattern matching to 14 failure archetypes
3. **The Balcony** — Statler & Waldorf dialog roasting the performance
4. **The Honest Curtain Call** — Actionable feedback (mandatory)
5. **The Epitaph** — One-line tombstone inscription

## Severity Levels

| Level | Name | Use When |
|---|---|---|
| 1 | Matinee | Execs in the room |
| 2 | Evening Show | Standard (default) |
| 3 | Late Night | Team can take it |
| 4 | Heckler's Veto | Among friends only |

## Guardrails

- No individual scoring
- No name attribution in critique
- No career commentary
- Humor targets systems, not humans
- Actionable feedback is mandatory
- Wisdom is load-bearing — every joke must trace to a known principle
- Thoroughness over brevity in prescriptions

See [SKILL.md](SKILL.md) for full specification.
