#!/usr/bin/env bash
# autodrive_build_goal.sh: the build goal tool, goal contract v1 (#3129 PR 2).
# The goal: build this change and open its pull request. default-workflow
# runs it through the shared loop (autodrive-goal-loop).
#
#   actions                    prints `open-pr mark-ready review wait prepare-worktree`
#   final                      prints merged or closed for the run's own pull
#                              request, else nothing (also when a read fails)
#   read <turn_dir>            goal.json (autodrive_build_reading.sh)
#   act <word> <turn_dir>      one action; always writes <turn_dir>/result.json
#                              ({outcome, summary, ...}) and exits 0
#   publish <body_file>        a comment on the run's own pull request; exit 1
#                              when there is none, or the text or gh fails
#
# The agent edits, commits and pushes in the worktree itself, through the
# repository's hooks. Fixed code here opens and marks the pull request, runs a
# review round or a check wait through PR 1's goal tool, and restores or adopts
# a worktree. It never merges: there is no merge word, and nothing here calls
# `gh pr merge`. A build ends with its pull request open.
#
# AUTODRIVE_GOAL_CONTRACT must be 1, else exit 2. Inputs from the driver:
# AUTODRIVE_STATE_DIR, AUTODRIVE_REPO, AUTODRIVE_TASK_FILE and
# AUTODRIVE_GUIDANCE_UNKNOWN; from the run's context, RECIPE_VAR_* only.
# Exit codes: 0 done (an act always writes result.json), 1 publish failed,
# 2 misuse of the contract, 75 and 79 passed up unchanged.
#
# `act` checks, in order: the turn folder, decision.json, the arguments, the
# secret screen, the run's identity and a live read of the pull request,
# guidance (mark-ready only), the effect, then a read-back.
# docs/reference/build-goal-loop.md#actions.

set -uo pipefail
[ "${AUTODRIVE_GOAL_CONTRACT:-}" = 1 ] || { echo "ERROR: autodrive_build_goal.sh speaks goal contract 1 only (AUTODRIVE_GOAL_CONTRACT='${AUTODRIVE_GOAL_CONTRACT:-}')" >&2; exit 2; }
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
# shellcheck source=autodrive_build_lib.sh
. "$HERE/autodrive_build_lib.sh" || { echo "ERROR: autodrive_build_goal.sh: autodrive_build_lib.sh is missing beside it" >&2; exit 2; }
STATE="${AUTODRIVE_STATE_DIR:-}"; STATE="${STATE%/}"
# The run's repo_path, else the driver's launch checkout (goal contract v1).
LAUNCH="${RECIPE_VAR_repo_path:-}"
if [ -z "$LAUNCH" ]; then LAUNCH="${AUTODRIVE_REPO:-.}"; fi
LAUNCH="$(cd "$LAUNCH" 2>/dev/null && pwd -P)" || { echo "ERROR: autodrive_build_goal.sh: the launch checkout cannot be entered" >&2; exit 2; }
ACTIONS="open-pr mark-ready review wait prepare-worktree"
TD=""; WORD=""; S=""
cleanup() { [ -z "$S" ] || rm -rf -- "$S"; }
trap cleanup EXIT

result() { # result <outcome> <summary> [extra json object]
  local extra="${3:-}"
  [ -n "$extra" ] || extra='{}'
  jq -cn --arg o "$1" --arg s "$(printf '%s' "$2" | goal_clean_line 600)" --argjson x "$extra" '{outcome:$o, summary:$s} + $x' \
    | head -c 65536 | goal_atomic_write "$TD/result.json"
}
arg() { jq -r --arg k "$1" '.args[$k] // empty | strings' "$TD/decision.json" 2>/dev/null; }
passup() { case "$1" in 79|75) exit "$1" ;; esac; }

# identity_for_write: the repository and the run branch, derived again from the
# context (an adopted branch is checked against the adoption rule again).
identity_for_write() {
  if ! build_repo_slug "$LAUNCH"; then result failed "the repository is unknown: ${BUILD_REPO_ERR}; nothing was written to GitHub"; return 1; fi
  if [ "$BUILD_REPO_MISMATCH" = true ]; then
    result refused "the context pr_repository names another repository than the launch checkout's origin (${BUILD_REPO}); nothing was written"; return 1
  fi
  if ! build_run_branch; then
    result refused "the run branch is not known from worktree_setup.branch_name or existing_branch; nothing was written"; return 1
  fi
  if [ "$BUILD_BRANCH_SOURCE" = adopted ] && ! adoption_ok "$BUILD_BRANCH" "$(jq -r '.number // empty | tostring' "$STATE/pr.json" 2>/dev/null)"; then
    result refused "the adopted branch ${BUILD_BRANCH} no longer meets the adoption rule (${ADOPT_WHY}); nothing was written"; return 1
  fi
  return 0
}

# own_verified <need_open>: the run's own pull request, read live. Writes the
# outcome itself and returns 1 when there is none or it fails the checks.
own_verified() {
  local rc
  build_own_pr; rc=$?; passup "$rc"
  if [ -z "$OWN_PR" ]; then
    if [ -n "$CROSS_PR" ]; then result refused-cross-repo "the only open pull request on branch ${BUILD_BRANCH} is #${CROSS_PR}, from another repository; a build never changes a cross-repository pull request"
    elif [ "$PR_LIST_FAILED" = true ]; then result failed "the run's pull request could not be looked up: ${PR_LIST_ERR}"
    else result no-pr "no open pull request of this run was found on branch ${BUILD_BRANCH}; open-pr opens it"; fi
    return 1
  fi
  build_verify_pr "$OWN_PR" "$BUILD_BRANCH" "$1"; rc=$?; passup "$rc"
  [ "$VERIFY_CLASS" = ok ] && return 0
  result "$VERIFY_CLASS" "$VERIFY_WHY" "$(jq -cn --argjson n "$OWN_PR" --arg h "$OWN_HOW" '{pr:$n, found_by:$h}')"
  return 1
}

record_pr() { # record_pr <number> <url> <how>
  [ -n "$STATE" ] || return 0
  jq -cn --argjson n "$1" --arg u "$2" --arg h "$3" --arg b "$BUILD_BRANCH" --arg t "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{schema_version:1, number:$n, url:$u, how:$h, branch:$b, time:$t}' | goal_atomic_write "$STATE/pr.json"
}

# --- open-pr -------------------------------------------------------------------
act_open_pr() {
  local tf="$S/title" bf="$S/body" bad tok out rc url n base text tres
  jq -e '(.args.title | type) == "string" and (.args.body | type) == "string"' "$TD/decision.json" >/dev/null 2>&1 \
    || { result invalid-args "open-pr needs args.title and args.body as strings"; return 0; }
  jq -j '.args.title' "$TD/decision.json" > "$tf"; jq -j '.args.body' "$TD/decision.json" > "$bf"
  n="$(wc -c < "$tf" | tr -d ' ')"
  [ "$n" -ge 1 ] && [ "$n" -le 256 ] || { result invalid-args "the title is ${n} bytes; it must be one line of 1 to 256 bytes"; return 0; }
  bad="$(LC_ALL=C tr -d '\040-\176\200-\377' < "$tf" | wc -c | tr -d ' ')"
  [ "$bad" -eq 0 ] || { result invalid-args "the title holds a line break or another control character; it must be one line"; return 0; }
  n="$(wc -c < "$bf" | tr -d ' ')"
  [ "$n" -ge 1 ] && [ "$n" -le 60000 ] || { result invalid-args "the body is ${n} bytes; it must be 1 to 60,000 bytes"; return 0; }
  bad="$(LC_ALL=C tr -d '\011\012\015\040-\176\200-\377' < "$bf" | wc -c | tr -d ' ')"
  [ "$bad" -eq 0 ] || { result invalid-args "the body holds NUL or a control character other than tab, carriage return and line feed"; return 0; }
  if ! goal_secret_screen "$tf" || ! goal_secret_screen "$bf"; then
    result refused-secret "the title or body matched the secret screen (${GOAL_SECRET_CLASS}); nothing was created or kept"; return 0
  fi
  # The mention and closing-keyword filter is an argument check: it runs before
  # the identity, with the repository name when origin gives one.
  build_repo_slug "$LAUNCH" || :
  # The operator's guidance is part of the allowed text (later guidance
  # governs); a read that fails leaves it out and says so in the result.
  build_allow_text "$S/allow" "$(jq -r '.url // empty | strings' "$STATE/pr.json" 2>/dev/null)"
  if ! tok="$(build_mentions_allowed "$bf" "$S/allow")" || ! tok="$(build_mentions_allowed "$tf" "$S/allow")"; then
    local unread='' extra
    if [ -n "$BUILD_GUIDANCE_UNREAD" ]; then
      unread="; the run's guidance could not be read (amplihack recipe guidance show exited ${BUILD_GUIDANCE_UNREAD}), so it was not counted"
      extra="$(jq -cn --arg t "$tok" --argjson g "$BUILD_GUIDANCE_UNREAD" '{refused:$t, guidance_unread:$g}')"
    else
      extra="$(jq -cn --arg t "$tok" '{refused:$t}')"
    fi
    result invalid-args "the text holds ${tok}, which the task text, the goal notes, closing_reference, issue_number and the operator's guidance do not give${unread}; a build adds no mention or closing keyword of its own. Raise it in a comment if it is needed" "$extra"
    return 0
  fi
  identity_for_write || return 0
  mkdir -p -- "$STATE/pr-text" && cp -- "$tf" "$STATE/pr-text/title" && cp -- "$bf" "$STATE/pr-text/body" \
    || { result failed "the intended text could not be kept in the state folder"; return 0; }
  build_own_pr; rc=$?; passup "$rc"
  # The recorded pull request, then the one open on the branch: either one,
  # read live as open on this branch, is the run's own and is not opened again.
  for n in "$STATE_PR" "$BRANCH_PR"; do
    goal_is_pr "$n" || continue
    build_verify_pr "$n" "$BUILD_BRANCH" true; rc=$?; passup "$rc"
    if [ "$VERIFY_CLASS" = ok ]; then
      url="$(printf '%s' "$VERIFY_JSON" | jq -r '.url // empty | strings')"
      record_pr "$n" "$url" "$([ "$n" = "$STATE_PR" ] && echo state || echo branch)"
      text="$(apply_text "$n" "$tf" "$bf")"; rc=$?; passup "$rc"
      result already-open "pull request #${n} is already open on branch ${BUILD_BRANCH}; it was recorded, not created again, and its text read back: $(printf '%s' "$text" | jq -r .result)" \
        "$(jq -cn --argjson n "$n" --arg u "$url" --argjson t "$text" '{pr:$n, url:$u, text:$t}')"
      return 0
    fi
    # A pull request that could not be read is unknown: never open a second one.
    [ "$VERIFY_CLASS" != failed ] || { result failed "${VERIFY_WHY}; a second pull request is not opened while the first cannot be read"; return 0; }
  done
  if [ "$PR_LIST_FAILED" = true ]; then
    result failed "the open pull requests on branch ${BUILD_BRANCH} could not be listed (${PR_LIST_ERR}), so a second pull request is not opened; read again next turn"; return 0
  fi
  [ -z "$CROSS_PR" ] || { result refused-cross-repo "pull request #${CROSS_PR} from another repository is open on a branch named ${BUILD_BRANCH}; nothing was created"; return 0; }
  git -C "$LAUNCH" ls-remote --exit-code --heads origin "refs/heads/${BUILD_BRANCH}" >/dev/null 2>"$S/ls.err"; rc=$?
  case "$rc" in
    0) ;;
    2) result not-pushed "branch ${BUILD_BRANCH} is not on origin yet; push it (git push -u origin ${BUILD_BRANCH}) and choose open-pr again"; return 0 ;;
    *) result failed "origin could not be asked for branch ${BUILD_BRANCH} (git ls-remote exit ${rc}: $(build_redact_tail < "$S/ls.err"))"; return 0 ;;
  esac
  base="${RECIPE_VAR_base_branch:-}"; build_valid_branch "$base" || base="$ID_BASE"
  if ! build_valid_branch "$base"; then
    base="$(gh repo view --json defaultBranchRef --jq .defaultBranchRef.name 2>/dev/null)"; rc=$?; passup "$rc"
    build_valid_branch "$base" || { result failed "the base branch is not known from base_branch, the run identity or the repository default"; return 0; }
  fi
  out="$(cd "$LAUNCH" && gh pr create --draft --head "$BUILD_BRANCH" --base "$base" "--title=$(cat "$tf")" --body-file "$bf" 2>"$S/create.err")"; rc=$?
  passup "$rc"
  if [ "$rc" -ne 0 ]; then result failed "gh pr create exited ${rc}: $(build_redact_tail < "$S/create.err")"; return 0; fi
  url="$(printf '%s\n' "$out" | grep -oE 'https://[^[:space:]]+/pull/[0-9]+' | tail -n 1)"
  n="${url##*/}"
  goal_is_pr "$n" || { result failed "gh pr create exited 0 but printed no pull request URL; read again next turn"; return 0; }
  record_pr "$n" "$url" opened
  text="$(apply_text "$n" "$tf" "$bf")"; rc=$?; passup "$rc"
  tres="$(printf '%s' "$text" | jq -r .result)"
  if [ "$tres" = differs ]; then
    result text-differs "pull request #${n} was opened as a draft into ${base}, and its text still differs after one correction: $(printf '%s' "$text" | jq -r '.differs | join(", ")')" \
      "$(jq -cn --argjson n "$n" --arg u "$url" --argjson t "$text" '{pr:$n, url:$u, text:$t}')"
  else
    result opened "pull request #${n} was opened as a draft into ${base}; its text read back ${tres}" \
      "$(jq -cn --argjson n "$n" --arg u "$url" --argjson t "$text" '{pr:$n, url:$u, text:$t}')"
  fi
}

apply_text() { # apply_text <pr> <title file> <body file>: pr_text_apply's JSON line
  local line rc
  # shellcheck source=workflow_pr_text.sh
  . "$HERE/workflow_pr_text.sh" || { jq -cn '{result:"unread", differs:[], error:"workflow_pr_text.sh is missing"}'; return 0; }
  line="$(pr_text_apply "$1" "$2" "$3")"; rc=$?
  passup "$rc"
  printf '%s' "$line" | jq -c 'select(type == "object")' 2>/dev/null || jq -cn '{result:"unread", differs:[], error:"the read-back gave no result"}'
}

# --- mark-ready ------------------------------------------------------------------
act_mark_ready() {
  local before after rc
  identity_for_write || return 0
  own_verified true || return 0
  if [ "${AUTODRIVE_GUIDANCE_UNKNOWN:-false}" != false ]; then
    result guidance-unknown "guidance could not be read again after this turn's decision, so the pull request was not marked ready; the next turn reads guidance again and decides"
    return 0
  fi
  before="$(printf '%s' "$VERIFY_JSON" | jq -r '.isDraft | tostring')"
  (cd "$LAUNCH" && gh pr ready "$OWN_PR") >/dev/null 2>"$S/ready.err"; rc=$?
  passup "$rc"
  [ "$rc" -eq 0 ] || { result failed "gh pr ready ${OWN_PR} exited ${rc}: $(build_redact_tail < "$S/ready.err"); the next turn reads the state again" "$(jq -cn --argjson n "$OWN_PR" '{pr:$n}')"; return 0; }
  after="$(gh pr view "$OWN_PR" --json isDraft --jq .isDraft 2>/dev/null)"; rc=$?
  passup "$rc"
  case "$(printf '%s' "$after" | tr -d '[:space:]')" in
    false) if [ "$before" = false ]; then result already-ready "pull request #${OWN_PR} was already ready for review" "$(jq -cn --argjson n "$OWN_PR" '{pr:$n}')"
           else result marked-ready "pull request #${OWN_PR} is marked ready for review" "$(jq -cn --argjson n "$OWN_PR" '{pr:$n}')"; fi ;;
    true) result still-draft "gh pr ready exited 0, but pull request #${OWN_PR} still reads as a draft" "$(jq -cn --argjson n "$OWN_PR" '{pr:$n}')" ;;
    *) result failed "gh pr ready exited 0, but the draft flag of pull request #${OWN_PR} could not be read back (exit ${rc}); the next reading shows it" "$(jq -cn --argjson n "$OWN_PR" '{pr:$n}')" ;;
  esac
}

# --- review and wait: PR 1's goal tool, at the run's own pull request -----------
delegate() { # delegate <review|wait>
  local wt rc
  identity_for_write || return 0
  own_verified true || return 0
  build_identity_load || :
  wt="${ID_WORKTREE:-$(build_ws_field worktree_path)}"
  if ! build_worktree_check "$wt" "$BUILD_BRANCH" "$LAUNCH"; then
    if [ "$1" = review ]; then result failed "a review round needs the run's worktree: ${BUILD_WT_WHY}; choose prepare-worktree first"; return 0; fi
    wt="$LAUNCH"
  fi
  RECIPE_VAR_pr_number="$OWN_PR" AUTODRIVE_WORKTREE="$wt" AUTODRIVE_REPO="$LAUNCH" AUTODRIVE_GOAL_CONTRACT=1 \
    bash "$HERE/autodrive_pr_goal.sh" act "$1" "$TD" >"$S/delegate.log" 2>&1; rc=$?
  passup "$rc"
  if [ "$rc" -ne 0 ] || ! goal_json_file "$TD/result.json" 65536; then
    result failed "the ${1} step of autodrive_pr_goal.sh exited ${rc}: $(build_redact_tail < "$S/delegate.log")" "$(jq -cn --argjson n "$OWN_PR" '{pr:$n}')"
  fi
}

# --- prepare-worktree ------------------------------------------------------------
ADOPT_WHY=""; ADOPT_AUTHOR=""
# adoption_ok <branch> <pr>: SEC-4. The pull request is open, same-repository,
# on <branch>, and is the recorded one, the context pr_number, one the task
# text names, or one the authenticated user wrote. Never the base, default or
# a protected branch.
adoption_ok() {
  local b="$1" n="$2" v rc def prot login
  ADOPT_WHY=""; ADOPT_AUTHOR=""
  goal_is_pr "$n" || { ADOPT_WHY="no pull request number is known for ${b}"; return 1; }
  def="$(gh repo view --json defaultBranchRef --jq .defaultBranchRef.name 2>/dev/null)"; rc=$?; passup "$rc"
  if [ "$rc" -ne 0 ] || [ -z "$def" ]; then ADOPT_WHY="the default branch could not be read"; return 1; fi
  if [ "$b" = "$def" ] || [ "$b" = "${RECIPE_VAR_base_branch:-}" ] || [ "$b" = "$ID_BASE" ]; then ADOPT_WHY="${b} is the base or default branch"; return 1; fi
  prot="$(gh api "repos/${BUILD_REPO_NAME}/branches/${b}" --jq .protected 2>/dev/null)"; rc=$?; passup "$rc"
  case "$rc:$prot" in 0:false) ;; 0:true) ADOPT_WHY="${b} is a protected branch"; return 1 ;; *) ADOPT_WHY="whether ${b} is protected could not be read"; return 1 ;; esac
  build_verify_pr "$n" "$b" true; rc=$?; passup "$rc"
  [ "$VERIFY_CLASS" = ok ] || { ADOPT_WHY="$VERIFY_WHY"; return 1; }
  ADOPT_AUTHOR="$(printf '%s' "$VERIFY_JSON" | jq -r '.author.login // empty | strings' | goal_clean_line 100)"
  [ "$n" = "$(jq -r '.number // empty | tostring' "$STATE/pr.json" 2>/dev/null)" ] && return 0
  [ "$n" = "${RECIPE_VAR_pr_number:-}" ] && return 0
  build_task_text "$S/adopt-task"
  LC_ALL=C grep -qE "(#|/pull/)${n}([^0-9]|\$)" "$S/adopt-task" && return 0
  login="$(gh api user --jq .login 2>/dev/null)"; rc=$?; passup "$rc"
  [ "$rc" -eq 0 ] && [ -n "$login" ] && [ "$login" = "$ADOPT_AUTHOR" ] && return 0
  ADOPT_WHY="pull request #${n} is not the recorded one, not the context pr_number, not named in the task text, and not written by the authenticated user"
  return 1
}

# worktree_for <branch>: an existing linked worktree on <branch>, or a new one
# under the repository's worktree root. Never -f, never a reset.
WT_PATH=""
worktree_for() {
  local b="$1" listed root target rc
  WT_PATH=""
  listed="$(build_worktree_lookup "$LAUNCH" "$b")"
  if [ -n "$listed" ] && [ -d "$listed" ]; then WT_PATH="$(cd "$listed" && pwd -P)"; return 0; fi
  git -C "$LAUNCH" worktree prune 2>/dev/null || :
  root="$(bash "$HERE/workflow_worktree_root.sh" root "$LAUNCH" 2>/dev/null)" || { echo "the repository's main checkout could not be worked out"; return 1; }
  target="${ID_WORKTREE:-}"
  if [ -z "$target" ] || [ -e "$target" ] || [ -L "$target" ] || ! build_path_under "$target" "$root/worktrees"; then target="$root/worktrees/$b"; fi
  build_path_under "$target" "$root/worktrees" || { echo "the worktree path for ${b} is not a plain path under ${root}/worktrees"; return 1; }
  [ ! -e "$target" ] && [ ! -L "$target" ] || { echo "the worktree path ${target} is already occupied"; return 1; }
  mkdir -p -- "${target%/*}" || { echo "the folder for ${target} could not be made"; return 1; }
  if git -C "$LAUNCH" rev-parse --verify -q "refs/heads/${b}" >/dev/null; then
    git -C "$LAUNCH" worktree add -- "$target" "$b" >"$S/wt.log" 2>&1; rc=$?
  else
    git -C "$LAUNCH" fetch --quiet --no-tags origin "+refs/heads/${b}:refs/remotes/origin/${b}" >"$S/wt.log" 2>&1 \
      && git -C "$LAUNCH" worktree add --track -b "$b" -- "$target" "origin/${b}" >>"$S/wt.log" 2>&1; rc=$?
  fi
  [ "$rc" -eq 0 ] || { echo "git worktree add failed (exit ${rc}): $(build_redact_tail < "$S/wt.log")"; return 1; }
  WT_PATH="$(cd "$target" && pwd -P)"
}

write_identity() { # write_identity <branch> <worktree> <source> [previous json]
  local prev="${4:-}"
  [ -n "$prev" ] || prev=null
  jq -cn --arg b "$1" --arg w "$2" --arg s "$3" --arg base "$ID_BASE" --arg st "$ID_STARTED" --argjson p "$prev" \
    '{schema_version:1, branch:$b, worktree:$w, source:$s, base:$base, started:$st} + (if $p == null then {} else {previous:$p} end)' \
    | goal_atomic_write "$STATE/identity.json"
}

linked_names() { # the host inputs that are links in the worktree, as a JSON array
  bash "$HERE/workflow_worktree_host_inputs.sh" link "$LAUNCH" "$1" 2>>"$S/link.log" >/dev/null || :
  bash "$HERE/workflow_worktree_host_inputs.sh" facts "$LAUNCH" "$1" 2>/dev/null \
    | jq -c '[.worktree[]? | select(.kind == "link") | .name]' 2>/dev/null || echo '[]'
}

act_prepare_worktree() {
  local want why linked prev n
  [ -n "$STATE" ] || { result failed "no state folder (AUTODRIVE_STATE_DIR)"; return 0; }
  want="$(arg branch)"
  if ! build_repo_slug "$LAUNCH"; then result failed "the repository is unknown: ${BUILD_REPO_ERR}"; return 0; fi
  build_run_branch || :
  build_identity_load || :
  if [ -n "$want" ] && [ "$want" != "$BUILD_BRANCH" ]; then
    build_valid_branch "$want" || { result invalid-args "args.branch is not a valid branch name"; return 0; }
    n="$(arg pr)"
    if ! goal_is_pr "$n"; then
      n="$(gh pr list --head "$want" --state open --json number,headRefName,isCrossRepository 2>/dev/null)"; passup "$?"
      n="$(printf '%s' "$n" | jq -r --arg b "$want" '[.[]? | select(.headRefName == $b and .isCrossRepository == false) | .number] | sort | .[0] // empty' 2>/dev/null)"
    fi
    if ! adoption_ok "$want" "$n"; then result refused "branch ${want} was not adopted: ${ADOPT_WHY}"; return 0; fi
    if ! why="$(worktree_for "$want")"; then result failed "branch ${want} could not be given a worktree: ${why}"; return 0; fi
    prev="$(jq -c '{branch, worktree, source}' "$STATE/identity.json" 2>/dev/null)"; [ -n "$prev" ] || prev=null
    BUILD_BRANCH="$want"
    write_identity "$want" "$WT_PATH" adopted "$prev" || { result failed "identity.json could not be written"; return 0; }
    record_pr "$n" "$(printf '%s' "$VERIFY_JSON" | jq -r '.url // empty | strings')" adopted
    echo "autodrive_build_goal: adopted branch ${want} for pull request #${n}, written by ${ADOPT_AUTHOR:-an unknown author}" >&2
    linked="$(linked_names "$WT_PATH")"
    result ready "branch ${want} of pull request #${n} is adopted, with its worktree at ${WT_PATH}" \
      "$(jq -cn --arg w "$WT_PATH" --argjson l "$linked" --argjson n "$n" '{worktree:$w, linked:$l, adopted:true, pr:$n}')"
    return 0
  fi
  [ -n "$BUILD_BRANCH" ] || { result failed "the run branch is not known from worktree_setup.branch_name or existing_branch; name one in args.branch"; return 0; }
  if ! why="$(worktree_for "$BUILD_BRANCH")"; then result failed "the worktree for ${BUILD_BRANCH} could not be restored: ${why}"; return 0; fi
  write_identity "$BUILD_BRANCH" "$WT_PATH" "${ID_SOURCE:-$BUILD_BRANCH_SOURCE}" || { result failed "identity.json could not be written"; return 0; }
  linked="$(linked_names "$WT_PATH")"
  result ready "the worktree for ${BUILD_BRANCH} is at ${WT_PATH}; linked host inputs: $(printf '%s' "$linked" | jq -r 'if length == 0 then "none" else join(", ") end')" \
    "$(jq -cn --arg w "$WT_PATH" --argjson l "$linked" '{worktree:$w, linked:$l}')"
}

# --- final and publish -------------------------------------------------------------
do_final() {
  local n v rc out word
  build_repo_slug "$LAUNCH" || return 0
  build_run_branch || :
  for n in "$(jq -r '.number // empty | tostring' "$STATE/pr.json" 2>/dev/null)" "${RECIPE_VAR_pr_number:-}"; do
    goal_is_pr "$n" || continue
    v="$(gh pr view "$n" --json state,headRefName,isCrossRepository 2>/dev/null)"; rc=$?; passup "$rc"
    [ "$rc" -eq 0 ] && printf '%s' "$v" | jq -e 'type == "object" and .isCrossRepository == false' >/dev/null 2>&1 || continue
    [ -z "$BUILD_BRANCH" ] || [ "$(printf '%s' "$v" | jq -r '.headRefName')" = "$BUILD_BRANCH" ] || continue
    case "$(printf '%s' "$v" | jq -r '.state')" in OPEN) return 0 ;; MERGED) echo merged; return 0 ;; CLOSED) echo closed; return 0 ;; esac
  done
  [ -n "$BUILD_BRANCH" ] || return 0
  out="$(gh pr list --head "$BUILD_BRANCH" --state all --limit 30 --json number,state,headRefName,isCrossRepository,createdAt 2>/dev/null)"; rc=$?; passup "$rc"
  [ "$rc" -eq 0 ] || return 0
  word="$(printf '%s' "$out" | jq -r --arg b "$BUILD_BRANCH" --arg since "$ID_STARTED" '
    if type != "array" then empty else
    [.[] | objects | select(.headRefName == $b and .isCrossRepository == false)] as $mine
    | if any($mine[]; .state == "OPEN") then empty
      else [$mine[] | select($since == "" or ((.createdAt // "") >= $since))] | sort_by(.number) | last
        | if . == null then empty elif .state == "MERGED" then "merged" elif .state == "CLOSED" then "closed" else empty end end end' 2>/dev/null)"
  [ -z "$word" ] || echo "$word"
}

do_publish() {
  local f="${1:-}" rc
  [ -f "$f" ] || { echo "ERROR: publish needs a body file" >&2; exit 2; }
  build_repo_slug "$LAUNCH" || { echo "ERROR: the repository is unknown: ${BUILD_REPO_ERR}" >&2; exit 1; }
  [ "$BUILD_REPO_MISMATCH" = false ] || { echo "ERROR: pr_repository names another repository than origin; nothing was posted" >&2; exit 1; }
  build_run_branch || { echo "ERROR: the run branch is not known, so there is no pull request of this run to post to" >&2; exit 1; }
  build_own_pr; rc=$?; passup "$rc"
  goal_is_pr "$OWN_PR" || { echo "ERROR: this run has no pull request yet; nothing was posted (the text stays in the turn folder)" >&2; exit 1; }
  build_verify_pr "$OWN_PR" "$BUILD_BRANCH" false; rc=$?; passup "$rc"
  [ "$VERIFY_CLASS" = ok ] || { echo "ERROR: ${VERIFY_WHY}; nothing was posted" >&2; exit 1; }
  goal_secret_screen "$f" || { echo "ERROR: the comment matched the secret screen (${GOAL_SECRET_CLASS}); nothing was posted" >&2; exit 1; }
  (cd "$LAUNCH" && exec gh pr comment "$OWN_PR" --body-file "$f"); rc=$?
  passup "$rc"
  [ "$rc" -eq 0 ] || exit 1
}

case "${1:-}" in
  actions) [ "$#" -eq 1 ] || { echo "usage: autodrive_build_goal.sh actions" >&2; exit 2; }; echo "$ACTIONS" ;;
  final) do_final ;;
  read)
    [ "$#" -eq 2 ] || { echo "usage: autodrive_build_goal.sh read <turn_dir>" >&2; exit 2; }
    build_turn_dir "$2" || exit 2
    bash "$HERE/autodrive_build_reading.sh" "$2" ;;
  act)
    [ "$#" -eq 3 ] || { echo "usage: autodrive_build_goal.sh act <word> <turn_dir>" >&2; exit 2; }
    WORD="$2"; TD="$3"
    case " $ACTIONS " in *" $WORD "*) ;; *) echo "ERROR: act takes one of: ${ACTIONS}" >&2; exit 2 ;; esac
    build_turn_dir "$TD" || exit 2
    S="$(mktemp -d "$TD/.act.XXXXXX")" || exit 2
    rm -f -- "$TD/result.json"
    if ! goal_json_file "$TD/decision.json" 131072; then
      result invalid-args "there is no decision.json of at most 128 KiB in the turn folder, so ${WORD} did not run"
    elif [ "$(jq -r '.action // empty | strings' "$TD/decision.json")" != "$WORD" ]; then
      result invalid-args "decision.json names another action than ${WORD}, so nothing ran"
    elif ! jq -e '(.args // {}) | type == "object"' "$TD/decision.json" >/dev/null 2>&1; then
      result invalid-args "args in decision.json is not an object"
    else
      case "$WORD" in
        open-pr) act_open_pr ;;
        mark-ready) act_mark_ready ;;
        review|wait) delegate "$WORD" ;;
        prepare-worktree) act_prepare_worktree ;;
      esac
    fi
    goal_json_file "$TD/result.json" 65536 || result failed "the ${WORD} action wrote no result" ;;
  publish) [ "$#" -eq 2 ] || { echo "usage: autodrive_build_goal.sh publish <body_file>" >&2; exit 2; }; do_publish "$2" ;;
  *) echo "usage: autodrive_build_goal.sh actions|final|read <turn_dir>|act <word> <turn_dir>|publish <body_file>" >&2; exit 2 ;;
esac
