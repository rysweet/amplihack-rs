#!/usr/bin/env bash
# Static TDD tests for workflow recipe commit identity safety.
#
# Every amplihack-created commit path must source the shared identity helper and
# call amplihack_prepare_git_commit_identity immediately before git commit.
# Expected before implementation: FAIL. Expected after implementation: PASS.
#
# Run: bash tests/recipe_commit_identity_static_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPES_DIR="$REPO_ROOT/amplifier-bundle/recipes"
HELPER="$REPO_ROOT/amplifier-bundle/tools/git-identity.sh"

pass=0
fail=0

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    if [ $# -gt 1 ] && [ -n "$2" ]; then
        printf '      %s\n' "$2"
    fi
    fail=$((fail + 1))
}

assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        record_pass "$desc"
    else
        record_fail "$desc" "condition: $cond"
    fi
}

commit_command_lines() {
    local file="$1"
    grep -nE '(^|[;&|[:space:]])(PRE_COMMIT_ALLOW_NO_CONFIG=1[[:space:]]+)?git[[:space:]]+commit([[:space:]]|$)' "$file" \
        | grep -vE '^[0-9]+:[[:space:]]*echo .*git commit|git commit failed during|git commit message|git commit messages'
}

commit_recipe_files() {
    local file
    find "$RECIPES_DIR" -name '*.yaml' -type f | sort | while IFS= read -r file; do
        if [ -n "$(commit_command_lines "$file")" ]; then
            printf '%s\n' "$file"
        fi
    done
}

has_prepare_immediately_before_commit() {
    local file="$1"
    local line="$2"
    local start=$((line - 8))
    if [ "$start" -lt 1 ]; then
        start=1
    fi
    sed -n "${start},${line}p" "$file" | grep -q 'amplihack_prepare_git_commit_identity'
}

echo "=== Recipe commit identity static TDD tests ==="
echo "Recipes: $RECIPES_DIR"
echo "Helper:  $HELPER"
echo

assert "shared git identity helper exists" "[ -f '$HELPER' ]"

expected_files=(
    "$RECIPES_DIR/workflow-publish.yaml"
    "$RECIPES_DIR/workflow-finalize.yaml"
    "$RECIPES_DIR/workflow-pr-review.yaml"
    "$RECIPES_DIR/consensus-publish.yaml"
)

for file in "${expected_files[@]}"; do
    assert "$(basename "$file") exists" "[ -f '$file' ]"
done

mapfile -t files < <(commit_recipe_files)
if [ "${#files[@]}" -gt 0 ]; then
    record_pass "found commit-producing recipe files"
else
    record_fail "found commit-producing recipe files" "no git commit commands found under $RECIPES_DIR"
fi

for file in "${files[@]}"; do
    name="$(basename "$file")"

    assert "$name sources git-identity.sh" \
        "grep -q 'git-identity\.sh' '$file'"

    assert "$name calls amplihack_prepare_git_commit_identity" \
        "grep -q 'amplihack_prepare_git_commit_identity' '$file'"

    assert "$name does not silence git commit failures as success" \
        "! grep -Eq 'git[[:space:]]+commit.*2>/dev/null[[:space:]]*\\|\\|[[:space:]]*echo' '$file'"

    while IFS=: read -r line_no line_text; do
        [ -n "$line_no" ] || continue
        if has_prepare_immediately_before_commit "$file" "$line_no"; then
            record_pass "$name line $line_no prepares identity before git commit"
        else
            record_fail "$name line $line_no prepares identity before git commit" "$line_text"
        fi
    done < <(commit_command_lines "$file")
done

if command -v python3 >/dev/null 2>&1; then
    if python3 - <<'PY' "$RECIPES_DIR"
import pathlib
import sys

try:
    import yaml
except Exception:
    sys.exit(2)

recipes = pathlib.Path(sys.argv[1])
for path in recipes.glob("*.yaml"):
    with path.open("r", encoding="utf-8") as fh:
        yaml.safe_load(fh)
PY
    then
        record_pass "all recipe YAML files parse"
    else
        rc=$?
        if [ "$rc" = "2" ]; then
            record_pass "PyYAML unavailable; YAML parse check skipped"
        else
            record_fail "all recipe YAML files parse" "python yaml.safe_load failed"
        fi
    fi
else
    record_pass "python3 unavailable; YAML parse check skipped"
fi

echo
echo "=== Results: $pass passed, $fail failed ==="
if [ "$fail" -ne 0 ]; then
    exit 1
fi
