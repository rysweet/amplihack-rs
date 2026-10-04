#!/usr/bin/env bash
# Issue #1525 — migrate.sh `detect_cli` must agree with the Rust resolver
# (crates/amplihack-utils/src/agent_binary.rs), which is authoritative:
#
#   * a session marker outranks any launcher_context.json, and the marker list
#     is agent_binary::SESSION_MARKERS, in the same order;
#   * a launcher context counts only while fresh (24h). A stale one is
#     passed over silently. One with no timestamp, or with one that chrono's
#     RFC 3339 parser would refuse, is unusable and named, even where
#     `date -d` would read it;
#   * the walk-up stops at a .git boundary and at a world-writable directory;
#   * an unusable file (empty, not JSON, wrong shape, unknown launcher) is
#     walked past, but named on stderr with the reason;
#   * the value is trimmed, not stripped: inner whitespace and control
#     characters reject it;
#   * without jq no file can be read, and none is blamed for it;
#   * the same holds on a BSD userland (macOS). The file-reading rows run
#     twice: once with the host's tools, once with stand-ins on PATH that
#     reject the GNU-only forms BSD lacks (`stat -c`, `date -d`, `readlink -f`)
#     and pad `wc` output as BSD does. CI runs on Linux only, so this is where
#     a GNU-only call would show. A tool that fails outright is named on
#     stderr; it never turns a valid file into a silent `copilot`.
#
# Before #1525, detect_cli read launcher_context.json ahead of any session
# evidence and had no staleness bound. Inside Claude Code, a days-old
# `launcher: copilot` file made migrate resume `copilot --resume <id>`.
#
# The crusty review of #1490 found the first version of this layer calling
# `stat -c` and `date -d`. On macOS the first `stat` failed, the walk-up
# stopped without a word, and a fresh `launcher: claude` file resolved to
# `copilot`.
#
# The shell's one extra layer, the parent process chain, is stubbed out here
# except where it is under test.

set -uo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
RESOLVER="crates/amplihack-utils/src/agent_binary.rs"
[ -f "$SCRIPT" ] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }
[ -f "$RESOLVER" ] || { echo "missing $RESOLVER (run from repo root)"; exit 1; }
command -v jq >/dev/null || { echo "  FAIL  jq is required by detect_cli's launcher-context layer"; exit 1; }

fails=0
label=""
pass() { printf '  ok    %s%s\n' "$label" "$1"; }
fail() { printf '  FAIL  %s%s\n' "$label" "$1"; fails=$((fails + 1)); }

eval "$(awk '/^log_warn\(\)/' "$SCRIPT")"
eval "$(awk '/^_?detect_cli[a-z_]*\(\) \{/,/^\}/' "$SCRIPT")"
declare -F detect_cli >/dev/null || { echo "  FAIL  detect_cli not defined"; exit 1; }

# --- the marker list must be SESSION_MARKERS, entry for entry -------------
rust_markers="$(awk '/pub const SESSION_MARKERS/,/^\];/' "$RESOLVER" \
  | sed -n 's/^ *("\([A-Z_]*\)", "\([a-z]*\)"),.*/\1:\2/p')"
shell_markers="$(awk '/local -a session_markers=\(/,/^ *\)$/' "$SCRIPT" \
  | sed -n 's/^ *\([A-Z_]*:[a-z]*\) *$/\1/p')"
if [ -n "$rust_markers" ] && [ "$rust_markers" = "$shell_markers" ]; then
  pass "shell marker list is SESSION_MARKERS ($(printf '%s\n' "$rust_markers" | wc -l | tr -d ' ') entries, same order)"
else
  fail "shell marker list differs from SESSION_MARKERS:
rust:
$rust_markers
shell:
$shell_markers"
fi

root="$(mktemp -d)"
trap 'chmod -R u+rwx "$root" 2>/dev/null; rm -rf "$root"' EXIT
now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
old="$(date -u -d '3 days ago' +%Y-%m-%dT%H:%M:%SZ)"

# fixture <dir> <body>: a project dir (with a .git boundary unless <dir> ends
# in /nogit) holding <body> as its launcher context ("" for an empty file,
# "-" for none).
fixture() {
  local dir="${1%/nogit}" body="$2"
  mkdir -p "$dir"
  [ "$dir" = "$1" ] && mkdir -p "$dir/.git"
  if [ "$body" != "-" ]; then
    mkdir -p "$dir/.claude/runtime"
    printf '%s' "$body" > "$dir/.claude/runtime/launcher_context.json"
  fi
}

# run <dir> [VAR=value ...]: detect_cli from <dir> with no markers, no agent
# binary variables and no agent CLI in the process chain, plus the given
# variables, with $userland (when set) ahead of PATH. Sets $out and $err.
userland=""
run() {
  local dir="$1"; shift
  local errfile="$root/stderr"
  out="$(
    exec 2>"$errfile"
    cd "$dir" || exit 1
    for entry in $shell_markers; do unset "${entry%%:*}"; done
    unset AMPLIHACK_AGENT_BINARY AMPLIHACK_AGENT_BINARY_SOURCE
    ps() { :; }
    [ -z "$userland" ] || PATH="$userland:$PATH"
    for assignment in "$@"; do export "${assignment?}"; done
    detect_cli
  )"
  err="$(cat "$errfile")"
}

expect() {
  local desc="$1" want="$2"
  if [ "$out" = "$want" ]; then pass "$desc -> $out"; else fail "$desc: expected $want, got $out (stderr: $err)"; fi
}

expect_warning() {
  local desc="$1" want="$2"
  case "$err" in
    *"$want"*) pass "$desc" ;;
    *) fail "$desc: stderr lacks '$want': $err" ;;
  esac
}

# --- the marker layer ranks above a fresh file ----------------------------
fixture "$root/m" "{\"launcher\":\"copilot\",\"timestamp\":\"$now\"}"
run "$root/m" CLAUDE_CODE_ENTRYPOINT=cli
expect "a Claude marker beats a fresh copilot file" claude
for entry in $shell_markers; do
  run "$root/m" "${entry%%:*}=1"
  expect "marker ${entry%%:*} alone" "${entry##*:}"
done
run "$root/m" CLAUDECODE=
expect "an empty marker is no marker" copilot

# For the offset rows: an instant 23 hours ago written at -05:00, and one 25
# hours ago written at +05:00.
west_23h="$(date -u -d '-28 hours' +%Y-%m-%dT%H:%M:%S)-05:00"
east_25h="$(date -u -d '-20 hours' +%Y-%m-%dT%H:%M:%S)+05:00"

# launcher_context_rows <base>: every row that reads a launcher context,
# with fixtures under <base>.
launcher_context_rows() {
  local base="$1"
  # --- freshness ------------------------------------------------------------
  fixture "$base/fresh" "{\"launcher\":\"claude\",\"timestamp\":\"$now\"}"
  run "$base/fresh"
  expect "a fresh file answers" claude
  fixture "$base/stale" "{\"launcher\":\"claude\",\"timestamp\":\"$old\"}"
  run "$base/stale"
  expect "a three-day-old file is ignored" copilot
  [ -z "$err" ] && pass "a stale file is not reported as broken" || fail "stale file warned: $err"
  fixture "$base/notime" '{"launcher":"claude"}'
  run "$base/notime"
  expect "a file without a timestamp is not used" copilot
  expect_warning "...and is named, because its age is unknown" \
    "ignored $base/notime/.claude/runtime/launcher_context.json: it has no timestamp, so its age is unknown."

  # The Rust resolver parses with chrono's parse_from_rfc3339. `date -d` reads
  # far more, so detect_cli once answered `claude` here while Rust answered
  # `copilot` and named nothing. Both now refuse the file and name it.
  local_time="$(date -u +'%Y-%m-%d %H:%M:%S')"
  fixture "$base/nooffset" "{\"launcher\":\"claude\",\"timestamp\":\"$local_time\"}"
  run "$base/nooffset"
  expect "a timestamp with no offset is not used, as in Rust" copilot
  expect_warning "...and is named with its reason" \
    "ignored $base/nooffset/.claude/runtime/launcher_context.json: it has a timestamp that is not RFC 3339."
  local ts
  for ts in yesterday "$(date -u +%s)" "$(date -u +%Y-%m-%dT%H:%M:%S+0000)" "$(date -u +%Y-%m-%dT%H:%M:%S+24:00)"; do
    fixture "$base/badts" "{\"launcher\":\"claude\",\"timestamp\":\"$ts\"}"
    run "$base/badts"
    expect "timestamp '$ts' is not RFC 3339" copilot
  done
  fixture "$base/ctlts" "{\"launcher\":\"claude\",\"timestamp\":\"$now\\n\"}"
  run "$base/ctlts"
  expect "a timestamp ending in a newline is not RFC 3339, as in Rust" copilot
  # ...and the forms chrono does accept are read, not refused.
  for ts in "$(date -u +'%Y-%m-%d %H:%M:%S+00:00')" "$(date -u +%Y-%m-%dt%H:%M:%S.%Nz)" \
            "$(date -u +%Y-%m-%dT%H:%M:60Z)" "$(date +%Y-%m-%dT%H:%M:%S%:z)"; do
    fixture "$base/okts" "{\"launcher\":\"claude\",\"timestamp\":\"$ts\"}"
    run "$base/okts"
    expect "RFC 3339 form '$ts' is read" claude
  done

  # --- unusable files are named, and walked past ----------------------------
  fixture "$base/empty" ""
  run "$base/empty"
  expect "an empty file falls through" copilot
  expect_warning "an empty file is named with its reason" \
    "ignored $base/empty/.claude/runtime/launcher_context.json: it is empty."
  fixture "$base/badjson" '{"launcher": "claude"'
  run "$base/badjson"
  expect "invalid JSON falls through" copilot
  expect_warning "invalid JSON is named with its reason" "it is not valid JSON."
  fixture "$base/shape" '{"launcher": 5}'
  run "$base/shape"
  expect_warning "a wrong shape is named with its reason" "it is JSON but not a launcher context"
  fixture "$base/vim" "{\"launcher\":\"vim\",\"timestamp\":\"$now\"}"
  run "$base/vim"
  expect_warning "an unknown launcher is named" "it does not name amplifier, claude, codex or copilot"
  fixture "$base/ctl" "{\"launcher\":\"claude\\n\",\"timestamp\":\"$now\"}"
  run "$base/ctl"
  expect "a launcher with a control character is rejected, as in Rust" copilot

  fixture "$base/walk" "{\"launcher\":\"codex\",\"timestamp\":\"$now\"}"
  fixture "$base/walk/sub/nogit" ""
  run "$base/walk/sub"
  expect "a bad file in a subdirectory lets the project's file answer" codex
  expect_warning "...and the bad file is still named" \
    "ignored $base/walk/sub/.claude/runtime/launcher_context.json: it is empty."

  # --- walk-up boundaries ---------------------------------------------------
  fixture "$base/above" "{\"launcher\":\"claude\",\"timestamp\":\"$now\"}"
  rm -rf "$base/above/.git"
  fixture "$base/above/repo" "-"
  run "$base/above/repo"
  expect "the walk-up stops at a .git boundary" copilot
  fixture "$base/shared/nogit" "{\"launcher\":\"claude\",\"timestamp\":\"$now\"}"
  chmod 1777 "$base/shared"
  fixture "$base/shared/work/nogit" "-"
  chmod 700 "$base/shared/work"
  run "$base/shared/work"
  expect "the walk-up stops at a world-writable directory" copilot

  # --- what reading the file needs ----------------------------------------
  # The path is resolved without `readlink -f`, the size taken without
  # `stat -c`, and the age computed without `date -d`.
  fixture "$base/dirlink" "-"
  mkdir -p "$base/dirlink/state/runtime"
  printf '{"launcher":"claude","timestamp":"%s"}' "$now" \
    > "$base/dirlink/state/runtime/launcher_context.json"
  ln -s state "$base/dirlink/.claude"
  run "$base/dirlink"
  expect "a linked .claude directory inside the project is followed" claude
  fixture "$base/link" "-"
  mkdir -p "$base/link/.claude/runtime"
  printf '{"launcher":"claude","timestamp":"%s"}' "$now" > "$base/link/.claude/context.json"
  ln -s ../context.json "$base/link/.claude/runtime/launcher_context.json"
  run "$base/link"
  expect "a link to a file inside the directory is followed" claude
  fixture "$base/escape" "-"
  mkdir -p "$base/escape/.claude/runtime"
  printf '{"launcher":"claude","timestamp":"%s"}' "$now" > "$base/outside.json"
  ln -s ../../../outside.json "$base/escape/.claude/runtime/launcher_context.json"
  run "$base/escape"
  expect "a link to a file outside the directory is not followed" copilot
  expect_warning "...and is named with its reason" \
    "ignored $base/escape/.claude/runtime/launcher_context.json: it is a link to a file outside its directory."

  local prefix="{\"launcher\":\"claude\",\"timestamp\":\"$now\",\"pad\":\""
  local pad=$((65536 - ${#prefix} - 2))
  fixture "$base/limit" "$prefix$(head -c "$pad" /dev/zero | tr '\0' x)\"}"
  run "$base/limit"
  expect "a file of exactly 64 KiB is read, as in Rust" claude
  fixture "$base/big" "$prefix$(head -c "$((pad + 1))" /dev/zero | tr '\0' x)\"}"
  run "$base/big"
  expect "a file one byte over 64 KiB is not used" copilot
  expect_warning "...and is named with its reason" "it is larger than the 64 KiB limit."

  # Offsets: a sign read the wrong way round moves each of these across the
  # 24-hour line.
  fixture "$base/west" "{\"launcher\":\"claude\",\"timestamp\":\"$west_23h\"}"
  run "$base/west"
  expect "23 hours old at -05:00 ($west_23h) is fresh" claude
  fixture "$base/east" "{\"launcher\":\"claude\",\"timestamp\":\"$east_25h\"}"
  run "$base/east"
  expect "25 hours old at +05:00 ($east_25h) is stale" copilot
  [ -z "$err" ] && pass "...and is not reported as broken" || fail "stale offset file warned: $err"
  # Calendar: chrono refuses a day the month does not have.
  for ts in 2026-02-29T00:00:00Z 2100-02-29T00:00:00Z 2026-04-31T00:00:00Z; do
    fixture "$base/nodate" "{\"launcher\":\"claude\",\"timestamp\":\"$ts\"}"
    run "$base/nodate"
    expect_warning "$ts is not a date, so not RFC 3339" "it has a timestamp that is not RFC 3339."
  done
  fixture "$base/leapday" '{"launcher":"claude","timestamp":"2024-02-29T00:00:00Z"}'
  run "$base/leapday"
  expect "2024-02-29 is a date (and stale)" copilot
  [ -z "$err" ] && pass "...so it is not reported as broken" || fail "leap day warned: $err"
}

# --- a BSD userland -------------------------------------------------------
# Stand-ins for the tools whose GNU and BSD forms differ. Each rejects the
# GNU-only form, with BSD's message, and otherwise runs the host tool. They
# model what BSD lacks, not all of BSD; `find`, `tr`, `dirname` and `jq` are
# called only in forms both provide, so they are not stood in for.
bsd="$root/bsd-bin"
mkdir -p "$bsd"
cat > "$bsd/stat" <<EOF
#!/bin/sh
# BSD stat(1) formats with -f; it has no -c and no long options.
for a in "\$@"; do
  case "\$a" in
    -c*|--*)
      echo "stat: illegal option -- c" >&2
      echo "usage: stat [-FLnq] [-f format | -l | -r | -s | -x] [-t timefmt] [file|handle ...]" >&2
      exit 1 ;;
  esac
done
exec "$(command -v stat)" "\$@"
EOF
cat > "$bsd/date" <<EOF
#!/bin/sh
# BSD date(1) parses with -j -f; it has no -d and no long options.
for a in "\$@"; do
  case "\$a" in
    --*|-*d*)
      echo "date: illegal option -- d" >&2
      echo "usage: date [-jnRu] [-I[date|hours|minutes|seconds]] [-f input_fmt] [-r filename|seconds] [-v[+|-]val[y|m|w|d|H|M|S]] [[[[mm]dd]HH]MM[[cc]yy][.SS] | new_date] [+output_fmt]" >&2
      exit 1 ;;
  esac
done
exec "$(command -v date)" "\$@"
EOF
cat > "$bsd/readlink" <<EOF
#!/bin/sh
# readlink(1) on macOS before 12.3 takes -n and nothing else.
for a in "\$@"; do
  case "\$a" in
    -n) ;;
    -*) echo "readlink: illegal option -- \${a#-}" >&2
        echo "usage: readlink [-n] [file ...]" >&2
        exit 1 ;;
  esac
done
exec "$(command -v readlink)" "\$@"
EOF
cat > "$bsd/wc" <<EOF
#!/bin/sh
# BSD wc(1) right-aligns each count in a field of eight, even reading stdin.
out="\$("$(command -v wc)" "\$@")" || exit
printf '%s\n' "\$out" | awk '{ for (i = 1; i <= NF; i++) printf "%8s", \$i; print "" }'
EOF
chmod +x "$bsd/stat" "$bsd/date" "$bsd/readlink" "$bsd/wc"
# The stand-ins must refuse what BSD refuses, or the BSD pass proves nothing.
for gnu_only in "stat -c %a /" "date -d now +%s" "readlink -f /"; do
  # shellcheck disable=SC2086  # the words are the command
  if ( PATH="$bsd:$PATH"; $gnu_only ) >/dev/null 2>&1; then
    fail "BSD stand-in accepted the GNU-only '$gnu_only'"
  else
    pass "BSD stand-in refuses '$gnu_only'"
  fi
done
case "$(printf 'abc' | PATH="$bsd:$PATH" wc -c)" in
  "       3") pass "BSD stand-in pads wc -c as BSD does" ;;
  *) fail "BSD stand-in wc -c printed '$(printf 'abc' | PATH="$bsd:$PATH" wc -c)'" ;;
esac

launcher_context_rows "$root/host"
userland="$bsd" label="[BSD userland] "
launcher_context_rows "$root/bsd"
userland="" label=""

# --- a tool that fails is named, and blames no file ----------------------
broken="$root/broken-bin"
mkdir -p "$broken"
printf '#!/bin/sh\nexit 1\n' > "$broken/find"
printf '#!/bin/sh\nexit 1\n' > "$broken/date"
chmod +x "$broken/find" "$broken/date"
for tool in find date; do
  mkdir -p "$root/no-$tool"
  ln -s "$broken/$tool" "$root/no-$tool/$tool"
done
run "$root/host/fresh" "PATH=$root/no-find:$PATH"
expect "if find fails, the directory is not trusted" copilot
expect_warning "...and the warning names the directory and the tool" \
  "could not check whether $root/host/fresh is world-writable (find failed)"
run "$root/host/fresh" "PATH=$root/no-date:$PATH"
expect "if date fails, no age can be known" copilot
expect_warning "...and the warning names the tool and the file" \
  "date +%s did not give the current time; launcher context $root/host/fresh/.claude/runtime/launcher_context.json not read."
case "$err" in
  *"Fix or delete it"*) fail "a failing date blamed the valid file: $err" ;;
  *) pass "...and the valid file is not called broken" ;;
esac

# --- the environment variable is trimmed, not stripped --------------------
run "$root/host/fresh" "AMPLIHACK_AGENT_BINARY=  Codex  "
expect "surrounding spaces are trimmed" codex
run "$root/host/fresh" "AMPLIHACK_AGENT_BINARY=co dex"
expect "inner whitespace rejects the value, as in Rust" claude

# --- without jq, nothing is blamed ---------------------------------------
# PATH holds every external command detect_cli runs, except jq.
nojq="$root/nojq-bin"
mkdir -p "$nojq"
for tool in date dirname find readlink tr wc; do
  ln -s "$(command -v "$tool")" "$nojq/$tool"
done
run "$root/host/fresh" "PATH=$nojq"
expect "without jq the launcher context is not read" copilot
expect_warning "...and the warning says jq is missing" \
  "jq not found; launcher context $root/host/fresh/.claude/runtime/launcher_context.json not read."
case "$err" in
  *"not valid JSON"*|*"Fix or delete it"*) fail "without jq, a valid file was blamed: $err" ;;
  *) pass "...and the valid file is not called broken" ;;
esac

# --- the process chain still ranks above the file -------------------------
# The stub is defined out here: bash 3.2 cannot parse a `case` pattern's `)`
# inside `$( ... )`.
claude_ancestor() {
  case "$*" in
    *comm=*) echo claude ;;
    *) echo 1 ;;
  esac
}
out="$(
  cd "$root/m" || exit 1
  for entry in $shell_markers; do unset "${entry%%:*}"; done
  unset AMPLIHACK_AGENT_BINARY AMPLIHACK_AGENT_BINARY_SOURCE
  ps() { claude_ancestor "$@"; }
  detect_cli
)"
err=""
expect "a claude ancestor beats a fresh copilot file" claude

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detect_cli parity: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detect_cli parity: all checks passed"
