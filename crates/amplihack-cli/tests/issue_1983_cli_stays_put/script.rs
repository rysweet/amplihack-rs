//! Script templates for the fake CLIs and the stub recipe runner.
//!
//! Neither script uses `eval`. Marker names are spliced into the fake CLI at
//! build time only after [`marker_names`] has checked each against
//! `^[A-Z_][A-Z0-9_]*$`; every other value is read from the environment and
//! quoted.

/// The names in `SESSION_MARKERS`, checked to be safe to splice into a script.
pub fn marker_names() -> Vec<&'static str> {
    let names: Vec<&str> = amplihack_utils::agent_binary::SESSION_MARKERS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    for name in &names {
        let mut chars = name.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_');
        let rest_ok = chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        assert!(first_ok && rest_ok, "unsafe marker name {name:?}");
    }
    names
}

/// The fake agent CLI, installed as both `claude` and `codex`. `markers` is
/// the space-separated list of validated marker names.
pub fn fake_cli(markers: &str) -> String {
    FAKE_CLI.replace("@MARKERS@", markers)
}

const FAKE_CLI: &str = r#"#!/bin/sh
# Fake agent CLI for issue #1983. It never runs a real claude or codex.
me=$(basename "$0")
case "${1-}" in
  --version)
    if [ "$me" = codex ]; then echo 'codex-cli 0.160.0'; else echo '2.1.0 (Claude Code)'; fi
    exit 0 ;;
esac
# Service calls a launcher may make (plugin registration and the like) are
# answered and logged, never recorded as a run.
case "${1-}" in
  plugin|mcp|login|features)
    printf '%s %s\n' "$me" "$*" >> "$RECORD_DIR/service.log"
    echo '{"installed":[]}'
    exit 0 ;;
esac

out=
prev=
last=
for a; do
  if [ "$prev" = --output-last-message ]; then out=$a; fi
  prev=$a
  last=$a
done
if [ -n "$out" ]; then
  case "$out" in
    "$RECORD_DIR"/*|"$WORK_DIR"/*) ;;
    *) echo "fake $me: refusing --output-last-message outside the fixture: $out" >&2; exit 43 ;;
  esac
fi
# `codex exec ... -` reads its prompt from stdin; drain it so the caller never
# sees a broken pipe.
if [ "$me" = codex ] && [ "$last" = - ]; then cat > /dev/null; fi

seq=$(flock "$RECORD_DIR/.seq.lock" sh -c \
  'n=$(cat "$1" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$1"; echo "$n"' \
  sh "$RECORD_DIR/.seq")
seen=$("$AMPLIHACK_UNDER_TEST" agent-binary < /dev/null 2> "$RECORD_DIR/children_see.$seq.log" | head -n 1)
seen=${seen%% *}
[ -n "$seen" ] || seen='<none>'

part=$(mktemp "$RECORD_DIR/part.XXXXXX")
{
  printf 'seq=%s\n' "$seq"
  printf 'ran=%s\n' "$me"
  printf 'self=%s\n' "$(realpath "$0")"
  printf 'role=%s\n' "${FAKE_ROLE-top}"
  printf 'argv=%s\n' "$(printf '%s ' "$@" | tr '\n' ' ')"
  printf 'binary=%s\n' "${AMPLIHACK_AGENT_BINARY-<unset>}"
  printf 'source=%s\n' "${AMPLIHACK_AGENT_BINARY_SOURCE-<unset>}"
  if [ -n "${AMPLIHACK_SESSION_NONCE+x}" ]; then echo nonce_set=true; else echo nonce_set=false; fi
  for m in @MARKERS@; do
    v=$(printenv "$m") || v='<unset>'
    printf '%s=%s\n' "$m" "$v"
  done
  printf 'children_see=%s\n' "$seen"
} > "$part"
mv "$part" "$RECORD_DIR/rec.$seq"
[ -z "$out" ] || printf 'fake %s done\n' "$me" > "$out"

if [ -n "${FAKE_THEN-}" ]; then
  then_cmd=$FAKE_THEN
  unset FAKE_THEN
  if [ "$me" = claude ]; then
    CLAUDECODE=1
    CLAUDE_CODE_ENTRYPOINT=cli
    export CLAUDECODE CLAUDE_CODE_ENTRYPOINT
  fi
  FAKE_ROLE=nested sh -c "$then_cmd" > "$RECORD_DIR/then.$seq.log" 2>&1 < /dev/null || exit 42
fi
exit 0
"#;

/// The stub `recipe-runner-rs`. Each agent step copies `build_agent_command`
/// of the pinned runner (rev 4d5187ea, src/adapters/cli_subprocess.rs,
/// lines 781 to 866; the codex branch ends at line 812): the launcher is
/// `$AMPLIHACK_LAUNCHER_BINARY <cli> --`, codex gets
/// `exec --output-last-message <file> -` with the prompt on stdin, and claude
/// gets `-p <prompt> --system-prompt <text> --add-dir <cwd>`. The real runner
/// starts every step without `CLAUDECODE`.
pub const STUB_RUNNER: &str = r#"#!/bin/sh
if [ "${1-}" = --capabilities ]; then
  printf '%s\n' '{"schema_version":1,"version":"stub","capabilities":["codex_exec"]}'
  exit 0
fi
printf 'binary=%s\nsource=%s\n' \
  "${AMPLIHACK_AGENT_BINARY-<unset>}" "${AMPLIHACK_AGENT_BINARY_SOURCE-<unset>}" \
  > "$RECORD_DIR/runner.env"
cli=${AMPLIHACK_AGENT_BINARY-}
case "$cli" in
  claude|codex) ;;
  *) echo "stub runner: unsupported agent binary '$cli'" >&2; exit 52 ;;
esac
for role in outer sub subsub; do
  if [ "$cli" = codex ]; then
    final=$(mktemp "$RECORD_DIR/final-message.XXXXXX")
    printf 'prompt for %s\n' "$role" | FAKE_ROLE="step:$role" env -u CLAUDECODE \
      "$AMPLIHACK_LAUNCHER_BINARY" codex -- exec --output-last-message "$final" - \
      >> "$RECORD_DIR/steps.log" 2>&1 || exit 53
  else
    FAKE_ROLE="step:$role" env -u CLAUDECODE \
      "$AMPLIHACK_LAUNCHER_BINARY" claude -- -p "prompt for $role" \
      --system-prompt "stub system prompt" --add-dir "$WORK_DIR" \
      < /dev/null >> "$RECORD_DIR/steps.log" 2>&1 || exit 53
  fi
done
printf '%s' '{"recipe_name":"probe","success":true,"step_results":[],"context":{}}'
"#;

/// A recipe for `recipe run`. The stub runner ignores its content.
pub const PROBE_RECIPE: &str =
    "name: probe\nsteps:\n  - id: noop\n    type: bash\n    command: \"true\"\n";
