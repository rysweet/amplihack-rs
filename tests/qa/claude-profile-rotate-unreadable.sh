#!/usr/bin/env bash
# Issue #2820: acceptance test for the rotator when it cannot read every
# process, and when the live login is spend-blocked.
#
# The candidate amplihack binary runs against a temporary HOME, a fake
# `claude`, a loopback stand-in for the usage and profile endpoints (`socat`,
# bound to 127.0.0.1 only), and a stand-in process table named by
# AMPLIHACK_TEST_PROC_ROOT: a 0700 folder under the run's own TMPDIR, whose
# processes carry this user's uid, pids above pid_max, and, for a process that
# is not dumpable, an `environ` that is a directory, so it cannot be read. No
# real login, no real /proc, no network, no CLAUDE_CONFIG_DIR, no real profile
# root. No real non-dumpable process is started.
#
# It replays 19 and 23 November 2026: a Tailscale SSH child whose remote
# command mentions amplihack made every login count as live, and the rotator
# stayed on a spend-blocked live login while other logins had room. Cases:
#
#   1. at the deployed --session-at 95 --week-at 98: the unreadable
#      `tailscaled`, an unreadable `claude` naming `busy`, an unreadable
#      `amplihack` naming no profile, a readable `claude` using `held`, and
#      an unreadable `claude` naming no profile whose session file Claude
#      Code wrote under `xxavier` (`sessions/<pid>.json`). The one-shot
#      cycle names each unreadable process once, skips only `busy`, `held`
#      and `xxavier`, and moves the spend-blocked live login to `wwest`;
#   2. the same at 85/90;
#   3. no secret reaches the log: an assignment and a `--token` value in an
#      unreadable process's arguments are masked, and no process environment
#      is logged;
#   4. HTTP 429 for every candidate, with a last good reading of `wwest`: the
#      spend-blocked live login still moves, `unconfirmed (HTTP 429)`, and the
#      swap line says why the live login has no room and until when;
#   5. the dry run names the same move and moves nothing;
#   6. a waiting step wakes on the move. A real supervised step
#      (`amplihack claude --subprocess-safe -p`, with a stand-in `claude`
#      that meets a usage limit four hours from its reset on its first call)
#      parks in its usage-limit wait on the blocked live login. Once its
#      `start` line is written, the one-shot rotator cycle of cases 1 and 2
#      moves the login, and the wait ends `rotator capacity`, at 95/98 and
#      at 85/90; the retry runs on `wwest`. When the move is made on a last
#      good reading (case 4), the wait reads the move and stays, because no
#      room was read (#2857). The next cycle, which reads `wwest` with room,
#      ends it. The driver reacts to the step's own lines, read from a pipe;
#      it adds no sleep or timer.
#
# Needs socat and GNU `date -d` (Linux). Runs locally and is not part of CI.
#
# Usage: AMPLIHACK_QA_BINARY=target/debug/amplihack bash tests/qa/claude-profile-rotate-unreadable.sh
set -euo pipefail

fail() { echo "FAIL: $*" >&2; exit 1; }

if [[ -z "${AMPLIHACK_QA_BINARY:-}" || ! -x "${AMPLIHACK_QA_BINARY:-}" ]]; then
  fail "AMPLIHACK_QA_BINARY must name an executable candidate amplihack binary"
fi
command -v socat >/dev/null 2>&1 \
  || fail "socat is required for the usage stand-in (apt install socat); this driver cannot run without it"
BIN="$(cd "$(dirname "$AMPLIHACK_QA_BINARY")" && pwd)/$(basename "$AMPLIHACK_QA_BINARY")"

SOCAT_PID=""
DRAIN_PID=""
STEP_PID=""
WORK=""
cleanup() {
  [[ -z "$STEP_PID" ]] || kill "$STEP_PID" 2>/dev/null || true
  [[ -z "$DRAIN_PID" ]] || kill "$DRAIN_PID" 2>/dev/null || true
  [[ -z "$SOCAT_PID" ]] || kill "$SOCAT_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  if [[ -n "$WORK" && "$WORK" == "${TMPDIR:-/tmp}"/rotate-unreadable-qa.* && -d "$WORK" ]]; then
    rm -rf -- "$WORK"
  fi
}
trap cleanup EXIT
WORK="$(mktemp -d "${TMPDIR:-/tmp}/rotate-unreadable-qa.XXXXXX")"
[[ "$WORK" =~ ^[A-Za-z0-9._/-]+$ ]] || fail "unsuitable scratch folder name: $WORK"

FX="$WORK/fx"
QA_BIN_DIR="$WORK/bin"
mkdir -p "$FX/usage" "$FX/profile" "$QA_BIN_DIR" "$WORK/tmp"
chmod 700 "$WORK/tmp"

# --- The fake claude -----------------------------------------------------------
cat > "$QA_BIN_DIR/claude" <<CLAUDE
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then echo "2.1.319 (Claude Code)"; exit 0; fi
printf '%s\n' "\$*" >> "$WORK/claude-calls"
exit 1
CLAUDE
chmod +x "$QA_BIN_DIR/claude"

# --- The stand-in endpoints (as in claude-profile-rotate-room.sh) ----------------------
cat > "$WORK/handler.sh" <<'HANDLER'
#!/usr/bin/env bash
set -u
FIX="${QA_FIXTURE:?}"
reply() { # status text, body
  printf 'HTTP/1.1 %s\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' \
    "$1" "${#2}" "$2"
}
IFS= read -r -t 10 -n 2048 request || exit 0
request=${request%$'\r'}
auth=""
count=0
while IFS= read -r -t 10 -n 4096 header; do
  header=${header%$'\r'}
  [[ -n "$header" ]] || break
  lower=$(printf '%s' "${header%%:*}" | tr '[:upper:]' '[:lower:]')
  case "$lower" in authorization) auth=${header#*: } ;; esac
  count=$((count + 1))
  [[ $count -lt 64 ]] || exit 0
done
read -r method path _ <<< "$request"
path=${path%%\?*}
token=${auth#Bearer }
if [[ ! "$token" =~ ^qa-([a-z0-9][a-z0-9-]{0,63})$ ]]; then
  reply "401 Unauthorized" '{"error":"unauthorized"}'; exit 0
fi
name=${BASH_REMATCH[1]}
[[ "$method" == "GET" ]] || { reply "405 Method Not Allowed" '{}'; exit 0; }
case "$path" in
  /api/oauth/usage)
    if [[ "$(cat "$FIX/usage-mode" 2>/dev/null)" == "429" ]] && ! grep -qx -- "$name" "$FIX/usage-ok" 2>/dev/null; then
      reply "429 Too Many Requests" '{"error":"rate_limited"}'; exit 0
    fi
    [[ -f "$FIX/usage/$name.json" ]] || { reply "404 Not Found" '{}'; exit 0; }
    reply "200 OK" "$(cat "$FIX/usage/$name.json")" ;;
  /api/oauth/profile)
    [[ -f "$FIX/profile/$name.json" ]] || { reply "404 Not Found" '{}'; exit 0; }
    reply "200 OK" "$(cat "$FIX/profile/$name.json")" ;;
  *) reply "404 Not Found" '{}' ;;
esac
HANDLER
chmod +x "$WORK/handler.sh"

mkfifo "$WORK/socat.fifo"
QA_FIXTURE="$FX" socat -d -d -T 10 \
  "TCP-LISTEN:0,bind=127.0.0.1,reuseaddr,fork,max-children=8,range=127.0.0.1/32" \
  "EXEC:bash $WORK/handler.sh" 2> "$WORK/socat.fifo" &
SOCAT_PID=$!
exec 3< "$WORK/socat.fifo"
PORT=""
while IFS= read -r -u 3 line; do
  if [[ "$line" =~ listening\ on\ AF=2\ 127\.0\.0\.1:([0-9]+) ]]; then
    PORT=${BASH_REMATCH[1]}
    break
  fi
done
[[ -n "$PORT" ]] || fail "the stand-in listener never reported a port"
cat <&3 >> "$WORK/socat.log" &
DRAIN_PID=$!
exec 3<&-
echo "stand-in listening on 127.0.0.1:$PORT"

# --- The fleet -----------------------------------------------------------------
# The live login is ggrant at 94% / 75%, as on 23 November at 11:00Z, with a
# spend block written in each case. `busy` has the most room but an unreadable
# `claude` names it; `held` is used by a readable `claude`. Every login is in
# one organisation, so no move is a crossing.
LIVE=ggrant
FLEET=("busy 5 5" "held 10 10" "wwest 30 40" "xxavier 50 50")
ALL=("$LIVE")
for row in "${FLEET[@]}"; do read -r n _ <<< "$row"; ALL+=("$n"); done
RESET_SESSION="2026-11-23T15:00:00+00:00"
RESET_WEEK="2026-11-26T09:00:00+00:00"

write_usage() { # name session week
  printf '{"five_hour":{"utilization":%s,"resets_at":"%s"},"seven_day":{"utilization":%s,"resets_at":"%s"}}' \
    "$2" "$RESET_SESSION" "$3" "$RESET_WEEK" > "$FX/usage/$1.json"
}
write_profile() { # name
  printf '{"account":{"uuid":"u-%s","email":"%s@example.invalid"},"organization":{"uuid":"org-a"}}' \
    "$1" "$1" > "$FX/profile/$1.json"
}
write_login() { # directory name
  mkdir -p "$1"
  printf '{"claudeAiOauth":{"accessToken":"qa-%s","expiresAt":4102444800000}}' "$2" > "$1/.credentials.json"
  chmod 600 "$1/.credentials.json"
  printf '{"oauthAccount":{"accountUuid":"u-%s","emailAddress":"%s@example.invalid","organizationUuid":"org-a"}}' \
    "$2" "$2" > "$1/.claude.json"
  chmod 600 "$1/.claude.json"
}

write_usage "$LIVE" 94 75
write_profile "$LIVE"
write_profile start
for row in "${FLEET[@]}"; do
  read -r who s w <<< "$row"
  write_usage "$who" "$s" "$w"
  write_profile "$who"
done
echo ok > "$FX/usage-mode"

# --- The stand-in process table ----------------------------------------------------
# A fresh 0700 table under the run's TMPDIR for each case. It always holds
# `self/environ`, which the scan reads to see that the table can be used.
UIDN="$(id -u)"
PROCT=""
new_table() { # case name
  PROCT="$WORK/tmp/proc.$1"
  rm -rf -- "$PROCT"
  mkdir -p "$PROCT/self"
  chmod 700 "$PROCT"
  printf 'HOME=/stand-in\0' > "$PROCT/self/environ"
}
# One process: pid, status name, then `-` for an environment that cannot be
# read or the environment one entry per line (written NUL-separated, as the
# kernel shows it; a shell string cannot hold NUL), then the arguments.
proc_entry() { # pid name environ args...
  local pid=$1 name=$2 environ=$3 dir
  shift 3
  dir="$PROCT/$pid"
  mkdir -p "$dir"
  printf 'Name:\t%s\nState:\tS (sleeping)\nUid:\t%s\t%s\t%s\t%s\n' \
    "$name" "$UIDN" "$UIDN" "$UIDN" "$UIDN" > "$dir/status"
  printf '%s\0' "$@" > "$dir/cmdline"
  if [[ "$environ" == "-" ]]; then
    mkdir "$dir/environ"
  else
    printf '%s\n' "$environ" | tr '\n' '\0' > "$dir/environ"
  fi
}

# --- A home and a profile root for each case ------------------------------------------------
QA_HOME=""
ROOT=""
run_cli() { # args; stdout to $WORK/stdout, stderr to $WORK/stderr, status in $STATUS
  set +e
  (cd "$WORK" && env -i \
    HOME="$QA_HOME" \
    PATH="$QA_BIN_DIR:/usr/bin:/bin" \
    TERM=dumb \
    TMPDIR="$WORK/tmp" \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_TEST_HOME_GUARD=1 \
    AMPLIHACK_TEST_PROC_ROOT="$PROCT" \
    AMPLIHACK_PROFILE_USAGE_API_BASE="http://127.0.0.1:$PORT" \
    AMPLIHACK_PROFILE_USAGE_TOKEN_URL="http://127.0.0.1:$PORT/oauth/token" \
    AMPLIHACK_PROFILE_USAGE_PROBE=direct \
    "$BIN" "$@") > "$WORK/stdout" 2> "$WORK/stderr"
  STATUS=$?
  set -e
}

live_token() { sed -n 's/.*"accessToken":"\([^"]*\)".*/\1/p' "$QA_HOME/.claude/.credentials.json"; }

new_home() { # case name: a fresh HOME, every login stored, the live one in ~/.claude
  new_table "$1"
  QA_HOME="$WORK/home.$1"
  ROOT="$QA_HOME/.amplihack/claude-profiles"
  rm -rf -- "$QA_HOME"
  mkdir -p "$QA_HOME/.claude"
  chmod 700 "$QA_HOME" "$QA_HOME/.claude"
  write_login "$QA_HOME/.claude" start
  mv "$QA_HOME/.claude/.claude.json" "$QA_HOME/.claude.json"
  local who
  for who in "${ALL[@]}"; do
    run_cli profile add "$who"
    [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile add $who"; }
    write_login "$ROOT/$who" "$who"
  done
  for who in "${ALL[@]}"; do
    run_cli profile seed-record "$who" --write
    [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile seed-record $who"; }
  done
  run_cli profile switch "$LIVE"
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile switch $LIVE"; }
  run_cli profile switch --none
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile switch --none"; }
  run_cli profile remove default-original --yes
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile remove default-original"; }
  [[ "$(live_token)" == "qa-$LIVE" ]] \
    || fail "the setup left ~/.claude holding '$(live_token)', not qa-$LIVE"
  mkdir -p "$ROOT/.auto-switch.d"
  chmod 700 "$ROOT/.auto-switch.d"
}

# The processes of 19 and 23 November, in the current table.
nov23_processes() {
  proc_entry 4200001 tailscaled - \
    tailscaled be-child ssh --remote-ip=100.64.0.7 "amplihack recipe run wait"
  proc_entry 4200002 claude - \
    claude "CLAUDE_CONFIG_DIR=$ROOT/busy" "TOKEN=qa-sentinel-argv-secret"
  proc_entry 4200003 amplihack - amplihack recipe run
  proc_entry 4200004 claude \
    "SECRET=qa-sentinel-environ-secret"$'\n'"CLAUDE_CONFIG_DIR=$ROOT/held" \
    claude
  # Names no profile; only Claude Code's own session file ties it to xxavier.
  proc_entry 4200005 claude - \
    claude -p "fix it" --token qa-sentinel-flag-secret
  mkdir -p "$ROOT/xxavier/sessions"
  printf '{}' > "$ROOT/xxavier/sessions/4200005.json"
}

# The live login's spend block on 23 November, added to the rotate ledger the
# setup wrote: from a minute ago until four hours from now. Prints the end as
# the swap line shows it.
block_live() {
  local ledger="$ROOT/.auto-switch.d/rotate/ledger.json" now until
  [[ -f "$ledger" ]] || fail "the setup wrote no rotate ledger to add the block to"
  ! grep -q '"overflow_blocked"' "$ledger" || fail "the setup's ledger already holds a block"
  now=$(date +%s)
  until=$((now + 4 * 3600))
  sed -i "0,/{/s//{\"overflow_blocked\":{\"$LIVE\":{\"at\":$((now - 60)),\"until\":$until,\"reason\":\"spend_limit\"}},/" "$ledger"
  date -u -d "@$until" +%Y-%m-%dT%H:%MZ
}

rfc3339() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }

# One usage-history line for $1 with session $2 and week $3, taken $4 seconds ago.
write_history() { # name session week age
  local now at week_reset
  now=$(date +%s)
  at=$((now - $4))
  week_reset=$((now + 3 * 86400))
  printf '{"at":"%s","by":"rotate","profiles":{"%s":{"s":%s,"w":%s,"w_reset":"%s"}}}\n' \
    "$(rfc3339 "$at")" "$1" "$2" "$3" "$(rfc3339 "$week_reset")" \
    > "$ROOT/.auto-switch.d/usage-history.jsonl"
  chmod 600 "$ROOT/.auto-switch.d/usage-history.jsonl"
}

rotate_log() {
  local file="$ROOT/.auto-switch.d/rotate/rotate.log"
  [[ -f "$file" ]] || { echo "FAIL: the rotator wrote no rotate.log" >&2; return 1; }
  cat "$file"
}
count() { grep -cF -- "$1" <<< "$2" || true; }
evidence() { grep -E "$1" | sed 's/^/  | /' || true; }

DEPLOYED=(--session-at 95 --week-at 98 --margin 3 --upgrade-margin 30 --hold 3600)
REPORTER=(--session-at 85 --week-at 90 --margin 3 --upgrade-margin 30 --hold 3600)

# --- Cases 1 to 3: unreadable processes, a spend-blocked live login ------------------------
check_unreadable_case() { # label flags...
  local label=$1 log until
  shift
  new_home "unreadable-${label//\//-}"
  nov23_processes
  until="$(block_live)"
  run_cli profile rotate "$@"
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "$label: rotate exited $STATUS"; }
  log="$(rotate_log)"
  [[ "$(live_token)" == "qa-wwest" ]] \
    || { echo "$log" >&2; fail "$label: ~/.claude holds $(live_token), not qa-wwest"; }
  grep -q " swap $LIVE->wwest " <<< "$log" || { echo "$log" >&2; fail "$label: no swap line to wwest"; }
  ! grep -q "cannot see running sessions" <<< "$log" \
    || { echo "$log" >&2; fail "$label: the rotator said it cannot see running sessions"; }
  ! grep -q "not read (session running)" <<< "$log" \
    || { echo "$log" >&2; fail "$label: logins were counted as not read (session running)"; }
  [[ "$(count "skip: busy (session running)" "$log")" == "1" ]] \
    || { echo "$log" >&2; fail "$label: busy, named by the unreadable claude, was not skipped once"; }
  [[ "$(count "skip: held (session running)" "$log")" == "1" ]] \
    || { echo "$log" >&2; fail "$label: held, used by a readable claude, was not skipped once"; }
  [[ "$(count "skip: xxavier (session running)" "$log")" == "1" ]] \
    || { echo "$log" >&2; fail "$label: xxavier, tied by its session file to the unreadable claude, was not skipped once"; }
  ! grep -q "skip: wwest " <<< "$log" || { echo "$log" >&2; fail "$label: wwest was skipped"; }
  local want
  for want in \
    'unreadable process 4200001 (tailscaled) "tailscaled be-child ssh --remote-ip=100.64.0.7 amplihack recipe run wait": not counted (not a Claude executable)' \
    'unreadable process 4200002 (claude) "claude CLAUDE_CONFIG_DIR=<hidden> TOKEN=<hidden>": counted as a session on busy' \
    'unreadable process 4200003 (amplihack) "amplihack recipe run": not counted (profile cannot be told)' \
    'unreadable process 4200005 (claude) "claude -p fix it --token <hidden>": counted as a session on xxavier'; do
    [[ "$(count "$want" "$log")" == "1" ]] \
      || { echo "$log" >&2; fail "$label: expected exactly one line: $want"; }
  done
  [[ "$(count "unreadable process " "$log")" == "4" ]] \
    || { echo "$log" >&2; fail "$label: expected four unreadable process lines"; }
  # Case 3: no secret, no environment and no profile path in the log.
  local secret
  for secret in qa-sentinel-argv-secret qa-sentinel-flag-secret qa-sentinel-environ-secret "$ROOT/busy" "$ROOT/held"; do
    ! grep -qF -- "$secret" <<< "$log" || fail "$label: the log holds '$secret'"
    ! grep -qF -- "$secret" "$WORK/stdout" "$WORK/stderr" || fail "$label: the output holds '$secret'"
  done
  ! grep -q " ALERT: " <<< "$log" || { echo "$log" >&2; fail "$label: the cycle wrote an ALERT"; }
  echo "PASS: $label: each unreadable process is named once, only busy, held and xxavier are skipped, and the spend-blocked live login moves to wwest"
  evidence ' (unreadable process|skip:|swap) ' <<< "$log"
}
check_unreadable_case "case 1 (95/98)" "${DEPLOYED[@]}"
check_unreadable_case "case 2 (85/90)" "${REPORTER[@]}"
echo "PASS: case 3: no argument secret, environment or profile path reached the log or the output"

# --- Case 4: HTTP 429 for every candidate, a last good reading of wwest ------------------------
new_home h429
nov23_processes
until="$(block_live)"
echo 429 > "$FX/usage-mode"
printf '%s\n' "$LIVE" > "$FX/usage-ok"
write_history wwest 30 40 1800
run_cli profile rotate "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 4: rotate exited $STATUS"; }
log="$(rotate_log)"
[[ "$(live_token)" == "qa-wwest" ]] || { echo "$log" >&2; fail "case 4: ~/.claude holds $(live_token), not qa-wwest"; }
tail_text=" unconfirmed (HTTP 429); live $LIVE has no room: spend limit until $until"
grep -qF -- "$tail_text" <<< "$log" \
  || { echo "$log" >&2; fail "case 4: the swap line does not end '$tail_text'"; }
! grep -q "stay: no candidate confirmed by a fresh reading" <<< "$log" \
  || { echo "$log" >&2; fail "case 4: the rotator stayed on the blocked live login"; }
echo ok > "$FX/usage-mode"
: > "$FX/usage-ok"
echo "PASS: case 4: with every candidate unread, the spend-blocked live login moves on wwest's last good reading and says why"
evidence ' swap ' <<< "$log"

# --- Case 5: the dry run ------------------------------------------------------------------------
new_home dry
nov23_processes
block_live > /dev/null
before="$(live_token)"
run_cli profile rotate --dry-run "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 5: dry run exited $STATUS"; }
grep -q "would swap $LIVE->wwest " "$WORK/stdout" \
  || { cat "$WORK/stdout" >&2; fail "case 5: the dry run did not name the swap to wwest"; }
[[ "$(live_token)" == "$before" ]] || fail "case 5: the dry run moved ~/.claude"
for secret in qa-sentinel-argv-secret qa-sentinel-flag-secret qa-sentinel-environ-secret; do
  ! grep -qF -- "$secret" "$WORK/stdout" "$WORK/stderr" || fail "case 5: the output holds '$secret'"
done
echo "PASS: case 5: the dry run names the move to wwest and moves nothing"
evidence '^would (swap|stay|alert)' < "$WORK/stdout"

# --- Case 6: a waiting step wakes on the move ---------------------------------------------------
# A supervised step on the live login (a `$HOME/.claude` step) meets a usage
# limit whose reset is four hours away, as the steps of 23 November did, and
# waits. Its stand-in `claude` notes which stand-in login ~/.claude holds at
# each call, meets the limit on its first call and succeeds on the next.
# Auto-switch is off, so the step waits rather than rerunning on another
# profile, and no rotator is autostarted. The step's stderr comes through a
# pipe, and the driver acts on each wait line as the step writes it.
STEP_BIN="$WORK/step-bin"
mkdir -p "$STEP_BIN"
cat > "$STEP_BIN/claude" <<'STUB'
#!/usr/bin/env bash
if [[ "${1:-}" == "--version" ]]; then echo "2.1.319 (Claude Code)"; exit 0; fi
n=$(( $(cat "$STEP_RECORD/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$STEP_RECORD/count"
printf '%s\n' "$(sed -n 's/.*"accessToken":"qa-\([^"]*\)".*/\1/p' "$HOME/.claude/.credentials.json")" >> "$STEP_RECORD/logins"
echo "attempt $n output"
if [[ "$n" == 1 ]]; then
  printf 'Claude AI usage limit reached|%s\n' "$LIMIT_EPOCH" >&2
  exit 1
fi
exit 0
STUB
chmod +x "$STEP_BIN/claude"

WAIT_PREFIX='amplihack.recipe.usage_limit_wait'
STEP_RECORD=""
STEP_ERR=""
STEP_RC=""
NEXT=""
start_step() { # case name: starts the step; its stderr is read on descriptor 4
  STEP_RECORD="$WORK/step.$1"
  mkdir -p "$STEP_RECORD"
  STEP_ERR="$STEP_RECORD/stderr"
  : > "$STEP_ERR"
  mkfifo "$STEP_RECORD/stderr.fifo"
  (cd "$WORK" && exec env -i \
    HOME="$QA_HOME" \
    PATH="$STEP_BIN:/usr/bin:/bin" \
    TERM=dumb \
    LANG=C \
    TZ=UTC \
    TMPDIR="$WORK/tmp" \
    STEP_RECORD="$STEP_RECORD" \
    LIMIT_EPOCH="$(( $(date +%s) + 4 * 3600 ))" \
    AMPLIHACK_CLAUDE_BINARY_PATH="$STEP_BIN/claude" \
    AMPLIHACK_CLAUDE_AUTO_SWITCH=0 \
    AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_GOVERNOR=off \
    AMPLIHACK_NONINTERACTIVE=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_SKIP_POWER_STEERING=1 \
    AMPLIHACK_SKIP_REFLECTION=1 \
    AMPLIHACK_TEST_HOME_GUARD=1 \
    AMPLIHACK_PROFILE_USAGE_API_BASE="http://127.0.0.1:$PORT" \
    AMPLIHACK_PROFILE_USAGE_TOKEN_URL="http://127.0.0.1:$PORT/oauth/token" \
    AMPLIHACK_PROFILE_USAGE_PROBE=direct \
    "$BIN" claude --subprocess-safe -- -p step) \
    > "$STEP_RECORD/stdout" 2> "$STEP_RECORD/stderr.fifo" < /dev/null &
  STEP_PID=$!
  exec 4< "$STEP_RECORD/stderr.fifo"
}
# Reads the step's stderr, keeping every line, up to its next wait line, and
# sets NEXT to the rest of that line after the prefix. Fails when the step
# closes its stderr first.
next_wait_line() {
  local line
  NEXT=""
  while IFS= read -r -u 4 line; do
    printf '%s\n' "$line" >> "$STEP_ERR"
    if [[ "$line" == *"$WAIT_PREFIX "* ]]; then
      NEXT="${line#*"$WAIT_PREFIX "}"
      return 0
    fi
  done
  return 1
}
# Reads the rest of the step's stderr, then its exit status.
finish_step() {
  local line
  while IFS= read -r -u 4 line; do printf '%s\n' "$line" >> "$STEP_ERR"; done
  exec 4<&-
  set +e
  wait "$STEP_PID"
  STEP_RC=$?
  set -e
  STEP_PID=""
}
step_fail() { # label, message
  cat "$STEP_ERR" >&2
  [[ -z "$STEP_PID" ]] || kill "$STEP_PID" 2>/dev/null || true
  fail "$1: $2"
}
# The step is parked: its wait began on the blocked live login and, a minute
# in, wrote its first progress line with nothing having woken it.
await_parked() { # label
  next_wait_line || step_fail "$1" "the step ended without starting a usage-limit wait"
  [[ "$NEXT" == "start reason=usage_limit "* ]] || step_fail "$1" "the first wait line is '$NEXT', not a usage-limit start"
  next_wait_line || step_fail "$1" "the step ended while it should be waiting"
  [[ "$NEXT" == "progress remaining="* ]] || step_fail "$1" "the wait wrote '$NEXT' before anything moved"
  [[ "$(cat "$STEP_RECORD/count")" == "1" ]] || step_fail "$1" "the stand-in claude ran more than once before the move"
}
# The step ran again on wwest after its wait ended on the rotator's record,
# and succeeded.
assert_resumed_on_wwest() { # label
  finish_step
  [[ "$STEP_RC" -eq 0 ]] || step_fail "$1" "the step exited $STEP_RC"
  [[ "$(tr '\n' ' ' < "$STEP_RECORD/logins")" == "$LIVE wwest " ]] \
    || step_fail "$1" "the attempts ran on '$(tr '\n' ' ' < "$STEP_RECORD/logins")', not '$LIVE wwest'"
  ! grep -q "$WAIT_PREFIX \(failed\|skipped\) " "$STEP_ERR" || step_fail "$1" "the wait wrote a failed or skipped line"
}

# 6a and 6b: a move confirmed by a fresh reading of wwest ends the wait at once.
check_wake_case() { # label flags...
  local label=$1 log
  shift
  new_home "wake-${label//[^a-z0-9]/-}"
  nov23_processes
  block_live > /dev/null
  start_step "wake-${label//[^a-z0-9]/-}"
  await_parked "$label"
  run_cli profile rotate "$@"
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; step_fail "$label" "rotate exited $STATUS"; }
  log="$(rotate_log)"
  grep -q " swap $LIVE->wwest " <<< "$log" || { echo "$log" >&2; step_fail "$label" "no swap line to wwest"; }
  next_wait_line || step_fail "$label" "the step ended without ending its wait"
  [[ "$NEXT" == "end reason=rotator capacity" ]] \
    || step_fail "$label" "after the move the wait wrote '$NEXT', not 'end reason=rotator capacity'"
  assert_resumed_on_wwest "$label"
  echo "PASS: $label: a step parked on the spend-blocked live login wakes on the rotator's move and runs again on wwest"
  evidence "$WAIT_PREFIX (start|end|resume)" < "$STEP_ERR"
  evidence ' swap ' <<< "$log"
}
check_wake_case "case 6a (95/98)" "${DEPLOYED[@]}"
check_wake_case "case 6b (85/90)" "${REPORTER[@]}"

# 6c: a move made on wwest's last good reading (HTTP 429 for every
# candidate) records no room, so the wait reads it and stays (#2857); the next
# cycle reads wwest, now live, with room, and the wait ends on that record.
label="case 6c (95/98, last good reading)"
new_home wake-h429
nov23_processes
block_live > /dev/null
echo 429 > "$FX/usage-mode"
printf '%s\n' "$LIVE" > "$FX/usage-ok"
write_history wwest 30 40 1800
start_step wake-h429
await_parked "$label"
run_cli profile rotate "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; step_fail "$label" "the first rotate exited $STATUS"; }
log="$(rotate_log)"
grep -q " swap $LIVE->wwest .* unconfirmed (HTTP 429)" <<< "$log" \
  || { echo "$log" >&2; step_fail "$label" "no unconfirmed swap line to wwest"; }
[[ "$(live_token)" == "qa-wwest" ]] || step_fail "$label" "~/.claude holds $(live_token), not qa-wwest"
next_wait_line || step_fail "$label" "the step ended after the unconfirmed move"
[[ "$NEXT" == "progress remaining="* ]] \
  || step_fail "$label" "after a move with no room read the wait wrote '$NEXT', not a progress line"
echo ok > "$FX/usage-mode"
: > "$FX/usage-ok"
run_cli profile rotate "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; step_fail "$label" "the second rotate exited $STATUS"; }
log="$(rotate_log)"
next_wait_line || step_fail "$label" "the step ended without ending its wait"
[[ "$NEXT" == "end reason=rotator capacity" ]] \
  || { echo "$log" >&2; step_fail "$label" "after the cycle that read wwest the wait wrote '$NEXT', not 'end reason=rotator capacity'"; }
assert_resumed_on_wwest "$label"
echo "PASS: $label: the wait stays through a move on a last good reading and ends when the next cycle reads wwest with room"
evidence "$WAIT_PREFIX (start|progress|end|resume)" < "$STEP_ERR"
evidence ' (swap|cycle) ' <<< "$log"

[[ ! -s "$WORK/claude-calls" ]] || fail "claude was started: $(cat "$WORK/claude-calls")"
echo "ROTATE_UNREADABLE_QA_PASSED"
