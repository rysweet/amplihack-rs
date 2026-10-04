#!/usr/bin/env bash
# Offline integration fixtures for the neutrality guard. No Python test assets.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
fixture="$(mktemp -d "${TMPDIR:?Set TMPDIR to the evidence directory}/neutrality.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/amplifier-bundle/recipes/tests" "$fixture/docs/claude"
cp -a "$repo_root/amplifier-bundle/skills" "$fixture/amplifier-bundle/"
cp -a "$repo_root/docs/claude/skills" "$fixture/docs/claude/"
cp "$repo_root/amplifier-bundle/recipes/tests/"{test-skill-model-neutrality.sh,skill-model-neutrality-audit.md} "$fixture/amplifier-bundle/recipes/tests/"
guard="$fixture/amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh"
output="$fixture/output.log"
reject() {
    if (cd "$TMPDIR" && bash "$guard") >"$output" 2>&1; then
        echo "FAIL: guard accepted fixture: $1" >&2
        exit 1
    fi
    if ! grep -Fq "$1" "$output"; then
        cat "$output" >&2
        exit 1
    fi
}
(cd "$TMPDIR" && bash "$guard") >"$output" 2>&1
grep -Fq '130 skills;' "$output"
for tree in amplifier-bundle/skills docs/claude/skills; do
    path="$fixture/$tree/poet-analyst/regression.md"
    printf 'Haiku is a poetic form.\nmodel: sonnet\n' >"$path"
    reject "$tree/poet-analyst/regression.md:2:"
    rm "$path"
done
path="$fixture/amplifier-bundle/skills/regression/SKILL.md"
mkdir -p "$(dirname "$path")"
printf '%s\n' '---' 'name: regression' 'model: runtime-default' '---' >"$path"
reject 'execution model override in frontmatter'
grep -Fq 'Expected 130 skills; scanned 131' "$output"
rm -r "$(dirname "$path")"
path="$fixture/amplifier-bundle/skills/poet-analyst/regression.md"
for line in 'Recommended model: Claude' 'default_model = "GPT"' \
    'Use the Claude model for this skill' 'primary_model: sonnet' \
    'fallback_model = "haiku"' 'run --model claude' '/model gpt'; do
    printf '%s\n' "$line" >"$path"
    reject 'amplifier-bundle/skills/poet-analyst/regression.md:1:'
done
rm "$path"
original="$fixture/amplifier-bundle/skills/poet-analyst/SKILL.md"
mv "$original" "${original%/*}/RENAMED.md"
mkdir "$fixture/amplifier-bundle/skills/unreviewed"
printf '%s\n' '---' 'name: unreviewed' '---' >"$fixture/amplifier-bundle/skills/unreviewed/SKILL.md"
reject 'Reviewed skill inventory changed'
mv "${original%/*}/RENAMED.md" "$original"
rm -r "$fixture/amplifier-bundle/skills/unreviewed"
path="$fixture/docs/claude/skills/microsoft-agent-framework/examples/04-basic-agent.cs"
sed -i 's/string.IsNullOrWhiteSpace(value)/value == null/g' "$path"
reject 'missing blank configuration rejection'
echo 'PASS: six neutrality integration scenarios, including seven provider recommendation fixtures.'
