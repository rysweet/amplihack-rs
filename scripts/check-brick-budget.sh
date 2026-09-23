#!/usr/bin/env bash
# Fail a phase sub-recipe that exceeds the brick line budget — locally, in about
# a second, instead of after a full CI cycle.
#
# `every_phase_subrecipe_under_400_lines` in
# tests/integration/default_workflow_decomposition_test.rs already enforces this.
# It is the authority; this script exists only to move the feedback earlier.
# Three PRs in one day hit the limit and each learned about it roughly thirteen
# minutes later, from a `Test` job that had to compile the workspace first.
#
# These files sit at or near the limit by design, so there is effectively no
# headroom and any inline addition trips it. That is the rule working: a full
# brick means extract to `amplifier-bundle/tools/` and call out, not compress
# the file until the counter is satisfied.
#
# The recipe list and the limit are READ FROM the Rust test rather than copied,
# so this cannot drift from the rule it reports.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
TEST_FILE="$ROOT/tests/integration/default_workflow_decomposition_test.rs"
RECIPE_DIR="$ROOT/amplifier-bundle/recipes"

if [[ ! -f "$TEST_FILE" ]]; then
  echo "ERROR: cannot find $TEST_FILE — the brick rule's source of truth" >&2
  exit 1
fi

limit="$(sed -n 's/^const BRICK_LIMIT: usize = \([0-9]\+\);.*/\1/p' "$TEST_FILE" | head -1)"
if [[ -z "$limit" ]]; then
  echo "ERROR: could not read BRICK_LIMIT from $TEST_FILE" >&2
  echo "  The test may have been restructured; this check would pass vacuously." >&2
  exit 1
fi

# Names between `const PHASE_RECIPES: &[&str] = &[` and its closing `];`.
# while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
recipes=(); while IFS= read -r _ml; do recipes+=("$_ml"); done < <(
  awk '/^const PHASE_RECIPES: &\[&str\] = &\[/{f=1; next} f && /^\];/{exit} f' "$TEST_FILE" \
    | sed -n 's/.*"\([^"]\+\)".*/\1/p'
)

# `default-workflow` is checked too: the test chains it onto PHASE_RECIPES.
recipes+=("default-workflow")

if (( ${#recipes[@]} <= 1 )); then
  echo "ERROR: parsed no PHASE_RECIPES from $TEST_FILE — this check would pass vacuously" >&2
  exit 1
fi

violations=0
for name in "${recipes[@]}"; do
  f="$RECIPE_DIR/$name.yaml"
  [[ -f "$f" ]] || continue
  lines="$(wc -l < "$f" | tr -d ' ')"
  if (( lines >= limit )); then
    printf 'brick rule violation: %-32s %s lines (must be < %s)\n' \
      "amplifier-bundle/recipes/$name.yaml" "$lines" "$limit" >&2
    violations=$((violations + 1))
  fi
done

if (( violations > 0 )); then
  cat >&2 <<'MSG'

A phase sub-recipe is over its line budget. These files are kept at or near the
limit deliberately, so an inline addition will usually trip it.

Do NOT compress comments to fit. Either:
  * extract the logic into amplifier-bundle/tools/<name>.sh and call it from the
    recipe, resolved via the AMPLIHACK_HOME / REPO_PATH / cwd / ~/.copilot /
    ~/.amplihack cascade (see autodrive-build.yaml). This also makes it directly
    testable from a shell test; or
  * fold it into an adjacent block the new code subsumes, at equal line count.
MSG
  exit 1
fi

exit 0
