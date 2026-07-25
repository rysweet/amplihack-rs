#!/usr/bin/env bash
set -uo pipefail
SIGNAL_CLI="${SIGNAL_CLI:-$HOME/.local/bin/signal-cli}"
SIGNAL_CLI_REMOTE="/home/azureuser/.local/bin/signal-cli"
RESOURCE_GROUP="${SIGNAL_SETUP_RG:-rysweet-linux-vm-pool}"
DAEMON_TCP="${SIGNAL_SETUP_DAEMON_TCP:-127.0.0.1:7583}"
QR_MARGIN=2
WINDOW_SECONDS=55   # advertise a hair under 60 so the user is never late
# Linkage-verification budget. Decoupled from the QR window on purpose: a single
# remote `az vm run-command` poll can itself take up to AZ_RUN_TIMEOUT_SECONDS
# (~90s), so a 55s window yields only ~1 real remote attempt and can trip a
# spurious "not verified" re-mint even though linking succeeded. Operators on
# slow remote paths should raise SIGNAL_SETUP_VERIFY_TIMEOUT_SECONDS.
VERIFY_TIMEOUT_SECONDS="${SIGNAL_SETUP_VERIFY_TIMEOUT_SECONDS:-$WINDOW_SECONDS}"
AZ_RUN_TIMEOUT_SECONDS="${SIGNAL_SETUP_AZ_TIMEOUT_SECONDS:-90}"
LOCAL_SIGNAL_TIMEOUT_SECONDS="${SIGNAL_SETUP_LOCAL_TIMEOUT_SECONDS:-10}"
DAEMON_WAIT_ATTEMPTS="${SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS:-20}"
RPC_TIMEOUT_SECONDS="${SIGNAL_SETUP_RPC_TIMEOUT_SECONDS:-15}"
HOST=""
PHONE="${SIGNAL_PHONE:-}"
GROUP_NAME="amplihack"
MODE=""             # local | remote (auto-detected if empty)
DO_DAEMON=1
ASSUME_YES=0
DAEMON_UNIT=""
export PATH="$HOME/.local/bin:$PATH"
info() { printf '\033[0;36m%s\033[0m\n' "$*" >&2; }
ok()   { printf '\033[0;32m%s\033[0m\n' "$*" >&2; }
warn() { printf '\033[0;33m%s\033[0m\n' "$*" >&2; }
err()  { printf '\033[0;31m%s\033[0m\n' "$*" >&2; }
die()  { err "!! $*"; exit 1; }
usage() {
  cat >&2 <<'USAGE'
signal-setup.sh — link a host to Signal for amplihack (end-to-end, idempotent).
USAGE:
  signal-setup.sh --host <name> [options]
OPTIONS:
  --host <name>        Target host. "local"/current hostname => mint locally.
                       Any other name => remote azlin VM (via az run-command).
  --phone <+E164>      Signal phone number for verify/daemon/group steps.
                       Falls back to $SIGNAL_PHONE. Required for daemon/group.
  --group <name>       Self-group name for the post-test (default: amplihack).
  --resource-group <rg>  Azure RG for remote hosts (default: rysweet-linux-vm-pool).
  --local              Force local mint.
  --remote             Force remote mint.
  --no-daemon          Skip the daemon + self-group + post-test step.
  --daemon             Force the daemon + self-group + post-test step (default).
  -y, --yes            Non-interactive: assume the phone scan screen is ready.
  -h, --help           Show this help.
ENVIRONMENT:
  SIGNAL_SETUP_VERIFY_TIMEOUT_SECONDS  Linkage-verify budget (default: 55).
                       Raise on slow remote hosts where one az run-command poll
                       can take ~90s, to avoid a spurious re-mint.
USAGE
}
# JSON-escape a raw string for safe embedding inside a JSON string value.
json_escape() { # json_escape <raw>
  local s="$1"
  s="${s//\\/\\\\}"   # backslash first
  s="${s//\"/\\\"}"   # double-quote
  s="${s//$'\n'/\\n}" # newline
  s="${s//$'\r'/\\r}" # carriage return
  s="${s//$'\t'/\\t}" # tab
  printf '%s' "$s"
}
# Build a JSON-RPC request line for the signal-cli TCP daemon.
rpc() { # rpc <method> <params-json>
  printf '{"jsonrpc":"2.0","method":"%s","params":%s,"id":1}\n' "$1" "$2"
}
# True iff the signal-cli JSON-RPC daemon is accepting connections on DAEMON_TCP.
# Derives the /dev/tcp path from DAEMON_TCP so the endpoint is defined once.
daemon_up() {
  [ "${SIGNAL_SETUP_TEST_DAEMON_UP:-0}" = "1" ] && return 0
  (exec 3<>"/dev/tcp/${DAEMON_TCP/:/\/}") 2>/dev/null
}
# Run a command ON the remote target host via az run-command. Captures FULL
# output first (never pipes az/azlin into an early-closing reader — SIGPIPE
# core-dumps azlin), then returns it for the caller to filter.
remote_run() {
  local script="$1" out
  out="$(timeout "$AZ_RUN_TIMEOUT_SECONDS" az vm run-command invoke -g "$RESOURCE_GROUP" -n "$HOST" \
    --command-id RunShellScript --scripts "$script" \
    --query 'value[0].message' -o tsv 2>&1)"
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    [ -n "$out" ] && printf '%s\n' "$out" >&2
    return "$rc"
  fi
  printf '%s' "$out"
}
# --------------------------------------------------------------------------- #
# Argument parsing
# --------------------------------------------------------------------------- #
while [ $# -gt 0 ]; do
  case "$1" in
    --host)           HOST="${2:-}"; shift 2 ;;
    --phone)          PHONE="${2:-}"; shift 2 ;;
    --group)          GROUP_NAME="${2:-}"; shift 2 ;;
    --resource-group) RESOURCE_GROUP="${2:-}"; shift 2 ;;
    --local)          MODE="local"; shift ;;
    --remote)         MODE="remote"; shift ;;
    --no-daemon)      DO_DAEMON=0; shift ;;
    --daemon)         DO_DAEMON=1; shift ;;
    -y|--yes)         ASSUME_YES=1; shift ;;
    -h|--help)        usage; exit 0 ;;
    *) die "unknown argument: $1 (use --help)" ;;
  esac
done
[ -n "$HOST" ] || { usage; die "--host is required"; }
# --------------------------------------------------------------------------- #
# Input validation — fail closed. These values flow into shell command lines,
# az run-command payloads (executed as root remotely), and JSON-RPC strings,
# so they MUST be strictly constrained to prevent command/argument injection.
# --------------------------------------------------------------------------- #
validate() { # validate <label> <value> <regex>
  case "$2" in
    "") die "$1 must not be empty" ;;
  esac
  # Bash's [[ =~ ]] applies the ERE natively, avoiding a grep subprocess per
  # call. $3 stays unquoted so it is treated as a pattern, not a literal.
  [[ "$2" =~ $3 ]] \
    || die "$1 contains invalid characters: '$2' (allowed: $3)"
}
# Hostnames / VM names: DNS-label + Azure resource charset.
validate "--host" "$HOST" '^[A-Za-z0-9._-]+$'
# Azure resource group naming charset.
validate "--resource-group" "$RESOURCE_GROUP" '^[A-Za-z0-9._()-]+$'
# Self-group name: printable, no shell/JSON metacharacters or whitespace tricks.
validate "--group" "$GROUP_NAME" '^[A-Za-z0-9._ -]+$'
# Phone (when provided): strict E.164.
if [ -n "$PHONE" ]; then
  validate "--phone" "$PHONE" '^\+[1-9][0-9]{7,14}$'
fi
# Daemon JSON-RPC endpoint: MUST stay loopback-only. The signal-cli JSON-RPC
# daemon is UNAUTHENTICATED (SECURITY.md §6 / §10.4 / T5), so its ONLY security
# boundary is the network binding. SIGNAL_SETUP_DAEMON_TCP flows verbatim into
# `daemon --tcp`, /dev/tcp probes, and nc connections, so an operator setting a
# routable host (e.g. 0.0.0.0:7583) would expose full send/receive control of
# the linked account to the network — the exact threat SECURITY.md claims is
# mitigated. Fail closed: loopback host + numeric port only. Reject IPv6 /
# multi-colon forms, which would also break the host:port parsing that
# daemon_up() and the nc invocations rely on (single-colon assumption).
case "$DAEMON_TCP" in
  *:*:*) die "SIGNAL_SETUP_DAEMON_TCP must be host:port (got '$DAEMON_TCP'); IPv6 is unsupported to preserve the loopback-only invariant (SECURITY.md §6)" ;;
  *:*)   : ;;
  *)     die "SIGNAL_SETUP_DAEMON_TCP must be host:port (got '$DAEMON_TCP')" ;;
esac
DAEMON_TCP_HOST="${DAEMON_TCP%:*}"
DAEMON_TCP_PORT="${DAEMON_TCP##*:}"
case "$DAEMON_TCP_HOST" in
  127.0.0.1|localhost) : ;;
  *) die "SIGNAL_SETUP_DAEMON_TCP host must be loopback (127.0.0.1 or localhost); refusing to bind the unauthenticated daemon to '$DAEMON_TCP_HOST' (SECURITY.md §6/§10.4, threat T5)" ;;
esac
validate "SIGNAL_SETUP_DAEMON_TCP port" "$DAEMON_TCP_PORT" '^[1-9][0-9]{0,4}$'
[ "$DAEMON_TCP_PORT" -le 65535 ] \
  || die "SIGNAL_SETUP_DAEMON_TCP port out of range (1-65535): $DAEMON_TCP_PORT"
# Numeric loop/timeout tunables: these flow UNQUOTED into the root-executed
# remote payload (`seq 1 $DAEMON_WAIT_ATTEMPTS`, `timeout $RPC_TIMEOUT_SECONDS`),
# so constrain them to a bare positive integer for parity with the port rule
# and to close the self-injection path (SECURITY.md T4/§5, defense-in-depth).
validate "SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS" "$DAEMON_WAIT_ATTEMPTS" '^[1-9][0-9]{0,3}$'
validate "SIGNAL_SETUP_RPC_TIMEOUT_SECONDS" "$RPC_TIMEOUT_SECONDS" '^[1-9][0-9]{0,3}$'
# Auto-detect mode if not forced.
if [ -z "$MODE" ]; then
  if [ "$HOST" = "local" ] || [ "$HOST" = "localhost" ] || [ "$HOST" = "$(hostname)" ]; then
    MODE="local"
  else
    MODE="remote"
  fi
fi
NAME="amplihack-$HOST"
UNIT="sig-link-$HOST"
DAEMON_UNIT="sig-daemon-$HOST"
# Secret paths use an unguessable per-run token; systemd units pin UMask=0077.
umask 077
# Prefer a 128-bit CSPRNG token from /dev/urandom so the /tmp path is not merely
# hard-to-guess but computationally infeasible to pre-create (defeats the classic
# symlink pre-plant on BOTH the local and remote paths — SECURITY.md T2). Falls
# back to the epoch/PID/$RANDOM composite only if /dev/urandom is unreadable.
RUN_TOKEN="$(LC_ALL=C tr -dc 'a-f0-9' </dev/urandom 2>/dev/null | head -c 32)"
[ "${#RUN_TOKEN}" -ge 32 ] || RUN_TOKEN="${EPOCHSECONDS}-$$-${RANDOM}${RANDOM}${RANDOM}"
URI_FILE="/tmp/slink-${HOST}-${RUN_TOKEN}.out"
LOG_FILE="/tmp/scli-${HOST}-${RUN_TOKEN}.log"
DAEMON_LOG="/tmp/signal-daemon-${HOST}-${RUN_TOKEN}.log"
# Flipped to 1 only on a fully successful run. Governs whether the trace/daemon
# logs (which hold identity material but NOT the sgnl:// link secret) are purged
# or retained-0600-for-debugging by cleanup_secrets. The link secret (URI_FILE)
# is ALWAYS purged regardless of outcome.
RUN_SUCCEEDED=0
cleanup_secrets() {
  # The sgnl:// provisioning secret (URI_FILE) is unconditionally purged — it is
  # the crown jewel and must never survive the process.
  # The -vv trace (LOG_FILE) and the local daemon log (DAEMON_LOG) contain no
  # link secret; on FAILURE we retain them (already 0600) and print the path so
  # a hard-to-reproduce, ~60s-window link failure stays debuggable. On success
  # they are purged (SECURITY.md §7).
  if [ "$MODE" = "local" ]; then
    rm -f "$URI_FILE" 2>/dev/null
    sudo rm -f "$URI_FILE" 2>/dev/null
    if [ "$RUN_SUCCEEDED" -eq 1 ]; then
      rm -f "$LOG_FILE" "$DAEMON_LOG" 2>/dev/null
      sudo rm -f "$LOG_FILE" "$DAEMON_LOG" 2>/dev/null
    else
      local _f
      for _f in "$LOG_FILE" "$DAEMON_LOG"; do
        [ -s "$_f" ] && warn "  Retained on failure for debugging (0600): $_f"
      done
    fi
  else
    remote_run "rm -f $URI_FILE" >/dev/null 2>&1
    if [ "$RUN_SUCCEEDED" -eq 1 ]; then
      remote_run "rm -f $LOG_FILE" >/dev/null 2>&1
    else
      warn "  Retained remote trace for debugging (0600 on $HOST): $LOG_FILE"
    fi
  fi
}
trap cleanup_secrets EXIT INT TERM
info "=== signal-setup: host=$HOST mode=$MODE unit=$UNIT ==="
# --------------------------------------------------------------------------- #
# Step 1: Prerequisites
# --------------------------------------------------------------------------- #
check_prereqs_local() {
  info "[1/6] Checking prerequisites (local)..."
  [ -x "$SIGNAL_CLI" ] || die "signal-cli not found/executable at $SIGNAL_CLI (known-good: 0.14.5 at ~/.local/opt/signal-cli-0.14.5/bin/signal-cli symlinked to ~/.local/bin/signal-cli)"
  command -v qrencode >/dev/null 2>&1 || die "qrencode not installed (apt-get install -y qrencode)"
  command -v systemd-run >/dev/null 2>&1 || die "systemd-run not available on this host"
  sudo -n true >/dev/null 2>&1 || die "passwordless sudo is required for the local systemd-run link unit"
  ok "  signal-cli: $("$SIGNAL_CLI" --version 2>/dev/null || echo present)"
  ok "  qrencode + systemd-run present"
}
check_prereqs_remote() {
  info "[1/6] Checking prerequisites (remote: $HOST)..."
  command -v az >/dev/null 2>&1 || die "az CLI not installed (required for remote hosts)"
  # We render the QR LOCALLY, so qrencode must also exist locally.
  command -v qrencode >/dev/null 2>&1 || die "qrencode not installed locally (needed to render the remote QR)"
  local out
  out="$(remote_run "test -x $SIGNAL_CLI_REMOTE && echo SIGCLI_OK; command -v systemd-run >/dev/null 2>&1 && echo SYSTEMD_OK")" \
    || die "az vm run-command failed for $HOST in $RESOURCE_GROUP"
  case "$out" in
    *SIGCLI_OK*) : ;;
    *) die "signal-cli missing on $HOST at $SIGNAL_CLI_REMOTE" ;;
  esac
  case "$out" in
    *SYSTEMD_OK*) : ;;
    *) die "systemd-run missing on $HOST" ;;
  esac
  ok "  remote signal-cli + systemd-run present; local qrencode present"
}
# --------------------------------------------------------------------------- #
# Step 2: Idempotency — already linked?
# --------------------------------------------------------------------------- #
already_linked() {
  # Prints the linked number if the host already has an account, else nothing.
  local accounts
  if [ "$MODE" = "local" ]; then
    accounts="$(timeout "$LOCAL_SIGNAL_TIMEOUT_SECONDS" "$SIGNAL_CLI" listAccounts 2>&1)" || return 2
  else
    accounts="$(remote_run "out=\$($SIGNAL_CLI_REMOTE listAccounts 2>&1); rc=\$?; if [ \$rc -ne 0 ]; then echo __SIGNAL_CLI_FAILED__\$rc; printf '%s\n' \"\$out\"; else printf '%s\n' \"\$out\"; fi")" || return 2
    case "$accounts" in
      *__SIGNAL_CLI_FAILED__*) printf '%s\n' "$accounts" >&2; return 2 ;;
    esac
  fi
  # Prefer an explicit phone match when --phone given; otherwise any Number line.
  # NOTE: a non-matching [[ ]] test must NOT leak exit status 1 as the function's
  # return code — the caller ("$(already_linked)" || die) reserves non-zero for
  # a genuine probe FAILURE (return 2 above). "Probe succeeded, not linked yet"
  # is success with empty output, so force an explicit return 0 below.
  if [ -n "$PHONE" ]; then
    # Match the WHOLE extracted number, not a substring. A bare
    # `*"Number: $PHONE"*` test collides on prefixes: a host linked to
    # +155512345678 would falsely match --phone +15551234567, skip minting,
    # and then drive the daemon/send steps with the wrong (unlinked) number.
    printf '%s\n' "$accounts" \
      | sed -n 's/.*Number: \(+[0-9][0-9]*\).*/\1/p' \
      | grep -qxF "$PHONE" && printf '%s' "$PHONE"
  else
    local numbers count
    numbers="$(printf '%s\n' "$accounts" | sed -n 's/.*Number: \(+[0-9][0-9]*\).*/\1/p')"
    count="$(printf '%s\n' "$numbers" | sed '/^$/d' | wc -l | tr -d ' ')"
    [ "$count" -le 1 ] || { err "Multiple Signal accounts found; rerun with --phone to choose one."; return 2; }
    printf '%s\n' "$numbers" | head -n1
  fi
  return 0
}
# --------------------------------------------------------------------------- #
# Step 3: Mint the link URI under a transient systemd unit
# --------------------------------------------------------------------------- #
mint_local() {
  local run_uid run_gid run_home
  run_uid="$(id -u)"
  run_gid="$(id -g)"
  run_home="$HOME"
  systemctl --user reset-failed "$UNIT" 2>/dev/null
  sudo systemctl reset-failed "$UNIT" 2>/dev/null
  sudo systemctl stop "$UNIT" 2>/dev/null
  sudo rm -f "$URI_FILE" "$LOG_FILE"
  local launch_out launch_rc
  launch_out="$(sudo systemd-run --unit="$UNIT" --uid="$run_uid" --gid="$run_gid" \
    --property=UMask=0077 \
    --setenv=HOME="$run_home" \
    --setenv=PATH="$run_home/.local/bin:/usr/bin:/bin" \
    /bin/bash -c '"$1" -vv --log-file "$2" link -n "$3" > "$4" 2>&1' \
      bash "$SIGNAL_CLI" "$LOG_FILE" "$NAME" "$URI_FILE" \
    2>&1)"
  launch_rc=$?
  if [ "$launch_rc" -ne 0 ]; then
    [ -n "$launch_out" ] && printf '%s\n' "$launch_out" >&2
    return 1
  fi
  local _i
  for ((_i = 1; _i <= DAEMON_WAIT_ATTEMPTS; _i++)); do
    grep -q '^sgnl://' "$URI_FILE" 2>/dev/null && break
    sleep 0.5
  done
  grep -m1 '^sgnl://' "$URI_FILE" 2>/dev/null
}
mint_remote() {
  # run-command runs as ROOT, so --uid/--gid=azureuser is REQUIRED so the
  # linked account lands under the azureuser home, not root's.
  local script out
  script="$(cat <<REMOTE
umask 077
systemctl reset-failed $UNIT 2>/dev/null
systemctl stop $UNIT 2>/dev/null
rm -f $URI_FILE $LOG_FILE
launch_out=\$(systemd-run --unit=$UNIT --uid=azureuser --gid=azureuser \
  --property=UMask=0077 \
  --setenv=HOME=/home/azureuser \
  --setenv=PATH=/home/azureuser/.local/bin:/usr/bin:/bin \
  /bin/bash -c '$SIGNAL_CLI_REMOTE -vv --log-file $LOG_FILE link -n $NAME > $URI_FILE 2>&1' 2>&1)
launch_rc=\$?
if [ "\$launch_rc" -ne 0 ]; then echo LINK_LAUNCH_FAILED; echo "\$launch_out"; exit 0; fi
for i in \$(seq 1 $DAEMON_WAIT_ATTEMPTS); do grep -q '^sgnl://' $URI_FILE 2>/dev/null && break; sleep 0.5; done
echo URI_START; grep -m1 '^sgnl://' $URI_FILE 2>/dev/null; echo URI_END
REMOTE
)"
  # Capture FULL output first (SIGPIPE gotcha), then extract.
  out="$(remote_run "$script")" || return 1
  case "$out" in
    *LINK_LAUNCH_FAILED*) printf '%s\n' "$out" >&2; return 1 ;;
  esac
  printf '%s\n' "$out" | sed -n 's/.*URI_START//p; /sgnl:\/\//p' | grep -m1 '^sgnl://'
}
# --------------------------------------------------------------------------- #
# Step 4: Verify linkage
# --------------------------------------------------------------------------- #
verify_linkage() {
  info "[4/6] Verifying linkage (up to ${VERIFY_TIMEOUT_SECONDS}s)..."
  # $EPOCHSECONDS is a fork-free bash builtin; using it instead of $(date +%s)
  # avoids ~2 subprocess forks per poll (~110 over the full window) in this hot
  # loop without changing behaviour.
  local num unit_state deadline
  deadline=$(( EPOCHSECONDS + VERIFY_TIMEOUT_SECONDS ))
  while (( EPOCHSECONDS < deadline )); do
    num="$(already_linked)" || { warn "  Could not query linked accounts yet; retrying."; num=""; }
    if [ -n "$num" ]; then
      # The transient unit exits (inactive) on success.
      if [ "$MODE" = "local" ]; then
        unit_state="$(systemctl is-active "$UNIT" 2>/dev/null)"
      else
        unit_state="$(remote_run "systemctl is-active $UNIT 2>/dev/null")"
      fi
      ok "  Linked: $num  (unit $UNIT: ${unit_state:-inactive})"
      [ -z "$PHONE" ] && PHONE="$num"
      return 0
    fi
    (( EPOCHSECONDS < deadline )) || break
    sleep 1
  done
  err "  No linked account detected within the window."
  err "  Trace log on host: $LOG_FILE"
  err "  Look for: 'Associated with: +<phone>' then 'Finishing new device registration'."
  return 1
}
# --------------------------------------------------------------------------- #
# Step 5+6: Daemon + self-group + post-test (JSON-RPC on 127.0.0.1:7583)
# --------------------------------------------------------------------------- #
remote_daemon_group_posttest() {
  local sigcli="$1" acct_j group_j script out
  acct_j="$(json_escape "$PHONE")"
  group_j="$(json_escape "$GROUP_NAME")"
  # Honour the validated SIGNAL_SETUP_DAEMON_TCP override on the remote path too.
  # Both values passed validation (loopback host + numeric 1-65535 port), so
  # they are safe to interpolate into the run-command payload. Previously the
  # remote heredoc hardcoded 127.0.0.1:7583, so a non-default override worked
  # locally but was silently ignored remotely (while [5/6] still printed the
  # unused $DAEMON_TCP).
  script="$(cat <<REMOTE
daemon_up() { (exec 3<>/dev/tcp/$DAEMON_TCP_HOST/$DAEMON_TCP_PORT) 2>/dev/null; }
if ! daemon_up; then
  systemctl reset-failed $DAEMON_UNIT 2>/dev/null
  systemd-run --unit=$DAEMON_UNIT --uid=azureuser --gid=azureuser \
    --property=UMask=0077 \
    --setenv=HOME=/home/azureuser \
    --setenv=PATH=/home/azureuser/.local/bin:/usr/bin:/bin \
    $sigcli -a '$acct_j' daemon --tcp $DAEMON_TCP_HOST:$DAEMON_TCP_PORT >/dev/null 2>&1
  for i in \$(seq 1 $DAEMON_WAIT_ATTEMPTS); do daemon_up && break; sleep 0.5; done
fi
daemon_up || { echo DAEMON_DOWN; exit 0; }
command -v nc >/dev/null 2>&1 || { echo NC_MISSING; exit 0; }
resp=\$(printf '%s\n' '{"jsonrpc":"2.0","method":"updateGroup","params":{"account":"$acct_j","name":"$group_j"},"id":1}' | timeout $RPC_TIMEOUT_SECONDS nc $DAEMON_TCP_HOST $DAEMON_TCP_PORT 2>/dev/null | head -n1)
group_id=\$(printf '%s' "\$resp" | sed -n 's/.*"groupId":"\([^"]*\)".*/\1/p')
[ -n "\$group_id" ] || { echo GROUP_ID_MISSING; echo "\$resp"; exit 0; }
group_id_j=\$(printf '%s' "\$group_id" | sed 's/\\/\\\\/g; s/"/\\"/g')
resp=\$(printf '{"jsonrpc":"2.0","method":"send","params":{"account":"$acct_j","groupId":"%s","message":"amplihack signal-setup: link verified"},"id":1}\n' "\$group_id_j" | timeout $RPC_TIMEOUT_SECONDS nc $DAEMON_TCP_HOST $DAEMON_TCP_PORT 2>/dev/null | head -n1)
case "\$resp" in
  *'"results":[]'*) echo POST_TEST_OK ;;
  *) echo POST_TEST_UNKNOWN; echo "\$resp" ;;
esac
REMOTE
)"
  out="$(remote_run "$script")" || { warn "  Remote daemon/group/post-test failed to run."; return 1; }
  case "$out" in
    *POST_TEST_OK*) ok "  Remote daemon reachable; post-test OK: {\"results\":[],...}" ;;
    *DAEMON_DOWN*) warn "  Remote daemon did not come up on $DAEMON_TCP."; return 1 ;;
    *NC_MISSING*) warn "  Remote 'nc' not available; cannot run JSON-RPC self-group post-test."; return 1 ;;
    *GROUP_ID_MISSING*) warn "  Remote updateGroup did not return groupId: $out"; return 1 ;;
    *) warn "  Remote post-test response (verify manually): $out"; return 1 ;;
  esac
}
local_daemon_group_posttest() {
  local sigcli="$1"
  if ! daemon_up; then
    # Parity with the remote daemon path: supervise the daemon under a transient
    # systemd unit (reset-failed + UMask=0077) instead of a bare `&` background
    # job, so the local and remote paths get identical lifecycle handling and
    # owner-only isolation for any state the daemon writes. systemd-run's own
    # launch output is captured to DAEMON_LOG for the failure diagnostic below;
    # the daemon's runtime output goes to journald under $DAEMON_UNIT.
    sudo systemctl reset-failed "$DAEMON_UNIT" 2>/dev/null
    # SC2024: the redirect is intentionally the caller's, not root's — DAEMON_LOG
    # captures systemd-run's launch message as an owner-only (umask 077) file;
    # the daemon's own runtime output is journald-captured under $DAEMON_UNIT.
    # shellcheck disable=SC2024
    sudo systemd-run --unit="$DAEMON_UNIT" --uid="$(id -u)" --gid="$(id -g)" \
      --property=UMask=0077 \
      --setenv=HOME="$HOME" \
      --setenv=PATH="$HOME/.local/bin:/usr/bin:/bin" \
      "$sigcli" -a "$PHONE" daemon --tcp "$DAEMON_TCP" >"$DAEMON_LOG" 2>&1
    local _i
    for ((_i = 1; _i <= DAEMON_WAIT_ATTEMPTS; _i++)); do
      daemon_up && break
      sleep 0.5
    done
  fi
  daemon_up \
    || { warn "  Daemon did not come up on $DAEMON_TCP.";
         if [ -s "$DAEMON_LOG" ]; then
           warn "  systemd-run launch log ($DAEMON_LOG):"; tail -n 20 "$DAEMON_LOG" >&2
         fi
         # The daemon runs under a transient systemd unit, so its own runtime
         # output is journald-captured (not in DAEMON_LOG). Point the operator at
         # it so a link/daemon failure inside the ~60s window stays debuggable.
         warn "  Daemon runtime output is journald-captured; inspect with:";
         warn "    sudo journalctl -u $DAEMON_UNIT -n 50 --no-pager";
         return 1; }
  ok "  Daemon reachable on $DAEMON_TCP"
  command -v nc >/dev/null 2>&1 || { warn "  'nc' not available; cannot run JSON-RPC self-group post-test."; return 1; }
  info "[6/6] Ensuring self-group '$GROUP_NAME' + post-test ..."
  local resp group_id acct_j group_j nc_host nc_port
  nc_host="${DAEMON_TCP%:*}"; nc_port="${DAEMON_TCP##*:}"
  acct_j="$(json_escape "$PHONE")"
  group_j="$(json_escape "$GROUP_NAME")"
  resp="$(rpc updateGroup "{\"account\":\"$acct_j\",\"name\":\"$group_j\"}" \
    | timeout "$RPC_TIMEOUT_SECONDS" nc "$nc_host" "$nc_port" 2>/dev/null | head -n1)"
  group_id="$(printf '%s' "$resp" | sed -n 's/.*"groupId":"\([^"]*\)".*/\1/p')"
  if [ -z "$group_id" ]; then
    warn "  Could not obtain groupId from updateGroup response:"
    warn "    $resp"
    return 1
  fi
  ok "  self-group id: $group_id"

  local gid_j
  gid_j="$(json_escape "$group_id")"
  resp="$(rpc send "{\"account\":\"$acct_j\",\"groupId\":\"$gid_j\",\"message\":\"amplihack signal-setup: link verified\"}" \
    | timeout "$RPC_TIMEOUT_SECONDS" nc "$nc_host" "$nc_port" 2>/dev/null | head -n1)"
  # Empty results array is NORMAL/success for a self-only group.
  if printf '%s' "$resp" | grep -q '"results":\[\]'; then
    ok "  Post-test OK: {\"results\":[],...} (empty results = success for self-group)"
  else
    warn "  Post-test response (verify manually): $resp"
    return 1
  fi
}

daemon_group_posttest() {
  [ -n "$PHONE" ] || { warn "  --phone not set; skipping daemon/group/post-test."; return 0; }
  info "[5/6] Ensuring JSON-RPC daemon on $DAEMON_TCP ..."
  if [ "$MODE" = "remote" ]; then
    remote_daemon_group_posttest "$SIGNAL_CLI_REMOTE"
  else
    local_daemon_group_posttest "$SIGNAL_CLI"
  fi
}

# --------------------------------------------------------------------------- #
# Main
# --------------------------------------------------------------------------- #
main() {
  if [ "$MODE" = "local" ]; then check_prereqs_local; else check_prereqs_remote; fi

  # Idempotency: if already linked, do not re-mint.
  local existing
  existing="$(already_linked)" \
    || die "Could not inspect existing Signal accounts on $HOST; refusing to mint a fresh link until the account probe succeeds."
  if [ -n "$existing" ]; then
    ok "[2/6] Host already linked as $existing — nothing to do (idempotent)."
    [ -z "$PHONE" ] && PHONE="$existing"
    if [ "$DO_DAEMON" -eq 1 ]; then
      daemon_group_posttest || die "Daemon/self-group/post-test failed."
    fi
    ok "=== signal-setup complete (already linked) ==="
    RUN_SUCCEEDED=1
    return 0
  fi

  # Prompt: the phone MUST be on the scan screen BEFORE we mint (60s window).
  info "[3/6] Prepare your phone: Signal > Settings > Linked Devices > 'Link New Device'."
  info "      Get to the CAMERA / scan screen NOW."
  if [ "$ASSUME_YES" -ne 1 ]; then
    printf '\033[0;36mAre you on the scan screen and ready? [y/N] \033[0m' >&2
    local answer=""
    read -r answer </dev/tty || answer=""
    case "$answer" in
      y|Y|yes|YES) : ;;
      *) die "Aborted — re-run when you are on the scan screen." ;;
    esac
  fi

  info "  Minting fresh link (unit $UNIT)..."
  local uri
  if [ "$MODE" = "local" ]; then uri="$(mint_local)"; else uri="$(mint_remote)"; fi
  [ -n "${uri:-}" ] || die "Failed to obtain a link URI for $HOST. Check signal-cli/systemd-run on the host (trace: $LOG_FILE)."

  # Zero-latency delivery: QR to the TERMINAL only, inverted for dark backgrounds.
  printf '\n' >&2
  printf '############################################################\n' >&2
  printf '#  SCAN NOW — ~%ss until Signal closes the socket (1001)   #\n' "$WINDOW_SECONDS" >&2
  printf '#  Signal > Linked Devices > Link New Device > scan below   #\n' >&2
  printf '############################################################\n\n' >&2
  # Feed the secret via STDIN, never argv: passing the sgnl:// URI as a
  # command-line argument would expose the crown-jewel link secret in
  # qrencode's argv (visible to other local users via `ps` / /proc/<pid>/cmdline)
  # for the whole render. qrencode reads the data from stdin when no string
  # argument is given. printf '%s' avoids appending a newline into the QR data.
  printf '%s' "$uri" | qrencode -t ANSIUTF8i -m "$QR_MARGIN"
  printf '\n(host=%s unit=%s minted=%s)\n' "$HOST" "$UNIT" "$(date -u +%H:%M:%SZ)" >&2

  # The URI is now on-screen; delete the on-disk copy of the secret immediately
  # rather than leaving it readable for the whole provisioning window.
  if [ "$MODE" = "local" ]; then
    rm -f "$URI_FILE" 2>/dev/null; sudo rm -f "$URI_FILE" 2>/dev/null
  else
    remote_run "rm -f $URI_FILE" >/dev/null 2>&1
  fi
  uri=""

  verify_linkage || die "Linkage not verified. Re-run to mint a fresh QR (the old one has expired)."

  if [ "$DO_DAEMON" -eq 1 ]; then
    daemon_group_posttest || die "Daemon/self-group/post-test failed."
  fi

  ok "=== signal-setup complete for $HOST ==="
  RUN_SUCCEEDED=1
}

main
