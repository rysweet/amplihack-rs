#!/usr/bin/env bash
# Suite execution for autodrive_qa_evidence.sh; sourced only from beside it.
# Inputs: selected KINDS/TEXTS, ROOT, QA_DIR, QA_HELPER and scope metadata.
# Outputs: CMDTEXT, RAN, RC, TAIL and S_FAILED/S_MISSING/S_NOTINST.
# Uses the adapter's cause/san/ascii functions. Infrastructure failures exit 1;
# a completed suite's non-zero status is a measurement, never a helper failure.

# stat differs between GNU/Linux and macOS. Return only owner and octal mode.
qa_path_metadata() {
  stat -c '%u %a' "$1" 2>/dev/null || stat -f '%u %Op' "$1" 2>/dev/null
}

# Resolve symlinks, then check the root AND every ancestor before creating state.
# Each must belong to this user or root. Shared writable directories require
# the sticky bit, so another user cannot replace our private child directory.
# An owner-controlled but currently read-only ancestor is still untrusted.
qa_trusted_temp_root() {
  local dir="$1" owner mode extra
  case "$dir" in /*) ;; *) dir="./$dir" ;; esac
  # The marker preserves any trailing newlines in a directory name.
  dir="$(cd -- "$dir" 2>/dev/null && pwd -P && printf '.')" || return 1
  dir="${dir%$'\n'.}"
  QA_TEMP_ROOT="$dir"
  while :; do
    read -r owner mode extra <<< "$(qa_path_metadata "$dir")"
    case "$owner" in ''|*[!0-9]*) return 1 ;; esac
    case "$mode" in ''|*[!0-7]*) return 1 ;; esac
    [ -z "$extra" ] && { [ "$owner" -eq 0 ] || [ "$owner" -eq "$EUID" ]; } || return 1
    if (( (8#$mode & 0022) != 0 && (8#$mode & 01000) == 0 )); then return 1; fi
    [ "$dir" != / ] || break
    dir="${dir%/*}"; [ -n "$dir" ] || dir=/
  done
}

qa_private_completion_dir() {
  local owner mode extra
  [ -d "$HELPER_DIR" ] && [ ! -L "$HELPER_DIR" ] || return 1
  read -r owner mode extra <<< "$(qa_path_metadata "$HELPER_DIR")"
  case "$mode" in ''|*[!0-7]*) return 1 ;; esac
  [ "$owner" = "$EUID" ] && (( (8#$mode & 07777) == 0700 )) && [ -z "$extra" ]
}

# The EXIT trap also covers helper startup failure and malformed completion.
qa_execution_cleanup() {
  rm -f -- "$LOG"
  [ -z "$HELPER_DIR" ] || rm -rf -- "$HELPER_DIR"
}

LOG="$(mktemp -t autodrive-qa-XXXXXX)" || { echo "ERROR: cannot create a temporary log" >&2; exit 1; }
HELPER_DIR=""
trap qa_execution_cleanup EXIT
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
  elif [ "$kind" = "singular" ]; then
    dir="$QA_DIR"
    if [ ! -d "$dir" ]; then
      S_MISSING="true"; cause "AUTODRIVE_QA_DIR $(printf '%s' "$dir" | san | ascii | cut -c1-120) not found"
      continue
    fi
    if [ -z "$QA_HELPER" ]; then
      echo "ERROR: amplihack QA command helper not found" >&2
      exit 1
    fi
    # A literal '-' is a directory, never $OLDPWD. The directory is a separate
    # argument; the helper reads the unchanged shell source from the environment.
    case "$dir" in /*) ;; *) dir="./$dir" ;; esac
    if ! qa_trusted_temp_root "${TMPDIR:-/tmp}"; then
      echo "ERROR: untrusted QA completion temporary root" >&2
      exit 1
    fi
    HELPER_DIR="$(umask 077; mktemp -d "${QA_TEMP_ROOT}/autodrive-qa-result.XXXXXX")" \
      || { echo "ERROR: cannot create a private QA completion directory" >&2; exit 1; }
    qa_private_completion_dir || { echo "ERROR: untrusted QA completion directory" >&2; exit 1; }
    HELPER_RESULT="$HELPER_DIR/status"
    # Only completed Bash execution is measured. Helper startup/compatibility
    # failures cannot be distinguished from QA statuses using exit codes alone.
    if ! "$QA_HELPER" orch helper autodrive-qa-command --dir="$dir" --result-file="$HELPER_RESULT" >>"$LOG" 2>&1 </dev/null; then
      tail -n 60 "$LOG" >&2
      echo "ERROR: QA command helper failed; no measured QA evidence" >&2
      exit 1
    fi
    # The protocol is exactly one decimal status and a newline. Checking the
    # byte count also rejects extra lines and bytes Bash cannot store (NUL).
    if ! qa_private_completion_dir || [ ! -f "$HELPER_RESULT" ] || [ -L "$HELPER_RESULT" ] \
       || ! IFS= read -r rc < "$HELPER_RESULT" || ! [[ "$rc" =~ ^(0|[1-9][0-9]{0,2})$ ]] \
       || [ "$rc" -gt 255 ] || ! [ "$(wc -c < "$HELPER_RESULT")" -eq "$((${#rc} + 1))" ]; then
      echo "ERROR: missing or invalid QA command helper completion result" >&2
      exit 1
    fi
    rm -rf -- "$HELPER_DIR"; HELPER_DIR=""
  else
    dir="$QA_DIR"
    first="$(set -f; set -- $text; printf '%s' "${1:-}")"
    # A missing AUTODRIVE_QA_DIR is named, never "<cmd> not installed" (#1516).
    if [ "$kind" = "word" ] && [ ! -d "$dir" ]; then
      # shellcheck disable=SC2034 # S_MISSING is read by the sourcing adapter.
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
      # shellcheck disable=SC2034 # S_NOTINST is read by the sourcing adapter.
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
# shellcheck disable=SC2034 # RC is read by the sourcing adapter.
if [ "$RAN" -gt 0 ]; then RC="${FIRST_NZ:-0}"; else RC=""; fi
# shellcheck disable=SC2034 # TAIL is read by the sourcing adapter.
TAIL="$(tail -n 5 "$LOG" | san | ascii | cut -c1-300)"
tail -n 60 "$LOG" >&2 || true
rm -f "$LOG"
