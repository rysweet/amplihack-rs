#!/usr/bin/env bash
# Bootstrap a destination host for amplihack. Idempotent: checks for each
# toolchain component and installs only what is missing.
#
# Streamed over ssh via azlin connect; reads no arguments from stdin.
set -euo pipefail

log_info() { printf '\033[1;34m[bootstrap]\033[0m %s\n' "$*"; }
log_warn() { printf '\033[1;33m[bootstrap]\033[0m %s\n' "$*" >&2; }
log_err()  { printf '\033[1;31m[bootstrap]\033[0m %s\n' "$*" >&2; }

# -- Node.js (>= 18) --------------------------------------------------------
if command -v node >/dev/null 2>&1; then
  NODE_MAJOR=$(node -v | sed 's/^v\([0-9]*\).*/\1/')
  if [[ "${NODE_MAJOR:-0}" -ge 18 ]]; then
    log_info "node $(node -v) already installed"
  else
    log_warn "node < 18 detected; upgrading via nodesource"
    curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
    sudo apt-get install -y nodejs
  fi
else
  log_info "installing node.js 20.x"
  curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
  sudo apt-get install -y nodejs
fi

# -- GitHub CLI (gh) --------------------------------------------------------
if command -v gh >/dev/null 2>&1; then
  log_info "gh $(gh --version | head -1) already installed"
else
  log_info "installing gh (GitHub CLI)"
  # Use the official APT repo. Targeted apt flags work around stale repos
  # with bad signatures (documented in the reference procedure).
  type -p curl >/dev/null || sudo apt-get install -y curl
  curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | sudo dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg
  sudo chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | sudo tee /etc/apt/sources.list.d/github-cli.list >/dev/null
  sudo apt-get update -o Dir::Etc::sourcelist="sources.list.d/github-cli.list" \
    -o Dir::Etc::sourceparts="-" -o APT::Get::List-Cleanup="0" -y
  sudo apt-get install -y gh
fi

# -- uv ---------------------------------------------------------------------
if command -v uv >/dev/null 2>&1; then
  log_info "uv $(uv --version) already installed"
else
  log_info "installing uv"
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

# -- GitHub Copilot CLI -----------------------------------------------------
if command -v copilot >/dev/null 2>&1; then
  log_info "copilot $(copilot --version 2>/dev/null | head -1) already installed"
else
  log_info "installing @github/copilot via npm"
  npm install -g @github/copilot
fi

# -- amplihack --------------------------------------------------------------
if command -v amplihack >/dev/null 2>&1; then
  log_info "amplihack $(amplihack --version 2>/dev/null || echo '?') already installed"
else
  log_info "installing amplihack via uv tool (from amplihack-rs git)"
  uv tool install --force git+https://github.com/rysweet/amplihack-rs.git || {
    log_warn "uv tool install failed; falling back to npx bootstrap"
    npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install
  }
fi

log_info "bootstrap complete"
