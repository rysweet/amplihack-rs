#!/usr/bin/env bash
# Issue #3156: acceptance test for the limit-resume hook on the process layout
# found on a factory host, run with real processes on a real terminal.
#
# The Rust stand-in tests inject the process tree the pane check reads, so
# they never read /proc. That is how #3156 stayed hidden: every session in
# Claude Code's timed limit wait was logged as pane-not-reading. This script
# runs the given amplihack-hooks binary unchanged, on a new pseudo-terminal
# made by util-linux `script`, under real processes laid out as on the host:
#
#   script            makes the terminal and its session
#   └ launcher        bash, session leader, the terminal's foreground group
#     └ claude        the stand-in for Claude Code: the terminal on stdin,
#       │             in the launcher's group, as all 18 interactive
#       │             sessions on the host were on 24 Nov 2026
#       ├ wrapper     /bin/sh -c, in a session of its own with no terminal
#       │ │           and a pipe on stdin, as Claude Code runs a hook command
#       │ └ hook      amplihack-hooks limit-resume, the binary under test
#       └ agent       (some cases) a background agent: a session of its own,
#                     no terminal, stdin /dev/null, running throughout
#
# The hook reads the real /proc, the real pidfd of its parent and real files.
# Only tmux is a stand-in: a shell script, first and alone on the hook's PATH,
# that records each run and answers from fixtures. `display-message` prints
# the terminal the claude stand-in holds (or another one), `capture-pane`
# prints the screen fixture and `send-keys` is recorded and goes nowhere. On
# `display-message` it also writes the real ancestry of the hook to `layout`,
# from /proc stat fields. TMUX names a socket path inside the sandbox, so no
# tmux server is ever reached, and no key reaches any terminal.
#
# The rotator is a stand-in too: a process whose command line holds
# `profile rotate watch`, named by the sandbox's rotate.pid. The swap record
# is on disk before the hook starts, dated after the session's stop (the
# transcript's last entry), so the hook judges it at its first read and never
# waits. Nothing here sleeps, polls or retries.
#
# Cases (expected outcome for a fixed binary):
#   timed-wait            the host layout, Claude Code's timed wait screen:
#                         resumed-keys, the four keys, exit 0
#   timed-wait-with-agent the same with a background agent running:
#                         resumed-keys, the four keys, exit 0
#   foreign-pane          tmux names another terminal for the pane:
#                         pane-unverified tty-mismatch, no key, exit 2
#   unrecognised-screen   the session's own pane shows a shell prompt:
#                         screen-unrecognised screen-unknown, no key, exit 2
#   session-in-background the claude stand-in is a background job of the
#                         launcher, so the launcher reads the terminal:
#                         pane-unverified not-foreground, no key, exit 2
#
# Usage: tests/qa/limit-resume-live-layout.sh <amplihack-hooks binary>
#
# Prints the facts of each case, the layout the hook ran under, and a PASS or
# FAIL line per check; exits 0 only when every check passed. Every process it
# starts ends by itself; the exit trap stops only the PIDs it recorded.
# Needs util-linux `script` and `setsid`, `mkfifo` and GNU `date -d` (Linux).
# Runs locally and is not part of CI.

set -uo pipefail
umask 077

[ $# -eq 1 ] || { echo "usage: $0 <amplihack-hooks binary>" >&2; exit 2; }
HOOK=$1
[ -f "$HOOK" ] && [ -x "$HOOK" ] || { echo "FAIL: $HOOK is not an executable file" >&2; exit 2; }
HOOK=$(cd "$(dirname "$HOOK")" && pwd -P)/$(basename "$HOOK")
for tool in script setsid mkfifo date readlink env; do
  command -v "$tool" >/dev/null || { echo "FAIL: $tool is required" >&2; exit 2; }
done

BASE=$(mktemp -d "${TMPDIR:-/tmp}/limit-resume-live-layout.XXXXXX")
case $BASE in
  *[\'\"\\\ ]* | *$'\n'*) echo "FAIL: the sandbox path must not need quoting: $BASE" >&2; exit 2 ;;
esac
echo "sandbox: $BASE"
echo "binary:  $HOOK"

FAILS=0
pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*"; FAILS=$((FAILS + 1)); }

# ── The rotator stand-in, shared by every case ──────────────────────────────

ROTATOR=
stop_recorded() {
  # A wrapper still running means its hook is still waiting: stop that
  # wrapper, by the PID it recorded, so the hook ends as session-gone.
  local f pid
  for f in "$BASE"/*/wrapper.pid; do
    [ -f "$f" ] || continue
    pid=$(<"$f")
    [ -f "$(dirname "$f")/hook.exit" ] && continue
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then kill "$pid"; fi
  done
  if [ -n "$ROTATOR" ]; then
    printf 'stop\n' >&5
    wait "$ROTATOR"
    ROTATOR=
  fi
}
trap stop_recorded EXIT
# A signal from the runner's time limit ends the script through the exit trap.
trap 'exit 143' TERM INT HUP

mkfifo "$BASE/rotator.fifo" "$BASE/rotator.ready"
exec 5<>"$BASE/rotator.fifo" 6<>"$BASE/rotator.ready"
sh -c 'printf "ready\n" >&4; exec 4>&-; read -r _ <&3' sh profile rotate watch \
  3<&5 4>&6 5>&- 6>&- </dev/null >/dev/null 2>&1 &
ROTATOR=$!
# The stand-in says ready once it runs as `sh ... profile rotate watch`.
read -r _ <&6

# ── The claude stand-in ─────────────────────────────────────────────────────

cat > "$BASE/claude-standin.sh" <<'CLAUDE'
#!/usr/bin/env bash
# Stands in for Claude Code in its timed limit wait: the terminal on stdin,
# in the launcher's process group. Runs the hook command the way Claude Code
# does: a /bin/sh -c wrapper in a session of its own, the StopFailure payload
# on a pipe.
set -u
W=$1 AGENT=$2 PANE_TTY=$3 HOOK=$4
# The terminal this session holds, and the one tmux names for the pane.
tty > "$W/session-tty"
if [ "$PANE_TTY" = own ]; then
  cp "$W/session-tty" "$W/tty"
else
  printf '/dev/pts/999999\n' > "$W/tty"
fi
agent=
if [ "$AGENT" = 1 ]; then
  mkfifo "$W/agent.fifo" "$W/agent.ready"
  exec 7<>"$W/agent.fifo" 8<>"$W/agent.ready"
  # The agent waits on its own open of the FIFO, so its stdin stays
  # /dev/null throughout (a shell `read <&3` would move the FIFO onto it).
  setsid sh -c 'printf "ready\n" >&4; exec 4>&-; exec head -n 1 /dev/fd/3' agent \
    3<&7 4>&8 7<&- 8<&- </dev/null >/dev/null 2>&1 &
  agent=$!
  # Ready means the agent runs after setsid, in a session of its own.
  read -r _ <&8
  printf '%s\n' "$agent" > "$W/agent.pid"
  read -r line < "/proc/$agent/stat"
  rest=${line##*) }
  read -r _st ppid pgrp sid _tty tpgid _ <<< "$rest"
  printf 'role=agent pid=%s ppid=%s pgrp=%s sid=%s tpgid=%s stdin=%s\n' \
    "$agent" "$ppid" "$pgrp" "$sid" "$tpgid" "$(readlink "/proc/$agent/fd/0")" >> "$W/agent-layout"
fi
# The payload reaches the hook through a pipe, as Claude Code writes it. With
# no job control here, the wrapper is not a group leader, so setsid gives it
# a session of its own without forking.
cat "$W/payload.json" |
  env -i PATH="$W/bin" HOME="$W/home" AMPLIHACK_CLAUDE_PROFILE_ROOT="$W/profile" \
    TMUX="$W/tmux-standin.sock,4242,0" TMUX_PANE=%5 \
    /usr/bin/setsid /bin/sh -c 'printf "%s\n" "$$" > "$1/wrapper.pid"; "$0" limit-resume; exit $?' \
    "$HOOK" "$W" > "$W/hook.stdout" 2> "$W/hook.stderr" 7>&- 8>&-
printf '%s\n' "${PIPESTATUS[1]}" > "$W/hook.exit"
if [ -n "$agent" ]; then
  # The agent ran throughout the hook; it ends once told to.
  printf 'done\n' >&7
  wait "$agent"
  exec 7>&- 8>&-
fi
exit 0
CLAUDE

# ── One case ────────────────────────────────────────────────────────────────

TIMED_WAIT_SCREEN='Usage limit reached · Continuing automatically at 1:50am'
SHELL_SCREEN='ryan@candide:~/src/amplihack-rs$ '

write_standin_tmux() {
  local w=$1
  mkdir -m 700 "$w/bin"
  cat > "$w/bin/tmux" <<STANDIN
#!/bin/sh
# amplihack-limit-resume-qa-standin-tmux (issue #3156). Shell built-ins only,
# except /usr/bin/readlink for the layout record: PATH is this directory.
{ for a in "\$@"; do printf '[%s]' "\$a"; done; printf '\n'; } >> '$w/record'
cmd=\$1
case "\$cmd" in
  display-message)
    pid=\$PPID; n=0
    while [ "\$n" -lt 8 ] && [ "\$pid" -gt 1 ]; do
      IFS= read -r line < "/proc/\$pid/stat" || break
      comm=\${line#*(}; comm=\${comm%%)*}
      rest=\${line##*) }
      set -- \$rest
      fd0=\$(/usr/bin/readlink "/proc/\$pid/fd/0" 2>/dev/null)
      printf 'pid=%s comm=%s ppid=%s pgrp=%s sid=%s tpgid=%s stdin=%s\n' \\
        "\$pid" "\$comm" "\$2" "\$3" "\$4" "\$6" "\${fd0:-?}"
      [ "\$comm" = script ] && break
      pid=\$2; n=\$((n + 1))
    done >> '$w/layout'
    f='$w/tty' ;;
  capture-pane) f='$w/screen' ;;
  *) f= ;;
esac
if [ -n "\$f" ] && [ -f "\$f" ]; then
  while IFS= read -r l; do printf '%s\n' "\$l"; done < "\$f"
fi
exit 0
STANDIN
  chmod 700 "$w/bin/tmux"
}

# run_case NAME SCREEN PANE_TTY(own|foreign) AGENT(0|1) LAUNCH(fg|bg)
run_case() {
  local name=$1 screen=$2 pane_tty=$3 agent=$4 launch=$5
  local w="$BASE/$name" now rotate cmd
  mkdir -m 700 "$w" "$w/home" "$w/profile" "$w/profile/.auto-switch.d"
  rotate="$w/profile/.auto-switch.d/rotate"
  mkdir -m 700 "$rotate"
  printf 'enabled=on\n' > "$w/profile/.auto-switch"
  printf '%s\n' "$ROTATOR" > "$rotate/rotate.pid"
  write_standin_tmux "$w"
  printf '%s\n' "$screen" > "$w/screen"

  now=$(date +%s)
  # The session stopped two minutes ago; the rotator swapped a minute ago
  # onto a login with room. Written as the rotator writes it: private, then
  # renamed into place.
  printf '{"type":"assistant","timestamp":"%s"}\n' \
    "$(date -u -d "@$((now - 120))" +%Y-%m-%dT%H:%M:%SZ)" > "$w/transcript.jsonl"
  printf '{"seq":1,"live_login":"spare","from_login":"work","swapped_at":%s,"reading":{"session":12,"week":40,"measured_at":%s}}\n' \
    "$((now - 60))" "$((now - 90))" > "$rotate/.swap.json.tmp"
  mv "$rotate/.swap.json.tmp" "$rotate/swap.json"

  printf '{"hook_event_name":"StopFailure","session_id":"qa3156-%s","error":"rate_limit","last_assistant_message":"You have hit your session limit","transcript_path":"%s"}' \
    "$name" "$w/transcript.jsonl" > "$w/payload.json"

  cmd="bash '$BASE/claude-standin.sh' '$w' '$agent' '$pane_tty' '$HOOK'"
  if [ "$launch" = bg ]; then
    # Job control on: the claude stand-in runs as a background job, in a
    # group of its own, while the launcher keeps the terminal.
    cmd="set -m; $cmd & wait \$!; exit \$?"
  else
    cmd="$cmd; exit \$?"
  fi
  env -i HOME="$w/home" PATH=/usr/bin:/bin SHELL=/bin/bash TERM=dumb \
    script -q -e -c "$cmd" /dev/null </dev/null >"$w/pty.out" 2>&1 5>&- 6>&-
  echo "$?" > "$w/script.exit"
}

# ── Checks ──────────────────────────────────────────────────────────────────

outcome_line() { grep ' outcome=' "$1/profile/.auto-switch.d/resume/resume.log" 2>/dev/null; }
# The value of `key=` in a line of space-separated fields, at its start or
# after a space.
field() { sed -n "s/^\\(.* \\)\\{0,1\\}$2=\\([^ ]*\\).*/\\2/p" <<< "$1"; }
count_files() { local d=$1; [ -d "$d" ] && find "$d" -mindepth 1 -maxdepth 1 | wc -l || echo 0; }

report_case() {
  local name=$1 w="$BASE/$1"
  echo
  echo "== case: $name"
  echo "script exit: $(cat "$w/script.exit" 2>/dev/null)"
  echo "hook exit:   $(cat "$w/hook.exit" 2>/dev/null || echo none)"
  echo "log:         $(outcome_line "$w" || echo none)"
  echo "tmux runs:"
  sed 's/^/  /' "$w/record" 2>/dev/null || echo "  none"
  echo "layout at the pane proof (hook first, from /proc):"
  sed 's/^/  /' "$w/layout" 2>/dev/null || echo "  none"
  [ -f "$w/agent-layout" ] && sed 's/^/  /' "$w/agent-layout"
  echo "hook stdout bytes: $(wc -c < "$w/hook.stdout" 2>/dev/null || echo none)"
  echo "hook stderr bytes: $(wc -c < "$w/hook.stderr" 2>/dev/null || echo none)"
}

# The host layout must really be the one under test, whatever the outcome.
check_layout() {
  local name=$1 launch=$2 w="$BASE/$1" hook wrapper claude launcher
  hook=$(sed -n 1p "$w/layout" 2>/dev/null)
  wrapper=$(sed -n 2p "$w/layout" 2>/dev/null)
  claude=$(sed -n 3p "$w/layout" 2>/dev/null)
  launcher=$(sed -n 4p "$w/layout" 2>/dev/null)
  if [[ $hook == *comm=amplihack-hooks* && $hook == *stdin=pipe:* ]]; then
    pass "$name: the hook ran with a pipe on stdin"
  else fail "$name: the hook's own entry is not as expected: $hook"; fi
  if [[ $wrapper == *comm=sh* && $wrapper == *stdin=pipe:* && $wrapper == *tpgid=-1* ]] &&
     [ "$(field "$wrapper" pgrp)" = "$(field "$wrapper" pid)" ] &&
     [ "$(field "$wrapper" sid)" = "$(field "$wrapper" pid)" ]; then
    pass "$name: the hook's parent is a wrapper in a session of its own, no terminal"
  else fail "$name: the wrapper is not as on the host: $wrapper"; fi
  if [[ $claude == *comm=bash* && $claude == *stdin=/dev/pts/* ]] &&
     [ "$(field "$claude" stdin)" = "$(cat "$w/session-tty")" ] &&
     [ "$(field "$launcher" pgrp)" = "$(field "$launcher" tpgid)" ]; then
    pass "$name: the claude stand-in holds the pane's terminal; the launcher is foreground"
  else fail "$name: the claude stand-in or launcher is not as on the host: $claude / $launcher"; fi
  if [ "$launch" = fg ]; then
    if [ "$(field "$claude" pgrp)" = "$(field "$launcher" pgrp)" ] &&
       [ "$(field "$claude" pgrp)" = "$(field "$claude" tpgid)" ]; then
      pass "$name: the claude stand-in is in the launcher's group, the foreground group"
    else fail "$name: the claude stand-in is not in the foreground group: $claude"; fi
    if [ "$(field "$wrapper" pgrp)" != "$(field "$claude" tpgid)" ]; then
      pass "$name: the wrapper's group is not the terminal's foreground group (the #3156 layout)"
    else fail "$name: the wrapper's group is the foreground group, so this is not the #3156 layout"; fi
  else
    if [ "$(field "$claude" pgrp)" != "$(field "$claude" tpgid)" ]; then
      pass "$name: the claude stand-in is a background group of its terminal"
    else fail "$name: the claude stand-in is in the foreground: $claude"; fi
  fi
}

check_resumed() {
  local name=$1 w="$BASE/$1" line expected
  line=$(outcome_line "$w")
  [ "$(cat "$w/hook.exit" 2>/dev/null)" = 0 ] && pass "$name: the hook exited 0" ||
    fail "$name: the hook exited $(cat "$w/hook.exit" 2>/dev/null || echo none), not 0"
  [ "$(field "$line" outcome)" = resumed-keys ] && pass "$name: outcome=resumed-keys" ||
    fail "$name: outcome=$(field "$line" outcome), not resumed-keys"
  [ "$(field "$line" keys_sent)" = 4 ] && pass "$name: keys_sent=4" ||
    fail "$name: keys_sent=$(field "$line" keys_sent), not 4"
  expected=$'[display-message][-p][-t][%5][#{pane_tty}]\n[capture-pane][-p][-t][%5]\n[send-keys][-t][%5][Escape]\n[send-keys][-t][%5][C-u]\n[send-keys][-t][%5][-l][continue]\n[send-keys][-t][%5][Enter]'
  [ "$(cat "$w/record" 2>/dev/null)" = "$expected" ] &&
    pass "$name: proved the pane, read the screen, then Escape, C-u, continue, Enter to %5 only" ||
    fail "$name: the tmux runs were not the proof, the read and the four keys"
  [ "$(count_files "$w/profile/.auto-switch.d/resume/events")" = 1 ] &&
    pass "$name: the resume is recorded once" || fail "$name: the resume is not recorded once"
  [ ! -s "$w/hook.stdout" ] && [ ! -s "$w/hook.stderr" ] &&
    pass "$name: nothing on stdout or stderr" || fail "$name: the hook wrote to stdout or stderr"
}

check_refused() {
  local name=$1 outcome=$2 detail=$3 runs=$4 w="$BASE/$1" line
  line=$(outcome_line "$w")
  [ "$(cat "$w/hook.exit" 2>/dev/null)" = 2 ] && pass "$name: the hook exited 2 (the wake)" ||
    fail "$name: the hook exited $(cat "$w/hook.exit" 2>/dev/null || echo none), not 2"
  [ "$(field "$line" outcome)" = "$outcome" ] && [ "$(field "$line" detail)" = "$detail" ] &&
    pass "$name: outcome=$outcome detail=$detail" ||
    fail "$name: outcome=$(field "$line" outcome) detail=$(field "$line" detail), not $outcome $detail"
  ! grep -q '^\[send-keys\]' "$w/record" 2>/dev/null && pass "$name: no key was sent" ||
    fail "$name: a key was sent"
  [ "$(grep -c . "$w/record" 2>/dev/null)" = "$runs" ] && pass "$name: tmux ran $runs time(s)" ||
    fail "$name: tmux ran $(grep -c . "$w/record" 2>/dev/null) times, not $runs"
  [ "$(count_files "$w/profile/.auto-switch.d/resume/events")" = 0 ] &&
    pass "$name: nothing is recorded, so the event stays armed" || fail "$name: a resume was recorded"
  [ -s "$w/hook.stderr" ] && pass "$name: the wake message is on stderr" ||
    fail "$name: no wake message on stderr"
}

# ── The cases ───────────────────────────────────────────────────────────────

run_case timed-wait "$TIMED_WAIT_SCREEN" own 0 fg
run_case timed-wait-with-agent "$TIMED_WAIT_SCREEN" own 1 fg
run_case foreign-pane "$TIMED_WAIT_SCREEN" foreign 0 fg
run_case unrecognised-screen "$SHELL_SCREEN" own 0 fg
run_case session-in-background "$TIMED_WAIT_SCREEN" own 0 bg

for c in timed-wait timed-wait-with-agent foreign-pane unrecognised-screen session-in-background; do
  report_case "$c"
done
echo

check_layout timed-wait fg
check_resumed timed-wait

check_layout timed-wait-with-agent fg
check_resumed timed-wait-with-agent
agent_line=$(cat "$BASE/timed-wait-with-agent/agent-layout" 2>/dev/null)
claude_pid=$(field "$(sed -n 3p "$BASE/timed-wait-with-agent/layout" 2>/dev/null)" pid)
if [ -n "$agent_line" ] && [ "$(field "$agent_line" ppid)" = "$claude_pid" ] &&
   [ "$(field "$agent_line" sid)" = "$(field "$agent_line" pid)" ] &&
   [ "$(field "$agent_line" tpgid)" = -1 ] && [ "$(field "$agent_line" stdin)" = /dev/null ]; then
  pass "timed-wait-with-agent: a background agent under the claude stand-in ran in a session of its own"
else
  fail "timed-wait-with-agent: the background agent is not as on the host: $agent_line"
fi

check_layout foreign-pane fg
if [ "$(cat "$BASE/foreign-pane/tty")" != "$(cat "$BASE/foreign-pane/session-tty")" ]; then
  pass "foreign-pane: tmux named a terminal other than the session's"
else
  fail "foreign-pane: tmux named the session's own terminal"
fi
check_refused foreign-pane pane-unverified tty-mismatch 1

check_layout unrecognised-screen fg
check_refused unrecognised-screen screen-unrecognised screen-unknown 2

check_layout session-in-background bg
check_refused session-in-background pane-unverified not-foreground 1

echo
if [ "$FAILS" -eq 0 ]; then
  echo "RESULT: PASS (every check passed)"
  exit 0
fi
echo "RESULT: FAIL ($FAILS check(s) failed)"
exit 1
