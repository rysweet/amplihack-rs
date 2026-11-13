#!/usr/bin/env bash
# Issue #1278: git's own environment must not leak into what the hooks run.
#
# git exports GIT_DIR (and, per hook, GIT_INDEX_FILE / GIT_PREFIX /
# GIT_WORK_TREE) into every hook. Those variables outrank the working
# directory when git picks a repository, so anything the hook starts --
# cargo, a test binary, a git subprocess -- operates on the hook's repository
# instead of the one it was pointed at. That made the pre-push `cargo test`
# gate unpassable, which is exactly the pressure that produces `--no-verify`.
#
# This test locks in all three halves of the fix:
#   1. scripts/git-hook-cargo.sh removes the leaked variables.
#   2. Its list stays identical to amplihack_git::REPOSITORY_ENV_VARS.
#   3. scripts/check-git-command-sanitised.sh actually rejects a raw
#      Command::new("git"), so the Rust-side fix cannot be silently opted out of.
#   4. The build cache the wrapper picks is specific to the checkout (#1381).
#      One hardcoded CARGO_TARGET_DIR shared by every worktree and parallel
#      agent on a host makes cargo reuse another branch's compiled metadata
#      against your source, so the hook fails on somebody else's code.
#
# Run: bash tests/issue_1278_git_env_isolation.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRAPPER="$REPO_ROOT/scripts/git-hook-cargo.sh"
GUARD="$REPO_ROOT/scripts/check-git-command-sanitised.sh"
HELPER_SRC="$REPO_ROOT/crates/amplihack-git/src/lib.rs"
PRECOMMIT="$REPO_ROOT/.pre-commit-config.yaml"

pass=0
fail=0
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT

# ---------------------------------------------------------------------------
# Scrub the inherited git environment BEFORE building any fixture.
#
# This is not ceremony. It is the bug under test biting the test itself:
# `git init <path>` honours an ambient GIT_DIR and *ignores its path argument*.
# Run from a git hook -- the exact environment this file exists to describe --
# the fixture repositories below are never created where the test believes they
# are. The control assertion then points GIT_DIR at a directory that was never
# created, fails, and reports "git no longer honours GIT_DIR the way this test
# assumes", which is the precise opposite of what happened.
#
# CI runs with a clean environment, so that gap is invisible there and shows up
# only for the people this change is written for. A test about an environment
# leak has to be correct in the presence of the leak.
#
# The list is read out of the Rust constant so it cannot drift away from
# amplihack_git::REPOSITORY_ENV_VARS.
# ---------------------------------------------------------------------------
# while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
rust_vars=(); while IFS= read -r _ml; do rust_vars+=("$_ml"); done < <(
    sed -n '/^pub const REPOSITORY_ENV_VARS/,/^];$/p' "$HELPER_SRC" \
        | grep -o '"GIT_[A-Z_]*"' | tr -d '"' | sort
)
if [ "${#rust_vars[@]}" -gt 0 ]; then
    unset "${rust_vars[@]}"
fi

# Never touch the real HOME: git reads config from it and can be made to write
# there. Every fixture in this file is disposable; the developer's HOME is not.
export HOME="$TMPROOT/home"
mkdir -p "$HOME"

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    if [ $# -gt 1 ] && [ -n "$2" ]; then
        printf '      %s\n' "$2"
    fi
    fail=$((fail + 1))
}

assert() {
    local desc="$1"
    shift
    if "$@"; then
        record_pass "$desc"
    else
        record_fail "$desc" "command failed: $*"
    fi
}

assert_eq() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "$actual" = "$expected" ]; then
        record_pass "$desc"
    else
        record_fail "$desc" "expected: <$expected>; actual: <$actual>"
    fi
}

# A hijacked fixture must fail loudly as a setup error rather than quietly as a
# wrong result three assertions later. `git init` returns success either way.
assert_repo_created() {
    local dir="$1"
    if [ -d "$dir/.git" ]; then
        record_pass "fixture: $(basename "$dir") is a real repository"
    else
        record_fail "fixture: $(basename "$dir") is a real repository" \
            "no $dir/.git -- an inherited GIT_DIR hijacked \`git init\`"
    fi
}

assert "scripts/git-hook-cargo.sh exists" test -x "$WRAPPER"
assert "scripts/check-git-command-sanitised.sh exists" test -x "$GUARD"

# ---------------------------------------------------------------------------
# 1. The wrapper removes every repository-selecting variable it is handed.
# ---------------------------------------------------------------------------

# rust_vars was parsed and scrubbed at the top of this file, before any fixture
# existed. Confirm it actually found something: an empty list would silently
# turn every check below into a no-op.
assert "REPOSITORY_ENV_VARS parsed from the Rust source" test "${#rust_vars[@]}" -gt 0

leaked=""
for var in "${rust_vars[@]}"; do
    # Set every one, then ask the wrapper what the child actually sees.
    seen="$(env "$var=/nonexistent/leaked" "$WRAPPER" printenv "$var" 2>/dev/null)"
    if [ -n "$seen" ]; then
        leaked="$leaked $var"
    fi
done
assert_eq "git-hook-cargo.sh scrubs every REPOSITORY_ENV_VARS entry" "" "$leaked"

# ...and leaves the commit identity alone, which selects an author, not a repo.
identity="$(env GIT_AUTHOR_NAME=preserved "$WRAPPER" printenv GIT_AUTHOR_NAME 2>/dev/null)"
assert_eq "git-hook-cargo.sh preserves GIT_AUTHOR_NAME" "preserved" "$identity"

# ---------------------------------------------------------------------------
# 2. The hooks that build or run code go through the wrapper.
# ---------------------------------------------------------------------------

for hook in artifact-guard cargo-clippy cargo-test; do
    entry="$(awk -v id="      - id: $hook" '
        $0 == id { found = 1; next }
        found && /^        entry:/ { print; exit }
    ' "$PRECOMMIT")"
    case "$entry" in
        *scripts/git-hook-cargo.sh*)
            record_pass "hook '$hook' runs through git-hook-cargo.sh" ;;
        *)
            record_fail "hook '$hook' runs through git-hook-cargo.sh" \
                "entry was: ${entry:-<not found>}" ;;
    esac
done

# ---------------------------------------------------------------------------
# 3. The guard rejects a raw Command::new("git") and accepts a clean tree.
# ---------------------------------------------------------------------------

clean_tree="$TMPROOT/clean/crates/demo/src"
mkdir -p "$clean_tree"
cat > "$clean_tree/lib.rs" <<'RS'
pub fn head() -> std::io::Result<std::process::Output> {
    amplihack_git::command().args(["rev-parse", "HEAD"]).output()
}
RS
assert "guard accepts a tree that uses amplihack_git::command()" \
    "$GUARD" "$TMPROOT/clean"

dirty_tree="$TMPROOT/dirty/crates/demo/src"
mkdir -p "$dirty_tree"
cat > "$dirty_tree/lib.rs" <<'RS'
pub fn head() -> std::io::Result<std::process::Output> {
    std::process::Command::new("git").args(["rev-parse", "HEAD"]).output()
}
RS
if "$GUARD" "$TMPROOT/dirty" >/dev/null 2>&1; then
    record_fail "guard rejects a raw Command::new(\"git\")" "guard exited 0"
else
    record_pass "guard rejects a raw Command::new(\"git\")"
fi

# interactive_command() may prompt (issue #1764), so the guard allows it only in
# the files it names. A new caller elsewhere must fail.
interactive_tree="$TMPROOT/interactive/crates/demo/src"
mkdir -p "$interactive_tree"
cat > "$interactive_tree/lib.rs" <<'RS'
pub fn clone(url: &str) -> std::io::Result<std::process::ExitStatus> {
    amplihack_git::interactive_command().args(["clone", url]).status()
}
RS
if "$GUARD" "$TMPROOT/interactive" >/dev/null 2>&1; then
    record_fail "guard rejects interactive_command() outside its allowlist" "guard exited 0"
else
    record_pass "guard rejects interactive_command() outside its allowlist"
fi

# The guard matches the whole word: without the call's bracket, an alias or a
# function value is still a new caller.
guard_rejects_sample() {
    local name="$1" sample="$2" tree="$TMPROOT/$1"
    mkdir -p "$tree/crates/demo/src"
    printf '%s\n' "$sample" > "$tree/crates/demo/src/lib.rs"
    if "$GUARD" "$tree" >/dev/null 2>&1; then
        record_fail "guard rejects $name" "guard exited 0"
    else
        record_pass "guard rejects $name"
    fi
}
guard_rejects_sample "alias-import" 'use amplihack_git::interactive_command as ic;'
guard_rejects_sample "alias-in-group" 'use amplihack_git::{command, interactive_command as ic};'
guard_rejects_sample "function-value" 'pub fn f() { let g = amplihack_git::interactive_command; g(); }'

# A word that merely contains the name is not a caller.
longer_tree="$TMPROOT/longer/crates/demo/src"
mkdir -p "$longer_tree"
echo 'pub fn not_interactive_command_helper() {}' > "$longer_tree/lib.rs"
assert "guard accepts a longer name that contains interactive_command" \
    "$GUARD" "$TMPROOT/longer"

# A scan that did not finish must not report OK. Each fake grep breaks one site
# and passes everything else to the real grep, so a pass here is not an accident
# of the first failure. PATH is set for the guard's command only. A fake rather
# than an unreadable file, because root can read that file.
real_grep="$(command -v grep)"
fake_grep_bin="$TMPROOT/fake-grep"
mkdir -p "$fake_grep_bin"
cat > "$fake_grep_bin/grep" <<SH
#!/bin/sh
case "\$FAKE_GREP_FAILS_ON \$*" in
    raw-search*'Command::new'*) exit 2 ;;
    filter*-v*) exit 2 ;;
    allowlist-search*'-w'*) exit 2 ;;
esac
exec "$real_grep" "\$@"
SH
chmod +x "$fake_grep_bin/grep"
for site in raw-search filter allowlist-search; do
    if out="$(FAKE_GREP_FAILS_ON="$site" PATH="$fake_grep_bin:$PATH" \
        "$GUARD" "$TMPROOT/clean" 2>&1)"; then
        record_fail "guard fails when grep fails at: $site" "guard exited 0: $out"
    else
        case "$out" in
            *OK:*) record_fail "guard fails when grep fails at: $site" "printed OK: $out" ;;
            *) record_pass "guard fails when grep fails at: $site" ;;
        esac
    fi
done

allowed_tree="$TMPROOT/allowed/crates/amplihack-cli/src/commands/plugin"
mkdir -p "$allowed_tree"
cp "$interactive_tree/lib.rs" "$allowed_tree/manager.rs"
assert "guard accepts interactive_command() in plugin/manager.rs" \
    "$GUARD" "$TMPROOT/allowed"

# Other branches' checkouts live under the gitignored worktrees/ directory of
# the main checkout and carry their own copy of crates/amplihack-git, which
# names git directly. They are not this tree's source and must not fail it.
nested="$TMPROOT/clean/worktrees/feat/other/crates/amplihack-git/src"
mkdir -p "$nested"
cat > "$nested/lib.rs" <<'RS'
pub fn command() -> std::process::Command {
    std::process::Command::new("git")
}
RS
assert "guard ignores other checkouts under worktrees/" \
    "$GUARD" "$TMPROOT/clean"

assert "guard passes on the real repository" "$GUARD"

# ---------------------------------------------------------------------------
# 4. The behaviour itself: an ambient GIT_DIR must not follow git around.
#
# This is the bug in one command. A directory that is not a repository must
# stay not-a-repository even when the environment names one somewhere else.
# ---------------------------------------------------------------------------

decoy="$TMPROOT/decoy"
plain="$TMPROOT/plain"
mkdir -p "$decoy" "$plain"
git init --quiet "$decoy" >/dev/null 2>&1
assert_repo_created "$decoy"

# Control: unsanitised, the ambient GIT_DIR hijacks the probe. If this ever
# stops being true the assertion below stops proving anything, so assert it.
if GIT_DIR="$decoy/.git" git -C "$plain" rev-parse --git-dir >/dev/null 2>&1; then
    record_pass "control: an ambient GIT_DIR hijacks an unsanitised git command"
else
    record_fail "control: an ambient GIT_DIR hijacks an unsanitised git command" \
        "git no longer honours GIT_DIR the way this test assumes"
fi

# Sanitised the way the wrapper does it: the plain directory stays plain.
if GIT_DIR="$decoy/.git" "$WRAPPER" git -C "$plain" rev-parse --git-dir >/dev/null 2>&1; then
    record_fail "a scrubbed environment leaves a non-repo a non-repo" \
        "git still resolved a repository for $plain"
else
    record_pass "a scrubbed environment leaves a non-repo a non-repo"
fi

# ---------------------------------------------------------------------------
# 5. The build cache is per-checkout (issue #1381).
#
# A constant CARGO_TARGET_DIR is the obvious "simplification" and it is exactly
# the bug: two checkouts sharing one cache makes cargo compile your source
# against another branch's metadata.
# ---------------------------------------------------------------------------

# Two distinct checkouts must not resolve to the same cache.
one="$TMPROOT/checkout-one"
two="$TMPROOT/checkout-two"
git init --quiet "$one" >/dev/null 2>&1
git init --quiet "$two" >/dev/null 2>&1
assert_repo_created "$one"
assert_repo_created "$two"

target_dir_for() {
    # `env -u` so an ambient CARGO_TARGET_DIR cannot mask the default.
    (cd "$1" && env -u CARGO_TARGET_DIR "$WRAPPER" printenv CARGO_TARGET_DIR)
}

dir_one="$(target_dir_for "$one")"
dir_two="$(target_dir_for "$two")"

if [ -n "$dir_one" ] && [ "$dir_one" != "$dir_two" ]; then
    record_pass "separate checkouts get separate build caches"
else
    record_fail "separate checkouts get separate build caches" \
        "both resolved to <$dir_one>"
fi

# ...but one checkout must stay stable across runs, or the cache is pointless.
assert_eq "the same checkout reuses its own cache" "$dir_one" "$(target_dir_for "$one")"

# An explicit value is a caller's stated intent and must win. Ambient state
# overriding explicit intent is the bug this whole change is about.
explicit="$(cd "$one" && CARGO_TARGET_DIR=/explicit/path "$WRAPPER" printenv CARGO_TARGET_DIR)"
assert_eq "an explicit CARGO_TARGET_DIR is not overridden" "/explicit/path" "$explicit"

# ---------------------------------------------------------------------------
# 6. Issue #2033: one shell copy of the lists, in amplifier-bundle/tools.
#
# auto-drive ran another repository's tests with GIT_* set and the tests'
# commits landed in the real PR worktree. Every shell step that runs a
# repository's tests sources git-repo-env.sh. The checks below fail CI when
# the helper and amplihack_git::REPOSITORY_ENV_VARS part ways, when a name
# stops being cleared, or when a refusal message changes.
# ---------------------------------------------------------------------------

REPO_ENV="$REPO_ROOT/amplifier-bundle/tools/git-repo-env.sh"
RUN_NAMES="AMPLIHACK_RECIPE_RUN_ID AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID AMPLIHACK_SESSION_TREE_DIR AMPLIHACK_TREE_ID"

if [ -f "$REPO_ENV" ] && [ ! -L "$REPO_ENV" ]; then
    record_pass "git-repo-env.sh exists and is not a symlink"
else
    record_fail "git-repo-env.sh exists and is not a symlink" "missing: $REPO_ENV"
fi

# helper_sh <script>: run <script> in a clean bash with the helper sourced
# under set -euo pipefail. env -i so nothing from this shell leaks in.
helper_sh() {
    env -i PATH="$PATH" HOME="$HOME" bash -c 'set -euo pipefail; . "$0"; '"$1" "$REPO_ENV" 2>&1
}

# 6.1 Same names, same order, as the Rust constant (unsorted, file order).
rust_order="$(sed -n '/^pub const REPOSITORY_ENV_VARS/,/^];$/p' "$HELPER_SRC" \
    | grep -o '"GIT_[A-Z_]*"' | tr -d '"' | tr '\n' ' ' | sed 's/ $//')"
shell_order="$(helper_sh 'printf "%s" "$AMPLIHACK_GIT_REPO_ENV_VARS"' | tr -s ' \n' ' ' | sed 's/^ //; s/ $//')"
printf '%s\n' $rust_order | sort > "$TMPROOT/rust.list"
printf '%s\n' $shell_order | sort > "$TMPROOT/shell.list"
drift="$(comm -3 "$TMPROOT/rust.list" "$TMPROOT/shell.list" | tr -d '\t' | tr '\n' ' ')"
assert_eq "helper list and REPOSITORY_ENV_VARS hold the same names (comm -3)" "" "$drift"
assert_eq "helper list is in REPOSITORY_ENV_VARS order" "$rust_order" "$shell_order"
assert_eq "helper lists 12 git names" "12" "$(printf '%s\n' $shell_order | grep -c .)"
assert_eq "helper run-identity list is the 5 names, in order" "$RUN_NAMES" \
    "$(helper_sh 'printf "%s" "$AMPLIHACK_RUN_IDENTITY_VARS"' | tr -s ' \n' ' ' | sed 's/^ //; s/ $//')"

# 6.2 Sourcing only defines things: it succeeds under set -euo pipefail,
# prints nothing, and clears nothing until a function is called.
src_out="$(env -i PATH="$PATH" GIT_DIR=/kept bash -c 'set -euo pipefail; . "$0"; echo "rc=$? dir=${GIT_DIR-unset}"' "$REPO_ENV" 2>&1)"
assert_eq "sourcing the helper changes nothing and prints nothing" "rc=0 dir=/kept" "$src_out"

# 6.3 Each git name clears, and only that name is recorded.
for var in $rust_order; do
    got="$(env -i PATH="$PATH" "$var=" bash -c 'set -euo pipefail; . "$1"; amplihack_clear_git_repo_env; echo "${!2+set}|$AMPLIHACK_GIT_ENV_CLEARED"' _ "$REPO_ENV" "$var" 2>&1)"
    assert_eq "clearing removes $var (even when empty) and records exactly it" "|$var" "$got"
done
assert_eq "with no git name set, AMPLIHACK_GIT_ENV_CLEARED is none" "none" \
    "$(helper_sh 'amplihack_clear_git_repo_env; printf %s "$AMPLIHACK_GIT_ENV_CLEARED"')"
all_set="$(for v in $rust_order; do printf '%s=/decoy ' "$v"; done)"
# shellcheck disable=SC2086 # word-splitting the NAME=value list is intended
assert_eq "with every git name set, all 12 are recorded in list order" "$rust_order" \
    "$(env -i PATH="$PATH" $all_set bash -c '. "$0"; amplihack_clear_git_repo_env; printf %s "$AMPLIHACK_GIT_ENV_CLEARED"' "$REPO_ENV" 2>&1)"
assert_eq "a forged AMPLIHACK_GIT_ENV_CLEARED is overwritten" "none" \
    "$(env -i PATH="$PATH" AMPLIHACK_GIT_ENV_CLEARED=forged bash -c '. "$0"; amplihack_clear_git_repo_env; printf %s "$AMPLIHACK_GIT_ENV_CLEARED"' "$REPO_ENV" 2>&1)"
assert_eq "an inherited list variable cannot shrink the list" "|GIT_DIR" \
    "$(env -i PATH="$PATH" GIT_DIR=/decoy AMPLIHACK_GIT_REPO_ENV_VARS=GIT_PREFIX bash -c '. "$0"; amplihack_clear_git_repo_env; echo "${GIT_DIR+set}|$AMPLIHACK_GIT_ENV_CLEARED"' "$REPO_ENV" 2>&1)"
assert_eq "an exported unset() function cannot stop the clear" "|GIT_DIR" \
    "$(env -i PATH="$PATH" GIT_DIR=/decoy bash -c 'unset() { :; }; export -f unset; exec bash -c '"'"'. "$0"; amplihack_clear_git_repo_env; echo "${GIT_DIR+set}|$AMPLIHACK_GIT_ENV_CLEARED"'"'"' "$0"' "$REPO_ENV" 2>&1)"
assert_eq "the git clear leaves commit identity and GIT_TERMINAL_PROMPT alone" "a|c|0" \
    "$(env -i PATH="$PATH" GIT_AUTHOR_NAME=a GIT_COMMITTER_NAME=c GIT_TERMINAL_PROMPT=0 bash -c '. "$0"; amplihack_clear_git_repo_env; echo "$GIT_AUTHOR_NAME|$GIT_COMMITTER_NAME|$GIT_TERMINAL_PROMPT"' "$REPO_ENV" 2>&1)"

# 6.4 Each run-identity name clears; depth and AMPLIHACK_HOME survive.
for var in $RUN_NAMES; do
    got="$(env -i PATH="$PATH" "$var=outer" bash -c 'set -euo pipefail; . "$1"; amplihack_clear_run_identity_env; echo "${!2+set}|$AMPLIHACK_RUN_ENV_CLEARED"' _ "$REPO_ENV" "$var" 2>&1)"
    assert_eq "run-identity clear removes $var and records exactly it" "|$var" "$got"
done
assert_eq "with no run-identity name set, AMPLIHACK_RUN_ENV_CLEARED is none" "none" \
    "$(helper_sh 'amplihack_clear_run_identity_env; printf %s "$AMPLIHACK_RUN_ENV_CLEARED"')"
assert_eq "AMPLIHACK_HOME, SESSION_DEPTH and MAX_DEPTH survive the run-identity clear" "/h|3|3" \
    "$(env -i PATH="$PATH" AMPLIHACK_HOME=/h AMPLIHACK_SESSION_DEPTH=3 AMPLIHACK_MAX_DEPTH=3 AMPLIHACK_TREE_ID=t bash -c '. "$0"; amplihack_clear_run_identity_env; echo "$AMPLIHACK_HOME|$AMPLIHACK_SESSION_DEPTH|$AMPLIHACK_MAX_DEPTH"' "$REPO_ENV" 2>&1)"
assert_eq "the run-identity clear leaves git names alone" "/decoy" \
    "$(env -i PATH="$PATH" GIT_DIR=/decoy bash -c '. "$0"; amplihack_clear_run_identity_env; echo "$GIT_DIR"' "$REPO_ENV" 2>&1)"
assert_eq "the git clear leaves run-identity names alone" "outer" \
    "$(env -i PATH="$PATH" AMPLIHACK_TREE_ID=outer bash -c '. "$0"; amplihack_clear_git_repo_env; echo "$AMPLIHACK_TREE_ID"' "$REPO_ENV" 2>&1)"

# 6.5 The helper is bash 3.2 safe: no arrays, no local, no declare -g, and
# none of the bash 4 array readers.
if grep -nE '^[^#]*(\blocal\b|\bmapfile\b|\breadarray\b|declare -g|=\(|\[@\])' "$REPO_ENV" >/dev/null 2>&1; then
    record_fail "git-repo-env.sh is bash 3.2 safe" "$(grep -nE '^[^#]*(\blocal\b|\bmapfile\b|\breadarray\b|declare -g|=\(|\[@\])' "$REPO_ENV")"
else
    record_pass "git-repo-env.sh is bash 3.2 safe"
fi

# 6.6 No hand-written copies: only the helper may list the names.
for copy in scripts/git-hook-cargo.sh \
            amplifier-bundle/tools/autodrive_qa_evidence.sh \
            amplifier-bundle/tools/autodrive_qa_sources.sh; do
    if grep -n 'GIT_QUARANTINE_PATH' "$REPO_ROOT/$copy" >/dev/null 2>&1; then
        record_fail "$copy has no hand-written copy of the git list" \
            "$(grep -n 'GIT_QUARANTINE_PATH' "$REPO_ROOT/$copy")"
    else
        record_pass "$copy has no hand-written copy of the git list"
    fi
done

# 6.6a autodrive_trust.sh keeps the one recorded copy: it is a merge-guard
# GATE_FILE and does not load git-repo-env.sh (see its comment). Its list must
# be the helper's 12 names in order, then GIT_REPLACE_REF_BASE, and
# autodrive_trust_git must remove every one of them before git runs.
TRUST_SH="$REPO_ROOT/amplifier-bundle/tools/autodrive_trust.sh"
trust_list="$(env -i PATH="$PATH" HOME="$HOME" AUTODRIVE_TRUST_GIT_UNSET_VARS=GIT_DIR bash -c '. "$0" >/dev/null 2>&1; printf "%s" "$AUTODRIVE_TRUST_GIT_UNSET_VARS"' "$TRUST_SH" 2>&1)"
assert_eq "autodrive_trust.sh unsets git-repo-env.sh's list, then GIT_REPLACE_REF_BASE" \
    "$shell_order GIT_REPLACE_REF_BASE" "$trust_list"
TRUST_REPO="$TMPROOT/trust-repo"
git init -q "$TRUST_REPO"
trust_seen="$(for v in $shell_order GIT_REPLACE_REF_BASE; do printf '%s=/decoy ' "$v"; done)"
# The alias prints git's environment, then a marker, so a git that refused to
# start (as it would with GIT_DIR=/decoy) cannot pass for a clean one.
# shellcheck disable=SC2086 # word-splitting the NAME=value list is intended
trust_out="$(env -i PATH="$PATH" HOME="$HOME" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null $trust_seen \
    bash -c '. "$0" >/dev/null 2>&1; autodrive_trust_git "$1" -c alias.envdump="!env; echo TRUST_GIT_RAN" envdump' "$TRUST_SH" "$TRUST_REPO" 2>&1)"
case "$trust_out" in
    *TRUST_GIT_RAN*) assert_eq "autodrive_trust_git passes none of the 13 names to git" "" \
        "$(printf '%s\n' "$trust_out" | grep -E '^GIT_[A-Z_]+=/decoy$' | tr '\n' ' ')" ;;
    *) record_fail "autodrive_trust_git passes none of the 13 names to git" "git did not run: $trust_out" ;;
esac

# 6.7 The hook refuses a missing or symlinked helper, by exact message, and
# runs nothing. A scratch tree holds a copy of the hook so the real helper is
# never moved.
hook_refuses() {
    local label="$1" tree="$TMPROOT/hook-$1" out rc
    mkdir -p "$tree/scripts" "$tree/amplifier-bundle/tools"
    cp "$WRAPPER" "$tree/scripts/git-hook-cargo.sh"
    if [ "$label" = symlink ]; then
        ln -s "$REPO_ENV" "$tree/amplifier-bundle/tools/git-repo-env.sh"
    fi
    out="$(cd "$tree" && bash "$tree/scripts/git-hook-cargo.sh" sh -c 'echo RAN' 2>&1)"; rc=$?
    case "$out" in
        *"ERROR: git-repo-env.sh missing or a symlink"*"(#2033)"*) ;;
        *) record_fail "hook refuses a $label helper with the #2033 message" "got: $out"; return ;;
    esac
    case "$out" in
        *RAN*) record_fail "hook refuses a $label helper with the #2033 message" "the command ran: $out"; return ;;
    esac
    assert_eq "hook refuses a $label helper with the #2033 message and exit 1" "1" "$rc"
}
hook_refuses missing
hook_refuses symlink

# 6.8 The recipe steps carry the exact STOP lines the reference documents.
EVIDENCE_YAML="$REPO_ROOT/amplifier-bundle/recipes/autodrive-merge-evidence.yaml"
ROUND_YAML="$REPO_ROOT/amplifier-bundle/recipes/autodrive-merge-round.yaml"
step_body() { awk -v id="  - id: \"$2\"" '$0 == id { on = 1; print; next } on && /^  - id:/ { exit } on { print }' "$1"; }
case "$(step_body "$EVIDENCE_YAML" step-02-qa-team-scenarios)" in
    *'STOP: cannot load git-repo-env.sh from the tools directory (#2033)'*)
        record_pass "merge-evidence step-02 has the exact #2033 STOP line" ;;
    *) record_fail "merge-evidence step-02 has the exact #2033 STOP line" "not found in step-02" ;;
esac
# #2342: the CI read, the QA gate and the blocked QA path are bash steps that
# run gh or git, so each stops with exit 1 when the helper cannot be loaded.
for sid in step-03b-expected-checks step-02a-ci-gate step-02b-qa-blocked-on-ci; do
    case "$(step_body "$EVIDENCE_YAML" "$sid")" in
        *'type: "bash"'*'STOP: cannot load git-repo-env.sh from the tools directory (#2033)" >&2; exit 1; }'*)
            record_pass "merge-evidence $sid has the exact #2033 STOP line" ;;
        *) record_fail "merge-evidence $sid has the exact #2033 STOP line" "not found in $sid" ;;
    esac
done
# Step-01 is an agent step (#2342) that may merge and push, so bash step-00
# runs the loading line first and stops the recipe (exit 1) when it fails,
# before the agent starts. The agent's own shells are other processes, so its
# prompt still carries the loading line and forbids every git write when that
# line fails, returning the git-repo-env-unavailable no-update decision.
case "$(step_body "$EVIDENCE_YAML" step-00-git-env-guard)" in
    *'type: "bash"'*'STOP: cannot load git-repo-env.sh from the tools directory (#2033)" >&2; exit 1; }'*)
        record_pass "merge-evidence step-00 enforces the #2033 STOP before the step-01 agent" ;;
    *) record_fail "merge-evidence step-00 enforces the #2033 STOP before the step-01 agent" "not found in step-00-git-env-guard" ;;
esac
first_step="$(grep -m1 -E '^  - id: ' "$EVIDENCE_YAML")"
case "$first_step" in
    *'"step-00-git-env-guard"'*) record_pass "merge-evidence step-00 is the first step" ;;
    *) record_fail "merge-evidence step-00 is the first step" "first step is: $first_step" ;;
esac
step01="$(step_body "$EVIDENCE_YAML" step-01-sync-with-base)"
step01_has() {
    case "$step01" in
        *"$2"*) record_pass "merge-evidence step-01 $1" ;;
        *) record_fail "merge-evidence step-01 $1" "not found in step-01: $2" ;;
    esac
}
step01_has "loads git-repo-env.sh, refusing a symlink" \
    '[ ! -L "$TOOLS_D/git-repo-env.sh" ] && . "$TOOLS_D/git-repo-env.sh" && amplihack_clear_git_repo_env && amplihack_clear_run_identity_env'
step01_has "forbids git writes when the helper is unavailable" \
    'no git write of any kind'
step01_has "returns git-repo-env-unavailable when the helper is unavailable" \
    '"git-repo-env-unavailable"'
case "$step01" in
    *'STOP: cannot load git-repo-env.sh'*)
        record_fail "merge-evidence step-01 has no STOP line it cannot act on" "step-01 is an agent step" ;;
    *) record_pass "merge-evidence step-01 has no STOP line it cannot act on" ;;
esac
case "$(step_body "$ROUND_YAML" step-04-address-blockers)" in
    *'STOP: cannot load git-repo-env.sh from the tools directory (#2033)'*)
        record_pass "merge-round step-04 has the exact #2033 STOP line" ;;
    *) record_fail "merge-round step-04 has the exact #2033 STOP line" "not found in step-04" ;;
esac

echo
echo "passed: $pass, failed: $fail"
if [ "$fail" -eq 0 ]; then
    echo "PASS: git env isolation (12 git names, 5 run-identity names)"
fi
[ "$fail" -eq 0 ]
