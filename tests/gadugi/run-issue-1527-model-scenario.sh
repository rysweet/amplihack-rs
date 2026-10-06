#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #1527: Claude model ids in
# the dotted spelling GitHub Copilot CLI uses (`claude-opus-5.5`).
#
# This runs the real `amplihack` binary from the outside. Each case runs
# `amplihack <tool> [args]` under `env -i` with a fresh HOME. Stub `claude`,
# `copilot` and `codex` executables record the argv and environment they were
# launched with. The case then checks the `--model` the tool received and what
# amplihack printed to stderr. Nothing inside amplihack is replaced.
#
# The cases cover every path a dotted id can take:
#   - AMPLIHACK_DEFAULT_MODEL holding a dotted Claude id: rewritten to hyphens,
#     and the stderr line names the original spelling.
#   - An explicit --model: forwarded as typed. A Claude-compatible tool gets
#     one warning naming the hyphenated spelling.
#   - copilot and codex: no rewrite, no warning. The LiteLLM gateway: the
#     gateway model, unchanged and without a warning.
#   - ANTHROPIC_BASE_URL: makes no difference. A dotted default model is
#     rewritten behind Anthropic's own URL and behind any other. An operator
#     whose endpoint serves the dotted spelling passes an explicit --model,
#     which is forwarded as typed.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script directly against the amplihack binary that job installs, on
# every pull request to main, merge-queue run and push to main. That job is a
# required status check on main, so these cases are re-run on whatever merges
# and a failure blocks the merge. CI does not run gadugi-test itself; the YAML
# in tests/gadugi/scenarios wraps this script for anyone running it through
# gadugi-test by hand. The auto-drive QA step does not run it: for a Rust repo
# that step runs `cargo test`.
#
# Run by hand: bash tests/gadugi/run-issue-1527-model-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_1527_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED and
# exits 0.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT" || { echo "FAIL: cannot cd to $REPO_ROOT"; echo "SCENARIO_FAILED"; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1527-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
trap 'rm -rf -- "$WORK"' EXIT

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_1527_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # json-render-diagnostics keeps compiler errors readable on stderr while
  # stdout carries the artifact records that name the executable's path. A
  # single awk reads every record and prints the last path, so cargo never
  # writes into a closed pipe (issue #1434) and pipefail reports cargo's status.
  # 14 is the length of `"executable":"`; 15 adds the closing quote.
  if ! BIN="$(cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
echo "binary under test: $BIN"

# --- stub tools -------------------------------------------------------------
# Each stub writes the argv of every launch, NUL-separated, to its own file,
# along with the two variables the cases care about. `--version` is answered
# with a version new enough for the LiteLLM gateway preflight.
mkdir -p "$WORK/bin"
cat > "$WORK/bin/stub" <<'STUB'
#!/bin/sh
name=$(basename "$0")
case "${1-}" in
  --version|-v|version) echo "2.1.247 (Claude Code)"; exit 0 ;;
esac
dir="$QA_1527_RECORD/$name"
mkdir -p "$dir"
n=0
while [ -e "$dir/argv.$n" ]; do n=$((n + 1)); done
for a in "$@"; do printf '%s\0' "$a"; done > "$dir/argv.$n"
printf '%s' "${ANTHROPIC_BASE_URL-<unset>}" > "$dir/base_url.$n"
printf '%s' "${AMPLIHACK_DEFAULT_MODEL-<unset>}" > "$dir/default_model.$n"
exit 0
STUB
chmod +x "$WORK/bin/stub"
for tool in claude copilot codex; do ln -s stub "$WORK/bin/$tool"; done

# The --model value in a recorded argv: the value itself, `=<value>` for the
# `--model=<value>` form, or NONE.
model_of() {
  local file="$1" arg want=0
  [ -f "$file" ] || { echo MISSING; return; }
  while IFS= read -r -d '' arg; do
    if [ "$want" = 1 ]; then printf '%s\n' "$arg"; return; fi
    case "$arg" in
      --model) want=1 ;;
      --model=*) printf '=%s\n' "${arg#--model=}"; return ;;
    esac
  done < "$file"
  echo NONE
}

fail=0
passed=0
GATEWAY=(AMPLIHACK_LITELLM_ENDPOINT=http://127.0.0.1:9 AMPLIHACK_LITELLM_API_KEY=sk-qa AMPLIHACK_LITELLM_MODEL=claude-opus-5.5)
PROXY_URL=https://llm-gateway.example.com

# check <id> <tool> <want --model> <want warning: yes|no> <want stderr substring, or -> \
#       [VAR=value ...] -- [amplihack args ...]
check() {
  local id="$1" tool="$2" want_model="$3" want_warning="$4" want_stderr="$5"
  shift 5
  local envs=() args=()
  while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
  [ "$#" -gt 0 ] && shift
  args=("$@")

  local case_dir="$WORK/case-$id"
  mkdir -p "$case_dir/home" "$case_dir/record"
  # Most cases have no variables or no arguments. Bash before 4.4 (macOS ships
  # 3.2) treats an empty "${a[@]}" as unbound under `set -u` and aborts, so
  # each array is expanded as ${a[@]+"${a[@]}"}: nothing when it is empty.
  env -i HOME="$case_dir/home" PATH="$WORK/bin:/usr/bin:/bin" TERM=dumb \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 QA_1527_RECORD="$case_dir/record" \
    ${envs[@]+"${envs[@]}"} "$BIN" "$tool" ${args[@]+"${args[@]}"} \
    </dev/null >"$case_dir/stdout" 2>"$case_dir/stderr"
  local rc=$?

  local got_model got_warning=no problems=()
  got_model="$(model_of "$case_dir/record/$tool/argv.0")"
  grep -q '^amplihack: warning: passing `--model' "$case_dir/stderr" && got_warning=yes
  [ "$rc" -eq 0 ] || problems+=("exit code $rc")
  [ "$got_model" = "$want_model" ] || problems+=("--model '$got_model', want '$want_model'")
  [ "$got_warning" = "$want_warning" ] || problems+=("warning $got_warning, want $want_warning")
  if [ "$want_stderr" != "-" ] && ! grep -qF -- "$want_stderr" "$case_dir/stderr"; then
    problems+=("stderr lacks: $want_stderr")
  fi

  local label="$tool, env: ${envs[*]:-none}, args: ${args[*]:-none}"
  if [ "${#problems[@]}" -eq 0 ]; then
    echo "PASS: $id ($label) -> --model $got_model, warning $got_warning"
    passed=$((passed + 1))
  else
    echo "FAIL: $id ($label): ${problems[*]}"
    # The first ten stderr lines about the model or an error, in one awk that
    # reads the whole file (issue #1434).
    awk 'tolower($0) ~ /model|error/ && n < 10 { print "    stderr: " $0; n++ }' "$case_dir/stderr"
    fail=1
  fi
}

# Explicit --model: forwarded as typed; a dotted Claude id gets the warning.
check explicit-dotted claude claude-opus-5.5 yes 'Use `--model claude-opus-5-5`.' -- --model claude-opus-5.5
check explicit-dotted-equals claude =claude-opus-5.5 yes 'Use `--model claude-opus-5-5`.' -- --model=claude-opus-5.5
check explicit-hyphenated claude claude-opus-5-5 no - -- --model claude-opus-5-5

# AMPLIHACK_DEFAULT_MODEL: a dotted Claude id is rewritten; the suffix is kept.
check env-dotted claude 'claude-opus-5-5[1m]' no 'normalised from `claude-opus-5.5[1m]`: Claude model ids use hyphens, not dots' \
  'AMPLIHACK_DEFAULT_MODEL=claude-opus-5.5[1m]' --
check env-dotted-and-explicit claude claude-opus-5.5 yes - \
  'AMPLIHACK_DEFAULT_MODEL=claude-opus-5.5[1m]' -- --model claude-opus-5.5

# copilot and codex: the dotted spelling is Copilot's own.
check copilot-explicit copilot claude-opus-4.5 no - -- --model claude-opus-4.5
check copilot-env copilot NONE no - AMPLIHACK_DEFAULT_MODEL=claude-opus-5.5 --
check codex-explicit codex claude-opus-4.5 no - -- --model claude-opus-4.5

# The LiteLLM gateway routes on the exact name.
check gateway-explicit claude claude-opus-5.5 no - "${GATEWAY[@]}" -- --model claude-opus-5.5
check gateway-model claude claude-opus-5.5 no '(from AMPLIHACK_LITELLM_MODEL). Set AMPLIHACK_LITELLM_MODEL to change it.' \
  "${GATEWAY[@]}" --
check telemetry-only-env claude claude-sonnet-4-5 no '(from AMPLIHACK_DEFAULT_MODEL)' \
  "AMPLIHACK_LITELLM_TELEMETRY_FILE=$WORK/telemetry.jsonl" AMPLIHACK_LITELLM_TARGET=x AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4-5 --
check telemetry-only-explicit claude claude-opus-5.5 yes - \
  "AMPLIHACK_LITELLM_TELEMETRY_FILE=$WORK/telemetry.jsonl" -- --model claude-opus-5.5

# ANTHROPIC_BASE_URL makes no difference: amplihack does not read it.
check base-url-anthropic-env-dotted claude claude-sonnet-4-5 no 'normalised from `claude-sonnet-4.5`' \
  ANTHROPIC_BASE_URL=https://api.anthropic.com AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4.5 --
check base-url-proxy-env-dotted claude claude-sonnet-4-5 no 'normalised from `claude-sonnet-4.5`' \
  "ANTHROPIC_BASE_URL=$PROXY_URL" AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4.5 --
# The escape hatch for a proxy that serves the dotted spelling.
check base-url-proxy-explicit-dotted claude claude-opus-5.5 yes 'Use `--model claude-opus-5-5`.' \
  "ANTHROPIC_BASE_URL=$PROXY_URL" AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4.5 -- --model claude-opus-5.5

# The launched tool inherits AMPLIHACK_DEFAULT_MODEL and ANTHROPIC_BASE_URL as
# set; only the --model argument changes.
child_default="$(cat "$WORK/case-env-dotted/record/claude/default_model.0" 2>/dev/null)"
if [ "$child_default" = 'claude-opus-5.5[1m]' ]; then
  echo "PASS: child-env (claude inherited AMPLIHACK_DEFAULT_MODEL=$child_default unchanged)"
  passed=$((passed + 1))
else
  echo "FAIL: child-env: claude inherited AMPLIHACK_DEFAULT_MODEL='$child_default', want 'claude-opus-5.5[1m]'"
  fail=1
fi
child_base="$(cat "$WORK/case-base-url-proxy-env-dotted/record/claude/base_url.0" 2>/dev/null)"
if [ "$child_base" = "$PROXY_URL" ]; then
  echo "PASS: child-base-url (claude inherited ANTHROPIC_BASE_URL=$child_base unchanged)"
  passed=$((passed + 1))
else
  echo "FAIL: child-base-url: claude inherited ANTHROPIC_BASE_URL='$child_base', want '$PROXY_URL'"
  fail=1
fi

echo "cases passed: $passed"
if [ "$fail" -eq 0 ]; then
  echo "issue 1527 dotted model id contract green"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
