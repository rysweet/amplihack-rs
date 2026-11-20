#!/usr/bin/env bash
# Regression tests for issue #1680 — a fresh auto-drive run (no pr_number, no
# existing_branch) kept its whole state under a key made from the base
# checkout's branch (…_mistt-ui-platform_main), so two fresh runs in one
# repository at once shared verdict files.
#
# Issue #2329 retired the bash bodies of autodrive-build's step-01 and step-03:
# both are agent steps now and the keying duty is stated in their prompts.
# This test pins that prompt contract:
#   1. Shape: step-01 and step-03 are agent steps with outputs build_preflight
#      and build_result; neither carries a command body; build_result carries state_dir.
#   2. State is keyed by full host/repository/PR identity, never by a branch
#      name alone (two fresh runs that open different PRs therefore get
#      different directories).
#   3. Step-03 keys a fresh run's state by the PR it resolved, the directory a
#      later run given that PR number resolves to. The fresh-run build decision
#      is covered by crates/amplihack-cli/tests/issue_2552_build_preflight_decision.rs.
#   4. An explicit autodrive_state_dir is kept.
#   5. State is private and outside the worktree (step-03); historical state never
#      establishes current readiness.
#
# Run: bash tests/issue_1680_fresh_run_state_key.sh
set -uo pipefail
# Issue #1764: no git this script runs may wait on a credential prompt.
export GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=true SSH_ASKPASS=true
exec </dev/null
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$REPO_ROOT/amplifier-bundle/recipes/autodrive-build.yaml"
pass=0; fail=0
ok()  { pass=$((pass + 1)); echo "PASS: $1"; }
bad() { fail=$((fail + 1)); echo "FAIL: $1 -- $2"; }

[ -f "$BUILD" ] || { echo "FAIL: missing $BUILD"; exit 1; }

step_block() {
  awk -v id="  - id: \"$1\"" '
    $0 == id { s=1; print; next }
    s && /^  - id: / { exit }
    s && /^[a-z]/ { exit }
    s { print }' "$BUILD"
}
step_prompt() {
  step_block "$1" | awk '
    /^    prompt: \|/ { p=1; next }
    p { if ($0 ~ /^    [a-zA-Z_]+:/) exit; sub(/^      /, ""); print }' | tr '\n' ' '
}

S01_BLOCK="$(step_block step-01-build-preflight)"; S01="$(step_prompt step-01-build-preflight)"
S03_BLOCK="$(step_block step-03-resolve-pr)";      S03="$(step_prompt step-03-resolve-pr)"
[ -n "$S01" ] && [ -n "$S03" ] || { echo "FAIL: step-01-build-preflight / step-03-resolve-pr prompt missing"; exit 1; }

# has <name> <step-label> <prompt> <clause>
has() { case "$3" in *"$4"*) ok "$2: $1" ;; *) bad "$2: $1" "prompt lacks: $4" ;; esac; }
both() { has "$1" step-01 "$S01" "$2"; has "$1" step-03 "$S03" "$2"; }

# 1. Shape.
for pair in "step-01:build_preflight" "step-03:build_result"; do
  label="${pair%%:*}"; out="${pair#*:}"
  if [ "$label" = step-01 ]; then blk="$S01_BLOCK"; else blk="$S03_BLOCK"; fi
  if printf '%s\n' "$blk" | grep -qE '^    type: "agent"$' \
     && printf '%s\n' "$blk" | grep -qE "^    output: \"$out\"\$" \
     && ! printf '%s\n' "$blk" | grep -qE '^    command: '; then
    ok "$label is an agent step with output $out and no bash body"
  else
    bad "$label shape" "expected type: agent, output: $out, no command:"
  fi
done
has "build_result carries state_dir" step-03 "$S03" 'Return build_result as JSON with pr, pr_url, branch, state_dir'

# 2. Keyed by host, repository and PR; never a branch alone.
both "state is keyed by full host/repository/PR identity" \
  "otherwise use a collision-resistant key of full host/repository/PR identity under the configured durable work root"
both "state is never keyed by a branch name alone" \
  "never a key made from a branch name alone such as the launch checkout's main (issue #1680)"

# 3. Fresh run: step-03 keys state by the PR it resolved.
has "a fresh run's state is keyed by the PR step-03 resolved" step-03 "$S03" \
  "State for such a fresh run is keyed by the PR this step resolved, the same directory a later run given that PR number resolves to, never the preflight's branch-keyed location (issue #1680)"

# 4. Explicit state dir.
both "an explicit autodrive_state_dir is kept" \
  "Prefer the provided state dir {{autodrive_state_dir}}"

# 5. Private, outside the worktree, never evidence of readiness.
has "state is private and outside the worktree" step-03 "$S03" \
  'Create private state outside the worktree using `amplihack orch helper autodrive state --directory <absolute-path>`'
both "historical state never establishes readiness" \
  "Existing historical evidence never establishes current readiness."
both "state identity is never changed to escape a budget" \
  "change state identity to escape a budget"

echo "issue #1680 fresh-run state key: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
