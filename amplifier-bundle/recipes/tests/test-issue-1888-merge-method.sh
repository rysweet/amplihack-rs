#!/usr/bin/env bash
# test-issue-1888-merge-method.sh — the merge method an authorized auto-drive
# merge uses (issue #1888).
#
# #2329 retired the shell merge gate that first ran the selector. The merge is
# now requested by autodrive-finalize's `finalize-observed-action` agent step,
# which sources amplifier-bundle/tools/autodrive_merge_method.sh beside
# autodrive_merge_message.sh. The selector itself (sources, allowed set,
# rulesets, the repository file) is tested in Rust, in
# crates/amplihack-cli/tests/issue_1888_merge_method_selection.rs. This suite
# covers what lies between the selector's answer and `gh`:
#
#   1. The helper passes the selector its six inputs unchanged.
#   2. For each method, the selector's choice reaches `gh pr merge` as one
#      flag, with that method's message rules: the argv and stdin `gh` got.
#   3. A selector error, a policy refusal (79), malformed output and a missing
#      jq block with a reason and leave no flag and no argv.
#   4. autodrive_merge_argv refuses what is not a PR number, a full head SHA
#      or, for a squash, a subject.
#   5. The finalize prompt: its preamble loads both helpers from the launch
#      bundle and refuses anything else, it passes the context through, takes
#      the argv from the helper, and names no merge-method flag of its own.
#   6. merge_method is declared empty and forwarded down to autodrive-finalize.
#
# No network, no real gh, no agent runtime. The selector is a stub that prints
# a scripted answer and records its argv.
set -uo pipefail
exec </dev/null
# Child shells here run a stub `gh`. A BASH_ENV left by an older install
# (the retired #1551 guard) would define its own `gh` in each of them, so it
# is dropped, as the Rust wrapper that runs this suite drops it too.
unset BASH_ENV ENV

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "${HERE}/../../.." && pwd -P)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
METHOD_HELPER="${TOOLS}/autodrive_merge_method.sh"
MESSAGE_HELPER="${TOOLS}/autodrive_merge_message.sh"
FINALIZE="${RECIPES}/autodrive-finalize.yaml"

command -v jq >/dev/null 2>&1 || { echo "HARNESS-ERROR: jq is required" >&2; exit 2; }
for f in "$METHOD_HELPER" "$MESSAGE_HELPER" "$FINALIZE"; do
  [ -f "$f" ] || { echo "FAIL[MM-files-present]: missing $f"; exit 1; }
done

PASS_COUNT=0; FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2"; }

WORK="$(mktemp -d)" || exit 2
trap 'rm -rf "$WORK"' EXIT
STUB="$WORK/bin"; mkdir -p "$STUB"

# The selector stub: records its argv one argument per line, then prints
# STUB_SELECT_OUT and exits STUB_SELECT_RC.
cat > "$STUB/amplihack" <<'STUB_EOF'
#!/usr/bin/env bash
: > "$STUB_SELECT_LOG"
for a in "$@"; do printf '%s\n' "$a" >> "$STUB_SELECT_LOG"; done
printf '%s' "${STUB_SELECT_OUT:-}"
exit "${STUB_SELECT_RC:-0}"
STUB_EOF
# The gh stub: records its argv one argument per line, and its stdin.
cat > "$STUB/gh" <<'STUB_EOF'
#!/usr/bin/env bash
: > "$GH_LOG.argv"
for a in "$@"; do printf '%s\n' "$a" >> "$GH_LOG.argv"; done
cat > "$GH_LOG.stdin"
STUB_EOF
chmod +x "$STUB/amplihack" "$STUB/gh"
export STUB_SELECT_LOG="$WORK/select.argv" GH_LOG="$WORK/gh"
export AMPLIHACK_BIN="$STUB/amplihack" PATH="$STUB:$PATH"

URL="https://github.com/owner/project/pull/7"
ROOT="/nonexistent/linked-worktree"
BASE="1111111111111111111111111111111111111111"
HEAD="2222222222222222222222222222222222222222"
NL='
'
PV="$(jq -nc --arg t 'Fix the thing' --arg b "Why.${NL}${NL}<!-- merge-ready-report: x -->${NL}## Merge readiness${NL}- Verdict: MERGE_READY${NL}" '{title: $t, body: $b}')"
selection() { # method source
  local commit="$BASE"; [ "$2" = explicit ] && commit=""
  jq -nc --arg m "$1" --arg s "$2" --arg c "$commit" \
    '{method: $m, source: $s, repository: "owner/project", config_commit: (if $c == "" then null else $c end), rationale: ("Selected " + $m + " from the fixture")}'
}
lines() { printf '%s\n' "$@"; }

# The finalize prompt, as one line per prompt line.
FP="$(awk '
  /^  - id: "finalize-observed-action"/ { s=1; next }
  s && /^  - id: / { exit }
  s && /^    prompt: \|/ { p=1; next }
  s && p { if ($0 ~ /^    [a-zA-Z_]+:/) exit; print }' "$FINALIZE")"
# The preamble the prompt tells the agent to start its shell call with, and
# the merge pipeline it gives as step (4), both taken verbatim.
PREAMBLE="${FP#*start that call with \`}"; PREAMBLE="${PREAMBLE%%\` (AMPLIHACK_HOME is*}"
PIPELINE="${FP#*(4) Pipe the body from memory: \`}"; PIPELINE="${PIPELINE%%\`*}"
case "$PREAMBLE" in 'case "${AMPLIHACK_HOME:-}"'*) ;; *) echo "FAIL[MM-finalize-prompt]: no preamble found in finalize-observed-action"; exit 1 ;; esac
case "$PIPELINE" in "printf '%s' \"\$MERGE_BODY\" | gh pr merge "*) ;; *) echo "FAIL[MM-finalize-prompt]: no merge pipeline found in finalize-observed-action"; exit 1 ;; esac

# The whole merge, as the prompt orders it: the preamble with the real
# helpers, selection, message, argv, then the prompt's own pipeline line.
# Exits 10 to 13 at the step that refused, else gh's status.
merge_flow() { # explicit-merge_method
  AMPLIHACK_HOME="$REPO_ROOT" bash -c "$PREAMBLE"'
    autodrive_merge_method "$1" "$2" main "$3" "$4" "$5" || exit 10
    subject="$(autodrive_merge_subject "$6" 7)" || exit 11
    autodrive_merge_body "$6" || exit 12
    autodrive_merge_argv 7 "$4" "$subject" || exit 13
    '"$PIPELINE" _ "$URL" "$ROOT" "$BASE" "$HEAD" "${1:-}" "$PV"
}
gh_got() { # id desc expected-argv... ; compares gh's argv with what follows
  local id="$1" desc="$2"; shift 2
  lines "$@" > "$WORK/want.argv"
  if cmp -s "$WORK/want.argv" "$GH_LOG.argv"; then pass "$id" "$desc"
  else fail "$id" "gh argv was: $(tr '\n' ' ' < "$GH_LOG.argv" 2>/dev/null)"; fi
}

# ── 1. The selector's inputs ─────────────────────────────────────────────────
STUB_SELECT_OUT="$(selection squash default)" STUB_SELECT_RC=0 bash -c '. "$1"; autodrive_merge_method "$2" "$3" main "$4" "$5" ""' _ \
  "$METHOD_HELPER" "$URL" "$ROOT" "$BASE" "$HEAD"
lines orch helper select-merge-method --pr-url "$URL" --repo-root "$ROOT" --base-ref main --base "$BASE" --head "$HEAD" --merge-method "" > "$WORK/want.select"
if cmp -s "$WORK/want.select" "$STUB_SELECT_LOG"; then pass MM-selector-argv "the selector gets the URL, worktree, base branch, base SHA, head and an empty context unchanged"
else fail MM-selector-argv "selector argv was: $(tr '\n' ' ' < "$STUB_SELECT_LOG")"; fi
STUB_SELECT_OUT='{"error":"x"}' STUB_SELECT_RC=1 bash -c '. "$1"; autodrive_merge_method "$2" "$3" main "$4" "$5" "--admin"' _ \
  "$METHOD_HELPER" "$URL" "$ROOT" "$BASE" "$HEAD"
if [ "$(tail -n 1 "$STUB_SELECT_LOG")" = "--admin" ] && [ "$(tail -n 2 "$STUB_SELECT_LOG" | head -n 1)" = "--merge-method" ]; then
  pass MM-selector-context-verbatim "a context spelled like a flag reaches the selector as the value, to be refused there"
else fail MM-selector-context-verbatim "selector argv was: $(tr '\n' ' ' < "$STUB_SELECT_LOG")"; fi

# ── 2. Each method reaches gh ────────────────────────────────────────────────
rm -f "$GH_LOG.argv" "$GH_LOG.stdin"
STUB_SELECT_OUT="$(selection squash default)" STUB_SELECT_RC=0 merge_flow ""; rc=$?
if [ "$rc" -eq 0 ]; then
  gh_got MM-squash-reaches-gh "a squash: --squash, the subject and the body on stdin" \
    pr merge 7 --squash --match-head-commit "$HEAD" --subject 'Fix the thing (#7)' --body-file -
  if [ "$(cat "$GH_LOG.stdin"; printf x)" = "Why.x" ]; then pass MM-squash-body "the squash body is the description, the merge-ready report cut off"
  else fail MM-squash-body "gh read '$(cat "$GH_LOG.stdin")'"; fi
else fail MM-squash-reaches-gh "the flow stopped with $rc"; fi

rm -f "$GH_LOG.argv" "$GH_LOG.stdin"
STUB_SELECT_OUT="$(selection merge repository)" STUB_SELECT_RC=0 merge_flow ""; rc=$?
if [ "$rc" -eq 0 ]; then
  gh_got MM-merge-reaches-gh "a merge commit: --merge and the body alone, no subject" \
    pr merge 7 --merge --match-head-commit "$HEAD" --body-file -
  if [ "$(cat "$GH_LOG.stdin"; printf x)" = "Why.x" ]; then pass MM-merge-body "the merge commit body is the description, never the merge-ready report"
  else fail MM-merge-body "gh read '$(cat "$GH_LOG.stdin")'"; fi
else fail MM-merge-reaches-gh "the flow stopped with $rc"; fi

rm -f "$GH_LOG.argv" "$GH_LOG.stdin"
STUB_SELECT_OUT="$(selection rebase explicit)" STUB_SELECT_RC=0 merge_flow rebase; rc=$?
if [ "$rc" -eq 0 ]; then
  gh_got MM-rebase-reaches-gh "a rebase: --rebase and no message flags" \
    pr merge 7 --rebase --match-head-commit "$HEAD"
else fail MM-rebase-reaches-gh "the flow stopped with $rc"; fi

OUT="$(STUB_SELECT_OUT="$(selection merge repository)" STUB_SELECT_RC=0 bash -c '. "$1"; autodrive_merge_method "$2" "$3" main "$4" "$5" "" || exit 9
  printf "%s|%s|%s" "$MERGE_METHOD" "$MERGE_FLAG" "$MERGE_METHOD_SELECTION"' _ "$METHOD_HELPER" "$URL" "$ROOT" "$BASE" "$HEAD")"
if [ "$OUT" = "merge|--merge|$(selection merge repository)" ]; then pass MM-selection-recorded "MERGE_METHOD, MERGE_FLAG and the selection JSON are set for the record"
else fail MM-selection-recorded "got '$OUT'"; fi

# ── 3. Failures block ────────────────────────────────────────────────────────
# blocked id desc rc-want reason-want selector-out selector-rc
blocked() {
  local got rc
  rm -f "$GH_LOG.argv"
  got="$(STUB_SELECT_OUT="$5" STUB_SELECT_RC="$6" bash -c '. "$1"; MERGE_FLAG=--squash; MERGE_METHOD=squash
    autodrive_merge_method "$2" "$3" main "$4" "$5" ""; rc=$?
    autodrive_merge_argv 7 "$5" "subject" && echo ARGV-BUILT
    printf "%s|%s|%s|%s|%s" "$rc" "$MERGE_METHOD" "$MERGE_FLAG" "${#MERGE_ARGV[@]}" "$MERGE_METHOD_ERROR"' _ \
    "$METHOD_HELPER" "$URL" "$ROOT" "$BASE" "$HEAD")"; rc=$?
  case "$got" in
    "$3|||0|$4") pass "$1" "$2" ;;
    *) fail "$1" "got '$got'" ;;
  esac
}
blocked MM-error-blocks "a selector error blocks with its reason; no stale flag or argv survives" 1 \
  "merge-method selection failed (exit 1): selected merge method squash is disallowed by repository permissions; no other method is tried" \
  '{"error":"selected merge method squash is disallowed by repository permissions; no other method is tried"}' 1
blocked MM-refusal-79 "a policy refusal from gh api returns 79 with its reason" 79 \
  "merge-method selection failed (exit 79): gh api refused by policy" '{"error":"gh api refused by policy"}' 79
blocked MM-error-no-reason "a failure without the error object still blocks, and says so" 1 \
  "merge-method selection failed (exit 2): the selector gave no reason" 'error: unexpected argument' 2
blocked MM-error-one-line "a reason with control characters is kept to one printable line" 1 \
  "merge-method selection failed (exit 1): a b [c" "$(jq -nc '{error: "a\nb\u001b[c"}')" 1
MALFORMED=(
  ""
  "$(selection squash default)$(selection squash default)"
  "note: $(selection squash default)"
  "$(selection squash default | jq -c '.method = "MERGE"')"
  "$(selection squash default | jq -c '.method = "--admin"')"
  "$(selection squash default | jq -c '.source = "operator"')"
  "$(selection squash explicit | jq -c '.config_commit = "1111111111111111111111111111111111111111"')"
  "$(selection squash default | jq -c '.config_commit = null')"
  "$(selection squash default | jq -c '.config_commit = "abc"')"
  "$(selection squash default | jq -c '.repository = "owner/project/extra"')"
  "$(selection squash default | jq -c 'del(.rationale)')"
  '["squash"]'
)
MALFORMED_OK=1
for m in "${MALFORMED[@]}"; do
  got="$(STUB_SELECT_OUT="$m" STUB_SELECT_RC=0 bash -c '. "$1"; autodrive_merge_method "$2" "$3" main "$4" "$5" ""; printf "%s|%s|%s" "$?" "$MERGE_FLAG" "$MERGE_METHOD_ERROR"' _ \
    "$METHOD_HELPER" "$URL" "$ROOT" "$BASE" "$HEAD")"
  [ "$got" = "1||merge-method selection failed: the selector returned malformed or incompatible output" ] || { MALFORMED_OK=0; fail MM-malformed-refused "accepted or misreported '${m:0:80}': $got"; }
done
[ "$MALFORMED_OK" -eq 1 ] && pass MM-malformed-refused "every malformed selector answer (${#MALFORMED[@]} shapes) blocks and sets no flag"
NOJQ="$WORK/nojq"; mkdir -p "$NOJQ"
for t in bash tr printf cut cat; do p="$(command -v "$t" 2>/dev/null)"; [ -n "$p" ] && [ -x "$p" ] && ln -s "$p" "$NOJQ/$t"; done
: > "$STUB_SELECT_LOG"
SEL="$(selection squash default)"
got="$(PATH="$NOJQ" STUB_SELECT_OUT="$SEL" STUB_SELECT_RC=0 "$NOJQ/bash" -c '. "$1"; autodrive_merge_method "$2" "$3" main "$4" "$5" ""; printf "%s|%s" "$?" "$MERGE_FLAG"' _ \
  "$METHOD_HELPER" "$URL" "$ROOT" "$BASE" "$HEAD" 2>/dev/null)"
if [ "$got" = "1|" ] && [ ! -s "$STUB_SELECT_LOG" ]; then pass MM-no-jq "without jq nothing is selected and the selector is not run"
else fail MM-no-jq "got '$got'"; fi
rm -f "$GH_LOG.argv"
STUB_SELECT_OUT='{"error":"x"}' STUB_SELECT_RC=1 merge_flow ""; rc=$?
if [ "$rc" -eq 10 ] && [ ! -e "$GH_LOG.argv" ]; then pass MM-error-no-merge "after a selector error the flow stops before gh is called"
else fail MM-error-no-merge "rc=$rc, gh called: $([ -e "$GH_LOG.argv" ] && echo yes || echo no)"; fi

# ── 4. autodrive_merge_argv refuses bad input ────────────────────────────────
argv_refused() { # method flag pr head subject
  bash -c '. "$1"; MERGE_METHOD="$2"; MERGE_FLAG="$3"; MERGE_ARGV=(stale)
    autodrive_merge_argv "$4" "$5" "$6" && exit 1; [ "${#MERGE_ARGV[@]}" -eq 0 ]' _ "$METHOD_HELPER" "$@"
}
ARGV_OK=1
for c in "squash|--squash||$HEAD|s" "squash|--squash|07|$HEAD|s" "squash|--squash|7a|$HEAD|s" "squash|--squash|-7|$HEAD|s" \
         "squash|--squash|7|${HEAD:0:39}|s" "squash|--squash|7|$(printf '%s' "$HEAD" | tr 2 A)|s" "squash|--squash|7|--admin|s" \
         "squash|--squash|7|$HEAD|" "||7|$HEAD|s" "squash|--merge|7|$HEAD|s" "merge|--admin|7|$HEAD|s"; do
  IFS='|' read -r m f p h s <<< "$c"
  argv_refused "$m" "$f" "$p" "$h" "$s" || { ARGV_OK=0; fail MM-argv-refuses "built an argv for method=$m flag=$f pr=$p head=$h subject=$s"; }
done
[ "$ARGV_OK" -eq 1 ] && pass MM-argv-refuses "no argv without a selected method, a PR number, a full lowercase head SHA and, for a squash, a subject"
if bash -c '. "$1"; MERGE_METHOD=rebase; MERGE_FLAG=--rebase; autodrive_merge_argv 7 "$2" "" && [ "${MERGE_ARGV[*]}" = "7 --rebase --match-head-commit $2" ]' _ \
   "$METHOD_HELPER" "$(printf '%064d' 3)"; then pass MM-argv-sha256 "a 64-digit head SHA is accepted"
else fail MM-argv-sha256 "a SHA-256 head was refused"; fi

# ── 5. The finalize prompt ───────────────────────────────────────────────────
if AMPLIHACK_HOME="$REPO_ROOT" bash -c "$PREAMBLE"'
   declare -F autodrive_merge_method autodrive_merge_argv autodrive_merge_subject autodrive_merge_body >/dev/null' 2>/dev/null; then
  pass MM-preamble-loads "the prompt's preamble loads both helpers from an absolute launch-bundle home"
else fail MM-preamble-loads "the preamble did not load both helpers"; fi
# preamble_stops id desc home want-stderr: the preamble, run with that
# AMPLIHACK_HOME from the repository root, exits 1 with exactly that one
# line on stderr and never reaches the command after it.
preamble_stops() {
  local out err rc
  out="$(cd "$REPO_ROOT" && AMPLIHACK_HOME="$3" bash -c "$PREAMBLE"$'\n''echo LOADED' 2>"$WORK/preamble.err")"; rc=$?
  err="$(cat "$WORK/preamble.err")"
  if [ "$rc" -eq 1 ] && [ -z "$out" ] && [ "$err" = "$4" ]; then pass "$1" "$2"
  else fail "$1" "rc=$rc stdout='$out' stderr='$err'"; fi
}
preamble_stops MM-preamble-relative "a relative AMPLIHACK_HOME stops the call" . \
  "STOP: AMPLIHACK_HOME is not the absolute launch-bundle home (#2056)"
LINKED="$WORK/linked-home"; mkdir -p "$LINKED/amplifier-bundle/tools"
cp "$MESSAGE_HELPER" "$LINKED/amplifier-bundle/tools/"
ln -s "$METHOD_HELPER" "$LINKED/amplifier-bundle/tools/autodrive_merge_method.sh"
preamble_stops MM-preamble-symlink "a symlinked autodrive_merge_method.sh stops the call" "$LINKED" \
  "STOP: cannot load the trusted autodrive_merge_method.sh from the launch bundle (#1888)"
rm -f "$LINKED/amplifier-bundle/tools/autodrive_merge_method.sh"
preamble_stops MM-preamble-missing "a launch bundle without autodrive_merge_method.sh stops the call" "$LINKED" \
  "STOP: cannot load the trusted autodrive_merge_method.sh from the launch bundle (#1888)"
cp "$METHOD_HELPER" "$LINKED/amplifier-bundle/tools/"
if [ "$(cd "$REPO_ROOT" && AMPLIHACK_HOME="$LINKED" bash -c "$PREAMBLE"$'\n''echo LOADED' 2>/dev/null)" = LOADED ]; then
  pass MM-preamble-copy "a regular copy of both helpers loads, so the refusals above are about the symlink and the absence"
else fail MM-preamble-copy "a regular copy of both helpers did not load"; fi

FP1="$(printf '%s' "$FP" | tr '\n' ' ')"
PROMPT_OK=1
for clause in \
  'by the merge method the selector chooses (issue #1888)' \
  "autodrive_merge_method <pr-url> <repo-root> <base-branch> <base-sha> <verified-head> '{{merge_method}}'" \
  'amplihack orch helper select-merge-method' \
  'A non-zero return is a blocker: request no merge, set failure_cause to "$MERGE_METHOD_ERROR"' \
  'outcome refused when the return is 79 (terminal, never retried)' \
  'The selector is the only source of the method' \
  'never choose, substitute or fall back to another method yourself, and never retry with a different one' \
  'autodrive_merge_argv <number> <verified-head> "$subject"' \
  'a squash takes the subject and the body, a merge commit takes the body alone' \
  'never write a merge-method flag yourself' \
  'Record MERGE_METHOD_SELECTION' \
  'sends the method and message chosen above'; do
  case "$FP1" in *"$clause"*) ;; *) PROMPT_OK=0; fail MM-finalize-prompt "finalize-observed-action lacks: $clause" ;; esac
done
[ "$PROMPT_OK" -eq 1 ] && pass MM-finalize-prompt "finalize-observed-action takes the method from the selector and the argv from the helper"
# No method flag anywhere in the recipe, comments included: `--squash` is
# what #1888 was opened for, and #2340 brought it back as prompt text.
if grep -nE -- '--(squash|rebase)|--merge($|[^-])' "$FINALIZE" >/dev/null; then
  fail MM-finalize-no-method-flag "autodrive-finalize.yaml names a method flag: $(grep -nE -- '--(squash|rebase)|--merge($|[^-])' "$FINALIZE" | cut -c1-120)"
else pass MM-finalize-no-method-flag "autodrive-finalize.yaml names no --merge, --squash or --rebase of its own"; fi

# ── 6. The context reaches autodrive-finalize ────────────────────────────────
# step_context recipe step-id key: the value of `key:` in that step's context.
step_context() {
  awk -v id="$2" -v key="$3" '
    $0 ~ "^  - id: \"" id "\"" { s=1; next }
    s && /^  - id: / { exit }
    s && $0 ~ "^      " key ": " { sub("^      " key ": ", ""); print; exit }' "$RECIPES/$1.yaml"
}
top_context() { awk -v key="$2" '/^context:/ { s=1; next } s && /^[^ ]/ { exit } s && $0 ~ "^  " key ": " { sub("^  " key ": ", ""); print; exit }' "$RECIPES/$1.yaml"; }
CTX_OK=1
for r in auto-drive-to-merge auto-drive-light autodrive-merge-loop autodrive-finalize; do
  [ "$(top_context "$r" merge_method)" = '""' ] || { CTX_OK=0; fail MM-context-forwarded "$r does not declare merge_method: \"\""; }
done
for want in "auto-drive-to-merge autodrive-merge-loop" "auto-drive-light light-merge-loop" "auto-drive-light light-escalate" \
            "autodrive-merge-loop step-03-finalize"; do
  read -r recipe step_id <<< "$want"
  [ "$(step_context "$recipe" "$step_id" merge_method)" = '"{{merge_method}}"' ] || { CTX_OK=0; fail MM-context-forwarded "$recipe/$step_id does not forward merge_method unchanged"; }
done
[ "$CTX_OK" -eq 1 ] && pass MM-context-forwarded "merge_method is declared empty and forwarded unchanged down to autodrive-finalize"

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ]
