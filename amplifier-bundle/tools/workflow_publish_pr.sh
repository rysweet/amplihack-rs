#!/usr/bin/env bash
set -euo pipefail

# Fixed operation names only: never print BASH_COMMAND (may contain secrets).
PUBLISH_OPERATION="publishing prerequisites"
PUBLISH_DIAGNOSED=false
CREATE_STDERR_FILE=""
CREATE_STDOUT_FILE=""
PUBLISH_CHILD=""
publish_exit_diagnostic() {
  local status=$?
  if [ -n "$PUBLISH_CHILD" ]; then gh_cancel_owned "$PUBLISH_CHILD"; fi
  [ -z "$CREATE_STDERR_FILE" ] || gh_remove_capture "$CREATE_STDERR_FILE"
  [ -z "$CREATE_STDOUT_FILE" ] || gh_remove_capture "$CREATE_STDOUT_FILE"
  if [ "$status" -ne 0 ] && [ "$PUBLISH_DIAGNOSED" != true ]; then
    printf 'ERROR: workflow_publish_pr.sh: %s failed (exit %s)\n' "$PUBLISH_OPERATION" "$status" >&2
  fi
}
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  trap publish_exit_diagnostic EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq is required by workflow_publish_pr.sh." >&2
  exit 2
fi

PUBLISH_STATE="unknown"
LEGACY_PUBLISH_STATE="unknown"
TERMINAL_STATUS="failure"
PR_URL_RESULT=""
PR_NUMBER_RESULT=""
BRANCH_DIFF_STATUS="unknown"
MESSAGE="not classified"
# #1762: set to <B> when the worktree is on an alias of <B>; `gh pr create` then names it.
PR_HEAD_ARG=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PR_SCOPE_HELPER="${WORKFLOW_PR_SCOPE_HELPER:-${SCRIPT_DIR}/workflow_pr_scope.sh}"
# workflow_pr_scope.sh validates headRefName, baseRefName, headRefOid,
# isCrossRepository, expected_pr_title_prefix, and created_after.
CLAIM_CHECK_HELPER="${WORKFLOW_ISSUE_CLAIM_CHECK_HELPER:-${SCRIPT_DIR}/workflow_issue_claim_check.sh}"
# workflow_issue_claim_check.sh answers "is a pull request already working on
# this ISSUE" (#1361). workflow_pr_scope.sh above answers "is there a PR for this
# BRANCH" — a different question, and the one a second run trivially defeats by
# deriving a new branch.
ISSUE_REFERENCE_HELPER="${WORKFLOW_ISSUE_REFERENCE_HELPER:-${SCRIPT_DIR}/workflow_issue_reference.sh}"
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
[ -f "$GH_RETRY_HELPER" ] || { echo "ERROR: workflow_publish_pr.sh requires the shared retry helper at $GH_RETRY_HELPER" >&2; exit 2; }
# shellcheck source=/dev/null
. "$GH_RETRY_HELPER"
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow_publish_pr.sh requires the shared GitHub repo identity parser at $GITHUB_REPO_HELPER" >&2
  exit 2
fi
# Defines parse_github_repo_identity (issue #1642).
# shellcheck source=workflow_github_repo.sh disable=SC1090
. "$GITHUB_REPO_HELPER"
# write_body_file keeps the PR body out of argv: gh reads it with --body-file,
# truncated below GitHub's 65,536-character limit (issue #1570).
BODY_FILE_HELPER="${WORKFLOW_BODY_FILE_HELPER:-${SCRIPT_DIR}/workflow_body_file.sh}"
[ -f "$BODY_FILE_HELPER" ] || { echo "ERROR: workflow_publish_pr.sh requires the body-file helper at $BODY_FILE_HELPER" >&2; exit 2; }
# shellcheck source=/dev/null
. "$BODY_FILE_HELPER"
# base_branch and strict_delivery checks (docs/reference/workflow-delivery-inputs.md).
DELIVERY_INPUTS_HELPER="${WORKFLOW_DELIVERY_INPUTS_HELPER:-${SCRIPT_DIR}/workflow_delivery_inputs.sh}"
[ -f "$DELIVERY_INPUTS_HELPER" ] || { echo "ERROR: workflow_publish_pr.sh requires the delivery-input helper at $DELIVERY_INPUTS_HELPER" >&2; exit 2; }
# shellcheck source=workflow_delivery_inputs.sh
. "$DELIVERY_INPUTS_HELPER"

# base_branch in the publish result (#2520): the base this step checked and
# gave the pull request, so finalize can find a pull request on a base other
# than origin/HEAD. Set only from a checked value: the --base given to
# gh pr create, or the base of a matched pull request that passed the scoped
# lookup. Every other exit, and a create that let GitHub choose, records "".
BASE_BRANCH_RESULT=""
emit_publish_result() {
  jq -nc \
    --arg state "$PUBLISH_STATE" \
    --arg legacy_state "$LEGACY_PUBLISH_STATE" \
    --arg terminal_status "$TERMINAL_STATUS" \
    --arg pr_url "$PR_URL_RESULT" \
    --arg pr_number "$PR_NUMBER_RESULT" \
    --arg branch_diff_status "$BRANCH_DIFF_STATUS" \
    --arg message "$MESSAGE" \
    --arg base_branch "$BASE_BRANCH_RESULT" \
    '{state:$state,legacy_state:$legacy_state,terminal_status:$terminal_status,pr_url:$pr_url,pr_number:$pr_number,branch_diff_status:$branch_diff_status,message:$message,base_branch:$base_branch}'
}

# Best-effort: apply caller-configured labels to the GitHub PR that was just
# published (created OR matched as an existing open PR for this branch). The
# labels come from the comma-separated `WORKFLOW_PR_LABELS` env var, so the
# generic bundle stays policy-free — the caller (e.g. an autonomous agent that
# needs a durable "eligible for gated self-merge" marker) decides what to apply.
#
# This NEVER fails the publish: a label that does not exist in the repo, a
# transient GitHub API error, or a missing `gh` must not block the PR flow, so
# every failure is warn-and-continue and the function always returns 0. It is a
# no-op unless the host is GitHub, a numeric PR number was resolved, and at
# least one label was requested.
apply_pr_labels_best_effort() {
  [ -n "${WORKFLOW_PR_LABELS:-}" ] || return 0
  [ "${HOST_TYPE:-}" = "github" ] || return 0
  case "${PR_NUMBER_RESULT:-}" in '' | *[!0-9]*) return 0 ;; esac
  # Only label PRs that are newly created or currently OPEN. A MERGED/CLOSED
  # terminal is a no-op success (nothing to gate for the merge queue), and
  # labeling a closed PR would be pointless churn. PR_STATE is empty on the
  # create-new path (no pre-existing PR was found), so this still labels
  # freshly created PRs.
  case "${PR_STATE:-}" in MERGED | CLOSED) return 0 ;; esac
  command -v gh >/dev/null 2>&1 || return 0

  # `gh pr edit --add-label` splits comma-separated labels natively, so the raw
  # CSV is passed straight through in a single call — no bespoke parsing needed.
  # `timeout 60` mirrors every other gh call in this script: a hung `gh pr edit`
  # must never block finish_publish from emitting its JSON.
  if ! timeout 60 gh pr edit "$PR_NUMBER_RESULT" --add-label "$WORKFLOW_PR_LABELS" >/dev/null 2>&1; then
    echo "WARNING: workflow_publish_pr.sh: best-effort labels '${WORKFLOW_PR_LABELS}' not applied to PR #${PR_NUMBER_RESULT} (a label may not exist in this repo, GitHub API unavailable, or timed out)" >&2
  fi
  return 0
}

finish_publish() {
  local state="$1"
  local legacy_state="$2"
  local terminal_status="$3"
  local message="$4"
  local exit_code="${5:-0}"

  PUBLISH_STATE="$state"
  LEGACY_PUBLISH_STATE="$legacy_state"
  TERMINAL_STATUS="$terminal_status"
  MESSAGE="$message"
  if [ "$terminal_status" = "success" ]; then
    apply_pr_labels_best_effort
  fi
  if [ "$terminal_status" != "success" ]; then
    BASE_BRANCH_RESULT=""
    printf 'ERROR: %s (exit %s): %s\n' "$state" "$exit_code" "$message" >&2
    PUBLISH_DIAGNOSED=true
  fi
  PUBLISH_OPERATION="emit_publish_result (jq structured result)"
  emit_publish_result
  exit "$exit_code"
}

load_pr_fields() {
  local pr_json="$1" _ml
  # while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
  PR_FIELDS=(); while IFS= read -r _ml; do PR_FIELDS+=("$_ml"); done < <(
    printf '%s' "$pr_json" | jq -r '.url // "", ((.number // "") | tostring), .state // "", .mergedAt // ""'
  )
  PR_URL_RESULT="${PR_FIELDS[0]:-}"
  PR_NUMBER_RESULT="${PR_FIELDS[1]:-}"
  PR_STATE="${PR_FIELDS[2]:-}"
  PR_MERGED_AT="${PR_FIELDS[3]:-}"
}

resolve_pr_base_ref() {
  local candidate
  # base_branch, when the caller sets it, is the base: no discovery and no
  # fallback (delivery_base_ref in workflow_delivery_inputs.sh).
  if [ -n "${RECIPE_VAR_base_branch:-}" ]; then
    delivery_base_ref "$RECIPE_VAR_base_branch" "$CURRENT_BRANCH" "${RECIPE_VAR_forbidden_base_branches:-}"
    return
  fi
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "$candidate"; return 0; fi
  git remote set-head origin -a >/dev/null 2>&1 || true
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "$candidate"; return 0; fi
  for candidate in origin/main origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "$candidate"; return 0; fi
  done
  echo "ERROR: no supported remote base ref found. Expected origin/HEAD, origin/main, origin/master, or origin/develop." >&2
  return 1
}

# Rate-limit-aware retry wrappers. Retry logic is centralised in the shared
# workflow_gh_retry.sh driver: a genuine rate limit waits for the authoritative
# reset (adaptive, bounded by the reset window) instead of burning three
# immediate retries against a still-exhausted quota, and READ existence lookups
# fall back to the REST/core budget when GraphQL is exhausted. Auth errors are
# never retried. Each wrapper keeps its tailored fail-closed message so the
# duplicate-PR safety and diagnostics are preserved verbatim.

# REST (core-budget) fallback for the existing-PR view read path.
_publish_rest_view() {
  local n
  n="$(gh_pr_number_from_url "${PR_URL_RESULT:-}")" || return 1
  gh_pr_view_rest "$REPO_IDENTITY" "$n"
}

# REST (core-budget) fallback for the branch existence list read path.
_publish_rest_list() { gh_pr_exists_rest "${REPO_IDENTITY:-}" "${CURRENT_BRANCH:-}"; }

gh_pr_list_with_retry() {
  local status
  # READ path: existence lookup may fall back to REST on the core budget.
  GH_RETRY_REST_FALLBACK=_publish_rest_list \
    _gh_retry_core "pr list" pr list "$@" && return 0
  status=$?
  echo "ERROR: gh pr list failed (exit ${status}); refusing to risk duplicate PR creation." >&2
  return "$status"
}

gh_pr_view_with_retry() {
  local status
  GH_RETRY_REST_FALLBACK=_publish_rest_view \
    _gh_retry_core "pr view" pr view "$@" && return 0
  status=$?
  echo "ERROR: gh pr view failed (exit ${status}); existing PR state is ambiguous." >&2
  return "$status"
}

gh_pr_create_with_retry() (
  local status=0 body_file="" GH_OWNED_PID=""
  trap 'gh_cancel_owned "$GH_OWNED_PID"; [ -z "$body_file" ] || gh_remove_capture "$body_file"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
  if body_file="$(umask 077; mktemp "${TMPDIR:-/tmp}/workflow-pr-body.XXXXXX" 2>/dev/null)"; then :; else
    status=$?
    printf 'ERROR: gh pr create body temp creation failed (exit %s)\n' "$status" >&2
    return "$status"
  fi
  gh_start_owned write_body_file "$body_file" "the workflow publish step log" < <(printf '%s' "$PR_BODY")
  if wait "$GH_OWNED_PID"; then GH_OWNED_PID=""; else
    status=$?
    gh_remove_capture "$body_file"
    body_file=""
    printf 'ERROR: gh pr create body-file write failed (exit %s)\n' "$status" >&2
    return "$status"
  fi
  # WRITE path: no REST fallback (never blind-create); rate limits wait for reset.
  gh_start_owned _gh_retry_core "pr create" pr create --draft ${RECIPE_VAR_base_branch:+--base="$BASE_BRANCH"} ${PR_HEAD_ARG:+--head="$PR_HEAD_ARG"} --title "$PR_TITLE" --body-file "$body_file"
  wait "$GH_OWNED_PID" || status=$?
  GH_OWNED_PID=""
  gh_remove_capture "$body_file"
  body_file=""
  [ "$status" -eq 0 ] && return 0
  echo "ERROR: gh pr create failed (exit $status) — PR may already exist for this branch or GitHub API is unavailable" >&2
  return "$status"
)

# stdin -> stdout: masks URL userinfo, GitHub tokens, Anthropic and OpenAI
# keys (sk-, sk-ant-, sk-proj-) and AWS access key IDs. The one copy of these
# patterns, shared by provider stderr and derived PR title text (#1968). The sk-
# and AWS patterns need a boundary, so "disk-usage-..." is not a key. Azure
# DevOps PATs have no distinctive prefix and are not matched.
# LC_ALL=C: every pattern is ASCII, and BSD sed in a UTF-8 locale aborts on
# bytes that are not valid UTF-8, which pipefail would turn into a failed run.
redact_credentials() {
  # Re-scan AWS matches so a shared separator cannot hide an adjacent key.
  LC_ALL=C sed -E \
    -e 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' \
    -e 's#gh[pousr]_[A-Za-z0-9]{16,}#REDACTED_TOKEN#g' \
    -e 's#github_pat_[A-Za-z0-9_]{16,}#REDACTED_TOKEN#g' \
    -e 's#(^|[^A-Za-z0-9_-])sk-[A-Za-z0-9_-]{20,}#\1REDACTED_TOKEN#g' \
    -e ':aws' \
    -e 's#(^|[^A-Za-z0-9])(AKIA|ASIA)[0-9A-Z]{16}([^0-9A-Za-z]|$)#\1REDACTED_TOKEN\3#g' \
    -e 't aws'
}

sanitize_provider_stderr() {
  redact_credentials <"$1" | tr '\n' ' ' | head -c 500
}

# ---------------------------------------------------------------------------
# PR title helpers (#1968). The issue title is untrusted text and the task
# description is operator text that has never been published before, so both
# are treated as data: bounded, stripped of control characters, redacted,
# defused of issue claims, capped, and only ever passed as a printf %s argument
# or a single argv element. Nothing here evaluates its input. Every sed runs
# with LC_ALL=C, so input that is not valid UTF-8 cannot abort it.

# stdin -> stdout: removes the UTF-8 encodings of invisible and direction
# controls that tr cannot see byte by byte: C1 controls (U+0080-009F), the
# Arabic letter mark (U+061C), zero-width and direction marks (U+200B-200F),
# bidi embeddings and overrides (U+202A-202E), word joiners and bidi isolates
# (U+2060-206F) and the BOM (U+FEFF). A Trojan Source title would otherwise
# reach the PR title and the squash commit subject.
strip_invisible_chars() {
  LC_ALL=C sed \
    -e $'s/\302[\200-\237]//g' \
    -e $'s/\330\234//g' \
    -e $'s/\342\200[\213-\217\252-\256]//g' \
    -e $'s/\342\201[\240-\257]//g' \
    -e $'s/\357\273\277//g'
}

# normalise_pr_title_text <text>: one line, no control characters, credentials
# redacted, single spaces, trimmed, no trailing full stops (so "..." is no
# title at all). Order: bound the input to 8 KiB, strip control characters (so
# they cannot split a token), redact, collapse spaces.
# The length of the result is bounded by cap_pr_title. Bytes >= 0x80 pass.
normalise_pr_title_text() {
  # A here-string, not a pipe from printf: head cannot SIGPIPE a producer.
  head -c 8192 <<<"${1:-}" \
    | LC_ALL=C tr '\r\n\t' '   ' \
    | LC_ALL=C tr -d '\000-\037\177' \
    | strip_invisible_chars \
    | redact_credentials \
    | LC_ALL=C sed -E 's/  +/ /g; s/^ //; s/[. ]+$//'
}

# Preserve newlines and tabs, strip other controls before redacting so controls
# cannot split credentials. Never expose partial output from a failed pipeline.
sanitize_body_evidence() (
  set -o pipefail
  local text
  text=$(printf '%s' "$1" | LC_ALL=C tr -d '\000-\010\013-\037\177' |
    strip_invisible_chars | redact_credentials) || return $?
  text=$(defuse_issue_refs "$text") || return $?
  defuse_mentions "$text"
)

# conventional_pr_title <text>: prefixes "<type>: ". Only the Conventional
# Commits types in pr_title_types count as an existing prefix, in any case
# (Conventional Commits 1.0.0, item 16: types are not case-sensitive). Such a
# type is lower-cased and the rest returned unchanged, except that a scope made
# only of an issue reference is dropped: "fix(#5): x" becomes "fix: x" and
# "Fix: x" becomes "fix: x". Any other lowercase "word: rest"
# (a component, as in "release: ..." or "profile: ...") is not a type: the
# type is inferred from the rest and the word becomes the scope, so
# "release: built from the wrong SHA" becomes "fix(release): built from the
# wrong SHA". The scope is the title's own word; none is invented.
# Inference: a leading verb decides the type (fix, add, document, test,
# refactor and their forms); otherwise the first whole-word keyword group to
# match, in the order fix, docs, test, refactor, feat. Text from which no type
# can be inferred is returned unchanged.
pr_title_types='feat|fix|docs|test|refactor|chore|perf|ci|build|style|revert|security'
conventional_pr_title() {
  local text="$1" scope="" rest lower first rule group re word typed
  local issue_scope_re="^(${pr_title_types})\((#[0-9]+|[Gg][Hh]-[0-9]+)\)(!?): (.*)\$"
  local existing_prefix_re="^(${pr_title_types})(\([^)]*\))?!?: "
  local component_re='^([a-z][a-z0-9_-]*): (.*[^ ].*)$'
  # The leading word up to "(", "!" or ":" with only that word lower-cased, so
  # "Fix: x" and "FEAT(ui): y" match the type regexes below. The scope and the
  # rest keep their case. Used only when one of those regexes matches.
  word="${text%%[(!:]*}"
  typed="$(printf '%s' "$word" | LC_ALL=C tr '[:upper:]' '[:lower:]')" || return $?
  typed="${typed}${text#"$word"}"
  if [[ "$typed" =~ $issue_scope_re ]]; then
    printf '%s%s: %s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[3]}" "${BASH_REMATCH[4]}"
    return 0
  fi
  if [[ "$typed" =~ $existing_prefix_re ]]; then
    printf '%s\n' "$typed"
    return 0
  fi
  rest="$text"
  if [[ "$text" =~ $component_re ]]; then
    scope="${BASH_REMATCH[1]}"
    rest="${BASH_REMATCH[2]}"
  fi
  lower="$(printf '%s' "$rest" | LC_ALL=C tr '[:upper:]' '[:lower:]')" || return $?
  first="${lower%% *}"
  case "$first" in
    fix | fixes | fixed) rule="fix" ;;
    add | adds | implement | implements | support | supports | introduce | introduces) rule="feat" ;;
    document | documents | docs) rule="docs" ;;
    test | tests) rule="test" ;;
    refactor | refactors) rule="refactor" ;;
    *) rule="" ;;
  esac
  if [ -z "$rule" ]; then
    for group in \
      'fix:fix|fixes|fixed|bug|bugs|regression|broken|wrong|wrongly|incorrect|incorrectly|crash|crashes|fails|failing' \
      'docs:docs|documentation|document' \
      'test:test|tests|testing' \
      'refactor:refactor|refactors|refactoring' \
      'feat:add|adds|implement|implements|support|introduce|introduces'; do
      rule="${group%%:*}"
      # [^[:alnum:]_] rather than \b: bash 3.2 and BSD regex have no \b.
      re="(^|[^[:alnum:]_])(${group#*:})([^[:alnum:]_]|\$)"
      [[ "$lower" =~ $re ]] && break
      rule=""
    done
  fi
  if [ -z "$rule" ]; then
    printf '%s\n' "$text"
  elif [ -n "$scope" ]; then
    printf '%s(%s): %s\n' "$rule" "$scope" "$rest"
  else
    printf '%s: %s\n' "$rule" "$text"
  fi
}

# defuse_issue_refs <text>: rewrites every issue reference in derived text,
# so it can neither claim nor close an issue. The PR's own " (#N)" suffix is
# added afterwards by cap_pr_title and is the only reference the title keeps.
#   https://.../issues/M  -> issue M
#   issue #M              -> issue M
#   GH-M                  -> issue M     (at a word start)
#   owner/repo#M, x#M     -> ... issue M (a space goes before "issue")
#   #M, (#M), **#M**      -> issue M, (issue M), **issue M**
# No "#<digits>" is left, so a closing keyword has nothing to close however it
# is written (emphasis, code span, link text, a non-breaking space), and
# workflow_issue_claim_check.sh never sees a "(#M)" claim. Case-insensitive
# through bracket classes; BSD sed has no I flag.
defuse_issue_refs() {
  printf '%s\n' "$1" | LC_ALL=C sed -E \
    -e 's,https?://[^[:space:]]+/issues/([0-9]+),issue \1,g' \
    -e 's,([Ii][Ss][Ss][Uu][Ee])[[:space:]]*#([0-9]+),\1 \2,g' \
    -e 's,(^|[^A-Za-z0-9_])[Gg][Hh]-([0-9]+),\1issue \2,g' \
    -e 's,([A-Za-z0-9_])#([0-9]+),\1 issue \2,g' \
    -e 's,#([0-9]+),issue \1,g'
}

# defuse_mentions <text>: wraps each @name or @org/team outside a code span in
# backticks, so the PR body does not notify the people an issue title names.
# GitHub sends no notification for a mention inside a code span. A mention
# already inside a code span, or preceded by a word character (an e-mail
# address), is left alone. Titles do not notify, so only the body uses this.
defuse_mentions() {
  printf '%s\n' "$1" | LC_ALL=C awk '{
    s = $0; out = ""; code = 0; prev = ""
    while (s != "") {
      c = substr(s, 1, 1)
      if (c == "`") code = !code
      if (c == "@" && !code && prev !~ /[A-Za-z0-9_]/ \
          && match(s, /^@[A-Za-z0-9][A-Za-z0-9-]*(\/[A-Za-z0-9_.-]+)?(@[A-Za-z0-9_.-]+)*/)) {
        out = out "`" substr(s, 1, RLENGTH) "`"
        prev = substr(s, RLENGTH, 1); s = substr(s, RLENGTH + 1)
        continue
      }
      out = out c; prev = c; s = substr(s, 2)
    }
    print out
  }'
}

# cap_pr_title <base> <issue-number> [<issue-reference>]: base cut so that
# "<base> (<ref>)" fits in 200 bytes, never inside a UTF-8 sequence, trailing
# spaces and full stops dropped. <ref> defaults to `#N`; step-16 passes
# ISSUE_TAG, which is `owner/repo#N` for a strict_delivery issue in another
# repository, because GitHub links a bare #N to this repository's issue N. The
# suffix is always kept when there is an issue: the claim check's title signal
# relies on it. An empty issue number adds no suffix.
cap_pr_title() {
  local base="$1" suffix="" limit
  # Issue #1976: no linked issue, no suffix and nothing for the claim check.
  [ -z "$2" ] || suffix=" (${3:-#$2})"
  limit=$((200 - ${#suffix}))
  base="$(head -c "$limit" <<<"$base")" || return $?
  # Drop an incomplete trailing multibyte sequence left by the byte cut: a lead
  # byte followed by fewer continuation bytes than it announces.
  base="$(printf '%s\n' "$base" | LC_ALL=C sed -E \
    -e $'s/[\300-\337]$//' \
    -e $'s/[\340-\357][\200-\277]?$//' \
    -e $'s/[\360-\367][\200-\277]{0,2}$//')" || return $?
  base="${base%"${base##*[!. ]}"}"
  printf '%s%s\n' "$base" "$suffix"
}

# stdin: changed paths, one per line. Prints the true count, at most 40
# entries, and how many were left out. One awk pass and no head, so pipefail
# never sees a SIGPIPE.
format_changed_files_body() {
  awk '
    $0 == "" { next }
    { n++; if (n <= 40) shown[n] = $0 }
    END {
      if (n == 0) { print "- No changed files detected by git diff"; exit }
      printf "%d %s changed\n", n, (n == 1 ? "file" : "files")
      for (i = 1; i <= n && i <= 40; i++) print "- " shown[i]
      if (n > 40) printf "- ... and %d more\n", n - 40
    }'
}

# First non-blank line of this run's task description, without a leading
# Markdown heading marker: "# Title" and "## Title" give "Title", and a line
# that is only a marker is skipped. "#9 crashes" is not a heading and is kept.
# Reads only RECIPE_VAR_task_description: the bare TASK_DESCRIPTION alias may
# be inherited from another run and is dropped by the runner for large values
# (#1684).
first_task_line() {
  local line heading_re='^[[:space:]]*#+([[:space:]]+(.*))?$'
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    [[ "$line" =~ $heading_re ]] && line="${BASH_REMATCH[2]}"
    case "$line" in
      *[![:space:]]*) printf '%s\n' "$line"; return 0 ;;
    esac
  done <<<"${RECIPE_VAR_task_description:-}"
  return 0
}

# lookup_issue_title <issue-number> <owner/repo>: the issue's title, or nothing.
# Advisory: any failure (no identity, gh error, timeout, non-JSON, no title)
# prints nothing and returns 0, and the caller falls back to the next source.
# A non-zero gh exit is reported by its status only; gh's output never is.
# No identity means no call, so gh can never resolve GH_REPO or another remote
# and answer with a different repository's issue. Not routed through
# _gh_retry_core: one bounded attempt, no retry.
lookup_issue_title() {
  local num="$1" repo="$2" raw="" cmd=()
  [ -n "$repo" ] || return 0
  if command -v timeout >/dev/null 2>&1; then cmd=(timeout 30); fi
  cmd+=(gh issue view "$num" --repo "$repo" --json title)
  local rc=0
  raw="$("${cmd[@]}" </dev/null 2>/dev/null)" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "NOTE: gh issue view for #${num} exited ${rc} (#1968)" >&2
    return 0
  fi
  [ -n "$raw" ] || return 0
  # Parse the complete response atomically. Do not release a valid prefix
  # followed by a parse error, coerce non-string titles, or accept JSON streams.
  local title
  if title="$(jq -r -s '
    if length == 1 and (.[0] | type) == "object"
      and (.[0].title | type) == "string"
    then .[0].title else "" end' <<<"$raw" 2>/dev/null)"; then
    printf '%s\n' "$title"
  else
    rc=$?
    printf 'NOTE: gh issue view output for #%s is not JSON (jq exited %s) (#1968)\n' "$num" "$rc" >&2
  fi
  return 0
}

provider_stderr_file() {
  local prefix="${1:-workflow-provider}"
  (umask 077; mktemp "${GH_RETRY_TMPDIR:-${TMPDIR:-/tmp}}/.${prefix}.XXXXXX")
}

azdo_common_args=()
configure_azdo_args() {
  local org_url project_name
  org_url="${AZDO_ORG_URL:-${SYSTEM_COLLECTIONURI:-}}"
  project_name="${AZDO_PROJECT:-${SYSTEM_TEAMPROJECT:-}}"
  azdo_common_args=()
  [ -n "$org_url" ] && azdo_common_args+=(--org "$org_url")
  [ -n "$project_name" ] && azdo_common_args+=(--project "$project_name")
}

# Azure DevOps rate-limit wait. ADO throttling is unrelated to GitHub's API
# budget, so we must NOT consult `gh api rate_limit` here. Derive the wait from
# the provider's own Retry-After / reset header (provider-neutral parse) with a
# short floor; fall back to a small fixed backoff when no header is present.
az_wait_for_rate_limit() {
  local stderr_file="${1:-}" now epoch wait floor=5 buffer=3
  now="$(date +%s)"
  epoch="$(gh_reset_epoch_from_stderr "$stderr_file" 2>/dev/null || true)"
  if [ -z "$epoch" ]; then
    wait="$floor"
    echo "WARNING: Azure DevOps rate limit; no reset header present, waiting ${wait}s (floor) before retrying." >&2
  else
    wait=$(( epoch - now + buffer ))
    [ "$wait" -lt "$floor" ] && wait="$floor"
    echo "WARNING: Azure DevOps rate limit; waiting ${wait}s for the provider reset (header-derived; GitHub API not consulted)." >&2
  fi
  sleep "$wait"
}

az_repos_pr_list_with_retry() {
  local stderr_file output status attempt delay=1 class
  configure_azdo_args
  for attempt in 1 2 3; do
    stderr_file="$(provider_stderr_file step16-az-pr-list)" || return 1
    if output=$(timeout 60 az repos pr list "${azdo_common_args[@]}" --status active --source-branch "$CURRENT_BRANCH" --target-branch "$BASE_BRANCH" --output json 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    class="$(classify_gh_error "$stderr_file")"
    if [ "$class" = "auth" ]; then
      echo "ERROR: az repos pr list failed with an authentication error (exit ${status}); not retrying: $(sanitize_provider_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; return "$status"
    fi
    if [ "$attempt" -lt 3 ] && [ "$class" = "rate_limit" ]; then
      echo "WARNING: az repos pr list hit a rate limit (exit ${status}); waiting for reset (${attempt}/3): $(sanitize_provider_stderr "$stderr_file")" >&2
      az_wait_for_rate_limit "$stderr_file"
      rm -f "$stderr_file"; continue
    fi
    if [ "$attempt" -lt 3 ] && [ "$class" = "transient" ]; then
      echo "WARNING: az repos pr list failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_provider_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: az repos pr list failed (exit ${status}); refusing to risk duplicate PR creation." >&2
    [ ! -s "$stderr_file" ] || echo "az repos pr list stderr: $(sanitize_provider_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

az_repos_pr_create_with_retry() {
  local stderr_file output status attempt delay=1 class
  configure_azdo_args
  for attempt in 1 2 3; do
    stderr_file="$(provider_stderr_file step16-az-pr-create)" || return 1
    if output=$(timeout 60 az repos pr create "${azdo_common_args[@]}" --source-branch "$CURRENT_BRANCH" --target-branch "$BASE_BRANCH" --title "$PR_TITLE" --description "$PR_BODY" --output json 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    class="$(classify_gh_error "$stderr_file")"
    if [ "$class" = "auth" ]; then
      echo "ERROR: az repos pr create failed with an authentication error (exit ${status}); not retrying: $(sanitize_provider_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; return "$status"
    fi
    if [ "$attempt" -lt 3 ] && [ "$class" = "rate_limit" ]; then
      echo "WARNING: az repos pr create hit a rate limit (exit ${status}); waiting for reset (${attempt}/3): $(sanitize_provider_stderr "$stderr_file")" >&2
      az_wait_for_rate_limit "$stderr_file"
      rm -f "$stderr_file"; continue
    fi
    if [ "$attempt" -lt 3 ] && [ "$class" = "transient" ]; then
      echo "WARNING: az repos pr create failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_provider_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: az repos pr create failed (exit ${status}); provider state is ambiguous." >&2
    [ ! -s "$stderr_file" ] || echo "az repos pr create stderr: $(sanitize_provider_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

select_matching_azdo_pr() {
  local pr_list_json="$1"
  printf '%s' "$pr_list_json" | jq -c \
    --arg src "$CURRENT_BRANCH" \
    --arg src_ref "refs/heads/$CURRENT_BRANCH" \
    --arg tgt "$BASE_BRANCH" \
    --arg tgt_ref "refs/heads/$BASE_BRANCH" \
    'map(select(
      ((.sourceRefName // "") == $src or (.sourceRefName // "") == $src_ref) and
      ((.targetRefName // "") == $tgt or (.targetRefName // "") == $tgt_ref) and
      (((.status // "") | ascii_downcase) == "active")
    )) | .[0] // empty'
}

load_azdo_pr_fields() {
  local pr_json="$1" _ml
  # while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
  AZDO_PR_FIELDS=(); while IFS= read -r _ml; do AZDO_PR_FIELDS+=("$_ml"); done < <(
    printf '%s' "$pr_json" | jq -r '.url // "", ((.pullRequestId // "") | tostring), .status // "", .sourceRefName // "", .targetRefName // ""'
  )
  PR_URL_RESULT="${AZDO_PR_FIELDS[0]:-}"
  PR_NUMBER_RESULT="${AZDO_PR_FIELDS[1]:-}"
}

scoped_pr_lookup() {
  local scope_output reason created_after expected_pr_title_prefix
  [ -x "$PR_SCOPE_HELPER" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "workflow_pr_scope.sh is missing or not executable at $PR_SCOPE_HELPER" 1
  created_after="${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}"
  # No default title prefix (#1968), as in workflow_final_status.sh and
  # workflow_pr_ready.sh. One branch can now hold PRs with both title styles
  # ("Update ..." after a failed issue lookup, "fix: ..." otherwise); a default
  # of "Update" would let a closed template-titled PR win a tie over the open
  # one and fail the publish with FAILED_CLOSED_UNMERGED.
  expected_pr_title_prefix="${EXPECTED_PR_TITLE_PREFIX:-${PR_EXPECTED_TITLE_PREFIX:-}}"
  scope_args=(
    --repo "$REPO_IDENTITY"
    --head "$CURRENT_BRANCH"
    --base "$BASE_BRANCH"
    --issue "$ISSUE_NUM"
    --work-item "$ISSUE_NUM"
    --head-sha "$LOCAL_HEAD"
  )
  [ -n "$expected_pr_title_prefix" ] && scope_args+=(--expected-pr-title-prefix "$expected_pr_title_prefix")
  [ -n "$created_after" ] && scope_args+=(--created-after "$created_after")
  if scope_output="$("$PR_SCOPE_HELPER" "${scope_args[@]}")"; then
    printf '%s\n' "$scope_output"
    return 0
  fi
  reason="$(printf '%s' "$scope_output" | jq -r '.reason // ""' 2>/dev/null || true)"
  if [ "$reason" = "no_scoped_pr" ]; then
    printf '{}\n'
    return 0
  fi
  finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "scoped PR lookup failed: ${reason:-unknown}" 1
}

# Test seam: when this file is *sourced* with WORKFLOW_PUBLISH_PR_LIB_ONLY set,
# return after defining the helper functions above so unit tests can exercise
# pure helpers (e.g. apply_pr_labels_best_effort) without running the publish
# flow. Guarded by `BASH_SOURCE[0] != $0` so it can ONLY short-circuit when
# sourced — a leaked env var can never abort a directly-executed production run.
if [ -n "${WORKFLOW_PUBLISH_PR_LIB_ONLY:-}" ] && [ "${BASH_SOURCE[0]}" != "${0}" ]; then
  return 0
fi

HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
# #1762: in an alias worktree (<B>--wt-<n> tracking origin/<B>) the PR head is <B>.
CURRENT_BRANCH="$(bash "$SCRIPT_DIR/workflow_remote_branch.sh" .)" || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "workflow_remote_branch.sh is missing or failed at $SCRIPT_DIR; cannot name the PR head branch" 1
[ "$CURRENT_BRANCH" = "$(git branch --show-current)" ] || PR_HEAD_ARG="$CURRENT_BRANCH"
# Issue #1976 (D7): classify issue_number once. All digits is a linked issue.
# Empty, or a local tracking reference in the step-03b character set
# (workflow-prep.yaml) with at most 128 characters after `local-`, is "no
# linked issue": no (#N), no AB#, no claim check, no closing keyword. Anything
# else fails closed before any provider call. Only a value that passed one of
# these anchored checks reaches the title, the body or a `gh`/`az` argument.
ISSUE_NUMBER="${ISSUE_NUMBER:-}"
HAS_ISSUE_LINK=false
LOCAL_ISSUE_REF=""
ISSUE_NUM=""
if [[ "$ISSUE_NUMBER" =~ ^[0-9]+$ ]]; then
  HAS_ISSUE_LINK=true
  ISSUE_NUM="$ISSUE_NUMBER"
elif [ -z "$ISSUE_NUMBER" ]; then
  :
elif [[ "$ISSUE_NUMBER" =~ ^local-[A-Za-z0-9._:-]{1,128}$ ]]; then
  LOCAL_ISSUE_REF="$ISSUE_NUMBER"
else
  INVALID_ISSUE_SHOWN="$(printf '%s' "$ISSUE_NUMBER" | LC_ALL=C tr -d '\000-\037\177' | redact_credentials)" || INVALID_ISSUE_SHOWN="unavailable"
  INVALID_ISSUE_SHOWN="${INVALID_ISSUE_SHOWN:0:64}"
  finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" \
    "issue_number '${INVALID_ISSUE_SHOWN}' is neither numeric nor a local tracking id" 1
fi
[ -n "$CURRENT_BRANCH" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "current branch is empty; cannot publish from detached HEAD" 1
LOCAL_HEAD="$(git rev-parse --verify HEAD 2>/dev/null)" || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "local HEAD does not resolve" 1

PUBLISH_OPERATION="resolve_pr_base_ref (git base-ref lookup)"
BASE_REF="$(resolve_pr_base_ref)"
BASE_BRANCH="${BASE_REF#origin/}"
RUNTIME_ARTIFACT_HELPER="${WORKFLOW_RUNTIME_ARTIFACT_HELPER:-${SCRIPT_DIR}/workflow_runtime_artifacts.sh}"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found: $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
# shellcheck source=/dev/null
. "$RUNTIME_ARTIFACT_HELPER"
PUBLISH_OPERATION="preflight_known_workflow_runtime_artifacts"
preflight_known_workflow_runtime_artifacts .
PUBLISH_OPERATION="git status --porcelain"
if WORKTREE_STATUS="$(git status --porcelain)"; then :; else
  WORKTREE_STATUS_CODE=$?
  finish_publish FAILED_DIRTY_WORKTREE dirty-worktree failure "git status --porcelain failed (exit $WORKTREE_STATUS_CODE)" "$WORKTREE_STATUS_CODE"
fi
if [ -n "$WORKTREE_STATUS" ]; then
  finish_publish "FAILED_DIRTY_WORKTREE" "dirty-worktree" "failure" "dirty worktree blocks no-diff or obsolete terminal success; commit or discard changes explicitly" 1
fi
if git diff --quiet "${BASE_REF}..HEAD"; then BRANCH_DIFF_STATUS="no-diff"; else BRANCH_DIFF_STATUS="has-diff"; fi
# Issue #1968: the changed-file list, count and diff stat describe what the
# branch changed, so they are measured from the merge base. A two-dot range
# also lists every file that landed on the base after the branch was cut.
# BRANCH_DIFF_STATUS, COMMITS_AHEAD and RECENT_COMMITS keep their ranges.
if git merge-base "$BASE_REF" HEAD >/dev/null 2>&1; then
  DIFF_RANGE="${BASE_REF}...HEAD"
else
  echo "WARNING: no merge base between ${BASE_REF} and HEAD (shallow clone?); listing changed files with the two-dot range ${BASE_REF}..HEAD, which may include base-branch changes (#1968)" >&2
  DIFF_RANGE="${BASE_REF}..HEAD"
fi
# The full list feeds the count, scope, risk and closing-keyword decisions;
# only the body's display list is cut at 40. Both providers use these.
if ! CHANGED_FILES_ALL=$(git diff --name-only "$DIFF_RANGE" 2>/dev/null); then
  echo "WARNING: git diff --name-only ${DIFF_RANGE} failed; changed-file list is empty (#1968)" >&2
  CHANGED_FILES_ALL=""
fi
if ! DIFF_STAT=$(git diff --stat "$DIFF_RANGE" 2>/dev/null); then
  echo "WARNING: git diff --stat ${DIFF_RANGE} failed; the PR body has no diff stat (#1968)" >&2
  DIFF_STAT=""
fi
DIFF_STAT=$(printf '%s\n' "$DIFF_STAT" | tail -20) || { DIFF_STAT=""; echo 'WARNING: optional diff stat formatting failed' >&2; }
DIFF_STAT_BODY="${DIFF_STAT:-No diff stat available}"
CHANGED_FILES_BODY=$(printf '%s\n' "$CHANGED_FILES_ALL" | format_changed_files_body) || {
  echo 'WARNING: optional changed-files body formatting failed' >&2
  CHANGED_FILES_BODY="Changed-file listing unavailable."
}

if [ "$HOST_TYPE" = "azdo" ]; then
  if [ "$BRANCH_DIFF_STATUS" = "no-diff" ]; then
    finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "branch has no diff against base; no PR created"
  fi
  COMMITS_AHEAD=$(git rev-list --count "${BASE_REF}..HEAD" 2>/dev/null || echo "0")
  if [ "$COMMITS_AHEAD" -eq 0 ]; then
    BRANCH_DIFF_STATUS="no-diff"
    finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "0 commits ahead of ${BASE_BRANCH}; no PR created"
  fi
  if ! command -v az >/dev/null 2>&1; then
    finish_publish "BLOCKED_PROVIDER" "azdo-cli-missing" "failure" "Azure DevOps CLI ('az') is required to publish AzDO pull requests" 1
  fi
  if ! AZDO_PR_LIST_JSON="$(az_repos_pr_list_with_retry)"; then
    finish_publish "BLOCKED_PROVIDER" "azdo-pr-list-failed" "failure" "AzDO active PR lookup failed; provider/auth state is ambiguous" 1
  fi
  if AZDO_PR_JSON="$(select_matching_azdo_pr "$AZDO_PR_LIST_JSON")" && [ -n "$AZDO_PR_JSON" ]; then
    load_azdo_pr_fields "$AZDO_PR_JSON"
    finish_publish "EXISTING_OPEN_PR" "existing-open-pr" "success" "existing active AzDO PR found for branch"
  fi

  if $HAS_ISSUE_LINK; then
    PR_TITLE="Update ${CURRENT_BRANCH} (AB#${ISSUE_NUM})"
    AZDO_SUMMARY="Workflow-generated Azure DevOps PR for AB#${ISSUE_NUM}."
  else
    PR_TITLE="Update ${CURRENT_BRANCH}"
    AZDO_SUMMARY="Workflow-generated Azure DevOps PR with no linked work item."
  fi
  PR_TITLE="${PR_TITLE:0:200}"
  PR_BODY=$(printf '## Summary\n%s\n\n## Changed files\n%s\n\n## Diff stat\n```text\n%s\n```\n' "$AZDO_SUMMARY" "$CHANGED_FILES_BODY" "$DIFF_STAT_BODY")
  if ! AZDO_CREATED_JSON="$(az_repos_pr_create_with_retry)"; then
    finish_publish "BLOCKED_PROVIDER" "azdo-pr-create-failed" "failure" "AzDO PR creation failed; provider state is ambiguous" 1
  fi
  load_azdo_pr_fields "$AZDO_CREATED_JSON"
  [ -n "$PR_NUMBER_RESULT" ] || finish_publish "BLOCKED_PROVIDER" "azdo-pr-create-invalid" "failure" "AzDO PR creation returned no pullRequestId" 1
  PUBLISH_STATE="FOLLOWUP_CREATED"
  LEGACY_PUBLISH_STATE="create-new-pr"
  TERMINAL_STATUS="success"
  MESSAGE="AzDO PR created"
  emit_publish_result
  exit 0
fi

if [ "$HOST_TYPE" != "github" ]; then
  finish_publish "non-github" "non-github" "success" "non-GitHub host does not use provider PR creation"
fi

if ! command -v gh >/dev/null 2>&1; then echo "ERROR: workflow_publish_pr.sh requires the GitHub CLI ('gh') on PATH." >&2; exit 127; fi
REPO_IDENTITY="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
[ -n "$REPO_IDENTITY" ] || finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "unable to determine current GitHub repo identity from origin remote" 1

# ISSUE_TAG is how the title, body and summary name the issue. GitHub resolves
# a bare #N in this repository, so a strict_delivery issue owned by another
# repository is written owner/repo#N (docs/reference/workflow-delivery-inputs.md).
# The caller's pr_repository and closing_reference are checked against this
# checkout before any provider write; a disagreement fails the publish.
ISSUE_TAG=""
ISSUE_REPO="$REPO_IDENTITY"
if $HAS_ISSUE_LINK; then ISSUE_TAG="#${ISSUE_NUM}"; fi
if [ "${RECIPE_VAR_strict_delivery:-false}" = true ]; then
  $HAS_ISSUE_LINK || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "strict_delivery requires the caller's numeric issue_number" 1
  [ -n "${RECIPE_VAR_repository:-}" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "strict_delivery requires repository, the owner/repo that owns issue ${ISSUE_NUM}" 1
  same_github_repository "${RECIPE_VAR_pr_repository:-}" "$REPO_IDENTITY" || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" \
    "strict_delivery: pr_repository '${RECIPE_VAR_pr_repository:-}' is not this checkout's origin repository '${REPO_IDENTITY}'" 1
  ISSUE_TAG="$(strict_issue_tag "$ISSUE_NUM" "$RECIPE_VAR_repository" "$REPO_IDENTITY")"
  ISSUE_REPO="$RECIPE_VAR_repository"
  [ "${RECIPE_VAR_closing_reference:-}" = "$ISSUE_TAG" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" \
    "strict_delivery: closing_reference '${RECIPE_VAR_closing_reference:-}' must be '${ISSUE_TAG}' (issue ${ISSUE_NUM} in ${RECIPE_VAR_repository}, pull request in ${REPO_IDENTITY})" 1
fi

PUBLISH_OPERATION="scoped_pr_lookup"
PR_JSON="$(scoped_pr_lookup)"
load_pr_fields "$PR_JSON"

if [ -n "$PR_URL_RESULT" ]; then
  # The scoped lookup matched this pull request with --base "$BASE_BRANCH".
  BASE_BRANCH_RESULT="$BASE_BRANCH"
  if [ -z "$PR_STATE" ]; then
    VIEW_JSON="$(gh_pr_view_with_retry "$PR_URL_RESULT" --json url,number,state,mergedAt,headRefName,baseRefName,headRefOid,headRepositoryOwner,headRepository,isCrossRepository)"
    load_pr_fields "$VIEW_JSON"
  fi
  case "$PR_STATE:$PR_MERGED_AT" in
    OPEN:*) finish_publish "existing-open-pr" "existing-open-pr" "success" "existing open PR found for branch" ;;
    MERGED:*) finish_publish "MERGED" "already-merged" "success" "branch PR is already merged" ;;
    CLOSED:?*) finish_publish "MERGED" "closed-after-merge" "success" "branch PR is closed after merge" ;;
    CLOSED:*)
      if [ "$BRANCH_DIFF_STATUS" = "has-diff" ]; then
        finish_publish "FAILED_CLOSED_UNMERGED" "closed-unmerged-with-diff" "failure" "existing PR was closed without merge and branch still has diff; reopen it or create a new branch intentionally" 1
      fi
      finish_publish "CLOSED_OBSOLETE" "no-diff" "success" "closed unmerged PR exists but branch has no diff against base" ;;
  esac
fi

if [ "$BRANCH_DIFF_STATUS" = "no-diff" ]; then
  finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "branch has no diff against base; no PR created"
fi

COMMITS_AHEAD=$(git rev-list --count "${BASE_REF}..HEAD" 2>/dev/null || echo "0")
if [ "$COMMITS_AHEAD" -eq 0 ]; then
  BRANCH_DIFF_STATUS="no-diff"
  finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "0 commits ahead of ${BASE_BRANCH}; no PR created"
fi

# ---------------------------------------------------------------------------
# Issue claim check (#1361). Everything above this line asked about the BRANCH.
# This asks about the ISSUE, and it is the last thing checked before a new pull
# request is opened: the scoped lookup already established that no PR exists for
# THIS head, which is exactly the state a second run for an already-claimed
# issue is in. The prep phase runs the same check before any work happens; this
# one closes the window between the two, which in the recorded cases was the
# whole race (#1131 opened 01:03, #1132 merged 01:32).
#
# Advisory by construction: the helper exits non-zero ONLY when it actually saw
# a claiming pull request. No `gh` on PATH, a rate limit, an API error or a
# missing helper all warn and return 0, because an unreadable answer is not
# evidence that an issue is unclaimed — nor that it is claimed.
if ! $HAS_ISSUE_LINK; then
  : # Issue #1976: no linked issue, so there is no claim to check.
elif [ "$ISSUE_TAG" != "#${ISSUE_NUM}" ]; then
  # The helper asks about issue N of THIS repository, which is not the issue
  # here. A strict_delivery run publishes the branch its caller assigned
  # (existing_branch is required), so the failure the check exists for, a run
  # deriving a second branch for one issue, cannot happen; the scoped lookup
  # above already found no pull request for this head.
  echo "INFO: issue claim check skipped: ${ISSUE_TAG} is owned by another repository and this strict_delivery run publishes its assigned branch" >&2
elif [ -f "$CLAIM_CHECK_HELPER" ]; then
  if ! CLAIM_OUTPUT="$(bash "$CLAIM_CHECK_HELPER" --issue "$ISSUE_NUM" --repo "$REPO_IDENTITY" --head "$CURRENT_BRANCH" 2>&1 >/dev/null)"; then
    printf '%s\n' "$CLAIM_OUTPUT" >&2
    finish_publish "FAILED_DUPLICATE_CLAIM" "duplicate-issue-claim" "failure" \
      "a pull request already claims issue #${ISSUE_NUM}; refusing to open a duplicate (issue #1361)" 1
  fi
else
  echo "WARNING: workflow_issue_claim_check.sh not found at ${CLAIM_CHECK_HELPER} — creating this PR without a duplicate-claim check (#1361)" >&2
fi

# Optional metadata must not inherit errexit's conditional-call semantics.
# Every operation below is checked, including commands inside helper functions.
usable_title() {
  local base="$1"
  [ "$base" != "(${ISSUE_TAG})" ] || return 1
  [ -z "$ISSUE_TAG" ] || base="${base%" (${ISSUE_TAG})"}"
  case "$base" in *[![:space:].]*) return 0 ;; *) return 1 ;; esac
}

prepare_title_text() {
  local text
  METADATA_OPERATION="normalise_pr_title_text (title normalization)"
  text="$(normalise_pr_title_text "$1")" || return $?
  usable_title "$text" || return 1
  METADATA_OPERATION="conventional_pr_title"
  text="$(conventional_pr_title "$text")" || return $?
  METADATA_OPERATION="defuse_issue_refs"
  text="$(defuse_issue_refs "$text")" || return $?
  METADATA_OPERATION="cap_pr_title (head and UTF-8 boundary)"
  PR_TITLE="$(cap_pr_title "$text" "$ISSUE_NUM" "$ISSUE_TAG")" || return $?
  usable_title "$PR_TITLE"
}

derive_pr_title() {
  METADATA_OPERATION="awk changed-file count"
  CHANGED_COUNT=$(printf '%s\n' "$CHANGED_FILES_ALL" | awk '$0 != "" {n++} END {print n + 0}') || return $?
  # Issue #929: derive the PR-title scope from the first *substantive* changed
  # file, ignoring generated/lockfiles. The lockfile-sync step (#915) commits
  # Cargo.lock, which sorts first in `git diff --name-only`; without this filter a
  # full feature diff gets mislabeled "Update Cargo.lock". Only a genuinely
  # lockfile-only diff falls back to the lockfile scope. Filenames are treated as
  # data: quoted expansions, basename matched via a static case, never eval'd.
  is_generated_scope_file() {
    case "${1##*/}" in
      *.lock|package-lock.json|npm-shrinkwrap.json|pnpm-lock.yaml|go.sum) return 0 ;;
      *) return 1 ;;
    esac
  }
  SUBSTANTIVE_FIRST=""
  while IFS= read -r changed_path; do
    [ -n "$changed_path" ] || continue
    if ! is_generated_scope_file "$changed_path"; then
      SUBSTANTIVE_FIRST="$changed_path"
      break
    fi
  done <<<"$CHANGED_FILES_ALL"
  if [ -n "$SUBSTANTIVE_FIRST" ]; then
    FIRST_CHANGED="$SUBSTANTIVE_FIRST"
  else
    FIRST_CHANGED=$(printf '%s\n' "$CHANGED_FILES_ALL" | awk 'NF {print; exit}') || return $?
  fi
  METADATA_OPERATION="cut changed-file title scope"
  case "$FIRST_CHANGED" in amplifier-bundle/recipes/*) PR_SCOPE="workflow recipes" ;; crates/amplihack-cli/*) PR_SCOPE="amplihack CLI" ;; crates/*) PR_SCOPE="$(printf '%s' "$FIRST_CHANGED" | cut -d/ -f2)" || return $? ;; tests/*) PR_SCOPE="regression coverage" ;; docs/*) PR_SCOPE="documentation" ;; "") PR_SCOPE="workflow changes" ;; *) PR_SCOPE="$(printf '%s' "$FIRST_CHANGED" | cut -d/ -f1)" || return $? ;; esac
  local raw text
  PR_TITLE_SOURCE="template"
  raw=""
  if $HAS_ISSUE_LINK; then
    raw="$(lookup_issue_title "$ISSUE_NUM" "$ISSUE_REPO")" || return $?
  fi
  METADATA_OPERATION="normalise_pr_title_text (title normalization)"
  text="$(normalise_pr_title_text "$raw")" || return $?
  if [ -n "$text" ]; then
    PR_TITLE_SOURCE="issue"
  else
    METADATA_OPERATION="first_task_line"
    raw="$(first_task_line)" || return $?
    METADATA_OPERATION="normalise_pr_title_text (task normalization)"
    text="$(normalise_pr_title_text "$raw")" || return $?
    [ -z "$text" ] || PR_TITLE_SOURCE="task"
  fi
  if [ "$PR_TITLE_SOURCE" = template ]; then
    if [ "$CHANGED_COUNT" -gt 1 ]; then
      text="Update ${PR_SCOPE} with ${CHANGED_COUNT} changed files"
    else
      text="Update ${PR_SCOPE}"
    fi
    METADATA_OPERATION="template title sanitization"
    text="$(normalise_pr_title_text "$text")" || return $?
    text="$(defuse_issue_refs "$text")" || return $?
    METADATA_OPERATION="cap_pr_title (head and UTF-8 boundary)"
    PR_TITLE="$(cap_pr_title "$text" "$ISSUE_NUM" "$ISSUE_TAG")" || return $?
    usable_title "$PR_TITLE" || return 1
  else
    prepare_title_text "$text" || return $?
  fi
}

recover_pr_title() {
  local raw branch suffix="" limit
  PR_TITLE_SOURCE="recovery-issue"
  if $HAS_ISSUE_LINK; then
    if raw="$(lookup_issue_title "$ISSUE_NUM" "$ISSUE_REPO")" && prepare_title_text "$raw"; then
      return 0
    fi
    suffix=" (${ISSUE_TAG})"
  fi
  # Independent of every external formatter, Git query and failed normalizer.
  # An ASCII allowlist also makes the byte cap independent of the locale.
  branch="$CURRENT_BRANCH"
  case "$branch" in
    ''|*[!a-zA-Z0-9/_.:-]*|*gh[pousr]_*|*github_pat_*|*sk-*|*AKIA*|*ASIA*) branch="branch" ;;
  esac
  limit=$((200 - 7 - ${#suffix}))
  [ "$limit" -gt 0 ] || return 1
  PR_TITLE="Update ${branch:0:limit}${suffix}"
  PR_TITLE_SOURCE="recovery-branch"
}

PUBLISH_OPERATION="title derivation"
if derive_pr_title; then :; else
  metadata_status=$?
  printf 'WARNING: PR title derivation: %s failed or produced an unusable title (exit %s); recovering\n' "$METADATA_OPERATION" "$metadata_status" >&2
  recover_pr_title || finish_publish "FAILED_METADATA" "metadata-failed" failure "safe title recovery failed" 1
fi
RECENT_COMMITS=$(git log --oneline --no-decorate "${BASE_REF}..HEAD" -6 2>/dev/null) || {
  echo 'WARNING: optional git log summary failed' >&2
  RECENT_COMMITS=""
}
COMMITS_AHEAD="${COMMITS_AHEAD:-unknown}"
if [ "$PR_TITLE_SOURCE" = template ]; then
  if safe_scope="$(sanitize_body_evidence "$PR_SCOPE" 2>/dev/null)"; then :; else safe_scope="branch changes"; fi
  PR_SUMMARY="Concise workflow-generated PR for ${safe_scope}."
else
  PR_SUMMARY="Workflow-generated PR."
fi
# The body summary repeats the capped title, so it is bounded and trimmed the
# same way, with @mentions put in code spans. The pattern is quoted so "(" and
# "#" match literally under extglob.
if [ "$PR_TITLE_SOURCE" != "template" ]; then
  SUMMARY_TITLE="$PR_TITLE"
  if $HAS_ISSUE_LINK; then SUMMARY_TITLE="${SUMMARY_TITLE%" (${ISSUE_TAG})"}"; fi
  if SUMMARY_TITLE="$(sanitize_body_evidence "$SUMMARY_TITLE" 2>/dev/null)"; then :; else
    summary_status=$?
    printf 'WARNING: optional defuse_mentions summary failed (exit %s)\n' "$summary_status" >&2
    SUMMARY_TITLE="Branch changes"
  fi
  if $HAS_ISSUE_LINK; then
    PR_SUMMARY="Workflow-generated PR for ${ISSUE_TAG}: ${SUMMARY_TITLE}."
  else
    PR_SUMMARY="Workflow-generated PR: ${SUMMARY_TITLE}."
  fi
fi
# The source only: the title itself may be untrusted text.
echo "PR title source: ${PR_TITLE_SOURCE}" >&2
# Both outputs are parse_json objects, which the runner never exports under a
# bare name, so only RECIPE_VAR_<key> holds this run's evidence (#1684).
VALIDATION_SOURCE="${RECIPE_VAR_local_testing_gate:-${RECIPE_VAR_precommit_results:-}}"
if [ -n "$VALIDATION_SOURCE" ]; then VALIDATION_BODY=$(printf '%s\n' "$VALIDATION_SOURCE" | sed -n '1,12p') || { echo 'WARNING: optional validation formatting failed' >&2; VALIDATION_BODY="Validation excerpt unavailable."; }; else VALIDATION_BODY="step-13 local testing gate is expected before ready-for-review; no structured step-13 output was available to step-16."; fi
if [ -n "$RECENT_COMMITS" ]; then BEHAVIOR_BODY=$(printf 'Implemented behavior through these branch commits:\n%s' "$RECENT_COMMITS"); else BEHAVIOR_BODY="Branch is ${COMMITS_AHEAD} commit(s) ahead of ${BASE_BRANCH}; behavior impact is represented by the changed files and diff stat."; fi
case "$CHANGED_FILES_ALL" in *amplifier-bundle/recipes/*) RISK_BODY="Workflow behavior changed; review recipe gates and regression coverage carefully." ;; *crates/amplihack-cli/*) RISK_BODY="CLI behavior changed; verify command help, exit codes, and JSON/table output contracts." ;; *) RISK_BODY="No high-risk subsystem pattern detected from changed paths." ;; esac
# The closing keyword is EARNED BY THE DIFF (#1361). PR #1283's body said
# `Closes #1277` while its whole change was one added planning document; had it
# merged, a genuine bug report would have been auto-closed by an artifact
# describing what somebody intended to do about it. An unreachable helper yields
# the conservative `Refs`: under-linking costs a manual close, over-linking
# closes a live bug silently.
if ! $HAS_ISSUE_LINK; then
  # Issue #1976: a local run has no issue to link or close.
  if [ -n "$LOCAL_ISSUE_REF" ]; then
    ISSUE_LINK="No linked issue (local tracking reference \`${LOCAL_ISSUE_REF}\`)."
  else
    ISSUE_LINK="No linked issue."
  fi
else
  if [ -f "$ISSUE_REFERENCE_HELPER" ]; then
    ISSUE_KEYWORD="$(printf '%s\n' "$CHANGED_FILES_ALL" | bash "$ISSUE_REFERENCE_HELPER" --keyword)" || {
      echo 'WARNING: optional issue reference derivation failed; using Refs' >&2
      ISSUE_KEYWORD="Refs"
    }
  else
    echo "WARNING: workflow_issue_reference.sh not found at ${ISSUE_REFERENCE_HELPER} — using the non-closing 'Refs' keyword (#1361)" >&2
    ISSUE_KEYWORD="Refs"
  fi
  ISSUE_LINK="${ISSUE_KEYWORD:-Refs} ${ISSUE_TAG}"
fi
# Treat evidence as untrusted multiline text; generated issue links stay outside
# this boundary so the diff-based closing policy remains unchanged.
for body_field in CHANGED_FILES_BODY DIFF_STAT_BODY BEHAVIOR_BODY VALIDATION_BODY RISK_BODY COMMITS_AHEAD BASE_BRANCH; do
  if safe_evidence=$(sanitize_body_evidence "${!body_field}" 2>/dev/null); then
    printf -v "$body_field" '%s' "$safe_evidence"
  else
    printf 'WARNING: optional %s sanitization failed; using safe text\n' "$body_field" >&2
    printf -v "$body_field" '%s' 'Evidence unavailable.'
  fi
done
PR_BODY=$(printf '## Summary\n%s\n\n## Issue\n%s\n\n## Changed files\n%s\n\n## Diff stat\n```text\n%s\n```\n\n## Behavior\n%s\n\n## Validation\n%s\n\n## Risk\n%s\n\n## Checklist\n- [x] Branch has %s commit(s) ahead of %s\n- [ ] Code review completed\n- [ ] Philosophy check passed\n\n---\n*This PR was created as a draft for review before merging.*\n' "$PR_SUMMARY" "$ISSUE_LINK" "$CHANGED_FILES_BODY" "$DIFF_STAT_BODY" "$BEHAVIOR_BODY" "$VALIDATION_BODY" "$RISK_BODY" "$COMMITS_AHEAD" "$BASE_BRANCH") || {
  echo 'WARNING: optional rich PR body construction failed; using a minimal summary' >&2
  PR_BODY="$(printf '## Summary\nBranch changes prepared for review.\n\n')"
  if $HAS_ISSUE_LINK; then PR_BODY="${PR_BODY}"$'\n\n'"Refs ${ISSUE_TAG}"; else PR_BODY="${PR_BODY}"$'\n\n'"No linked issue."; fi
}

PUBLISH_STATE="FOLLOWUP_CREATED"
LEGACY_PUBLISH_STATE="create-new-pr"
PUBLISH_OPERATION="gh pr create capture temp creation"
if CREATE_STDERR_FILE="$(umask 077; mktemp "${TMPDIR:-/tmp}/workflow-pr-create.XXXXXX" 2>/dev/null)"; then :; else
  capture_status=$?
  finish_publish "FAILED_PR_CREATE" "create-pr-failed" failure "gh pr create stderr temp creation failed (exit $capture_status)" "$capture_status"
fi
PUBLISH_OPERATION="gh pr create"
PUBLISH_OPERATION="gh pr create stdout temp creation"
CREATE_STDOUT_FILE="$(umask 077; mktemp "${TMPDIR:-/tmp}/workflow-pr-result.XXXXXX")"
PUBLISH_OPERATION="gh pr create"
gh_start_owned gh_pr_create_with_retry >"$CREATE_STDOUT_FILE" 2>"$CREATE_STDERR_FILE"
PUBLISH_CHILD=$GH_OWNED_PID
if wait "$PUBLISH_CHILD"; then
  PUBLISH_CHILD=""
  PR_URL_RESULT=$(<"$CREATE_STDOUT_FILE")
  gh_remove_capture "$CREATE_STDERR_FILE"
  CREATE_STDERR_FILE=""
else
  CREATE_STATUS=$?
  PUBLISH_CHILD=""
  CREATE_DIAGNOSTIC="$(gh_safe_stderr "$CREATE_STDERR_FILE")"
  gh_remove_capture "$CREATE_STDERR_FILE"
  CREATE_STDERR_FILE=""
  CREATE_MESSAGE="gh pr create failed (exit ${CREATE_STATUS}): ${CREATE_DIAGNOSTIC}"
  # Keep the original create evidence across the independently scoped lookup.
  if RECOVERY_JSON="$(scoped_pr_lookup)"; then
    load_pr_fields "$RECOVERY_JSON"
    if [ -n "$PR_URL_RESULT" ] && [ "$PR_STATE" = "OPEN" ]; then
      BASE_BRANCH_RESULT="$BASE_BRANCH"
      finish_publish "existing-open-pr" "existing-open-pr" "success" "adopted existing open PR after create collision"
    fi
  else
    RECOVERY_STATUS=$?
    CREATE_MESSAGE="${CREATE_MESSAGE}; scoped collision lookup failed (exit ${RECOVERY_STATUS})"
  fi
  finish_publish "FAILED_PR_CREATE" "create-pr-failed" "failure" "$CREATE_MESSAGE" "$CREATE_STATUS"
fi
PUBLISH_OPERATION="gh pr create result parsing (PR identity)"
# Require one URL for the expected repository, without trailing messages,
# query strings or extra path components. The number helper alone is permissive.
case "$PR_URL_RESULT" in
  "https://github.com/${REPO_IDENTITY}/pull/"*) : ;;
  *) finish_publish "FAILED_PR_IDENTITY" "pr-identity-failed" failure "gh pr create succeeded but result parsing found an invalid PR identity URL" 1 ;;
esac
created_number="${PR_URL_RESULT#"https://github.com/${REPO_IDENTITY}/pull/"}"
case "$created_number" in
  ''|*[!0-9]*|0)
    finish_publish "FAILED_PR_IDENTITY" "pr-identity-failed" failure "gh pr create succeeded but result parsing found an invalid PR identity URL" 1 ;;
esac
if PR_NUMBER_RESULT="$(gh_pr_number_from_url "$PR_URL_RESULT")"; then :; else
  finish_publish "FAILED_PR_IDENTITY" "pr-identity-failed" failure "gh pr create succeeded but result parsing could not resolve PR identity" 1
fi
# Only a base this step passed with --base; without it GitHub chose the base.
[ -z "${RECIPE_VAR_base_branch:-}" ] || BASE_BRANCH_RESULT="$BASE_BRANCH"
finish_publish "$PUBLISH_STATE" "$LEGACY_PUBLISH_STATE" "success" "draft PR created"
