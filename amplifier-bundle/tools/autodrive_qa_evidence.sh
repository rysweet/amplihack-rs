#!/usr/bin/env bash
# autodrive_qa_evidence.sh — measure the facts merge-ready criterion 1 is
# judged from, for one round of the auto-drive-to-merge merge loop.
# step-02a-qa-inputs of autodrive-merge-evidence.yaml runs this file.
#
# This file MEASURES; it decides nothing. Criterion 1 itself is a judgement an
# agent step records inside the loop (#2569): the agent reads this record,
# decides whether the PR changes behaviour a user sees, and writes, extends and
# runs the scenarios this PR needs. Two measurements are taken here, both
# always, so the record lists every fact at once:
#
#  (1) The suite command from the first source present (#1763): env,
#      .amplihack/qa.toml, one clean CI step, or `cargo test`, `npm test` or
#      `pytest` detected in the project the changed paths pick (QA_UNRESOLVED
#      when they pick none, and nothing runs). Its exit status is evidence the
#      judgement reads; it is not the judgement (#2458).
#  (2) Where autodrive_gadugi_required says true (a Node project, or
#      AUTODRIVE_QA_SCENARIO_DIR set): whether gadugi-test is on PATH, which
#      scenario directory applies and in what state, and which scenario files
#      it holds. NO SCENARIO IS RUN OR VALIDATED HERE. gadugi_status is one of
#      NOT_REQUIRED, NOT_INSTALLED, NO_SCENARIOS and FOUND, and the eight
#      run-shaped fields stay empty or 0 so existing readers stay valid.
#      Elsewhere gadugi-test is never consulted: Rust CLI substitutes `cargo
#      test`, and a measured docs_only (#2219) or a base qa.toml declaration
#      (#2458) drops (2) in Node too.
#
# No value here is a verdict a consumer conditions criterion 1 on. Delete this
# record and criterion 1 still reaches a verdict: worse-informed, never stuck.
# What a reader may not do is read an unknown value as a pass; every enum
# below is closed, and an unknown value is the judgement's named blocker
# qa-measurement-unavailable.
#
# Input comes from the environment only:
#   REPO_PATH              the repository to measure (default: the working directory)
#   AUTODRIVE_QA_EVIDENCE  file the record is written to, private (optional)
#   AUTODRIVE_ROUND_LABEL  recorded as qa_round
#   AUTODRIVE_QA_BASE      the base branch name, for the changed paths; unset means no base was resolved, and nothing here guesses one
#   AUTODRIVE_QA_BASE_SOURCE  how the caller reached that base, checked here: operator, clone-default or none. Unset beside a base is operator; no base, or any other word, is none
#   AUTODRIVE_QA_COMMAND, AUTODRIVE_QA_DIR, AUTODRIVE_QA_COMMANDS and
#   AUTODRIVE_QA_SCENARIO_DIR, described where they are read below.
# Output: one JSON line on stdout, and the same line in AUTODRIVE_QA_EVIDENCE
# itself, never at a sibling path, so step-00d's hash of that path stays a
# tamper check on what was measured. Exit 0 whenever a record was produced,
# PASS or not; exit 1 when REPO_PATH, a temporary file or the helpers beside
# this file are unavailable, so no record exists. Executed, never sourced;
# helpers come from beside this file only.
# NO-TIMEOUT POLICY (issue #439): a test suite is never cut off mid-run.
set -uo pipefail
# This file's own directory, found before the cd below (BASH_SOURCE may be
# relative); without the helpers there, no evidence. An exported CDPATH would
# make a relative `cd` run the suite outside the repository (#1516 review).
# Inherited GIT_* and outer-run AMPLIHACK_* are cleared before any cd or git (#2033).
unset CDPATH
H="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
{ [ -n "$H" ] && [ -f "$H/autodrive_state.sh" ] && [ -f "$H/autodrive_trust.sh" ] && [ -f "$H/git-repo-env.sh" ] \
    && [ ! -L "$H/git-repo-env.sh" ] && . "$H/git-repo-env.sh"; } \
  || { echo "ERROR: autodrive-qa-helpers-not-found: autodrive_state.sh, autodrive_trust.sh or git-repo-env.sh is missing" >&2; exit 1; }
amplihack_clear_git_repo_env; amplihack_clear_run_identity_env; GCLR="$AMPLIHACK_GIT_ENV_CLEARED"; RCLR="$AMPLIHACK_RUN_ENV_CLEARED"
cd "${REPO_PATH:-.}" 2>/dev/null || { echo "ERROR: cannot cd to REPO_PATH" >&2; exit 1; }
ROOT="$(pwd -P)"
EVIDENCE="${AUTODRIVE_QA_EVIDENCE:-}"
# One shell asks every scope question (autodrive_qa_report), so the diff and the tree walk run once
# (#1763). autodrive_trust.sh loads the scope and sources files; without them the evidence is BLOCKED.
SCAN="$(. "$H/autodrive_trust.sh" || exit 1
  if [ -n "${_AD_SOURCES_OK:-}" ] && declare -F autodrive_qa_report >/dev/null; then autodrive_qa_report "$ROOT"
  else printf 'noscope\t1\nkind\t%s\ngreq\t%s\ndocsonly\t%s\ntarget\t%s\ndecl\tnone\n' "$(autodrive_repo_type "$ROOT")" \
      "$(autodrive_gadugi_required "$ROOT")" "$(autodrive_qa_docs_only "$ROOT")" "$(autodrive_qa_target "$ROOT")"; fi)" || SCAN=""
KIND=""; GREQ=""; DOCSONLY="unknown"; NOSCOPE=""; TARGET=""; PATHS="(unknown)"; DSRC=""; DDIR=""; DCMD=""; DBAD=""; CISKIP=""; DECL="none"; DECLSKIP=""
while IFS=$'\t' read -r key val; do
  case "$key" in
    kind) KIND="$val" ;;
    greq) GREQ="$val" ;;
    decl) DECL="$val" ;;
    declskip) [ "$val" != "ci-job-workflow-changed" ] || DECLSKIP="$val" ;;
    docsonly) case "$val" in true|false) DOCSONLY="$val" ;; *) DOCSONLY="unknown" ;; esac ;;
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
# The criterion-1 declaration (#2458), checked again: anything outside the whitelist is none.
DECL="$(export LC_ALL=C; case "$DECL" in multi-area-command) ;; ci-job:*) J="${DECL#ci-job:}"
  case "$J" in ''|-*|*[!A-Za-z0-9\ ._/\(\)+:,@%-]*) DECL="none" ;; esac; [ "${#J}" -le 128 ] || DECL="none" ;; *) DECL="none" ;; esac
  printf '%s' "$DECL")"
X="$DOCSONLY"; [ "$DECL" = "none" ] || X="true"
case "${KIND}:${GREQ}:${X}" in # node:false is legal only beside docs_only=true (#2219) or a declaration (#2458); anything else is a broken install, and gives no evidence
  rust-cli:true:*|rust-cli:false:*|node:true:*|node:false:true|python:true:*|python:false:*|unknown:true:*|unknown:false:*) ;;
  *) echo "ERROR: autodrive-qa-helpers-not-found: no repository type (${KIND:-none}:${GREQ:-none}:${DOCSONLY:-none})" >&2
     exit 1 ;;
esac
[ -z "$NOSCOPE" ] || PATHS="(unknown)"
# Where a detected command runs: `<type><TAB><dir>` or `unresolved<TAB><candidates>`,
# checked again here: an odd token, an unsafe directory, or a type that
# disagrees with KIND leaves no detected command, so the evidence is BLOCKED
# rather than run at the root.
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
# Free text is sanitised before it reaches the JSON: no quotes, backslashes or
# control bytes, so a hostile log line or directory name can neither break the
# evidence nor carry terminal escapes. ANSI CSI sequences (ECMA-48) go whole;
# tabs, CRs and newlines become spaces and backslashes '/', so a coloured or
# tabular runner log keeps its words (#1531).
san() { LC_ALL=C sed "s|$(printf '\033')\[[0-?]*[ -/]*[@-~]||g" | LC_ALL=C tr '\t\r\n\\' '   /' \
  | LC_ALL=C tr -d '\000-\037\177"'; }
# Text cut to a byte length is made ASCII first: a cut through a multibyte
# character leaves invalid UTF-8, which every JSON reader rejects.
ascii() { LC_ALL=C tr -d '\200-\377'; }
CAUSES=""; QBASE="$(printf '%s' "${AUTODRIVE_QA_BASE:-}" | san | ascii | cut -c1-200)"; QBSRC="${AUTODRIVE_QA_BASE_SOURCE-operator}"
case "$QBSRC" in operator|clone-default) [ -n "$QBASE" ] || QBSRC="none" ;; *) QBSRC="none" ;; esac
# No base means no change set was measured: the scope helper's `(none)` would read as a pull request that changed nothing.
[ -n "$QBASE" ] || PATHS="(unknown)"
cause() { CAUSES="${CAUSES}${CAUSES:+; }$1"; }
# The evidence binds to the tree it was measured on: the merge gate refuses evidence
# whose head_sha is not the SHA being merged, so an earlier round's PASS cannot stand in.
HEAD_SHA=""
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  HEAD_SHA="$(git rev-parse HEAD 2>/dev/null || printf '')"
  # git rev-parse prints its own argument when it cannot resolve it, so a
  # checkout with no commit in it would record the word HEAD here. The
  # value is an object name or nothing at all: hexadecimal, at least seven
  # digits long, and anything else becomes empty before it is printed.
  case "$HEAD_SHA" in
    [0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]*) ;;
    *) HEAD_SHA="" ;;
  esac
  case "$HEAD_SHA" in *[!0-9a-fA-F]*) HEAD_SHA="" ;; esac
else
  echo "[skip] not a git repo; the qa evidence records no head sha, so the merge gate refuses it rather than merging unbound evidence" >&2
fi
# --- suite commands --------------------------------------------------
# The first source present wins, even when it gives no usable command: env,
# repo-file (an invalid qa.toml is BLOCKED, qa-config-invalid), ci (when it
# gives none, its reason is qa_ci_skip), then detected. The operator variables
# come from the environment only (a recipe context default would hide them).
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
      symlink|not-regular|too-large|syntax|unknown-key|duplicate-key|declaration-conflict|unknown-value) ;;
      missing-command|unsafe-command|unsafe-dir|dir-not-found|changed-in-pr|paths-empty|paths-unknown) ;;
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
LOG="$(mktemp -t autodrive-qa-XXXXXX)" || { echo "ERROR: cannot create a temporary log" >&2; exit 1; }
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
    ( cd -- "$ROOT" && bash -c "$text" ) >>"$LOG" 2>&1 </dev/null
    rc=$?
  else
    dir="$QA_DIR"
    first="$(set -f; set -- $text; printf '%s' "${1:-}")"
    # A missing AUTODRIVE_QA_DIR is named, never "<cmd> not installed" (#1516).
    if [ "$kind" = "word" ] && [ ! -d "$dir" ]; then
      S_MISSING="true"; cause "AUTODRIVE_QA_DIR $(printf '%s' "$dir" | san | ascii | cut -c1-120) not found"
      continue
    fi
    # One run path for every word and detected command. Installed means
    # `command -v` finds it on PATH from qa_dir (a word may also be
    # ./<first> there). Where it resolves is not checked, and this is not a
    # sandbox: `npm test` runs scripts.test, pytest imports conftest.py and
    # cargo runs build.rs, so a suite always runs branch code; a project
    # `.venv/bin/pytest` on PATH is used as it is. `./` keeps a relative `-`
    # from meaning $OLDPWD. No timeout: a suite is never cut off mid-run.
    case "$dir" in /*) ;; *) dir="./$dir" ;; esac
    if ! ( cd -- "$dir" 2>/dev/null && { command -v -- "$first" >/dev/null 2>&1 \
           || { [ "$kind" = "word" ] && [ -x "./${first}" ]; }; } ); then
      S_NOTINST="true"; cause "$(printf '%s' "$first" | san | cut -c1-80) not installed"
      continue
    fi
    # shellcheck disable=SC2086
    ( set -f; cd -- "$dir" && $text ) >>"$LOG" 2>&1 </dev/null
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
# Consulted only where gadugi is required. AUTODRIVE_QA_SCENARIO_DIR comes
# from the environment only. Without it the directory is the first of
# tests/agentic, tests/gadugi/scenarios and scenarios that exists, else
# tests/agentic. An operator path that does not exist is a named state the
# judgement can act on, not a per-round failure: on 13 November a launcher
# named a directory that did not exist and every round failed with "explicit
# operator selection preserved", which no round could clear (#2569).
#
# gadugi_scenario_dir_state is a closed set, written from one of these
# literals and never interpolated from the operator's value:
#   not-consulted     gadugi is not required, so no directory was looked for
#   operator          the operator named a directory and it exists
#   operator-missing  the operator named one that does not exist
#   detected          one of the three names above exists
#   absent            none of the three exists
COUNT=0; SCEN=""; SYMLINKED=""; NONREG=""; SDIR=""; SREL=""; SSTATE="not-consulted"
if [ "$GREQ" = "true" ]; then
  SDIR_IN="${AUTODRIVE_QA_SCENARIO_DIR:-}"
  if [ -n "$SDIR_IN" ]; then
    SSTATE="operator"
  else
    SSTATE="absent"; SDIR_IN="tests/agentic"
    for d in tests/agentic tests/gadugi/scenarios scenarios; do [ -d "${ROOT}/${d}" ] && { SDIR_IN="$d"; SSTATE="detected"; break; }; done
  fi
  [ "$SDIR_IN" = "/" ] || SDIR_IN="${SDIR_IN%/}"
  case "$SDIR_IN" in /*) SDIR="$SDIR_IN" ;; *) SDIR="${ROOT}/${SDIR_IN}" ;; esac
  case "$SDIR" in "${ROOT}/"*) SREL="${SDIR#"${ROOT}/"}" ;; *) SREL="$SDIR" ;; esac
  if [ "$SSTATE" = "operator" ] && [ ! -d "$SDIR" ]; then
    SSTATE="operator-missing"
    echo "WARNING: the scenario directory the operator named does not exist: $(printf '%s' "$SREL" | san | ascii | cut -c1-200). The judgement falls back to the repository's own tracked directory or hands off; it does not fail the round." >&2
  fi
elif [ "$KIND" = "rust-cli" ] && [ -d "${ROOT}/${TDIR}/tests/parity/scenarios" ]; then
  # qa-team's Rust CLI parity fixtures of the detected workspace, named from
  # the repository root; cargo test runs them. Informational: nothing
  # decides a pass from the list.
  PFX="tests/parity/scenarios"; [ "$TDIR" = "." ] || PFX="${TDIR}/${PFX}"
  while IFS= read -r f; do
    [ -n "$f" ] && SCEN="${SCEN}${SCEN:+ }${PFX}/${f##*/}"
  done < <(find "${ROOT}/${TDIR}/tests/parity/scenarios" -maxdepth 1 -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | LC_ALL=C sort)
fi
# Top-level regular files only, because the tool that validates a directory
# of scenarios does not read subdirectories. A symlinked or non-regular
# *.yaml entry is named here and is not a file the judgement may run: the
# listing declined to follow the link, so the judgement does not either.
if [ -n "$SDIR" ] && [ -d "$SDIR" ]; then
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    COUNT=$((COUNT + 1)); SCEN="${SCEN}${SCEN:+ }${SREL}/${f##*/}"
  done < <(find "$SDIR" -maxdepth 1 -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | LC_ALL=C sort)
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    k="symlinked scenario"
    if [ -L "$f" ]; then SYMLINKED="${SYMLINKED}${SYMLINKED:+ }${SREL}/${f##*/}"; else NONREG="${NONREG}${NONREG:+ }${SREL}/${f##*/}"; k="non-regular scenario entry"; fi
    echo "WARNING: ${k} is named in the record and is not a file the judgement may run: $(printf '%s' "${SREL}/${f##*/}" | san)" >&2
  done < <(find "$SDIR" -mindepth 1 -maxdepth 1 ! -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | LC_ALL=C sort)
fi

# --- gadugi-test -----------------------------------------------------
# NO SCENARIO RUNS HERE (#2569). Running the suite was what turned a missing
# scenario, a failed validation and a host-caused scenario failure into holds
# no round could clear, and on 12 November running the whole suite on a shared
# host emptied /tmp twice. What is measured instead is the presence of the
# tool and the contents of the directory. The judgement step writes, extends,
# stages and runs the scenarios this PR changed or added, one at a time, and
# records what it did.
#
# Where gadugi is not required the tool is not consulted at all, installed or
# not. Elsewhere the four states are facts, each one something the judgement
# can act on: NOT_INSTALLED is a named recoverable blocker, NO_SCENARIOS is a
# journey to write, FOUND is a list to choose from.
G_STATUS=""
if [ "$GREQ" = "false" ]; then
  G_STATUS="NOT_REQUIRED"
  echo "INFO: gadugi scenarios are not part of criterion 1's measurement in a ${KIND} repository (qa-team's repo-type table, declaration ${DECL}); the judgement decides what exercises this PR's change" >&2
elif ! command -v gadugi-test >/dev/null 2>&1; then
  # A fact, never an install: the judgement meets criterion 1 by other
  # evidence or records a hand-off naming the tool and its owner.
  G_STATUS="NOT_INSTALLED"; cause "gadugi-test not installed"
elif [ "$COUNT" = "0" ]; then
  G_STATUS="NO_SCENARIOS"; cause "no scenarios in $(printf '%s' "$SREL" | san)"
else
  G_STATUS="FOUND"
  echo "INFO: ${COUNT} scenario(s) listed under ${SREL}; this file runs none of them" >&2
fi
[ -z "$DECLSKIP" ] || cause ".amplihack/qa.toml: scenarios_ci_job not used: ${DECLSKIP}"
[ -n "$SYMLINKED" ] && cause "symlinked scenario named and not runnable: ${SYMLINKED}"
[ -n "$NONREG" ] && cause "non-regular scenario entry named and not runnable: ${NONREG}"
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
[ "$S_MISSING" = "true" ] && pick "qa-command-missing"
[ "$S_NOTINST" = "true" ] && pick "qa-command-not-installed"
[ "$G_STATUS" = "NOT_INSTALLED" ] && pick "gadugi-test-missing"
# The gadugi half is a complete measurement in exactly two states: it was not
# required, so no directory was consulted; or scenarios were found and listed.
# NOT_INSTALLED and NO_SCENARIOS have already named their own reason above.
#
# This is where withdrawing the scenario run could have fail-closed into a
# hold of the kind #2569 removes: the earlier text accepted only
# gadugi_status PASS, which this file can no longer produce, so every Node
# repository with scenarios would have recorded BLOCKED/qa-command-missing
# every round, which no round can clear. FOUND is complete.
G_DONE="false"
{ [ "$G_STATUS" = "NOT_REQUIRED" ] || [ "$G_STATUS" = "FOUND" ]; } && G_DONE="true"
if [ -z "$REASON" ] && { [ "$RAN" -eq 0 ] || [ "$G_DONE" != "true" ]; }; then
  echo "ERROR: nothing failed by name, yet the measurement is incomplete (suite commands run=${RAN}, gadugi=${G_STATUS}); recording it as incomplete so the judgement sees a fact rather than a pass." >&2
  REASON="qa-command-missing"
fi
# Four values, closed. Each one is a fact the judgement reads and can act on;
# none of them is a criterion-1 verdict, and no consumer conditions criterion
# 1 on this field.
case "$REASON" in
  '') STATUS="PASS" ;;
  qa-command-failed|no-scenarios) STATUS="FAIL" ;;
  qa-config-invalid|qa-command-missing|qa-command-not-installed|gadugi-test-missing) STATUS="BLOCKED" ;;
  qa-unresolved) STATUS="QA_UNRESOLVED" ;;
  *) echo "ERROR: unknown qa_reason '${REASON}'; recording an incomplete measurement rather than inventing a verdict." >&2
     STATUS="BLOCKED"; REASON="qa-command-missing" ;;
esac
SUMMARY="$(printf '%s' "${CAUSES}${CAUSES:+${TAIL:+: }}${TAIL}" | san)"
# The evidence format: every field a string, so a reader never has to guess a type.
F='{"qa_status":"%s","qa_reason":"%s","qa_repo_type":"%s","qa_dir":"%s","qa_command":"%s",'
F="${F}"'"qa_command_source":"%s","qa_criterion1_declaration":"%s","qa_command_paths":"%s","qa_candidates":"%s","qa_ci_skip":"%s",'
F="${F}"'"qa_suite_commands_count":"%s","qa_scenarios":"%s","qa_exit_code":"%s","qa_summary":"%s",'
F="${F}"'"qa_round":"%s","head_sha":"%s","docs_only":"%s","gadugi_required":"%s","gadugi_status":"%s",'
# The eight run-shaped fields are literals: this file runs no scenario, so
# each is structurally empty or zero rather than a measurement. They stay in
# the record so a reader written against the earlier record is still valid,
# and autodrive_scenario_results stays in autodrive_trust.sh for the readers
# that do have a result list (#1517 D7). The agent's own scenario runs are
# reported in qa_judgement_notes, not here.
F="${F}"'"gadugi_validate_exit_code":"","gadugi_run_exit_code":"","gadugi_scenario_count":"%s",'
F="${F}"'"gadugi_scenario_dir":"%s","gadugi_scenario_dir_state":"%s","gadugi_scenarios_validated":"0",'
F="${F}"'"gadugi_scenarios_run":"0","gadugi_scenarios_passed":"0","gadugi_scenarios_failed":"0",'
F="${F}"'"gadugi_failed_scenarios":"","gadugi_scenario_results":"","qa_base":"%s","qa_base_source":"%s","git_env_cleared":"%s","run_env_cleared":"%s"}\n'
# shellcheck disable=SC2059 # F is the fixed format above, never input
OUT="$(printf "$F" \
  "$STATUS" "$REASON" "$TYPE" "$(printf '%s' "$QD" | san | ascii | cut -c1-200)" "$(printf '%s' "$CMDTEXT" | san | ascii | cut -c1-500)" \
  "$SRC" "$(printf '%s' "$DECL" | san | ascii)" "$(printf '%s' "$PATHS" | san | ascii)" "$(printf '%s' "$QUNRES" | san | ascii)" \
  "$(printf '%s' "$CISKIP" | san | ascii | cut -c1-40)" "$RAN" \
  "$(printf '%s' "$SCEN" | san | ascii | cut -c1-2000)" "$RC" "$SUMMARY" \
  "$(printf '%s' "${AUTODRIVE_ROUND_LABEL:-round}" | san)" "$HEAD_SHA" \
  "$DOCSONLY" "$GREQ" "$G_STATUS" "$COUNT" "$(printf '%s' "$SREL" | san | ascii | cut -c1-200)" "$SSTATE" \
  "$QBASE" "$QBSRC" "$(printf '%s' "$GCLR" | san | ascii)" "$(printf '%s' "$RCLR" | san | ascii)")"
# Private to this user (#1517 D5), and never written through a planted symlink.
[ -z "$EVIDENCE" ] || ( umask 077; rm -f -- "$EVIDENCE"; printf '%s' "$OUT" > "$EVIDENCE" )
printf '%s\n' "$OUT"
