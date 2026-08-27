#!/usr/bin/env bash
# Guard for issue #1334 — a step may not claim to be optional in prose only.
#
# A recipe step whose prompt says "(if applicable)", "Skip if ...", or "run this
# step only when ..." is describing control flow. Prose inside a prompt cannot
# skip anything: the engine has already spawned the agent by the time it reads
# the sentence. The cost of an optional step is paid at spawn, not at the answer.
#
# So: any step whose prompt makes a step-level skip claim MUST carry a
# `condition:` that plausibly governs that claim. A condition about something
# else does not count — every phase brick already carries an unrelated
# resume-checkpoint condition, and treating that as absolution is exactly how
# this bug survived in workflow-tdd.yaml while an earlier version of this guard
# reported clean.
#
# Run: bash scripts/check-prose-only-skips.sh [recipe-dir]

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPE_DIR="${1:-$REPO_ROOT/amplifier-bundle/recipes}"

if [[ ! -d "$RECIPE_DIR" ]]; then
  echo "error: recipe directory not found: $RECIPE_DIR" >&2
  exit 1
fi

python3 - "$RECIPE_DIR" <<'GUARD_EOF'
import os, re, sys, glob

recipe_dir = sys.argv[1]

# A step-level claim -- "this STEP is conditional" -- is a lie when the engine
# has no `condition:` to act on. Item-level guidance -- "cover this bullet if
# relevant", "skip this shell flag when X is empty" -- is ordinary prompt text.
#
# Flagged (step-level):
#     # Step 5c: Database Design (if applicable)
#     ## Step D: Migration Plan (optional)
#     **Step A: API Design (if applicable)**
#     Skip if no database work is required.
#     Run this step only if the task touches the database.
#     Perform this only when the change introduces a new endpoint.
# Not flagged (item-level):
#     - API documentation (if applicable)
#     3. Authentication issues (if applicable)
#     Run `gh issue view` (skip if `{{issue_number}}` is empty).
OPTIONAL_MARK = re.compile(r"\(\s*(?:if\s+applicable|optional)\s*\)", re.IGNORECASE)
STEP_HEADING = re.compile(r"^(?:#{1,6}\s|\*\*)")
SKIP_SENTENCE = re.compile(
    r"^(?:skip|omit)\s+(?:this\s+step\s+)?(?:if|when)\b"
    r"|^(?:run|perform|execute|do)\s+(?:this\s+|it\s+)?(?:step\s+)?only\s+(?:if|when)\b"
    r"|^only\s+(?:run|perform|execute)\b"
    r"|^this\s+step\s+is\s+optional\b",
    re.IGNORECASE)

PROMPT_INDENT = 6
LIST_ITEM = re.compile(r"^(?:[-*+]|\d+[.)])\s")
STEP_START = re.compile(r"^\s{2}-\s+id:\s*[\"']?([A-Za-z0-9_.-]+)")
FIELD = re.compile(r"^\s{4}([a-z_]+):")
WORD = re.compile(r"[a-z_]{4,}")

# Words carrying no topic. A condition sharing only these with the prose tells
# us nothing about whether it governs the claim.
STOPWORDS = {
    "this", "that", "step", "task", "when", "with", "from", "into", "then",
    "true", "false", "none", "null", "required", "requires", "work", "needed",
    "applicable", "optional", "skip", "only", "does", "have", "been", "will",
}


def topic_words(text):
    """Topic-bearing words, singularised so 'integrations' matches
    'requires_integration_work'."""
    out = set()
    for w in WORD.findall(text.lower()):
        if w in STOPWORDS:
            continue
        out.add(w)
        if w.endswith("s") and len(w) > 4:
            out.add(w[:-1])
    return out


def governs(claim, condition):
    """True when the condition plausibly refers to what the claim is about.

    Substring either way, because conditions name compound context fields
    (`requires_integration_work`) while prose uses bare nouns
    ('integrations').
    """
    cond = condition.lower()
    return any(w in cond for w in topic_words(claim))


def step_level_claim(text):
    """Return the claim text when the line asserts the STEP is conditional."""
    stripped = text.strip()
    indent = len(text) - len(text.lstrip())
    if indent < PROMPT_INDENT:        # YAML structure or comment, not prompt body
        return None
    if LIST_ITEM.match(stripped):      # a bullet is about the bullet
        return None
    if OPTIONAL_MARK.search(stripped) and STEP_HEADING.match(stripped):
        return stripped
    if SKIP_SENTENCE.match(stripped):  # anchored: a standalone sentence, not an aside
        return stripped
    return None


violations = []
scanned = 0

for path in sorted(glob.glob(os.path.join(recipe_dir, "*.yaml"))):
    lines = open(path, encoding="utf-8").read().splitlines()
    steps, cur = [], None
    for i, line in enumerate(lines):
        m = STEP_START.match(line)
        if m:
            if cur:
                steps.append(cur)
            cur = {"id": m.group(1), "condition": None, "prompt": []}
            continue
        if cur is None:
            continue
        f = FIELD.match(line)
        if f and f.group(1) == "condition":
            cur["condition"] = line
        cur["prompt"].append((i, line))
    if cur:
        steps.append(cur)

    for st in steps:
        scanned += 1
        for lineno, text in st["prompt"]:
            claim = step_level_claim(text)
            if not claim:
                continue
            if st["condition"] and governs(claim, st["condition"]):
                break
            violations.append(
                (os.path.relpath(path, os.path.dirname(recipe_dir)),
                 lineno + 1, st["id"], claim[:72],
                 "unrelated condition" if st["condition"] else "no condition"))
            break

if not scanned:
    print("error: no recipe steps found -- guard would pass vacuously", file=sys.stderr)
    sys.exit(1)

if violations:
    print("FAIL: %d step(s) claim to be optional in prose without a `condition:` "
          "that governs the claim\n" % len(violations), file=sys.stderr)
    for path, lineno, sid, text, why in violations:
        print("  %s:%d  step '%s'  (%s)" % (path, lineno, sid, why), file=sys.stderr)
        print("      %s" % text, file=sys.stderr)
    print("\nAdd a `condition:` the engine evaluates before spawning the agent,",
          file=sys.stderr)
    print("or remove the step-level skip language. See issue #1334.", file=sys.stderr)
    sys.exit(1)

print("OK: %d recipe steps scanned, no prose-only skips" % scanned)
GUARD_EOF
