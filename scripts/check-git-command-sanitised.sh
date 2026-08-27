#!/usr/bin/env bash
# Guard: every git subprocess in Rust code goes through amplihack-git (issue #1278).
#
# `Command::new("git")` inherits GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE from
# whatever ran it. Under a git hook those are always set, and they outrank
# `.current_dir()`, so the command silently operates on the hook's repository
# rather than the one the caller pointed at. That made the pre-push `cargo test`
# gate unpassable and it makes production code that runs under a hook wrong.
#
# `amplihack_git::command()` / `command_in()` / `tokio_command()` clear those
# variables first. This guard exists because the fix is one a new call site can
# silently opt out of just by typing the obvious thing.
set -euo pipefail

# Optional argument: the tree to scan. Defaults to the repository root, and
# exists so tests can point the guard at a fixture instead of the real tree.
cd "${1:-$(dirname "$0")/..}"

# The helper crate is the one place allowed to name git directly: it is the
# implementation, and its tests deliberately spawn an unsanitised command as the
# control that proves an ambient GIT_DIR really does hijack one.
allowed_prefix="crates/amplihack-git/"

hits=$(grep -rn --include='*.rs' 'Command::new("git")' . \
    --exclude-dir=target --exclude-dir=.git \
    | sed 's|^\./||' \
    | grep -v "^${allowed_prefix}" \
    || true)

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
    echo "  amplihack_git::command()            // std, cwd-based" >&2
    echo "  amplihack_git::command_in(dir)      // std, explicit directory" >&2
    echo "  amplihack_git::tokio_command()      // async callers" >&2
    echo "  amplihack_git::scrub(&mut cmd)      // non-git child that runs git" >&2
    echo >&2
    echo "Add amplihack-git = { workspace = true } to the crate's [dependencies]." >&2
    exit 1
fi

echo "OK: no unsanitised git subprocesses outside ${allowed_prefix}"
