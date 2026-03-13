# How to Diagnose Problems with amplihack doctor

`amplihack doctor` runs 7 system health checks and reports which prerequisites are satisfied. This guide explains what each failing check means and how to fix it.

## Run doctor first

```sh
amplihack doctor
```

Identify which lines start with `✗` and follow the relevant section below. All 7 checks always run, so you will see all failures at once.

## Fixing Each Failing Check

### ✗ hooks installed

**What it means:** `~/.claude/settings.json` either does not exist, contains no `hooks` section, or none of the hook command strings contain the word `"amplihack"`.

**Fix:** Run the installer:

```sh
amplihack install
```

`install` registers all 7 amplihack hooks in `settings.json`. Re-run `doctor` to confirm:

```
✓ hooks installed
```

**If the file exists but hooks are missing,** the file may have been manually edited or overwritten. `amplihack install` is idempotent — it is safe to re-run and will add the missing hooks without duplicating existing ones.

---

### ✗ settings.json valid JSON

**What it means:** `~/.claude/settings.json` is not valid JSON. This typically happens after a manual edit introduced a syntax error (trailing comma, missing quote, etc.).

**Fix — option 1: restore from backup**

`amplihack install` writes a timestamped backup before modifying `settings.json`. Look for files matching:

```sh
ls ~/.claude/settings.json.bak.*
```

Restore the most recent valid backup:

```sh
cp ~/.claude/settings.json.bak.1718000000 ~/.claude/settings.json
```

**Fix — option 2: validate and repair manually**

```sh
python3 -m json.tool ~/.claude/settings.json
```

`json.tool` prints the first syntax error with a line number. Fix the error, then re-run `doctor`.

**Fix — option 3: reinstall**

If you have no backup and cannot repair the file, run:

```sh
amplihack install
```

`install` will detect the invalid JSON, back up the broken file as `settings.json.bak.<timestamp>`, and write a valid replacement.

---

### ✗ recipe-runner available

**What it means:** `recipe-runner-rs` is not on `PATH` or is not executable.

**Fix — check PATH:**

```sh
which recipe-runner-rs
```

If the command is not found, install `recipe-runner-rs`:

```sh
# If distributed with amplihack-rs releases, copy from the release archive
cp path/to/release/recipe-runner-rs ~/.local/bin/
chmod +x ~/.local/bin/recipe-runner-rs
```

Confirm `~/.local/bin` is on your `PATH`:

```sh
echo $PATH | tr ':' '\n' | grep local
```

If it is not, add it to your shell init file (`~/.bashrc`, `~/.zshrc`, etc.):

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Reload your shell and re-run `doctor`.

---

### ✗ Python bridge working

**What it means:** Either `python3` is not on `PATH`, or the `amplihack` Python package is not importable in the active Python environment.

**Check which is failing:**

```sh
python3 --version       # confirm python3 exists
python3 -c "import amplihack; print(amplihack.__version__)"
```

**Fix — python3 not found:**

Install Python 3.11 or later via your system package manager or from [python.org](https://www.python.org/downloads/).

**Fix — amplihack package not found:**

```sh
pip3 install amplihack
```

If you are using a virtual environment, activate it first. If `pip3 install amplihack` fails because the package is not on PyPI (for local development), install from source:

```sh
pip3 install -e /path/to/amplihack-python-package
```

Confirm the fix:

```sh
python3 -c "import amplihack; print('OK')"
# OK
```

---

### ✗ tmux installed

**What it means:** `tmux` is not on `PATH`. tmux is required for amplihack's session management features.

**Fix:**

```sh
# Debian / Ubuntu
sudo apt install tmux

# macOS (Homebrew)
brew install tmux

# Arch Linux
sudo pacman -S tmux

# Fedora / RHEL
sudo dnf install tmux
```

**Windows note:** tmux does not have a native Windows port. On Windows, this check is expected to fail unless you are running in WSL2 or another Unix compatibility layer that provides tmux. Features that do not require tmux continue to work.

Confirm the fix:

```sh
tmux -V
# tmux 3.4
```

---

### ✗ amplihack version

This check is informational and only fails if the binary cannot report its own version (which is a compile-time constant). If you see this fail, the binary may be corrupted or compiled for the wrong architecture.

**Fix:** Reinstall from the appropriate release artifact for your platform. See [Install amplihack for the First Time](./first-install.md).

---

### ✗ settings.json path resolution

**What it means:** The `HOME` environment variable is not set, so the path `$HOME/.claude/settings.json` cannot be constructed. This is rare on Unix but can occur in minimal CI containers or when the environment has been stripped.

**Fix:**

```sh
export HOME="$( cd ~ && pwd )"
```

Or set `HOME` explicitly in the failing environment's configuration. Re-run `doctor` to confirm the fix.

---

## All Checks Pass — But Something Still Doesn't Work

`doctor` verifies prerequisites, not runtime behaviour. If all checks pass but Claude Code hooks are not firing:

1. Check that Claude Code is version 1.x or later: `claude --version`
2. Confirm hooks are enabled in Claude Code settings (not just registered): open Claude Code and check the hooks settings panel.
3. Check runtime logs: `ls ~/.amplihack/.claude/runtime/logs/`

For persistent issues, open a GitHub Issue with the output of `amplihack doctor` attached.

## Related

- [amplihack doctor — Command Reference](../reference/doctor-command.md) — Full reference for all checks, exit codes, and output format
- [amplihack install — Command Reference](../reference/install-command.md) — Fixes most hook and settings.json failures
- [Hook Specifications](../reference/hook-specifications.md) — What hooks amplihack registers
