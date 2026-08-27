#!/usr/bin/env bash
# Run a cargo command from inside a git hook (issues #1278, #1381).
#
# Two kinds of ambient global state silently override what the caller meant.
# This script removes both, in an order that matters -- see below.
#
# 1. git's repository selection (issue #1278)
#
# git exports the repository it is working on into every hook it runs: GIT_DIR,
# and depending on the hook GIT_INDEX_FILE, GIT_PREFIX, GIT_WORK_TREE and
# friends. cargo inherits them, every test binary inherits them, and every git
# subprocess those binaries spawn inherits them too. Those variables outrank the
# working directory when git picks a repository, so a test that builds a temp
# repo and shells out to git silently operates on amplihack's own repository
# instead -- and the pre-push `cargo test` gate becomes unpassable, which is
# what trains people into `--no-verify`.
#
# Removing them restores the plain, correct meaning of "the repository is the
# one you pointed at". Identity variables (GIT_AUTHOR_*, GIT_COMMITTER_*) are
# deliberately left alone: they select an author, not a repository.
#
# This list is the shell twin of amplihack_git::REPOSITORY_ENV_VARS.
# tests/issue_1278_git_env_isolation.sh reads that constant out of the Rust
# source and checks this script scrubs every entry, so adding a variable to one
# side without the other fails CI.
#
# 2. the build cache (issue #1381)
#
# The hooks used to share one hardcoded CARGO_TARGET_DIR across every worktree,
# clone and parallel agent on the host. cargo then reused another branch's
# compiled metadata against your source, producing failures that belong to
# somebody else's code -- an E0061 for a function signature that does not exist
# on your branch, or E0460 "possibly newer version of crate" on doctests. The
# directory also grew past 100 GB.
#
# The path below is therefore derived from the checkout root and MUST STAY
# checkout-specific. Shortening it back to a constant is the obvious
# "simplification" and it reintroduces the bug; two checkouts sharing a cache is
# the whole defect. A given checkout still reuses its own cache across runs,
# which is the reason to set the variable at all.
set -euo pipefail

unset \
    GIT_ALTERNATE_OBJECT_DIRECTORIES \
    GIT_CEILING_DIRECTORIES \
    GIT_COMMON_DIR \
    GIT_DIR \
    GIT_DISCOVERY_ACROSS_FILESYSTEM \
    GIT_INDEX_FILE \
    GIT_INDEX_VERSION \
    GIT_NAMESPACE \
    GIT_OBJECT_DIRECTORY \
    GIT_PREFIX \
    GIT_QUARANTINE_PATH \
    GIT_WORK_TREE

# Order matters: the checkout root is resolved only AFTER the scrub above.
# Asking git which repository we are in while GIT_DIR is still set would answer
# with the hook's repository rather than this checkout -- the very confusion
# this script exists to remove -- and every worktree driven by the same hook
# would collide on one cache again.
checkout_hash() {
    local root digest
    root="$(git rev-parse --show-toplevel 2>/dev/null)" || root="$PWD"
    # sha256sum is coreutils; shasum ships with macOS.
    if command -v sha256sum >/dev/null 2>&1; then
        digest="$(printf '%s' "$root" | sha256sum)"
    else
        digest="$(printf '%s' "$root" | shasum -a 256)"
    fi
    printf '%s' "${digest:0:12}"
}

# An explicitly provided CARGO_TARGET_DIR still wins: this is a default, not an
# override. Ambient state overriding explicit intent is the bug, not the fix.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/amplihack-precommit-target-$(checkout_hash)}"

exec "$@"
