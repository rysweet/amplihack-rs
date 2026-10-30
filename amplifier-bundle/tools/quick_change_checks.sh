#!/usr/bin/env bash
# quick_change_checks.sh - run the repository's own checks for a quick-change
# run and record each one as a command and an exit code (issue #1740).
#
# Usage:
#   quick_change_checks.sh <worktree> <base-sha> <table-out-file>
#
#   Cargo.toml               cargo fmt --all -- --check, always; with changed
#                            .rs files, cargo clippy and cargo test for each
#                            affected crate
#   .pre-commit-config.yaml  pre-commit run --files <changed files>
#   package.json             npm run lint and npm test, when the script exists
#
# A check that cannot run is recorded as `skipped (<reason>)`, from the closed
# set in docs/reference/quick-change-recipe.md, and never counts as passed.
# Check output goes to stderr only. Checks run with publishing and model
# tokens removed from the environment.
#
# Stdout is one line: QUICK_CHANGE_CHECKS=<passed|failed|none> ran= failed= skipped=
# <table-out-file> receives Markdown table rows only. Exit 0 once the checks
# have run, 2 on a usage error. Written for bash 3.2.

set -euo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX

usage() {
  echo "usage: quick_change_checks.sh <worktree> <base-sha> <table-out-file>" >&2
  exit 2
}

[ $# -eq 3 ] || usage
WT="$1"; BASE="$2"; TABLE_OUT="$3"
[[ "$BASE" =~ ^[0-9a-f]{40}$ ]] || { echo "ERROR: base-sha must be a 40-character hex SHA" >&2; usage; }
git -C "$WT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo "ERROR: '$WT' is not a git work tree" >&2; usage; }
git -C "$WT" cat-file -e "${BASE}^{commit}" 2>/dev/null || { echo "ERROR: base $BASE is not a commit" >&2; usage; }
WT="$(cd "$WT" && pwd -P)"

RAN=0; FAILED=0; SKIPPED=0; ROWS=""

# display <text>: one line, with no backtick, pipe or control character, so a
# path cannot break the Markdown table.
display() {
  printf '%s' "$1" | LC_ALL=C tr '`|[:cntrl:]' "'/ "
}

row() {
  ROWS="${ROWS}| \`$(display "$1")\` | $2 |"$'\n'
}

skip() {
  row "$1" "skipped ($2)"
  SKIPPED=$((SKIPPED + 1))
}

# run_check <label> <cmd...>: run in the worktree, output to stderr.
run_check() {
  local label="$1" rc=0
  shift
  echo "--- quick-change check: $label ---" >&2
  ( cd "$WT" && env -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
      "$@" </dev/null >&2 2>&1 ) || rc=$?
  row "$label" "exit $rc"
  RAN=$((RAN + 1))
  [ "$rc" -eq 0 ] || FAILED=$((FAILED + 1))
}

# Changed paths against the base, plus untracked non-ignored files. fsmonitor
# is off, as in the diff-limits tool, so a configured monitor never runs here.
CHANGED=()
while IFS= read -r -d '' path; do
  CHANGED+=("$path")
done < <(git -C "$WT" -c core.fsmonitor=false -c core.quotepath=off diff --no-renames --name-only -z "$BASE"; git -C "$WT" -c core.fsmonitor=false ls-files -z -o --exclude-standard)

# crate_of <path>: the [package] name of the nearest ancestor Cargo.toml.
crate_of() {
  local dir="${1%/*}"
  [ "$dir" != "$1" ] || dir="."
  while :; do
    if [ -f "$WT/$dir/Cargo.toml" ] && LC_ALL=C grep -q '^\[package\]' "$WT/$dir/Cargo.toml"; then
      LC_ALL=C awk '
        /^\[/ { in_pkg = ($0 ~ /^\[package\][[:space:]]*$/); next }
        in_pkg && /^[[:space:]]*name[[:space:]]*=/ {
          sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; exit
        }' "$WT/$dir/Cargo.toml"
      return 0
    fi
    [ "$dir" != "." ] || return 0
    case "$dir" in */*) dir="${dir%/*}" ;; *) dir="." ;; esac
  done
}

# --- Rust ----------------------------------------------------------------------
if [ -f "$WT/Cargo.toml" ]; then
  CRATES=""
  RS_CHANGED=0
  for path in ${CHANGED[@]+"${CHANGED[@]}"}; do
    case "$path" in *.rs) ;; *) continue ;; esac
    RS_CHANGED=1
    name="$(crate_of "$path")"
    if [[ "$name" =~ ^[A-Za-z0-9_-]+$ ]]; then
      case $'\n'"$CRATES" in *$'\n'"$name"$'\n'*) ;; *) CRATES="${CRATES}${name}"$'\n' ;; esac
    elif [ -n "$name" ]; then
      echo "WARNING: crate name '$(display "$name")' is not ^[A-Za-z0-9_-]+\$; not checked" >&2
    fi
  done
  if ! command -v cargo >/dev/null 2>&1; then
    skip "cargo fmt --all -- --check" "cargo not on PATH"
  else
    run_check "cargo fmt --all -- --check" cargo fmt --all -- --check
  fi
  if [ "$RS_CHANGED" -eq 0 ] || [ -z "$CRATES" ]; then
    reason="no .rs files changed"
    command -v cargo >/dev/null 2>&1 || reason="cargo not on PATH"
    skip "cargo clippy --all-targets -- -D warnings" "$reason"
    skip "cargo test" "$reason"
  else
    while IFS= read -r crate; do
      [ -n "$crate" ] || continue
      if ! command -v cargo >/dev/null 2>&1; then
        skip "cargo clippy -p $crate --all-targets -- -D warnings" "cargo not on PATH"
        skip "cargo test -p $crate" "cargo not on PATH"
      else
        run_check "cargo clippy -p $crate --all-targets -- -D warnings" cargo clippy -p "$crate" --all-targets -- -D warnings
        run_check "cargo test -p $crate" cargo test -p "$crate"
      fi
    done <<EOF
$CRATES
EOF
  fi
fi

# --- pre-commit ----------------------------------------------------------------
if [ -f "$WT/.pre-commit-config.yaml" ] && [ "${#CHANGED[@]}" -gt 0 ]; then
  label="pre-commit run --files ${CHANGED[*]}"
  if ! command -v pre-commit >/dev/null 2>&1; then
    skip "$label" "pre-commit not on PATH"
  else
    run_check "$label" pre-commit run --files "${CHANGED[@]}"
  fi
fi

# --- npm -----------------------------------------------------------------------
if [ -f "$WT/package.json" ]; then
  for script in lint test; do
    label="npm run $script"
    [ "$script" != test ] || label="npm test"
    if ! command -v npm >/dev/null 2>&1; then
      skip "$label" "npm not on PATH"
    elif ! command -v jq >/dev/null 2>&1; then
      skip "$label" "jq not on PATH"
    elif ! jq -e --arg s "$script" '(.scripts // {})[$s] | type == "string"' "$WT/package.json" >/dev/null 2>&1; then
      skip "$label" "no \"$script\" script"
    elif [ "$script" = test ]; then
      run_check "$label" npm test
    else
      run_check "$label" npm run "$script"
    fi
  done
fi

# --- report --------------------------------------------------------------------
TABLE_TMP="$(mktemp "${TABLE_OUT}.XXXXXX")"
printf '%s' "$ROWS" > "$TABLE_TMP"
mv -f "$TABLE_TMP" "$TABLE_OUT"

STATUS=none
[ "$RAN" -eq 0 ] || STATUS=passed
[ "$FAILED" -eq 0 ] || STATUS=failed
printf 'QUICK_CHANGE_CHECKS=%s ran=%s failed=%s skipped=%s\n' "$STATUS" "$RAN" "$FAILED" "$SKIPPED"
