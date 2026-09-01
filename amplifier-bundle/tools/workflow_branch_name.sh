#!/usr/bin/env bash
# workflow_branch_name.sh — decide the branch name a workflow run should use.
#
# ISSUE #1426. `workflow-worktree.yaml` derived its branch name — and therefore
# its worktree DIRECTORY name — by slugifying the first 50 characters of
# `task_description`. A task whose opening line was
#
#     Repository: /Users/ryan/src/mistt-qa/ws/142/jamestown (GitHub mistt-repo/jamestown).
#
# produced the ref
#
#     feat/issue-142-repository-usersryansrcmistt-qaws142jamestown-gith
#
# — sixty-five characters of path-mangled prose, cut mid-word at "gith", and
# created as a SECOND branch alongside `fix/142-band-edge-previous-slice`, which
# the same task had explicitly pinned and told the run not to branch away from.
# The same shape produced `feat/issue-1277-skip-workflow-launch-this-agent-is-
# already-executi`: a truncated prompt fragment leaked into a git ref.
#
# Two rules follow, and this helper implements both:
#
#   `explicit`  An explicitly NAMED branch wins. A task that says
#               `Branch: <ref>` (or `BRANCH — already created …:` with the ref on
#               the next line) has already answered the question; the caller uses
#               that answer verbatim and never derives a competitor.
#
#   `derive`    Absent one, the name is keyed to the ISSUE NUMBER, and the
#               descriptive tail is bounded (24 characters by default) and cut on
#               a WORD BOUNDARY — never mid-word, never a filesystem path, never
#               fifty characters of raw prose. With nothing usable to say, the
#               tail is a short stable HASH of the task rather than its prose.
#
# The task text is read from the TASK_DESCRIPTION *environment variable*, never
# from argv. On Linux a single argument is capped at MAX_ARG_STRLEN (128 KB), so
# a 150 KB task description passed as an argument would die with E2BIG before
# this script ever ran; and only the first 64 KB is ever examined, so the cost
# does not grow with the description either.
#
# PORTABILITY: bash 3.2 (the system bash on macOS) — no `${VAR,,}`, no `mapfile`,
# no associative arrays. Issue #1423 was exactly this mistake.
#
# `set -e` is deliberately NOT used: every subcommand here is best-effort by
# contract, returns its answer on stdout, and prints nothing on stdout when it
# has no answer. Failures are reported by an empty stdout, so the caller's own
# fallback ladder stays in charge.
#
# Usage:
#   TASK_DESCRIPTION=... workflow_branch_name.sh explicit
#   TASK_DESCRIPTION=... workflow_branch_name.sh derive --issue N --prefix feat [--title T]
#
# Direct tests: tests/issue_1426_branch_name_not_prose.sh

set -uo pipefail

# Bounds. Overridable for tests; the defaults are the contract.
MAX_SLUG_CHARS="${AMPLIHACK_BRANCH_SLUG_MAX:-24}"
MAX_NAME_CHARS="${AMPLIHACK_BRANCH_NAME_MAX:-72}"
MAX_INPUT_BYTES="${AMPLIHACK_BRANCH_INPUT_MAX:-65536}"

# Branches a run must never adopt from prose: "Branch: main" in a task
# description describes the base, never the branch to commit onto.
RESERVED_REFS=" main master develop trunk head "

task_text() { printf '%s' "${TASK_DESCRIPTION:-}" | head -c "$MAX_INPUT_BYTES"; }

lower() { printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]'; }

task_hash() {
  if command -v shasum >/dev/null 2>&1; then
    task_text | shasum -a 256 2>/dev/null | cut -c1-8
  elif command -v sha256sum >/dev/null 2>&1; then
    task_text | sha256sum 2>/dev/null | cut -c1-8
  else
    task_text | cksum | cut -d' ' -f1 | cut -c1-8
  fi
}

# ref_is_valid REF — a conservative shape check plus git's own authoritative
# checker. The regex runs first so a name containing shell metacharacters or a
# leading dash never reaches `git` as an argument at all.
ref_is_valid() {
  local ref="${1:-}"
  [ -n "$ref" ] || return 1
  [ "${#ref}" -le 200 ] || return 1
  printf '%s' "$ref" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._/-]*$' || return 1
  case "$ref" in *..*|*//*|*/) return 1 ;; esac
  if command -v git >/dev/null 2>&1; then
    git check-ref-format --branch "$ref" >/dev/null 2>&1 || return 1
  fi
  return 0
}

# first_token LINE — the first whitespace-delimited token, stripped of the
# decoration a human puts around a branch name in prose: backticks, quotes,
# angle brackets, brackets, and trailing sentence punctuation.
first_token() {
  printf '%s' "${1:-}" \
    | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]].*$//' \
          -e 's/[`"'"'"'<>(){}]//g' -e 's/[][]//g' -e 's/[.,;:]*$//'
}

# candidate_ok REF — would this token be an acceptable explicit branch name?
candidate_ok() {
  local c="${1:-}" lc
  ref_is_valid "$c" || return 1
  # Conservative on purpose. A prose line such as
  #     Branch protection rules: enabled
  # matches the directive shape, and "enabled" must NOT hijack the run into the
  # existing-branch path. Requiring a '/' or a '-' — the shape essentially every
  # real branch name has — rejects that class of false positive. A genuinely
  # single-word branch is still reachable through the `existing_branch` context
  # key, which is unambiguous and needs no guessing.
  case "$c" in *[/-]*) ;; *) return 1 ;; esac
  lc="$(lower "$c")"
  case "$RESERVED_REFS" in *" $lc "*) return 1 ;; esac
  return 0
}

# ---------------------------------------------------------------------------
# `explicit` — find the branch the task NAMES, if it names one.
#
# Recognised shapes (case-insensitive, the directive anchored at line start so
# the word "branch" buried in a sentence never triggers it):
#
#     Branch: fix/142-band-edge-previous-slice
#     BRANCH = fix/142-band-edge-previous-slice
#     Branch name: fix/142-band-edge-previous-slice
#     BRANCH — already created and checked out in this worktree:
#         fix/142-band-edge-previous-slice
#
# The last shape — value on the following line — is the one from the incident.
#
# Prints the branch on stdout, or nothing at all.
#
# EXIT CODE is the "does it already exist?" answer, and only when --repo-path is
# given: 0 when the named branch resolves to a local or origin ref (the caller
# should REUSE it through its existing-branch path), 10 when no branch was named
# or the named branch does not exist yet (the caller should CREATE it under
# exactly that name). Without --repo-path, 0 means "a branch was named".
# ---------------------------------------------------------------------------
cmd_explicit() {
  local line rest cand pending=0 repo=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --repo-path) repo="${2:-}"; shift 2 || break ;;
      *)           shift ;;
    esac
  done
  # `|| [ -n "$line" ]`: a task description with no trailing newline leaves its
  # last (often only) line in $line with read returning non-zero at EOF.
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    # A directive line whose value was empty: the value is the first token of
    # the next non-blank line.
    if [ "$pending" -eq 1 ]; then
      if [ -n "$(printf '%s' "$line" | tr -d '[:space:]')" ]; then
        pending=0
        cand="$(first_token "$line")"
        if candidate_ok "$cand"; then emit_explicit "$cand" "$repo"; return $?; fi
      else
        continue
      fi
    fi
    printf '%s' "$line" \
      | grep -Eqi '^[[:space:]]*branch([[:space:]]+[^:=]*)?[[:space:]]*[:=]' || continue
    rest="${line#*[:=]}"
    cand="$(first_token "$rest")"
    if candidate_ok "$cand"; then emit_explicit "$cand" "$repo"; return $?; fi
    if [ -z "$(printf '%s' "$rest" | tr -d '[:space:]')" ]; then pending=1; fi
  done < <(task_text)
  return 10
}

# emit_explicit REF [REPO] — print REF; exit 0 if it already exists (or if no
# repository was given to ask), 10 if it does not exist yet.
emit_explicit() {
  printf '%s' "$1"
  [ -n "${2:-}" ] || return 0
  git -C "$2" rev-parse --verify --quiet "refs/heads/$1" >/dev/null 2>&1 && return 0
  git -C "$2" rev-parse --verify --quiet "refs/remotes/origin/$1" >/dev/null 2>&1 && return 0
  return 10
}

# ---------------------------------------------------------------------------
# slug_stdin ISSUE — read text on stdin, print a bounded word-boundary slug.
#
# Tokens that are filesystem paths, URLs, or e-mail-ish (anything containing
# '/', '\' or '@') are dropped outright: they are the source of
# "usersryansrcmistt-qaws142jamestown". Overlong single tokens (identifier
# blobs, hashes) are dropped too. Words are then joined with '-' only while the
# result still fits MAX_SLUG_CHARS, so the slug always ends on a whole word.
# A leading "issue <N>" is skipped — the number is already in the branch name.
# ---------------------------------------------------------------------------
slug_stdin() {
  awk -v maxlen="$MAX_SLUG_CHARS" -v issue="${1:-}" '
    {
      n = split($0, w, /[ \t]+/); s = ""
      for (i = 1; i <= n; i++) {
        if (w[i] ~ /[\/\\@]/) continue
        s = s " " w[i]
      }
      gsub(/[^A-Za-z0-9]+/, " ", s)
      m = split(s, u, / +/)
      for (j = 1; j <= m; j++) {
        if (u[j] == "" || length(u[j]) > 20) continue
        words[++wc] = tolower(u[j])
      }
    }
    END {
      start = 1
      while (start <= wc && (words[start] == "issue" || words[start] == "issues" \
             || (issue != "" && words[start] == tolower(issue)))) start++
      out = ""
      for (k = start; k <= wc; k++) {
        cand = (out == "" ? words[k] : out "-" words[k])
        if (length(cand) > maxlen) {
          # Never truncate mid-word except when the very first word alone is
          # already over budget and there is nothing else to say.
          if (out == "") out = substr(words[k], 1, maxlen)
          break
        }
        out = cand
      }
      sub(/-+$/, "", out)
      print out
    }
  '
}

# ---------------------------------------------------------------------------
# `derive` — build a bounded, issue-keyed branch name.
# ---------------------------------------------------------------------------
cmd_derive() {
  local issue="" prefix="feat" title="" slug name
  while [ $# -gt 0 ]; do
    case "$1" in
      --issue)     issue="${2:-}"; shift 2 || break ;;
      --prefix)    prefix="${2:-}"; shift 2 || break ;;
      --title)     title="${2:-}"; shift 2 || break ;;
      --repo-path) shift 2 || break ;;
      *)           shift ;;
    esac
  done

  # The issue reference may be a number (GitHub/AzDO) or a local tracking id
  # such as `local-9f2c1a` (workflow_issue_tracking.sh). Both are kept; anything
  # else is reduced to ref-safe characters and bounded.
  issue="$(printf '%s' "$issue" | tr -cd 'A-Za-z0-9-' | cut -c1-24)"
  ref_is_valid "$prefix" || prefix="feat"

  # Preference order for the descriptive tail: an explicitly supplied title
  # (the issue title, when the caller has it), then the task text, then nothing.
  slug=""
  if [ -n "$title" ]; then slug="$(printf '%s' "$title" | slug_stdin "$issue")"; fi
  if [ -z "$slug" ]; then slug="$(task_text | slug_stdin "$issue")"; fi

  if [ -n "$issue" ]; then
    if [ -n "$slug" ]; then name="$prefix/issue-$issue-$slug"
    else                   name="$prefix/issue-$issue-$(task_hash)"; fi
  else
    if [ -n "$slug" ]; then name="$prefix/$slug-$(task_hash)"
    else                   name="$prefix/task-unnamed-$(task_hash)"; fi
  fi

  # Hard bound on the whole ref, cut back to a word boundary. Directory names
  # built from these have hit the filename limit before (#1249, #1260).
  if [ "${#name}" -gt "$MAX_NAME_CHARS" ]; then
    name="$(printf '%s' "$name" | cut -c1-"$MAX_NAME_CHARS" | sed -e 's/-[^-/]*$//' -e 's/[-/]*$//')"
  fi
  ref_is_valid "$name" || name="$prefix/task-unnamed-$(task_hash)"
  ref_is_valid "$name" || name="feat/task-unnamed-$(task_hash)"
  printf '%s' "$name"
}

case "${1:-}" in
  explicit) shift; cmd_explicit "$@" ;;
  derive)   shift; cmd_derive "$@" ;;
  *)
    echo "usage: workflow_branch_name.sh {explicit|derive --issue N --prefix P [--title T]}" >&2
    exit 2
    ;;
esac
