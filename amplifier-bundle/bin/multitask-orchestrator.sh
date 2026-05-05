#!/usr/bin/env bash
# Native compatibility wrapper for the legacy multitask orchestrator asset.

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: multitask-orchestrator.sh <workstreams.json>" >&2
    exit 2
fi

exec amplihack orch run "$1"
