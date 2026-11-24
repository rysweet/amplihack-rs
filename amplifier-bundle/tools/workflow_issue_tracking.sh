#!/usr/bin/env bash
# workflow_issue_tracking.sh — tracking-issue selection and local (provider-free)
# issue tracking metadata.
#
# The local-tracking functions were extracted from `step-03-create-issue` in
# amplifier-bundle/recipes/workflow-prep.yaml; the GitHub selection functions
# (#1595) were written here. That brick is at its 400-line budget
# (scripts/check-brick-budget.sh), and the remedy for a full brick is extraction,
# never compression; the room for the issue claim check (#1361) was bought the
# same way the 23-step plan banner moved to workflow_plan_banner.sh.
#
# These functions are meant to be SOURCED by the step, and they read the step's
# own variables (`EXISTING_ISSUE_NUMBER`, `TASK_DESC`) from the caller's scope.
#
#   derive_local_tracking_id  — a stable `local-issue-N` / `local-ab-N` /
#                               `local-<hash>` reference for a repository with
#                               no reachable issue provider
#   emit_local_metadata       — the tracking_* key/value block step-03b parses
#   sanitize_cli_output       — redact provider CLI output before it is logged
#   select_github_tracking_issue — GitHub tracking-issue choice (#1595): an
#                               explicit issue_number wins; otherwise the first
#                               task-text reference that is an open issue (not a
#                               PR) in this repository, marked ones first
#   _tracking_issue_masked_text — lower-cased task text with PR, owner/repo#N,
#                               AB#N and hex-colour references removed
#   _tracking_issue_refs      — `N kind` lines for one kind (marked or bare)
#   _tracking_issue_candidates — up to 5 `N kind` candidates, no network
#   _github_open_issue_url    — read-only check of one candidate via `gh api`
#   _is_github_issue_url      — the one URL check: https://…/issues/N, one line
#   select_strict_delivery_issue — strict_delivery (docs/reference/
#                               workflow-delivery-inputs.md): confirms the
#                               caller's issue in the repository that owns it,
#                               or fails; never creates or substitutes one
#   azdo_task_work_item       — the work item a task text names (AB#N, else #N),
#                               for the Azure DevOps reuse lookup

derive_local_tracking_id() {
  if [[ "$EXISTING_ISSUE_NUMBER" =~ ^#?([0-9]+)$ ]]; then
    printf 'local-issue-%s' "${BASH_REMATCH[1]}"
  elif [[ "$EXISTING_ISSUE_NUMBER" =~ AB#([0-9]+)|_workitems/edit/([0-9]+) ]]; then
    printf 'local-ab-%s' "${BASH_REMATCH[1]:-${BASH_REMATCH[2]}}"
  elif [[ "$EXISTING_ISSUE_NUMBER" =~ ^local-(issue|ab)-([0-9]+)$ ]]; then
    printf '%s' "$EXISTING_ISSUE_NUMBER"
  elif [[ "$TASK_DESC" =~ ([Ii]ssue[[:space:]#]*|#)([0-9]+) ]]; then
    printf 'local-issue-%s' "${BASH_REMATCH[2]}"
  elif [[ "$TASK_DESC" =~ AB#([0-9]+) ]]; then
    printf 'local-ab-%s' "${BASH_REMATCH[1]}"
  else
    printf 'local-%s' "$(printf '%s' "$TASK_DESC" | sha256sum | cut -c1-12)"
  fi
}

emit_local_metadata() { LOCAL_REF="$(derive_local_tracking_id)"; LOCAL_NUM=""; [[ "$LOCAL_REF" =~ local-(issue|ab)-([0-9]+)$ ]] && LOCAL_NUM="${BASH_REMATCH[2]}"; printf 'tracking_system=local\ntracking_reference=%s\ntracking_issue=%s\nissue_creation=local-tracking\n' "$LOCAL_REF" "$LOCAL_REF"; [ -n "$LOCAL_NUM" ] && printf 'issue_number=%s\n' "$LOCAL_NUM"; return 0; }

sanitize_cli_output() { printf '%s\n' "$1" | head -c 4000 | sed -E 's#https?://[^[:space:]]*@#https://<redacted>@#g; s#gh[pousr]_[A-Za-z0-9_]{8,}#<redacted-token>#g; s#github_pat_[A-Za-z0-9_]+#<redacted-token>#g; s#[Bb]earer[[:space:]]+[A-Za-z0-9._~+/=-]{20,}#Bearer <redacted-token>#g; s#[A-Za-z0-9]{52}#<redacted-token>#g'; }

# Percent-decode one path segment of an Azure DevOps remote URL. Returns 1 (and
# an empty result) on a malformed or NUL-bearing encoding so the caller falls
# back to local tracking rather than acting on a half-decoded org/project.
_pct_decode() {
  local e="$1" d="" i=0 ch hex
  while [ "$i" -lt "${#e}" ]; do
    ch="${e:$i:1}"
    if [ "$ch" = "%" ] && [ "$((i+2))" -le "${#e}" ]; then
      hex="${e:$((i+1)):2}"
      if [[ "$hex" =~ ^[0-9a-fA-F]{2}$ ]]; then
        [ "$hex" = "00" ] && { echo "WARN: NUL byte in percent-encoding" >&2; echo ""; return 1; }
        printf -v tmp "\\x$hex"; d+="$tmp"; i=$((i+3)); continue
      fi
      echo "WARN: Invalid percent-encoding '%${hex}' — using local tracking" >&2
      echo ""; return 1
    fi
    d+="$ch"; i=$((i+1))
  done
  printf '%s' "$d"
}

# Masked, lower-cased task text: PR references, owner/repo#N, AB#N and #N
# followed by a word character (hex colours) are removed so they are never
# looked up. Only the first 64 KiB is read (bash substring: no SIGPIPE).
_tracking_issue_masked_text() {
  printf '%s\n' "${TASK_DESC:0:65536}" | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/(^|[^a-z0-9_])(pr|pull request|pull)[[:space:]]*#[0-9]+/\1 /g; s/[a-z0-9_.-]+\/[a-z0-9_.-]+#[0-9]+/ /g; s/ab#[0-9]+/ /g; s/#[0-9]+[a-z_][a-z0-9_]*/ /g'
}

# `N kind` lines for references of one kind (`marked` or `bare`), in order of
# appearance. Only grep's no-match exit (1) is accepted; a grep error (2) or a
# sed failure is the function's non-zero status.
_tracking_issue_refs() {
  local masked
  masked="$(_tracking_issue_masked_text)"
  if [ "$1" = "marked" ]; then
    printf '%s\n' "$masked" | { grep -oE '(^|[^a-z0-9_])(issue|fix(es|ed)?|close[sd]?|resolve[sd]?)[[:space:]]*:?[[:space:]]*#[0-9]+' || [ $? -eq 1 ]; } | sed -E 's/.*#([0-9]+)$/\1 marked/'
  else
    printf '%s\n' "$masked" | { grep -oE '(^|[^a-z0-9_#])#[0-9]+' || [ $? -eq 1 ]; } | sed -E 's/.*#([0-9]+)$/\1 bare/'
  fi
}

# Up to 5 `N kind` candidates, marked references first, each number once (its
# first kind wins). awk reads all of its input (no early exit), so pipefail
# never sees a SIGPIPE.
_tracking_issue_candidates() {
  { _tracking_issue_refs marked; _tracking_issue_refs bare; } \
    | awk '$1 ~ /^[1-9][0-9]{0,8}$/ && !seen[$1]++ && c < 5 { print; c++ }'
}

# Return 0 only if $1 is a single-line https:// URL ending in /issues/$2.
_is_github_issue_url() { [[ "$1" =~ ^https://[^[:space:]]+/issues/${2}$ ]]; }

# Print the URL of issue N and return 0 only if it is an open issue, not a
# pull request, in this repository. Read-only; every other outcome returns 1.
_github_open_issue_url() {
  local n row rc state is_pr url
  n="${1:-}"
  [[ "$n" =~ ^[0-9]+$ ]] || return 1
  # gh's stderr stays discarded (it can hold tokens); its exit code is logged.
  row="$(timeout 30 gh api "repos/{owner}/{repo}/issues/$n" --jq '[.state, (.pull_request != null), .html_url] | @tsv' </dev/null 2>/dev/null)" \
    || { rc=$?; echo "WARN: could not verify candidate #${n} (gh exit ${rc})" >&2; return 1; }
  IFS=$'\t' read -r state is_pr url <<< "$row" || return 1
  [ "$state" = "open" ] && [ "$is_pr" = "false" ] && _is_github_issue_url "$url" "$n" || return 1
  printf '%s\n' "$url"
}

# GitHub tracking issue for step-03. Returns 0 with a URL (or, for an explicit
# number gh cannot resolve as an issue, local metadata) on stdout; returns 1
# with empty stdout when nothing is selected. Diagnostics go to stderr only.
select_github_tracking_issue() {
  local explicit n kind url rc=0 checked=""
  explicit="${EXISTING_ISSUE_NUMBER:-}"; explicit="${explicit#\#}"
  if [[ "$explicit" =~ ^[0-9]{1,9}$ ]]; then
    url="$(timeout 60 gh issue view "$explicit" --json url --jq '.url // ""' </dev/null 2>/dev/null)" || rc=$?
    if [ "$rc" -eq 0 ] && _is_github_issue_url "$url" "$explicit"; then
      echo "INFO: tracking issue #${explicit} selected (explicit)" >&2
      printf '%s\n' "$url"; return 0
    fi
    # gh's stderr stays discarded (it can hold tokens); the exit code is enough
    # to tell a timeout (124) or auth/network failure from a bad number.
    if [ "$rc" -ne 0 ]; then
      echo "WARN: could not verify issue #${explicit} (gh exit ${rc}); falling back to local tracking" >&2
    else
      echo "WARN: explicit issue_number #${explicit} could not be resolved as an issue in this repository — using local tracking" >&2
    fi
    emit_local_metadata; return 0
  elif [ -n "$explicit" ]; then
    echo "WARN: issue_number is not a valid issue number — ignoring it for tracking-issue selection" >&2
  fi
  while read -r n kind; do
    checked+=" $n"
    url="$(_github_open_issue_url "$n")" || continue
    echo "INFO: tracking issue #${n} selected (${kind})" >&2
    printf '%s\n' "$url"; return 0
  done < <(_tracking_issue_candidates)
  [ -z "$checked" ] || echo "INFO: no task-text candidate accepted as an open issue (checked:${checked}); falling back to title search" >&2
  return 1
}

# 0 when $1 is a plain owner/repo slug (no `.` or `..` segment).
_strict_slug_ok() {
  [[ "${1:-}" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || return 1
  case "/$1/" in */./*|*/../*) return 1 ;; esac
}

# strict_delivery (docs/reference/workflow-delivery-inputs.md). The caller has
# already chosen the issue, the repository that owns it, the pull request's
# repository and the branch; step-03 only confirms them. Arguments, in order:
# issue_number create_issue repository pr_repository closing_reference
# existing_branch base_branch forbidden_base_branches (base_branch may be
# empty). The base is checked here as well as at step-16, so a misdirected run
# fails before any work. The issue is read on github.com: publication accepts
# only a github.com origin, and a closing reference cannot cross hosts.
# The issue must be open, as on the ordinary path (#1595): a task closed while
# it waited in the caller's queue (completed, or not planned) is not delivered.
# A caller that wants a closed issue worked on reopens it first.
# Prints the issue's URL and returns 0, or returns 2 with the reason on stderr.
# It never creates an issue, never reads the task text, never consults the
# remote host type and never falls back to local tracking: a strict run on the
# wrong issue is worse than a failed one.
select_strict_delivery_issue() {
  local n="${1:-}" create="${2:-}" repo="${3:-}" pr_repo="${4:-}" closing="${5:-}" branch="${6:-}"
  local base="${7:-}" forbidden="${8:-}" host="github.com"
  local dir origin tag url observed kind rc=0
  dir="$(dirname "${BASH_SOURCE[0]}")"
  # shellcheck source=/dev/null
  if ! . "$dir/workflow_delivery_inputs.sh" || ! . "$dir/workflow_github_repo.sh"; then
    echo "ERROR: strict_delivery: workflow_delivery_inputs.sh or workflow_github_repo.sh is missing next to $dir/workflow_issue_tracking.sh" >&2
    return 2
  fi
  [[ "$n" =~ ^[1-9][0-9]{0,8}$ ]] || { echo "ERROR: strict_delivery: issue_number must be the number of the caller's issue" >&2; return 2; }
  case "$(printf '%s' "$create" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')" in
    true|1|yes) echo "ERROR: strict_delivery: create_issue must not be set; a strict run works on the caller's issue and never opens another" >&2; return 2 ;;
  esac
  _strict_slug_ok "$repo" || { echo "ERROR: strict_delivery: repository must be the owner/repo that owns issue ${n}" >&2; return 2; }
  _strict_slug_ok "$pr_repo" || { echo "ERROR: strict_delivery: pr_repository must be the owner/repo that receives the pull request" >&2; return 2; }
  [ -n "$branch" ] || { echo "ERROR: strict_delivery=true requires existing_branch: a strict run works on the branch its caller assigned and never derives one" >&2; return 2; }
  [ -z "$base" ] || delivery_base_ref "$base" "$branch" "$forbidden" >/dev/null || return 2
  origin="$(parse_github_repo_identity "$(git remote get-url origin 2>/dev/null)")" || origin=""
  same_github_repository "$pr_repo" "$origin" || { echo "ERROR: strict_delivery: pr_repository '${pr_repo}' is not this checkout's origin repository ('${origin:-unknown}')" >&2; return 2; }
  tag="$(strict_issue_tag "$n" "$repo" "$pr_repo")"
  [ "$closing" = "$tag" ] || { echo "ERROR: strict_delivery: closing_reference must be '${tag}' (issue ${n} in ${repo}, pull request in ${pr_repo})" >&2; return 2; }
  url="https://${host}/${repo}/issues/${n}"
  # gh's stderr stays discarded (it can hold tokens); its exit code is logged.
  # The filter answers `<kind><TAB><html_url>`, where kind is the issue's state
  # (`open`, `closed`) or `pull_request`: the issues endpoint also returns pull
  # requests.
  observed="$(GH_PROMPT_DISABLED=1 timeout 30 gh api --hostname "$host" "repos/${repo}/issues/${n}" --jq '[(if has("pull_request") then "pull_request" else (.state // "") end), (.html_url // "")] | @tsv' </dev/null 2>/dev/null)" || rc=$?
  [ "$rc" -eq 0 ] || { echo "ERROR: strict_delivery: could not read ${repo}#${n} on ${host} (gh exit ${rc}); a strict run never substitutes another issue" >&2; return 2; }
  kind=""
  IFS=$'\t' read -r kind observed <<< "$observed" || true
  if [ "$kind" = pull_request ] || [ "$(printf '%s' "$observed" | tr '[:upper:]' '[:lower:]')" != "$(printf '%s' "$url" | tr '[:upper:]' '[:lower:]')" ]; then
    echo "ERROR: strict_delivery: ${repo}#${n} on ${host} is not an issue at ${url} (a pull request, or moved)" >&2
    return 2
  fi
  [ "$kind" = open ] || {
    echo "ERROR: strict_delivery: ${repo}#${n} is ${kind:-in no reported state}, not open; a strict run delivers only an open issue and never implements a task closed while it waited" >&2
    return 2
  }
  echo "INFO: tracking issue ${repo}#${n} selected (strict_delivery)" >&2
  printf '%s\n' "$observed"
}

# The Azure DevOps work item a task text names, for step-03's reuse lookup:
# the first AB#N, else the first #N; nothing when there is none. GitHub never
# uses this (#1595: select_github_tracking_issue chooses GitHub's issue).
azdo_task_work_item() {
  if [[ "${TASK_DESC:-}" =~ AB#([0-9]+) ]] || [[ "${TASK_DESC:-}" =~ \#([0-9]+) ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
  fi
  return 0
}
