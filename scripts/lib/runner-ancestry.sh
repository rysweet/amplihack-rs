# shellcheck shell=sh
# Runner ancestry primitives for the recipe-runner installers (issue #1593).
#
# Sourced by scripts/install-pinned-recipe-runner.sh. claude-plugin/bin/
# install-runtime ships without scripts/, so it carries a copy of the block
# between the BEGIN and END markers below; tests/runner_ancestry_lib_parity_test.sh
# fails when the two copies differ. Edit both together.
#
# POSIX sh only: install-runtime runs under /bin/sh. Nothing read from a
# receipt, a pin or the environment is evaluated, and every SHA is checked
# against ^[0-9a-f]{40}$ before it reaches git.
#
#   runner_receipt_revision FILE URL     installed SHA from a Cargo receipt
#   runner_relation GITDIR NEW OLD       equal|descendant|ancestor|diverged|missing
#   runner_classify INSTALLED PIN URL CARGO_HOME
#                                        Unknown|Equal|Older|Newer|Diverged|Unverifiable
#   runner_skip_message INSTALLED PIN CLASS
#
# The guard prevents accidental regression only; integrity still rests on the
# pinned SHA, HTTPS and cargo install --locked --rev.

# BEGIN runner-ancestry
runner_is_sha() {
  case $1 in
    *[!0123456789abcdef]*) return 1 ;;
  esac
  [ "${#1}" -eq 40 ]
}

# A git command that cannot prompt, run hooks or fsmonitor, use stored
# credentials, follow an inherited repository, or reach a transport other
# than https and local files.
runner_git() {
  (
    unset GIT_ASKPASS SSH_ASKPASS GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE \
      GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR \
      GIT_NAMESPACE GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT
    GIT_TERMINAL_PROMPT=0
    export GIT_TERMINAL_PROMPT
    git -c protocol.allow=never -c protocol.https.allow=always \
      -c protocol.file.allow=always -c credential.helper= \
      -c core.fsmonitor=false -c core.hooksPath=/dev/null \
      -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 "$@"
  )
}

# runner_receipt_revision FILE URL: print the revision of the one
# recipe-runner-rs entry installed from exactly URL (followed by '?' or '#')
# that lists the recipe-runner-rs binary. Prints nothing for a missing,
# oversize or ambiguous receipt. Always exits 0.
runner_receipt_revision() {
  [ -f "$1" ] || return 0
  _rrr_size=$(wc -c <"$1" 2>/dev/null) || return 0
  _rrr_size=$(printf '%s' "$_rrr_size" | tr -d ' ')
  case $_rrr_size in
    '' | *[!0123456789]*) return 0 ;;
  esac
  [ "$_rrr_size" -le 1048576 ] || return 0
  RUNNER_RECEIPT_URL=$2 LC_ALL=C awk '
    { text = text $0 "\n" }
    END {
      url = ENVIRON["RUNNER_RECEIPT_URL"]
      prefix = "\"recipe-runner-rs "
      count = 0
      s = text
      while ((i = index(s, prefix)) > 0) {
        s = substr(s, i + length(prefix))
        j = index(s, "\"")
        if (j == 0) break
        key = substr(s, 1, j - 1)
        record = substr(s, j + 1)
        k = index(key, " (git+")
        if (k < 2 || substr(key, length(key), 1) != ")") continue
        origin = substr(key, k + 6, length(key) - k - 6)
        if (url == "" || index(origin, url) != 1) continue
        next_char = substr(origin, length(url) + 1, 1)
        if (next_char != "?" && next_char != "#") continue
        sha = origin
        while ((h = index(sha, "#")) > 0) sha = substr(sha, h + 1)
        if (sha == origin) continue
        e = index(record, "}")
        if (e > 0) record = substr(record, 1, e)
        b = index(record, "\"bins\":[")
        if (b == 0) continue
        bins = substr(record, b + 8)
        e = index(bins, "]")
        if (e == 0) continue
        if (index(substr(bins, 1, e), "\"recipe-runner-rs\"") == 0) continue
        if (!(sha in seen)) {
          seen[sha] = 1
          count++
          found = sha
        }
      }
      if (count == 1 && length(found) == 40 && found !~ /[^0-9a-f]/) print found
    }
  ' "$1" 2>/dev/null
  return 0
}

# runner_relation GITDIR NEW OLD: how NEW relates to OLD in the repository at
# GITDIR (bare, or a work tree). Prints missing when either commit is absent
# or the repository cannot be read; prints nothing and returns 2 for invalid
# arguments.
runner_relation() {
  runner_is_sha "$2" && runner_is_sha "$3" || return 2
  case $1 in
    '' | -*) return 2 ;;
  esac
  _rl_dir=$1
  [ -d "$_rl_dir/.git" ] && _rl_dir=$_rl_dir/.git
  for _rl_sha in "$2" "$3"; do
    if ! runner_git --git-dir="$_rl_dir" cat-file -e "$_rl_sha^{commit}" >/dev/null 2>&1; then
      echo missing
      return 0
    fi
  done
  if [ "$2" = "$3" ]; then
    echo equal
    return 0
  fi
  runner_git --git-dir="$_rl_dir" merge-base --is-ancestor "$2" "$3" >/dev/null 2>&1
  _rl_status=$?
  if [ "$_rl_status" -eq 0 ]; then
    echo ancestor
    return 0
  fi
  if [ "$_rl_status" -ne 1 ]; then
    echo missing
    return 0
  fi
  runner_git --git-dir="$_rl_dir" merge-base --is-ancestor "$3" "$2" >/dev/null 2>&1
  _rl_status=$?
  if [ "$_rl_status" -eq 0 ]; then
    echo descendant
  elif [ "$_rl_status" -eq 1 ]; then
    echo diverged
  else
    echo missing
  fi
  return 0
}

# runner_relation_from SOURCE NEW OLD: runner_relation against a local
# repository directory, or against an https:// or file:// URL whose two
# commits are fetched into a temporary bare repository.
runner_relation_from() {
  case $1 in
    '' | -*) return 2 ;;
  esac
  if [ -d "$1" ]; then
    runner_relation "$1" "$2" "$3"
    return $?
  fi
  case $1 in
    https://* | file://*) ;;
    *) return 2 ;;
  esac
  runner_is_sha "$2" && runner_is_sha "$3" || return 2
  _rf_tmp=$(mktemp -d 2>/dev/null) || return 2
  if runner_git init --quiet --bare "$_rf_tmp" >/dev/null 2>&1 &&
    runner_git --git-dir="$_rf_tmp" fetch --quiet --no-tags \
      --filter=blob:none -- "$1" "$2" "$3" >/dev/null 2>&1; then
    runner_relation "$_rf_tmp" "$2" "$3"
  fi
  rm -rf "$_rf_tmp"
  return 0
}

# runner_classify INSTALLED PIN URL CARGO_HOME: print the class of the
# installed revision against the pin. RECIPE_RUNNER_ANCESTRY_REPO (tests)
# replaces both ancestry sources; otherwise Cargo's git databases are read
# first and URL is fetched only when they cannot answer.
runner_classify() {
  if [ -z "$1" ]; then
    echo Unknown
    return 0
  fi
  if ! runner_is_sha "$1" || ! runner_is_sha "$2"; then
    echo Unverifiable
    return 0
  fi
  if [ "$1" = "$2" ]; then
    echo Equal
    return 0
  fi
  _rc_relation=
  if [ -n "${RECIPE_RUNNER_ANCESTRY_REPO:-}" ]; then
    _rc_relation=$(runner_relation_from "$RECIPE_RUNNER_ANCESTRY_REPO" "$1" "$2")
  else
    for _rc_db in "$4"/git/db/amplihack-recipe-runner-*; do
      [ -d "$_rc_db" ] || continue
      _rc_relation=$(runner_relation "$_rc_db" "$1" "$2")
      case $_rc_relation in
        ancestor | descendant | diverged) break ;;
      esac
      _rc_relation=
    done
    if [ -z "$_rc_relation" ]; then
      _rc_relation=$(runner_relation_from "$3" "$1" "$2")
    fi
  fi
  case $_rc_relation in
    ancestor) echo Older ;;
    descendant) echo Newer ;;
    diverged) echo Diverged ;;
    *) echo Unverifiable ;;
  esac
  return 0
}

# runner_skip_message INSTALLED PIN CLASS: the warning for a kept runner.
runner_skip_message() {
  case $3 in
    Newer)
      printf 'kept recipe-runner-rs %s (newer than pinned %s); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to downgrade\n' "$1" "$2"
      ;;
    Diverged)
      printf 'kept recipe-runner-rs %s (diverged from pinned %s); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to replace it\n' "$1" "$2"
      ;;
    *)
      printf 'kept recipe-runner-rs %s (could not verify order against pinned %s); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to replace it\n' "$1" "$2"
      ;;
  esac
}
# END runner-ancestry
