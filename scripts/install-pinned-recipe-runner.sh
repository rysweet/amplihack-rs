#!/usr/bin/env bash
# Install recipe-runner-rs at the revision pinned in claude-plugin/recipe-runner.rev,
# for CI jobs that run recipes. Used by the Test and Install Smoke Test jobs in
# .github/workflows/ci.yml, so both build the runner the same way.
#
# Installs under $RUNNER_TEMP/recipe-runner (or a temporary directory outside
# GitHub Actions), checks that the binary reports its version and
# capabilities, and, under GitHub Actions, puts it on PATH and exports
# RECIPE_RUNNER_RS_PATH for later steps.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pin="$repo_root/claude-plugin/recipe-runner.rev"

# Require one immutable revision, including exactly one line in the pin.
if [[ "$(wc -l < "$pin")" -ne 1 || "$(wc -c < "$pin")" -ne 41 ]]; then
  echo "ERROR: recipe-runner.rev must contain exactly 40 hex bytes and a newline" >&2
  exit 1
fi
rev="$(cat "$pin")"
if [[ ! "$rev" =~ ^[0-9a-fA-F]{40}$ ]]; then
  echo "ERROR: recipe-runner.rev must contain one immutable 40-hex revision" >&2
  exit 1
fi

base="${RUNNER_TEMP:-$(mktemp -d)}"
install_root="$base/recipe-runner"
CARGO_TARGET_DIR="$base/recipe-runner-target" cargo install \
  --git https://github.com/rysweet/amplihack-recipe-runner \
  --rev "$rev" --locked --root "$install_root"
runner="$install_root/bin/recipe-runner-rs"
test -x "$runner"
"$runner" --version
"$runner" --capabilities
if [[ -n "${GITHUB_PATH:-}" ]]; then
  printf '%s\n' "$install_root/bin" >> "$GITHUB_PATH"
fi
if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf 'RECIPE_RUNNER_RS_PATH=%s\n' "$runner" >> "$GITHUB_ENV"
fi
echo "installed $runner at $rev"
