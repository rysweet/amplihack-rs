#!/usr/bin/env bash
# Guard for issue #1334 — a step may not claim to be optional in prose only.
#
# A recipe step whose prompt says "(if applicable)", "Skip if ...", or "only if
# ..." is describing control flow. Prose inside a prompt cannot skip anything:
# the engine has already spawned the agent by the time it reads the sentence.
# The cost of an optional step is paid at spawn, not at the answer.
#
# So: any step whose prompt contains skip language MUST carry a `condition:`
# that the engine evaluates before spawning. This guard fails otherwise.
#
# Run: bash scripts/check-prose-only-skips.sh [recipe-dir]

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPE_DIR="${1:-$REPO_ROOT/amplifier-bundle/recipes}"

if [[ ! -d "$RECIPE_DIR" ]]; then
  echo "error: recipe directory not found: $RECIPE_DIR" >&2
  exit 1
fi

python3 - "$RECIPE_DIR" <<'PY'
import os, re, sys, glob

recipe_dir = sys.argv[1]

# A step-level claim -- "this STEP is conditional" -- looks different from an
# item-level one -- "cover this bullet if relevant". Only the former is a lie
# when there is no `condition:`; the latter is ordinary prompt guidance.
#
# Step-level, matched:
#     # Step 5c: Database Design (if applicable)      <- markdown heading
#     Skip if no database work is required.           <- standalone sentence
# Item-level, ignored:
#     - API documentation (if applicable)             <- list item
#     3. Authentication issues (if applicable)        <- list item
IF_APPLICABLE = re.compile(r"\(\s*if\s+applicable\s*\)", re.IGNORECASE)
SKIP_SENTENCE = re.compile(
    r"\b(?:skip|omit)\s+(?:this\s+step\s+)?if\b|"
    r"\bnot\s+required\s+for\s+this\s+(?:task|change)\b",
    re.IGNORECASE)

# Prompt body is indented at least 6 spaces; YAML comments and keys sit above
# that, so indentation alone separates prompt text from recipe structure.
PROMPT_INDENT = 6
HEADING = re.compile(r"^\s{%d,}#{1,6}\s" % PROMPT_INDENT)
LIST_ITEM = re.compile(r"^\s{%d,}(?:[-*+]|\d+[.)])\s" % PROMPT_INDENT)
YAML_COMMENT = re.compile(r"^\s{0,%d}#" % (PROMPT_INDENT - 1))


def step_level_claim(text):
    """True when the line claims the STEP is optional, not a list item."""
    if YAML_COMMENT.match(text):
        return False
    if len(text) - len(text.lstrip()) < PROMPT_INDENT:
        return False
    if LIST_ITEM.match(text):
        return False
    if IF_APPLICABLE.search(text) and HEADING.match(text):
        return True
    return bool(SKIP_SENTENCE.search(text))


# Deliberately text-based, not YAML-based: the check must report the exact line
# a human has to edit, and must not depend on a YAML lib being installed.
STEP_START = re.compile(r"^\s{2}-\s+id:\s*[\"']?([A-Za-z0-9_.-]+)")
FIELD = re.compile(r"^\s{4}([a-z_]+):")

violations = []
scanned = 0

for path in sorted(glob.glob(os.path.join(recipe_dir, "*.yaml"))):
    lines = open(path, encoding="utf-8").read().splitlines()
    steps = []
    cur = None
    for i, line in enumerate(lines):
        m = STEP_START.match(line)
        if m:
            if cur:
                cur["end"] = i
                steps.append(cur)
            cur = {"id": m.group(1), "start": i, "end": len(lines),
                   "has_condition": False, "prompt": []}
            continue
        if cur is None:
            continue
        f = FIELD.match(line)
        if f and f.group(1) == "condition":
            cur["has_condition"] = True
        cur["prompt"].append((i, line))
    if cur:
        steps.append(cur)

    for st in steps:
        scanned += 1
        if st["has_condition"]:
            continue
        for lineno, text in st["prompt"]:
            if step_level_claim(text):
                violations.append(
                    (os.path.relpath(path, os.path.dirname(recipe_dir)),
                     lineno + 1, st["id"], text.strip()[:70]))
                break

if not scanned:
    print("error: no recipe steps found -- guard would pass vacuously", file=sys.stderr)
    sys.exit(1)

if violations:
    print(f"FAIL: {len(violations)} step(s) claim to be optional in prose but have no `condition:`\n",
          file=sys.stderr)
    for path, lineno, sid, text in violations:
        print(f"  {path}:{lineno}  step '{sid}'", file=sys.stderr)
        print(f"      {text}", file=sys.stderr)
    print("\nAdd a `condition:` the engine can evaluate before spawning the agent,",
          file=sys.stderr)
    print("or remove the skip language from the prompt. See issue #1334.", file=sys.stderr)
    sys.exit(1)

print(f"OK: {scanned} recipe steps scanned, no prose-only skips")
PY
