#!/usr/bin/env bash
# Issue #2788 release evidence: the shipped Crusty round, installed binary,
# and pinned recipe runner, with only Codex and GitHub replaced by fixtures.
# Shell composes the existing CLI/recipe processes; production code is Rust.
# Run in CI, never on a factory host. No live credentials or host agent state.
set -euo pipefail
# Owner-only from the first mkdir. `amplihack recipe run` opens
# AMPLIHACK_WORK_ROOT through the pinned runner's private store, which refuses
# any root that is not owned and mode 0700 (recipe-runner-rs 2833a0b,
# src/recovery/private_fs.rs). Under umask 022 the work, home and tmp roots
# were 0755 and the run stopped before its first step.
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd -P)"
BIN="${AMPLIHACK_2788_QA_BIN:-$(command -v amplihack || true)}"
RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs || true)}"
for executable in "$BIN" "$RUNNER"; do
  if [ -z "$executable" ] || [ ! -x "$executable" ]; then
    echo 'FAIL: installed amplihack and pinned recipe-runner-rs are required'
    echo SCENARIO_FAILED
    exit 1
  fi
done
BIN="$(realpath -- "$BIN")"
RUNNER="$(realpath -- "$RUNNER")"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2788-qa.XXXXXX")"
WORK="$(cd -- "$WORK" && pwd -P)"
# fail <label>: name the failing step on stdout; the EXIT trap then prints
# each evidence file under its own header and SCENARIO_FAILED. Without the
# headers a tail of round.err cannot be told apart from the job's own stderr:
# the first CI failure of this driver (job 134724256224) printed the tail of
# round.err, `recipe run`'s "Error: recovery directory must be owned and mode
# 0700", and it read as a failure in the setup commands before the run.
fail() {
  echo "FAIL: $*"
  exit 1
}
cleanup() {
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    for log in round.err round.out launches/events.jsonl writer.out \
      prepared.json state/crusty-round-1.json completion.json gh-calls; do
      if [ -f "$WORK/$log" ]; then
        echo "--- $log (last 60 lines) ---"
        tail -n 60 -- "$WORK/$log"
      fi
    done
    echo SCENARIO_FAILED
  fi
  rm -rf -- "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/bin" "$WORK/home/.codex" "$WORK/home/.claude" \
  "$WORK/home/.amplihack" "$WORK/home/.config" "$WORK/tmp" "$WORK/work" \
  "$WORK/state" "$WORK/launches" || fail 'create scratch folders'
for root in "$WORK" "$WORK/home" "$WORK/tmp" "$WORK/work" "$WORK/state"; do
  [ "$(stat -c %a -- "$root")" = 700 ] || fail "scratch root ${root#"$WORK"} is not mode 0700"
done

# No inherited PATH: a missing fixture must never start a real agent.
for tool in bash sh cat cmp cp chmod cut dirname find git grep head id jq ls mkdir \
  mktemp mv readlink realpath rm sed sha256sum sort stat tail tr uname wc env awk basename; do
  path="$(command -v "$tool")"
  [ -x "$path" ] || { echo "FAIL: required tool $tool is missing"; exit 1; }
  ln -s -- "$path" "$WORK/bin/$tool"
done
ln -s -- "$BIN" "$WORK/bin/amplihack"
ln -s -- "$RUNNER" "$WORK/bin/recipe-runner-rs"
printf '# configuration sentinel\nsandbox_mode = "read-only"\n' > "$WORK/home/.codex/config.toml"
cp -- "$WORK/home/.codex/config.toml" "$WORK/codex-config.before"

scenario_env() {
  env -i HOME="$WORK/home" PATH="$WORK/bin" TMPDIR="$WORK/tmp" LANG=C TERM=dumb \
    CODEX_HOME="$WORK/home/.codex" CLAUDE_CONFIG_DIR="$WORK/home/.claude" \
    XDG_CONFIG_HOME="$WORK/home/.config" GIT_CONFIG_GLOBAL=/dev/null \
    GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 \
    GIT_ASKPASS=true SSH_ASKPASS=true GIT_PAGER=cat GH_PAGER=cat PAGER=cat \
    AMPLIHACK_HOME="$REPO_ROOT" AMPLIHACK_BIN="$BIN" \
    AMPLIHACK_WORK_ROOT="$WORK/work" AMPLIHACK_LAUNCHER_BINARY="$BIN" \
    AMPLIHACK_AGENT_BINARY=codex AMPLIHACK_CODEX_BINARY_PATH="$WORK/bin/codex" \
    CODEX_BINARY_PATH="$WORK/bin/codex" RECIPE_RUNNER_RS_PATH="$RUNNER" \
    AMPLIHACK_NONINTERACTIVE=1 AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 AMPLIHACK_NO_UPDATE_CHECK=1 \
    AMPLIHACK_SKIP_POWER_STEERING=1 AMPLIHACK_SKIP_REFLECTION=1 \
    AMPLIHACK_ROTATE_AUTOSTART=0 AMPLIHACK_DISABLE_BLARIFY=1 \
    AMPLIHACK_GOVERNOR=off AMPLIHACK_USAGE_LIMIT_WAIT=0 AMPLIHACK_CLEANUP_ON_EXIT=0 \
    "$@"
}
fixture_git() {
  scenario_env git -c user.name=QA -c user.email=qa@example.invalid \
    -c commit.gpgsign=false -c init.templateDir= "$@"
}
fixture_git init -q -b main "$WORK/main" || fail 'fixture git init'
fixture_git -C "$WORK/main" commit --allow-empty -qm baseline || fail 'fixture git commit'
fixture_git -C "$WORK/main" worktree add -qb qa/pr-7 "$WORK/pr" || fail 'fixture git worktree add'
fixture_git -C "$WORK/pr" rev-parse HEAD > "$WORK/head" || fail 'fixture git rev-parse'

cat > "$WORK/bin/codex" <<'STUB'
#!/bin/bash
set -euo pipefail
W="$(cd -- "$(dirname -- "$0")/.." && pwd -P)"
case "${1:-}" in
  --version) echo 'codex-cli 0.166.0'; exit 0 ;;
  plugin) echo '{"installed":[]}'; exit 0 ;;
esac
argv=("$@")
skip=0 final= directory= directory_set=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --) shift; break ;;
    --output-last-message|-o) final="$2"; shift 2 ;;
    --cd|-C) directory="$2"; directory_set=1; shift 2 ;;
    --cd=*) directory="${1#--cd=}"; directory_set=1; shift ;;
    -C?*) directory="${1#-C}"; directory="${directory#=}"; directory_set=1; shift ;;
    --model|-m|--config|-c|--sandbox|-s|--profile|-p|--image|-i|--add-dir|--enable|--disable)
      shift 2 ;;
    --skip-git-repo-check) skip=$((skip + 1)); shift ;;
    *) shift ;;
  esac
done
if [ "$directory_set" = 1 ]; then
  case "$directory" in
    '~') directory="$HOME" ;;
    '~/'*) directory="$HOME/${directory#??}" ;;
  esac
  cd -- "$directory"
fi
prompt="$(cat)"
# The shipped round's declared agent sequence; no prompt text controls the fixture.
n="$(if [ -f "$W/launches/events.jsonl" ]; then jq -s length "$W/launches/events.jsonl"; else printf 0; fi)"
case "$n" in
  0) label=context ;;
  1) label=review ;;
  2) label=assessor ;;
  3) label=decision ;;
  *) echo 'unexpected Codex invocation' >&2; exit 43 ;;
esac
printf '%s\000' "${argv[@]}" > "$W/launches/$label.argv"
in_git=false
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then in_git=true; fi
jq -cn --arg label "$label" --arg cwd "$(pwd -P)" --argjson skip "$skip" \
  --argjson git_checkout "$in_git" \
  '{label:$label,cwd:$cwd,skip_count:$skip,git_checkout:$git_checkout}' >> "$W/launches/events.jsonl"
if [ "$in_git" = false ] && [ "$skip" = 0 ]; then
  echo 'not inside a trusted directory' >&2; exit 42
fi
head="$(cat "$W/head")"
case "$label" in
  context)
    result="$(jq -cn --arg head "$head" '{pr:"7",head_sha:$head,resolved_concerns:"",round_label:"round-1",blockers:[],attempts:[]}')" ;;
  assessor) result='{"assessment":"unchanged; preserve prior work and continue"}' ;;
  review)
    result="$(jq -cn --arg head "$head" '{crusty_verdict:"CLEAN",concerns:[],summary:("Reviewed commit " + $head),reviewed_head_sha:$head}')" ;;
  decision)
    # Always invoke the production writer. Missing or ineligible evidence
    # selects evaluate; CLEAN needs no minor/severity sidecars.
    action=evaluate
    reason='Round evidence is missing or does not establish completion'
    if jq -e '(.crusty_verdict == "CLEAN" or .crusty_verdict == "MINOR_ONLY") and
      .reviewed_head_sha == .head_sha and .commits_this_round == 0' \
      "$W/state/crusty-round-1.json" >/dev/null 2>&1; then
      action=complete
      reason='Recorded review covers the current head with zero round commits'
    fi
    private_state="$(jq -r .private_state "$W/prepared.json")"
    amplihack orch helper autodrive-completion-write --private-state "$private_state" \
      --action "$action" --reason "$reason" > "$W/writer.out"
    result="$(jq -cn --arg action "$action" '{action:$action}')" ;;
esac
[ -n "$final" ] || { echo 'runner omitted final-message path' >&2; exit 44; }
printf '%s\n' "$result" > "$final"
printf '%s\n' "$result"
STUB
cat > "$WORK/bin/gh" <<'STUB'
#!/bin/bash
set -euo pipefail
W="$(cd -- "$(dirname -- "$0")/.." && pwd -P)"
printf '%s\n' "$*" >> "$W/gh-calls"
if [ "${1:-} ${2:-}" = 'pr view' ]; then cat "$W/head"; exit 0; fi
echo 'unexpected GitHub operation in dry-run scenario' >&2
exit 45
STUB
printf '#!/bin/sh\nexit 0\n' > "$WORK/bin/codex-code-mode-host"
printf '#!/bin/sh\necho v22.14.0\n' > "$WORK/bin/node"
chmod 700 "$WORK/bin/codex" "$WORK/bin/gh" "$WORK/bin/codex-code-mode-host" "$WORK/bin/node"

TOOLS="$REPO_ROOT/amplifier-bundle/tools"
RECIPE="$REPO_ROOT/amplifier-bundle/recipes/autodrive-crusty-round.yaml"
REVIEW="$WORK/pr"
LAUNCH="$WORK/pr"
HEAD_SHA="$(cat "$WORK/head")"
RECORD="$WORK/state/crusty-round-1.json"
scenario_env "$BIN" orch helper autodrive-completion-prepare \
  --state-dir "$WORK/state" --round-label round-1 --record "$RECORD" \
  --repo "$WORK/pr" --launch-tools "$TOOLS" > "$WORK/prepared.json" ||
  fail 'autodrive-completion-prepare'
PRIVATE_STATE="$(jq -er .private_state "$WORK/prepared.json")" ||
  fail 'autodrive-completion-prepare printed no private_state'

# This is the unchanged shipped recipe, not a substitute returning fixed
# step outputs. All three agent steps cross the actual Codex launch path.
# `set -e` does not apply inside a subshell on the left of `||`, so the
# directory change is checked explicitly.
(
  cd -- "$WORK/main" || exit 1
  scenario_env "$BIN" recipe run "$RECIPE" --working-dir "$WORK/main" --no-detach \
    -c "repo_path=$WORK/pr" \
    -c "reviewed_head_sha=$HEAD_SHA" -c "autodrive_launch_tools=$TOOLS" \
    -c pr_number=7 -c pr_url=https://github.com/example/fixture/pull/7 \
    -c 'task_description=Verify Codex review launch execution for issue 2788' \
    -c "autodrive_round_record=$RECORD" -c "autodrive_completion_state=$PRIVATE_STATE" \
    -c autodrive_round_label=round-1 -c autodrive_dry_run=true
) > "$WORK/round.out" 2> "$WORK/round.err" || fail 'recipe run autodrive-crusty-round'

jq -se --arg launch "$LAUNCH" '
  (map(.label) == ["context","review","assessor","decision"]) and
  all(.[]; .cwd == $launch and .git_checkout == true and .skip_count == 0)
' "$WORK/launches/events.jsonl" >/dev/null ||
  fail "codex_context_review_assessor_and_decision_in_pr_worktree (launch $LAUNCH)"
echo 'PASS: codex_context_review_assessor_and_decision_in_pr_worktree'
jq -e --arg head "$HEAD_SHA" '
  .crusty_verdict == "CLEAN" and .reviewed_head_sha == $head and .head_sha == $head and
  .commits_this_round == 0 and .concern_count == 0 and (has("review_generation") | not)
' "$RECORD" >/dev/null || fail "clean_verdict_matches_reviewed_head (head $HEAD_SHA)"
[ ! -s "$RECORD.findings" ] && [ ! -e "$RECORD.minor" ] && [ ! -e "$RECORD.severity" ] ||
  fail 'clean_verdict_matches_reviewed_head: unexpected findings, minor or severity sidecar'
echo 'PASS: clean_verdict_matches_reviewed_head'

# The loop normally publishes these copies after a round. Reproduce that
# publication, then validate completion through the production dispatcher.
cp -- "$RECORD" "$WORK/state/crusty-latest.json" || fail 'publish crusty-latest.json'
cp -- "$RECORD.findings" "$WORK/state/crusty-latest.json.findings" ||
  fail 'publish crusty-latest.json.findings'
HASH="$(fixture_git -C "$WORK/pr" hash-object --no-filters "$RECORD")" || fail 'hash round record'
printf 'round-1\tcrusty-round-1.json\t%s\n' "$HASH" > "$WORK/state/crusty-records.tsv"
scenario_env "$BIN" orch helper autodrive-completion-dispatch \
  --private-state "$PRIVATE_STATE" --child-exit-code 0 > "$WORK/completion.json" ||
  fail 'autodrive-completion-dispatch'
jq -e '.loop_result == "DONE" and .round_verdict == "CLEAN" and .evaluator_invoked == false' \
  "$WORK/completion.json" >/dev/null || fail 'real_completion_writer_and_dispatcher_complete (dispatch result)'
DECISION="$(jq -er .decision_file "$WORK/prepared.json")" || fail 'prepared.json has no decision_file'
jq -e '.action == "complete"' "$DECISION" >/dev/null ||
  fail 'real_completion_writer_and_dispatcher_complete (decision action)'
echo 'PASS: real_completion_writer_and_dispatcher_complete'
if ! cmp -s -- "$WORK/codex-config.before" "$WORK/home/.codex/config.toml"; then
  echo 'FAIL: Codex configuration changed'; exit 1
fi
if grep -Eq '^(pr comment|pr review|api) ' "$WORK/gh-calls"; then
  echo 'FAIL: unexpected GitHub write'; exit 1
fi
echo 'PASS: isolated_configuration_unchanged'
echo ALL_CASES_PASSED
