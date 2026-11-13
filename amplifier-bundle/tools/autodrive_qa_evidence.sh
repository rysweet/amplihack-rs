#!/usr/bin/env bash
# autodrive_qa_evidence.sh — measure the facts merge-ready criterion 1 is
# judged from, for one round of the auto-drive-to-merge merge loop (#2569).
# step-02a-qa-inputs of autodrive-merge-evidence.yaml and light-qa-inputs of
# auto-drive-light.yaml run this file.
#
# It issues no verdict. Criterion 1 is judged by an agent step following
# amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md, which
# records its judgement with `amplihack orch helper autodrive qa-record`. No
# repository type, missing scenario, missing tool or failing command stops
# anything here: each is written down for the agent.
#
# What it measures, into qa-inputs.json beside AUTODRIVE_QA_EVIDENCE:
#   - the declared QA command from the first source present (#1763): env,
#     the merge-base .amplihack/qa.toml, one clean merge-base CI step, then
#     manifest detection; it is run, and its exit status and log tail kept;
#   - the changed files (git diff --name-only -z, merge base to head);
#   - docs_only and the repository type (`unknown` outside the table);
#   - the scenario directory and the scenario files tracked in it;
#   - the head and merge-base SHAs.
#
# Input comes from the environment only:
#   REPO_PATH              the repository to measure (default: the working directory)
#   AUTODRIVE_QA_EVIDENCE  absolute path of qa-evidence.json; qa-inputs.json
#                          is written beside it, and an earlier round's
#                          qa-evidence.json there is removed
#   AUTODRIVE_ROUND_LABEL  recorded as qa_round
#   AUTODRIVE_QA_BASE      the base branch name, for the changed paths
#   AUTODRIVE_QA_HEAD      the head commit (default HEAD)
#   AUTODRIVE_QA_COMMAND, AUTODRIVE_QA_DIR, AUTODRIVE_QA_COMMANDS and
#   AUTODRIVE_QA_SCENARIO_DIR, described where they are read below.
# Output: the inputs JSON on stdout and in qa-inputs.json (mode 0600). Exit 1
# only when AUTODRIVE_QA_EVIDENCE is not absolute or the file cannot be
# written; every other condition is recorded in the file.
# Executed, never sourced; helpers come from beside this file only.
# NO-TIMEOUT POLICY (issue #439): a test suite is never cut off mid-run.
set -uo pipefail
unset CDPATH
EVIDENCE="${AUTODRIVE_QA_EVIDENCE:-}"
case "$EVIDENCE" in
  /*) ;;
  *) echo "ERROR: AUTODRIVE_QA_EVIDENCE must be an absolute path; nothing written" >&2; exit 1 ;;
esac
case "$EVIDENCE" in *$'\n'*) echo "ERROR: AUTODRIVE_QA_EVIDENCE holds a newline; nothing written" >&2; exit 1 ;; esac
OUT_DIR="${EVIDENCE%/*}"; [ -n "$OUT_DIR" ] || OUT_DIR="/"
INPUTS="${OUT_DIR%/}/qa-inputs.json"
[ -d "$OUT_DIR" ] || { echo "ERROR: the directory of AUTODRIVE_QA_EVIDENCE does not exist; nothing written" >&2; exit 1; }
# Inherited GIT_* and outer-run AMPLIHACK_* are cleared before any cd or git (#2033).
H="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
GCLR="unavailable"; RCLR="unavailable"; HELPERS=""
if [ -n "$H" ] && [ -f "$H/autodrive_trust.sh" ] && [ -f "$H/git-repo-env.sh" ] && [ ! -L "$H/git-repo-env.sh" ] \
   && . "$H/git-repo-env.sh"; then
  amplihack_clear_git_repo_env; amplihack_clear_run_identity_env
  GCLR="$AMPLIHACK_GIT_ENV_CLEARED"; RCLR="$AMPLIHACK_RUN_ENV_CLEARED"; HELPERS="1"
else
  # Inherited GIT_* cannot be cleared, so nothing here runs git or a command:
  # either could act on another repository (#2033). The file still records why.
  echo "WARNING: autodrive_trust.sh or git-repo-env.sh is missing beside this tool; the command source is recorded as unavailable" >&2
fi
cd "${REPO_PATH:-.}" 2>/dev/null || { echo "WARNING: cannot cd to REPO_PATH; measuring the working directory" >&2; }
ROOT="$(pwd -P)"
# Free text for the log tail: no control bytes or ANSI CSI sequences.
san() { LC_ALL=C sed "s|$(printf '\033')\[[0-?]*[ -/]*[@-~]||g" | LC_ALL=C tr '\t\r\n' '   ' | LC_ALL=C tr -d '\000-\037\177'; }
# jstr_v <text>: sets JSTR to <text> as a JSON string. Quotes, backslashes and
# every control byte are escaped, so any file name stays one array entry.
# Bytes that are not UTF-8 are dropped, since no JSON reader accepts them.
# Plain ASCII text, the usual case, starts no process.
jstr_v() {
  local s="$1" c o e i LC_ALL=C
  case "$s" in *[$'\200'-$'\377']*)
    if ! printf '%s' "$s" | iconv -f UTF-8 -t UTF-8 >/dev/null 2>&1; then
      s="$(printf '%s' "$s" | tr -d '\200-\377'; printf x)"; s="${s%x}"
    fi ;;
  esac
  s="${s//\\/\\\\}"; s="${s//\"/\\\"}"
  case "$s" in *[[:cntrl:]]*)
    s="${s//$'\n'/\\n}"; s="${s//$'\r'/\\r}"; s="${s//$'\t'/\\t}"
    for i in 1 2 3 4 5 6 7 8 11 12 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 127; do
      printf -v o '%03o' "$i"; printf -v c "\\$o"
      case "$s" in *"$c"*) printf -v e '\\u%04x' "$i"; s="${s//"$c"/$e}" ;; esac
    done ;;
  esac
  JSTR="\"$s\""
}
jstr() { jstr_v "$1"; printf '%s' "$JSTR"; }
# jlist: NUL-separated stdin -> a JSON array.
jlist() {
  local f sep="" out="["
  while IFS= read -r -d '' f; do jstr_v "$f"; out="${out}${sep}${JSTR}"; sep=","; done
  printf '%s]' "$out"
}

# --- head, merge base and changed files ------------------------------
HEAD_REF="${AUTODRIVE_QA_HEAD-HEAD}"; HEAD_SHA=""; MB=""; DIFF_OK=""
# The head is HEAD or a full 40- or 64-hex SHA; a prefix git would expand is refused.
case "${#HEAD_REF}:$HEAD_REF" in 4:HEAD|40:*|64:*) ;; *) HEAD_REF="" ;; esac
case "$HEAD_REF" in HEAD) ;; *[!0-9a-f]*) HEAD_REF="" ;; esac
[ -n "$HELPERS" ] && [ -n "$HEAD_REF" ] && case "$HEAD_REF" in HEAD|[0-9a-f]*) HEAD_SHA="$(git rev-parse --verify --quiet "${HEAD_REF}^{commit}" 2>/dev/null)" || HEAD_SHA="" ;; esac
case "${#HEAD_SHA}:$HEAD_SHA" in 40:*[!0-9a-f]*|64:*[!0-9a-f]*) HEAD_SHA="" ;; 40:*|64:*) ;; *) HEAD_SHA="" ;; esac
B="${AUTODRIVE_QA_BASE:-}"
if [ -n "$HEAD_SHA" ] && [ -z "${AUTODRIVE_QA_BASE+x}" ]; then
  MB="$HEAD_SHA"
elif [ -n "$HEAD_SHA" ] && [ -n "$B" ] && case "$B" in -*) false ;; *) true ;; esac \
     && git check-ref-format --branch "$B" >/dev/null 2>&1; then
  BASE_TIP="$(git rev-parse --verify --quiet "refs/remotes/origin/${B}^{commit}" 2>/dev/null)" || BASE_TIP=""
  [ -z "$BASE_TIP" ] || MB="$(git merge-base --end-of-options "$BASE_TIP" "$HEAD_SHA" 2>/dev/null)" || MB=""
  [ -z "$MB" ] || DIFF_OK="1"
fi
[ -n "$MB" ] || echo "WARNING: the merge base is unknown (AUTODRIVE_QA_BASE unset, invalid or not fetched); changed_files is empty" >&2
CHANGED="[]"
if [ -n "$DIFF_OK" ]; then
  # A failed diff is not "no files changed": base_sha is cleared too, the
  # recorded sign that the change set is unknown.
  if ! CHANGED="$(git diff --name-only --no-renames --no-ext-diff --no-textconv -z --end-of-options "$MB" "$HEAD_SHA" 2>/dev/null | jlist)"; then
    echo "WARNING: git diff from the merge base failed; changed_files is empty and base_sha is cleared" >&2
    CHANGED="[]"; MB=""
  fi
fi

# --- repository type, docs_only and the command sources --------------
# One shell asks every scope question (autodrive_qa_report), so the diff and
# the tree walk run once (#1763).
SCAN=""
if [ -n "$HELPERS" ]; then
  SCAN="$(. "$H/autodrive_trust.sh" || exit 1
    S="$H/autodrive_qa_sources.sh"
    # shellcheck source=autodrive_qa_sources.sh # a regular file beside this tool, never a symlink
    if [ -n "${_AD_SCOPE_OK:-}" ] && [ -f "$S" ] && [ ! -L "$S" ] && . "$S" \
       && declare -F autodrive_qa_report >/dev/null; then
      autodrive_qa_report "$ROOT"
    else
      printf 'noscope\t1\n'
    fi)" || SCAN=""
fi
[ -n "$SCAN" ] || { [ -z "$HELPERS" ] || echo "WARNING: the QA scope scan failed; repo_type and docs_only are unknown" >&2; SCAN=$'noscope\t1'; }
KIND="unknown"; DOCSONLY="unknown"; NOSCOPE=""; TARGET=""; DSRC=""; DDIR=""; DCMD=""; DBAD=""; CISKIP=""
while IFS=$'\t' read -r key val; do
  case "$key" in
    kind) case "$val" in rust-cli|node|python) KIND="$val" ;; *) KIND="unknown" ;; esac ;;
    docsonly) case "$val" in true|false) DOCSONLY="$val" ;; *) DOCSONLY="unknown" ;; esac ;;
    noscope) NOSCOPE="1" ;;
    target) TARGET="$val" ;;
    source) DSRC="$val" ;;
    dir) DDIR="$val" ;;
    command) DCMD="$val" ;;
    invalid) DBAD="$val" ;;
    ciskip) CISKIP="$val" ;;
  esac
done <<< "$SCAN"
# Where a detected command runs: `<type><TAB><dir>` or `unresolved<TAB><candidates>`.
# An odd token or unsafe directory leaves no detected command.
TKIND="${TARGET%%$'\t'*}"; TDIR="${TARGET#*$'\t'}"; UNRES=""
case "$TARGET" in *$'\t'*) ;; *) TKIND="unknown"; TDIR="." ;; esac
case "$TKIND" in
  unresolved) UNRES="$TDIR"; TDIR="." ;;
  "$KIND") ;;
  *) TDIR="." ;;
esac
case "$TDIR" in ''|/*|*//*|..|../*|*/..|*/../*|*[!A-Za-z0-9._/-]*) TDIR="." ;; esac

# --- the declared QA command -----------------------------------------
# The first source present wins, even when it gives no usable command.
# The operator variables come from the environment only. Set but empty means
# "no command", never "detect one":
#   AUTODRIVE_QA_COMMAND   one command, word-split with globbing off, run in
#                          AUTODRIVE_QA_DIR (the issue #1516 semantics)
#   AUTODRIVE_QA_COMMANDS  one command per line (blank and # lines skipped),
#                          each run alone with bash -c from the repository root
SRC="none"; QA_DIR="$TDIR"; QD=""; STATUS=""; CAUSE=""; KINDS=(); TEXTS=()
if [ -z "$HELPERS" ]; then
  STATUS="missing"; CAUSE="git-repo-env.sh did not load, so no command is run"
elif [ -n "${AUTODRIVE_QA_COMMAND+x}" ] || [ -n "${AUTODRIVE_QA_COMMANDS+x}" ]; then
  SRC="env"; QA_DIR="${AUTODRIVE_QA_DIR:-.}"
  [ -z "${AUTODRIVE_QA_COMMAND:-}" ] || { KINDS+=(word); TEXTS+=("$AUTODRIVE_QA_COMMAND"); QD="$QA_DIR"; }
  while IFS= read -r line || [ -n "$line" ]; do
    case "$(printf '%s' "$line" | sed 's/^[[:space:]]*//')" in ''|'#'*) continue ;; esac
    KINDS+=(entry); TEXTS+=("$line")
  done <<< "${AUTODRIVE_QA_COMMANDS:-}"
elif [ -n "$NOSCOPE" ]; then
  STATUS="missing"; CAUSE="the QA scope helpers did not load"
elif [ -n "$DBAD" ]; then
  # #2458: add kind / scenarios fields from qa.toml here. They are read from
  # the merge-base copy only, by autodrive_qa_config, never the PR head.
  SRC="repo-file"; STATUS="config-invalid"
  case "$DBAD" in
    symlink|not-regular|too-large|syntax|unknown-key|duplicate-key|missing-command|unsafe-command) CAUSE="$DBAD" ;;
    unsafe-dir|dir-not-found|changed-in-pr|paths-empty|paths-unknown) CAUSE="$DBAD" ;;
    *) CAUSE="syntax" ;;
  esac
elif [ -n "$DCMD" ] && { [ "$DSRC" = "repo-file" ] || [ "$DSRC" = "ci" ]; }; then
  SRC="$DSRC"; QA_DIR="$DDIR"; QD="$QA_DIR"; KINDS+=(word); TEXTS+=("$DCMD")
elif [ -n "$UNRES" ]; then
  STATUS="unresolved"
else
  case "$KIND" in
    rust-cli) KINDS+=(detected); TEXTS+=("cargo test --workspace --locked --no-fail-fast") ;;
    node) KINDS+=(detected); TEXTS+=("npm test") ;;
    python) KINDS+=(detected); TEXTS+=("pytest") ;;
  esac
  [ "${#TEXTS[@]}" -eq 0 ] || { SRC="detected"; QD="$QA_DIR"; }
fi
[ -n "$STATUS" ] || [ "${#TEXTS[@]}" -gt 0 ] || { STATUS="missing"; CAUSE="no command for repository type ${KIND}"; }
LOG="$(mktemp "${TMPDIR:-/tmp}/autodrive-qa.XXXXXX" 2>/dev/null)" || LOG=""
[ -n "$LOG" ] || { echo "WARNING: cannot create a temporary log; the declared command is not run" >&2; [ -n "$STATUS" ] || { STATUS="missing"; CAUSE="no temporary log"; }; TEXTS=(); KINDS=(); }
CMDTEXT=""; RAN=0; FIRST_NZ=""
i=0
while [ "$i" -lt "${#TEXTS[@]}" ]; do
  kind="${KINDS[$i]}"; text="${TEXTS[$i]}"; i=$((i + 1))
  CMDTEXT="${CMDTEXT}${CMDTEXT:+; }${text}"
  if [ "$kind" = "entry" ]; then
    # Its own process from the repository root: a `cd` in one entry never
    # reaches the next, and entries are never joined or sourced.
    ( cd -- "$ROOT" && bash -c "$text" ) >>"$LOG" 2>&1 </dev/null
    rc=$?
  else
    dir="$QA_DIR"
    first="$(set -f; set -- $text; printf '%s' "${1:-}")"
    if [ "$kind" = "word" ] && [ ! -d "$dir" ]; then
      lbl="directory"; [ "$SRC" = "env" ] && lbl="AUTODRIVE_QA_DIR"
      [ -n "$CAUSE" ] || CAUSE="${lbl} $(printf '%s' "$dir" | san | cut -c1-120) not found"
      continue
    fi
    # Installed means `command -v` finds it from qa_dir (a word may also be
    # ./<first> there). This is not a sandbox: a suite always runs branch code.
    case "$dir" in /*) ;; *) dir="./$dir" ;; esac
    if ! ( cd -- "$dir" 2>/dev/null && { command -v -- "$first" >/dev/null 2>&1 \
           || { [ "$kind" = "word" ] && [ -x "./${first}" ]; }; } ); then
      [ -n "$CAUSE" ] || CAUSE="$(printf '%s' "$first" | san | cut -c1-80) not installed"
      [ -n "$STATUS" ] || STATUS="not-installed"
      continue
    fi
    # shellcheck disable=SC2086
    ( set -f; cd -- "$dir" && $text ) >>"$LOG" 2>&1 </dev/null
    rc=$?
  fi
  RAN=$((RAN + 1))
  echo "INFO: declared QA command exited ${rc}: $(printf '%s' "$text" | san | cut -c1-200)" >&2
  [ "$rc" -eq 0 ] || [ -n "$FIRST_NZ" ] || FIRST_NZ="$rc"
done
EXIT="null"
if [ "$RAN" -gt 0 ]; then STATUS="ran"; EXIT="${FIRST_NZ:-0}"; fi
[ -n "$STATUS" ] || { STATUS="missing"; [ -n "$CAUSE" ] || CAUSE="no command ran"; }
TAIL=""
if [ -n "$LOG" ]; then
  TAIL="$(tail -n 20 "$LOG" | san | cut -c1-4000)"
  tail -n 60 "$LOG" >&2 || true
  rm -f -- "$LOG"
fi

# --- scenarios -------------------------------------------------------
# Where the agent looks for, and writes, outside-in scenarios.
# AUTODRIVE_QA_SCENARIO_DIR comes from the environment only and must be a
# relative path inside the repository.
SDIR="${AUTODRIVE_QA_SCENARIO_DIR:-}"
case "$SDIR" in ''|/*|..|../*|*/..|*/../*) SDIR="" ;; esac
if [ -z "$SDIR" ]; then
  SDIR="tests/agentic"
  for d in tests/agentic tests/gadugi/scenarios scenarios; do [ -d "${ROOT}/${d}" ] && { SDIR="$d"; break; }; done
fi
SDIR="${SDIR%/}"
SCENARIOS="[]"
if [ -n "$HELPERS" ] \
   && ! SCENARIOS="$(git ls-files -z -- "${SDIR}/*.yaml" "${SDIR}/*.yml" 2>/dev/null | jlist)"; then
  echo "WARNING: git ls-files failed; existing_scenarios is empty because the listing failed, not because none exist" >&2
  SCENARIOS="[]"
fi

# --- the inputs file -------------------------------------------------
OUT="$(printf '{"schema":1,"qa_round":%s,"head_sha":%s,"base_sha":%s,"repo_type":%s,"docs_only":%s,' \
  "$(jstr "${AUTODRIVE_ROUND_LABEL:-round}")" "$(jstr "$HEAD_SHA")" "$(jstr "$MB")" "$(jstr "$KIND")" "$(jstr "$DOCSONLY")"
  printf '"changed_files":%s,"scenario_dir":%s,"existing_scenarios":%s,' "$CHANGED" "$(jstr "$SDIR")" "$SCENARIOS"
  printf '"declared_qa":{"source":%s,"dir":%s,"command":%s,"status":%s,"exit_status":%s,' \
    "$(jstr "$SRC")" "$(jstr "$QD")" "$(jstr "$CMDTEXT")" "$(jstr "$STATUS")" "$EXIT"
  printf '"cause":%s,"candidates":%s,"ci_skip":%s,"log_tail":%s},' \
    "$(jstr "$CAUSE")" "$(jstr "$UNRES")" "$(jstr "$CISKIP")" "$(jstr "$TAIL")"
  printf '"git_env_cleared":%s,"run_env_cleared":%s}' "$(jstr "$GCLR")" "$(jstr "$RCLR")")"
# Private to this user, written whole, and never through a planted symlink:
# mv replaces a link at the target rather than following it.
TMP="$(umask 077; mktemp "${OUT_DIR%/}/.qa-inputs.XXXXXX" 2>/dev/null)" || TMP=""
[ -n "$TMP" ] || { echo "ERROR: cannot create a temporary file beside ${INPUTS}" >&2; exit 1; }
if ! { printf '%s\n' "$OUT" > "$TMP" && chmod -- 600 "$TMP" && mv -f -- "$TMP" "$INPUTS"; }; then
  rm -f -- "$TMP"
  echo "ERROR: cannot write ${INPUTS}" >&2
  exit 1
fi
# An earlier round's judgement cannot stand in for this one.
if [ -e "$EVIDENCE" ] || [ -L "$EVIDENCE" ]; then
  rm -f -- "$EVIDENCE" || { echo "ERROR: cannot remove the earlier ${EVIDENCE}" >&2; exit 1; }
fi
printf '%s\n' "$OUT"
