#!/usr/bin/env bash
# autodrive_trust.sh — the range check and the qa evidence chain for
# auto-drive-to-merge (issue #1517, D4 to D6).
#
# Criterion 3 is met by a CLEAN crusty round, and a clean verdict covers only
# the head crusty reviewed. autodrive_crusty_range walks the commits made
# after that head and allows only base merges and description or evidence
# changes; anything else goes back to crusty. Criterion 1 is met by the qa
# evidence, which an agent step running as the same user could edit after it
# was measured; autodrive_qa_trusted accepts it only through the hash chain
# that starts at the loop's merge-ready-records.tsv manifest.
#
# Every function prints only a fixed token, a hex SHA, or a JSON line built
# from those. Commit subjects, paths and file contents are never printed, so
# nothing from the branch under review reaches a log or a prompt through here.
#
# Callers source autodrive_state.sh first: autodrive_blob_hash,
# autodrive_manifest_row, autodrive_crusty_final and autodrive_clear_phase live
# there. This file only DEFINES functions and one read-only variable; sourcing
# it has no other side effect. Needs bash 3.2 or later, git and coreutils.
#
# Same-user limit: these checks catch an agent following instructions found in
# branch text, an accidental edit, and the replay of an older record. They are
# not a boundary against a deliberate forger running as the same user; the
# platform (required CI, review state, --match-head-commit) is. See
# docs/reference/auto-drive-to-merge.md#trust-model.

# --- the allowlist ---------------------------------------------------------
#
# The only paths a commit after the clean crusty round may change without a
# new crusty review. Matching is byte-exact and case-sensitive. An entry that
# ends in `/` is a prefix; a path under it that contains `..` does not match.
# Scenario files are tests, and tests are code, so they are not on the list.
case "$(declare -p AUTODRIVE_RANGE_ALLOWLIST 2>/dev/null)" in
  "declare -ar "*) ;;
  *) readonly -a AUTODRIVE_RANGE_ALLOWLIST=("PR_DESCRIPTION.md" ".github/pull_request_template.md" ".autodrive/evidence/") ;;
esac

# --- small checks ----------------------------------------------------------

# autodrive_is_sha <value>: a whole value of 40 or 64 lowercase hex characters.
autodrive_is_sha() {
  case "${#1}" in 40|64) ;; *) return 1 ;; esac
  case "$1" in *[!0123456789abcdef]*) return 1 ;; esac
  return 0
}

# autodrive_trust_git <repo> <git args...>: git with history rewriting turned
# off. Replace refs and grafts cannot change what is walked, an inherited
# GIT_DIR or GIT_WORK_TREE cannot point git at another repository, and git
# never prompts for credentials.
autodrive_trust_git() {
  local repo="$1"; shift
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE -u GIT_OBJECT_DIRECTORY \
    -u GIT_ALTERNATE_OBJECT_DIRECTORIES -u GIT_COMMON_DIR -u GIT_REPLACE_REF_BASE \
    GIT_NO_REPLACE_OBJECTS=1 GIT_GRAFT_FILE=/dev/null GIT_TERMINAL_PROMPT=0 LC_ALL=C \
    git -C "$repo" "$@"
}

# autodrive_range_path_allowed <path>: the path is on AUTODRIVE_RANGE_ALLOWLIST.
autodrive_range_path_allowed() {
  local p="$1" e
  for e in "${AUTODRIVE_RANGE_ALLOWLIST[@]}"; do
    case "$e" in
      */) case "$p" in "$e"?*) case "$p" in *..*) ;; *) return 0 ;; esac ;; esac ;;
      *) [ "$p" = "$e" ] && return 0 ;;
    esac
  done
  return 1
}

# --- the base SHA ----------------------------------------------------------

# autodrive_base_sha <repo> <baseRefName> -> the SHA of the pull request's
# base branch, fetched now, or "" (return 1).
#
# The SHA never comes from an existing local ref, which could be stale or set
# by anyone with write access to the clone. The name must pass
# `git check-ref-format --branch`, must not start with `-`, and may contain
# only A-Z a-z 0-9 . _ / -. The fetch forces refs/remotes/origin/<base> to the
# remote's value; if it fails, the result is "" and the commits a base merge
# brought in count as code.
autodrive_base_sha() {
  local repo="${1:-}" ref="${2:-}" sha=""
  [ -n "$repo" ] && [ -n "$ref" ] || { printf '\n'; return 1; }
  case "$ref" in
    -*|*[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/-]*) printf '\n'; return 1 ;;
  esac
  if ! autodrive_trust_git "$repo" check-ref-format --branch "$ref" >/dev/null 2>&1 \
     || ! autodrive_trust_git "$repo" fetch --quiet --no-tags --force --end-of-options origin \
          "+refs/heads/${ref}:refs/remotes/origin/${ref}" >/dev/null 2>&1; then
    printf '\n'; return 1
  fi
  sha="$(autodrive_trust_git "$repo" rev-parse --verify --quiet "refs/remotes/origin/${ref}^{commit}" 2>/dev/null)"
  autodrive_is_sha "$sha" || { printf '\n'; return 1; }
  printf '%s\n' "$sha"
}

# --- the range rule --------------------------------------------------------

# autodrive_range_allowed <repo> <commit> <base_sha> -> return 0 when the
# commit needs no crusty review, 1 when it is code, 2 when it cannot be read.
#
# Allowed are (a) a base merge: exactly two parents, the second an ancestor of
# base_sha, and a tree equal to `git merge-tree --write-tree <p1> <p2>`; and
# (b) a one-parent commit whose diff against that parent lists at least one
# path, every path on the allowlist, every new mode 100644, 100755 or 000000.
# Every other commit is code: root commits, empty commits, octopus merges,
# merges of other branches, merges with hand-resolved conflicts or extra
# edits, symlinks (120000) and submodules (160000). `merge-tree --write-tree`
# needs git 2.38; on older git it fails and the merge counts as code.
autodrive_range_allowed() {
  local -x LC_ALL=C
  local repo="${1:-}" c="${2:-}" base="${3:-}" parents="" p1="" p2="" extra="" tree="" merged=""
  local meta="" path="" mode="" n=0 out=""
  autodrive_is_sha "$c" || return 2
  [ -z "$base" ] || autodrive_is_sha "$base" || return 2
  parents="$(autodrive_trust_git "$repo" show -s --format=%P "$c" 2>/dev/null)" || return 2
  read -r p1 p2 extra <<EOF
$parents
EOF
  [ -n "$p1" ] || return 1
  if [ -n "$p2" ]; then
    [ -z "$extra" ] && [ -n "$base" ] || return 1
    autodrive_trust_git "$repo" merge-base --is-ancestor "$p2" "$base" 2>/dev/null || return 1
    tree="$(autodrive_trust_git "$repo" rev-parse --verify --quiet "${c}^{tree}" 2>/dev/null)" || return 2
    # Exit 1 means conflicts, and a merge with conflicts is never a clean base merge.
    merged="$(autodrive_trust_git "$repo" merge-tree --write-tree "$p1" "$p2" 2>/dev/null)" || return 1
    merged="${merged%%
*}"
    autodrive_is_sha "$merged" && [ "$merged" = "$tree" ] && return 0
    return 1
  fi
  out="$(mktemp "${TMPDIR:-/tmp}/autodrive-range.XXXXXX" 2>/dev/null)" || return 2
  if ! autodrive_trust_git "$repo" diff-tree -r -z --no-renames --raw "$p1" "$c" > "$out" 2>/dev/null; then
    rm -f -- "$out"; return 2
  fi
  while IFS= read -r -d '' meta && IFS= read -r -d '' path; do
    n=$((n + 1))
    mode="${meta#:}"; mode="${mode#* }"; mode="${mode%% *}"
    case "$mode" in 100644|100755|000000) ;; *) rm -f -- "$out"; return 1 ;; esac
    autodrive_range_path_allowed "$path" || { rm -f -- "$out"; return 1; }
  done < "$out"
  rm -f -- "$out"
  [ "$n" -gt 0 ] || return 1
  return 0
}

# autodrive_crusty_range <repo> <reviewed> <head> <base_sha> -> one of
#
#   ok                                 every commit after <reviewed> is allowed
#   crusty-unreviewed-commits:<sha>    the oldest commit that needs crusty
#   crusty-range-unreadable            a bad SHA, a git failure, or a shallow clone
#
# The walk is `git rev-list --reverse --topo-order <reviewed>..<head> ^<base_sha>`
# (no ^<base_sha> when it is empty), so the commits a base merge brings in are
# not walked. <reviewed> must be an ancestor of <head>: a head moved back
# behind the reviewed commit, or a rewritten history, leaves the walk empty, so
# it is reported as crusty-unreviewed-commits:<head> instead. Returns 0 only
# for ok.
autodrive_crusty_range() {
  local -x LC_ALL=C
  local repo="${1:-}" reviewed="${2:-}" head="${3:-}" base="${4:-}" list="" c="" rc=0
  local -a excl=()
  if ! autodrive_is_sha "$reviewed" || ! autodrive_is_sha "$head" \
     || { [ -n "$base" ] && ! autodrive_is_sha "$base"; } || [ -z "$repo" ]; then
    printf 'crusty-range-unreadable\n'; return 1
  fi
  if [ "$(autodrive_trust_git "$repo" rev-parse --is-shallow-repository 2>/dev/null)" != "false" ]; then
    printf 'crusty-range-unreadable\n'; return 1
  fi
  for c in "$reviewed" "$head" $base; do
    autodrive_trust_git "$repo" cat-file -e "${c}^{commit}" 2>/dev/null || { printf 'crusty-range-unreadable\n'; return 1; }
  done
  autodrive_trust_git "$repo" merge-base --is-ancestor "$reviewed" "$head" 2>/dev/null; rc=$?
  case "$rc" in
    0) ;;
    1) printf 'crusty-unreviewed-commits:%s\n' "$head"; return 1 ;;
    *) printf 'crusty-range-unreadable\n'; return 1 ;;
  esac
  rc=0
  [ -z "$base" ] || excl=("^${base}")
  list="$(autodrive_trust_git "$repo" rev-list --reverse --topo-order "${reviewed}..${head}" ${excl[@]+"${excl[@]}"} 2>/dev/null)" \
    || { printf 'crusty-range-unreadable\n'; return 1; }
  while IFS= read -r c; do
    [ -n "$c" ] || continue
    autodrive_is_sha "$c" || { printf 'crusty-range-unreadable\n'; return 1; }
    autodrive_range_allowed "$repo" "$c" "$base"; rc=$?
    case "$rc" in
      0) ;;
      1) printf 'crusty-unreviewed-commits:%s\n' "$c"; return 1 ;;
      *) printf 'crusty-range-unreadable\n'; return 1 ;;
    esac
  done <<EOF
$list
EOF
  printf 'ok\n'
}

# autodrive_rereview_decision <state_dir> <repo> <baseRefName> -> the whole
# output of merge round step-00b, one JSON line with four keys:
#
#   {"rereview":"true|false","range":"<token>","first_unreviewed_sha":"<sha>","base_sha":"<sha>"}
#
# Only a crusty loop that ended DONE and CLEAN is checked; otherwise range is
# `not-checked`, and step-01b reports why. Only crusty-unreviewed-commits
# removes the `crusty-loop` row from phases.tsv and asks for a re-review. An
# unreadable range does not: re-running crusty cannot deepen a clone or fix
# git, so step-01b, step-03 and the merge gate block on it instead.
autodrive_rereview_decision() {
  local dir="${1:-}" repo="${2:-}" ref="${3:-}" reviewed="" base="" head="" res="" first="" again="false"
  if ! reviewed="$(autodrive_crusty_final "$dir" 2>/dev/null)"; then
    printf '{"rereview":"false","range":"not-checked","first_unreviewed_sha":"","base_sha":""}\n'
    return 0
  fi
  base="$(autodrive_base_sha "$repo" "$ref")" || base=""
  head="$(autodrive_trust_git "$repo" rev-parse --verify --quiet "HEAD^{commit}" 2>/dev/null)" || head=""
  res="$(autodrive_crusty_range "$repo" "$reviewed" "$head" "$base")"
  case "$res" in
    ok) ;;
    crusty-unreviewed-commits:*)
      first="${res#*:}"; res="crusty-unreviewed-commits"
      if ! autodrive_is_sha "$first"; then
        first=""; res="crusty-range-unreadable"
      elif autodrive_clear_phase "$dir" "crusty-loop"; then
        again="true"
      else
        echo "ERROR: could not clear the crusty-loop phase in the state dir; crusty is not re-run and criterion 3 stays unmet." >&2
      fi ;;
    *) res="crusty-range-unreadable" ;;
  esac
  printf '{"rereview":"%s","range":"%s","first_unreviewed_sha":"%s","base_sha":"%s"}\n' "$again" "$res" "$first" "$base"
}

# --- the qa evidence chain -------------------------------------------------

# autodrive_qa_evidence_sha <file> -> the git blob hash of a regular file that
# is not a symlink, or "" (return 1) for anything else.
autodrive_qa_evidence_sha() {
  local f="${1:-}" h=""
  { [ -n "$f" ] && [ ! -L "$f" ] && [ -f "$f" ]; } || { printf '\n'; return 1; }
  h="$(autodrive_blob_hash "$(dirname -- "$f")" < "$f")"
  autodrive_is_sha "$h" || { printf '\n'; return 1; }
  printf '%s\n' "$h"
}

# autodrive_qa_trusted <state_dir> <record_copy> <qa_copy> <head_sha> -> `ok`
# or the token of the first check that failed:
#
#   qa-manifest-missing   merge-ready-records.tsv absent, a symlink, or its last
#                         row malformed (file name ^merge-ready-[A-Za-z0-9._-]+\.json$)
#   qa-record-modified    record_copy's hash differs from the manifest hash, or it
#                         is not one line starting with {"merge_ready_verdict":",
#                         or it lacks one 40- or 64-hex qa_evidence_sha
#   qa-evidence-modified  qa_copy's hash differs from qa_evidence_sha
#   qa-evidence-stale     qa_copy's one head_sha is not <head_sha>
#
# record_copy and qa_copy are the caller's private copies; they are hashed and
# parsed as given and the originals are never re-read, so what the caller
# reads afterwards is what was verified. The manifest is read once.
autodrive_qa_trusted() {
  local -x LC_ALL=C
  local dir="${1:-}" rec="${2:-}" qa="${3:-}" head="${4:-}" row="" want="" qsha="" qhead=""
  row="$(autodrive_manifest_row "$dir" merge-ready)" || { printf 'qa-manifest-missing\n'; return 1; }
  want="${row#* }"
  if [ -z "$rec" ] || [ ! -f "$rec" ] \
     || [ "$(autodrive_blob_hash "$dir" < "$rec")" != "$want" ] \
     || [ "$(awk 'END { print NR }' "$rec")" != "1" ] \
     || ! grep -q '^{"merge_ready_verdict":"' "$rec" \
     || [ "$(grep -o '"qa_evidence_sha":' "$rec" | wc -l | tr -d ' ')" != "1" ]; then
    printf 'qa-record-modified\n'; return 1
  fi
  qsha="$(sed -n 's/.*"qa_evidence_sha":"\([^"]*\)".*/\1/p' "$rec")"
  autodrive_is_sha "$qsha" || { printf 'qa-record-modified\n'; return 1; }
  if [ -z "$qa" ] || [ ! -f "$qa" ] || [ "$(autodrive_blob_hash "$dir" < "$qa")" != "$qsha" ]; then
    printf 'qa-evidence-modified\n'; return 1
  fi
  if [ "$(grep -o '"head_sha":' "$qa" | wc -l | tr -d ' ')" = "1" ]; then
    qhead="$(sed -n 's/.*"head_sha":"\([^"]*\)".*/\1/p' "$qa")"
  fi
  if ! autodrive_is_sha "$head" || [ "$qhead" != "$head" ]; then
    printf 'qa-evidence-stale\n'; return 1
  fi
  printf 'ok\n'
}

# --- per-scenario results (D6) ---------------------------------------------

# autodrive_scenario_results <list> -> the gadugi_scenario_results field.
#
# <list> holds one `<file><TAB><PASS|FAIL|INVALID>` line per scenario entry,
# written by the evidence step. Every byte of <file> outside A-Z a-z 0-9 . _ /
# - becomes `_`, the key is cut to 128 bytes, an unknown result becomes
# INVALID, and the entries are sorted by key with LC_ALL=C and joined by `,`.
# A missing or empty list gives "". The field is informational: nothing
# decides a pass from it.
autodrive_scenario_results() {
  local -x LC_ALL=C
  local list="${1:-}" line="" key="" res="" tab
  tab="$(printf '\t')"
  { [ -n "$list" ] && [ -f "$list" ] && [ ! -L "$list" ]; } || { printf '\n'; return 0; }
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in *"$tab"*) ;; *) continue ;; esac
    key="${line%"$tab"*}"; res="${line##*"$tab"}"
    case "$res" in PASS|FAIL|INVALID) ;; *) res="INVALID" ;; esac
    key="$(printf '%s' "$key" | tr -c 'A-Za-z0-9._/-' '_' | cut -c1-128)"
    [ -n "$key" ] && printf '%s=%s\n' "$key" "$res"
  done < "$list" | sort -t '=' -k1,1 | paste -s -d ',' -
}
