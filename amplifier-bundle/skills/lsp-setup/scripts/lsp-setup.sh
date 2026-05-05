#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: lsp-setup.sh [status|recommend]

Detects project languages and reports language-server availability.
EOF
}

has_file() {
    local pattern="$1"
    find . \
        -path './.git' -prune -o \
        -path './target' -prune -o \
        -path './node_modules' -prune -o \
        -maxdepth 5 -name "$pattern" -print -quit 2>/dev/null | grep -q .
}

detect_languages() {
    local langs=()
    if [[ -f Cargo.toml ]] || has_file '*.rs'; then langs+=(rust); fi
    if [[ -f go.mod ]] || has_file '*.go'; then langs+=(go); fi
    if [[ -f package.json ]] || has_file '*.ts' || has_file '*.tsx' || has_file '*.js'; then langs+=(typescript); fi
    if [[ -f pyproject.toml ]] || [[ -f requirements.txt ]] || has_file '*.py'; then langs+=(python); fi
    if has_file '*.sln' || has_file '*.csproj' || has_file '*.cs'; then langs+=(dotnet); fi
    if [[ -f pom.xml ]] || [[ -f build.gradle ]] || has_file '*.java'; then langs+=(java); fi

    printf '%s\n' "${langs[@]}" | awk 'NF && !seen[$0]++'
}

server_for() {
    case "$1" in
        rust) echo rust-analyzer ;;
        go) echo gopls ;;
        typescript) echo typescript-language-server ;;
        python) echo pyright ;;
        dotnet) echo roslyn ;;
        java) echo jdtls ;;
        *) echo "" ;;
    esac
}

install_hint() {
    case "$1" in
        rust) echo "rustup component add rust-analyzer" ;;
        go) echo "go install golang.org/x/tools/gopls@latest" ;;
        typescript) echo "npm install -g typescript typescript-language-server" ;;
        python) echo "npm install -g pyright" ;;
        dotnet) echo "install Roslyn/C# Dev Kit language server for your editor/runtime" ;;
        java) echo "install Eclipse JDT LS using your editor/runtime package manager" ;;
        *) echo "no recommendation" ;;
    esac
}

status() {
    local found=0
    echo "lsp status:"
    while IFS= read -r lang; do
        [[ -n "$lang" ]] || continue
        found=1
        server="$(server_for "$lang")"
        if [[ -n "$server" ]] && command -v "$server" >/dev/null 2>&1; then
            echo "  $lang: available ($server)"
        else
            echo "  $lang: missing (${server:-unknown})"
            echo "    install: $(install_hint "$lang")"
        fi
    done < <(detect_languages)

    if [[ "$found" == 0 ]]; then
        echo "  no supported project language detected"
    fi
}

recommend() {
    while IFS= read -r lang; do
        [[ -n "$lang" ]] || continue
        echo "$lang: $(install_hint "$lang")"
    done < <(detect_languages)
}

cmd="${1:-status}"
case "$cmd" in
    status) status ;;
    recommend|recommendations) recommend ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
