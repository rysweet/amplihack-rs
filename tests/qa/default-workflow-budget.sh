#!/usr/bin/env bash
# Execute the real recipe topology with harmless leaves to test the global budget.
set -euo pipefail
: "${RECIPE_RUNNER_QA_BINARY:?set RECIPE_RUNNER_QA_BINARY to the recipe runner candidate}"
runner=$(realpath "$RECIPE_RUNNER_QA_BINARY")
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/amplihack-budget-qa.XXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/recipes" "$fixture/home"
render() {
    local name=$1 source="$repo/amplifier-bundle/recipes/$1.yaml" cap child
    [[ "$name" =~ ^[a-z0-9-]+$ ]]
    test ! -f "$fixture/recipes/$name.yaml" || return 0
    cap=$(awk '$1 == "max_total_steps:" { print $2; exit }' "$source")
    [[ "$cap" =~ ^[0-9]+$ ]]
    awk -v name="$name" -v cap="$cap" '
        function flush() {
            if (!active) return
            print "  - id: \"qa-" count "\""
            if (child != "") {
                print "    type: recipe"
                print "    recipe: \"" child "\""
            } else {
                print "    type: bash"
                print "    command: printf '\''leaf\\n'\'' >> \"$QA_STEP_LOG\""
            }
        }
        BEGIN {
            print "name: \"" name "\"\nversion: \"1.0.0\""
            print "recursion:\n  max_depth: 6\n  max_total_steps: " cap
            print "context: {}\nsteps:"
        }
        /^  - id:/ { flush(); active=1; count++; child="" }
        /^    recipe:/ { child=$2; gsub(/"/, "", child) }
        END { flush() }
    ' "$source" > "$fixture/recipes/$name.yaml"
    while IFS= read -r child; do
        render "$child"
    done < <(awk '/^    recipe:/ { gsub(/"/, "", $2); print $2 }' "$source")
}
render default-workflow
expected=0
total=0
count_leaves() {
    local name=$1 depth=$2 direct child
    test "$depth" -lt 12
    direct=$(awk '/^    type: bash$/ { count++ } END { print count+0 }' "$fixture/recipes/$name.yaml")
    expected=$((expected + direct))
    total=$((total + direct))
    while IFS= read -r child; do
        total=$((total + 1))
        count_leaves "$child" "$((depth + 1))"
    done < <(awk '/^    recipe:/ { gsub(/"/, "", $2); print $2 }' "$fixture/recipes/$name.yaml")
}
count_leaves default-workflow 0
test "$total" -gt 80
run() {
    env -i HOME="$fixture/home" PATH=/usr/bin:/bin QA_STEP_LOG="$fixture/steps" \
        "$runner" "$fixture/recipes/default-workflow.yaml" \
        --working-dir "$fixture" --recipe-dir "$fixture/recipes" \
        --no-auto-stage --output-format json
}
run > "$fixture/pass.log" 2>&1 || { cat "$fixture/pass.log"; exit 1; }
test "$(wc -l < "$fixture/steps")" -eq "$expected"
# Negative control proves the old budget fails in the actual execution engine.
sed -i 's/max_total_steps: [0-9]*/max_total_steps: 80/' "$fixture/recipes/default-workflow.yaml"
rm "$fixture/steps"
if run > "$fixture/fail.log" 2>&1; then
    echo 'FAIL: old 80-step budget unexpectedly completed' >&2
    exit 1
fi
rg -qi '(max.*(total|step)|step.*(limit|exceed|budget))' "$fixture/fail.log"
test "$(wc -l < "$fixture/steps")" -lt "$expected"
printf 'PASS: current default-workflow budget completes %s total steps (%s leaves); old 80-step budget fails\n' "$total" "$expected"
