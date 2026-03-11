#!/usr/bin/env bash
set -euo pipefail

# Bump the workspace version in Cargo.toml and commit.
# Usage: ./scripts/bump-version.sh [patch|minor|major]
#
# The auto-release workflow detects version changes on push to main
# and creates a GitHub release automatically.

BUMP_TYPE="${1:-patch}"
CARGO_TOML="Cargo.toml"

# Get current version
CURRENT=$(grep '^version' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP_TYPE" in
  patch) PATCH=$((PATCH + 1)) ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
  *) echo "Usage: $0 [patch|minor|major]"; exit 1 ;;
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"

echo "Bumping version: ${CURRENT} → ${NEW_VERSION}"

# Update Cargo.toml
sed -i "s/^version = \"${CURRENT}\"/version = \"${NEW_VERSION}\"/" "$CARGO_TOML"

# Update Cargo.lock
cargo generate-lockfile 2>/dev/null || true

echo "Updated ${CARGO_TOML} to ${NEW_VERSION}"
echo ""
echo "Next steps:"
echo "  git add Cargo.toml Cargo.lock"
echo "  git commit -m 'chore: bump version to ${NEW_VERSION}'"
echo "  git push origin main"
echo ""
echo "The auto-release workflow will create v${NEW_VERSION} on push."
