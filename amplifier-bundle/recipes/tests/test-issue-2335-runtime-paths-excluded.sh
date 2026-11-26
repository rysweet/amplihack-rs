#!/usr/bin/env bash
# shellcheck source=/dev/null  # the helpers are sourced from a computed path
# test-issue-2335-runtime-paths-excluded.sh — contract for #2335.
#
# amplihack's own runtime files (.amplihack/graph_db, the indexer's files,
# .claude/runtime/) must never make a run's worktree dirty. They are listed in
# the shared info/exclude by the runtime-artifact preflight and by the step-04
# worktree sweep. Tracked files are never hidden.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2335-runtime-paths-excluded.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -uo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
    GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX GIT_CEILING_DIRECTORIES

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tools="$(cd "$here/../../tools" && pwd)"
helper="$tools/workflow_runtime_artifacts.sh"
sweep="$tools/workflow_worktree_sweep.sh"
[ -f "$helper" ] && [ -f "$sweep" ] || { echo "harness: tools missing" >&2; exit 2; }

scratch="$(mktemp -d "${TMPDIR:-/tmp}/issue-2335.XXXXXX")" || exit 2
chmod 0700 "$scratch"
trap 'chmod -R u+rwx "$scratch" 2>/dev/null; rm -rf -- "$scratch"' EXIT
export HOME="$scratch/home"; mkdir -p "$HOME"; chmod 0700 "$HOME"
export GIT_CONFIG_GLOBAL="$scratch/gitconfig" GIT_CONFIG_NOSYSTEM=1
git config --global user.email t@example.com
git config --global user.name Tester
git config --global init.defaultBranch main

PATTERNS=(
  '/index.scip'
  '/.amplihack/graph_db'
  '/.amplihack/indexing*'
  '/.amplihack/indexes/'
  '/.amplihack/index.scip*'
  '/.amplihack/blarify.json'
  '/.amplihack/step*'
  '/.claude/runtime/'
)

pass=0; fail=0
ok()  { pass=$((pass + 1)); echo "PASS: $1"; }
bad() { fail=$((fail + 1)); echo "FAIL: $1" >&2; }
check() { if "${@:2}"; then ok "$1"; else bad "$1"; fi; }

n=0
# new_pair: creates a main repo with one commit and a linked worktree.
# Sets MAIN and WT.
new_pair() {
  n=$((n + 1))
  MAIN="$scratch/main$n"; WT="$scratch/wt$n"
  git init -q "$MAIN" || return 1
  ( cd "$MAIN" && echo base > README.md && git add README.md && git commit -qm base \
      && git worktree add -q -b "feat/t$n" "$WT" ) || return 1
}

make_runtime_files() {
  mkdir -p "$1/.amplihack/indexes" "$1/.claude/runtime/logs"
  : > "$1/.amplihack/graph_db"
  echo 123 > "$1/.amplihack/indexing.pid"
  : > "$1/.amplihack/index.scip"
  : > "$1/.amplihack/index.scip.tmp"
  echo '{}' > "$1/.amplihack/blarify.json"
  : > "$1/.amplihack/step-01.json"
  : > "$1/.amplihack/indexes/a"
  : > "$1/.claude/runtime/logs/x"
}

exclude_file() { git -C "$1" rev-parse --path-format=absolute --git-path info/exclude; }

run_preflight() {
  ( set -euo pipefail; . "$helper"; preflight_known_workflow_runtime_artifacts "$1" ) 2>"$scratch/pre.err"
}

# a. Preflight leaves a linked worktree clean.
t_a() {
  new_pair || return 1; make_runtime_files "$WT"
  run_preflight "$WT" || return 1
  [ -z "$(git -C "$WT" status --porcelain)" ] || { git -C "$WT" status --porcelain >&2; return 1; }
  local p; for p in "${PATTERNS[@]}"; do grep -qxF -- "$p" "$(exclude_file "$WT")" || { echo "missing $p" >&2; return 1; }; done
}
check "a: preflight leaves untracked runtime files out of porcelain" t_a

# b. Sweep writes the same patterns at creation time.
t_b() {
  new_pair || return 1
  ( . "$sweep"; exclude_worktrees_dir "$MAIN" ) 2>"$scratch/sweep.err" || return 1
  make_runtime_files "$WT"
  [ -z "$(git -C "$WT" status --porcelain)" ] || { git -C "$WT" status --porcelain >&2; return 1; }
  grep -qxF '/worktrees/' "$(exclude_file "$MAIN")"
}
check "b: sweep exclusion hides runtime files in a linked worktree" t_b

# b2. The sweep writes every canonical pattern (one list, no copy).
t_b2() {
  new_pair || return 1
  ( . "$sweep"; exclude_worktrees_dir "$MAIN" ) 2>"$scratch/sweep.err" || return 1
  local p; for p in "${PATTERNS[@]}"; do
    grep -qxF -- "$p" "$(exclude_file "$MAIN")" || { echo "sweep missing $p" >&2; return 1; }
  done
}
check "b2: sweep writes every canonical pattern including /index.scip" t_b2

# b3. Sourcing the sweep and running it does not leak set -e / -u / pipefail.
t_b3() {
  new_pair || return 1
  ( set +e +u +o pipefail; . "$sweep"; exclude_worktrees_dir "$MAIN" 2>/dev/null
    case "$-" in *e*|*u*) exit 1;; esac
    set -o | grep -q 'pipefail.*off' ) || return 1
}
check "b3: sweep delegation does not leak shell options" t_b3

# b4. Sweep still refuses a symlinked info/exclude and leaves the target alone.
t_b4() {
  new_pair || return 1
  local ex target; ex="$(exclude_file "$MAIN")"; mkdir -p "$(dirname "$ex")"
  target="$scratch/victim4"; : > "$target"; rm -f "$ex"; ln -s "$target" "$ex"
  ( . "$sweep"; exclude_worktrees_dir "$MAIN" ) 2>/dev/null
  [ ! -s "$target" ]
}
check "b4: sweep leaves a symlinked info/exclude untouched" t_b4

# b5. git check-ignore attributes each runtime path to info/exclude.
t_b5() {
  new_pair || return 1
  ( . "$sweep"; exclude_worktrees_dir "$MAIN" ) 2>/dev/null || return 1
  mkdir -p "$WT/.amplihack" "$WT/.claude/runtime"
  local p out; for p in .amplihack/graph_db .amplihack/indexing.pid .amplihack/index.scip \
      .amplihack/step-x .claude/runtime/x index.scip; do
    out="$(git -C "$WT" check-ignore -v -- "$p")" || { echo "not ignored: $p" >&2; return 1; }
    case "$out" in *info/exclude*) ;; *) echo "wrong source for $p: $out" >&2; return 1;; esac
  done
}
check "b5: check-ignore reports info/exclude for every runtime path" t_b5

# b6. A tracked root index.scip is never hidden.
t_b6() {
  new_pair || return 1
  echo v1 > "$WT/index.scip"; git -C "$WT" add -f index.scip && git -C "$WT" commit -qm track || return 1
  run_preflight "$WT" || return 1
  echo v2 > "$WT/index.scip"
  git -C "$WT" status --porcelain | grep -q '^ M index.scip'
}
check "b6: modified tracked root index.scip is still dirty" t_b6

# b7. An untracked product file named like a nested index.scip is not hidden.
t_b7() {
  new_pair || return 1; run_preflight "$WT" || return 1
  mkdir -p "$WT/sub"; echo x > "$WT/sub/index.scip"
  git -C "$WT" status --porcelain | grep -q '^?? sub/'
}
check "b7: /index.scip is root-anchored, nested files stay visible" t_b7

# c. Tracked .amplihack file stays visible when modified.
t_c() {
  new_pair || return 1
  mkdir -p "$WT/.amplihack"; echo v1 > "$WT/.amplihack/graph_db"
  git -C "$WT" add -f .amplihack/graph_db && git -C "$WT" commit -qm track || return 1
  echo v2 > "$WT/.amplihack/graph_db"
  run_preflight "$WT" || return 1
  git -C "$WT" status --porcelain | grep -q '^ M .amplihack/graph_db'
}
check "c: modified tracked .amplihack file is still dirty" t_c

# d. Idempotent: exactly one copy of each pattern.
t_d() {
  new_pair || return 1
  run_preflight "$WT" && run_preflight "$WT" || return 1
  local p c; for p in "${PATTERNS[@]}"; do
    c="$(grep -cxF -- "$p" "$(exclude_file "$WT")")"; [ "$c" = 1 ] || { echo "$p x$c" >&2; return 1; }
  done
}
check "d: running twice leaves one copy of each pattern" t_d

# e. Unterminated last line keeps the user's rule.
t_e() {
  new_pair || return 1
  local ex; ex="$(exclude_file "$WT")"; mkdir -p "$(dirname "$ex")"
  printf '*.userrule' > "$ex"
  run_preflight "$WT" || return 1
  grep -qxF '*.userrule' "$ex" && grep -qxF '/.amplihack/graph_db' "$ex"
}
check "e: unterminated last line is preserved" t_e

# f. Product files stay visible.
t_f() {
  new_pair || return 1; make_runtime_files "$WT"; echo x > "$WT/new.rs"
  run_preflight "$WT" || return 1
  local s; s="$(git -C "$WT" status --porcelain)"
  [ "$s" = '?? new.rs' ]
}
check "f: untracked product file is still reported, runtime files are not" t_f

# g. Unwritable exclude: one WARNING, exit 0.
t_g() {
  [ "$(id -u)" != 0 ] || { echo "SKIP g: running as root" >&2; return 0; }
  new_pair || return 1
  local ex; ex="$(exclude_file "$WT")"; mkdir -p "$(dirname "$ex")"; : > "$ex"; chmod 0444 "$ex"
  run_preflight "$WT"; local rc=$?
  chmod 0644 "$ex"
  [ "$rc" = 0 ] && [ "$(grep -c '^WARNING:' "$scratch/pre.err")" = 1 ]
}
check "g: unwritable exclude gives one WARNING and exit 0" t_g

# R5. GIT_DIR pointing elsewhere must not redirect the write.
t_r5() {
  new_pair || return 1; local other="$MAIN"
  new_pair || return 1
  local ex_other; ex_other="$(exclude_file "$other")"; mkdir -p "$(dirname "$ex_other")"; : > "$ex_other"
  ( set -euo pipefail; . "$helper"
    GIT_DIR="$other/.git" GIT_WORK_TREE="$other" preflight_known_workflow_runtime_artifacts "$WT" ) 2>/dev/null
  [ ! -s "$ex_other" ] && grep -qxF '/.amplihack/graph_db' "$(exclude_file "$WT")"
}
check "R5: GIT_DIR for another repository is not written to" t_r5

# R6. A symlinked info/exclude is untouched.
t_r6() {
  new_pair || return 1
  local ex target; ex="$(exclude_file "$WT")"; mkdir -p "$(dirname "$ex")"
  target="$scratch/victim"; : > "$target"; rm -f "$ex"; ln -s "$target" "$ex"
  run_preflight "$WT"; local rc=$?
  [ "$rc" = 0 ] && [ ! -s "$target" ] && grep -q '^WARNING:' "$scratch/pre.err"
}
check "R6: symlinked info/exclude is untouched, WARNING, exit 0" t_r6

# R8. A symlinked .claude/runtime is removed as a link only.
t_r8() {
  new_pair || return 1
  mkdir -p "$scratch/keep"; : > "$scratch/keep/precious"; mkdir -p "$WT/.claude"
  ln -s "$scratch/keep" "$WT/.claude/runtime"
  run_preflight "$WT" || return 1
  [ -e "$scratch/keep/precious" ] && [ ! -e "$WT/.claude/runtime" ]
}
check "R8: symlinked .claude/runtime target survives cleanup" t_r8

echo "passed=$pass failed=$fail"
[ "$fail" = 0 ]
