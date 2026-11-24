#!/usr/bin/env bash
# Controlled Rust fixtures; no hosting writes and no agent runtime are executed.
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p amplihack-cli --lib autodrive_contract_tests
cargo test --locked -p amplihack-cli --test cli_integration issue_2329_autodrive_outcomes
