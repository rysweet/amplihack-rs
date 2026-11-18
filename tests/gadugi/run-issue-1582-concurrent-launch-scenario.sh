#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #1582: two launches that
# start together on one home with missing framework assets re-stage once.
#
# Before the fix the launch bootstrap decided to re-stage before it took
# ~/.amplihack/.install.lock. Two launches that started together both decided,
# then ran one after the other, so the second re-staged again over the first
# one's fresh install. With the fix, the launch that waited re-checks the
# assets under the lock and finds nothing to do.
#
# This runs the real `amplihack` binary from the outside, under env -i, with a
# HOME of its own inside one mktemp directory:
#   1. `amplihack install --local <this checkout>`, then records the files
#      under ~/.amplihack/.claude and the release directories.
#   2. Removes ~/.amplihack/.claude/agents. Only the launch bootstrap checks
#      for it; startup self-heal checks the stamp and the bundle, which stay
#      as installed.
#   3. Takes the install lock itself (flock(1) here and amplihack's lock are
#      both flock(2) on the same file), starts two `amplihack claude`
#      launches, each on a pseudo-terminal from script(1) because the
#      bootstrap runs only on a terminal, and reads until each launch has
#      printed the lock-wait line. Both have then decided, without the lock,
#      that the assets are missing, and both are waiting for the lock.
#   4. Releases the lock and waits for both launches.
#
# Assertions: both launches exit 0; exactly one new release directory;
# exactly one "Bootstrapping amplihack framework assets" line across both
# launches; every path recorded in step 1 exists again. Without the fix both
# launches print that line and each creates a release directory.
#
# There are no timers. A reader per launch copies its output to a log and
# writes one token to a FIFO: `waiting` when the lock-wait line appears (it is
# printed after about two seconds of waiting), or `exited` if the launch ends
# without it. A launch that never waits for the lock fails the scenario at
# once. A launch that blocks anywhere else blocks the scenario; CI bounds the
# step with timeout-minutes.
#
# claude, node, amplihack-hooks and recipe-runner-rs are stubs first on PATH,
# so nothing real is launched. AMPLIHACK_HOME names this checkout, so the
# re-stage reads it and fetches nothing, as long as the binary under test was
# built from this checkout.
#
# Run by hand (Linux: flock(1) and script(1) come from util-linux):
#   cargo install --path bins/amplihack --locked --root target/qa-1582
#   AMPLIHACK_1582_QA_BIN="$PWD/target/qa-1582/bin/amplihack" \
#     bash tests/gadugi/run-issue-1582-concurrent-launch-scenario.sh
# On success it prints ALL_CASES_PASSED and exits 0. Otherwise it prints
# SCENARIO_FAILED and exits 1. CI's Install Smoke Test job runs it against the
# binary it installs.
set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

die() {
  echo "FAIL: $1"
  echo "SCENARIO_FAILED"
  exit 1
}

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_1582_QA_BIN:-}"
[ -n "$BIN" ] || die "set AMPLIHACK_1582_QA_BIN to the amplihack binary under test"
case "$BIN" in
  *$'\n'*) die "AMPLIHACK_1582_QA_BIN contains a newline" ;;
  /*) ;;
  *) die "AMPLIHACK_1582_QA_BIN must be an absolute path (got '$BIN')" ;;
esac
if ! { [ -f "$BIN" ] && [ -x "$BIN" ]; }; then
  die "AMPLIHACK_1582_QA_BIN is not an executable file: $BIN"
fi
for tool in flock script mkfifo git find stat comm; do
  command -v "$tool" >/dev/null 2>&1 || die "this scenario needs $tool (Linux, util-linux)"
done
echo "binary under test: $BIN"

# --- one private directory for everything ------------------------------------
root="$(mktemp -d "${TMPDIR:-/tmp}/issue-1582-qa.XXXXXX")" ||
  die "cannot create a temporary directory"
case "$root" in
  /?*) ;;
  *) die "the temporary directory is not an absolute path: '$root'" ;;
esac
# The scenario never reads or assigns HOME for its own paths: every path below
# is under $qa_home, and every child gets HOME=$qa_home through env -i.
qa_home="$root/home"

lock_held=0
lockfd=""
tok=""
# PIDs this run started and has not waited for yet. Only these are stopped.
pending=()

cleanup() {
  set +e
  local p
  for p in "${pending[@]}"; do
    kill "$p" 2>/dev/null
    wait "$p" 2>/dev/null
  done
  if [ "$lock_held" = 1 ]; then
    flock -u "$lockfd"
    exec {lockfd}>&-
    lock_held=0
  fi
  rm -rf -- "${root:?}"
}
trap cleanup EXIT

# reap <pid>: wait for one recorded PID, leave its status in $reaped_status
# and drop it from $pending.
reaped_status=0
reap() {
  local target="$1" p
  local kept=()
  reaped_status=0
  wait "$target" || reaped_status=$?
  for p in "${pending[@]}"; do
    [ "$p" = "$target" ] || kept+=("$p")
  done
  pending=("${kept[@]}")
}

bin="$root/bin"
mkdir -p "$qa_home" "$bin" "$root/tmp" "$root/work"

# --- stub tools ---------------------------------------------------------------
# Padded past the installer's minimum binary size, as tests/qa/codex-startup.sh
# does.
write_stub() {
  cat > "$bin/$1"
  printf '# %1200s\n' '' >> "$bin/$1"
  chmod 700 "$bin/$1"
}
write_stub claude <<'STUB'
#!/bin/sh
case "$*" in
  *--version*) echo '2.1.319 (Claude Code)' ;;
  *plugin*) echo '{}' ;;
esac
exit 0
STUB
write_stub node <<'STUB'
#!/bin/sh
echo v24.1.0
STUB
write_stub amplihack-hooks <<'STUB'
#!/bin/sh
echo '{}'
STUB
write_stub recipe-runner-rs <<'STUB'
#!/bin/sh
printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}'
STUB

# The only environment any child gets. AMPLIHACK_NONINTERACTIVE and
# AMPLIHACK_SKIP_AUTO_INSTALL are deliberately absent: the launch bootstrap
# runs only interactively, and self-heal must run as it does for a user.
CHILD_ENV=(
  "PATH=$bin:/usr/bin:/bin"
  "HOME=$qa_home"
  "TERM=dumb"
  "LANG=C.UTF-8"
  "TMPDIR=$root/tmp"
  "AMPLIHACK_HOME=$REPO_ROOT"
  "AMPLIHACK_AGENT_BINARY=claude"
  "AMPLIHACK_NO_UPDATE_CHECK=1"
  "AMPLIHACK_NO_FRESHNESS_CHECK=1"
  "AMPLIHACK_SKIP_MMDC=1"
  "AMPLIHACK_ROTATE_AUTOSTART=0"
  "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH=$bin/amplihack-hooks"
  "RECIPE_RUNNER_RS_PATH=$bin/recipe-runner-rs"
  # Only for a run as root in a container: a root launch outside a sandbox
  # stops before the bootstrap.
  "IS_SANDBOX=1"
  "AMPLIHACK_1582_BIN=$BIN"
  "GIT_CONFIG_NOSYSTEM=1"
  "GIT_CONFIG_GLOBAL=/dev/null"
  "GIT_TERMINAL_PROMPT=0"
)

amp="${qa_home:?}/.amplihack"
claude_dir="$amp/.claude"
releases="$amp/releases"
target="$claude_dir/agents"
lockfile="$amp/.install.lock"

# Every path under ~/.amplihack/.claude, following links, relative and sorted.
list_files() {
  find -L "$claude_dir" -mindepth 1 -printf '%P\n' | LC_ALL=C sort
}
# Release roots only. Names starting with a dot, such as .retired, are not releases.
list_releases() {
  find "$releases" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%f\n' |
    LC_ALL=C sort
}

# --- 1. install from this checkout -------------------------------------------
if ! (trap - EXIT; cd "$root/work" && exec env -i "${CHILD_ENV[@]}" "$BIN" install --local "$REPO_ROOT") \
    </dev/null >"$root/install.log" 2>&1; then
  tail -40 "$root/install.log" | sed 's/^/    install: /'
  die "amplihack install --local failed"
fi
[ -d "$target" ] ||
  die "install --local staged no $target; the staged layout changed, so remove another framework directory"
[ -d "$releases" ] || die "install --local created no releases directory"
list_files > "$root/files.installed" || die "cannot list $claude_dir"
list_releases > "$root/releases.installed" || die "cannot list $releases"
echo "installed: $(wc -l < "$root/files.installed") paths under ~/.amplihack/.claude, $(wc -l < "$root/releases.installed") release(s)"

# --- 2. remove a framework directory only the bootstrap checks ----------------
rm -rf -- "${target:?}"
if [ -e "$target" ] || [ -L "$target" ]; then
  die "could not remove $target"
fi
echo "removed ~/.amplihack/.claude/agents in the scenario home"

# --- 3. hold the install lock while both launches start ----------------------
# The install created the lock file. The scenario never creates it, so the
# open below cannot follow a link or make a file with other permissions.
if [ ! -f "$lockfile" ] || [ -L "$lockfile" ]; then
  die "install left no regular lock file at $lockfile"
fi
lock_meta="$(stat -c '%a %u' "$lockfile")"
[ "$lock_meta" = "600 $(id -u)" ] ||
  die "lock file mode and owner are '$lock_meta', expected '600 $(id -u)'"
exec {lockfd}<>"$lockfile"
flock -x "$lockfd"
lock_held=1

# Read-write, so neither side blocks on open and the readers never see SIGPIPE.
mkfifo "$root/tokens"
exec {tok}<>"$root/tokens"

declare -a launch_pid reader_pid
for i in 1 2; do
  mkdir -p "$root/proj.$i"
  if ! (trap - EXIT; cd "$root/proj.$i" && exec env -i "${CHILD_ENV[@]}" git init -q) \
      </dev/null >/dev/null 2>&1; then
    die "git init failed in the project for launch $i"
  fi
  mkfifo "$root/out.$i"
  : > "$root/log.$i"

  # The reader keeps the token FIFO open and closes the lock descriptor.
  (
    trap - EXIT
    set +e
    seen=0
    while IFS= read -r line || [ -n "$line" ]; do
      printf '%s\n' "$line" >> "$root/log.$i"
      if [ "$seen" = 0 ] && [[ "$line" == *"waiting for another amplihack install to finish"* ]]; then
        seen=1
        echo waiting >&"$tok"
      fi
    done
    if [ "$seen" = 0 ]; then
      echo exited >&"$tok"
    fi
  ) <"$root/out.$i" {lockfd}>&- &
  reader_pid[i]=$!
  pending+=("$!")

  # The launch closes both descriptors: it must not share the scenario's lock,
  # and its end of file must not depend on the token FIFO. The command string
  # is constant; the child shell reads the binary from the environment.
  # shellcheck disable=SC2016
  (
    trap - EXIT
    cd "$root/proj.$i" &&
      exec env -i "${CHILD_ENV[@]}" script -qec 'exec "$AMPLIHACK_1582_BIN" claude' /dev/null
  ) </dev/null >"$root/out.$i" 2>&1 {lockfd}>&- {tok}>&- &
  launch_pid[i]=$!
  pending+=("$!")
done
echo "started launches ${launch_pid[1]} and ${launch_pid[2]} while holding the install lock"

show_logs() {
  local i
  for i in 1 2; do
    echo "  --- launch $i output ---"
    sed 's/^/    /' "$root/log.$i"
  done
}

tokens=""
for _ in 1 2; do
  read -r -u "$tok" token || die "the token FIFO closed"
  tokens="$tokens $token"
done
if [ "$tokens" != " waiting waiting" ]; then
  echo "tokens:$tokens"
  show_logs
  die "a launch ended without waiting for the install lock, so it never decided to re-stage"
fi
echo "both launches are waiting for the install lock"

# --- 4. release the lock and wait for both launches --------------------------
flock -u "$lockfd"
exec {lockfd}>&-
lock_held=0

reap "${launch_pid[1]}"
status_1=$reaped_status
reap "${launch_pid[2]}"
status_2=$reaped_status
reap "${reader_pid[1]}"
reap "${reader_pid[2]}"
exec {tok}>&-

# --- assertions -------------------------------------------------------------
fail=0
pass() { echo "PASS: $1 - $2"; }
flunk() { echo "FAIL: $1 - $2"; fail=1; }

if [ "$status_1" = 0 ] && [ "$status_2" = 0 ]; then
  pass both-launches-exit-0 "both launches exited 0"
else
  flunk both-launches-exit-0 "launch exit statuses were $status_1 and $status_2"
fi

list_releases > "$root/releases.after" || die "cannot list $releases"
new_releases="$(LC_ALL=C comm -13 "$root/releases.installed" "$root/releases.after" | wc -l)"
if [ "$new_releases" -eq 1 ]; then
  pass one-new-release "one release directory was created after the install"
else
  flunk one-new-release "$new_releases release directories were created after the install, expected 1"
  sed 's/^/    after: /' "$root/releases.after"
fi

bootstraps="$({ grep -h 'Bootstrapping amplihack framework assets' "$root/log.1" "$root/log.2" || true; } | wc -l)"
if [ "$bootstraps" -eq 1 ]; then
  pass one-bootstrap-line "one launch re-staged and the other found nothing to do"
else
  flunk one-bootstrap-line "$bootstraps launches printed the re-stage line, expected 1"
fi

list_files > "$root/files.after" || die "cannot list $claude_dir"
missing="$(LC_ALL=C comm -23 "$root/files.installed" "$root/files.after")"
if [ -z "$missing" ]; then
  pass assets-complete "every path staged by the install exists again"
else
  flunk assets-complete "$(printf '%s\n' "$missing" | wc -l) staged path(s) are missing"
  printf '%s\n' "$missing" | head -20 | sed 's/^/    missing: /'
fi

if [ "$fail" -ne 0 ]; then
  show_logs
  echo "SCENARIO_FAILED"
  exit 1
fi
echo "issue 1582 concurrent launch contract green"
echo "ALL_CASES_PASSED"
