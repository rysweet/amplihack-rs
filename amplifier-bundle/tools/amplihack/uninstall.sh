#!/bin/bash
set -euo pipefail

# Uninstall the amplihack tools
echo "Backing up ~/.claude/settings.json..."
if [ -f ~/.claude/settings.json ]; then
  cp ~/.claude/settings.json ~/.claude/settings.json.bak.amplihack
  echo "  Backup created at ~/.claude/settings.json.bak.amplihack"
fi

echo "Removing amplihack directories..."
rm -rf ~/.claude/agents/amplihack
rm -rf ~/.claude/commands/amplihack
rm -rf ~/.claude/tools/amplihack

echo "Removing amplihack hook registrations from settings.json..."
if [ -f ~/.claude/settings.json ]; then
  # Remove amplihack-hooks references but preserve the file structure
  sed -i.tmp -e '/amplihack-hooks/d' ~/.claude/settings.json
  rm -f ~/.claude/settings.json.tmp
  echo "  Hook registrations removed (settings.json preserved)"
else
  echo "  No settings.json found — nothing to clean"
fi

echo "✅ Amplihack uninstalled. Backup at ~/.claude/settings.json.bak.amplihack"
