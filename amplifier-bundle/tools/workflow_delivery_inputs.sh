#!/usr/bin/env bash
# workflow_delivery_inputs.sh — checks for two caller-supplied inputs of
# default-workflow: `base_branch`, and the opt-in `strict_delivery` contract.
# Reference: docs/reference/workflow-delivery-inputs.md.
#
# Nothing here reads RECIPE_VAR_*. Every value arrives as an argument, so the
# recipe that passes a value is the one that declares its key (#1561).
#
# Sourced for its functions by step-03 (directly, and through
# workflow_issue_tracking.sh), by workflow_publish_pr.sh (step-16), by the
# finalize helpers (workflow_pr_ready.sh, workflow_final_status.sh,
# workflow_terminal_evidence.sh, workflow_agentic_finalization.sh) and by the
# workflow-terminal-state probe.
# Executed by step-04-setup-worktree:
#
#   workflow_delivery_inputs.sh worktree BASE_BRANCH STRICT EXISTING_BRANCH FORBIDDEN
#       Fails when STRICT is `true` and EXISTING_BRANCH is empty: a strict run
#       works on the branch its caller assigned and never derives one. Prints
#       origin/BASE_BRANCH when BASE_BRANCH is set (see delivery_base_ref) and
#       nothing when it is empty, so an ordinary run keeps its own discovery.
#
# Functions:
#   delivery_base_ref BASE BRANCH FORBIDDEN
#       Prints origin/BASE. Returns 1, with the reason on stderr, when BASE is
#       not a valid branch name, is written as a ref (`refs/...`) or `@`,
#       equals BRANCH (the branch being delivered; may be empty), matches a
#       pattern in FORBIDDEN, or origin/BASE does not resolve to a commit.
#       FORBIDDEN is shell globs separated by spaces, tabs or newlines, for
#       example `release/* prod`; every line counts. It also returns 1 when an
#       entry holds a quote or a comma or starts with `[` (a JSON array, a YAML
#       flow list or a comma-separated list), so a list in the wrong syntax is
#       refused instead of being enforced in part. It never substitutes another
#       base and never fetches: the caller has fetched.
#   delivery_run_base EXPLICIT RECORDED PR_REF REPO BRANCH FORBIDDEN
#       Prints the base branch (no `origin/`) of the pull request this run
#       opened (#2520). The first rule that gives a value decides:
#         1. EXPLICIT, the run's base_branch. Checked for format only; a
#            mismatch with the pull request is caught by the caller's scope
#            and identity checks, so it fails closed.
#         2. RECORDED, the base_branch field of step-16's publish result.
#         3. The baseRefName of PR_REF, read with one `gh pr view`. PR_REF must
#            be a positive number or https://github.com/<REPO>/pull/<n>;
#            anything else returns 1 before gh runs.
#         4. A base from 2 or 3 must pass delivery_base_ref (a plain branch
#            name, refs/remotes/origin/<base>, not in FORBIDDEN, not BRANCH).
#         5. Only when RECORDED and PR_REF are both empty: origin/HEAD, then
#            origin/main, origin/master, origin/develop. Once a pull request is
#            recorded this list is never used, even when gh fails.
#       Returns 1, with the reason on stderr, when no rule gives a checked
#       base. Never retries, never fetches.
#   delivery_strict_mode STRICT REPOSITORY PR_REPOSITORY CLOSING_REFERENCE
#       Prints `true` when STRICT is `true`, and `false` when STRICT is `false`
#       or empty and the three strict-only inputs are empty. Returns 1, with
#       the reason on stderr, for any other STRICT (`1`, `yes`, `TRUE`, ...) and
#       for a strict-only input set while strict delivery is off: either would
#       otherwise run the ordinary path, which may choose or create an issue,
#       while the caller believes the strict contract holds.
#   same_github_repository A B
#       Returns 0 when owner/repo A and B name the same repository. GitHub
#       compares owner and repository names case-insensitively.
#   strict_issue_tag ISSUE REPOSITORY PR_REPOSITORY
#       Prints `#ISSUE` when the issue and the pull request share a repository,
#       else `REPOSITORY#ISSUE`. GitHub resolves a bare #N in the repository of
#       the text that contains it, so an issue in another repository must be
#       written in full.
#
# This file defines functions only when sourced: no set -e/-u/-o and nothing
# printed. Runs on bash 3.2: case folding goes through tr, never the bash 4
# lowercase expansion.

delivery_base_ref() {
  local base="${1:-}" branch="${2:-}" forbidden="${3:-}" pattern
  local patterns=()
  if ! git check-ref-format --branch "$base" >/dev/null 2>&1; then
    printf 'ERROR: base_branch %q is not a valid branch name\n' "$base" >&2
    return 1
  fi
  case "$base" in
    refs/*|@)
      echo "ERROR: base_branch '$base' must be a plain branch name, not a ref" >&2
      return 1 ;;
  esac
  if [ -n "$branch" ] && [ "$base" = "$branch" ]; then
    echo "ERROR: base_branch '$base' is the branch being delivered; a pull request needs a different base" >&2
    return 1
  fi
  # The whole value, not its first line: -d '' reads up to a NUL, which the
  # value cannot hold, so a list written one pattern per line (a YAML block
  # scalar, entries joined with newlines, CRLF line ends) is enforced in full.
  # read splits on the IFS given here whatever the caller's IFS is and, unlike
  # an unquoted expansion, never expands a pattern against the file system. It
  # returns 1 at the end of the value, after it has filled the array.
  IFS=$' \t\r\n' read -r -d '' -a patterns <<< "$forbidden" || true
  # A deny list that is misread must fail, not shrink. A quote, a comma or a
  # leading `[` means the list was written in another syntax: a JSON array or
  # a YAML flow sequence is one word that matches almost nothing, and
  # `release/*,prod` is one glob that matches neither branch. Every entry is
  # checked before any is matched, so the same bad list always fails.
  for pattern in ${patterns[@]+"${patterns[@]}"}; do
    case "$pattern" in
      \[*|*\"*|*\'*|*,*)
        printf "ERROR: forbidden_base_branches entry '%s' is not a branch glob (a quote, a comma or a leading '['); write whitespace-separated shell globs such as 'release/* prod'. The list is refused, not applied in part\n" "$pattern" >&2
        return 1 ;;
    esac
  done
  for pattern in ${patterns[@]+"${patterns[@]}"}; do
    # shellcheck disable=SC2254 # the pattern is meant to match as a glob
    case "$base" in
      $pattern)
        echo "ERROR: base_branch '$base' matches forbidden_base_branches pattern '$pattern'" >&2
        return 1 ;;
    esac
  done
  if ! git rev-parse --verify --quiet "refs/remotes/origin/${base}^{commit}" >/dev/null; then
    echo "ERROR: base_branch '$base' has no remote-tracking ref origin/$base; fetch it first (no other base is substituted)" >&2
    return 1
  fi
  printf 'origin/%s\n' "$base"
}

delivery_run_base() {
  local explicit="${1:-}" recorded="${2:-}" pr_ref="${3:-}" repo="${4:-}" branch="${5:-}" forbidden="${6:-}"
  local candidate="" url_repo raw ref
  # 1. The run's own base_branch is never replaced.
  if [ -n "$explicit" ]; then
    if ! git check-ref-format --branch "$explicit" >/dev/null 2>&1; then
      printf 'ERROR: base_branch %q is not a valid branch name\n' "$explicit" >&2
      return 1
    fi
    case "$explicit" in
      refs/*|@)
        printf 'ERROR: base_branch %q must be a plain branch name, not a ref\n' "$explicit" >&2
        return 1 ;;
    esac
    printf '%s\n' "$explicit"
    return 0
  fi
  if [ -n "$recorded" ]; then
    # 2. The base step-16 recorded when it opened or matched the pull request.
    candidate="$recorded"
  elif [ -n "$pr_ref" ]; then
    # 3. The pull request's own base. Only a reference this run recorded, in
    # a shape gh cannot read as an option or another repository.
    if [[ "$pr_ref" =~ ^[1-9][0-9]*$ ]]; then
      :
    elif [[ "$pr_ref" =~ ^https://github\.com/([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)/pull/[1-9][0-9]*/?$ ]]; then
      url_repo="${BASH_REMATCH[1]}"
      if ! same_github_repository "$url_repo" "$repo"; then
        printf 'ERROR: pull request %q is not in this repository %q; its base is not read\n' "$pr_ref" "$repo" >&2
        return 1
      fi
    else
      printf 'ERROR: pull request reference %q is neither a number nor a github.com pull request URL; its base is not read\n' "$pr_ref" >&2
      return 1
    fi
    # gh's own stderr is dropped: it can quote a remote URL with credentials.
    if ! raw="$(gh pr view "$pr_ref" --json baseRefName 2>/dev/null)"; then
      printf 'ERROR: could not read the base of pull request %q (gh pr view failed); no other base is substituted\n' "$pr_ref" >&2
      return 1
    fi
    if ! candidate="$(printf '%s' "$raw" | jq -r '.baseRefName // empty')"; then
      printf 'ERROR: gh pr view for %q returned output that is not JSON\n' "$pr_ref" >&2
      return 1
    fi
    if [ -z "$candidate" ]; then
      printf 'ERROR: pull request %q reported no baseRefName\n' "$pr_ref" >&2
      return 1
    fi
  else
    # 5. No pull request recorded: the old discovery.
    candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
      printf '%s\n' "${candidate#origin/}"
      return 0
    fi
    for candidate in origin/main origin/master origin/develop; do
      if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
        printf '%s\n' "${candidate#origin/}"
        return 0
      fi
    done
    echo "ERROR: no pull request recorded and none of origin/HEAD, origin/main, origin/master, origin/develop resolves" >&2
    return 1
  fi
  # 4. A base worked out from data gets the same checks as an explicit one.
  ref="$(delivery_base_ref "$candidate" "$branch" "$forbidden")" || return 1
  printf '%s\n' "${ref#origin/}"
}

delivery_strict_mode() {
  local strict="${1:-}" stray=""
  case "$strict" in
    true) printf 'true\n'; return 0 ;;
    false|'') ;;
    *)
      printf "ERROR: strict_delivery must be exactly 'true', 'false' or empty, not '%s'; any other value would run without the strict contract\n" "$strict" >&2
      return 1 ;;
  esac
  [ -z "${2:-}" ] || stray="$stray repository"
  [ -z "${3:-}" ] || stray="$stray pr_repository"
  [ -z "${4:-}" ] || stray="$stray closing_reference"
  if [ -n "$stray" ]; then
    echo "ERROR: strict_delivery is off but strict-only inputs are set:${stray}. Only strict_delivery=true reads them: set strict_delivery=true, or leave them empty (a parent recipe's key of the same name reaches this run too)" >&2
    return 1
  fi
  printf 'false\n'
}

same_github_repository() {
  [ -n "${1:-}" ] && [ "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" = "$(printf '%s' "${2:-}" | tr '[:upper:]' '[:lower:]')" ]
}

strict_issue_tag() {
  if same_github_repository "${2:-}" "${3:-}"; then
    printf '#%s\n' "$1"
  else
    printf '%s#%s\n' "$2" "$1"
  fi
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  case "${1:-}" in
    worktree)
      base="${2:-}" strict="${3:-}" branch="${4:-}" forbidden="${5:-}"
      if [ "$strict" = true ] && [ -z "$branch" ]; then
        echo "ERROR: strict_delivery=true requires existing_branch: a strict run works on the branch its caller assigned and never derives one" >&2
        exit 1
      fi
      [ -z "$base" ] || delivery_base_ref "$base" "$branch" "$forbidden"
      ;;
    *)
      echo "usage: workflow_delivery_inputs.sh worktree BASE_BRANCH STRICT EXISTING_BRANCH FORBIDDEN" >&2
      exit 2 ;;
  esac
fi
