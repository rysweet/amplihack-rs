#!/usr/bin/env bash
# Issue #3258: acceptance test for the use rate and the runway line of
# `amplihack profile usage`, run against the recorded history of a night.
#
# The candidate amplihack binary runs against a temporary HOME, a fake `claude`
# and a loopback stand-in for the usage and profile endpoints (`socat`, bound
# to 127.0.0.1 only). No real login, no network, no real profile root.
#
# The usage history is the fixture testdata/issue_3258_26nov.jsonl, moved in
# time so that its last line is two minutes old. The stand-in serves that last
# line's figures as the current readings. The fixture reproduces the night of
# 25 to 26 November 2026: single low readings that the next reading reverses,
# falls of 44 to 50 points that stay with the weekly reset time unchanged, and
# five full logins whose weeks renew within 24 hours. Three cases:
#
#   1. `--summary-json`: the rate is about 0.3 login-weeks an hour, not the
#      0.9 to 1.1 printed that night; the room lasts beyond 24 hours once the
#      renewals are counted; the lowest room is above zero; runway_hours,
#      which leaves the renewals out, is still given.
#   2. the plain text: the runway line gives no figure of a few hours.
#   3. `--json`: the rows still parse as one array, one row per login.
#
# Needs socat, jq and GNU `date -d` (Linux). Runs locally and is not part of
# CI; the Rust tests in tests_capacity_summary.rs check the same figures in CI.
#
# Usage: AMPLIHACK_QA_BINARY=target/debug/amplihack bash tests/qa/issue-3258-usage-rate-runway.sh
set -euo pipefail

fail() { echo "FAIL: $*" >&2; exit 1; }

if [[ -z "${AMPLIHACK_QA_BINARY:-}" || ! -x "${AMPLIHACK_QA_BINARY:-}" ]]; then
  fail "AMPLIHACK_QA_BINARY must name an executable candidate amplihack binary"
fi
command -v socat >/dev/null 2>&1 || fail "socat is required for the usage stand-in"
command -v jq >/dev/null 2>&1 || fail "jq is required to read the summary JSON"
BIN="$(cd "$(dirname "$AMPLIHACK_QA_BINARY")" && pwd)/$(basename "$AMPLIHACK_QA_BINARY")"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURE="$REPO/crates/amplihack-cli/src/commands/profile/testdata/issue_3258_26nov.jsonl"
[[ -f "$FIXTURE" ]] || fail "the fixture is missing: $FIXTURE"

SOCAT_PID=""
DRAIN_PID=""
WORK=""
cleanup() {
  [[ -z "$DRAIN_PID" ]] || kill "$DRAIN_PID" 2>/dev/null || true
  [[ -z "$SOCAT_PID" ]] || kill "$SOCAT_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  if [[ -n "$WORK" && "$WORK" == "${TMPDIR:-/tmp}"/usage-runway-qa.* && -d "$WORK" ]]; then
    rm -rf -- "$WORK"
  fi
}
trap cleanup EXIT
WORK="$(mktemp -d "${TMPDIR:-/tmp}/usage-runway-qa.XXXXXX")"
[[ "$WORK" =~ ^[A-Za-z0-9._/-]+$ ]] || fail "unsuitable scratch folder name: $WORK"

FX="$WORK/fx"
QA_BIN_DIR="$WORK/bin"
mkdir -p "$FX/usage" "$FX/profile" "$QA_BIN_DIR" "$WORK/tmp"

# --- The fake claude ---------------------------------------------------------
# Every read here is direct, so `claude` must never be started for a probe.
cat > "$QA_BIN_DIR/claude" <<CLAUDE
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then echo "2.1.319 (Claude Code)"; exit 0; fi
printf '%s\n' "\$*" >> "$WORK/claude-calls"
exit 1
CLAUDE
chmod +x "$QA_BIN_DIR/claude"

# --- The stand-in endpoints --------------------------------------------------
# One handler per connection, run by socat, as in claude-profile-rotate-room.sh.
# It takes the login from `Authorization: Bearer qa-<name>` and answers
#   GET /api/oauth/usage    fx/usage/<name>.json
#   GET /api/oauth/profile  fx/profile/<name>.json
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
    [[ -f "$FIX/usage/$name.json" ]] || { reply "404 Not Found" '{}'; exit 0; }
    reply "200 OK" "$(cat "$FIX/usage/$name.json")" ;;
  /api/oauth/profile)
    [[ -f "$FIX/profile/$name.json" ]] || { reply "404 Not Found" '{}'; exit 0; }
    reply "200 OK" "$(cat "$FIX/profile/$name.json")" ;;
  *) reply "404 Not Found" '{}' ;;
esac
HANDLER
chmod +x "$WORK/handler.sh"

# The listener's own line gives its port; reading the FIFO blocks until socat
# writes it, and ends if socat dies, so a failed bind fails loudly.
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

# --- The history, moved in time ----------------------------------------------
# Every time in the fixture moves by the same amount, so that the last line is
# 120 seconds old. The spacing of the readings and the distance to each weekly
# reset stay as recorded.
epoch() { date -u -d "$1" +%s; }
rfc3339() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }
LAST_AT="$(tail -n 1 "$FIXTURE" | jq -r .at)"
SHIFT=$(( $(date -u +%s) - 120 - $(epoch "$LAST_AT") ))
: > "$WORK/shift.sed"
while IFS= read -r t; do
  printf 's/"%s"/"%s"/g\n' "$t" "$(rfc3339 $(( $(epoch "$t") + SHIFT )))" >> "$WORK/shift.sed"
done < <(grep -oE '"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z"' "$FIXTURE" | tr -d '"' | sort -u)
sed -f "$WORK/shift.sed" "$FIXTURE" > "$WORK/history.jsonl"
LAST="$(tail -n 1 "$WORK/history.jsonl")"
mapfile -t NAMES < <(jq -r '.profiles | keys[]' <<< "$LAST")
[[ ${#NAMES[@]} -eq 15 ]] || fail "expected 15 logins in the fixture, found ${#NAMES[@]}"

# The current readings: the last history line's figures, as the endpoint gives
# them. A login with no session figure has no five-hour window.
for who in "${NAMES[@]}"; do
  jq -c --arg n "$who" '.profiles[$n] | {
      five_hour: (if .s == null then null else {utilization: .s, resets_at: null} end),
      seven_day: {utilization: .w, resets_at: .w_reset}
    }' <<< "$LAST" > "$FX/usage/$who.json"
  printf '{"account":{"uuid":"u-%s","email":"%s@example.invalid"},"organization":{"uuid":"org-%s"}}' \
    "$who" "$who" "$who" > "$FX/profile/$who.json"
done
printf '{"account":{"uuid":"u-start","email":"start@example.invalid"},"organization":{"uuid":"org-start"}}' \
  > "$FX/profile/start.json"

write_login() { # directory name
  mkdir -p "$1"
  printf '{"claudeAiOauth":{"accessToken":"qa-%s","expiresAt":4102444800000}}' "$2" > "$1/.credentials.json"
  chmod 600 "$1/.credentials.json"
  printf '{"oauthAccount":{"accountUuid":"u-%s","emailAddress":"%s@example.invalid","organizationUuid":"org-%s"}}' \
    "$2" "$2" "$2" > "$1/.claude.json"
  chmod 600 "$1/.claude.json"
}

QA_HOME="$WORK/home"
ROOT="$QA_HOME/.amplihack/claude-profiles"
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

# --- The home ----------------------------------------------------------------
mkdir -p "$QA_HOME/.claude"
chmod 700 "$QA_HOME" "$QA_HOME/.claude"
write_login "$QA_HOME/.claude" start
mv "$QA_HOME/.claude/.claude.json" "$QA_HOME/.claude.json"
for who in "${NAMES[@]}"; do
  run_cli profile add "$who"
  [[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile add $who"; }
  write_login "$ROOT/$who" "$who"
done
LIVE="${NAMES[0]}"
run_cli profile switch "$LIVE"
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile switch $LIVE"; }
run_cli profile switch --none
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile switch --none"; }
run_cli profile remove default-original --yes
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "profile remove default-original"; }
mkdir -p "$ROOT/.auto-switch.d"
chmod 700 "$ROOT/.auto-switch.d"
cp "$WORK/history.jsonl" "$ROOT/.auto-switch.d/usage-history.jsonl"
chmod 600 "$ROOT/.auto-switch.d/usage-history.jsonl"

# --- Case 1: the summary JSON ------------------------------------------------
run_cli profile usage --summary-json
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 1: profile usage --summary-json exited $STATUS"; }
jq -e . "$WORK/stdout" > /dev/null || { cat "$WORK/stdout" >&2; fail "case 1: the output is not JSON"; }
SUMMARY="$(jq -c '.summary' "$WORK/stdout")"
jq -e 'type == "object" and has("rate_login_weeks_per_hour")' <<< "$SUMMARY" > /dev/null \
  || { jq . "$WORK/stdout" >&2; fail "case 1: no summary object with rate_login_weeks_per_hour"; }
RATE="$(jq -r .rate_login_weeks_per_hour <<< "$SUMMARY")"
RUNWAY="$(jq -r .runway_hours <<< "$SUMMARY")"
BEYOND="$(jq -r .runway_beyond_24h <<< "$SUMMARY")"
WITH="$(jq -r .runway_with_renewals_hours <<< "$SUMMARY")"
LOWEST="$(jq -r .lowest_room_login_weeks <<< "$SUMMARY")"
RESETS="$(jq -r '.weekly_resets_next_24h | length' <<< "$SUMMARY")"
echo "  | rate_login_weeks_per_hour=$RATE runway_hours=$RUNWAY runway_with_renewals_hours=$WITH"
echo "  | runway_beyond_24h=$BEYOND lowest_room_login_weeks=$LOWEST weekly_resets_next_24h=$RESETS"
[[ "$RATE" != null ]] || fail "case 1: the rate is unknown"
jq -e '.rate_login_weeks_per_hour > 0.2 and .rate_login_weeks_per_hour < 0.5' <<< "$SUMMARY" > /dev/null \
  || fail "case 1: the rate $RATE is not between 0.2 and 0.5 login-weeks an hour"
[[ "$BEYOND" == true ]] || fail "case 1: runway_beyond_24h is $BEYOND, not true"
[[ "$WITH" == null ]] || fail "case 1: runway_with_renewals_hours is $WITH, though the room lasts beyond 24 h"
jq -e '.lowest_room_login_weeks > 0' <<< "$SUMMARY" > /dev/null \
  || fail "case 1: the lowest room $LOWEST is not above zero"
[[ "$RUNWAY" != null ]] || fail "case 1: runway_hours is missing"
[[ "$RESETS" -ge 1 ]] || fail "case 1: no weekly reset listed in the next 24 h"
[[ ! -s "$WORK/claude-calls" ]] || fail "case 1: claude was started for a probe"
echo "PASS: case 1: the rate is about 0.3 and the room lasts beyond 24 h with the renewals"

# --- Case 2: the plain text --------------------------------------------------
run_cli profile usage
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 2: profile usage exited $STATUS"; }
grep -qi "runway" "$WORK/stdout" || { cat "$WORK/stdout" >&2; fail "case 2: no runway line in the text"; }
if grep -i "runway" "$WORK/stdout" | grep -qE '~[0-9]+(\.[0-9])? h'; then
  grep -i runway "$WORK/stdout" >&2
  fail "case 2: the runway line gives an hours figure"
fi
echo "PASS: case 2: the runway line gives no hours figure"
grep -iE "rate|runway" "$WORK/stdout" | sed 's/^/  | /'

# --- Case 3: the rows --------------------------------------------------------
run_cli profile usage --json
[[ $STATUS -eq 0 ]] || { cat "$WORK/stderr" >&2; fail "case 3: profile usage --json exited $STATUS"; }
ROWS="$(jq 'if type == "array" then length else -1 end' "$WORK/stdout")" \
  || { cat "$WORK/stdout" >&2; fail "case 3: the rows are not JSON"; }
[[ "$ROWS" -ge 15 ]] || fail "case 3: expected at least 15 rows, found $ROWS"
echo "PASS: case 3: --json gives $ROWS rows as one array"
echo "PASS: issue-3258-usage-rate-runway"
