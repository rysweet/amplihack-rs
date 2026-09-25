#!/usr/bin/env bash
# Re-measure the skill listing budget (issue #1459) without cargo, in about a
# second, so the number can be checked before a commit and quoted in a PR.
#
# Claude Code keeps the `name` + `description` pair of every installed skill
# resident in every session and budgets that listing at a fraction of the
# context window. Past the limit it SILENTLY truncates the tail to bare names,
# and a skill with no description cannot be matched to a task. On a clean
# install 67 of the 130 bundled skills were reaching the model with no
# description at all.
#
# `i02_bundled_skill_listing_is_under_budget` in
# tests/integration/skill_description_budget_test.rs is the authority; this
# script exists to move the feedback earlier, the same way
# scripts/check-brick-budget.sh does for the brick rule. `I-03` asserts the two
# report the same total, which is why the trimming rule below has to match the
# Rust walk exactly.
#
# The limit is READ FROM the Rust module rather than copied, so this cannot
# drift from the rule it reports.
#
# bash 3.2 compatible (no mapfile, no associative arrays) so it runs on stock
# macOS — see issue #1423.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
BUDGET_RS="$ROOT/crates/amplihack-cli/src/skill_listing_budget.rs"
SKILLS_DIR="$ROOT/amplifier-bundle/skills"

if [[ ! -f "$BUDGET_RS" ]]; then
  echo "ERROR: cannot find $BUDGET_RS — the budget's source of truth" >&2
  exit 1
fi

# `20_000` in Rust; strip the underscores before using it as a number.
limit="$(sed -n 's/^pub const SKILL_LISTING_BUDGET_CHARS: usize = \([0-9_]\+\);.*/\1/p' \
  "$BUDGET_RS" | head -1 | tr -d '_')"

# Validate the SHAPE, not just non-emptiness. Bash `(( ))` evaluates its
# operands as arithmetic expressions, and arithmetic evaluation runs command
# substitution inside an array subscript — an unvalidated `x[$(...)]` scraped
# into this variable would be code execution at pre-commit time.
if [[ ! "$limit" =~ ^[0-9]+$ ]]; then
  echo "ERROR: could not read SKILL_LISTING_BUDGET_CHARS from $BUDGET_RS" >&2
  echo "  The module may have been restructured; this check would pass vacuously." >&2
  exit 1
fi

if [[ ! -d "$SKILLS_DIR" ]]; then
  echo "ERROR: cannot find $SKILLS_DIR" >&2
  exit 1
fi

# POSIX `find` does not descend symlinked directories without -H/-L, and -type f
# excludes symlinked files. This matches the Rust walk, so the two totals agree.
# while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
skill_files=(); while IFS= read -r _f; do skill_files+=("$_f"); done < <(
  find "$SKILLS_DIR" -type f -name 'SKILL.md' | LC_ALL=C sort
)

if (( ${#skill_files[@]} == 0 )); then
  echo "ERROR: found no SKILL.md files under $SKILLS_DIR — this check would pass vacuously" >&2
  exit 1
fi

# One "<chars> <label>" line per skill. awk reads only the frontmatter block and
# counts the trimmed `name` and `description` values in BYTES, matching
# `str::len()` on the trimmed scalar in the Rust walk.
#
# A block scalar (`|` or `>`) description is a HARD ERROR rather than a silent
# miscount: after the #1459 rewrite every bundled description is a single-line
# scalar, so a block scalar means the padded form came back. The Rust library
# deliberately tolerates them, because the tree it measures at install time is
# user-authored.
measure_one() {
  LC_ALL=C awk -v label="$2" '
    function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t\r]+$/, "", s); return s }
    NR == 1 && $0 !~ /^---[\r]?$/ { bad = "no frontmatter"; exit }
    NR == 1 { infm = 1; next }
    infm && /^(---|\.\.\.)[\r]?$/ { infm = 0; exit }
    infm && /^(name|description)[ \t]*:/ {
      key = $0; sub(/[ \t]*:.*$/, "", key)
      val = $0; sub(/^[^:]*:/, "", val); val = trim(val)
      if (val == "|" || val == ">" || val ~ /^[|>][0-9+-]*$/) {
        bad = key " is a block scalar"; exit
      }
      # Strip one layer of matching quotes, as the YAML parser would.
      if (val ~ /^".*"$/ || val ~ /^'"'"'.*'"'"'$/) val = substr(val, 2, length(val) - 2)
      total += length(val)
    }
    END {
      if (bad != "") { printf "ERROR: %s: %s\n", label, bad > "/dev/stderr"; exit 2 }
      printf "%d %s\n", total, label
    }
  ' "$1"
}

# A newline in a skill path would have been split into two bogus records by the
# read loop above. Neither piece names a real file, so this catches it as a hard
# error rather than silently measuring the wrong thing.
lines=""
for f in "${skill_files[@]}"; do
  if [[ ! -f "$f" ]]; then
    echo "ERROR: not a readable file: $f" >&2
    echo "  A newline in a skill path splits the file list; fix the path." >&2
    exit 1
  fi
  label="$(basename "$(dirname "$f")")"
  if ! one="$(measure_one "$f" "$label")"; then
    echo "  Fix the frontmatter above, or the budget cannot be measured." >&2
    exit 1
  fi
  lines="$lines$one
"
done

total=0
while IFS=' ' read -r chars _label; do
  [[ -n "$chars" ]] || continue
  total=$((total + chars))
done <<EOF
$lines
EOF

count=${#skill_files[@]}

if (( total > limit )); then
  echo "skill listing budget EXCEEDED: $total / $limit characters ($count skills, $((total - limit)) over)" >&2
  echo "largest first:" >&2
  printf '%s' "$lines" | LC_ALL=C sort -rn -k1,1 | head -20 \
    | while IFS=' ' read -r chars label; do
        [[ -n "$chars" ]] || continue
        printf '%6s  %s\n' "$chars" "$label" >&2
      done
  cat >&2 <<'MSG'

Claude Code truncates the tail of this listing to bare names, so the skills past
the cut reach the model with no description and cannot be matched to a task.

Shorten the `description` frontmatter of the skills above to about 120 chars,
in the form: <What it does>. Use when <triggers>.
Do NOT raise SKILL_LISTING_BUDGET_CHARS — it already sits below a measured
truncation point.
MSG
  exit 1
fi

echo "skill listing budget: $total / $limit characters ($count skills, $((limit - total)) to spare)"
exit 0
