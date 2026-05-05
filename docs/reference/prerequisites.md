# Prerequisites

**Type**: Reference (Information-Oriented)

Detailed installation instructions for all tools required by amplihack-rs
across different platforms.

## Required Tools

| Tool        | Min Version | Purpose                               |
| ----------- | ----------- | ------------------------------------- |
| **Rust**    | 1.70+       | Compiles and runs amplihack-rs        |
| **Node.js** | v18+        | Runs Claude Code CLI                  |
| **npm**     | (with Node) | Installs Claude Code CLI              |
| **git**     | 2.0+        | Version control, worktrees, workflows |
| **claude**  | latest      | Claude Code CLI (AI coding assistant) |

## Quick Check

```bash
rustc --version && cargo --version && node --version && npm --version && git --version && echo "All prerequisites OK"
```

## Platform Installation

### macOS

```bash
# Rust (via rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node.js and npm
brew install node

# git
brew install git

# Claude Code CLI
npm install -g @anthropic-ai/claude-code
```

### Ubuntu / Debian

```bash
# Rust (via rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node.js 18+ (via NodeSource)
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs

# git
sudo apt install -y git

# Claude Code CLI
npm install -g @anthropic-ai/claude-code
```

### Fedora / RHEL

```bash
# Rust (via rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node.js and npm
sudo dnf install nodejs npm

# git
sudo dnf install git

# Claude Code CLI
npm install -g @anthropic-ai/claude-code
```

### Windows (WSL2 Recommended)

```bash
# Install WSL2 first
wsl --install

# Then follow Ubuntu instructions above inside WSL2
```

For native Windows without WSL2, install:

- Rust: [rustup-init.exe](https://rustup.rs/)
- Node.js: [nodejs.org](https://nodejs.org/) (LTS installer)
- git: [git-scm.com](https://git-scm.com/download/win)

## Verification

After installation, verify each tool:

```bash
rustc --version    # Should show 1.70+
cargo --version    # Should show 1.70+
node --version     # Should show v18+
npm --version      # Should show 9+
git --version      # Should show 2.0+
```

## Optional Tools

| Tool           | Purpose                          | Install                    |
| -------------- | -------------------------------- | -------------------------- |
| `docker`       | Build documentation site         | `docker run --rm -v "$PWD:/docs" squidfunk/mkdocs-material build --strict` |
| `gh`           | GitHub CLI for PR management     | `brew install gh` / `apt install gh` |
| `cargo-audit`  | Security vulnerability scanning  | `cargo install cargo-audit` |

## Post-Install: amplihack-rs

```bash
# Clone and build
git clone https://github.com/rysweet/amplihack-rs.git
cd amplihack-rs
cargo build --release

# Install
cargo install --path crates/amplihack-cli

# Verify
amplihack --version
```

## Troubleshooting

### Rust Build Errors

```bash
# Update Rust toolchain
rustup update stable
```

### Node.js Version Too Old

```bash
# Check version
node --version

# If < 18, use nvm to install latest LTS
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.0/install.sh | bash
nvm install --lts
```

### Claude Code CLI Not Found

```bash
# Reinstall globally
npm install -g @anthropic-ai/claude-code

# Verify npm global bin is in PATH
npm bin -g
```

## Related

- [First-Time Install](../howto/first-install.md) — amplihack-rs installation guide
- [Developing amplihack](../howto/develop-amplihack.md) — development environment setup
- [Environment Variables](../reference/environment-variables.md) — configurable env vars
