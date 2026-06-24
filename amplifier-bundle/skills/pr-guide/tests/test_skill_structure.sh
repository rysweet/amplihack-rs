#!/usr/bin/env bash
# TDD contract tests for the `pr-guide` skill — structural validation.
#
# Run: bash amplifier-bundle/skills/pr-guide/tests/test_skill_structure.sh
#
# These tests are the executable specification for the skill. They encode every
# requirement from the design (frontmatter scope, triviality filter, the fixed
# 5-section document contract, dual-platform support, GUI/TUI capture, deep
# links, temp-file output, input resolution, and the security mandate). They are
# self-contained (no network, no build) and follow the crusty-old-engineer /
# code-philosophy test pattern already used in this repo.
#
# Written test-first: each assertion describes the behavior the SKILL.md +
# reference.md must specify. A failing line names exactly which part of the
# contract is unmet.

set -uo pipefail

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
SKILL_FILE="$SKILL_DIR/SKILL.md"
REFERENCE_FILE="$SKILL_DIR/reference.md"

# Combined corpus lets a requirement be satisfied by either file (SKILL.md is
# the index, reference.md is the detail). Individual tests still target the
# correct file where the spec mandates a specific location.
BOTH_FILES=("$SKILL_FILE" "$REFERENCE_FILE")

# grep across whichever of the two files exist.
grep_both() { grep -qiE "$1" "${BOTH_FILES[@]}" 2>/dev/null; }
# case-sensitive variant
grep_both_cs() { grep -qE "$1" "${BOTH_FILES[@]}" 2>/dev/null; }

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: pr-guide skill — Structure"
echo "═══════════════════════════════════════════════════════"

# ─── Test 1: Required files exist ────────────────────────────────────────────

echo ""
echo "Test 1: Required files exist"

if [[ -f "$SKILL_FILE" ]]; then
  pass "SKILL.md exists"
else
  fail "SKILL.md not found at $SKILL_FILE"
  echo "  (Cannot run remaining tests without SKILL.md)"
  echo ""
  echo "═══════════════════════════════"
  echo "Results: $PASS passed, $FAIL failed"
  echo "═══════════════════════════════"
  exit 1
fi

if [[ -f "$REFERENCE_FILE" ]]; then
  pass "reference.md exists"
else
  fail "reference.md not found at $REFERENCE_FILE"
fi

# ─── Test 2: Frontmatter — name + description ONLY ──────────────────────────
# Acceptance criterion: frontmatter must contain exactly `name` and
# `description` (no version/invokes/token_budget). Matches skill-builder and
# mermaid-diagram-generator conventions.

echo ""
echo "Test 2: YAML frontmatter is exactly name + description"

if [[ "$(head -1 "$SKILL_FILE")" == "---" ]]; then
  pass "frontmatter starts with --- on the first byte"
else
  fail "frontmatter: first line must be '---'"
fi

DELIM_COUNT=$(grep -c "^---$" "$SKILL_FILE" || true)
if [[ "$DELIM_COUNT" -ge 2 ]]; then
  pass "frontmatter has opening and closing --- delimiters"
else
  fail "frontmatter missing closing --- delimiter (found $DELIM_COUNT)"
fi

# Extract just the frontmatter block (between the first two --- lines).
FRONTMATTER=$(awk 'NR==1 && $0=="---"{f=1; next} f && $0=="---"{exit} f{print}' "$SKILL_FILE")

if echo "$FRONTMATTER" | grep -qE "^name:[[:space:]]*pr-guide[[:space:]]*$"; then
  pass "frontmatter: name is 'pr-guide'"
else
  fail "frontmatter: name must be exactly 'pr-guide'"
fi

if echo "$FRONTMATTER" | grep -qE "^description:"; then
  pass "frontmatter: description field present"
else
  fail "frontmatter: description field missing"
fi

# Collect top-level frontmatter keys (lines like `key:` not indented).
TOP_KEYS=$(echo "$FRONTMATTER" | grep -oE "^[a-zA-Z_][a-zA-Z0-9_-]*:" | sed 's/:$//' | sort -u)
EXPECTED_KEYS=$'description\nname'
if [[ "$TOP_KEYS" == "$EXPECTED_KEYS" ]]; then
  pass "frontmatter has ONLY name + description (no extra keys)"
else
  fail "frontmatter must contain only name+description; found keys: $(echo "$TOP_KEYS" | tr '\n' ' ')"
fi

for forbidden in version invokes token_budget activation_keywords; do
  if echo "$FRONTMATTER" | grep -qE "^${forbidden}:"; then
    fail "frontmatter must NOT contain '$forbidden' (locked to name+description)"
  else
    pass "frontmatter correctly omits '$forbidden'"
  fi
done

# ─── Test 3: Description — third person + trigger keywords ───────────────────

echo ""
echo "Test 3: Description is third-person and carries trigger keywords"

DESC=$(echo "$FRONTMATTER" | sed -n 's/^description:[[:space:]]*//p')

# Third person: should start with a verb like "Generates"/"Creates"/"Produces",
# not "I", "You", or an imperative addressed to the reader.
if echo "$DESC" | grep -qE "^(Generates|Creates|Produces|Builds|Turns|Writes)"; then
  pass "description is written in the third person"
else
  fail "description should be third person (e.g. 'Generates an illustrated...')"
fi

for kw in "PR" "pull request" "illustrated guide" "walkthrough"; do
  if echo "$DESC" | grep -qiF "$kw"; then
    pass "description contains trigger keyword: '$kw'"
  else
    fail "description missing required trigger keyword: '$kw'"
  fi
done

# Must mention both supported platforms so discovery works for either.
if echo "$DESC" | grep -qiE "GitHub" && echo "$DESC" | grep -qiE "Azure DevOps"; then
  pass "description mentions both GitHub and Azure DevOps"
else
  fail "description should mention both GitHub and Azure DevOps"
fi

# ─── Test 4: SKILL.md size — index, not a novel ─────────────────────────────

echo ""
echo "Test 4: SKILL.md stays under the 500-line budget"

SKILL_LINES=$(wc -l <"$SKILL_FILE")
if [[ "$SKILL_LINES" -le 500 ]]; then
  pass "SKILL.md is $SKILL_LINES lines (<= 500)"
else
  fail "SKILL.md is $SKILL_LINES lines — exceeds the 500-line limit"
fi

if [[ "$SKILL_LINES" -ge 40 ]]; then
  pass "SKILL.md is substantive ($SKILL_LINES lines)"
else
  fail "SKILL.md is only $SKILL_LINES lines — likely incomplete"
fi

if [[ -f "$REFERENCE_FILE" ]]; then
  REF_LINES=$(wc -l <"$REFERENCE_FILE")
  if [[ "$REF_LINES" -ge 100 ]]; then
    pass "reference.md is substantive ($REF_LINES lines)"
  else
    fail "reference.md is only $REF_LINES lines — likely incomplete"
  fi
fi

# ─── Test 5: Triviality filter (skip + reason) ──────────────────────────────

echo ""
echo "Test 5: Triviality filter rules are specified"

# Threshold: fewer than 3 files changed.
if grep_both "3" && grep_both "files? changed"; then
  pass "documents the <3-files threshold"
else
  fail "must document the 'fewer than 3 files changed' threshold"
fi

# Threshold: fewer than ~30 meaningful lines.
if grep_both "30" && grep_both "meaningful"; then
  pass "documents the ~30 meaningful-lines threshold"
else
  fail "must document the '~30 meaningful lines' threshold"
fi

# Config/typo only.
if grep_both "config" && grep_both "typo"; then
  pass "documents the config/typo-only skip rule"
else
  fail "must document skipping config-only / typo-only changes"
fi

# Skip must EMIT a reason, not silently drop.
if grep_both "skip" && grep_both "reason"; then
  pass "skip path emits a reason"
else
  fail "skipping must emit a brief reason (do not skip silently)"
fi

# ─── Test 6: The fixed 5-section document contract ──────────────────────────

echo ""
echo "Test 6: Document defines the five required sections in order"

declare -a SECTIONS=(
  "Problem Statement"
  "Approach Overview"
  "Detailed Walkthrough"
  "Key Decisions & Trade-offs|Key Decisions and Trade-offs"
  "Testing"
)
for sec in "${SECTIONS[@]}"; do
  if grep_both "$sec"; then
    pass "section documented: '${sec%%|*}'"
  else
    fail "missing required section: '${sec%%|*}'"
  fi
done

# Sections must appear in the mandated order within SKILL.md's structure block.
# Match the bold section labels in the body (e.g. **Problem Statement**) so the
# frontmatter description prose does not interfere with the ordering check.
ORDER_OK=1
LAST=0
for label in "Problem Statement" "Approach Overview" "Detailed Walkthrough" "Key Decisions" "Testing"; do
  N=$(grep -nF "**${label}" "$SKILL_FILE" | head -1 | cut -d: -f1)
  if [[ -z "$N" || "$N" -lt "$LAST" ]]; then
    ORDER_OK=0
    break
  fi
  LAST="$N"
done
if [[ "$ORDER_OK" -eq 1 ]]; then
  pass "the five sections appear in the required order in SKILL.md"
else
  fail "the five sections must appear in the fixed order in SKILL.md"
fi

# ─── Test 7: Detailed Walkthrough requirements ──────────────────────────────

echo ""
echo "Test 7: Detailed Walkthrough sub-requirements"

if grep_both "exemplar"; then
  pass "uses exemplar snippets (one per repeated pattern, not exhaustive)"
else
  fail "must specify exemplar snippets rather than listing every identical change"
fi

if grep_both "mermaid"; then
  pass "uses mermaid diagrams for complex flows/architecture"
else
  fail "must mention mermaid diagrams for complex flows"
fi

if grep_both "configurable" || grep_both "constant"; then
  pass "highlights configurable constants / important defaults"
else
  fail "must call out configurable constants and important defaults"
fi

if grep_both "deep link" || grep_both "diff"; then
  pass "links to relevant diffs"
else
  fail "must provide deep links to relevant diffs"
fi

# ─── Test 8: Platform support — GitHub + Azure DevOps ───────────────────────

echo ""
echo "Test 8: Dual-platform support (GitHub + Azure DevOps)"

if grep_both "git remote get-url" || grep_both "remote .*origin"; then
  pass "detects platform from the git remote URL"
else
  fail "must detect platform from 'git remote get-url origin'"
fi

# GitHub via gh CLI.
if grep_both "gh pr view"; then
  pass "GitHub PR data via 'gh pr view'"
else
  fail "must use 'gh' CLI (gh pr view) for GitHub PR data"
fi

# Azure DevOps via az repos.
if grep_both "az repos"; then
  pass "Azure DevOps PR data via 'az repos'"
else
  fail "must use 'az repos' CLI for Azure DevOps PR data"
fi

# GitHub deep-link format with #diff anchor and /files fallback.
if grep_both "github.com" && grep_both "/pull/" && grep_both "/files" && grep_both "#diff-"; then
  pass "GitHub deep links use /pull/N/files#diff-<hash> format"
else
  fail "must build GitHub deep links as github.com/.../pull/N/files#diff-<hash>"
fi

if grep_both "fallback" && grep_both "/files"; then
  pass "GitHub anchor falls back to /files when hash is unavailable"
else
  fail "must document /files fallback for the diff anchor"
fi

# Azure DevOps deep-link format.
if grep_both "dev.azure.com" && grep_both "_a=files" && grep_both "path="; then
  pass "Azure DevOps deep links use ?_a=files&path= format"
else
  fail "must build ADO deep links as dev.azure.com/.../pullrequest/N?_a=files&path=PATH"
fi

# ─── Test 9: GUI/TUI detection + screenshots + fallback ─────────────────────

echo ""
echo "Test 9: GUI/TUI handling"

UI_EXTS=0
for ext in "tsx" "jsx" "vue" "svelte"; do
  grep_both "\\.${ext}" && UI_EXTS=$((UI_EXTS + 1))
done
if [[ "$UI_EXTS" -ge 4 ]]; then
  pass "detects UI changes via .tsx/.jsx/.vue/.svelte"
else
  fail "must detect UI changes from .tsx/.jsx/.vue/.svelte file extensions"
fi

if grep_both "css"; then
  pass "detects CSS changes"
else
  fail "must detect CSS changes as a UI signal"
fi

if grep_both "playwright"; then
  pass "attempts Playwright for screenshot capture"
else
  fail "must attempt Playwright (or similar) for screenshots"
fi

if grep_both "screenshot"; then
  pass "embeds screenshots in the document"
else
  fail "must embed screenshots when UI changes are present"
fi

# Graceful fallback when capture is unavailable.
if (grep_both "fallback" || grep_both "graceful" || grep_both "degrade") && grep_both "textual|describe.*ui|text"; then
  pass "falls back to a textual UI description when capture is unavailable"
else
  fail "must fall back gracefully to a textual UI description"
fi

# ─── Test 10: Output — temp file, path, offer to post ───────────────────────

echo ""
echo "Test 10: Output handling"

if grep_both "temp" && (grep_both "TMPDIR" || grep_both "/tmp"); then
  pass "writes the document to an OS temp file"
else
  fail "must write output to an OS temp file (\$TMPDIR / /tmp)"
fi

if grep_both "0600"; then
  pass "temp file uses 0600 permissions"
else
  fail "temp file should be created with 0600 permissions"
fi

if grep_both "absolute path" || grep_both "print.*path"; then
  pass "prints the absolute output path"
else
  fail "must print the absolute path of the generated file"
fi

# Offer (not force) to post as description or comment.
if grep_both "offer" && (grep_both "comment" && grep_both "description"); then
  pass "offers to post as PR description or comment"
else
  fail "must offer to post the content as a PR description or comment"
fi

if grep_both "confirm" || grep_both "opt-in" || grep_both "no-op"; then
  pass "publishing is confirmation-gated (default no-op)"
else
  fail "publishing must be confirmation-gated, default no-op"
fi

# Never auto-commit the generated doc.
if grep_both "never commit" || grep_both "not commit" || grep_both "no auto-commit" || grep_both "never.*auto-commit"; then
  pass "never auto-commits the generated document"
else
  fail "must state the doc is never committed"
fi

# ─── Test 11: Input resolution ──────────────────────────────────────────────

echo ""
echo "Test 11: Input resolution"

if grep_both "pr number" && grep_both "branch"; then
  pass "accepts a PR number or branch name as input"
else
  fail "must accept a PR number or branch name as input"
fi

if grep_both "infer" && grep_both "current branch"; then
  pass "infers the PR from the current branch when input is omitted"
else
  fail "must infer the PR from the current branch when no input is given"
fi

if grep_both "standalone" && grep_both "default-workflow"; then
  pass "invocable standalone and at the end of default-workflow"
else
  fail "must be invocable standalone and at the end of default-workflow"
fi

# ─── Test 12: Mermaid inclusion policy ──────────────────────────────────────

echo ""
echo "Test 12: Mermaid used only when appropriate"

if grep_both "when appropriate" || grep_both "earns its place" || grep_both "only when" || grep_both "architectural"; then
  pass "mermaid is included only when appropriate (not every PR)"
else
  fail "must specify mermaid is included only when appropriate"
fi

# SKILL.md should itself contain a mermaid fenced block (the pipeline diagram)
# as a concrete example of the convention.
if grep -q '```mermaid' "$SKILL_FILE"; then
  pass "SKILL.md contains an example mermaid block"
else
  fail "SKILL.md should include at least one mermaid example block"
fi

# ─── Test 13: Security mandate ──────────────────────────────────────────────

echo ""
echo "Test 13: Security mandate"

if grep_both "argv"; then
  pass "mandates argv-array CLI invocation (no shell interpolation)"
else
  fail "must mandate building CLI calls as argv arrays"
fi

# PR-number and branch validation regexes.
if grep_both_cs '\^\\d\+\$' || grep_both 'd\+\$' || grep_both "validate.*pr number"; then
  pass "validates PR numbers (e.g. ^\\d+\$)"
else
  fail "must validate PR numbers against a numeric pattern"
fi

if grep_both 'w\./-' || grep_both "validate.*branch"; then
  pass "validates branch names (e.g. ^[\\w./-]+\$)"
else
  fail "must validate branch names against a safe pattern"
fi

# Untrusted PR content.
if grep_both "untrusted" || grep_both "inert data" || (grep_both "data" && grep_both "not.*command"); then
  pass "treats fetched PR content as inert data, not commands"
else
  fail "must treat PR content as inert data (prompt-injection guard)"
fi

# No credential/token handling.
if grep_both "token" && (grep_both "never.*store" || grep_both "never.*log" || grep_both "no credential"); then
  pass "never reads, stores, or logs credentials/tokens"
else
  fail "must avoid handling credentials/tokens directly"
fi

# ─── Test 14: No executable code or secrets leaked into the skill files ──────

echo ""
echo "Test 14: No leaked secrets in skill files"

if grep -qE "(sk-[A-Za-z0-9]{10,}|ghp_[A-Za-z0-9]{20,}|xoxb-|AKIA[0-9A-Z]{16})" "${BOTH_FILES[@]}" 2>/dev/null; then
  fail "skill files may contain secrets or API keys"
else
  pass "no secrets detected in skill files"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
