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
# Without these the path checks below would accept any absolute path.
: "${RECORD_DIR:?fake cli: RECORD_DIR unset}" "${WORK_DIR:?fake cli: WORK_DIR unset}"
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
    */../*|*/..) echo "fake $me: refusing --output-last-message with '..': $out" >&2; exit 43 ;;
    "$RECORD_DIR"/*|"$WORK_DIR"/*) ;;
    # The real runner's codex adapter asks for `final-message` in a private
    # directory it made under $TMPDIR, and reads that file back.
    "${TMPDIR%/}"/*/final-message)
      [ -n "${TMPDIR-}" ] || { echo "fake $me: TMPDIR unset" >&2; exit 43; } ;;
    *) echo "fake $me: refusing --output-last-message outside the fixture: $out" >&2; exit 43 ;;
  esac
fi
# `codex exec ... -` reads its prompt from stdin; read it to the end so the
# caller never sees a broken pipe.
stdin_text=
if [ "$me" = codex ] && [ "$last" = - ]; then stdin_text=$(cat); fi
# The step a prompt came from, when the prompt names one.
tag=$(printf '%s\n%s\n' "$*" "$stdin_text" | grep -o 'issue1983-level-[a-z]*' | head -n 1)
[ -n "$tag" ] || tag='<none>'

# Every fake in a test runs one after another (the runners start steps in
# sequence and FAKE_THEN runs synchronously), so a plain counter is enough.
# If two ever ran at once, the `ln` below refuses to overwrite a record and
# the fake fails, rather than losing one silently.
seq=$(cat "$RECORD_DIR/.seq" 2>/dev/null || echo 0)
seq=$((seq + 1))
echo "$seq" > "$RECORD_DIR/.seq"
seen=$("$AMPLIHACK_UNDER_TEST" agent-binary < /dev/null 2> "$RECORD_DIR/children_see.$seq.log" | head -n 1)
seen=${seen%% *}
[ -n "$seen" ] || seen='<none>'

part=$(mktemp "$RECORD_DIR/part.XXXXXX")
{
  printf 'seq=%s\n' "$seq"
  printf 'ran=%s\n' "$me"
  printf 'self=%s\n' "$(realpath "$0")"
  printf 'role=%s\n' "${FAKE_ROLE-top}"
  printf 'prompt_tag=%s\n' "$tag"
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
ln "$part" "$RECORD_DIR/rec.$seq" || { echo "fake $me: record $seq already exists" >&2; exit 44; }
rm -f "$part"
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

/// The runner revision [`STUB_RUNNER`] was copied from. A test fails when
/// `claude-plugin/recipe-runner.rev` moves away from it.
pub const STUB_RUNNER_REV: &str = "4d5187ea";

/// The stub `recipe-runner-rs`, used by the level 1 tests. It runs three
/// agent steps one after another; it does not nest them, and it does not
/// stand in for the runner's sub-recipe handling, which the level 2 tests
/// check with the real runner. Each agent step copies `build_agent_command`
/// of the pinned runner (rev [`STUB_RUNNER_REV`],
/// src/adapters/cli_subprocess.rs, lines 781 to 866; the codex branch ends at
/// line 812): the launcher is
/// `$AMPLIHACK_LAUNCHER_BINARY <cli> --`, codex gets
/// `exec --output-last-message <file> -` with the prompt on stdin, and claude
/// gets `-p <prompt> --system-prompt <text> --add-dir <cwd>`. The real runner
/// starts every step without `CLAUDECODE`.
pub const STUB_RUNNER: &str = r#"#!/bin/sh
: "${RECORD_DIR:?stub runner: RECORD_DIR unset}" "${WORK_DIR:?stub runner: WORK_DIR unset}"
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
    printf 'issue1983-level-%s\n' "$role" | FAKE_ROLE="step:$role" env -u CLAUDECODE \
      "$AMPLIHACK_LAUNCHER_BINARY" codex -- exec --output-last-message "$final" - \
      >> "$RECORD_DIR/steps.log" 2>&1 || exit 53
  else
    FAKE_ROLE="step:$role" env -u CLAUDECODE \
      "$AMPLIHACK_LAUNCHER_BINARY" claude -- -p "issue1983-level-$role" \
      --system-prompt "stub system prompt" --add-dir "$WORK_DIR" \
      < /dev/null >> "$RECORD_DIR/steps.log" 2>&1 || exit 53
  fi
done
printf '%s' '{"recipe_name":"probe","success":true,"step_results":[],"context":{}}'
"#;

/// A recipe for `recipe run`. The stub runner ignores its content.
pub const PROBE_RECIPE: &str =
    "name: probe\nsteps:\n  - id: noop\n    type: bash\n    command: \"true\"\n";

/// Recipes for the level 2 tests, which run the real runner. The top recipe
/// starts `issue1983-sub` as a sub-recipe, which starts `issue1983-subsub`;
/// each level has one agent step whose prompt names the level, so a record's
/// `prompt_tag` shows which step started that fake.
pub const NESTED_RECIPES: [(&str, &str); 3] = [
    (
        "issue1983-outer.yaml",
        "name: issue1983-outer\nversion: 1.0.0\nsteps:\n  - id: outer-agent\n    type: agent\n    prompt: \"issue1983-level-outer\"\n  - id: into-sub\n    type: recipe\n    recipe: issue1983-sub\n",
    ),
    (
        "issue1983-sub.yaml",
        "name: issue1983-sub\nversion: 1.0.0\nsteps:\n  - id: sub-agent\n    type: agent\n    prompt: \"issue1983-level-sub\"\n  - id: into-subsub\n    type: recipe\n    recipe: issue1983-subsub\n",
    ),
    (
        "issue1983-subsub.yaml",
        "name: issue1983-subsub\nversion: 1.0.0\nsteps:\n  - id: subsub-agent\n    type: agent\n    prompt: \"issue1983-level-subsub\"\n",
    ),
];
