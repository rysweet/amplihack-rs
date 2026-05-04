#!/usr/bin/env bash

set -euo pipefail

state_dir="${AMPLIHACK_CONTEXT_DIR:-${HOME}/.amplihack/context-management}"

usage() {
    cat <<'EOF'
Usage: context-management.sh <status|snapshot|list|show> [args]

Commands:
  status                 Show current repository/session context health.
  snapshot [message]     Write a markdown context snapshot.
  list                   List saved snapshots.
  show [file|latest]     Print a saved snapshot.
EOF
}

ensure_state_dir() {
    mkdir -p "$state_dir"
}

git_root() {
    git rev-parse --show-toplevel 2>/dev/null || pwd
}

git_branch() {
    git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "no-git"
}

git_dirty_count() {
    git status --porcelain 2>/dev/null | wc -l | tr -d ' '
}

latest_snapshot() {
    find "$state_dir" -maxdepth 1 -type f -name '*.md' 2>/dev/null | sort | tail -1
}

status() {
    ensure_state_dir
    local latest
    latest="$(latest_snapshot || true)"

    echo "context status:"
    echo "  cwd: $(pwd)"
    echo "  repo: $(git_root)"
    echo "  branch: $(git_branch)"
    echo "  changed_files: $(git_dirty_count)"
    echo "  snapshot_dir: $state_dir"
    if [[ -n "$latest" ]]; then
        echo "  latest_snapshot: $latest"
    else
        echo "  latest_snapshot: none"
    fi
}

snapshot() {
    ensure_state_dir
    local message="${*:-manual snapshot}"
    local stamp file
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    file="$state_dir/${stamp}-context.md"

    {
        echo "# Context Snapshot"
        echo
        echo "- Created: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo "- Message: $message"
        echo "- CWD: $(pwd)"
        echo "- Repo: $(git_root)"
        echo "- Branch: $(git_branch)"
        echo "- Changed files: $(git_dirty_count)"
        echo
        echo "## Git Status"
        echo
        echo '```'
        git --no-pager status --short 2>/dev/null || echo "not a git repository"
        echo '```'
        echo
        echo "## Handoff"
        echo
        echo "### Goal"
        echo
        echo "### User requirements"
        echo
        echo "### Decisions made"
        echo
        echo "### Files changed or inspected"
        echo
        echo "### Validation"
        echo
        echo "### Remaining work"
        echo
        echo "### Blockers"
    } > "$file"

    echo "$file"
}

list_snapshots() {
    ensure_state_dir
    find "$state_dir" -maxdepth 1 -type f -name '*.md' -print | sort
}

show_snapshot() {
    ensure_state_dir
    local target="${1:-latest}"
    if [[ "$target" == "latest" ]]; then
        target="$(latest_snapshot || true)"
    fi
    if [[ -z "$target" || ! -f "$target" ]]; then
        echo "No snapshot found: ${1:-latest}" >&2
        exit 1
    fi
    sed -n '1,240p' "$target"
}

cmd="${1:-status}"
shift || true

case "$cmd" in
    status) status ;;
    snapshot) snapshot "$@" ;;
    list) list_snapshots ;;
    show) show_snapshot "${1:-latest}" ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
