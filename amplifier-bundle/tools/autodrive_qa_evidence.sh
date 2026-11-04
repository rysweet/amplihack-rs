#!/usr/bin/env bash
# autodrive_qa_evidence.sh — measure merge-ready criterion 1 for one round of
# the auto-drive-to-merge merge loop. step-02-qa-team-scenarios of
# autodrive-merge-evidence.yaml runs this file (moved out of the step, PR #1520).
#
# QA uses the selected suite plus gadugi where required (Node or an explicit scenario directory).
# Input comes from the environment only:
#   REPO_PATH              the repository to measure (default: the working directory)
#   AUTODRIVE_QA_EVIDENCE  file the evidence is written to, private (optional)
#   AUTODRIVE_ROUND_LABEL  recorded as qa_round
#   AUTODRIVE_QA_BASE      the base branch name, for the changed paths
#   AUTODRIVE_QA_COMMAND, AUTODRIVE_QA_DIR, AUTODRIVE_QA_COMMANDS and
#   AUTODRIVE_QA_SCENARIO_DIR, described where they are read below.
# Output: one JSON line on stdout, and the same line in AUTODRIVE_QA_EVIDENCE.
# qa_status is PASS only when every measurement that applies passes. Exit 0
# whenever evidence was produced, PASS or not; exit 1 when REPO_PATH, a
# temporary file or the helpers beside this file are unavailable, so that no
# evidence exists. Executed, never sourced; the helpers are sourced from
# beside this file and from nowhere else.
# NO-TIMEOUT POLICY (issue #439): a test suite is never cut off mid-run.
set +aCevx; set -uo pipefail
# Resolve helpers before cd; CDPATH must not redirect relative directories (#1516).
unset CDPATH
H="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
{ [ -n "$H" ] && [ -f "$H/autodrive_state.sh" ] && [ -f "$H/autodrive_trust.sh" ]; } \
  || { echo "ERROR: autodrive-qa-helpers-not-found: autodrive_state.sh or autodrive_trust.sh is missing" >&2; exit 1; }
cd "${REPO_PATH:-.}" 2>/dev/null || { echo "ERROR: cannot cd to REPO_PATH" >&2; exit 1; }
ROOT="$(pwd -P)"
EVIDENCE="${AUTODRIVE_QA_EVIDENCE:-}"
# One shell asks every scope question (autodrive_qa_report), so the diff and the tree walk
# run once (#1763). Without autodrive_qa_sources.sh or the scope file the evidence is BLOCKED.
SCAN="$(. "$H/autodrive_trust.sh" || exit 1
  S="$H/autodrive_qa_sources.sh"
  # shellcheck source=autodrive_qa_sources.sh # a regular file beside this tool, never a symlink
  if [ -n "${_AD_SCOPE_OK:-}" ] && [ -f "$S" ] && [ ! -L "$S" ] && . "$S" \
     && declare -F autodrive_qa_report >/dev/null; then
    autodrive_qa_report "$ROOT"
  else
    printf 'noscope\t1\nkind\t%s\ngreq\t%s\ntarget\t%s\n' "$(autodrive_repo_type "$ROOT")" \
      "$(autodrive_gadugi_required "$ROOT")" "$(autodrive_qa_target "$ROOT")"
  fi)" || SCAN=""
KIND=""; GREQ=""; NOSCOPE=""; TARGET=""; PATHS="(unknown)"; DSRC=""; DDIR=""; DCMD=""; DBAD=""; CISKIP=""
while IFS=$'\t' read -r key val; do
  case "$key" in
    kind) KIND="$val" ;;
    greq) GREQ="$val" ;;
    noscope) NOSCOPE="1" ;;
    target) TARGET="$val" ;;
    paths) PATHS="$val" ;;
    source) DSRC="$val" ;;
    dir) DDIR="$val" ;;
    command) DCMD="$val" ;;
    invalid) DBAD="$val" ;;
    ciskip) CISKIP="$val" ;;
  esac
done <<< "$SCAN"
case "${KIND}:${GREQ}" in
  rust-cli:true|rust-cli:false|node:true|python:true|python:false|unknown:true|unknown:false) ;;
  *) echo "ERROR: autodrive-qa-helpers-not-found: no repository type (${KIND:-none}:${GREQ:-none})" >&2
     exit 1 ;;
esac
[ -z "$NOSCOPE" ] || PATHS="(unknown)"
# Validate the scope target again; unsafe or disagreeing targets cannot select a command.
TKIND="${TARGET%%$'\t'*}"; TDIR="${TARGET#*$'\t'}"; UNRES=""; TBAD=""
case "$TARGET" in *$'\t'*) ;; *) TBAD="1" ;; esac
case "$TKIND" in
  unresolved) UNRES="$TDIR"; TDIR="."; [ "$KIND" = "unknown" ] || TBAD="1" ;;
  rust-cli|node|python|unknown) [ "$TKIND" = "$KIND" ] || TBAD="1" ;;
  *) TBAD="1" ;;
esac
case "$TDIR" in ''|/*|*//*|..|../*|*/..|*/../*|*[!A-Za-z0-9._/-]*) TBAD="1" ;; esac
if [ -n "$TBAD" ]; then
  echo "ERROR: autodrive_qa_target disagreed with autodrive_repo_type or gave an unsafe directory." >&2
  KIND="unknown"; TDIR="."; UNRES=""
fi
# Strip JSON/control bytes and ANSI CSI; normalise whitespace and backslashes (#1531).
san() { LC_ALL=C sed "s|$(printf '\033')\[[0-?]*[ -/]*[@-~]||g" | LC_ALL=C tr '\t\r\n\\' '   /' \
  | LC_ALL=C tr -d '\000-\037\177"'; }
# Text cut to a byte length is made ASCII first: a cut through a multibyte
# character leaves invalid UTF-8, which every JSON reader rejects.
ascii() { LC_ALL=C tr -d '\200-\377'; }
CAUSES=""
cause() { CAUSES="${CAUSES}${CAUSES:+; }$1"; }
# The evidence binds to the tree it was measured on: the merge gate refuses evidence
# whose head_sha is not the SHA being merged, so an earlier round's PASS cannot stand in.
HEAD_SHA=""
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  HEAD_SHA="$(git rev-parse HEAD 2>/dev/null || printf '')"
else
  echo "[skip] not a git repo; the qa evidence records no head sha, so the merge gate refuses it rather than merging unbound evidence" >&2
fi
res() { printf '%s\t%s\n' "${1##*/}" "$2" >> "$RES"; } # res <file> <PASS|FAIL|INVALID>
# --- suite commands --------------------------------------------------
# First present source wins: env (even empty), committed repo-file, CI, detected.
# Set but empty means "no command", never "detect one":
#   AUTODRIVE_QA_COMMAND   one command, word-split with globbing off, run in
#                          AUTODRIVE_QA_DIR (the issue #1516 semantics)
#   AUTODRIVE_QA_COMMANDS  one command per line (blank and # lines skipped),
#                          each run alone with bash -c from the repository root
# qa_dir records where the word or detected command ran, else `.`.
QA_DIR="$TDIR"; QD="."; SRC=""; CFGBAD=""; QUNRES=""
TYPE="unknown"; KINDS=(); TEXTS=()
if [ -n "$NOSCOPE" ]; then
  echo "ERROR: autodrive_qa_scope.sh or autodrive_qa_sources.sh did not load; no QA command source can be read." >&2
  cause "the QA scope helper autodrive_qa_scope.sh or autodrive_qa_sources.sh is missing"
elif [ -n "${AUTODRIVE_QA_COMMAND+x}" ] || [ -n "${AUTODRIVE_QA_COMMANDS+x}" ]; then
  TYPE="configured"; QA_DIR="${AUTODRIVE_QA_DIR:-.}"; SRC="env"
  [ -z "${AUTODRIVE_QA_COMMAND:-}" ] || { KINDS+=(word); TEXTS+=("$AUTODRIVE_QA_COMMAND"); QD="$QA_DIR"; }
  while IFS= read -r line || [ -n "$line" ]; do
    case "$(printf '%s' "$line" | sed 's/^[[:space:]]*//')" in ''|'#'*) continue ;; esac
    KINDS+=(entry); TEXTS+=("$line")
  done <<< "${AUTODRIVE_QA_COMMANDS:-}"
else
  if [ -n "$DBAD" ]; then
    TYPE="configured"; CFGBAD="$DBAD"
    case "$CFGBAD" in
      symlink|not-regular|too-large|syntax|unknown-key|duplicate-key|missing-command|unsafe-command) ;;
      unsafe-dir|dir-not-found|changed-in-pr|paths-empty|paths-unknown) ;;
      *) CFGBAD="syntax" ;;
    esac
    cause ".amplihack/qa.toml: ${CFGBAD}"
  elif [ -n "$DCMD" ] && { [ "$DSRC" = "repo-file" ] || [ "$DSRC" = "ci" ]; }; then
    TYPE="configured"; SRC="$DSRC"; QA_DIR="$DDIR"; QD="$QA_DIR"
    KINDS+=(word); TEXTS+=("$DCMD")
  fi
  if [ -z "$SRC" ] && [ -z "$CFGBAD" ]; then
    TYPE="$KIND"
    if [ -n "$UNRES" ]; then
      QUNRES="$UNRES"; QD=""
      cause "unresolved QA project candidates: $(printf '%s' "$UNRES" | san | ascii | cut -c1-1500)"
    else
      case "$KIND" in
        rust-cli) KINDS+=(detected); TEXTS+=("cargo test --workspace --locked --no-fail-fast") ;;
        node) KINDS+=(detected); TEXTS+=("npm test") ;;
        python) KINDS+=(detected); TEXTS+=("pytest") ;;
      esac
      [ "${#TEXTS[@]}" -eq 0 ] || { QD="$QA_DIR"; SRC="detected"; }
    fi
  fi
fi
# Read selection/evidence controls above; isolate only the execution children.
# Scratch ownership comes from this allocation, never a caller variable.
QA_TMP=""; NEW_DIRS=""
qa_clean_path() {
  local d m
  d="$(cd -- "$1" 2>/dev/null && pwd -P)" && [ -n "$d" ] || return 1
  case "$d/" in "$ROOT/"*) return 1 ;; esac
  while :; do
    for m in .claude src/amplihack/.claude amplifier-bundle; do
      { [ -e "$d/$m" ] || [ -L "$d/$m" ]; } && return 1
    done
    [ "$d" != / ] || break; d="${d%/*}"; [ -n "$d" ] || d=/
  done
}
qa_cleanup() {
  local d rc=0
  for d in $NEW_DIRS; do
    if [ -d "$d" ] && [ ! -L "$d" ]; then rm -rf -- "./$d" || rc=1; fi
  done
  if [ -n "$QA_TMP" ] && [ -d "$QA_TMP" ] && [ ! -L "$QA_TMP" ]; then
    rm -rf -- "$QA_TMP" || rc=1
  fi
  return "$rc"
}
trap 'qa_cleanup || exit 1' EXIT
trap 'exit 1' INT TERM
QA_BASE=""
for d in /tmp /var/tmp; do qa_clean_path "$d" && { QA_BASE="$(cd -- "$d" && pwd -P)"; break; }; done
[ -n "$QA_BASE" ] || { echo "ERROR: no QA scratch base without framework ancestry" >&2; exit 1; }
QA_TMP="$(mktemp -d "$QA_BASE/autodrive-qa.XXXXXX")" || { echo "ERROR: cannot create a temporary log" >&2; echo "ERROR: cannot allocate QA scratch" >&2; exit 1; }
qa_clean_path "$QA_TMP" || { echo "ERROR: QA scratch has unsafe ancestry" >&2; exit 1; }
qa_child() (
  local v
  # Preserve tool/build configuration and explicit artifacts, remove recipe selectors.
  while IFS= read -r v; do
    case "$v" in AMPLIHACK_*|AUTODRIVE_*|RECIPE_*|REPO_PATH|BASH_ENV|ENV|ZDOTDIR|CDPATH|GLOBIGNORE) unset "$v" || exit 1 ;; esac
  done < <(compgen -e)
  while IFS= read -r v; do unset -f "$v" || exit 1; done < <(compgen -A function)
  export -n SHELLOPTS BASHOPTS QA_TMP STAGE_ROOT
  export TMPDIR="$QA_TMP" TMP="$QA_TMP" TEMP="$QA_TMP" CARGO_TARGET_DIR="$QA_TMP/target"
  "$@"
)
LOG="$(mktemp "$QA_TMP/autodrive-qa-XXXXXX")" || { echo "ERROR: cannot create a temporary log" >&2; exit 1; }
CMDTEXT=""; RAN=0; FIRST_NZ=""; S_FAILED="false"; S_MISSING="false"; S_NOTINST="false"
if [ "${#TEXTS[@]}" -eq 0 ] && [ -z "${QUNRES}${CFGBAD}" ]; then
  S_MISSING="true"; [ -n "$NOSCOPE" ] || cause "no repository test command for repository type ${TYPE}"
fi
i=0
while [ "$i" -lt "${#TEXTS[@]}" ]; do
  kind="${KINDS[$i]}"; text="${TEXTS[$i]}"; i=$((i + 1))
  CMDTEXT="${CMDTEXT}${CMDTEXT:+; }${text}"
  if [ "$kind" = "entry" ]; then
    # Its own process from the repository root: a `cd` in one entry
    # never reaches the next, and entries are never joined or sourced.
    ( cd -- "$ROOT" && qa_child bash -c "$text" ) >>"$LOG" 2>&1 </dev/null
    rc=$?
  else
    dir="$QA_DIR"
    first="$(set -f; set -- $text; printf '%s' "${1:-}")"
    # A missing AUTODRIVE_QA_DIR is named, never "<cmd> not installed" (#1516).
    if [ "$kind" = "word" ] && [ ! -d "$dir" ]; then
      S_MISSING="true"; cause "AUTODRIVE_QA_DIR $(printf '%s' "$dir" | san | ascii | cut -c1-120) not found"
      continue
    fi
    # Probe from qa_dir in the same clean environment used for execution.
    # Branch scripts/build.rs remain trusted; no timeout or executable-location restriction.
    case "$dir" in /*) ;; *) dir="./$dir" ;; esac
    if ! ( cd -- "$dir" 2>/dev/null && { qa_child command -v -- "$first" >/dev/null 2>&1 \
           || { [ "$kind" = "word" ] && [ -x "./${first}" ]; }; } ); then
      S_NOTINST="true"; cause "$(printf '%s' "$first" | san | cut -c1-80) not installed"
      continue
    fi
    # shellcheck disable=SC2086
    ( set -f; cd -- "$dir" && qa_child $text ) >>"$LOG" 2>&1 </dev/null
    rc=$?
  fi
  RAN=$((RAN + 1))
  echo "INFO: qa-team (${TYPE}) suite command exited ${rc}: $(printf '%s' "$text" | san | cut -c1-200)" >&2
  if [ "$rc" -ne 0 ]; then S_FAILED="true"; [ -n "$FIRST_NZ" ] || FIRST_NZ="$rc"; fi
done
[ "$S_FAILED" = "true" ] && cause "repository test failure"
RC=""; [ "$RAN" -gt 0 ] && RC="${FIRST_NZ:-0}"
TAIL="$(tail -n 5 "$LOG" | san | ascii | cut -c1-300)"
tail -n 60 "$LOG" >&2 || true
rm -f "$LOG"
# --- the scenario directory ------------------------------------------
# Resolve scenarios only when required; Rust parity fixtures are informational.
COUNT=0; SCEN=""; FILES=(); SYMLINKED=""; NONREG=""; SYMCOUNT=0; ODD=(); SDIR=""; SREL=""
if [ "$GREQ" = "true" ]; then
  SDIR_IN="${AUTODRIVE_QA_SCENARIO_DIR:-}"
  if [ -z "$SDIR_IN" ]; then
    SDIR_IN="tests/agentic"
    for d in tests/agentic tests/gadugi/scenarios scenarios; do [ -d "${ROOT}/${d}" ] && { SDIR_IN="$d"; break; }; done
  fi
  [ "$SDIR_IN" = "/" ] || SDIR_IN="${SDIR_IN%/}"
  case "$SDIR_IN" in /*) SDIR="$SDIR_IN" ;; *) SDIR="${ROOT}/${SDIR_IN}" ;; esac
  case "$SDIR" in "${ROOT}/"*) SREL="${SDIR#"${ROOT}/"}" ;; *) SREL="$SDIR" ;; esac
elif [ "$KIND" = "rust-cli" ] && [ -d "${ROOT}/${TDIR}/tests/parity/scenarios" ]; then
  # qa-team's Rust CLI parity fixtures of the detected workspace, named from
  # the repository root; cargo test runs them. Informational: nothing
  # decides a pass from the list.
  PFX="tests/parity/scenarios"; [ "$TDIR" = "." ] || PFX="${TDIR}/${PFX}"
  while IFS= read -r f; do
    [ -n "$f" ] && SCEN="${SCEN}${SCEN:+ }${PFX}/${f##*/}"
  done < <(find "${ROOT}/${TDIR}/tests/parity/scenarios" -maxdepth 1 -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | LC_ALL=C sort)
fi
# Top-level regular files only: gadugi-test validate -d does not read
# subdirectories. A symlinked or non-regular *.yaml entry is never run, so
# it is named here and, once validate passes, fails like a failed run.
if [ -n "$SDIR" ] && [ -d "$SDIR" ]; then
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    COUNT=$((COUNT + 1)); SCEN="${SCEN}${SCEN:+ }${SREL}/${f##*/}"; FILES+=("$f")
  done < <(find "$SDIR" -maxdepth 1 -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | LC_ALL=C sort)
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    SYMCOUNT=$((SYMCOUNT + 1)); ODD+=("$f"); k="symlinked scenario"
    if [ -L "$f" ]; then SYMLINKED="${SYMLINKED}${SYMLINKED:+ }${SREL}/${f##*/}"; else NONREG="${NONREG}${NONREG:+ }${SREL}/${f##*/}"; k="non-regular scenario entry"; fi
    echo "WARNING: ${k} not run: $(printf '%s' "${SREL}/${f##*/}" | san)" >&2
  done < <(find "$SDIR" -mindepth 1 -maxdepth 1 ! -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | LC_ALL=C sort)
fi

# --- gadugi-test -----------------------------------------------------
RES="$(mktemp "$QA_TMP/autodrive-results.XXXXXX" 2>/dev/null)" || { echo "ERROR: cannot create a temporary log" >&2; exit 1; }
# Missing gadugi stops its checks; failed validate stops runs. No extra time limit.
G_STATUS=""; G_VRC=""; G_RRC=""; RUN_NZ=""; VALIDATED=0; RUNS=0; PASSED=0; FAILED=0
UNNAMED=""; RUNFAIL=""; STAGEFAIL=""; SYMFAIL=""
if [ "$GREQ" = "false" ]; then
  G_STATUS="NOT_REQUIRED"
  echo "INFO: gadugi-test is not part of criterion 1 in a ${KIND} repository (qa-team's repo-type table); AUTODRIVE_QA_SCENARIO_DIR asks for it" >&2
elif ! qa_child command -v gadugi-test >/dev/null 2>&1; then
  G_STATUS="NOT_INSTALLED"; cause "gadugi-test not installed"
elif [ "$COUNT" = "0" ]; then
  # gadugi-test run exits 0 on an empty directory, so zero is checked here.
  G_STATUS="NO_SCENARIOS"; cause "no scenarios in $(printf '%s' "$SREL" | san)"
else
  GLOG="$(mktemp "$QA_TMP/autodrive-gadugi-XXXXXX")" || { echo "ERROR: cannot create a temporary log" >&2; rm -f -- "$RES"; exit 1; }
  # gadugi-test writes logs/ and outputs/ into its working directory. Only
  # the ones this step creates are removed afterwards, so the artifact
  # guard does not block the next commit on this run's leftovers.
  NEW_DIRS=""
  for d in logs outputs; do [ -e "$d" ] || [ -L "$d" ] || NEW_DIRS="${NEW_DIRS} ${d}"; done
  qa_child gadugi-test validate -d "$SDIR" >"$GLOG" 2>&1 </dev/null
  G_VRC=$?
  if [ "$G_VRC" -ne 0 ]; then
    G_STATUS="VALIDATE_FAILED"; cause "gadugi-test validation failure"
    for f in ${FILES[@]+"${FILES[@]}"}; do res "$f" INVALID; done
  else
    VALIDATED="$COUNT"
    # Each scenario runs alone: shared CLI runners kill concurrent siblings (#207).
    # Stage only outside the checkout, beneath this invocation's scratch.
    STAGE_ROOT="$(mktemp -d "$QA_TMP/autodrive-gadugi.XXXXXX" 2>/dev/null)" || STAGE_ROOT=""
    STAGE_OK="false"
    if [ -n "$STAGE_ROOT" ] && [ -d "$STAGE_ROOT" ]; then
      case "$(cd -- "$STAGE_ROOT" && pwd -P)/" in
        "${ROOT}/"*) echo "WARNING: the temporary directory is inside the repository; no scenario is staged there." >&2 ;;
        *) STAGE_OK="true" ;;
      esac
    fi
    for f in ${FILES[@]+"${FILES[@]}"}; do
      rel="${SREL}/${f##*/}"
      # The scenario's name, from autodrive_scenario_name.sh beside this file.
      name="$(qa_child bash "$H/autodrive_scenario_name.sh" "$f" 2>/dev/null)" || name=""
      # A name gadugi-test could read as an option, or one carrying a
      # control byte, or one over 200 bytes, is no name: the file fails.
      ok="true"
      case "$name" in ''|-*) ok="false" ;; esac
      printf '%s' "$name" | LC_ALL=C grep -q '[[:cntrl:]]' && ok="false"
      [ "$(printf '%s' "$name" | LC_ALL=C wc -c | tr -d ' ')" -le 200 ] || ok="false"
      if [ "$ok" != "true" ]; then
        UNNAMED="${UNNAMED}${UNNAMED:+ }${rel}"; FAILED=$((FAILED + 1)); res "$f" INVALID; continue
      fi
      stage=""
      [ "$STAGE_OK" = "true" ] && stage="$(mktemp -d "${STAGE_ROOT}/scenario.XXXXXX" 2>/dev/null)" \
        && cp -P -- "$f" "${stage}/" 2>/dev/null || stage=""
      if [ -z "$stage" ]; then
        STAGEFAIL="${STAGEFAIL}${STAGEFAIL:+ }${rel}"; FAILED=$((FAILED + 1)); res "$f" FAIL; continue
      fi
      (cd -- "$ROOT" && qa_child gadugi-test run -d "$stage" --scenario "$name") >>"$GLOG" 2>&1 </dev/null
      rrc=$?
      RUNS=$((RUNS + 1))
      if [ "$rrc" -eq 0 ]; then PASSED=$((PASSED + 1)); res "$f" PASS
      else FAILED=$((FAILED + 1)); RUNFAIL="${RUNFAIL}${RUNFAIL:+ }${rel}"; [ -n "$RUN_NZ" ] || RUN_NZ="$rrc"; res "$f" FAIL; fi
    done
    [ "$RUNS" -gt 0 ] && G_RRC="${RUN_NZ:-0}"
    [ "$SYMCOUNT" -gt 0 ] && { SYMFAIL="${SYMLINKED} ${NONREG}"; FAILED=$((FAILED + SYMCOUNT)); }
    [ -n "$UNNAMED" ] && cause "gadugi scenario without a name: ${UNNAMED}"
    [ -n "$STAGEFAIL" ] && cause "gadugi scenario could not be staged: ${STAGEFAIL}"
    [ -n "$RUNFAIL" ] && cause "gadugi scenario run failure: ${RUNFAIL}"
    if [ -n "${UNNAMED}${STAGEFAIL}${RUNFAIL}${SYMFAIL}" ]; then G_STATUS="RUN_FAILED"; else G_STATUS="PASS"; fi
  fi
  for f in ${ODD[@]+"${ODD[@]}"}; do res "$f" INVALID; done
  echo "INFO: gadugi-test on ${SREL} (${COUNT} scenario(s), ${RUNS} run, ${PASSED} passed): ${G_STATUS}" >&2
  tail -n 60 "$GLOG" >&2 || true
  rm -f "$GLOG"
fi
[ -n "$SYMLINKED" ] && cause "symlinked scenario not run: ${SYMLINKED}"
[ -n "$NONREG" ] && cause "non-regular scenario entry not run: ${NONREG}"
RESULTS="$(. "$H/autodrive_state.sh" && . "$H/autodrive_trust.sh" && autodrive_scenario_results "$RES")" || RESULTS=""
rm -f -- "$RES"

# --- qa_status and qa_reason -----------------------------------------
# qa_reason is the first token that applies, in this fixed order; the
# FAIL tokens come first because the merge round can act on them.
REASON=""
pick() { [ -n "$REASON" ] || REASON="$1"; }
# A missing scope helper, QA_UNRESOLVED and an invalid qa.toml come first: no
# suite ran, an operator must act, and gadugi is then named in qa_summary only.
[ -n "$NOSCOPE" ] && pick "qa-command-missing"
[ -n "$QUNRES" ] && pick "qa-unresolved"
[ -n "$CFGBAD" ] && pick "qa-config-invalid"
[ "$S_FAILED" = "true" ] && pick "qa-command-failed"
[ "$G_STATUS" = "NO_SCENARIOS" ] && pick "no-scenarios"
[ "$G_STATUS" = "VALIDATE_FAILED" ] && pick "gadugi-validate-failed"
[ -n "$UNNAMED" ] && pick "gadugi-scenario-unnamed"
[ -n "${RUNFAIL}${STAGEFAIL}${SYMFAIL}" ] && pick "gadugi-run-failed"
[ "$S_MISSING" = "true" ] && pick "qa-command-missing"
[ "$S_NOTINST" = "true" ] && pick "qa-command-not-installed"
[ "$G_STATUS" = "NOT_INSTALLED" ] && pick "gadugi-test-missing"
# The gadugi half is complete when it is not required (set only where GREQ
# is false), or when every scenario found ran and passed.
G_DONE="false"
{ [ "$G_STATUS" = "NOT_REQUIRED" ] || { [ "$G_STATUS" = "PASS" ] && [ "$PASSED" = "$COUNT" ]; }; } && G_DONE="true"
if [ -z "$REASON" ] && { [ "$RAN" -eq 0 ] || [ "$G_DONE" != "true" ]; }; then
  echo "ERROR: no check failed by name, yet the evidence is incomplete (suite commands run=${RAN}, gadugi=${G_STATUS}); refusing to report PASS." >&2
  REASON="qa-command-missing"
fi
case "$REASON" in
  '') STATUS="PASS" ;;
  qa-command-failed|no-scenarios|gadugi-validate-failed|gadugi-scenario-unnamed|gadugi-run-failed) STATUS="FAIL" ;;
  qa-config-invalid|qa-command-missing|qa-command-not-installed|gadugi-test-missing) STATUS="BLOCKED" ;;
  qa-unresolved) STATUS="QA_UNRESOLVED" ;;
  *) echo "ERROR: unknown qa_reason '${REASON}'; reporting FAIL." >&2; STATUS="FAIL"; REASON="gadugi-run-failed" ;;
esac
SUMMARY="$(printf '%s' "${CAUSES}${CAUSES:+${TAIL:+: }}${TAIL}" | san)"
# The evidence format: every field a string, so a reader never has to guess a type.
F='{"qa_status":"%s","qa_reason":"%s","qa_repo_type":"%s","qa_dir":"%s","qa_command":"%s",'
F="${F}"'"qa_command_source":"%s","qa_command_paths":"%s","qa_candidates":"%s","qa_ci_skip":"%s",'
F="${F}"'"qa_suite_commands_count":"%s","qa_scenarios":"%s","qa_exit_code":"%s",'
F="${F}"'"qa_summary":"%s","qa_round":"%s","head_sha":"%s","gadugi_required":"%s","gadugi_status":"%s",'
F="${F}"'"gadugi_validate_exit_code":"%s","gadugi_run_exit_code":"%s",'
F="${F}"'"gadugi_scenario_count":"%s","gadugi_scenario_dir":"%s",'
F="${F}"'"gadugi_scenarios_validated":"%s","gadugi_scenarios_run":"%s",'
F="${F}"'"gadugi_scenarios_passed":"%s","gadugi_scenarios_failed":"%s",'
F="${F}"'"gadugi_failed_scenarios":"%s","gadugi_scenario_results":"%s"}\n'
# shellcheck disable=SC2059 # F is the fixed format above, never input
OUT="$(printf "$F" \
  "$STATUS" "$REASON" "$TYPE" "$(printf '%s' "$QD" | san | ascii | cut -c1-200)" "$(printf '%s' "$CMDTEXT" | san | ascii | cut -c1-500)" \
  "$SRC" "$(printf '%s' "$PATHS" | san | ascii)" "$(printf '%s' "$QUNRES" | san | ascii)" \
  "$(printf '%s' "$CISKIP" | san | ascii | cut -c1-40)" "$RAN" \
  "$(printf '%s' "$SCEN" | san)" "$RC" "$SUMMARY" "$(printf '%s' "${AUTODRIVE_ROUND_LABEL:-round}" | san)" "$HEAD_SHA" \
  "$GREQ" "$G_STATUS" "$G_VRC" "$G_RRC" "$COUNT" "$(printf '%s' "$SREL" | san)" \
  "$VALIDATED" "$RUNS" "$PASSED" "$FAILED" \
  "$(printf '%s' "${UNNAMED} ${STAGEFAIL} ${RUNFAIL} ${SYMFAIL}" | san | tr -s ' ' | sed 's/^ //; s/ $//')" \
  "$(printf '%s' "$RESULTS" | san)")"
# Cleanup must succeed before publishing fresh evidence, even for failing commands.
qa_cleanup || { echo "ERROR: cannot clean QA scratch" >&2; exit 1; }
# Private to this user (#1517 D5), and never written through a planted symlink.
[ -z "$EVIDENCE" ] || ( umask 077; rm -f -- "$EVIDENCE"; printf '%s' "$OUT" > "$EVIDENCE" )
printf '%s\n' "$OUT"
