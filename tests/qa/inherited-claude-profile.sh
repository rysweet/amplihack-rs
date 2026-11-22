#!/usr/bin/env bash
# Outside-in check for issue #2922: `cargo test`, started from a shell that
# carries an agent launch's login profile, must leave that profile unchanged.
#
# The shell exports the five profile-redirecting variables, each pointing at a
# stand-in built under a temporary folder, as an auto-switch launch would:
# CLAUDE_CONFIG_DIR at a stand-in "real" profile, AMPLIHACK_CLAUDE_PROFILE and
# AMPLIHACK_CLAUDE_PROFILE_ROOT at a stand-in profile root, AMPLIHACK_HOME at a
# stand-in amplihack home, and AMPLIHACK_CLAUDE_AUTO_SWITCH=1. HOME is a
# temporary folder too, holding stand-in `.claude` and `.amplihack` folders, so
# no part of the run can reach the real ones. The tests then run, and every
# stand-in must be byte-for-byte what it was before.
#
# Usage: tests/qa/inherited-claude-profile.sh install|workspace
#   install    the amplihack-cli install tests, which drive the settings
#              writers named in the issue (launch_repair and friends)
#   workspace  the pre-push command, `cargo test --workspace`, which is the
#              command that overwrote a live profile on 21 November 2026
#
# QA_EVIDENCE_DIR, when set, receives the cargo log and both manifests.
# A failure names changed paths and digests only, never file contents.
set -euo pipefail

scenario=${1:?expected install or workspace}
case "$scenario" in
    install)
        test_cmd=(cargo test -p amplihack-cli --lib --locked -- commands::install::)
        ;;
    workspace)
        # The same arguments as the pre-push hook in .pre-commit-config.yaml.
        test_cmd=(cargo test --workspace --locked --no-fail-fast --
            --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu)
        ;;
    *)
        echo "unknown scenario: $scenario (expected install or workspace)" >&2
        exit 2
        ;;
esac

source_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# The toolchain stays where the caller has it; only HOME moves.
caller_cargo_home=${CARGO_HOME:-$HOME/.cargo}
caller_rustup_home=${RUSTUP_HOME:-$HOME/.rustup}

fixture=$(mktemp -d "${TMPDIR:-/tmp}/amplihack-qa-2922.XXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
chmod 0700 "$fixture"

outer_home="$fixture/home"
real_profile="$fixture/claude-profiles/stand-in-acct"
profile_root="$fixture/claude-profiles"
amplihack_home="$fixture/amplihack-home"
mkdir -p "$outer_home/.claude" "$outer_home/.amplihack" "$real_profile" "$amplihack_home"
chmod 0700 "$outer_home" "$profile_root" "$real_profile" "$amplihack_home"

# A live profile's settings in the shape the issue describes: a model, an env
# block, a theme and an owner's PreToolUse guard, and no amplihack hooks, so a
# launch repair that reached it would rewrite it. All values are placeholders.
cat > "$real_profile/settings.json" <<'SETTINGS'
{
  "model": "stand-in-owner-model",
  "theme": "stand-in-theme",
  "env": {"XPIA_ENABLED": "true", "STAND_IN_PLACEHOLDER": "not-a-secret"},
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "/stand-in/bin/block-tmux-kill"}]}
    ]
  }
}
SETTINGS
printf 'stand-in profile notes\n' > "$real_profile/CLAUDE.md"
cp "$real_profile/settings.json" "$outer_home/.claude/settings.json"
printf 'stand-in amplihack state\n' > "$amplihack_home/marker.txt"
printf 'stand-in amplihack state\n' > "$outer_home/.amplihack/marker.txt"
# Commits made by tests need an identity; the caller's own git config is not read.
printf '[user]\n\tname = QA Stand-in\n\temail = qa-stand-in@example.invalid\n' \
    > "$outer_home/.gitconfig"

protected=("$profile_root" "$amplihack_home" "$outer_home/.claude" "$outer_home/.amplihack")

# One line per entry: type, mode, digest (files only) and path. A rewrite with
# different bytes, a new file, a removed file or a mode change all show.
manifest() {
    local root
    for root in "${protected[@]}"; do
        find "$root" -printf '%y %m %p\n' | LC_ALL=C sort | while read -r kind mode path; do
            if [ "$kind" = f ]; then
                printf '%s %s %s %s\n' "$kind" "$mode" "$(sha256sum < "$path" | cut -d' ' -f1)" "$path"
            else
                printf '%s %s - %s\n' "$kind" "$mode" "$path"
            fi
        done
    done
}

manifest > "$fixture/before.manifest"
echo "stand-in entries before: $(wc -l < "$fixture/before.manifest")"
echo "running: ${test_cmd[*]}"

status=0
# AMPLIHACK_RUN_INSTALL_TESTS=1 below: a host cargo wrapper may skip the install
# tests on a checkout whose branch changes no install code, and those tests
# drive the settings writers this check is about. Plain cargo ignores it.
(
    cd "$source_root"
    # Whatever the caller's shell held for these is replaced, not inherited.
    exec env HOME="$outer_home" \
        CARGO_HOME="$caller_cargo_home" RUSTUP_HOME="$caller_rustup_home" \
        CLAUDE_CONFIG_DIR="$real_profile" \
        AMPLIHACK_CLAUDE_PROFILE=stand-in-acct \
        AMPLIHACK_CLAUDE_PROFILE_ROOT="$profile_root" \
        AMPLIHACK_CLAUDE_AUTO_SWITCH=1 \
        AMPLIHACK_HOME="$amplihack_home" \
        GIT_TERMINAL_PROMPT=0 \
        AMPLIHACK_RUN_INSTALL_TESTS=1 \
        "${test_cmd[@]}" </dev/null
) > "$fixture/cargo.log" 2>&1 || status=$?

manifest > "$fixture/after.manifest"

if [ -n "${QA_EVIDENCE_DIR:-}" ]; then
    mkdir -p "$QA_EVIDENCE_DIR"
    cp "$fixture/cargo.log" "$fixture/before.manifest" "$fixture/after.manifest" "$QA_EVIDENCE_DIR/"
fi

# The test summary lines, so a reader sees what ran without the whole log.
grep -E '^test result:|^error(\[|:)|^failures:|^    [a-z_:0-9]+$' "$fixture/cargo.log" | tail -n 60 || true
echo "cargo exit: $status"
passed=$(sed -n 's/^test result: [a-zA-Z]*\. \([0-9]*\) passed.*/\1/p' "$fixture/cargo.log" \
    | awk '{ total += $1 } END { print total + 0 }')
echo "tests passed: $passed"

if ! cmp -s "$fixture/before.manifest" "$fixture/after.manifest"; then
    echo "FAIL: a stand-in profile or home changed during '${test_cmd[*]}':" >&2
    diff "$fixture/before.manifest" "$fixture/after.manifest" >&2 || true
    exit 1
fi
echo "stand-in entries after: $(wc -l < "$fixture/after.manifest") (byte-for-byte unchanged)"

if [ "$passed" -eq 0 ]; then
    echo "FAIL: no test ran, so an unchanged stand-in proves nothing" >&2
    exit 1
fi

if [ "$status" -ne 0 ]; then
    echo "FAIL: the stand-ins are unchanged, but '${test_cmd[*]}' exited $status" >&2
    exit "$status"
fi
printf 'PASS: %s leaves an inherited Claude profile, profile root and amplihack home unchanged\n' "$scenario"
