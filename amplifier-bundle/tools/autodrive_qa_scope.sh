#!/usr/bin/env bash
# autodrive_qa_scope.sh — which project auto-drive's QA judges, and whether
# gadugi is required (issue #1763). autodrive_trust.sh loads this file from
# its own physical directory and from nowhere else: never $PWD, REPO_PATH or
# the pull request's tree. It is a gate file (GATE_FILES in merge_guard.rs),
# since the merge gate's gadugi decision comes from here.
#
# Public functions. Each reads AUTODRIVE_QA_BASE (the base branch name) and
# AUTODRIVE_QA_HEAD (the head commit, default HEAD) from the environment:
#   autodrive_qa_changed_paths <dir>  the changed paths, `(unknown)` or `(none)`
#   autodrive_qa_target <dir>         `<type>\t<dir>` or `unresolved\t<list>`
#   autodrive_qa_docs_only <dir>      `true|false|unknown` (autodrive_qa_docs.sh)
# autodrive_trust.sh builds autodrive_repo_type and autodrive_gadugi_required
# on these. The declared sources, .amplihack/qa.toml and the CI workflows,
# are read by autodrive_qa_sources.sh, which only the evidence tool loads.
# Contract: docs/reference/auto-drive-to-merge.md, "Choosing the QA command".
#
# This file only DEFINES functions and runs nothing it reads: no eval and no
# shell on built text. git runs via autodrive_trust_git and never contacts a
# remote. The manifests come from the merge-base tree (HEAD's when it is
# unknown), never the working tree. Output is a fixed token or a path with
# bytes outside A-Z a-z 0-9 . _ / - as `_`. LC_ALL=C; bash 3.2+, git, tr,
# grep and sort.
#
# Computed once. _autodrive_qa_scan runs the diff and the tree walk, and
# _autodrive_qa_choose makes the project choice, once per directory, base and
# head in a shell; every public function, and autodrive_qa_sources.sh, calls
# them first. Bash functions cannot return arrays, so the results are kept in
# these variables, which only those two functions and their helpers write:
#   _AD_KEY          the directory, base and head the scan was made for
#   _AD_PATHS_STATE  known, empty (an empty diff), nobase (no base given:
#                    the head stands in for the merge base) or unknown
#   _AD_PATHS        the changed paths, when known
#   _AD_MB           the merge base, or empty when it is unknown
#   _AD_MANIFESTS    the _autodrive_manifest_dirs lines
#   _AD_CHOSEN       set once _autodrive_qa_choose has run for _AD_KEY
#   _AD_KIND, _AD_DIR, _AD_CANDS, _AD_NODE  the choice
_AD_KEY=""

# _autodrive_qa_list <entry>... -> the entries joined by single spaces, each
# byte outside A-Z a-z 0-9 . _ / - as `_`, each cut to 128 bytes, at most 10
# and then `(+N more)`. The entry `!` is a search error: `(search-error)`.
_autodrive_qa_list() {
  local -x LC_ALL=C
  local n=0 out="" e=""
  for e in "$@"; do
    n=$((n + 1))
    [ "$n" -le 10 ] || continue
    if [ "$e" = "!" ]; then
      e="(search-error)"
    else
      e="${e//[!A-Za-z0-9._\/-]/_}"
      e="${e:0:128}"
    fi
    out="${out}${out:+ }${e}"
  done
  [ "$n" -le 10 ] || out="${out} (+$((n - 10)) more)"
  printf '%s\n' "$out"
}

# _autodrive_qa_scan <dir>: the changed paths and the manifest list, computed
# only when the directory, AUTODRIVE_QA_BASE (unset and empty differ) or
# AUTODRIVE_QA_HEAD differ from the last scan in this shell.
_autodrive_qa_scan() {
  local d="${1:-.}"
  local key="${d}|${AUTODRIVE_QA_BASE+set}:${AUTODRIVE_QA_BASE-}|${AUTODRIVE_QA_HEAD-HEAD}"
  [ "$key" != "$_AD_KEY" ] || return 0
  _autodrive_qa_changed_raw "$d"
  _AD_MANIFESTS="$(_autodrive_manifest_dirs "$d")"
  _AD_CHOSEN=""
  _AD_KEY="$key"
}

# _autodrive_qa_changed_raw <dir>: sets _AD_PATHS_STATE, _AD_PATHS and _AD_MB.
# The head is AUTODRIVE_QA_HEAD (the gate's headRefOid, which must be a full
# SHA present in the clone, else everything is unknown) or HEAD. With no
# AUTODRIVE_QA_BASE (a preflight) the head is the merge base. Otherwise the
# base must pass the branch-name rules and refs/remotes/origin/<base> must
# exist. The paths are `git diff --name-only --no-renames -z <merge-base>
# <head>`; a git failure, or a newline in a path, makes them unknown.
_autodrive_qa_changed_raw() {
  local -x LC_ALL=C
  local d="${1:-.}"
  local b="${AUTODRIVE_QA_BASE:-}"
  local h="${AUTODRIVE_QA_HEAD-HEAD}"
  local base="" head="" mb="" p=""
  _AD_PATHS=()
  _AD_PATHS_STATE="unknown"
  _AD_MB=""
  if [ "$h" != "HEAD" ] && ! autodrive_is_sha "$h"; then
    return 0
  fi
  head="$(autodrive_trust_git "$d" rev-parse --verify --quiet "${h}^{commit}" 2>/dev/null)" || return 0
  autodrive_is_sha "$head" || return 0
  if [ -z "${AUTODRIVE_QA_BASE+x}" ]; then
    _AD_MB="$head"
    _AD_PATHS_STATE="nobase"
    return 0
  fi
  case "$b" in
    ''|-*|*[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/-]*) return 0 ;;
  esac
  autodrive_trust_git "$d" check-ref-format --branch "$b" >/dev/null 2>&1 || return 0
  base="$(autodrive_trust_git "$d" rev-parse --verify --quiet "refs/remotes/origin/${b}^{commit}" 2>/dev/null)" \
    || return 0
  autodrive_is_sha "$base" || return 0
  mb="$(autodrive_trust_git "$d" merge-base --end-of-options "$base" "$head" 2>/dev/null)" || return 0
  autodrive_is_sha "$mb" || return 0
  _AD_MB="$mb"
  while IFS= read -r -d '' p; do
    case "$p" in *$'\n'*) _AD_PATHS=(); return 0 ;; esac
    _AD_PATHS+=("$p")
  done < <(autodrive_trust_git "$d" diff --name-only --no-renames --no-ext-diff --no-textconv -z \
             --end-of-options "$mb" "$head" 2>/dev/null || printf '\n\0')
  if [ "${#_AD_PATHS[@]}" -eq 0 ]; then
    _AD_PATHS_STATE="empty"
  else
    _AD_PATHS_STATE="known"
  fi
}

# autodrive_qa_changed_paths <dir> -> the changed paths (as _autodrive_qa_list),
# `(none)` for an empty diff or no base, or `(unknown)` when they cannot be read.
autodrive_qa_changed_paths() {
  _autodrive_qa_scan "${1:-.}"
  case "$_AD_PATHS_STATE" in
    known) _autodrive_qa_list "${_AD_PATHS[@]}" ;;
    empty|nobase) printf '(none)\n' ;;
    *) printf '(unknown)\n' ;;
  esac
}

# autodrive_qa_docs_only lives in autodrive_qa_docs.sh beside this file, which
# is at its brick budget. Loaded from this file's physical directory only, only
# as a regular file and not a symlink; without it the predicate says `unknown`,
# which keeps gadugi required — and says so, since nothing else would (#2219).
_ad_docs="$(unset CDPATH; cd -- "$(dirname -- "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || _ad_docs=""
_ad_docs="${_ad_docs:+${_ad_docs}/autodrive_qa_docs.sh}"
# shellcheck source=autodrive_qa_docs.sh
{ [ -n "$_ad_docs" ] && [ -f "$_ad_docs" ] && [ ! -L "$_ad_docs" ] && . "$_ad_docs"; } \
  || { echo "WARNING: autodrive_qa_docs.sh did not load beside autodrive_qa_scope.sh; docs_only is 'unknown', so gadugi stays required for every pull request." >&2; autodrive_qa_docs_only() { printf 'unknown\n'; }; }
unset _ad_docs

# _autodrive_manifest_dirs <dir> -> a `<dir><TAB><cargo|python|node>` line per
# regular Cargo.toml, pyproject.toml, setup.py or package.json blob in the
# merge-base tree (HEAD's when it is unknown, so a manifest the pull request
# adds is never a candidate): the root's (`.`) first, then those 1 or 2
# levels down, sorted, under no directory named target, worktrees,
# node_modules or `.*`. A git failure (the `!` marker) or a newline in a path
# (0x01 after tr) is a `!` line.
_autodrive_manifest_dirs() {
  local -x LC_ALL=C
  local e="" p="" m=""
  local -a sub=()
  while IFS= read -r e; do
    if [ "$e" = "!" ]; then
      sub+=("!")
      continue
    fi
    case "$e" in 100644\ blob\ *|100755\ blob\ *) ;; *) continue ;; esac
    p="${e#*$'\t'}"
    case "$p" in *$'\001'*) sub+=("!"); continue ;; esac
    case "/$p" in */target/*|*/worktrees/*|*/node_modules/*|*/.*/*|/*/*/*/*) continue ;; esac
    case "${p##*/}" in
      Cargo.toml) m="cargo" ;;
      package.json) m="node" ;;
      *) m="python" ;;
    esac
    case "$p" in
      */*) sub+=("${p%/*}"$'\t'"$m") ;;
      *) printf '.\t%s\n' "$m" ;;
    esac
  done < <({ autodrive_trust_git "${1:-.}" ls-tree -r -z --full-tree --end-of-options "${_AD_MB:-HEAD}" 2>/dev/null \
             || printf '!\0'; } | tr '\000\n' '\n\001' \
             | grep -E '^!$|(^|/|	)(Cargo\.toml|pyproject\.toml|setup\.py|package\.json)$')
  [ "${#sub[@]}" -eq 0 ] || printf '%s\n' "${sub[@]}" | sort
}

# _autodrive_qa_in <candidate> <path>: `.`, or the path starts with `<candidate>/`.
_autodrive_qa_in() {
  [ "$1" = "." ] && return 0
  case "$2" in "$1"/*) return 0 ;; esac
  return 1
}

# _autodrive_qa_wd <working-directory>: set _AD_WD to the value with its `.`
# components removed (leading `./`, inner `/./`, a trailing `/.`), and `.` when
# that leaves nothing. GitHub Actions accepts `./engine` for `engine`; without
# this, _autodrive_qa_covers compares `./engine` against `engine/src/` and
# refuses a step that covers the change. Nothing else is rewritten, so `..`,
# `//` and a leading `/` still reach the `dir` rule and are refused there.
_autodrive_qa_wd() {
  local w="${1:-}"
  while [[ "$w" == ./* ]]; do w="${w#./}"; done
  while [[ "$w" == */./* ]]; do w="${w//\/.\//\/}"; done
  while [[ "$w" == */. ]]; do w="${w%/.}"; done
  [ -n "$w" ] || w="."
  _AD_WD="$w"
}

# _autodrive_qa_covers <dir> <wd>: return 0 when a command run in <wd> covers
# the change. For each changed path, take the deepest detection candidate
# that contains it (a path's directory decides, as in _autodrive_qa_choose);
# <wd> must contain the path and lie inside that candidate. Paths no
# candidate contains do not count. So a step in a/ never stands for a change
# in b/, and a root step never stands for a change in a nested project.
_autodrive_qa_covers() {
  local p="" cand="" deep="" dd=-1 x=0
  _autodrive_qa_choose "${1:-.}"
  for p in ${_AD_PATHS[@]+"${_AD_PATHS[@]}"}; do
    case "$p" in */*) p="${p%/*}/" ;; *) p="" ;; esac
    deep=""
    dd=-1
    for cand in ${_AD_CANDS[@]+"${_AD_CANDS[@]}"}; do
      { [ "$cand" != "!" ] && _autodrive_qa_in "$cand" "$p"; } || continue
      x=0
      [ "$cand" = "." ] || x=$(( ${#cand} + 1 ))
      [ "$x" -gt "$dd" ] && { deep="$cand"; dd="$x"; }
    done
    [ -n "$deep" ] || continue
    _autodrive_qa_in "$2" "$p" && _autodrive_qa_in "$deep" "$2/" || return 1
  done
  return 0
}

# _autodrive_qa_choose <dir>: the detection choice, made once per scan. Sets
# _AD_KIND (rust-cli, python, node, unknown or unresolved), _AD_DIR (the
# chosen directory), _AD_CANDS (the candidate directories after collapse,
# `!` first after a search error) and _AD_NODE (a candidate has a
# package.json).
#
# Each directory with a manifest is a candidate typed Cargo, then Python, then
# Node. A candidate is dropped when the nearest directory above it that has a
# manifest has one of the candidate's own ecosystem (Cargo, npm and Python
# workspaces). A root project file listed in AUTODRIVE_ROOT_PROJECT_FILES
# with no root manifest gives `unknown .`. With known changed paths, paths no
# candidate contains are set aside and the deepest candidate containing every
# other path wins. With no such path, or unknown paths, the one top-level
# candidate wins. A search error falls back to the root project, if any.
# Everything else, and a chosen directory outside the safe alphabet or with a
# `..` or empty component, is unresolved.
_autodrive_qa_choose() {
  _autodrive_qa_scan "${1:-.}"
  [ -z "$_AD_CHOSEN" ] || return 0
  _autodrive_qa_choose_now "${1:-.}"
  _AD_CHOSEN="1"
}

_autodrive_qa_choose_now() {
  local -x LC_ALL=C
  local d="${1:-.}" line="" dir="" eco="" err="" rootf="" i=0 j=0 k=0 a="" t="" best="" p="" h="" all=""
  local tab=$'\t' ad=-1 bd=-1 x=0
  local -a md=() me=() cdir=() ctyp=() rest=()
  _AD_KIND="unknown"
  _AD_DIR="."
  _AD_CANDS=()
  _AD_NODE=""
  # One entry per directory: md holds the directory, me its ecosystems.
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    if [ "$line" = "!" ]; then
      err="1"
      continue
    fi
    dir="${line%"$tab"*}"
    eco="${line##*"$tab"}"
    j=0
    while [ "$j" -lt "${#md[@]}" ] && [ "${md[$j]}" != "$dir" ]; do j=$((j + 1)); done
    if [ "$j" -lt "${#md[@]}" ]; then
      me[$j]="${me[$j]} ${eco}"
    else
      md+=("$dir")
      me+=("$eco")
    fi
  done <<EOF
$_AD_MANIFESTS
EOF
  if [ "${md[0]:-}" != "." ] && [ -n "${AUTODRIVE_ROOT_PROJECT_FILES[0]:+x}" ]; then
    rootf="$(autodrive_trust_git "$d" ls-tree -z --name-only --full-tree --end-of-options "${_AD_MB:-HEAD}" \
      -- "${AUTODRIVE_ROOT_PROJECT_FILES[@]}" 2>/dev/null | tr -d '\000')"
    [ -z "$rootf" ] || return 0
  fi
  # Same-ecosystem collapse against the nearest manifest directory above.
  # Depth (x) is 0 for `.`, else the length plus one: an ancestor is a
  # shorter prefix, so a larger number is a deeper directory.
  i=0
  while [ "$i" -lt "${#md[@]}" ]; do
    case " ${me[$i]} " in
      *" cargo "*) t="cargo" ;;
      *" python "*) t="python" ;;
      *) t="node" ;;
    esac
    a=""
    ad=-1
    j=0
    while [ "$j" -lt "${#md[@]}" ]; do
      if [ "$j" -ne "$i" ] && [ "${md[$i]}" != "." ] && _autodrive_qa_in "${md[$j]}" "${md[$i]}"; then
        x=0
        [ "${md[$j]}" = "." ] || x=$(( ${#md[$j]} + 1 ))
        if [ "$x" -gt "$ad" ]; then
          a="$j"
          ad="$x"
        fi
      fi
      j=$((j + 1))
    done
    case "${a:+ ${me[${a:-0}]} }" in
      *" $t "*) ;;
      *)
        cdir+=("${md[$i]}")
        ctyp+=("$t")
        case " ${me[$i]} " in *" node "*) _AD_NODE="1" ;; esac
        ;;
    esac
    i=$((i + 1))
  done
  [ -z "$err" ] || _AD_CANDS=("!")
  _AD_CANDS+=(${cdir[@]+"${cdir[@]}"})
  if [ "${#cdir[@]}" -eq 0 ]; then
    [ -z "$err" ] || _AD_KIND="unresolved"
    return 0
  fi
  if [ -n "$err" ]; then
    [ "${cdir[0]}" = "." ] && best="0"
  else
    if [ "$_AD_PATHS_STATE" = "known" ]; then
      # A path's directory decides containment; git sorts, so each run is tested once.
      h="/"
      for p in "${_AD_PATHS[@]}"; do
        case "$p" in */*) p="${p%/*}/" ;; *) p="" ;; esac
        [ "$p" != "$h" ] || continue
        h="$p"
        for dir in "${cdir[@]}"; do
          if _autodrive_qa_in "$dir" "$p"; then
            rest+=("$p")
            break
          fi
        done
      done
    fi
    i=0
    k=0
    while [ "$i" -lt "${#cdir[@]}" ]; do
      if [ "${#rest[@]}" -gt 0 ]; then
        # The deepest candidate that contains every remaining path.
        all="1"
        for p in "${rest[@]}"; do
          _autodrive_qa_in "${cdir[$i]}" "$p" || { all=""; break; }
        done
        x=0
        [ "${cdir[$i]}" = "." ] || x=$(( ${#cdir[$i]} + 1 ))
        if [ -n "$all" ] && [ "$x" -gt "$bd" ]; then
          best="$i"
          bd="$x"
        fi
      else
        # The one top-level candidate: no other candidate above it.
        a=""
        for dir in "${cdir[@]}"; do
          [ "$dir" != "${cdir[$i]}" ] && _autodrive_qa_in "$dir" "${cdir[$i]}" && a="1"
        done
        if [ -z "$a" ]; then
          k=$((k + 1))
          best="$i"
        fi
      fi
      i=$((i + 1))
    done
    [ "${#rest[@]}" -gt 0 ] || [ "$k" -eq 1 ] || best=""
  fi
  _AD_KIND="unresolved"
  [ -n "$best" ] || return 0
  dir="${cdir[$best]}"
  case "$dir" in .) ;; ''|/*|*//*|*/|..|../*|*/..|*/../*|*[!A-Za-z0-9._/-]*) return 0 ;; esac
  case "${ctyp[$best]}" in cargo) _AD_KIND="rust-cli" ;; *) _AD_KIND="${ctyp[$best]}" ;; esac
  _AD_DIR="$dir"
}

# autodrive_qa_target <dir> -> `<type><TAB><field>`, the one place the QA
# target is chosen: `rust-cli`, `python`, `node` or `unknown` with the
# relative directory, or `unresolved` with the candidate directories
# (limited as in _autodrive_qa_list). No manifest at all: `unknown .`.
autodrive_qa_target() {
  _autodrive_qa_choose "${1:-.}"
  if [ "$_AD_KIND" = "unresolved" ]; then
    printf 'unresolved\t%s\n' "$(_autodrive_qa_list ${_AD_CANDS[@]+"${_AD_CANDS[@]}"})"
  else
    printf '%s\t%s\n' "$_AD_KIND" "$_AD_DIR"
  fi
}

# _autodrive_qa_needs_gadugi <dir>: return 0 when the merge base is unknown
# (a preflight's is the head), when the choice is `node`, or when it is
# unresolved and any candidate directory has a package.json.
_autodrive_qa_needs_gadugi() {
  _autodrive_qa_choose "${1:-.}"
  [ -n "$_AD_MB" ] || return 0
  [ "$_AD_KIND" = "node" ] && return 0
  [ "$_AD_KIND" = "unresolved" ] && [ -n "$_AD_NODE" ]
}
