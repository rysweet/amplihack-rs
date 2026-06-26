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

# Attach to PR description or comment.
if (grep_both "attach" || grep_both "append") && (grep_both "comment" && grep_both "description"); then
  pass "attaches the guide to the PR description or comment"
else
  fail "must attach the guide to a PR description or comment"
fi

if grep_both "automatic" && (grep_both "attach" || grep_both "append"); then
  pass "publishing is automatic (description-first, comment-fallback)"
else
  fail "publishing must be automatic"
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

# grep across the combined corpus using a FIXED (non-regex) string. Needed for
# markers containing regex metacharacters like `[`, `]`, `(`, `)`, `#`, `?`.
grep_both_f() { grep -qiF -- "$1" "${BOTH_FILES[@]}" 2>/dev/null; }

# ─── Test 15: PR Description Clarity Pass (R1) ──────────────────────────────
# A NEW, separate action that rewrites the *existing* PR description in place,
# preserving meaning and removing jargon. Must be distinct from the existing
# Guide Clarity Pass and appear in BOTH files (SKILL index + reference detail).

echo ""
echo "Test 15: PR Description Clarity Pass (in-place description rewrite)"

# The named marker must exist in each file individually (not just the corpus),
# because SKILL.md is the index and reference.md is the detail.
if grep -qiF "PR Description Clarity Pass" "$SKILL_FILE"; then
  pass "SKILL.md names the 'PR Description Clarity Pass'"
else
  fail "SKILL.md must name the 'PR Description Clarity Pass'"
fi

if grep -qiF "PR Description Clarity Pass" "$REFERENCE_FILE"; then
  pass "reference.md names the 'PR Description Clarity Pass'"
else
  fail "reference.md must name the 'PR Description Clarity Pass'"
fi

# Must be disambiguated from the guide-clarity pass (both names present).
if grep_both_f "Guide Clarity Pass"; then
  pass "disambiguated from the 'Guide Clarity Pass'"
else
  fail "must keep 'Guide Clarity Pass' distinct from 'PR Description Clarity Pass'"
fi

# It is a SEPARATE action operating on the EXISTING description, in place.
if (grep_both "separate" || grep_both "distinct" || grep_both "additional") \
  && grep_both "existing" && (grep_both "in place" || grep_both "in-place"); then
  pass "framed as a separate, in-place rewrite of the existing description"
else
  fail "must frame it as a separate in-place rewrite of the existing description"
fi

# Meaning must be preserved by the rewrite.
if grep_both "preserv" && grep_both "meaning"; then
  pass "rewrite preserves the original meaning"
else
  fail "must state the description rewrite preserves meaning"
fi

# It removes jargon so unfamiliar reviewers can read it.
if grep_both "jargon" && (grep_both "unfamiliar" || grep_both "reviewer"); then
  pass "removes jargon for reviewers unfamiliar with the work"
else
  fail "must remove jargon so unfamiliar reviewers can understand the description"
fi

# Performed via PR API writes (gh pr edit / az repos pr update), never a commit.
if grep_both "gh pr edit" && grep_both "body-file"; then
  pass "GitHub rewrite uses 'gh pr edit --body-file'"
else
  fail "must rewrite the GitHub description via 'gh pr edit --body-file'"
fi

if grep_both "az repos pr update" && grep_both "description"; then
  pass "Azure DevOps rewrite uses 'az repos pr update --description'"
else
  fail "must rewrite the ADO description via 'az repos pr update --description'"
fi

# ─── Test 16: Guide-comment back-link (R2) ──────────────────────────────────
# When the guide overflows the description and is posted as a comment, a link to
# that comment must be inserted back into the PR description. Covers both
# platforms, comment-ID retrieval, numeric validation, and single-final-write.

echo ""
echo "Test 16: Guide-comment back-link (comment-overflow path)"

if grep_both "back-link" || grep_both "back link"; then
  pass "documents a guide-comment back-link"
else
  fail "must document inserting a back-link to the guide comment"
fi

# Back-link only in the comment-overflow case.
if grep_both "overflow" || grep_both "too long" || grep_both "comment-overflow"; then
  pass "back-link is scoped to the comment-overflow case"
else
  fail "must scope the back-link to the comment-overflow case"
fi

# GitHub shorthand back-link (exact illustrative form).
if grep_both_f "[Illustrated Guide](#issuecomment-"; then
  pass "GitHub shorthand back-link: [Illustrated Guide](#issuecomment-<ID>)"
else
  fail "must show the GitHub shorthand [Illustrated Guide](#issuecomment-<ID>)"
fi

# GitHub full-form comment URL.
if grep_both "github.com" && grep_both_f "#issuecomment-"; then
  pass "GitHub full-form comment URL uses #issuecomment-<ID>"
else
  fail "must document the GitHub #issuecomment-<ID> comment URL"
fi

# ADO back-link URL with threadId.
if grep_both "dev.azure.com" && grep_both_f "_a=overview&threadId="; then
  pass "ADO back-link uses ?_a=overview&threadId=<ID>"
else
  fail "must document the ADO ?_a=overview&threadId=<ID> back-link"
fi

# Comment-ID retrieval from the CLI response (GitHub URL parse; ADO thread id).
if grep_both "gh pr comment" && (grep_both "parse" || grep_both "trailing"); then
  pass "retrieves the GitHub comment ID by parsing the CLI response URL"
else
  fail "must retrieve the GitHub comment ID from the 'gh pr comment' response"
fi

if (grep_both "thread" && grep_both "id") && grep_both "threadId"; then
  pass "reuses the ADO thread id as threadId"
else
  fail "must reuse the ADO PR-thread id as the threadId"
fi

# Numeric validation of the ID before interpolation (injection guard).
if grep_both_f "^[0-9]+$" || grep_both "numeric"; then
  pass "validates the comment/thread ID as numeric before use"
else
  fail "must validate the comment/thread ID against ^[0-9]+\$"
fi

# Single final description write (avoid double updates with the clarity pass).
if grep_both "single final write"; then
  pass "combines clarity rewrite + back-link into a single final write"
else
  fail "must combine the clarity rewrite and back-link into one final write"
fi

# ─── Test 17: Mermaid on Azure DevOps (R3) ──────────────────────────────────
# ADO does not natively render mermaid in PR descriptions/comments. Document the
# image fallback (mmdc CLI or mermaid.ink) and the embed-as-image instruction,
# with a third-party privacy note. Lives in reference.md.

echo ""
echo "Test 17: Mermaid on Azure DevOps (image fallback)"

if grep -qiF "Mermaid on Azure DevOps" "$REFERENCE_FILE"; then
  pass "reference.md has a labeled 'Mermaid on Azure DevOps' section"
else
  fail "reference.md must include a 'Mermaid on Azure DevOps' section"
fi

# ADO has no native mermaid rendering in PR description/comment.
if grep_both "no native" || grep_both "does not render" || grep_both "not.*natively"; then
  pass "states ADO does not natively render mermaid in PR descriptions/comments"
else
  fail "must state ADO has no native mermaid rendering in PR descriptions/comments"
fi

# Note about checking for recent/native support.
if grep_both "release notes" || grep_both "recent" || grep_both "later" || grep_both "re-check"; then
  pass "notes checking for recent/native ADO mermaid support"
else
  fail "must note checking whether ADO added native support recently"
fi

# Fallback option a: local mermaid CLI (mmdc).
if grep_both "mmdc"; then
  pass "documents the local 'mmdc' (mermaid CLI) fallback"
else
  fail "must document the mmdc (mermaid CLI) fallback to SVG/PNG"
fi

# Fallback option b: mermaid.ink hosted renderer with base64 payload.
if grep_both "mermaid.ink" && grep_both "base64"; then
  pass "documents the mermaid.ink/<base64-encoded-diagram> fallback"
else
  fail "must document the mermaid.ink/img/<base64-encoded-diagram> fallback"
fi

# Embed as an image (![ ... ]) instead of a ```mermaid fence on ADO.
if grep_both_f "![" && (grep_both "instead of" || grep_both "rewrite") && grep_both "fence"; then
  pass "embeds an image instead of a mermaid fence on ADO"
else
  fail "must embed an image (![](url)) instead of a mermaid fence on ADO"
fi

# Third-party privacy note for mermaid.ink.
if grep_both "third-party" && (grep_both "privacy" || grep_both "sensitive" || grep_both "internal"); then
  pass "warns mermaid.ink sends diagrams to a third party (privacy note)"
else
  fail "must include a privacy note that mermaid.ink is a third-party service"
fi

# GitHub keeps native mermaid fences (no conversion there).
if grep_both "GitHub" && grep_both "native" && grep_both "fence"; then
  pass "GitHub retains native mermaid fences (conversion is ADO-only)"
else
  fail "must state GitHub keeps native mermaid fences (ADO-only conversion)"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
