# Troubleshooting: Node.js Version Too Low for Copilot CLI

Quick solution for when `amplihack copilot` or `amplihack install` reports that your Node.js version is insufficient for GitHub Copilot CLI.

## Quick Diagnosis

```bash
node --version
# If output is v23.x or lower, use managed install or upgrade system Node.js
```

---

## Issue: "Node.js v24 or higher required for Copilot CLI"

### Symptoms

One of two messages appears depending on the command:

**`amplihack copilot`** (auto-remediation on supported interactive hosts):

```
Downloading Node.js v24.1.0 (linux-x64)...
Installing Node.js v24.1.0...
Node.js v24.1.0 installed to ~/.amplihack/runtimes/node-v24.1.0-linux-x64
```

On unsupported platforms or in non-interactive environments, launch fails with
manual installation guidance instead of downloading Node.js.

**`amplihack install`** (warning — install continues):

```
⚠️  Node.js v20.19.4 detected — Copilot CLI requires v24+.
   The Copilot CLI plugin was registered. `amplihack copilot` may install a
   managed Node.js runtime on a supported interactive host.
   Upgrade: nvm install 24 && nvm use 24
```

### Cause

GitHub Copilot CLI v1.x requires Node.js v24 or higher. Systems running
Node.js v20 or v22 (common LTS versions at time of writing) cannot launch
Copilot CLI. Prior to this check, `amplihack install` would succeed silently
and `amplihack copilot` would fail with an opaque Node.js error.

### Solution

Use `amplihack copilot` on a supported interactive Linux or macOS host to let
amplihack install managed Node.js automatically, or upgrade system Node.js to
v24 or higher:

**Using nvm (recommended):**

```bash
nvm install 24
nvm use 24
node --version  # Should show v24.x.x
```

**Using Homebrew (macOS):**

```bash
brew install node@24
# or
brew upgrade node
node --version
```

**Using NodeSource (Ubuntu/Debian):**

```bash
curl -fsSL https://deb.nodesource.com/setup_24.x | sudo -E bash -
sudo apt install -y nodejs
node --version
```

**Using winget (Windows):**

```powershell
winget upgrade OpenJS.NodeJS
node --version
```

After upgrading, re-run the original command:

```bash
amplihack copilot   # Should now launch successfully
```

---

## Issue: Empty or Malformed Copilot config.json

### Symptoms

During `amplihack install` or `amplihack copilot`, you see:

```
Could not update Copilot CLI config.json: Expecting value: line 1 column 1 (char 0)
```

or:

```
Could not validate/repair config.json — nested agents may fail
```

### Cause

The Copilot CLI config file at `~/.copilot/config.json` is empty (0 bytes)
or contains only whitespace. This can happen when:

- A previous install or update was interrupted mid-write
- The file was manually truncated
- A filesystem sync issue left a zero-length placeholder

### Solution

amplihack handles this automatically. When it encounters an empty or
whitespace-only `config.json`, it treats the file as equivalent to `{}`
(an empty JSON object) and proceeds normally. The amplihack plugin entry
is inserted into the recovered object, and the file is rewritten with
valid JSON content.

**No manual intervention is required.** If you still see errors after
upgrading to the version with this fix, verify the file manually:

```bash
cat ~/.copilot/config.json
# If truly malformed (non-empty but invalid JSON), back up and recreate:
cp ~/.copilot/config.json ~/.copilot/config.json.bak
echo '{}' > ~/.copilot/config.json
amplihack install
```

Note: Only *empty* files are auto-recovered. Files containing non-empty
invalid JSON (e.g., truncated content like `{"installed`) still produce
a parse error, because guessing the user's intended content would be unsafe.

---

## Behavior Summary

| Scenario | `amplihack install` | `amplihack copilot` |
|----------|---------------------|---------------------|
| Node.js missing entirely | Warning at Copilot plugin step; install continues | Managed install on supported interactive hosts; otherwise hard error |
| Node.js < v24 | Warning at Copilot plugin step; install continues | Managed install on supported interactive hosts; otherwise hard error |
| Node.js ≥ v24 | No message | Normal launch |
| Node version unparseable | No message (fail-open) | No message (fail-open) |
| config.json empty/whitespace | Auto-recovered to `{}` | Auto-recovered to `{}` |
| config.json malformed (non-empty) | Parse error surfaced | Parse error surfaced |

---

## Related

- [Prerequisites](../PREREQUISITES.md) — required tool versions
- [Install Command Reference](../reference/install-command.md) — install phases and exit codes
- [Prerequisite Checking System](../reference/prerequisite-checking.md) — detection API
- [Copilot CLI Integration](../reference/copilot-cli.md) — full integration guide

---

**Last Updated**: 2026-06-05
**Introduced In**: Issue #679
**Affects**: `amplihack install` (warning), `amplihack copilot` (launch-time remediation or hard error)
