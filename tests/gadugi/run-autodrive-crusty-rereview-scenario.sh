#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #1517 D4 and the PR #1520
# review: a code commit made after the clean crusty round goes back to crusty
# before the next merge round, and that crusty loop runs at the merge-ready
# loop's session depth, where the recursion guard admits it. The re-review's
# crusty loop shares phase 2's state dir, so its rounds must be labelled after
# phase 2's: phase 2's crusty-round-1.json, .findings and .log stay unchanged
# and every row of crusty-records.tsv still names a record that hashes to it
# (PR #1520 review, round 2: before the fix the re-review rewrote round-1).
#
# Black boxes, all SHIPPED code:
#   - amplifier-bundle/tools/autodrive_loop.sh, run as autodrive-merge-loop.yaml
#     runs it, with amplifier-bundle/tools/autodrive_crusty_rereview.sh as its
#     --before-round step, against a real git repository and a crusty state
#     directory laid out the way the crusty loop writes it;
#   - the real `amplihack recipe run` and its recursion guard, against a sealed
#     ceiling of 3, with a stand-in recipe runner so nothing real is started.
#
# The rounds themselves are stand-ins: a stub `amplihack recipe run` writes the
# round record and logs the session depth it was started at; `orch helper` is
# the real amplihack. `gh` is a stub that names the base branch.
#
# Launched by the gadugi `execute` action with no arguments; prints one PASS or
# FAIL line per case and ALL_CASES_PASSED when every case holds.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOOLS="$REPO_ROOT/amplifier-bundle/tools"
for t in autodrive_loop.sh autodrive_crusty_rereview.sh autodrive_state.sh autodrive_trust.sh autodrive_round_evidence.sh; do
  [ -f "$TOOLS/$t" ] || { echo "FAIL: missing $TOOLS/$t"; echo "SCENARIO_FAILED"; exit 1; }
done
for c in git jq; do
  command -v "$c" >/dev/null 2>&1 || { echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1; }
done

# The real amplihack: the verdict pipeline (orch helper) and the recursion
# guard under test. A binary built from this tree first.
supports() { printf '{"a":"b"}' | "$1" orch helper extract-json --require-field a >/dev/null 2>&1; }
REAL=""
for cand in "${CARGO_TARGET_DIR:-}/debug/amplihack" "${CARGO_TARGET_DIR:-}/release/amplihack" \
            "$REPO_ROOT/target/debug/amplihack" "$REPO_ROOT/target/release/amplihack" \
            "$(command -v amplihack 2>/dev/null || true)"; do
  [ -n "$cand" ] && [ -x "$cand" ] && supports "$cand" && { REAL="$cand"; break; }
done
[ -n "$REAL" ] || { echo "FAIL: no amplihack providing 'orch helper extract-json --require-field' (build it with cargo build -p amplihack)"; echo "SCENARIO_FAILED"; exit 1; }

WORK="$(mktemp -d)"
trap 'chmod -R u+rwx "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
WORK="$(cd "$WORK" && pwd -P)"
mkdir -p "$WORK/bin" "$WORK/home"
SYS_PATH="$(dirname "$(command -v jq)"):$(dirname "$(command -v git)"):/usr/bin:/bin"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid

fail=0
pass() { echo "PASS: $1 — $2"; }
fl() { echo "FAIL: $1 — $2"; fail=1; }

# --- the repository --------------------------------------------------------
#   M0 --- R (code, reviewed by crusty) --- D (PR_DESCRIPTION.md only)
#                                       \-- C (code)
# origin/main is M0. Commits are written with plumbing, so no hook runs.
FX="$WORK/repo"; ORIGIN="$WORK/origin.git"; T=1767225600
git init -q --bare "$ORIGIN" && git init -q "$FX" && git -C "$FX" remote add origin "$ORIGIN" \
  || { echo "FAIL: could not create the fixture repository"; echo "SCENARIO_FAILED"; exit 1; }
commit() { # commit <parent|-> <message> <path>=<content>... -> sha
  local parent="$1" msg="$2" idx="$WORK/index" spec blob; shift 2
  rm -f "$idx"
  if [ "$parent" = "-" ]; then GIT_INDEX_FILE="$idx" git -C "$FX" read-tree --empty
  else GIT_INDEX_FILE="$idx" git -C "$FX" read-tree "$parent"; fi
  for spec in "$@"; do
    blob="$(printf '%s\n' "${spec#*=}" | git -C "$FX" hash-object -w --stdin)"
    GIT_INDEX_FILE="$idx" git -C "$FX" update-index --add --cacheinfo "100644,${blob},${spec%%=*}"
  done
  T=$((T + 60))
  if [ "$parent" = "-" ]; then
    GIT_AUTHOR_DATE="@$T +0000" GIT_COMMITTER_DATE="@$T +0000" git -C "$FX" commit-tree "$(GIT_INDEX_FILE="$idx" git -C "$FX" write-tree)" -m "$msg"
  else
    GIT_AUTHOR_DATE="@$T +0000" GIT_COMMITTER_DATE="@$T +0000" git -C "$FX" commit-tree "$(GIT_INDEX_FILE="$idx" git -C "$FX" write-tree)" -p "$parent" -m "$msg"
  fi
}
M0="$(commit - initial README.md=readme src.rs=v0)"
R="$(commit "$M0" "change src.rs" src.rs=v1)"
D="$(commit "$R" "describe the pull request" PR_DESCRIPTION.md=description)"
C="$(commit "$R" "change src.rs again" src.rs=v2)"
git -C "$FX" push -q origin "$M0:refs/heads/main" >/dev/null 2>&1 \
  || { echo "FAIL: could not push the fixture base"; echo "SCENARIO_FAILED"; exit 1; }

# --- stand-ins ---------------------------------------------------------------
cat > "$WORK/bin/gh" <<'GH'
#!/usr/bin/env bash
case " $* " in *" baseRefName "*) echo main; exit 0 ;; esac
exit 1
GH
cat > "$WORK/bin/amplihack" <<'STUB'
#!/usr/bin/env bash
[ "${1:-}" = orch ] && exec "$REAL_AMPLIHACK" "$@"
[ "${1:-} ${2:-}" = "recipe show" ] && exit 0
if [ "${1:-} ${2:-}" = "recipe run" ]; then
  printf '%s depth=%s\n' "${3:-}" "${AMPLIHACK_SESSION_DEPTH:-unset}" >> "$CALLS"
  REC=""; LABEL=""
  for a in "$@"; do
    case "$a" in autodrive_round_record=*) REC="${a#*=}" ;; autodrive_round_label=*) LABEL="${a#*=}" ;; esac
  done
  HEAD="$(git -C "$FIXTURE" rev-parse HEAD)"
  case "${3:-}" in
    loop-health-evaluator) echo "LOOP_HEALTH: DONE — converged"; exit 0 ;;
    autodrive-crusty-round)
      # As step-06 of autodrive-crusty-round.yaml does: under umask 077, the
      # old record goes first, then the new one is written under the label
      # the loop gave.
      umask 077
      rm -f -- "$REC"
      printf '{"crusty_verdict":"CLEAN","concern_count":0,"commits_this_round":0,"head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s","test_signal":"","ci_signal":""}\n' \
        "$HEAD" "$HEAD" "$LABEL" > "$REC"; : > "$REC.findings"; exit 0 ;;
    autodrive-merge-round)
      printf '{"merge_ready_verdict":"MERGE_READY","blocker_count":0,"head_sha":"%s"}\n' "$HEAD" > "$REC"; : > "$REC.findings"; exit 0 ;;
  esac
fi
exit 3
STUB
chmod +x "$WORK/bin/gh" "$WORK/bin/amplihack"

# A crusty loop that ended DONE with a CLEAN round that reviewed R, laid out
# as autodrive_loop.sh and autodrive_record_crusty_loop_done write it: the
# record, its findings (the concern that round settled) and its log.
seed() { # seed <dir>
  mkdir -p "$1"; chmod 0700 "$1"
  ( umask 077
    printf '{"crusty_verdict":"CLEAN","concern_count":0,"commits_this_round":0,"head_sha":"%s","reviewed_head_sha":"%s","round_label":"round-1","test_signal":"","ci_signal":""}\n' \
      "$R" "$R" > "$1/crusty-round-1.json"
    printf 'phase-2-concern\n' > "$1/crusty-round-1.json.findings"
    printf 'phase 2 crusty round-1 log\n' > "$1/crusty-round-1.log"
    cp "$1/crusty-round-1.json" "$1/crusty-latest.json"
    printf 'round-1\tcrusty-round-1.json\t%s\n' "$(git hash-object --no-filters --stdin < "$1/crusty-round-1.json")" > "$1/crusty-records.tsv"
    printf 'crusty-loop\t2026-10-05T00:00:00Z\n' > "$1/phases.tsv" )
}
# phase2_hashes <dir> -> one line per seeded phase-2 file: its name and hash.
phase2_hashes() {
  local f
  for f in crusty-round-1.json crusty-round-1.json.findings crusty-round-1.log; do
    printf '%s %s\n' "$f" "$(git hash-object --no-filters --stdin < "$1/$f" 2>/dev/null || echo missing)"
  done
}
# manifest_resolves <dir> -> 0 when every crusty-records.tsv row names a file
# in <dir> whose blob hash is the row's hash, and no label appears twice.
manifest_resolves() {
  local label file hash seen=" "
  while IFS=$'\t' read -r label file hash; do
    [ -n "$label" ] || continue
    case "$seen" in *" $label "*) return 1 ;; esac
    seen="$seen$label "
    [ -f "$1/$file" ] && [ "$(git hash-object --no-filters --stdin < "$1/$file")" = "$hash" ] || return 1
  done < "$1/crusty-records.tsv"
}
# merge_loop <head> <state-dir> <calls-file>: the merge-ready loop as
# autodrive-merge-loop.yaml step-02 runs it, from a step at session depth 2.
merge_loop() {
  git -C "$FX" update-ref --no-deref HEAD "$1"
  env -i HOME="$WORK/home" PATH="$WORK/bin:$SYS_PATH" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    AMPLIHACK_BIN="$WORK/bin/amplihack" REAL_AMPLIHACK="$REAL" FIXTURE="$FX" CALLS="$3" \
    AMPLIHACK_SESSION_DEPTH=2 AMPLIHACK_MAX_DEPTH=3 AMPLIHACK_TREE_ID=scenario \
    bash "$TOOLS/autodrive_loop.sh" --loop-name merge-ready --round-recipe autodrive-merge-round \
      --clean-token MERGE_READY --verdict-field merge_ready_verdict --repo "$FX" --state-dir "$2" \
      --context "repo_path=$FX" --context "pr_number=42" --context "autodrive_state_dir=$2" \
      --before-round "$TOOLS/autodrive_crusty_rereview.sh"
}
crusty_final() { # crusty_final <dir> -> the head criterion 3 accepts, or a token
  env -i PATH="$SYS_PATH" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    bash -c '. "$1/autodrive_state.sh"; . "$1/autodrive_trust.sh"; autodrive_crusty_final "$2"' _ "$TOOLS" "$1" 2>/dev/null
}

# 1. A code commit after the clean crusty round: crusty reviews it BEFORE the
#    merge round, both rounds are started at the loop's depth, criterion 3
#    then holds for the new head, and the loop converges.
S1="$WORK/state-code"; seed "$S1"
P2_BEFORE="$(phase2_hashes "$S1")"
P2_ROW="$(cat "$S1/crusty-records.tsv")"
OUT="$(merge_loop "$C" "$S1" "$WORK/calls-code" 2>"$WORK/err-code")"; RC=$?
WANT="autodrive-crusty-round depth=2
loop-health-evaluator depth=2
autodrive-merge-round depth=2
loop-health-evaluator depth=2"
if [ "$RC" -eq 0 ] && [ "$(cat "$WORK/calls-code" 2>/dev/null)" = "$WANT" ] && printf '%s' "$OUT" | grep -qF '"loop_result":"DONE"'; then
  pass code_commit_rereviewed_before_merge_round_at_loop_depth \
    "crusty re-reviews the code commit before the merge round, and both rounds start at depth 2, the loop's own"
else
  fl code_commit_rereviewed_before_merge_round_at_loop_depth \
    "rc=$RC calls=$(tr '\n' '|' < "$WORK/calls-code" 2>/dev/null) out=$OUT err=$(tail -n 5 "$WORK/err-code" | tr '\n' ' ')"
fi
if [ "$(crusty_final "$S1")" = "$C" ]; then
  pass criterion3_holds_for_rereviewed_head "after the re-review the crusty record trusted for criterion 3 is the one that reviewed the new head"
else
  fl criterion3_holds_for_rereviewed_head "autodrive_crusty_final gave '$(crusty_final "$S1")', want $C"
fi
# The re-review's crusty loop shares phase 2's state dir. Its rounds are
# labelled after phase 2's, so phase 2's record, findings and log are left
# exactly as they were, and the manifest keeps phase 2's row and gains one
# row for round-2, every row naming a file that still hashes to it.
if [ "$(phase2_hashes "$S1")" = "$P2_BEFORE" ]; then
  pass phase2_crusty_round_kept "the re-review left phase 2's crusty-round-1.json, its .findings and its .log unchanged"
else
  fl phase2_crusty_round_kept "before: $(printf '%s' "$P2_BEFORE" | tr '\n' '|') after: $(phase2_hashes "$S1" | tr '\n' '|')"
fi
WANT_ROWS="${P2_ROW}
round-2	crusty-round-2.json	$(git hash-object --no-filters "$S1/crusty-round-2.json" 2>/dev/null)"
if [ "$(cat "$S1/crusty-records.tsv")" = "$WANT_ROWS" ] && manifest_resolves "$S1" \
   && grep -qF '"round_label":"round-2"' "$S1/crusty-round-2.json" 2>/dev/null; then
  pass rereview_rounds_continue_the_manifest "the re-review's round is labelled round-2, and every row of crusty-records.tsv names a record that still hashes to it"
else
  fl rereview_rounds_continue_the_manifest "rows: $(tr '\t\n' ' |' < "$S1/crusty-records.tsv") files: $(ls "$S1" | tr '\n' ' ')"
fi

# 2. A description-only commit needs no re-review: the merge round runs alone.
S2="$WORK/state-desc"; seed "$S2"
OUT="$(merge_loop "$D" "$S2" "$WORK/calls-desc" 2>"$WORK/err-desc")"; RC=$?
if [ "$RC" -eq 0 ] && ! grep -q '^autodrive-crusty-round' "$WORK/calls-desc" && grep -q '^autodrive-merge-round depth=2$' "$WORK/calls-desc" \
   && [ "$(crusty_final "$S2")" = "$R" ]; then
  pass description_commit_needs_no_rereview "a PR_DESCRIPTION.md commit is on the allowlist: no crusty run, criterion 3 still holds"
else
  fl description_commit_needs_no_rereview "rc=$RC calls=$(tr '\n' '|' < "$WORK/calls-desc" 2>/dev/null) err=$(tail -n 5 "$WORK/err-desc" | tr '\n' ' ')"
fi

# 3. The real recursion guard, ceiling 3 sealed in a private tree store. A
#    stand-in recipe-runner-rs records the depth the guard hands it.
TREES="$WORK/trees"; mkdir -p "$TREES" "$WORK/tmp"
printf '{"sessions":{},"ceiling":3}' > "$TREES/scenario.json"
printf '#!/bin/sh\nprintf "depth=%%s\\n" "${AMPLIHACK_SESSION_DEPTH:-}" >> "$RUNNER_LOG"\nprintf "{}\\n"\n' > "$WORK/bin/recipe-runner-rs"
chmod +x "$WORK/bin/recipe-runner-rs"
guard() { # guard <depth> -> exit code of a real `recipe run` of the crusty round started at <depth>
  ( cd "$WORK" && env -i HOME="$WORK/home" PATH="$SYS_PATH" TMPDIR="$WORK/tmp" AMPLIHACK_SKIP_AUTO_INSTALL=1 \
      AMPLIHACK_SESSION_TREE_DIR="$TREES" AMPLIHACK_TREE_ID=scenario AMPLIHACK_SESSION_DEPTH="$1" AMPLIHACK_MAX_DEPTH=3 \
      AMPLIHACK_MIN_AVAILABLE_MIB=0 RECIPE_RUNNER_RS_PATH="$WORK/bin/recipe-runner-rs" RUNNER_LOG="$WORK/runner-$1.log" \
      "$REAL" recipe run "$REPO_ROOT/amplifier-bundle/recipes/autodrive-crusty-round.yaml" --dry-run ) \
    >"$WORK/guard-$1.out" 2>"$WORK/guard-$1.err"
}
# Depth 4 is a merge round's step: where the nested step-00c crusty loop ran.
guard 4; RC=$?
if [ "$RC" -eq 79 ] && grep -qF 'BLOCKED_TERMINAL orchestration_unavailable: depth 4 of max 3' "$WORK/guard-4.err" \
   && [ ! -e "$WORK/runner-4.log" ]; then
  pass round_step_depth_refused_by_real_guard "a crusty round started from a merge round step (depth 4) is refused with exit 79, before any runner starts"
else
  fl round_step_depth_refused_by_real_guard "rc=$RC err=$(tail -n 3 "$WORK/guard-4.err" | tr '\n' ' ')"
fi
# Depth 2 is the merge-ready loop's step, where the re-review runs now.
guard 2; RC=$?
if ! grep -qF 'BLOCKED_TERMINAL' "$WORK/guard-2.err" && [ "$(cat "$WORK/runner-2.log" 2>/dev/null)" = "depth=3" ]; then
  pass loop_depth_admitted_by_real_guard "a crusty round started from the loop's depth (2) is admitted, and its runner starts at depth 3"
else
  fl loop_depth_admitted_by_real_guard "rc=$RC runner=$(cat "$WORK/runner-2.log" 2>/dev/null) err=$(tail -n 3 "$WORK/guard-2.err" | tr '\n' ' ')"
fi

if [ "$fail" -eq 0 ]; then
  echo "crusty re-review runs before the merge round at the loop's depth"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
