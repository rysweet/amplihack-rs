#!/usr/bin/env bash
# autodrive_qa_sources.sh — the declared QA command sources (issue #1763):
# .amplihack/qa.toml and one clean CI test step. autodrive_trust.sh loads this
# file from its own physical directory after autodrive_qa_scope.sh, which gives
# the changed paths and merge base read here (_autodrive_qa_scan, _AD_PATHS,
# _AD_PATHS_STATE, _AD_MB). Public functions:
#   autodrive_qa_config <dir>       .amplihack/qa.toml: none, ok or invalid
#   autodrive_qa_declaration <dir>  how criterion 1 is met (#2458)
#   autodrive_qa_ci <dir>           the one clean CI test step, or none and a reason
#   autodrive_qa_report <dir>       every answer the evidence tool needs, at once
# Contract: docs/reference/auto-drive-to-merge.md, "Choosing the QA command".
# Only DEFINES functions and runs nothing it reads: no eval, no shell on built
# text, awk never calls a command. git runs via autodrive_trust_git, never a
# remote. qa.toml and the workflows come from the merge-base tree (HEAD's when
# no base is given), never the working tree. Output is a fixed token or a value
# in its checked alphabet. LC_ALL=C; bash 3.2+, git, awk, head and tr.

# _autodrive_qa_toml_changed: return 0 when the changed paths hold
# .amplihack/qa.toml or .amplihack itself: the pull request choosing its suite.
_autodrive_qa_toml_changed() {
  local p=""
  for p in ${_AD_PATHS[@]+"${_AD_PATHS[@]}"}; do
    case "$p" in .amplihack|.amplihack/qa.toml) return 0 ;; esac
  done
  return 1
}

# _autodrive_qa_entry <dir> <tree> <path> -> `<mode> <type> <oid> <size>` of
# <path> in <tree>, or nothing; 1 when git fails or lists anything else.
_autodrive_qa_entry() {
  local e="" out="" n=0
  while IFS= read -r -d '' e; do
    n=$((n + 1))
    { [ "$n" -eq 1 ] && [ "${e#*$'\t'}" = "$3" ]; } || return 1
    out="${e%%$'\t'*}"
  done < <(autodrive_trust_git "$1" ls-tree -z --long --full-tree --end-of-options "$2" -- "$3" 2>/dev/null \
             || printf '!\0')
  [ -z "$out" ] || printf '%s\n' "$out"
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

# _autodrive_qa_job_ok <name>: a scenarios_ci_job value: 1 to 128 bytes of
# A-Za-z0-9 space . _ / ( ) + : , @ % -, not starting with `-` (#2458).
_autodrive_qa_job_ok() {
  local -x LC_ALL=C
  [ "${#1}" -ge 1 ] && [ "${#1}" -le 128 ] || return 1
  case "$1" in -*|*[!A-Za-z0-9\ ._/\(\)+:,@%-]*) return 1 ;; esac
}

# autodrive_qa_config <dir> [decl-only] -> `none` when .amplihack/qa.toml is
# not present in the merge-base tree (HEAD's if unknown) and the pull request
# leaves it alone, `ok<TAB><dir><TAB><command>[<TAB><declaration>]`, or
# `invalid<TAB><problem>`: symlink not-regular too-large syntax unknown-key
# duplicate-key declaration-conflict unknown-value missing-command
# unsafe-command unsafe-dir dir-not-found changed-in-pr paths-empty
# paths-unknown. Keys: command, dir, multi_area ("command" gives
# multi-area-command) and scenarios_ci_job (_autodrive_qa_job_ok, ci-job:<name>).
# decl-only (no base: a preflight) accepts the head tree and prints
# `decl<TAB><declaration or none>` in place of ok. Only `dir` is checked on
# disk. The blob is read once, at most 4097 bytes (NUL as 0x01, rejected); no
# content is printed but checked values.
autodrive_qa_config() {
  local -x LC_ALL=C
  local r="${1:-.}" a="" q="" present="" oid="" size="" body="" line="" key="" val="" cmd="" dir="." seen=""
  local re="" okb="" decl="" job=""
  _autodrive_qa_scan "$r"
  if ! a="$(_autodrive_qa_entry "$r" "${_AD_MB:-HEAD}" .amplihack)" \
     || ! q="$(_autodrive_qa_entry "$r" "${_AD_MB:-HEAD}" .amplihack/qa.toml)"; then
    if [ -n "$_AD_MB" ]; then printf 'invalid\tpaths-unknown\n'; else printf 'none\n'; fi
    return 0
  fi
  case "$a" in
    '') present="" ;;
    040000\ *) present="${q:+1}" ;;
    *) present="1" ;;
  esac
  if [ -z "$present" ] && ! _autodrive_qa_toml_changed; then
    printf 'none\n'
    return 0
  fi
  # The file is used only when the diff shows the pull request left it alone.
  case "$_AD_PATHS_STATE:${2:-}" in
    known:*|nobase:decl-only) ;;
    empty:*) printf 'invalid\tpaths-empty\n'; return 0 ;;
    *) printf 'invalid\tpaths-unknown\n'; return 0 ;;
  esac
  if _autodrive_qa_toml_changed; then printf 'invalid\tchanged-in-pr\n'; return 0; fi
  case "$a" in
    120000\ *) printf 'invalid\tsymlink\n'; return 0 ;;
    040000\ tree\ *) ;;
    *) printf 'invalid\tnot-regular\n'; return 0 ;;
  esac
  case "$q" in
    120000\ *) printf 'invalid\tsymlink\n'; return 0 ;;
    100644\ blob\ *|100755\ blob\ *) ;;
    *) printf 'invalid\tnot-regular\n'; return 0 ;;
  esac
  read -r _ _ oid size <<< "$q"
  [ "$size" -le 4096 ] 2>/dev/null || { printf 'invalid\ttoo-large\n'; return 0; }
  body="$(autodrive_trust_git "$r" cat-file blob "$oid" 2>/dev/null | head -c 4097 | tr '\000' '\001'; printf 'x')"
  body="${body%x}"
  if [ "${#body}" -ne "$size" ]; then printf 'invalid\tnot-regular\n'; return 0; fi
  okb=$'\t\n'' -~'
  # shellcheck disable=SC2254 # okb is a bracket range on purpose
  case "$body" in *[!$okb]*) printf 'invalid\tsyntax\n'; return 0 ;; esac
  re='^([A-Za-z0-9_-]+)[ 	]*=[ 	]*"([^"\]*)"[ 	]*$'
  while IFS= read -r line; do
    line="${line#"${line%%[! 	]*}"}"
    case "$line" in ''|'#'*) continue ;; esac
    if ! [[ "$line" =~ $re ]]; then printf 'invalid\tsyntax\n'; return 0; fi
    key="${BASH_REMATCH[1]}"
    val="${BASH_REMATCH[2]}"
    case "$key" in command|dir|multi_area|scenarios_ci_job) ;; *) printf 'invalid\tunknown-key\n'; return 0 ;; esac
    case " $seen " in *" $key "*) printf 'invalid\tduplicate-key\n'; return 0 ;; esac
    seen="${seen} ${key}"
    case "$key" in command) cmd="$val" ;; dir) dir="$val" ;; multi_area) decl="$val" ;; *) job="$val" ;; esac
  done <<EOF
$body
EOF
  case "$seen " in
    *" multi_area "*scenarios_ci_job*|*" scenarios_ci_job "*multi_area*)
      printf 'invalid\tdeclaration-conflict\n'; return 0 ;;
    *" multi_area "*) [ "$decl" = "command" ] || { printf 'invalid\tunknown-value\n'; return 0; }
      decl="multi-area-command" ;;
    *" scenarios_ci_job "*) _autodrive_qa_job_ok "$job" || { printf 'invalid\tunknown-value\n'; return 0; }
      decl="ci-job:$job" ;;
  esac
  case "$cmd" in *[!\ $'\t']*) ;; *) printf 'invalid\tmissing-command\n'; return 0 ;; esac
  case "$cmd" in *[!A-Za-z0-9\ ._/=+:,@%-]*) printf 'invalid\tunsafe-command\n'; return 0 ;; esac
  _autodrive_qa_dir_check "$r" "$dir"
  case "$?" in
    0) ;;
    1) printf 'invalid\tunsafe-dir\n'; return 0 ;;
    *) printf 'invalid\tdir-not-found\n'; return 0 ;;
  esac
  if [ "${2:-}" = "decl-only" ]; then printf 'decl\t%s\n' "${decl:-none}"; return 0; fi
  printf 'ok\t%s\t%s%s\n' "$dir" "$cmd" "${decl:+$'\t'$decl}"
}

# autodrive_qa_declaration <dir> -> none, multi-area-command or ci-job:<name>,
# always exit 0 (#2458). Only the merge-base qa.toml counts (#2439); with no
# AUTODRIVE_QA_BASE at all (a preflight) the head tree's, via decl-only. `none`
# when AUTODRIVE_QA_SCENARIO_DIR, AUTODRIVE_QA_COMMAND or AUTODRIVE_QA_COMMANDS
# is set, qa.toml is absent or invalid, git fails, or the value fails its check
# again; ci-job also when the pull request changes a workflow (S3).
autodrive_qa_declaration() {
  local -x LC_ALL=C
  local r="${1:-.}" cfg="" d="" mode=""
  if [ -n "${AUTODRIVE_QA_SCENARIO_DIR+x}${AUTODRIVE_QA_COMMAND+x}${AUTODRIVE_QA_COMMANDS+x}" ]; then
    printf 'none\n'; return 0
  fi
  [ -n "${AUTODRIVE_QA_BASE+x}" ] || mode="decl-only"
  _autodrive_qa_scan "$r"
  cfg="$(autodrive_qa_config "$r" $mode)" || cfg=""
  case "$cfg" in
    decl$'\t'*) d="${cfg#decl$'\t'}" ;;
    ok$'\t'*$'\t'*$'\t'*) d="${cfg##*$'\t'}" ;;
  esac
  case "$d" in
    multi-area-command) ;;
    ci-job:*) { _autodrive_qa_job_ok "${d#ci-job:}" && ! _autodrive_qa_workflow_changed; } || d="" ;;
    *) d="" ;;
  esac
  printf '%s\n' "${d:-none}"
}

# autodrive_qa_lifting_declaration <dir> -> the declaration only where it lifts the gadugi
# need (D8): not docs-only, detection needs gadugi (autodrive_gadugi_required's order). Else none.
autodrive_qa_lifting_declaration() {
  if { ! declare -F autodrive_qa_docs_only >/dev/null || [ "$(autodrive_qa_docs_only "${1:-.}")" != "true" ]; } \
     && declare -F _autodrive_qa_needs_gadugi >/dev/null && _autodrive_qa_needs_gadugi "${1:-.}"
  then autodrive_qa_declaration "${1:-.}"; else printf 'none\n'; fi
}

# _autodrive_qa_workflow_changed: return 0 when the changed paths hold .github or
# any file below it (workflows and local actions alike), or the qa.toml paths.
_autodrive_qa_workflow_changed() {
  local p=""
  for p in ${_AD_PATHS[@]+"${_AD_PATHS[@]}"}; do
    case "$p" in .github|.github/*) return 0 ;; esac
  done
  _autodrive_qa_toml_changed
}

# autodrive_qa_ci <dir> -> `ok<TAB><dir><TAB><command>` for exactly one clean
# test step in .github/workflows/*.yml and *.yaml of the merge-base tree, else
# `none<TAB><reason>`. The source is advisory: `none` falls through to
# detection, and the evidence records the reason as qa_ci_skip. Reasons, in
# check order (the docs' table says what each means): paths-empty
# paths-unknown changed-in-pr no-workflows list-failed too-many-files (over 50)
# unsafe-name not-regular too-large (over 256 KiB) read-failed not-clean
# no-match several-matches unsafe-command unsafe-dir dir-not-found out-of-scope.
# A `run:` value matches when it starts with cargo test, cargo nextest run,
# npm test, npm run test, pytest, or python(3) -m pytest. It is clean only on
# one line with no `${{`, quote, backtick, `$`, `\`, `&`, `;`, `|`, `<`, `>`
# or leading NAME=value. A block scalar holding an invocation, a plain value
# continued on the next line and `defaults: run: working-directory` are not
# clean. The step's own working-directory, `.` components removed, must pass
# the qa.toml `dir` rule, exist and cover the change (_autodrive_qa_wd).
autodrive_qa_ci() {
  local -x LC_ALL=C
  local r="${1:-.}" e="" f="" mode="" typ="" oid="" size="" b="" res="" line="" wd="" cmd="" c=0 bad=""
  local prog="" gh="" wf=""
  local -a ents=() oids=()
  _autodrive_qa_scan "$r"
  case "$_AD_PATHS_STATE" in
    known) ;;
    empty) printf 'none\tpaths-empty\n'; return 0 ;;
    *) printf 'none\tpaths-unknown\n'; return 0 ;;
  esac
  if _autodrive_qa_workflow_changed; then printf 'none\tchanged-in-pr\n'; return 0; fi
  gh="$(_autodrive_qa_entry "$r" "$_AD_MB" .github)"
  wf="$(_autodrive_qa_entry "$r" "$_AD_MB" .github/workflows)"
  case "$gh|$wf" in
    040000\ tree\ *\|040000\ tree\ *) ;;
    *) printf 'none\tno-workflows\n'; return 0 ;;
  esac
  while IFS= read -r -d '' e; do
    case "${e#*$'\t'}" in
      .github/workflows/.*) ;;
      '!') printf 'none\tlist-failed\n'; return 0 ;;
      .github/workflows/*.yml|.github/workflows/*.yaml) ents+=("$e") ;;
    esac
  done < <(autodrive_trust_git "$r" ls-tree -z --long --full-tree --end-of-options "$_AD_MB" -- .github/workflows/ \
             2>/dev/null || printf '!\0')
  if [ "${#ents[@]}" -eq 0 ]; then printf 'none\tno-workflows\n'; return 0; fi
  if [ "${#ents[@]}" -gt 50 ]; then printf 'none\ttoo-many-files\n'; return 0; fi
  for e in "${ents[@]}"; do
    f="${e#*$'\t'.github/workflows/}"
    read -r mode typ oid size <<< "${e%%$'\t'*}"
    case "$f" in ''|*[!A-Za-z0-9._-]*) printf 'none\tunsafe-name\n'; return 0 ;; esac
    case "$mode:$typ" in
      100644:blob|100755:blob) ;;
      *) printf 'none\tnot-regular\n'; return 0 ;;
    esac
    autodrive_is_sha "$oid" || { printf 'none\tnot-regular\n'; return 0; }
    [ "$size" -le 262144 ] 2>/dev/null || { printf 'none\ttoo-large\n'; return 0; }
    oids+=("$oid")
  done
  # The parser prints `D` for a match that is not clean and
  # `C<TAB><working-directory><TAB><command>` for each clean one. It tracks
  # the key path by indentation (sk/si), list items by column (at), and the
  # step a key belongs to by its list item (id).
  prog='
    function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
    function unq(s) {
      s = trim(s); sub(/[ \t]+#.*$/, "", s)
      if (s ~ /^".*"$/ || s ~ /^\047.*\047$/) s = substr(s, 2, length(s) - 2)
      return s
    }
    function inv(s) {
      return s ~ /^(cargo test|cargo nextest run|npm test|npm run test|pytest|python3? -m pytest)([ \t]|$)/
    }
    function has(s) { return s ~ /(cargo test|cargo nextest run|npm test|npm run test|pytest)/ }
    FNR == 1 { sp = 0; blk = -1; pend = -1; split("", at) }
    {
      sub(/\r$/, "")
      line = $0
      match(line, /^ */)
      ind = RLENGTH
      # Inside a block scalar: an invocation in a run: block is not clean.
      if (blk >= 0) {
        if (line ~ /^[ \t]*$/ || ind > blk) {
          if (brun && has(line)) dirty = 1
          next
        }
        blk = -1
      }
      if (line ~ /^[ \t]*(#|$)/ || line ~ /^(---|\.\.\.)/) next
      # A plain run: value continued on the next line is not clean.
      if (pend >= 0 && ind > pend) dirty = 1
      pend = -1
      rest = substr(line, ind + 1)
      while (match(rest, /^-( +|$)/)) { ind += RLENGTH; rest = substr(rest, RLENGTH + 1); at[ind] = ++nit }
      if (!match(rest, /^[A-Za-z0-9_.-]+:/)) next
      key = substr(rest, 1, RLENGTH - 1)
      val = substr(rest, RLENGTH + 1)
      if (val != "" && val !~ /^[ \t]/) next
      val = trim(val)
      if (val ~ /^#/) val = ""
      while (sp > 0 && si[sp] >= ind) sp--
      parent = (sp > 0) ? sk[sp] : ""
      sp++
      sk[sp] = key
      si[sp] = ind
      id = (ind in at) ? at[ind] : ("top" NR)
      if (key == "working-directory") { if (parent == "run") dirty = 1; else wdv[id] = unq(val); next }
      if (val ~ /^[|>][-+0-9]*([ \t]+#.*)?$/) { blk = ind; brun = (key == "run"); next }
      if (key != "run" || val == "") next
      v = val
      sub(/[ \t]+#.*$/, "", v)
      q = e = 0
      if (v ~ /^["\047]/) { q = 1; v = substr(v, 2) }
      while (match(v, /^[A-Za-z_][A-Za-z0-9_]*=[^ \t]*[ \t]+/)) { v = substr(v, RLENGTH + 1); e = 1 }
      if (!inv(v)) next
      pend = ind
      if (q || e || val ~ /[$`"\047\\&;|<>]/) { dirty = 1; next }
      if (!(id in runv)) order[++nr] = id; else dirty = 1
      runv[id] = trim(v)
    }
    END {
      if (dirty) print "D"
      for (i = 1; i <= nr; i++) print "C\t" wdv[order[i]] "\t" runv[order[i]]
    }'
  # One awk run per blob: FNR == 1 already reset all per-file state.
  for oid in "${oids[@]}"; do
    { b="$(autodrive_trust_git "$r" cat-file blob "$oid")"; } 2>/dev/null || { printf 'none\tread-failed\n'; return 0; }
    res="${res}$(printf '%s\n' "$b" | awk "$prog" 2>/dev/null)"$'\n' || { printf 'none\tread-failed\n'; return 0; }
  done
  while IFS= read -r line; do
    case "$line" in
      D)
        bad="1"
        ;;
      C"	"*)
        c=$((c + 1))
        line="${line#C	}"
        wd="${line%%	*}"
        cmd="${line#*	}"
        ;;
    esac
  done <<EOT
$res
EOT
  if [ -n "$bad" ]; then printf 'none\tnot-clean\n'; return 0; fi
  if [ "$c" -eq 0 ]; then printf 'none\tno-match\n'; return 0; fi
  if [ "$c" -gt 1 ]; then printf 'none\tseveral-matches\n'; return 0; fi
  case "$cmd" in ''|*[!A-Za-z0-9\ ._/=+:,@%-]*) printf 'none\tunsafe-command\n'; return 0 ;; esac
  _autodrive_qa_wd "$wd"; wd="$_AD_WD"
  _autodrive_qa_dir_check "$r" "$wd"
  case "$?" in
    0) ;;
    1) printf 'none\tunsafe-dir\n'; return 0 ;;
    *) printf 'none\tdir-not-found\n'; return 0 ;;
  esac
  _autodrive_qa_covers "$r" "$wd" || { printf 'none\tout-of-scope\n'; return 0; }
  printf 'ok\t%s\t%s\n' "$wd" "$cmd"
}

# autodrive_qa_report <dir> -> what the evidence tool needs to choose its
# command, one `<key><TAB><value>` line each, computed in this one shell so
# the diff and the tree walk run once:
#   paths    autodrive_qa_changed_paths
#   kind     autodrive_repo_type
#   greq, docsonly  autodrive_gadugi_required, and autodrive_qa_docs_only as its own field so the round reads the exemption, not greq (#2219)
#   target   autodrive_qa_target
#   decl     autodrive_qa_declaration; declskip ci-job-workflow-changed when
#            qa.toml's scenarios_ci_job was not used for that reason (S3)
#   source, dir, command  `repo-file` or `ci`, when that source gives a command
#   invalid  the qa.toml problem, when qa.toml is invalid (qa-config-invalid)
#   ciskip   the reason the CI source gave none, when it was read
# The declared sources are read only when AUTODRIVE_QA_COMMAND and
# AUTODRIVE_QA_COMMANDS are both unset, so an env command never reads qa.toml.
autodrive_qa_report() {
  local r="${1:-.}" cfg="" ci="" rest=""
  _autodrive_qa_choose "$r"
  printf 'paths\t%s\n' "$(autodrive_qa_changed_paths "$r")"
  printf 'kind\t%s\n' "$(autodrive_repo_type "$r")"
  printf 'greq\t%s\ndocsonly\t%s\n' "$(autodrive_gadugi_required "$r")" "$(autodrive_qa_docs_only "$r")"
  printf 'decl\t%s\n' "$(autodrive_qa_lifting_declaration "$r")"
  printf 'target\t%s\n' "$(autodrive_qa_target "$r")"
  if [ -n "${AUTODRIVE_QA_COMMAND+x}" ] || [ -n "${AUTODRIVE_QA_COMMANDS+x}" ]; then
    return 0
  fi
  cfg="$(autodrive_qa_config "$r")" || cfg="invalid"$'\t'"syntax"
  case "$cfg" in
    none) ;;
    ok$'\t'*$'\t'?*)
      cfg="${cfg#ok$'\t'}"; rest="${cfg#*$'\t'}"
      printf 'source\trepo-file\ndir\t%s\ncommand\t%s\n' "${cfg%%$'\t'*}" "${rest%%$'\t'*}"
      case "$rest" in *$'\t'ci-job:*) ! _autodrive_qa_workflow_changed || printf 'declskip\tci-job-workflow-changed\n' ;; esac
      return 0
      ;;
    invalid$'\t'*) printf 'invalid\t%s\n' "${cfg#invalid$'\t'}"; return 0 ;;
    *) printf 'invalid\tsyntax\n'; return 0 ;;
  esac
  ci="$(autodrive_qa_ci "$r")" || ci="none"$'\t'"read-failed"
  case "$ci" in
    ok$'\t'*$'\t'?*) ci="${ci#ok$'\t'}"
      printf 'source\tci\ndir\t%s\ncommand\t%s\n' "${ci%%$'\t'*}" "${ci#*$'\t'}" ;;
    none$'\t'*) printf 'ciskip\t%s\n' "${ci#none$'\t'}" ;;
    *) printf 'ciskip\tread-failed\n' ;;
  esac
}
