#!/usr/bin/env bash
# Isolated CLI acceptance test for Claude login profiles.
set -euo pipefail

if [[ -z "${AMPLIHACK_QA_BINARY:-}" || ! -x "${AMPLIHACK_QA_BINARY:-}" ]]; then
  echo "FAIL: AMPLIHACK_QA_BINARY must name an executable candidate amplihack binary" >&2
  exit 1
fi
BIN="$(cd "$(dirname "$AMPLIHACK_QA_BINARY")" && pwd)/$(basename "$AMPLIHACK_QA_BINARY")"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/claude-profile-qa.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT

QA_HOME="$WORK/home"
QA_BIN_DIR="$WORK/bin"
QA_PROJECT="$WORK/project"
QA_RECORD="$WORK/record"
mkdir -p "$QA_HOME/.claude/agents" "$QA_BIN_DIR" "$QA_PROJECT" "$QA_RECORD"

cat > "$QA_HOME/.claude/agents/reviewer.md" <<'ASSET'
Shared review agent.
ASSET
cat > "$QA_HOME/.claude/settings.json" <<'SETTINGS'
{"language":"en","env":{"PROFILE_QA_SECRET":"must-not-copy"}}
SETTINGS
cat > "$QA_HOME/.claude/.credentials.json" <<'CREDENTIALS'
{"sourceCredential":"must-not-copy"}
CREDENTIALS

# Snapshot every source file before CLI operations; profile add must be read-only.
find "$QA_HOME/.claude" -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/source.before"

cat > "$QA_BIN_DIR/claude" <<'CLAUDE'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--version" ]]; then
  echo "2.1.247 (Claude Code)"
  exit 0
fi
record="${QA_PROFILE_RECORD:?}"
n=0
[[ ! -f "$record/count" ]] || n="$(cat "$record/count")"
printf '%s' "$((n + 1))" > "$record/count"
printf '%s' "${CLAUDE_CONFIG_DIR:-<unset>}" > "$record/config.$n"
printf '%s\0' "$@" > "$record/argv.$n"
if [[ -n "${CLAUDE_CONFIG_DIR:-}" ]]; then
  mkdir -p "$CLAUDE_CONFIG_DIR"
  printf 'created by isolated fake Claude\n' > "$CLAUDE_CONFIG_DIR/.qa-login-marker"
fi
CLAUDE
chmod +x "$QA_BIN_DIR/claude"
cat > "$QA_BIN_DIR/node" <<'NODE'
#!/usr/bin/env sh
if [ "${1:-}" = "--version" ]; then
  echo "v24.2.0"
  exit 0
fi
echo "unexpected node invocation in isolated profile QA" >&2
exit 127
NODE
chmod +x "$QA_BIN_DIR/node"

run_cli() {
  (cd "$QA_PROJECT" && env -i \
    HOME="$QA_HOME" \
    PATH="$QA_BIN_DIR:/usr/bin:/bin" \
    TERM=dumb \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    QA_PROFILE_RECORD="$QA_RECORD" \
    "$BIN" "$@") > "$WORK/stdout" 2> "$WORK/stderr" || {
      echo "FAIL: amplihack $*" >&2
      cat "$WORK/stderr" >&2
      exit 1
    }
}

ROOT="$QA_HOME/.amplihack/claude-profiles"
run_cli profile add work
run_cli profile add personal
for profile in work personal; do
  [[ "$(cat "$ROOT/$profile/agents/reviewer.md")" == "Shared review agent." ]]
  grep -q '"language": "en"' "$ROOT/$profile/settings.json"
  ! grep -q 'PROFILE_QA_SECRET\|must-not-copy' "$ROOT/$profile/settings.json"
  [[ ! -e "$ROOT/$profile/.credentials.json" ]]
done
echo "PASS: allowlisted shared assets copied; credentials and excluded settings omitted"

run_cli profile list
grep -q 'personal$' "$WORK/stdout"
grep -q 'work$' "$WORK/stdout"

run_cli profile login work
run_cli profile login personal
[[ "$(cat "$QA_RECORD/config.0")" == "$ROOT/work" ]]
[[ "$(cat "$QA_RECORD/config.1")" == "$ROOT/personal" ]]
[[ -f "$ROOT/work/.qa-login-marker" && -f "$ROOT/personal/.qa-login-marker" ]]
echo "PASS: login routed to distinct profile directories"

run_cli profile switch work
run_cli profile show
grep -q "$ROOT/work" "$WORK/stdout"
run_cli profile run personal -- --model profile-qa
[[ "$(cat "$QA_RECORD/config.2")" == "$ROOT/personal" ]]
run_cli claude --model default-qa
[[ "$(cat "$QA_RECORD/config.3")" == "$ROOT/work" ]]
echo "PASS: explicit run and switched default route to the selected profiles"

# Removing the selected default clears the pointer; removing another profile
# leaves the store empty.
run_cli profile remove work --yes
run_cli profile show
grep -q '^none$' "$WORK/stdout"
run_cli profile remove personal --yes
run_cli profile list
grep -q '^no profiles' "$WORK/stdout"
echo "PASS: removing the default clears it and both profiles can be removed"

find "$QA_HOME/.claude" -type f -print0 | sort -z | xargs -0 sha256sum > "$WORK/source.after"
cmp -s "$WORK/source.before" "$WORK/source.after"
echo "PASS: shared ~/.claude source remained unchanged"
echo "CLAUDE_PROFILES_QA_PASSED"
