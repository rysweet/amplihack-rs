#!/usr/bin/env bash
# Issue #1699 — the real profile root guard (scripts/check-real-profile-root.sh)
# notices when a test run changes a profile settings.json under a real profile
# root, and changes nothing itself.
#
# Every root here is a decoy under a fresh `mktemp -d`; the guard is run with
# GUARD_HOME (and HOME) pointing at a decoy HOME, and AMPLIHACK_CLAUDE_PROFILE_ROOT
# set or unset per case. The real HOME is never read.
#
# Under test:
#   1. static rules: the guard never evals, traces, or writes with rm/mv/cp/tee;
#   2. snapshot then compare with no change: exit 0, "N settings.json files
#      unchanged";
#   3. a changed settings.json: exit 1, a CHANGED line naming the path;
#      an added one: ADDED; a removed one: REMOVED;
#   4. the decoy root's listing and hashes are identical after snapshot and
#      compare (the guard is read-only);
#   5. no roots: exit 0, "nothing to check";
#   6. bad usage, and a snapshot file inside a watched root: exit 2 with the
#      documented messages;
#   7. roots from all four routes are watched, and one root reached twice is
#      counted once;
#   8. a root path holding a space and a `$(...)` string runs no command.
set +e -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GUARD="$ROOT/scripts/check-real-profile-root.sh"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

if [ ! -f "$GUARD" ]; then
  echo "missing $GUARD"
  exit 1
fi

# --- 1. static rules ---------------------------------------------------------
if grep -nE '\beval\b|set -x|\brm\b|\bmv\b|\bcp\b|\btee\b' "$GUARD" \
  | grep -v '^[0-9]*:[[:space:]]*#'; then
  fail "guard contains eval/set -x/rm/mv/cp/tee"
else
  pass "guard never evals, traces or writes files it watches"
fi

tmp=""
cleanup() { [ -n "$tmp" ] && [ -d "$tmp" ] && rm -rf -- "$tmp"; }
trap cleanup EXIT
tmp=$(cd "$(mktemp -d)" && pwd -P)

# run_guard HOME MODE SNAPFILE [ROOT_ENV] -> sets $out and $rc
out=""
rc=0
run_guard() {
  local home=$1 mode=$2 snap=$3 root_env=${4-}
  if [ -n "$root_env" ]; then
    out=$(env -u AMPLIHACK_CLAUDE_PROFILE_ROOT HOME="$home" GUARD_HOME="$home" \
      AMPLIHACK_CLAUDE_PROFILE_ROOT="$root_env" bash "$GUARD" "$mode" "$snap" 2>&1)
  else
    out=$(env -u AMPLIHACK_CLAUDE_PROFILE_ROOT HOME="$home" GUARD_HOME="$home" \
      bash "$GUARD" "$mode" "$snap" 2>&1)
  fi
  rc=$?
}

# has GREP-ARGS... -> grep "$out" without an early-exit pipeline (#1434)
has() { grep "$@" <<<"$out" >/dev/null; }

# listing PATH -> path, size and sha256 of every file under PATH, sorted
listing() {
  local f
  while IFS= read -r -d '' f; do
    printf '%s\t%s\t%s\n' "$f" "$(wc -c <"$f")" "$(sha256sum <"$f")"
  done < <(find "$1" -type f -print0) | LC_ALL=C sort
}

make_profile() { # ROOT NAME
  mkdir -p "$1/$2"
  printf '{"hooks":{},"statusLine":{"type":"command","command":"x"}}\n' >"$1/$2/settings.json"
}

# --- 2/3/4. one decoy root via ~/.claude-profiles ----------------------------
home="$tmp/home"
root="$home/.claude-profiles"
make_profile "$root" work
make_profile "$root" personal
printf '{"model":"m"}\n' >"$root/settings.json"
mkdir -p "$tmp/work"
snap="$tmp/work/profile-root.snap"

before=$(listing "$root")
run_guard "$home" snapshot "$snap"
[ "$rc" -eq 0 ] && pass "snapshot exits 0" || fail "snapshot exit $rc: $out"
run_guard "$home" compare "$snap"
if [ "$rc" -eq 0 ] && has -x 'check-real-profile-root: 3 settings.json files unchanged'; then
  pass "unchanged root: exit 0 and the count line"
else
  fail "unchanged root: exit $rc, output: $out"
fi
after=$(listing "$root")
[ "$before" = "$after" ] && pass "decoy root byte-identical after snapshot+compare" \
  || fail "guard changed the decoy root"

printf '{"hooks":{"Stop":[]}}\n' >"$root/work/settings.json"
run_guard "$home" compare "$snap"
if [ "$rc" -eq 1 ] && has -E "^CHANGED +$root/work/settings.json\$"; then
  pass "changed settings.json: exit 1 and a CHANGED line naming it"
else
  fail "changed settings.json: exit $rc, output: $out"
fi

run_guard "$home" snapshot "$snap"
make_profile "$root" scratch
rm -f -- "$root/personal/settings.json"
run_guard "$home" compare "$snap"
if [ "$rc" -eq 1 ] \
  && has -E "^ADDED +$root/scratch/settings.json\$" \
  && has -E "^REMOVED +$root/personal/settings.json\$"; then
  pass "added and removed files are reported"
else
  fail "added/removed: exit $rc, output: $out"
fi

# Deeper files and non-settings files are not watched.
run_guard "$home" snapshot "$snap"
mkdir -p "$root/work/deep"
printf '{}\n' >"$root/work/deep/settings.json"
printf 'token\n' >"$root/work/.credentials.json"
run_guard "$home" compare "$snap"
[ "$rc" -eq 0 ] && pass "only <root>/settings.json and <root>/*/settings.json are watched" \
  || fail "unwatched files reported: exit $rc, output: $out"

# --- 5. no roots --------------------------------------------------------------
empty_home="$tmp/empty-home"
mkdir -p "$empty_home"
run_guard "$empty_home" snapshot "$tmp/work/empty.snap"
run_guard "$empty_home" compare "$tmp/work/empty.snap"
if [ "$rc" -eq 0 ] && has -x 'check-real-profile-root: nothing to check'; then
  pass "no roots: exit 0, nothing to check"
else
  fail "no roots: exit $rc, output: $out"
fi

# --- 6. usage and snapshot-in-root -------------------------------------------
out=$(env HOME="$home" GUARD_HOME="$home" bash "$GUARD" 2>&1); rc=$?
if [ "$rc" -eq 2 ] && has -x 'usage: check-real-profile-root.sh snapshot|compare <snapshot-file>'; then
  pass "no arguments: exit 2 and the usage line"
else
  fail "no arguments: exit $rc, output: $out"
fi
run_guard "$home" frobnicate "$snap"
[ "$rc" -eq 2 ] && pass "unknown mode: exit 2" || fail "unknown mode: exit $rc, output: $out"

before=$(listing "$root")
run_guard "$home" snapshot "$root/work/guard.snap"
if [ "$rc" -eq 2 ] && has -F "check-real-profile-root: refusing snapshot file \"$root/work/guard.snap\" inside watched root \"$root\""; then
  pass "snapshot inside a watched root: exit 2 and the refusal"
else
  fail "snapshot inside root: exit $rc, output: $out"
fi
[ ! -e "$root/work/guard.snap" ] && [ "$before" = "$(listing "$root")" ] \
  && pass "refused snapshot wrote nothing under the root" \
  || fail "refused snapshot wrote under the root"

# --- 7. all four routes, de-duplicated ---------------------------------------
h7="$tmp/h7"
env_root="$tmp/env-root"
saved_root="$tmp/saved-root"
make_profile "$env_root" a
make_profile "$saved_root" b
make_profile "$h7/.amplihack/claude-profiles" c
make_profile "$h7/.claude-profiles" d
printf '%s\n' "$saved_root" >"$h7/.amplihack/profile-root"
# The env root reached a second time through a symlinked path.
ln -s "$env_root" "$tmp/env-root-link"
run_guard "$h7" snapshot "$tmp/work/h7.snap" "$tmp/env-root-link"
run_guard "$h7" compare "$tmp/work/h7.snap" "$env_root"
if [ "$rc" -eq 0 ] && has -x 'check-real-profile-root: 4 settings.json files unchanged'; then
  pass "env, profile-root file, claude-profiles and ~/.claude-profiles all watched once"
else
  fail "four routes: exit $rc, output: $out"
fi
for f in "$env_root/a" "$saved_root/b" "$h7/.amplihack/claude-profiles/c" "$h7/.claude-profiles/d"; do
  run_guard "$h7" snapshot "$tmp/work/h7.snap" "$env_root"
  printf '{"changed":true}\n' >"$f/settings.json"
  run_guard "$h7" compare "$tmp/work/h7.snap" "$env_root"
  [ "$rc" -eq 1 ] && pass "change under $(basename "$(dirname "$f")")/$(basename "$f") detected" \
    || fail "change in $f missed: exit $rc, output: $out"
done

# ~/.claude/settings.json is never watched.
mkdir -p "$h7/.claude"
printf '{}\n' >"$h7/.claude/settings.json"
run_guard "$h7" snapshot "$tmp/work/h7.snap" "$env_root"
printf '{"x":1}\n' >"$h7/.claude/settings.json"
run_guard "$h7" compare "$tmp/work/h7.snap" "$env_root"
[ "$rc" -eq 0 ] && pass "~/.claude/settings.json is not watched" \
  || fail "~/.claude/settings.json was watched: exit $rc, output: $out"

# --- 8. hostile root path ------------------------------------------------------
h8="$tmp/h8"
marker="$tmp/pwned"
evil_root="$tmp/evil root \$(touch $marker)"
make_profile "$evil_root" w
mkdir -p "$h8/.amplihack"
printf '%s\n' "$evil_root" >"$h8/.amplihack/profile-root"
run_guard "$h8" snapshot "$tmp/work/h8.snap"
run_guard "$h8" compare "$tmp/work/h8.snap"
if [ ! -e "$marker" ] && [ "$rc" -eq 0 ]; then
  pass "a root path with a space and \$(...) runs no command and is watched"
else
  fail "hostile root path: marker exists=$([ -e "$marker" ] && echo yes || echo no), exit $rc, output: $out"
fi

if [ "$fails" -gt 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"
