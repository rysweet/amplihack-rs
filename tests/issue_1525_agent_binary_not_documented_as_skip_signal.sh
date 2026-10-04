#!/usr/bin/env bash
# Issue #1525: no shipped text may list AMPLIHACK_AGENT_BINARY among the
# signals that skip the startup update check or make a launch subprocess-safe.
#
# #625 made it one; #1525 removed it from `classify_skip_reason` and
# `resolve_subprocess_safe`, because it names an agent CLI, and a user who
# exports it to choose one is still at a terminal. The crusty review of #1490
# then found five places still saying otherwise, among them the reference page
# an operator reads before exporting a host-wide default. Each listed the
# variable next to `AMPLIHACK_NONINTERACTIVE` or `CI`:
#
#   subprocess-safe skip signals (`CI`, `AMPLIHACK_AGENT_BINARY`, ...)
#   when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set
#   any of NONINTERACTIVE / AGENT_BINARY (non-empty) / CI (non-empty)
#
# This scan finds that shape: AGENT_BINARY and NONINTERACTIVE or CI side by
# side in a list (separated by `/`, `,`, "or" or "and"), within two adjacent
# lines, in Markdown and in Rust comments. To describe the old behaviour, do
# not write it as such a list.

set -uo pipefail

[ -d docs ] && [ -d crates ] || { echo "run from the repo root"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# scan <file...>: print file:line: text for each such list. Lines are read in
# pairs so that a list broken across two lines is still seen; a leading `>`,
# `//`, `///` or `//!` is dropped first. In Rust files only comments count.
scan() {
  awk '
    function bare(l) {
      sub(/^[[:space:]]*(\/\/[\/!]?|>)[[:space:]]?/, "", l)
      return l
    }
    BEGIN {
      sep = "[`*[:space:]]*(/|,|or|and)[`*[:space:]]*"
      ab = "(AMPLIHACK_)?AGENT_BINARY`?([[:space:]]*\\(non-empty\\))?"
      other = "((AMPLIHACK_)?NONINTERACTIVE|CI)"
      listed = "(" other "`?" sep ab ")|(" ab sep "`?" other "([^A-Za-z_]|$))"
    }
    FNR == 1 { prev = ""; last = -1 }
    {
      if (FILENAME ~ /\.rs$/ && $0 !~ /^[[:space:]]*\/\//) { prev = ""; next }
      cur = bare($0)
      if ((prev " " cur) ~ listed && last != FNR - 1) {
        print "    " FILENAME ":" FNR ": " cur
        last = FNR
      }
      prev = cur
    }' "$@"
}

# --- the scan finds the shapes the review found ---------------------------
samples="$(mktemp -d)"
trap 'rm -rf "$samples"' EXIT
cat > "$samples/stale.md" <<'EOF'
Unlike
the subprocess-safe skip signals (`CI`, `AMPLIHACK_AGENT_BINARY`,
`AMPLIHACK_NONINTERACTIVE`, `--subprocess-safe`, non-TTY stdin), this variable

- How `CI`, `AMPLIHACK_AGENT_BINARY`, `AMPLIHACK_NONINTERACTIVE`, and non-TTY stdin each suppress the prompt

- the prompt skips when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set

> and how it skips automatically, or when `AMPLIHACK_NONINTERACTIVE` /
> `AMPLIHACK_AGENT_BINARY` / `CI` is set, see the feature page
EOF
cat > "$samples/stale.rs" <<'EOF'
pub(super) enum SkipReason {
    /// Subprocess-safe skip: any of NONINTERACTIVE / AGENT_BINARY (non-empty)
    /// / CI (non-empty) / `--subprocess-safe` argv token.
    SubprocessSafe,
}
EOF
cat > "$samples/fine.md" <<'EOF'
| `AMPLIHACK_NONINTERACTIVE` | If `=1` → triggers subprocess-safe context. |
| `AMPLIHACK_AGENT_BINARY` | **No effect** on subprocess-safe (#1525). |

`AMPLIHACK_AGENT_BINARY` is **not** a signal. Set `AMPLIHACK_NONINTERACTIVE=1`
or pass `--subprocess-safe` instead.
EOF
cat > "$samples/fine.rs" <<'EOF'
fn f() {
    let s = "printf '%s' \"$AMPLIHACK_AGENT_BINARY|$AMPLIHACK_NONINTERACTIVE\"";
}
EOF
found="$(scan "$samples/stale.md" "$samples/stale.rs" | wc -l | tr -d ' ')"
if [ "$found" -eq 5 ]; then
  pass "the scan finds each of the 5 stale forms"
else
  fail "the scan found $found of the 5 stale forms:
$(scan "$samples/stale.md" "$samples/stale.rs")"
fi
false_hits="$(scan "$samples/fine.md" "$samples/fine.rs")"
if [ -z "$false_hits" ]; then
  pass "the scan passes over a table, a correction and code"
else
  fail "the scan flagged text that is correct:
$false_hits"
fi

# --- shipped text ---------------------------------------------------------
shipped=()
while IFS= read -r file; do
  shipped+=("$file")
done < <(git ls-files '*.md' 'crates/*.rs' 'bins/*.rs')
[ "${#shipped[@]}" -gt 0 ] || { echo "  FAIL  no shipped files found (is this a git checkout?)"; exit 1; }
offenders="$(scan "${shipped[@]}")"
if [ -z "$offenders" ]; then
  pass "no shipped text lists AMPLIHACK_AGENT_BINARY as a skip signal (${#shipped[@]} files)"
else
  fail "shipped text lists AMPLIHACK_AGENT_BINARY as an update-check or subprocess-safe signal, which it has not been since #1525:
$offenders"
fi

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 AMPLIHACK_AGENT_BINARY is not a skip signal: $fails failure(s)"
  exit 1
fi
echo "issue #1525 AMPLIHACK_AGENT_BINARY is not a skip signal: all checks passed"
