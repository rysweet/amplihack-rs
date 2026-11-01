#!/usr/bin/env bash
# Guard: every git subprocess in Rust code goes through amplihack-git (issue #1278).
#
# `Command::new("git")` inherits GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE from
# whatever ran it. Under a git hook those are always set, and they outrank
# `.current_dir()`, so the command silently operates on the hook's repository
# rather than the one the caller pointed at. That made the pre-push `cargo test`
# gate unpassable and it makes production code that runs under a hook wrong.
#
# `amplihack_git::command()` / `command_in()` / `tokio_command()` /
# `tokio_command_in()` clear those variables first, and also set
# GIT_TERMINAL_PROMPT=0 so a credential prompt fails at once instead of
# stopping the run (issue #1764). This guard exists because the fix is one a
# new call site can silently opt out of just by typing the obvious thing.
set -euo pipefail

# Optional argument: the tree to scan. Defaults to the repository root, and
# exists so tests can point the guard at a fixture instead of the real tree.
cd "${1:-$(dirname "$0")/..}"

# The helper crate is the one place allowed to name git directly: it is the
# implementation, and its tests deliberately spawn an unsanitised command as the
# control that proves an ambient GIT_DIR really does hijack one.
allowed_prefix="crates/amplihack-git/"

# search_rs <grep args>: set $found to the matches in the tree's Rust files.
# No match is fine; a scan that did not finish stops the guard. This runs in the
# main shell, not a pipeline or `$(...)`, so the exit status and the `exit` both
# count.
search_rs() {
    local rc=0
    found=$(grep -rn --include='*.rs' \
        --exclude-dir=target --exclude-dir=.git --exclude-dir=worktrees \
        "$@" . | sed 's|^\./||') || rc=$?
    if [ "$rc" -gt 1 ]; then
        echo "ERROR: grep failed (exit $rc), so the scan did not finish." >&2
        exit 2
    fi
}

# drop_prefix <prefix>: filter stdin. grep exits 1 when no lines are left, which
# is the good outcome; anything higher fails the pipeline, and with it the guard.
drop_prefix() {
    grep -v "^$1" || [ $? -eq 1 ]
}

search_rs 'Command::new("git")'
hits=$(printf '%s\n' "$found" | drop_prefix "$allowed_prefix")

if [ -n "$hits" ]; then
    echo "ERROR: raw Command::new(\"git\") found (issue #1278)." >&2
    echo >&2
    echo "$hits" >&2
    echo >&2
    echo "A raw git command inherits GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE from the" >&2
    echo "process that spawned it. Under a git hook those are always set and they" >&2
    echo "override .current_dir(), so the command operates on the wrong repository." >&2
    echo >&2
    echo "Use instead:" >&2
    echo "  amplihack_git::command()                // std, cwd-based" >&2
    echo "  amplihack_git::command_in(dir)          // std, explicit directory" >&2
    echo "  amplihack_git::tokio_command()          // async callers" >&2
    echo "  amplihack_git::tokio_command_in(dir)    // async, explicit directory" >&2
    echo "  amplihack_git::interactive_command()    // may prompt: user-invoked only" >&2
    echo "  amplihack_git::scrub(&mut cmd)          // non-git child that runs git" >&2
    echo >&2
    echo "Add amplihack-git = { workspace = true } to the crate's [dependencies]." >&2
    exit 1
fi

# `amplihack_git::interactive_command()` leaves GIT_TERMINAL_PROMPT alone, so
# git may stop on a credential prompt: the #1764 hang. It is for a person at a
# terminal who typed the URL, nothing else. Typing it is as easy as typing
# `Command::new("git")`, so each caller is named here and a new one fails until
# a reviewer adds it. The helper crate defines and tests it. The search matches
# the whole word so an alias or a function value is caught too, which also means
# a comment naming it outside the allowlist fails.
interactive_allowed=(
    "crates/amplihack-git/"
    "crates/amplihack-cli/src/commands/plugin/manager.rs"
)

search_rs -w 'interactive_command'
interactive_hits=$found
for allowed in "${interactive_allowed[@]}"; do
    interactive_hits=$(printf '%s\n' "$interactive_hits" | drop_prefix "$allowed")
done

if [ -n "$interactive_hits" ]; then
    echo "ERROR: amplihack_git::interactive_command() used outside its allowlist (issue #1764)." >&2
    echo >&2
    echo "$interactive_hits" >&2
    echo >&2
    echo "interactive_command() does not set GIT_TERMINAL_PROMPT=0, so git can stop" >&2
    echo "on a credential prompt and hang the run. Use amplihack_git::command()." >&2
    echo "It is allowed only when a person started the command at a terminal, the" >&2
    echo "URL comes from that person's own arguments, and the child inherits stdio" >&2
    echo "in the foreground process group. If all three hold, add the file to" >&2
    echo "interactive_allowed in $0 and say why in the review." >&2
    exit 1
fi

echo "OK: no unsanitised git subprocesses outside ${allowed_prefix}"
echo "OK: interactive_command() used only in: ${interactive_allowed[*]}"
