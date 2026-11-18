#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2346: under umask 0002
# a recipe run's recovery store stopped the run, and the message named no
# folder.
#
# Since #2334 `amplihack recipe run` opens its recovery store through the
# pinned recipe runner's private_fs::Directory::open, which stops any work
# root with a folder above it that is group- or world-writable without the
# sticky bit, and requires the work root itself to be owned and mode 0700.
# Under umask 0002, Ubuntu's default for a user with a private group,
# amplihack created ~/.amplihack at 0775 itself, so the first run in a fresh
# HOME ended at once with `Error: unsafe recovery ancestor`, naming neither
# the folder nor the mode bit. CI runs at umask 0022, where nothing showed;
# this script sets umask 0002 itself.
#
# Black boxes, run unchanged: the amplihack binary under test and the pinned
# recipe-runner-rs it starts. The recipe has one bash step and no agent step,
# so no agent CLI is started; `claude`, `copilot` and `codex` on PATH are
# stand-ins that only record that they were called, which fails the scenario.
#
# Cases, each in a fresh HOME under the scratch folder:
#   recipe_run_starts_under_umask_0002   a 0750 HOME and the default work root:
#                                        the run succeeds, ~/.amplihack is 700,
#                                        and every folder under the work root is
#                                        700 and every file 600
#   group_writable_ancestor_is_named     a 0775 folder made by hand above the
#                                        work root: the run stops, and stderr
#                                        names the folder, group-writable, 0775
#                                        and `chmod g-w <folder>`; the work root
#                                        is not created
#   owned_nonprivate_work_roots_unchanged  owned 0755, 0750 and 0775 roots:
#                                        each run stops with path, mode and
#                                        chmod 700 correction; the root stays
#                                        unchanged and no step runs
#
# Every scenario command runs under `env -i` with an explicit allowlist (see
# scenario_env below): no token, key or setting from the caller's environment
# reaches it. Everything is created under one mktemp folder that the EXIT trap
# removes; nothing is written to the checkout except by the cargo build.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script directly against the amplihack binary that job installs, after
# installing the pinned recipe runner. CI does not run gadugi-test itself;
# tests/gadugi/scenarios/issue-2346-recovery-store-under-umask-0002.yaml wraps
# this script for anyone running it through gadugi-test by hand.
#
# Run by hand: bash tests/gadugi/run-issue-2346-umask-recovery-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2346_QA_BIN to an
# amplihack binary to skip the build. The pinned recipe-runner-rs comes from
# RECIPE_RUNNER_RS_PATH or PATH; without it the scenario fails rather than
# skipping. On success it prints ALL_CASES_PASSED and exits 0; otherwise
# SCENARIO_FAILED and exit 1.
#
# It is bash, not Rust, because every gadugi driver is bash and because it
# runs the installed binary exactly as a user's shell does.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

for c in find stat id; do
  p="$(command -v "$c" 2>/dev/null)"
  case "$p" in
    /*) ;;
    *) echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1 ;;
  esac
done

RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs 2>/dev/null || true)}"
case "$RUNNER" in
  /*) ;;
  *) echo "FAIL: recipe-runner-rs is required (RECIPE_RUNNER_RS_PATH or PATH); install it with scripts/install-pinned-recipe-runner.sh"
     echo "SCENARIO_FAILED"; exit 1 ;;
esac
[ -x "$RUNNER" ] || { echo "FAIL: $RUNNER is not executable"; echo "SCENARIO_FAILED"; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2346-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
trap 'chmod -R u+rwx -- "${WORK:?}" 2>/dev/null; rm -rf -- "${WORK:?}"' EXIT
# The cases compare paths in messages and suggested commands literally, and
# the recipe embeds a path unquoted, so the scratch path must need no quoting.
case "$WORK" in
  *[!A-Za-z0-9/._-]*)
    echo "FAIL: the scratch path $WORK holds a character that needs quoting; set TMPDIR to a plain path"
    echo "SCENARIO_FAILED"; exit 1 ;;
esac

mode_of() { stat -c %a -- "$1" 2>/dev/null || stat -f %Lp -- "$1" 2>/dev/null; }
owner_of() { stat -c %u -- "$1" 2>/dev/null || stat -f %u -- "$1" 2>/dev/null; }

# The scratch folder must be this user's, private, and below folders the
# recovery store accepts; otherwise every case would fail for a reason outside
# the binary under test.
if [ "$(owner_of "$WORK")" != "$(id -u)" ] || [ "$(mode_of "$WORK")" != 700 ]; then
  echo "FAIL: $WORK is not a private folder owned by this user"; echo "SCENARIO_FAILED"; exit 1
fi
dir="$WORK"
while [ "$dir" != / ]; do
  dir="$(dirname -- "$dir")"
  m="$(mode_of "$dir")"
  if [ -L "$dir" ] || { [ $(( 8#$m & 8#022 )) -ne 0 ] && [ $(( 8#$m & 8#1000 )) -eq 0 ]; }; then
    echo "FAIL: $dir (mode $m) above the scratch folder is one the recovery store stops; set TMPDIR to a folder under folders only you can write"
    echo "SCENARIO_FAILED"; exit 1
  fi
done

SYS_PATH="$(dirname -- "$RUNNER"):/usr/bin:/bin"

# scenario_env <home> <command...>: runs the command with only the variables
# below, plus any NAME=value the caller puts before the command.
#   PATH                         the agent stand-ins first, then the runner's
#                                folder and the system tools
#   HOME, TMPDIR                 scratch folders, so nothing reads or writes the
#                                caller's
#   LANG                         a fixed locale
#   GIT_CONFIG_GLOBAL/NOSYSTEM   no user or system git config
#   GIT_TERMINAL_PROMPT          git never waits on a credential prompt
#   RECIPE_RUNNER_RS_PATH        the pinned runner
#   AMPLIHACK_AGENT_BINARY       claude: a name; $WORK/bin/claude is the stand-in
#   AMPLIHACK_SKIP_AUTO_INSTALL, AMPLIHACK_NONINTERACTIVE,
#   AMPLIHACK_NO_FRESHNESS_CHECK, AMPLIHACK_SKIP_POWER_STEERING,
#   AMPLIHACK_SKIP_REFLECTION    no install, prompt, network check or hook
#   AMPLIHACK_ROTATE_AUTOSTART=0 no login rotator is started
#   AMPLIHACK_GOVERNOR=off, AMPLIHACK_USAGE_LIMIT_WAIT=0,
#   AMPLIHACK_CLEANUP_ON_EXIT=0  no capacity wait and no end-of-run sweep
scenario_env() {
  local home="$1"
  shift
  env -i \
    PATH="$WORK/bin:$SYS_PATH" \
    HOME="$home" \
    TMPDIR="$WORK/tmp" \
    LANG=C \
    GIT_CONFIG_GLOBAL=/dev/null \
    GIT_CONFIG_NOSYSTEM=1 \
    GIT_TERMINAL_PROMPT=0 \
    RECIPE_RUNNER_RS_PATH="$RUNNER" \
    AMPLIHACK_AGENT_BINARY=claude \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_NONINTERACTIVE=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_SKIP_POWER_STEERING=1 \
    AMPLIHACK_SKIP_REFLECTION=1 \
    AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_GOVERNOR=off \
    AMPLIHACK_USAGE_LIMIT_WAIT=0 \
    AMPLIHACK_CLEANUP_ON_EXIT=0 \
    "$@"
}

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2346_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # As run-issue-2778-umask-crusty-scenario.sh does: one awk reads every
  # artifact record and prints the last executable path, so cargo never writes
  # into a closed pipe and pipefail reports cargo's status.
  if ! BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -n 40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$BIN" in
  /*) ;;
  *) BIN="$PWD/$BIN" ;;
esac
echo "binary under test: $BIN"
echo "recipe runner: $RUNNER"

mkdir -m 700 -- "$WORK/bin" "$WORK/tmp" "$WORK/records" || { echo "FAIL: cannot create the scratch folders"; echo "SCENARIO_FAILED"; exit 1; }

cat > "$WORK/agent-stub" <<'STUB'
#!/usr/bin/env bash
# No agent may start in this scenario: record the call and fail.
printf '%s %s\n' "$(basename -- "$0")" "$*" >> "$(cd -- "$(dirname -- "$0")/.." && pwd -P)/unexpected.log"
exit 2
STUB
stubs_ok=1
chmod 755 -- "$WORK/agent-stub" || stubs_ok=0
for a in claude copilot codex; do
  cp -p -- "$WORK/agent-stub" "$WORK/bin/$a" || stubs_ok=0
done
[ "$stubs_ok" = 1 ] || { echo "FAIL: could not write the stand-ins"; echo "SCENARIO_FAILED"; exit 1; }

# write_recipe <file> <marker>: one bash step that writes QA_2346_STEP_RAN to
# <marker>. The scratch path was checked above to need no quoting.
write_recipe() {
  cat > "$1" <<YAML
name: issue-2346-umask
description: one bash step
version: 1.0.0
context: {}
steps:
  - id: step
    type: bash
    command: |
      printf '%s' QA_2346_STEP_RAN > $2
YAML
}

# not_private <dir>: every entry at or below <dir> that is not a 0700 folder
# or a 0600 regular file, one per line.
not_private() {
  find "$1" \( -type d ! -perm 700 \) -o \( -type f ! -perm 600 \) -o \( ! -type d ! -type f \) 2>&1
}

# make_dir <mode> <dir>: mkdir with an explicit mode, or stop the scenario.
make_dir() {
  mkdir -m "$1" -- "$2" || { echo "FAIL: cannot create ${2#"$WORK"/}"; echo "SCENARIO_FAILED"; exit 1; }
}

fail=0
pass() { echo "PASS: $1 - $2"; }
fl() { echo "FAIL: $1 - $2"; fail=1; }
tail_of() { tail -n 6 -- "$1" 2>/dev/null | tr '\n' ' '; }

# Every case sets umask 0002 in its own subshell, after the scratch folders
# exist, exactly as a user's shell with that umask would.

# --- case 1: a recipe run starts under umask 0002 in a fresh HOME -------------
C1="$WORK/c1"
make_dir 700 "$C1"; make_dir 750 "$C1/home"; make_dir 700 "$C1/project"
write_recipe "$C1/recipe.yaml" "$WORK/records/c1"
(
  umask 0002
  cd -- "$C1/project" || exit 11
  scenario_env "$C1/home" "$BIN" recipe run "$C1/recipe.yaml" -w "$C1/project" --no-detach </dev/null
) >"$C1/out" 2>"$C1/err"; RC1=$?
OPEN1="$( [ -d "$C1/home/.amplihack/work" ] && not_private "$C1/home/.amplihack/work" )"
if [ "$RC1" -eq 0 ] && [ "$(cat "$WORK/records/c1" 2>/dev/null)" = QA_2346_STEP_RAN ] \
   && [ "$(mode_of "$C1/home/.amplihack")" = 700 ] && [ -d "$C1/home/.amplihack/work" ] && [ -z "$OPEN1" ]; then
  pass recipe_run_starts_under_umask_0002 "the run succeeded, ~/.amplihack is 700, and everything under the work root is 700 or 600"
else
  fl recipe_run_starts_under_umask_0002 "rc=$RC1 ~/.amplihack mode '$(mode_of "$C1/home/.amplihack")' not private: ${OPEN1//"$C1"\//} stderr: $(tail_of "$C1/err")"
fi

# --- case 2: a group-writable folder above the work root is named -------------
C2="$WORK/c2"
make_dir 700 "$C2"; make_dir 700 "$C2/home"; make_dir 700 "$C2/project"; make_dir 775 "$C2/shared"
write_recipe "$C2/recipe.yaml" "$WORK/records/c2"
(
  umask 0002
  cd -- "$C2/project" || exit 11
  scenario_env "$C2/home" AMPLIHACK_WORK_ROOT="$C2/shared/work" \
    "$BIN" recipe run "$C2/recipe.yaml" -w "$C2/project" --no-detach </dev/null
) >"$C2/out" 2>"$C2/err"; RC2=$?
missing2=""
for part in "$C2/shared" group-writable 0775 "chmod g-w $C2/shared"; do
  grep -qF -- "$part" "$C2/err" || missing2="$missing2 [$part]"
done
if [ "$RC2" -ne 0 ] && [ -z "$missing2" ] && [ ! -e "$C2/shared/work" ] && [ ! -e "$WORK/records/c2" ] \
   && [ "$(mode_of "$C2/shared")" = 775 ]; then
  pass group_writable_ancestor_is_named "the run stopped, and stderr named the folder, group-writable, 0775 and chmod g-w"
else
  fl group_writable_ancestor_is_named "rc=$RC2 missing from stderr:${missing2:- none} work root created: $([ -e "$C2/shared/work" ] && echo yes || echo no) stderr: $(tail_of "$C2/err")"
fi

# --- case 3: existing nonprivate work roots are left unchanged ----------------
C3="$WORK/c3"
make_dir 700 "$C3"
roots_ok=1
for mode3 in 755 750 775; do
  case3="$C3/$mode3"
  make_dir 700 "$case3"; make_dir 700 "$case3/home"; make_dir 700 "$case3/project"
  make_dir "$mode3" "$case3/work"
  write_recipe "$case3/recipe.yaml" "$WORK/records/c3-$mode3"
  (
    umask 0002
    cd -- "$case3/project" || exit 11
    scenario_env "$case3/home" AMPLIHACK_WORK_ROOT="$case3/work" \
      "$BIN" recipe run "$case3/recipe.yaml" -w "$case3/project" --no-detach </dev/null
  ) >"$case3/out" 2>"$case3/err"; RC3=$?
  missing3=""
  for part in "$case3/work" "0$mode3" "chmod 700 $case3/work"; do
    grep -qF -- "$part" "$case3/err" || missing3="$missing3 [$part]"
  done
  if [ "$RC3" -eq 0 ] || [ "$(mode_of "$case3/work")" != "$mode3" ] \
     || [ -n "$missing3" ] || [ -e "$WORK/records/c3-$mode3" ] \
     || [ -n "$(find "$case3/work" -mindepth 1 -print -quit)" ]; then
    roots_ok=0
    fl owned_nonprivate_work_roots_unchanged "mode=$mode3 rc=$RC3 actual mode '$(mode_of "$case3/work")' missing:${missing3:- none} stderr: $(tail_of "$case3/err")"
  fi
done
if [ "$roots_ok" -eq 1 ]; then
  pass owned_nonprivate_work_roots_unchanged "0755, 0750 and 0775 roots stopped each run unchanged with path, mode and chmod 700 correction"
fi

# --- no agent started in any case ----------------------------------------------
if [ -s "$WORK/unexpected.log" ]; then
  fl no_agent_started "unexpected: $(tr '\n' '|' < "$WORK/unexpected.log")"
fi

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
