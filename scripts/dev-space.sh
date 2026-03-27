#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKTREES_DIR="${REPO_ROOT}/worktrees"
REPO_TARGET_DIR="${REPO_ROOT}/target"
TARGET_PREFIX="amplihack-rs-target"
DEFAULT_TARGET_DIR="/tmp/${TARGET_PREFIX}-${USER:-$(id -u)}"
CURRENT_TARGET_DIR="${CARGO_TARGET_DIR:-${DEFAULT_TARGET_DIR}}"
STALE_DAYS="${AMPLIHACK_RS_TARGET_TTL_DAYS:-3}"

case "${STALE_DAYS}" in
    ''|*[!0-9]*)
        echo "AMPLIHACK_RS_TARGET_TTL_DAYS must be a non-negative integer" >&2
        exit 1
        ;;
esac

usage() {
    cat <<EOF
Usage:
  scripts/dev-space.sh status
  scripts/dev-space.sh cargo <cargo-args...>
  scripts/dev-space.sh prune-targets [--repo-target] [--current-target]
  scripts/dev-space.sh remove-worktree <path>

Commands:
  status          Show repo target, temp target, and worktree disk usage.
  cargo           Run cargo with CARGO_TARGET_DIR set to ${DEFAULT_TARGET_DIR}
                  (or your existing CARGO_TARGET_DIR) after pruning stale
                  temp targets older than ${STALE_DAYS} day(s).
  prune-targets   Prune stale temp targets; optionally remove the repo target
                  and/or the current temp target explicitly.
  remove-worktree Remove a worktree under ${WORKTREES_DIR} using
                  git-aware removal.
EOF
}

print_size() {
    local path="$1"
    if [[ -e "${path}" ]]; then
        du -sh "${path}"
    else
        printf 'missing\t%s\n' "${path}"
    fi
}

list_temp_targets() {
    find /tmp -maxdepth 1 -mindepth 1 -type d \
        \( -name "${TARGET_PREFIX}" -o -name "${TARGET_PREFIX}-*" \) \
        -print | sort
}

prune_stale_temp_targets() {
    local path
    while IFS= read -r path; do
        [[ -z "${path}" ]] && continue
        [[ "${path}" == "${CURRENT_TARGET_DIR}" ]] && continue
        if find "${path}" -maxdepth 0 -mtime "+${STALE_DAYS}" | grep -q .; then
            echo "Removing stale temp target ${path}"
            rm -rf "${path}"
        fi
    done < <(list_temp_targets)
}

status() {
    echo "Current temp target: ${CURRENT_TARGET_DIR}"
    echo
    echo "Targets:"
    print_size "${REPO_TARGET_DIR}" | sed 's/^/  /'
    while IFS= read -r path; do
        [[ -z "${path}" ]] && continue
        print_size "${path}" | sed 's/^/  /'
    done < <(list_temp_targets)
    echo
    echo "Git worktrees:"
    git -C "${REPO_ROOT}" worktree list --porcelain \
        | awk '/^worktree /{print substr($0,10)}' \
        | while IFS= read -r worktree; do
            print_size "${worktree}" | sed 's/^/  /'
        done
}

run_cargo() {
    if [[ $# -eq 0 ]]; then
        echo "cargo subcommand requires at least one cargo argument" >&2
        exit 1
    fi
    prune_stale_temp_targets
    mkdir -p "${CURRENT_TARGET_DIR}"
    echo "Using CARGO_TARGET_DIR=${CURRENT_TARGET_DIR}"
    CARGO_TARGET_DIR="${CURRENT_TARGET_DIR}" cargo "$@"
}

prune_targets() {
    local remove_repo_target=0
    local remove_current_target=0

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --repo-target)
                remove_repo_target=1
                ;;
            --current-target)
                remove_current_target=1
                ;;
            *)
                echo "Unknown prune-targets option: $1" >&2
                exit 1
                ;;
        esac
        shift
    done

    prune_stale_temp_targets

    if [[ ${remove_repo_target} -eq 1 && -d "${REPO_TARGET_DIR}" ]]; then
        echo "Removing repo target ${REPO_TARGET_DIR}"
        rm -rf "${REPO_TARGET_DIR}"
    fi

    if [[ ${remove_current_target} -eq 1 && -d "${CURRENT_TARGET_DIR}" ]]; then
        echo "Removing current temp target ${CURRENT_TARGET_DIR}"
        rm -rf "${CURRENT_TARGET_DIR}"
    fi
}

remove_worktree() {
    if [[ $# -ne 1 ]]; then
        echo "remove-worktree requires exactly one path" >&2
        exit 1
    fi

    local path="$1"
    case "${path}" in
        "${WORKTREES_DIR}"/*) ;;
        *)
            echo "Refusing to remove non-managed worktree path: ${path}" >&2
            exit 1
            ;;
    esac

    git -C "${REPO_ROOT}" worktree remove --force "${path}"
}

if [[ $# -eq 0 ]]; then
    usage
    exit 1
fi

command_name="$1"
shift

case "${command_name}" in
    status)
        status
        ;;
    cargo)
        run_cargo "$@"
        ;;
    prune-targets)
        prune_targets "$@"
        ;;
    remove-worktree)
        remove_worktree "$@"
        ;;
    *)
        echo "Unknown command: ${command_name}" >&2
        usage
        exit 1
        ;;
esac
