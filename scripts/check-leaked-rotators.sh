#!/usr/bin/env bash
# Issue #1690: report `amplihack profile rotate watch --spawned` daemons that a
# test run left behind.
#
# A process is reported when all of these hold:
#   - it is owned by this effective uid (checked before anything is read);
#   - its argv holds `profile`, `rotate`, `watch` and `--spawned` as exact
#     entries. These are the entries `daemon::spawned_args` builds; the unit
#     test `the_leak_scan_matches_the_argv_spawn_builds` (rotate/tests_daemon.rs)
#     fails if either side changes without the other;
#   - its executable (/proc/<pid>/exe, a trailing " (deleted)" removed, or
#     argv[0] when the link is unreadable) is under a `target*/` directory,
#     $CARGO_TARGET_DIR or this repo's target/ (symlinks resolved), so a
#     test build and never an installed amplihack;
#   - its HOME is under $TMPDIR or /tmp, so a test's temp home.
#
# Each match prints one `leaked rotator:` line with its pid, exe, HOME and
# AMPLIHACK_CLAUDE_PROFILE_ROOT only, control characters removed and each
# value cut short: CI logs are public, and environ can hold tokens.
#
# This script only reports. It sends no signal to any process.
#
# Exit: 0 clean (or no /proc on this host), 1 a leak was found, 2 /proc is
# there but could not be scanned.
set -u

if [ ! -r /proc/self/cmdline ]; then
  echo "check-leaked-rotators: no /proc on this host; nothing to scan"
  exit 0
fi

tmp_root="${TMPDIR:-/tmp}"
tmp_root="${tmp_root%/}"
# A TMPDIR of `/` (empty once trimmed) or a relative one would make every
# HOME look like a temp home; fall back to /tmp alone.
case "$tmp_root" in /?*) ;; *) tmp_root=/tmp ;; esac

if ! stat -c %u "/proc/$$" >/dev/null 2>&1 \
  || ! tr '\0' '\n' <"/proc/$$/cmdline" >/dev/null 2>&1; then
  echo "check-leaked-rotators: cannot read /proc/$$; scan impossible" >&2
  exit 2
fi

# clean VALUE: control characters removed, at most 256 bytes. The cut is a
# shell substring under LC_ALL=C (bytes, not characters), not `head -c`: a
# stage that stops reading early is banned in CI scripts (issue #1434).
clean() {
  local LC_ALL=C s
  s=$(printf '%s' "$1" | tr -d '\000-\037\177')
  printf '%s' "${s:0:256}"
}

# env_value PID NAME: the first NAME=... entry of the process's environ. One
# awk reads all its input, for the same reason as above.
env_value() {
  tr '\0' '\n' <"/proc/$1/environ" 2>/dev/null \
    | awk -v k="$2=" 'index($0, k) == 1 && !f { print substr($0, length(k) + 1); f = 1 }'
}

# Build directories besides `*/target*/*`: $CARGO_TARGET_DIR and this repo's
# target/, symlinks resolved, since /proc/<pid>/exe holds the resolved path. A
# target/ that is a symlink to, say, /d0/cargo-targets/<name>/ is otherwise
# missed.
build_dirs=()
script_root=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd -P) || script_root=""
for d in "${CARGO_TARGET_DIR:-}" "${script_root:+$script_root/target}"; do
  [ -n "$d" ] && [ -d "$d" ] || continue
  r=$(cd "$d" 2>/dev/null && pwd -P) || continue
  [ "$r" = / ] && continue
  build_dirs+=("$r")
done

# is_test_build EXE: under a `target*/` directory or one of $build_dirs.
is_test_build() {
  case "$1" in */target*/*) return 0 ;; esac
  local r
  for r in ${build_dirs[@]+"${build_dirs[@]}"}; do
    case "$1" in "$r"/*) return 0 ;; esac
  done
  return 1
}

# Pure bash builtins until a process's argv matches: a host can run
# thousands of processes, and a fork per process makes the scan crawl.
seen=0
leaks=0
for dir in /proc/[0-9]*; do
  [ -e "$dir" ] || continue
  seen=$((seen + 1))
  # Owned by this effective uid; checked before anything else is read.
  [ -O "$dir" ] || continue
  p=${dir#/proc/}

  argv0=""
  have_profile=0 have_rotate=0 have_watch=0 have_spawned=0 n=0
  { while IFS= read -r -d '' a; do
      n=$((n + 1))
      [ "$n" -eq 1 ] && argv0=$a
      case "$a" in
        profile) have_profile=1 ;;
        rotate) have_rotate=1 ;;
        watch) have_watch=1 ;;
        --spawned) have_spawned=1 ;;
      esac
    done <"$dir/cmdline"; } 2>/dev/null || continue
  [ $((have_profile + have_rotate + have_watch + have_spawned)) -eq 4 ] || continue

  exe=$(readlink "$dir/exe" 2>/dev/null) || exe=""
  exe="${exe% (deleted)}"
  [ -n "$exe" ] || exe=$argv0
  is_test_build "$exe" || continue

  home=$(env_value "$p" HOME)
  [ -n "$home" ] || continue
  case "$home" in
    "$tmp_root"/* | /tmp/*) ;;
    *) continue ;;
  esac

  root=$(env_value "$p" AMPLIHACK_CLAUDE_PROFILE_ROOT)
  printf 'leaked rotator: pid=%s exe=%s HOME=%s AMPLIHACK_CLAUDE_PROFILE_ROOT=%s\n' \
    "$p" "$(clean "$exe")" "$(clean "$home")" "$(clean "${root:--}")"
  leaks=$((leaks + 1))
done

if [ "$seen" -eq 0 ]; then
  echo "check-leaked-rotators: saw no processes under /proc; scan impossible" >&2
  exit 2
fi
if [ "$leaks" -gt 0 ]; then
  echo "check-leaked-rotators: $leaks leaked rotator daemon(s); a test started a real 'profile rotate watch --spawned' (issue #1690)" >&2
  exit 1
fi
echo "check-leaked-rotators: no leaked rotator daemons"
exit 0
