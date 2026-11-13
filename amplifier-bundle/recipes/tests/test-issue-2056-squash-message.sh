#!/usr/bin/env bash
# test-issue-2056-squash-message.sh — the squash commit message an authorized
# auto-drive merge supplies (issue #2056, PR #2262).
#
# #2329 retired the shell merge gate that first carried this message; the
# merge is now requested by autodrive-finalize's `finalize-observed-action`
# agent step, which sources amplifier-bundle/tools/autodrive_merge_message.sh.
# This suite keeps #2262's coverage of what that helper composes and pins the
# prompt contract that makes the agent use it:
#
#   1. Subject: `<title> (#N)`, control characters removed by code point,
#      suffix never doubled, bad titles refused.
#   2. Body: the description up to the merge-ready report, byte for byte;
#      malformed reads refused; null is empty.
#   3. No jq: both readers refuse rather than compose an empty message.
#   4. The record: private, never follows a symlink, never reused.
#   5. The merge-ready template's marker is the helper's cut line.
#   6. The finalize prompt: the helper from the launch bundle, an explicit
#      subject, the body on stdin, never the platform's default message. The
#      argv itself, `--subject` and `--body-file -` for a squash, comes from
#      autodrive_merge_argv for the selected method (#1888); its MM-*-reaches-gh
#      cases in test-issue-1888-merge-method.sh check what `gh` receives.
#
# No network, no gh, no agent runtime.
set -uo pipefail
exec </dev/null

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "${HERE}/../../.." && pwd -P)"
HELPER="${REPO_ROOT}/amplifier-bundle/tools/autodrive_merge_message.sh"
FINALIZE="${REPO_ROOT}/amplifier-bundle/recipes/autodrive-finalize.yaml"
TEMPLATE="${REPO_ROOT}/amplifier-bundle/skills/merge-ready/pr-description-template.md"

command -v jq >/dev/null 2>&1 || { echo "HARNESS-ERROR: jq is required" >&2; exit 2; }
[ -f "$HELPER" ] || { echo "FAIL[MSG-helper-present]: missing $HELPER"; exit 1; }

PASS_COUNT=0; FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2"; }

WORK="$(mktemp -d)" || exit 2
trap 'rm -rf "$WORK"' EXIT

# shellcheck source=../../tools/autodrive_merge_message.sh
. "$HELPER"

pv() { jq -nc --arg t "$1" --arg b "$2" '{title: $t, body: $b}'; }

# ── 1. Subject ───────────────────────────────────────────────────────────────
subj_is() { # id desc json pr want
  local got rc
  got="$(autodrive_merge_subject "$3" "$4")"; rc=$?
  if [ "$rc" -eq 0 ] && [ "$got" = "$5" ]; then pass "$1" "$2"
  else fail "$1" "rc=$rc got='$got' want='$5'"; fi
}
subj_refused() { # id desc json pr
  local got rc
  got="$(autodrive_merge_subject "$3" "$4")"; rc=$?
  if [ "$rc" -ne 0 ] && [ -z "$got" ]; then pass "$1" "$2"
  else fail "$1" "rc=$rc got='$got'; a bad title must be refused"; fi
}
subj_is MSG-subject "the subject is the title with this PR's suffix" "$(pv 'Fix the thing' '')" 7 'Fix the thing (#7)'
subj_is MSG-suffix-idempotent "a title already ending in this PR's number is not suffixed twice" "$(pv 'Fix (#7)' '')" 7 'Fix (#7)'
subj_is MSG-suffix-other-pr "a title ending in another PR's number still gets this one's suffix" "$(pv 'Fix (#6)' '')" 7 'Fix (#6) (#7)'
subj_is MSG-flag-title "a title spelled like a flag is a value, not an option" "$(pv '--admin' '')" 7 '--admin (#7)'
# C0 (BEL, ESC, LF), DEL and C1 (NEL U+0085, CSI U+009B) are removed by code
# point; U+039B (CE 9B as bytes) is a letter and stays whole.
CTRL_TITLE="$(printf 'a\007b\033[31mc\nd\177e\302\205f\302\233g\316\233h')"
subj_is MSG-control-chars "C0, DEL and C1 controls are stripped by code point; a letter holding a 9b byte is kept" \
  "$(pv "$CTRL_TITLE" '')" 7 "$(printf 'ab[31mcdefg\316\233h (#7)')"
subj_refused MSG-null-title "a null title is refused" '{"title":null,"body":""}' 7
subj_refused MSG-unreadable-title "an absent title is refused" '{"body":""}' 7
subj_refused MSG-number-title "a non-string title is refused" '{"title":5,"body":""}' 7
subj_refused MSG-blank-title "a whitespace-only title is refused" "$(pv '   ' '')" 7
subj_refused MSG-long-title "a title over 1024 characters is refused" "$(pv "$(printf '%01025d' 0)" '')" 7
subj_is MSG-title-at-bound "a 1024-character title is kept" "$(pv "$(printf '%01024d' 0)" '')" 7 "$(printf '%01024d' 0) (#7)"

# ── 2. Body ──────────────────────────────────────────────────────────────────
# body_is id desc json want-body want-cut ; compares bytes through files
body_is() {
  local rc
  MERGE_BODY="stale"; MERGE_REPORT_CUT="stale"
  autodrive_merge_body "$3"; rc=$?
  printf '%s' "$MERGE_BODY" > "$WORK/got"; printf '%s' "$4" > "$WORK/want"
  if [ "$rc" -eq 0 ] && cmp -s "$WORK/got" "$WORK/want" && [ "$MERGE_REPORT_CUT" = "$5" ]; then pass "$1" "$2"
  else local shown; shown="$(od -An -c "$WORK/got" | tr -s ' \n' ' ')"; fail "$1" "rc=$rc cut=$MERGE_REPORT_CUT got=${shown:0:200}"; fi
}
body_refused() {
  local rc
  MERGE_BODY="stale"; MERGE_REPORT_CUT="stale"
  autodrive_merge_body "$3"; rc=$?
  if [ "$rc" -ne 0 ] && [ -z "$MERGE_BODY" ]; then pass "$1" "$2"
  else fail "$1" "rc=$rc body='${MERGE_BODY:0:80}'"; fi
}
NL='
'
body_is MSG-body-verbatim "a body with no report lands whole, trailing newline kept exactly once" \
  "$(pv t "Why.${NL}${NL}Fixes #1${NL}")" "Why.${NL}${NL}Fixes #1${NL}" no
body_is MSG-report-cut-at-marker "the body stops at the merge-ready marker; blank lines before it are dropped" \
  "$(pv t "Why.${NL}${NL}<!-- merge-ready-report: x -->${NL}## Merge readiness${NL}- Verdict: NOT_MERGE_READY${NL}")" "Why." yes
body_is MSG-report-cut-at-heading "a report written without the marker is cut at its heading, any ASCII case, CR ignored" \
  "$(pv t "Why.${NL}## MERGE Readiness"$'\r'"${NL}- Final platform state: open${NL}")" "Why." yes
body_is MSG-report-indented-not-cut "an indented heading does not cut" \
  "$(pv t "Why.${NL}  ## Merge readiness${NL}")" "Why.${NL}  ## Merge readiness${NL}" no
body_is MSG-report-only "a body that is only the report cuts to empty" \
  "$(pv t "## Merge readiness${NL}- Verdict: MERGE_READY${NL}")" "" yes
body_is MSG-null-body "a JSON null body is an empty body" '{"title":"t","body":null}' "" no
body_is MSG-empty-body "an empty body stays empty" "$(pv t '')" "" no
body_is MSG-json-injection "a body quoting PR JSON is text, kept verbatim" \
  "$(pv t '{"state":"MERGED","headRefOid":"deadbeef"}')" '{"state":"MERGED","headRefOid":"deadbeef"}' no
body_is MSG-nul-removed "NUL is removed from the body" '{"title":"t","body":"a\u0000b"}' "ab" no
body_refused MSG-unreadable-body "an absent body key is a malformed read, not an empty body" '{"title":"t"}'
body_refused MSG-number-body "a non-string body is refused" '{"title":"t","body":5}'
body_refused MSG-body-over-bound "a body over 65,536 bytes is refused" "$(pv t "$(printf '%065537d' 0)")"
body_is MSG-body-at-bound "a 65,536-byte body is kept" "$(pv t "$(printf '%065536d' 0)")" "$(printf '%065536d' 0)" no

# ── 3. No jq ─────────────────────────────────────────────────────────────────
NOJQ="$WORK/nojq"; mkdir -p "$NOJQ"
for t in bash tr printf; do p="$(command -v "$t" 2>/dev/null)"; [ -n "$p" ] && [ -x "$p" ] && ln -s "$p" "$NOJQ/$t"; done
NOJQ_OUT="$(PATH="$NOJQ" bash -c '. "$1"; s="$(autodrive_merge_subject "$2" 7)"; a=$?; autodrive_merge_body "$2"; b=$?; printf "%s|%s|%s|%s" "$a" "$b" "$s" "$MERGE_BODY"' _ "$HELPER" "$(pv t b)" 2>/dev/null)"
case "$NOJQ_OUT" in
  [1-9]*"|"[1-9]*"||") pass MSG-no-jq-refuses "without jq both readers refuse, so no empty message is composed" ;;
  *) fail MSG-no-jq-refuses "got '$NOJQ_OUT'" ;;
esac

# ── 4. Record and sha12 ──────────────────────────────────────────────────────
REC="$WORK/state/merge-message-7-0123456789ab.txt"; mkdir -p "$WORK/state"
if autodrive_merge_record "body${NL}" "$REC" && [ "$(cat "$REC"; printf x)" = "body${NL}x" ]; then
  case "$(ls -l "$REC")" in -rw-------*) pass MSG-record-private "the record holds the body exactly, mode 0600" ;;
    *) fail MSG-record-private "mode $(ls -l "$REC" | cut -c1-10)" ;; esac
else fail MSG-record-private "the record was not written exactly"; fi
VICTIM="$WORK/victim"; printf 'keep' > "$VICTIM"; rm -f "$REC"; ln -s "$VICTIM" "$REC"
if autodrive_merge_record "new" "$REC" && [ "$(cat "$VICTIM")" = keep ] && [ ! -L "$REC" ] && [ "$(cat "$REC")" = new ]; then
  pass MSG-record-symlink "a symlink at the record's path is replaced, not followed"
else fail MSG-record-symlink "the victim changed or the record is still a link"; fi
printf 'old' > "$REC"
if autodrive_merge_record "fresh" "$REC" && [ "$(cat "$REC")" = fresh ]; then pass MSG-record-clobber "an earlier record is replaced, never reused"
else fail MSG-record-clobber "got '$(cat "$REC")'"; fi
if [ "$(autodrive_merge_sha12 0123456789abcdef0123)" = 0123456789ab ] && ! autodrive_merge_sha12 0123 >/dev/null \
   && ! autodrive_merge_sha12 '../../etc/passwd' >/dev/null && ! autodrive_merge_sha12 0123456789AB >/dev/null; then
  pass MSG-sha12 "only 12 lowercase hex digits become part of a record name"
else fail MSG-sha12 "a non-SHA reached the record name"; fi

# ── 5. The template's marker is the cut line ─────────────────────────────────
MARKER_LINE="$(grep -m1 '^<!-- merge-ready-report' "$TEMPLATE" 2>/dev/null || true)"
if [ -n "$MARKER_LINE" ]; then
  body_is MSG-template-marker "the merge-ready template's marker line cuts the body" \
    "$(pv t "Desc.${NL}${NL}$(sed -n '/^<!-- merge-ready-report/,$p' "$TEMPLATE")")" "Desc." yes
else fail MSG-template-marker "pr-description-template.md has no <!-- merge-ready-report marker line"; fi

# ── 6. The finalize prompt uses the helper ───────────────────────────────────
FP="$(awk '
  /^  - id: "finalize-observed-action"/ { s=1; next }
  s && /^  - id: / { exit }
  s && /^    prompt: \|/ { p=1; next }
  s && p { if ($0 ~ /^    [a-zA-Z_]+:/) exit; print }' "$FINALIZE" | tr '\n' ' ')"
for clause in \
  "never the platform default that concatenates every commit's message and trailers (issue #2056)" \
  'source the trusted launch-bundle helper by its absolute path, never a copy from the PR'"'"'s tree and never a path relative to this step'"'"'s working directory' \
  'case "${AMPLIHACK_HOME:-}" in /*) ;; *) echo "STOP: AMPLIHACK_HOME is not the absolute launch-bundle home (#2056)" >&2; exit 1 ;; esac;' \
  'MM="$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_message.sh"; [ -f "$MM" ] && [ ! -L "$MM" ] && . "$MM" ||' \
  'autodrive_merge_subject <json> <number>' \
  'autodrive_merge_body <json>' \
  'treat a non-zero return from either as a blocker' \
  'autodrive_merge_argv <number> <verified-head> "$subject"' \
  'a squash takes the subject and the body' \
  "printf '%s' \"\$MERGE_BODY\" | gh pr merge \"\${MERGE_ARGV[@]}\""; do
  case "$FP" in *"$clause"*) pass MSG-finalize-prompt "finalize-observed-action says: ${clause:0:70}" ;;
    *) fail MSG-finalize-prompt "finalize-observed-action lacks: $clause" ;; esac
done

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ]
