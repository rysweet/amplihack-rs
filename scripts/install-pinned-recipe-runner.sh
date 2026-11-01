#!/usr/bin/env bash
# Install recipe-runner-rs at the revision pinned in claude-plugin/recipe-runner.rev,
# for CI jobs that run recipes. Used by the Test and Install Smoke Test jobs in
# .github/workflows/ci.yml, so both build the runner the same way.
#
# Installs under $RUNNER_TEMP/recipe-runner (or a temporary directory outside
# GitHub Actions), checks that the binary reports its version and
# capabilities, and, under GitHub Actions, puts it on PATH and exports
# RECIPE_RUNNER_RS_PATH for later steps.
#
# A runner already installed there that is newer than the pin, has diverged
# from it, or cannot be ordered against it is kept, not downgraded (issue
# #1593). --force, or AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1, installs the
# pin anyway. See docs/reference/recipe-runner-validation.md.
#
# Usage: install-pinned-recipe-runner.sh [--force]
set -euo pipefail

force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    *)
      echo "usage: $(basename "$0") [--force]" >&2
      exit 2
      ;;
  esac
done
if [[ "${AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE:-}" == "1" ]]; then
  force=1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pin="$repo_root/claude-plugin/recipe-runner.rev"
runner_url=https://github.com/rysweet/amplihack-recipe-runner
# shellcheck source=scripts/lib/runner-ancestry.sh
. "$repo_root/scripts/lib/runner-ancestry.sh"

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
runner="$install_root/bin/recipe-runner-rs"

# The installed revision comes only from Cargo's receipt, and only while the
# binary it describes is executable.
installed=""
if [[ -x "$runner" ]]; then
  installed="$(runner_receipt_revision "$install_root/.crates2.json" "$runner_url")"
fi
class="$(runner_classify "$installed" "$rev" "$runner_url" "${CARGO_HOME:-$HOME/.cargo}")"

install=1
cargo_force=0
case "$class" in
  Equal)
    if [[ "$force" == 1 ]]; then
      cargo_force=1
      echo "reinstall forced: recipe-runner-rs $installed is already the pinned revision" >&2
    else
      install=0
    fi
    ;;
  Newer | Diverged | Unverifiable)
    if [[ "$force" == 1 ]]; then
      cargo_force=1
      echo "downgrade forced: replacing recipe-runner-rs $installed ($class) with pinned $rev" >&2
    else
      install=0
      runner_skip_message "$installed" "$rev" "$class" >&2
    fi
    ;;
esac

# Issue #1808: write the standard cache directory tag so backup tools skip the
# target folder (https://bford.info/cachedir/). An existing tag is kept.
write_cachedir_tag() {
  local dir="$1"
  mkdir -p "$dir" || return 1
  [ -e "$dir/CACHEDIR.TAG" ] && return 0
  printf '%s\n' \
    'Signature: 8a477f597d28d172789f06886806bc55' \
    '# This file is a cache directory tag created by amplihack.' \
    '# For information about cache directory tags see https://bford.info/cachedir/' \
    > "$dir/CACHEDIR.TAG"
}

if [[ "$install" == 1 ]]; then
  cargo_args=(install
    --git "$runner_url"
    --rev "$rev" --locked --root "$install_root")
  if [[ "$cargo_force" == 1 ]]; then
    cargo_args+=(--force)
  fi
  write_cachedir_tag "$base/recipe-runner-target"
  CARGO_TARGET_DIR="$base/recipe-runner-target" cargo "${cargo_args[@]}"
fi
test -x "$runner"
"$runner" --version
"$runner" --capabilities
if [[ -n "${GITHUB_PATH:-}" ]]; then
  printf '%s\n' "$install_root/bin" >> "$GITHUB_PATH"
fi
if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf 'RECIPE_RUNNER_RS_PATH=%s\n' "$runner" >> "$GITHUB_ENV"
fi
if [[ "$install" == 1 ]]; then
  echo "installed $runner at $rev (previously ${installed:-unknown}, class $class)"
else
  echo "kept $runner at $installed (pinned $rev, class $class)"
fi
