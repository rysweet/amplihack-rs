#!/usr/bin/env bash
# autodrive_qa_scope.sh — which QA suite command auto-drive runs, and where
# (issue #1763). autodrive_trust.sh loads this file from its own physical
# directory and from nowhere else: never $PWD, REPO_PATH or the pull
# request's tree. It is a gate file (GATE_FILES in merge_guard.rs).
#
# Public functions; each reads AUTODRIVE_QA_BASE (the base branch name) and
# AUTODRIVE_QA_HEAD (the head commit, default HEAD) from the environment:
#   autodrive_qa_changed_paths <dir>  the changed paths, `(unknown)` or `(none)`
#   autodrive_qa_target <dir>         `<type>\t<dir>` or `unresolved\t<list>`
#   autodrive_qa_config <dir>         .amplihack/qa.toml: none, ok or invalid
#   autodrive_qa_ci <dir>             the one clean CI test step, or none
# Contract: docs/reference/auto-drive-to-merge.md, "Choosing the QA command".
#
# This file only DEFINES functions and runs nothing it reads: no eval, no shell
# on built text, and awk never calls a command. git runs via autodrive_trust_git
# and never contacts a remote. qa.toml, the workflows and the manifests come
# from the merge-base tree (HEAD's if the base is unknown), never the working
# tree. Output is a fixed token, a path with bytes outside A-Z a-z 0-9 . _ / -
# as `_`, or a command in the command alphabet. LC_ALL=C; bash 3.2+, git, awk.

# _autodrive_qa_list <entry>... -> the entries joined by single spaces, each
# byte outside A-Z a-z 0-9 . _ / - as `_`, each cut to 128 bytes, at most 10
# and then `(+N more)`. The entry `!` is a search error: `(search-error)`.
_autodrive_qa_list() {
  local -x LC_ALL=C
  local n=0 out="" e=""
  for e in "$@"; do
    n=$((n + 1)); [ "$n" -le 10 ] || continue
    if [ "$e" = "!" ]; then e="(search-error)"; else e="${e//[!A-Za-z0-9._\/-]/_}"; e="${e:0:128}"; fi
    out="${out}${out:+ }${e}"
  done
  [ "$n" -le 10 ] || out="${out} (+$((n - 10)) more)"
  printf '%s\n' "$out"
}

# _autodrive_qa_changed_raw <dir>: sets _AD_PATHS_STATE (known, none, unknown),
# _AD_PATHS and _AD_MB (merge base or empty). Head: AUTODRIVE_QA_HEAD (the gate's
# headRefOid; a full SHA in the clone, else unknown) or HEAD. No AUTODRIVE_QA_BASE
# (a preflight): the head is the merge base. Else the base passes the name rules
# and refs/remotes/origin/<base> exists. Paths: `git diff --name-only
# --no-renames -z <merge-base> <head>`; a git failure or newline is unknown.
_autodrive_qa_changed_raw() {
  local -x LC_ALL=C
  local d="${1:-.}" b="${AUTODRIVE_QA_BASE:-}" h="${AUTODRIVE_QA_HEAD-HEAD}" base="" head="" mb="" p=""
  _AD_PATHS=(); _AD_PATHS_STATE="unknown"; _AD_MB=""
  [ "$h" = "HEAD" ] || autodrive_is_sha "$h" || return 0
  head="$(autodrive_trust_git "$d" rev-parse --verify --quiet "${h}^{commit}" 2>/dev/null)" || return 0
  autodrive_is_sha "$head" || return 0
  [ -n "${AUTODRIVE_QA_BASE+x}" ] || { _AD_MB="$head"; _AD_PATHS_STATE="none"; return 0; }
  case "$b" in ''|-*|*[!ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/-]*) return 0 ;; esac
  autodrive_trust_git "$d" check-ref-format --branch "$b" >/dev/null 2>&1 || return 0
  base="$(autodrive_trust_git "$d" rev-parse --verify --quiet "refs/remotes/origin/${b}^{commit}" 2>/dev/null)" || return 0
  autodrive_is_sha "$base" || return 0
  mb="$(autodrive_trust_git "$d" merge-base --end-of-options "$base" "$head" 2>/dev/null)" || return 0
  autodrive_is_sha "$mb" || return 0
  _AD_MB="$mb"
  while IFS= read -r -d '' p; do
    case "$p" in *$'\n'*) _AD_PATHS=(); return 0 ;; esac
    _AD_PATHS+=("$p")
  done < <(autodrive_trust_git "$d" diff --name-only --no-renames --no-ext-diff --no-textconv -z \
             --end-of-options "$mb" "$head" 2>/dev/null || printf '\n\0')
  if [ "${#_AD_PATHS[@]}" -eq 0 ]; then _AD_PATHS_STATE="none"; else _AD_PATHS_STATE="known"; fi
}

# autodrive_qa_changed_paths <dir> -> the changed paths (as _autodrive_qa_list),
# `(none)` for an empty diff, or `(unknown)` when they cannot be read.
autodrive_qa_changed_paths() {
  _autodrive_qa_changed_raw "${1:-.}"
  case "$_AD_PATHS_STATE" in
    known) _autodrive_qa_list "${_AD_PATHS[@]}" ;;
    none) printf '(none)\n' ;;
    *) printf '(unknown)\n' ;;
  esac
}

# _autodrive_qa_declared_changed <all|qa>: return 0 when _AD_PATHS holds
# qa.toml or (with `all`) a workflow file, or their directories: the pull
# request choosing its suite.
_autodrive_qa_declared_changed() {
  local p=""
  for p in ${_AD_PATHS[@]+"${_AD_PATHS[@]}"}; do
    case "$1:$p" in *:.amplihack|*:.amplihack/qa.toml|all:.github|all:.github/workflows|all:.github/workflows/*) return 0 ;; esac
  done
  return 1
}

# _autodrive_qa_entry <dir> <tree> <path> -> `<mode> <type> <oid> <size>` of
# <path> in <tree>, or nothing; 1 when git fails or lists anything else.
_autodrive_qa_entry() {
  local e="" out="" n=0
  while IFS= read -r -d '' e; do
    n=$((n + 1)); { [ "$n" -eq 1 ] && [ "${e#*$'\t'}" = "$3" ]; } || return 1
    out="${e%%$'\t'*}"
  done < <(autodrive_trust_git "$1" ls-tree -z --long --full-tree --end-of-options "$2" -- "$3" 2>/dev/null || printf '!\0')
  [ -z "$out" ] || printf '%s\n' "$out"
}

# _autodrive_manifest_dirs <dir> -> a `<dir><TAB><cargo|python|node>` line per
# regular Cargo.toml, pyproject.toml, setup.py or package.json blob in the
# merge-base tree (HEAD's if unknown; one the pull request adds is never a
# candidate): the root's (`.`) first, then those 1 or 2 levels down, sorted,
# under no directory named target, worktrees, node_modules or `.*`. A git
# failure (the `!` marker) or a newline in a path (0x01 after tr) is `!`.
_autodrive_manifest_dirs() {
  local -x LC_ALL=C
  local e="" p="" m="" sub=()
  while IFS= read -r e; do
    [ "$e" != "!" ] || { sub+=("!"); continue; }
    case "$e" in 100644\ blob\ *|100755\ blob\ *) ;; *) continue ;; esac
    p="${e#*$'\t'}"; case "$p" in *$'\001'*) sub+=("!"); continue ;; esac
    case "/$p" in */target/*|*/worktrees/*|*/node_modules/*|*/.*/*|/*/*/*/*) continue ;; esac
    case "${p##*/}" in Cargo.toml) m="cargo" ;; package.json) m="node" ;; *) m="python" ;; esac
    case "$p" in */*) sub+=("${p%/*}"$'\t'"$m") ;; *) printf '.\t%s\n' "$m" ;; esac
  done < <({ autodrive_trust_git "${1:-.}" ls-tree -r -z --full-tree --end-of-options "${_AD_MB:-HEAD}" 2>/dev/null \
             || printf '!\0'; } | tr '\000\n' '\n\001' | grep -E '^!$|(^|/|	)(Cargo\.toml|pyproject\.toml|setup\.py|package\.json)$')
  [ "${#sub[@]}" -eq 0 ] || printf '%s\n' "${sub[@]}" | sort
}

# _autodrive_qa_in <candidate> <path>: `.`, or the path starts with `<candidate>/`.
_autodrive_qa_in() {
  [ "$1" = "." ] && return 0
  case "$2" in "$1"/*) return 0 ;; esac
  return 1
}

# _autodrive_qa_choose <dir>: the detection choice. Sets _AD_KIND (rust-cli,
# python, node, unknown or unresolved), _AD_DIR (the chosen directory),
# _AD_CANDS (the raw candidate directories after collapse, `!` first after a
# search error), _AD_NODE (a candidate has a package.json) and _AD_MB.
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
  local -x LC_ALL=C
  local d="${1:-.}" line="" dir="" eco="" err="" i=0 j=0 k=0 a="" t="" best="" p="" h="" tab=$'\t' all=""
  local ad=-1 bd=-1 x=0
  local -a md=() me=() cdir=() ctyp=() rest=()
  _AD_KIND="unknown"; _AD_DIR="."; _AD_CANDS=(); _AD_NODE=""
  _autodrive_qa_changed_raw "$d"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    if [ "$line" = "!" ]; then err="1"; continue; fi
    dir="${line%"$tab"*}"; eco="${line##*"$tab"}"
    j=0; while [ "$j" -lt "${#md[@]}" ] && [ "${md[$j]}" != "$dir" ]; do j=$((j + 1)); done
    if [ "$j" -lt "${#md[@]}" ]; then me[$j]="${me[$j]} ${eco}"; else md+=("$dir"); me+=("$eco"); fi
  done <<EOF
$(_autodrive_manifest_dirs "$d")
EOF
  if [ "${md[0]:-}" != "." ] && [ -n "${AUTODRIVE_ROOT_PROJECT_FILES[0]:+x}" ] && [ -n "$(autodrive_trust_git "$d" ls-tree -z \
       --name-only --full-tree --end-of-options "${_AD_MB:-HEAD}" -- "${AUTODRIVE_ROOT_PROJECT_FILES[@]}" 2>/dev/null | tr -d '\000')" ]; then
    return 0
  fi
  # Same-ecosystem collapse against the nearest manifest directory above. Depth
  # is 0 for `.`, else the length plus one (an ancestor is a shorter prefix).
  i=0
  while [ "$i" -lt "${#md[@]}" ]; do
    case " ${me[$i]} " in *" cargo "*) t="cargo" ;; *" python "*) t="python" ;; *) t="node" ;; esac
    a=""; ad=-1; j=0
    while [ "$j" -lt "${#md[@]}" ]; do
      if [ "$j" -ne "$i" ] && [ "${md[$i]}" != "." ] && _autodrive_qa_in "${md[$j]}" "${md[$i]}"; then
        x=0; [ "${md[$j]}" = "." ] || x=$(( ${#md[$j]} + 1 ))
        [ "$x" -gt "$ad" ] && { a="$j"; ad="$x"; }
      fi
      j=$((j + 1))
    done
    case "${a:+ ${me[${a:-0}]} }" in *" $t "*) ;; *) cdir+=("${md[$i]}"); ctyp+=("$t")
      case " ${me[$i]} " in *" node "*) _AD_NODE="1" ;; esac ;; esac
    i=$((i + 1))
  done
  [ -z "$err" ] || _AD_CANDS=("!")
  _AD_CANDS+=(${cdir[@]+"${cdir[@]}"})
  if [ "${#cdir[@]}" -eq 0 ]; then [ -z "$err" ] || _AD_KIND="unresolved"; return 0; fi
  if [ -n "$err" ]; then
    [ "${cdir[0]}" = "." ] && best="0"
  else
    if [ "$_AD_PATHS_STATE" = "known" ]; then
      # A path's directory decides containment; git sorts, so each run is tested once.
      h="/"; for p in "${_AD_PATHS[@]}"; do
        case "$p" in */*) p="${p%/*}/" ;; *) p="" ;; esac; [ "$p" != "$h" ] || continue; h="$p"
        for dir in "${cdir[@]}"; do _autodrive_qa_in "$dir" "$p" && { rest+=("$p"); break; }; done
      done
    fi
    i=0; k=0
    while [ "$i" -lt "${#cdir[@]}" ]; do
      if [ "${#rest[@]}" -gt 0 ]; then
        # The deepest candidate that contains every remaining path.
        all="1"
        for p in "${rest[@]}"; do _autodrive_qa_in "${cdir[$i]}" "$p" || { all=""; break; }; done
        x=0; [ "${cdir[$i]}" = "." ] || x=$(( ${#cdir[$i]} + 1 ))
        [ -n "$all" ] && [ "$x" -gt "$bd" ] && { best="$i"; bd="$x"; }
      else
        # The one top-level candidate: no other candidate above it.
        a=""; for dir in "${cdir[@]}"; do [ "$dir" != "${cdir[$i]}" ] && _autodrive_qa_in "$dir" "${cdir[$i]}" && a="1"; done
        [ -n "$a" ] || { k=$((k + 1)); best="$i"; }
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

# _autodrive_qa_dir_check <root> <dir>: return 0 for a safe, existing `dir`
# whose physical path lies inside <root>; 1 when it breaks the name rule
# (relative, no leading `/` or `-`, no `..` or empty component, only
# A-Z a-z 0-9 . _ / -); 2 when it is missing or leaves the repository.
_autodrive_qa_dir_check() {
  local r="${1:-.}" dir="${2:-}" rp="" phys=""
  case "$dir" in ''|/*|-*|*//*|*/|..|../*|*/..|*/../*|*[!A-Za-z0-9._/-]*) return 1 ;; esac
  [ -d "$r/$dir" ] || return 2
  rp="$(unset CDPATH; cd -- "$r" 2>/dev/null && pwd -P)" || return 2
  phys="$(unset CDPATH; cd -- "$r/$dir" 2>/dev/null && pwd -P)" || return 2
  [ -n "$rp" ] && [ -n "$phys" ] || return 2
  [ "$phys" = "$rp" ] && return 0
  case "$phys/" in "${rp%/}/"*) return 0 ;; esac
  return 2
}

# autodrive_qa_config <dir> -> `none` when .amplihack/qa.toml is not present in
# the merge-base tree (HEAD's if unknown) and the pull request leaves it alone,
# `ok<TAB><dir><TAB><command>`, or `invalid<TAB><problem>`: symlink not-regular
# too-large syntax unknown-key duplicate-key missing-command unsafe-command
# unsafe-dir dir-not-found changed-in-pr paths-unknown. Present: .amplihack is
# not a tree, or qa.toml has an entry. Only the `dir` is checked on disk. The
# blob is read once, at most 4097 bytes (NUL as 0x01, which the byte check
# rejects); no content is printed except a valid command and dir.
autodrive_qa_config() {
  local -x LC_ALL=C
  local r="${1:-.}" a="" q="" oid="" size="" body="" line="" key="" val="" cmd="" dir="." seen="" re="" okb=""
  _autodrive_qa_changed_raw "$r"
  a="$(_autodrive_qa_entry "$r" "${_AD_MB:-HEAD}" .amplihack)" && q="$(_autodrive_qa_entry "$r" "${_AD_MB:-HEAD}" .amplihack/qa.toml)" \
    || { if [ -n "$_AD_MB" ]; then printf 'invalid\tpaths-unknown\n'; else printf 'none\n'; fi; return 0; }
  case "$a" in '') false ;; 040000\ *) [ -n "$q" ] ;; esac || _autodrive_qa_declared_changed qa \
    || { printf 'none\n'; return 0; }
  if [ "$_AD_PATHS_STATE" != "known" ]; then printf 'invalid\tpaths-unknown\n'; return 0; fi
  if _autodrive_qa_declared_changed qa; then printf 'invalid\tchanged-in-pr\n'; return 0; fi
  case "$a" in 120000\ *) printf 'invalid\tsymlink\n'; return 0 ;; 040000\ tree\ *) ;; *) printf 'invalid\tnot-regular\n'; return 0 ;; esac
  case "$q" in 120000\ *) printf 'invalid\tsymlink\n'; return 0 ;; 100644\ blob\ *|100755\ blob\ *) ;; *) printf 'invalid\tnot-regular\n'; return 0 ;; esac
  read -r _ _ oid size <<< "$q"
  [ "$size" -le 4096 ] 2>/dev/null || { printf 'invalid\ttoo-large\n'; return 0; }
  body="$(autodrive_trust_git "$r" cat-file blob "$oid" 2>/dev/null | head -c 4097 | tr '\000' '\001'; printf 'x')"; body="${body%x}"
  if [ "${#body}" -ne "$size" ]; then printf 'invalid\tnot-regular\n'; return 0; fi
  okb=$'\t\n'' -~'
  # shellcheck disable=SC2254 # okb is a bracket range on purpose
  case "$body" in *[!$okb]*) printf 'invalid\tsyntax\n'; return 0 ;; esac
  re='^([A-Za-z0-9_-]+)[ 	]*=[ 	]*"([^"\]*)"[ 	]*$'
  while IFS= read -r line; do
    line="${line#"${line%%[! 	]*}"}"
    case "$line" in ''|'#'*) continue ;; esac
    if ! [[ "$line" =~ $re ]]; then printf 'invalid\tsyntax\n'; return 0; fi
    key="${BASH_REMATCH[1]}"; val="${BASH_REMATCH[2]}"
    case "$key" in command|dir) ;; *) printf 'invalid\tunknown-key\n'; return 0 ;; esac
    case " $seen " in *" $key "*) printf 'invalid\tduplicate-key\n'; return 0 ;; esac
    seen="${seen} ${key}"
    if [ "$key" = "command" ]; then cmd="$val"; else dir="$val"; fi
  done <<EOF
$body
EOF
  case "$cmd" in *[!\ $'\t']*) ;; *) printf 'invalid\tmissing-command\n'; return 0 ;; esac
  case "$cmd" in *[!A-Za-z0-9\ ._/=+:,@%-]*) printf 'invalid\tunsafe-command\n'; return 0 ;; esac
  _autodrive_qa_dir_check "$r" "$dir"
  case "$?" in 0) ;; 1) printf 'invalid\tunsafe-dir\n'; return 0 ;; *) printf 'invalid\tdir-not-found\n'; return 0 ;; esac
  printf 'ok\t%s\t%s\n' "$dir" "$cmd"
}

# autodrive_qa_ci <dir> -> `ok<TAB><dir><TAB><command>` for exactly one clean
# test step in .github/workflows/*.yml and *.yaml of the merge-base tree, else
# `none` (advisory: every failure is `none`, with nothing printed). Skipped
# when the paths are unknown, the pull request changes a workflow, .github or
# .github/workflows is not a tree, there are over 50 files (counted before
# any is read), or an entry is not a regular blob, has a name outside
# A-Z a-z 0-9 . _ - or is over 256 KiB. A `run:` value matches when it starts
# with cargo test, cargo nextest run, npm test, npm run test, pytest, or
# python(3) -m pytest; it is clean only on one line with no `${{`, quote,
# backtick, `$`, `\`, `&`, `;`, `|`, `<`, `>` or leading NAME=value. A block
# scalar holding an invocation, a plain value continued on the next line and
# `defaults: run: working-directory` are not clean. The step's own
# working-directory, if any, must pass the qa.toml `dir` rule and exist.
autodrive_qa_ci() {
  local -x LC_ALL=C
  local r="${1:-.}" e="" f="" mode="" typ="" oid="" size="" b="" res="" line="" wd="" cmd="" c=0 bad="" prog="" ents=() oids=()
  _autodrive_qa_changed_raw "$r"
  if [ "$_AD_PATHS_STATE" != "known" ] || _autodrive_qa_declared_changed all; then printf 'none\n'; return 0; fi
  case "$(_autodrive_qa_entry "$r" "$_AD_MB" .github)|$(_autodrive_qa_entry "$r" "$_AD_MB" .github/workflows)" in
    040000\ tree\ *\|040000\ tree\ *) ;; *) printf 'none\n'; return 0 ;; esac
  while IFS= read -r -d '' e; do
    case "${e#*$'\t'}" in .github/workflows/.*) ;; '!'|.github/workflows/*.yml|.github/workflows/*.yaml) ents+=("$e") ;; esac
  done < <(autodrive_trust_git "$r" ls-tree -z --long --full-tree --end-of-options "$_AD_MB" -- .github/workflows/ 2>/dev/null || printf '!\0')
  if [ "${#ents[@]}" -eq 0 ] || [ "${#ents[@]}" -gt 50 ]; then printf 'none\n'; return 0; fi
  for e in "${ents[@]}"; do
    f="${e#*$'\t'.github/workflows/}"; read -r mode typ oid size <<< "${e%%$'\t'*}"
    case "$f" in ''|*[!A-Za-z0-9._-]*) printf 'none\n'; return 0 ;; esac
    case "$mode:$typ" in 100644:blob|100755:blob) ;; *) printf 'none\n'; return 0 ;; esac
    { [ "$size" -le 262144 ] 2>/dev/null && autodrive_is_sha "$oid"; } || { printf 'none\n'; return 0; }
    oids+=("$oid")
  done
  # The parser prints `D` for a match that is not clean and
  # `C<TAB><working-directory><TAB><command>` for each clean one.
  prog='
    function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
    function unq(s) { s = trim(s); sub(/[ \t]+#.*$/, "", s)
      if (s ~ /^".*"$/ || s ~ /^\047.*\047$/) s = substr(s, 2, length(s) - 2); return s }
    function inv(s) { return s ~ /^(cargo test|cargo nextest run|npm test|npm run test|pytest|python3? -m pytest)([ \t]|$)/ }
    function has(s) { return s ~ /(cargo test|cargo nextest run|npm test|npm run test|pytest)/ }
    FNR == 1 { sp = 0; blk = -1; pend = -1; split("", at) }
    {
      sub(/\r$/, ""); line = $0
      match(line, /^ */); ind = RLENGTH
      if (blk >= 0) {
        if (line ~ /^[ \t]*$/ || ind > blk) { if (brun && has(line)) dirty = 1; next }
        blk = -1
      }
      if (line ~ /^[ \t]*(#|$)/ || line ~ /^(---|\.\.\.)/) next
      if (pend >= 0 && ind > pend) dirty = 1
      pend = -1
      rest = substr(line, ind + 1)
      while (match(rest, /^-( +|$)/)) {
        ind += RLENGTH; rest = substr(rest, RLENGTH + 1); nit++; at[ind] = nit
      }
      if (!match(rest, /^[A-Za-z0-9_.-]+:/)) next
      key = substr(rest, 1, RLENGTH - 1); val = substr(rest, RLENGTH + 1)
      if (val != "" && val !~ /^[ \t]/) next
      val = trim(val); if (val ~ /^#/) val = ""
      while (sp > 0 && si[sp] >= ind) sp--
      parent = (sp > 0) ? sk[sp] : ""
      sp++; sk[sp] = key; si[sp] = ind
      id = (ind in at) ? at[ind] : ("top" NR)
      if (key == "working-directory") { if (parent == "run") dirty = 1; else wdv[id] = unq(val); next }
      if (val ~ /^[|>][-+0-9]*([ \t]+#.*)?$/) { blk = ind; brun = (key == "run"); next }
      if (key != "run" || val == "") next
      v = val; sub(/[ \t]+#.*$/, "", v); q = 0; e = 0
      if (v ~ /^["\047]/) { q = 1; v = substr(v, 2) }
      while (match(v, /^[A-Za-z_][A-Za-z0-9_]*=[^ \t]*[ \t]+/)) { v = substr(v, RLENGTH + 1); e = 1 }
      if (!inv(v)) next
      pend = ind
      if (q || e || val ~ /[$`"\047\\&;|<>]/) { dirty = 1; next }
      if (!(id in runv)) { nr++; order[nr] = id }
      else dirty = 1
      runv[id] = trim(v)
    }
    END {
      if (dirty) print "D"
      for (i = 1; i <= nr; i++) print "C\t" wdv[order[i]] "\t" runv[order[i]]
    }'
  # One awk run per blob: FNR == 1 already reset all per-file state.
  for oid in "${oids[@]}"; do
    { b="$(autodrive_trust_git "$r" cat-file blob "$oid")"; } 2>/dev/null || { printf 'none\n'; return 0; }
    res="${res}$(printf '%s\n' "$b" | awk "$prog" 2>/dev/null)"$'\n' || { printf 'none\n'; return 0; }
  done
  while IFS= read -r line; do
    case "$line" in
      D) bad="1" ;;
      C"	"*) c=$((c + 1)); line="${line#C	}"; wd="${line%%	*}"; cmd="${line#*	}" ;;
    esac
  done <<EOF
$res
EOF
  if [ -n "$bad" ] || [ "$c" -ne 1 ]; then printf 'none\n'; return 0; fi
  case "$cmd" in ''|*[!A-Za-z0-9\ ._/=+:,@%-]*) printf 'none\n'; return 0 ;; esac
  [ -n "$wd" ] || wd="."
  _autodrive_qa_dir_check "$r" "$wd" || { printf 'none\n'; return 0; }
  printf 'ok\t%s\t%s\n' "$wd" "$cmd"
}
