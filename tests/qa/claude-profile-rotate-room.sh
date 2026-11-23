#!/usr/bin/env bash
# Issue #2910: acceptance test for the rotator's choice of target, run with
# stand-in readings.
#
# The candidate amplihack binary runs against a temporary HOME, a fake `claude`
# and a loopback stand-in for the usage and profile endpoints (`socat`, bound
# to 127.0.0.1 only). No real login, no network, no CLAUDE_CONFIG_DIR, no real
# profile root.
#
# It replays 22 November 2026: the live login reached 100% of its session
# while several logins had room, and the rotator only alerted. Ten cases:
#
#   1. dry run: reports the swap to the login with a fresh session (llarame);
#   2. one-shot cycle: moves ~/.claude to it, reason=plan-room, no ALERT;
#   3. HTTP 429 from the usage endpoint, a last good reading in the history:
#      the target is rejected at confirmation and nothing moves (issue #3105:
#      no swap without a fresh reading, whatever room the live login has);
#   4. HTTP 429 and no history: one `no capacity known` ALERT, nothing moves;
#   5. HTTP 429 and the target's identity read returns another account:
#      nothing moves;
#   6. dry run at the 05:48Z readings (live room 14): would stay, with the note;
#   7. the stand-in's own guards: a bad bearer token, a missing one and an
#      oversized request line are refused and serve no fixture;
#   8. a recorded spend block on llarame: it is passed over for qquinn, the
#      next login the ranking gives, with `reject: llarame blocked until
#      <time> (spend limit)` at 85/90 and at 95/98 (issue #3105), and chosen
#      again once the block ended;
#   9. the thresholds the hosts run, --session-at 95 --week-at 98: the dry run
#      and the one-shot cycle choose qquinn (every login below 95% of its
#      session is in the first tier, and qquinn has the most room now), with
#      no ALERT, the ledger records room for a waiting step, and at the 05:48Z
#      readings the dry run would stay with no note;
#  10. the 23 November replay (issue #3105): llarame carries a spend block,
#      every live probe but the live login's fails with HTTP 429, and the
#      history holds an old, roomy reading of llarame (0/33). With a rotator
#      from another build running, so that each ledger save takes the form
#      for older readers, two one-shot cycles (95/98, then 85/90) never swap,
#      log `reject: llarame blocked until <time> (spend limit)` each time and
#      keep the block in the ledger. With no block at all, the old reading
#      still leads to no swap at 95/98: the target is rejected at
#      confirmation.
#
# Needs socat and GNU `date -d` (Linux); the handler itself avoids bash 4 only
# expansions. Runs locally and is not part of CI.
#
# Usage: AMPLIHACK_QA_BINARY=target/debug/amplihack bash tests/qa/claude-profile-rotate-room.sh
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
STANDIN_PID=""
WORK=""
cleanup() {
  [[ -z "$STANDIN_PID" ]] || kill "$STANDIN_PID" 2>/dev/null || true
  [[ -z "$DRAIN_PID" ]] || kill "$DRAIN_PID" 2>/dev/null || true
  [[ -z "$SOCAT_PID" ]] || kill "$SOCAT_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  if [[ -n "$WORK" && "$WORK" == "${TMPDIR:-/tmp}"/rotate-room-qa.* && -d "$WORK" ]]; then
    rm -rf -- "$WORK"
  fi
}
# Installed before the folder is made, so a scratch folder this driver then
# refuses is still removed.
trap cleanup EXIT
WORK="$(mktemp -d "${TMPDIR:-/tmp}/rotate-room-qa.XXXXXX")"
[[ "$WORK" =~ ^[A-Za-z0-9._/-]+$ ]] || fail "unsuitable scratch folder name: $WORK"

FX="$WORK/fx"
QA_BIN_DIR="$WORK/bin"
mkdir -p "$FX/usage" "$FX/profile" "$QA_BIN_DIR" "$WORK/tmp"

# --- The fake claude -----------------------------------------------------------
# Every read in this scenario is direct, so `claude` must never be started for
# a probe. It answers `--version` and records anything else.
cat > "$QA_BIN_DIR/claude" <<CLAUDE
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then echo "2.1.319 (Claude Code)"; exit 0; fi
printf '%s\n' "\$*" >> "$WORK/claude-calls"
exit 1
CLAUDE
chmod +x "$QA_BIN_DIR/claude"

# --- The stand-in endpoints ---------------------------------------------------------
# One handler per connection, run by socat. It reads a bounded request, takes
# the login from `Authorization: Bearer qa-<name>` (the name is validated
# before any path is built from it) and answers from $QA_FIXTURE:
#   GET /api/oauth/usage    usage-mode: ok | 429       fx/usage/<name>.json
#                           (with 429, every login answers 429 except the
#                           names listed in fx/usage-ok: the live login)
#   GET /api/oauth/profile  fx/profile/<name>.json; names listed in
#                           fx/profile-mismatch answer with another account.
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
  # The header name, lower-cased with `tr`. Bash's own case-modifying
  # expansion is bash 4 only and macOS /bin/bash is 3.2 (issue #1423).
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
    if grep -qx -- "$name" "$FIX/profile-mismatch" 2>/dev/null; then
      reply "200 OK" '{"account":{"uuid":"other-account","email":"other@example.invalid"},"organization":{"uuid":"org-other"}}'
    else
      reply "200 OK" "$(cat "$FIX/profile/$name.json")"
    fi ;;
  *) reply "404 Not Found" '{}' ;;
esac
HANDLER
chmod +x "$WORK/handler.sh"

# Start the listener on a free loopback port. Its own listening line says which
# port; reading the FIFO blocks until socat writes it, and ends if socat dies,
# so a failed bind fails loudly with no polling.
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

# --- The fleet ---------------------------------------------------------------------------
# Readings of the reporter's cycle at 05:56:57Z on 22 November, in percent used:
# "name session week". The live login is rrowan. A window at 100% is out of room.
LIVE=rrowan
FLEET=(
  "llarame 0 95" "qquinn 88 20" "yyandis 88 71" "jjansport 90 10" "uunze 90 71"
  "ssorrel 93 22" "eelsworth 96 82" "ddrake 100 74" "ggrant 100 12" "nnorth 100 50"
  "aanderson 0 100" "bbuck 0 100"
)
ALL=("$LIVE")
for row in "${FLEET[@]}"; do read -r n _ <<< "$row"; ALL+=("$n"); done
LIVE_SESSION=100
LIVE_WEEK=67
RESET_SESSION="2026-11-22T11:00:00+00:00"
RESET_WEEK="2026-11-26T09:00:00+00:00"

# The live login is alone in its organisation, as the reporter's was: nothing in
# its own organisation has room, so the move is a confirmed crossing, which the
# rotator's plan-room swap did not make before.
org_of() { if [[ "$1" == "$LIVE" ]]; then echo org-a; else echo org-b; fi; }

write_usage() { # name session week
  printf '{"five_hour":{"utilization":%s,"resets_at":"%s"},"seven_day":{"utilization":%s,"resets_at":"%s"}}' \
    "$2" "$RESET_SESSION" "$3" "$RESET_WEEK" > "$FX/usage/$1.json"
}
write_profile() { # name
  printf '{"account":{"uuid":"u-%s","email":"%s@example.invalid"},"organization":{"uuid":"%s"}}' \
    "$1" "$1" "$(org_of "$1")" > "$FX/profile/$1.json"
}
write_login() { # directory name
  mkdir -p "$1"
  printf '{"claudeAiOauth":{"accessToken":"qa-%s","expiresAt":4102444800000}}' "$2" > "$1/.credentials.json"
  chmod 600 "$1/.credentials.json"
  printf '{"oauthAccount":{"accountUuid":"u-%s","emailAddress":"%s@example.invalid","organizationUuid":"%s"}}' \
    "$2" "$2" "$(org_of "$2")" > "$1/.claude.json"
  chmod 600 "$1/.claude.json"
}

write_usage "$LIVE" "$LIVE_SESSION" "$LIVE_WEEK"
write_profile "$LIVE"
write_profile start
for row in "${FLEET[@]}"; do
  read -r who s w <<< "$row"
  write_usage "$who" "$s" "$w"
  write_profile "$who"
done
echo ok > "$FX/usage-mode"

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
    AMPLIHACK_PROFILE_USAGE_API_BASE="http://127.0.0.1:$PORT" \
    AMPLIHACK_PROFILE_USAGE_TOKEN_URL="http://127.0.0.1:$PORT/oauth/token" \
    AMPLIHACK_PROFILE_USAGE_PROBE=direct \
    "$BIN" "$@") > "$WORK/stdout" 2> "$WORK/stderr"
  STATUS=$?
  set -e
}

new_home() { # fresh HOME with every login stored, and the live one in ~/.claude
  QA_HOME="$WORK/home.$1"
  ROOT="$QA_HOME/.amplihack/claude-profiles"
  rm -rf -- "$QA_HOME"
  mkdir -p "$QA_HOME/.claude"
  chmod 700 "$QA_HOME" "$QA_HOME/.claude"
  # A starting login, so the first switch has one to return: it is kept under
  # default-original and removed again below.
  write_login "$QA_HOME/.claude" start
  mv "$QA_HOME/.claude/.claude.json" "$QA_HOME/.claude.json"
  local who
  for who in "${ALL[@]}"; do
    run_cli profile add "$who"
    [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile add $who"; }
    write_login "$ROOT/$who" "$who"
  done
  # Ownership records, from the stand-in's answers, so the rotator may move off
  # and onto these logins.
  for who in "${ALL[@]}"; do
    run_cli profile seed-record "$who" --write
    [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile seed-record $who"; }
  done
  # Put the live login in ~/.claude as an operator would, then let rotation
  # choose again.
  run_cli profile switch "$LIVE"
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile switch $LIVE"; }
  run_cli profile switch --none
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile switch --none"; }
  run_cli profile remove default-original --yes
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile remove default-original"; }
  # Every case that says "moved nothing" compares with this login.
  [[ "$(live_token)" == "qa-$LIVE" ]] \
    || fail "the setup left ~/.claude holding '$(live_token)', not qa-$LIVE"
  mkdir -p "$ROOT/.auto-switch.d"
  chmod 700 "$ROOT/.auto-switch.d"
}

rfc3339() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }

# One usage-history line for $1 with session $2 and week $3 percent, taken $4
# seconds ago, in the file's own format.
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

# A recorded block on $1, as a limit event leaves behind: in force from $2
# seconds ago until $3 seconds from now, for the limit kind $4. A negative $3
# is a block that has already ended. The ledger is the rotator's own
# `.auto-switch.d/rotate/ledger.json`, written before the cycle starts.
write_block() { # name from_ago until_in kind
  local now at until dir
  now=$(date +%s)
  at=$((now - $2))
  until=$((now + $3))
  dir="$ROOT/.auto-switch.d/rotate"
  mkdir -p "$dir"
  chmod 700 "$ROOT/.auto-switch.d" "$dir"
  printf '{"version":1,"overflow_blocked":{"%s":{"at":%s,"until":%s,"reason":"%s"}}}\n' \
    "$1" "$at" "$until" "$4" > "$dir/ledger.json"
  chmod 600 "$dir/ledger.json"
}

# The usage endpoint answers 429 for every login but the live one, as the
# reporter's did from 23:58Z on 21 November.
storm() { echo 429 > "$FX/usage-mode"; printf '%s\n' "$LIVE" > "$FX/usage-ok"; }

live_token() { sed -n 's/.*"accessToken":"\([^"]*\)".*/\1/p' "$QA_HOME/.claude/.credentials.json"; }
# The rotator's log. A missing log is a failure, not an empty log, so a check
# that something is absent cannot pass for a cycle that never ran.
rotate_log() {
  local file="$ROOT/.auto-switch.d/rotate/rotate.log"
  [[ -f "$file" ]] || { echo "FAIL: the rotator wrote no rotate.log" >&2; return 1; }
  cat "$file"
}
# The lines a case passed on, for the record: rotate-log text only (names,
# percentages, fixed labels and times), never a path.
evidence() { grep -E "$1" | sed 's/^/  | /' || true; }
FLAGS=(--session-at 85 --week-at 90 --margin 3 --upgrade-margin 30 --hold 3600)

# --- Case 1: the dry run -------------------------------------------------------------------------
new_home dry
before="$(live_token)"
run_cli profile rotate --dry-run "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 1: dry run exited $STATUS"; }
grep -q "would swap $LIVE->llarame " "$WORK/stdout" \
  || { cat "$WORK/stdout" >&2; fail "case 1: the dry run did not report the swap to llarame"; }
grep -q "reason=plan-room" "$WORK/stdout" || fail "case 1: the dry run did not say reason=plan-room"
! grep -q "no profile has capacity" "$WORK/stdout" || fail "case 1: the dry run said no profile has capacity"
[[ "$(live_token)" == "$before" ]] || fail "case 1: the dry run moved ~/.claude"
echo "PASS: case 1: the dry run reports the swap to llarame and moves nothing"
evidence '^(would swap|would alert|would stay|cycle )' < "$WORK/stdout"

# --- Case 2: the one-shot cycle -------------------------------------------------------------------
new_home swap
run_cli profile rotate "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 2: rotate exited $STATUS"; }
[[ "$(live_token)" == "qa-llarame" ]] || fail "case 2: ~/.claude holds $(live_token), not qa-llarame"
log="$(rotate_log)"
grep -q "swap $LIVE->llarame .*reason=plan-room" <<< "$log" || { echo "$log" >&2; fail "case 2: no plan-room swap line"; }
! grep -q "no profile has capacity" <<< "$log" || fail "case 2: the log says no profile has capacity"
! grep -q " ALERT: " <<< "$log" || { echo "$log" >&2; fail "case 2: the cycle wrote an ALERT"; }
echo "PASS: case 2: the one-shot cycle moves ~/.claude to llarame with reason=plan-room"
evidence ' (swap|affinity:) ' <<< "$log"

# --- Case 3: HTTP 429, a last good reading in the history -------------------------------------------
new_home h429
storm
write_history llarame 78 92 1800
before="$(live_token)"
run_cli profile rotate "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 3: rotate exited $STATUS"; }
[[ "$(live_token)" == "$before" ]] || fail "case 3: ~/.claude changed to $(live_token) on a reading nobody confirmed"
log="$(rotate_log)"
grep -q "llarame:unknown, HTTP 429, last s=78%,w=92% " <<< "$log" \
  || { echo "$log" >&2; fail "case 3: the cycle line does not show the probe's own error for llarame"; }
grep -q "reject: llarame reading unknown (HTTP 429)" <<< "$log" \
  || { echo "$log" >&2; fail "case 3: llarame was not rejected at confirmation"; }
grep -q "stay: no candidate confirmed by a fresh reading" <<< "$log" \
  || { echo "$log" >&2; fail "case 3: the cycle did not end on the no-candidate-confirmed note"; }
! grep -q " swap $LIVE->" <<< "$log" || { echo "$log" >&2; fail "case 3: the rotator swapped"; }
! grep -q "unconfirmed (" <<< "$log" || { echo "$log" >&2; fail "case 3: the log has an unconfirmed swap"; }
echo "PASS: case 3: HTTP 429 with a last good reading rejects llarame at confirmation and moves nothing"
evidence ' (reject:|stay:|swap) ' <<< "$log"
grep -o 'llarame:unknown, HTTP 429, last [^ ]* [0-9]*m ago' <<< "$log" | sed 's/^/  | /' || true

# --- Case 4: HTTP 429, no history ----------------------------------------------------------------------
new_home none
storm
before="$(live_token)"
run_cli profile rotate "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 4: rotate exited $STATUS"; }
run_cli profile rotate "${FLAGS[@]}"
# The second, identical cycle must have run and finished: one ALERT in the log
# proves the key held only if both cycles are in it.
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 4: the second rotate exited $STATUS"; }
[[ "$(live_token)" == "$before" ]] || fail "case 4: ~/.claude changed"
log="$(rotate_log)"
cycles="$(grep -c "cycle live=" <<< "$log" || true)"
[[ "$cycles" == "2" ]] || { echo "$log" >&2; fail "case 4: expected two cycle lines, found $cycles"; }
alerts="$(grep -c " ALERT: no capacity known " <<< "$log" || true)"
[[ "$alerts" == "1" ]] || { echo "$log" >&2; fail "case 4: expected one 'no capacity known' ALERT, found $alerts"; }
grep -q "ALERT: no capacity known live=$LIVE s=100% w=67% (candidates: 12 unknown (HTTP 429))" <<< "$log" \
  || { echo "$log" >&2; fail "case 4: the ALERT does not count the unknown logins by cause"; }
! grep -q "no profile has capacity" <<< "$log" || fail "case 4: the log says no profile has capacity"
echo "PASS: case 4: HTTP 429 with no history gives one 'no capacity known' ALERT and moves nothing"
evidence ' ALERT: no capacity known ' <<< "$log"

# --- Case 5: HTTP 429, the target's identity read returns another account -----------------------------------
new_home mismatch
storm
printf '%s\n' llarame > "$FX/profile-mismatch"
write_history llarame 78 92 1800
before="$(live_token)"
run_cli profile rotate "${FLAGS[@]}"
# Issue #3105: llarame's usage is unknown (HTTP 429), so it is rejected at
# confirmation, before any swap and so before its identity is read; the
# one-shot cycle ends on the stay note, with nothing refused to report.
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 5: rotate exited $STATUS"; }
log="$(rotate_log)"
# The cycle ran and tried llarame: it says so, and says why it did not move.
grep -q "llarame:unknown, HTTP 429, last s=78%,w=92% " <<< "$log" \
  || { echo "$log" >&2; fail "case 5: the cycle line does not show that llarame was tried"; }
grep -q "reject: llarame reading unknown (HTTP 429)" <<< "$log" \
  || { echo "$log" >&2; fail "case 5: llarame was not rejected at confirmation"; }
[[ "$(live_token)" == "$before" ]] || fail "case 5: ~/.claude changed for a login whose identity did not match"
! grep -q " swap $LIVE->llarame" <<< "$log" || fail "case 5: the rotator swapped to llarame"
: > "$FX/profile-mismatch"
echo "PASS: case 5: an identity that does not match moves nothing"
evidence ' (ALERT|swap|reject:) ' <<< "$log"

# --- Case 6: the dry run at 05:48Z, the live login still has room ------------------------------------------
# The live login read 86% / 65% (room 14). The logins that have room are in
# another organisation, which a live login with room never crosses to, so
# nothing qualifies: the dry run says it would stay, with the note, not alert.
echo ok > "$FX/usage-mode"   # the earlier cases left the stand-in answering 429
new_home stay
write_usage "$LIVE" 86 65
before="$(live_token)"
run_cli profile rotate --dry-run "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 6: dry run exited $STATUS"; }
grep -q "would stay (live room 14, only a login in another organisation has room, a move needs 17)" "$WORK/stdout" \
  || { cat "$WORK/stdout" >&2; fail "case 6: the dry run did not say it would stay, with the note"; }
! grep -qE "would (alert|swap)" "$WORK/stdout" || fail "case 6: the dry run would move or alert"
[[ "$(live_token)" == "$before" ]] || fail "case 6: the dry run moved ~/.claude"
write_usage "$LIVE" "$LIVE_SESSION" "$LIVE_WEEK"
echo "PASS: case 6: a live login with room 14 and nothing it may move to would stay, with the note"
evidence '^(would swap|would alert|would stay)' < "$WORK/stdout"

# --- Case 7: the stand-in's own guards ------------------------------------------------------
# The cases above rest on the handler validating the bearer token before it
# builds any path from it, and reading a bounded request. This case talks to the
# listener directly, with no rotator: a token holding `..`, an upper-case one,
# an over-long one and a missing header are all refused, and an oversized
# request line serves no fixture.
ask() { # token (empty for no header), request line -> the answer
  local token=$1 req=$2
  exec 3<>"/dev/tcp/127.0.0.1/$PORT" || return 0
  {
    printf '%s\r\n' "$req"
    [[ -z "$token" ]] || printf 'Authorization: Bearer %s\r\n' "$token"
    printf 'Connection: close\r\n\r\n'
  } >&3
  cat <&3
  exec 3<&- 3>&-
}
# The answer's status line, with its carriage return removed; nothing for no answer.
status_of() { sed -n '1s/\r$//p'; }

long_name="qa-$(printf 'a%.0s' $(seq 70))"
CASE7=()
for token in 'qa-../x' 'qa-A' 'qa-' "$long_name" 'nonsense'; do
  answer="$(ask "$token" 'GET /api/oauth/usage HTTP/1.1')"
  line="$(status_of <<< "$answer")"
  [[ "$line" == "HTTP/1.1 401 Unauthorized" ]] \
    || fail "case 7: the stand-in answered '$line' for the token '$token', not 401"
  CASE7+=("token refused: $line")
done
answer="$(ask "" 'GET /api/oauth/usage HTTP/1.1')"
line="$(status_of <<< "$answer")"
[[ "$line" == "HTTP/1.1 401 Unauthorized" ]] \
  || fail "case 7: the stand-in answered '$line' with no Authorization header, not 401"
CASE7+=("no Authorization header: $line")
# A request line of 2500 bytes names a path that is no fixture's: 404, and
# never 200. The status does not show the handler's 2048-byte read bound (a
# path cut at that bound is no fixture's either); it shows that the listener
# is up, answers a long line, and serves nothing for it.
answer="$(ask qa-llarame "GET /api/oauth/usage/$(printf 'a%.0s' $(seq 2500)) HTTP/1.1")"
line="$(status_of <<< "$answer")"
[[ "$line" == "HTTP/1.1 404 Not Found" ]] \
  || fail "case 7: the oversized request line was answered '${line:-no answer}', not 404"
CASE7+=("oversized request line: $line")
# The refused token named a path with `..`; nothing was read or written there.
[[ ! -e "$FX/x.json" && ! -e "$FX/usage/x.json" ]] \
  || fail "case 7: a token holding .. reached a path outside the fixtures"
[[ ! -s "$WORK/claude-calls" ]] || fail "case 7: claude was started"
echo "PASS: case 7: a bad or missing token is refused and an oversized request serves no fixture"
printf '  | %s\n' "${CASE7[@]}"

# --- Case 8: a recorded block is kept out until its reset ----------------------------------
# The first behaviour the issue asks for: never a login with a recorded block
# before its reset. On 21 November at 07:20:31Z the rotator moved to a login
# whose organisation had hit its spend limit until 26 November, and the next
# request met it. Here llarame, which every case above chooses, carries a spend
# block: the rotator must pass over it for the next login the ranking gives
# (qquinn, which has more room now than yyandis at the same session and a lower
# week), say why llarame was passed over, and choose llarame again once the
# block has ended. Both halves run as dry runs, so the readings are the same
# for each and nothing moves between them.
new_home blocked
before="$(live_token)"
write_block llarame 3600 3600 spend_limit
run_cli profile rotate --dry-run "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 8: the blocked dry run exited $STATUS"; }
blocked_out="$(cat "$WORK/stdout")"
! grep -q "would swap $LIVE->llarame " <<< "$blocked_out" \
  || { echo "$blocked_out" >&2; fail "case 8: the rotator chose llarame while its block was in force"; }
grep -q "would swap $LIVE->qquinn " <<< "$blocked_out" \
  || { echo "$blocked_out" >&2; fail "case 8: the rotator did not choose qquinn, the next login the ranking gives"; }
grep -q "llarame:.*blocked until .*(spend limit)" <<< "$blocked_out" \
  || { echo "$blocked_out" >&2; fail "case 8: the cycle line does not say why llarame was passed over"; }
rejects="$(grep -cE "^reject: llarame blocked until [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}Z \(spend limit\)$" <<< "$blocked_out" || true)"
[[ "$rejects" == "1" ]] \
  || { echo "$blocked_out" >&2; fail "case 8: expected one 'reject: llarame blocked until <time> (spend limit)' line, found $rejects"; }
! grep -q "no profile has capacity" <<< "$blocked_out" \
  || fail "case 8: the dry run said no profile has capacity"
[[ "$(live_token)" == "$before" ]] || fail "case 8: the blocked dry run moved ~/.claude"

# The same block at the thresholds the hosts run: llarame is still passed
# over, with the same one reject line.
run_cli profile rotate --dry-run --session-at 95 --week-at 98 --margin 3 --upgrade-margin 30 --hold 3600
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 8: the blocked dry run at 95/98 exited $STATUS"; }
blocked_hosts="$(cat "$WORK/stdout")"
! grep -q "would swap $LIVE->llarame " <<< "$blocked_hosts" \
  || { echo "$blocked_hosts" >&2; fail "case 8: at 95/98 the rotator chose llarame while its block was in force"; }
rejects="$(grep -cE "^reject: llarame blocked until [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}Z \(spend limit\)$" <<< "$blocked_hosts" || true)"
[[ "$rejects" == "1" ]] \
  || { echo "$blocked_hosts" >&2; fail "case 8: at 95/98 expected one block reject line for llarame, found $rejects"; }
[[ "$(live_token)" == "$before" ]] || fail "case 8: the blocked dry run at 95/98 moved ~/.claude"

# The same readings with the block already ended: llarame is chosen again.
write_block llarame 7200 -60 spend_limit
run_cli profile rotate --dry-run "${FLAGS[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 8: the dry run after the reset exited $STATUS"; }
after_out="$(cat "$WORK/stdout")"
grep -q "would swap $LIVE->llarame " <<< "$after_out" \
  || { echo "$after_out" >&2; fail "case 8: the rotator did not choose llarame once its block had ended"; }
! grep -q "llarame:.*blocked until" <<< "$after_out" \
  || { echo "$after_out" >&2; fail "case 8: the cycle line still calls llarame blocked after its reset"; }
! grep -q "^reject: llarame blocked" <<< "$after_out" \
  || { echo "$after_out" >&2; fail "case 8: a reject line still calls llarame blocked after its reset"; }
[[ "$(live_token)" == "$before" ]] || fail "case 8: the dry run after the reset moved ~/.claude"
echo "PASS: case 8: a blocked login is passed over until its reset and chosen after it"
evidence '^(would (swap|stay|alert)|reject: )' <<< "$blocked_out"
evidence 'llarame:' <<< "$blocked_out"
evidence '^(would (swap|stay|alert)|reject: )' <<< "$blocked_hosts"
evidence '^would (swap|stay|alert)' <<< "$after_out"

# --- Case 9: the thresholds the hosts run, 95/98 -------------------------------------------
# Cases 1 to 8 use the reporter's flags. The hosts run --session-at 95
# --week-at 98, where every login below 95% of its session is in the first
# tier: qquinn (88/20) and yyandis (88/71) have the most room now, 12, and
# qquinn the lower week. Its windows are below both thresholds, so the swap is
# a threshold swap. The live login has no room, so no ALERT is written while
# a login below 100% on both windows exists, and the ledger records room on
# the login now live, which is what wakes a step waiting on a usage limit.
DEPLOYED=(--session-at 95 --week-at 98 --margin 3 --upgrade-margin 30 --hold 3600)
new_home deployed
before="$(live_token)"
run_cli profile rotate --dry-run "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 9: the dry run exited $STATUS"; }
deployed_out="$(cat "$WORK/stdout")"
grep -q "would swap $LIVE->qquinn .*target s=88% w=20% reason=threshold" <<< "$deployed_out" \
  || { echo "$deployed_out" >&2; fail "case 9: the dry run did not report the threshold swap to qquinn"; }
! grep -q "no profile has capacity" <<< "$deployed_out" || fail "case 9: the dry run said no profile has capacity"
[[ "$(live_token)" == "$before" ]] || fail "case 9: the dry run moved ~/.claude"
run_cli profile rotate "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 9: rotate exited $STATUS"; }
[[ "$(live_token)" == "qa-qquinn" ]] || fail "case 9: ~/.claude holds $(live_token), not qa-qquinn"
log="$(rotate_log)"
grep -q "swap $LIVE->qquinn .*reason=threshold" <<< "$log" || { echo "$log" >&2; fail "case 9: no threshold swap line"; }
! grep -q " ALERT: " <<< "$log" || { echo "$log" >&2; fail "case 9: the cycle wrote an ALERT"; }
ledger="$ROOT/.auto-switch.d/rotate/ledger.json"
grep -qE '"live_room": *true' "$ledger" \
  || fail "case 9: the ledger does not record room on qquinn for a waiting step"
# 05:48Z at 95/98: the live login (86/65) is below both thresholds and no
# login is 30 points better, so the dry run would stay, with no note.
new_home deployed-stay
write_usage "$LIVE" 86 65
before="$(live_token)"
run_cli profile rotate --dry-run "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 9: the 05:48Z dry run exited $STATUS"; }
stay_out="$(cat "$WORK/stdout")"
grep -qx "would stay" <<< "$stay_out" \
  || { echo "$stay_out" >&2; fail "case 9: the 05:48Z dry run did not say a plain 'would stay'"; }
! grep -qE "would (alert|swap)" <<< "$stay_out" || fail "case 9: the 05:48Z dry run would move or alert"
[[ "$(live_token)" == "$before" ]] || fail "case 9: the 05:48Z dry run moved ~/.claude"
write_usage "$LIVE" "$LIVE_SESSION" "$LIVE_WEEK"
echo "PASS: case 9: at 95/98 the rotator moves to qquinn with no ALERT, records room, and stays at 05:48Z"
evidence '^would (swap|stay|alert)' <<< "$deployed_out"
evidence ' (swap|affinity:) ' <<< "$log"
evidence '^would (swap|stay|alert)' <<< "$stay_out"

# --- Case 10: the 23 November replay (issue #3105) ------------------------------------------
# On 23 November a login (here llarame) hit its spend limit at 06:00:21Z and
# was blocked until 10:40Z. Its live probes then failed, and the history still
# held its reading from 05:44:55Z, 0/33, which looks roomy. The rotator had
# been upgraded in place, so it ran another build than the one on disk, and
# every ledger save wrote the form for older readers, which dropped the block.
# At 06:18:29Z it swapped to the blocked login on the old reading, unconfirmed.
#
# Here the block is in the ledger, every probe but the live login's answers
# 429, and the old reading is in the history, 2006 seconds old. The rotator
# from another build is a stand-in process this driver starts: its command
# line reads as `profile rotate watch` and its executable is not the
# candidate's. It never cycles; it only makes each save take the older form.
# Two one-shot cycles run, at the hosts' 95/98 and then at 85/90. The second
# reads the ledger the first saved, which is where the block was lost before.
new_home replay
before="$(live_token)"
write_block llarame 1077 15822 spend_limit
storm
write_history llarame 0 33 2006
ledger="$ROOT/.auto-switch.d/rotate/ledger.json"
block_until() { tr -d ' \n' < "$ledger" | sed -n 's/.*"overflow_blocked":{"llarame":{[^}]*"until":\([0-9]*\).*/\1/p'; }
until_before="$(block_until)"
[[ -n "$until_before" ]] || fail "case 10: the setup left no block on llarame in the ledger"
# The stand-in says it is up only once it runs as `bash -c`, so its command
# line is the one the rotator checks before the pid file names it.
mkfifo "$WORK/standin.ready" "$WORK/standin.hold"
bash -c 'echo up > "$1"; read -r _ < "$2"' amplihack "$WORK/standin.ready" "$WORK/standin.hold" profile rotate watch &
STANDIN_PID=$!
read -r _ < "$WORK/standin.ready"
printf '%s\n' "$STANDIN_PID" > "$ROOT/.auto-switch.d/rotate/rotate.pid"
chmod 600 "$ROOT/.auto-switch.d/rotate/rotate.pid"
replay_cycle() { # label flags...
  local label=$1 seen
  shift
  run_cli profile rotate "$@"
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 10 ($label): rotate exited $STATUS"; }
  grep -q "runs another amplihack binary" "$WORK/stderr" \
    || { cat "$WORK/stderr" >&2; fail "case 10 ($label): the cycle did not see the stand-in as a rotator from another build"; }
  [[ "$(live_token)" == "$before" ]] || fail "case 10 ($label): ~/.claude changed to $(live_token)"
  seen="$(block_until)"
  [[ "$seen" == "$until_before" ]] \
    || fail "case 10 ($label): the ledger saved for older readers holds '${seen}' for llarame's block, not $until_before"
}
replay_cycle 95/98 "${DEPLOYED[@]}"
replay_cycle 85/90 "${FLAGS[@]}"
log="$(rotate_log)"
rejects="$(grep -cE " reject: llarame blocked until [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}Z \(spend limit\)$" <<< "$log" || true)"
[[ "$rejects" == "2" ]] \
  || { echo "$log" >&2; fail "case 10: expected one 'reject: llarame blocked until <time> (spend limit)' line per cycle, found $rejects in two"; }
! grep -q " swap $LIVE->" <<< "$log" || { echo "$log" >&2; fail "case 10: the rotator swapped"; }
! grep -q "unconfirmed (" <<< "$log" || { echo "$log" >&2; fail "case 10: the log has an unconfirmed swap"; }
kill "$STANDIN_PID" 2>/dev/null || true
wait "$STANDIN_PID" 2>/dev/null || true
STANDIN_PID=""
replay_log="$log"

# The same old reading with no block at all, as if the block had been lost:
# at 95/98 the target is still rejected at confirmation and nothing moves.
new_home replay-unblocked
before="$(live_token)"
storm
write_history llarame 0 33 2006
run_cli profile rotate "${DEPLOYED[@]}"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; cat "$WORK/stdout" >&2; fail "case 10: the unblocked cycle exited $STATUS"; }
[[ "$(live_token)" == "$before" ]] || fail "case 10: ~/.claude changed to $(live_token) on a reading nobody confirmed"
log="$(rotate_log)"
grep -q "reject: llarame reading unknown (HTTP 429)" <<< "$log" \
  || { echo "$log" >&2; fail "case 10: llarame was not rejected at confirmation"; }
grep -q "stay: no candidate confirmed by a fresh reading" <<< "$log" \
  || { echo "$log" >&2; fail "case 10: the cycle did not end on the no-candidate-confirmed note"; }
! grep -q " swap $LIVE->" <<< "$log" || { echo "$log" >&2; fail "case 10: the unblocked cycle swapped"; }
! grep -q "unconfirmed (" <<< "$log" || { echo "$log" >&2; fail "case 10: the unblocked cycle has an unconfirmed swap"; }
echo ok > "$FX/usage-mode"
: > "$FX/usage-ok"
echo "PASS: case 10: the 23 November replay never swaps to the blocked login or on its old reading, at 95/98 and 85/90"
evidence ' (reject:|stay:|swap|ALERT:) ' <<< "$replay_log"
evidence ' (reject:|stay:|swap) ' <<< "$log"

[[ ! -s "$WORK/claude-calls" ]] || fail "claude was started: $(cat "$WORK/claude-calls")"
echo "ROTATE_ROOM_QA_PASSED"
