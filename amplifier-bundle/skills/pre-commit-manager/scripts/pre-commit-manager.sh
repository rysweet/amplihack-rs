#!/usr/bin/env bash

set -euo pipefail

prefs_dir="${AMPLIHACK_STATE_DIR:-${HOME}/.amplihack/state}"
prefs_file="$prefs_dir/precommit_prefs"

usage() {
    cat <<'EOF'
Usage: pre-commit-manager.sh <status|configure|install|enable|disable|baseline> [template]

Templates for configure: generic, rust, go, javascript, typescript, python
EOF
}

ensure_prefs_dir() {
    mkdir -p "$prefs_dir"
}

precommit_available() {
    command -v pre-commit >/dev/null 2>&1
}

status() {
    echo "pre-commit status:"
    echo "  repo: $(git rev-parse --show-toplevel 2>/dev/null || pwd)"
    echo "  config: $(test -f .pre-commit-config.yaml && echo present || echo missing)"
    echo "  binary: $(precommit_available && command -v pre-commit || echo missing)"
    echo "  hook: $(test -f .git/hooks/pre-commit && echo installed || echo missing)"
    echo "  preference: $(cat "$prefs_file" 2>/dev/null || echo ask)"
}

write_config() {
    local template="${1:-generic}"
    if [[ -e .pre-commit-config.yaml ]]; then
        echo ".pre-commit-config.yaml already exists; refusing to overwrite" >&2
        exit 1
    fi

    case "$template" in
        generic)
            cat > .pre-commit-config.yaml <<'EOF'
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v4.6.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
      - id: check-yaml
      - id: check-added-large-files
EOF
            ;;
        rust)
            cat > .pre-commit-config.yaml <<'EOF'
repos:
  - repo: local
    hooks:
      - id: cargo-fmt
        name: cargo fmt
        entry: cargo fmt --check
        language: system
        pass_filenames: false
      - id: cargo-check
        name: cargo check
        entry: cargo check --locked
        language: system
        pass_filenames: false
EOF
            ;;
        go)
            cat > .pre-commit-config.yaml <<'EOF'
repos:
  - repo: local
    hooks:
      - id: gofmt
        name: gofmt
        entry: gofmt -w
        language: system
        files: \.go$
EOF
            ;;
        javascript|typescript)
            cat > .pre-commit-config.yaml <<'EOF'
repos:
  - repo: local
    hooks:
      - id: npm-test
        name: npm test
        entry: npm test
        language: system
        pass_filenames: false
EOF
            ;;
        python)
            cat > .pre-commit-config.yaml <<'EOF'
repos:
  - repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.4.10
    hooks:
      - id: ruff
      - id: ruff-format
EOF
            ;;
        *) echo "Unknown template: $template" >&2; exit 2 ;;
    esac

    echo ".pre-commit-config.yaml"
}

install_hook() {
    if ! precommit_available; then
        echo "pre-commit binary not found; install pre-commit first" >&2
        exit 1
    fi
    pre-commit install
}

set_pref() {
    ensure_prefs_dir
    printf '%s\n' "$1" > "$prefs_file"
    echo "$prefs_file"
}

baseline() {
    if command -v detect-secrets >/dev/null 2>&1; then
        detect-secrets scan > .secrets.baseline
        echo ".secrets.baseline"
    else
        echo "detect-secrets binary not found; cannot create baseline" >&2
        exit 1
    fi
}

cmd="${1:-status}"
shift || true

case "$cmd" in
    status) status ;;
    configure) write_config "${1:-generic}" ;;
    install) install_hook ;;
    enable) set_pref always ;;
    disable) set_pref never ;;
    baseline) baseline ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
