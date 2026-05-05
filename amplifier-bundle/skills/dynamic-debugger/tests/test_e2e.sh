#!/usr/bin/env bash
# Shell-native smoke checks for the dynamic-debugger skill bundle.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for script in "${ROOT_DIR}/scripts/start_dap_mcp.sh" "${ROOT_DIR}/scripts/cleanup_debug.sh"; do
    if [[ ! -f "$script" ]]; then
        echo "missing script: $script" >&2
        exit 1
    fi
    bash -n "$script"
done

for config in "${ROOT_DIR}/configs/debugpy.json" "${ROOT_DIR}/configs/lldb.json"; do
    if [[ ! -f "$config" ]]; then
        echo "missing config: $config" >&2
        exit 1
    fi
done

echo "PASS: dynamic-debugger shipped scripts/configs are present and shell-valid"
