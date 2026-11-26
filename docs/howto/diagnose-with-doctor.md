# How to Diagnose Problems with amplihack doctor

`amplihack doctor` runs 5 system health checks and reports which prerequisites are satisfied. This guide explains what each failing check means and how to fix it.

## Run doctor first

```sh
amplihack doctor
```

Identify which lines start with `✗` and follow the relevant section below. All 5 checks always run, so you will see all failures at once.

## Fixing Each Failing Check

### ✗ hooks installed

**What it means:** at least one settings file does not hold amplihack's full hooks block. Doctor prints one line per file it checked: `~/.claude/settings.json`, each profile's `settings.json`, and the launch settings file in `~/.amplihack/runtime/`. It does not check `$CLAUDE_CONFIG_DIR/settings.json` or the project's `.claude/settings.json`. For example (the heading and indentation follow doctor's usual layout; the `<path>: <state>` lines are fixed):

```text
✗ hooks installed
    /home/you/.claude/settings.json: drift missing=[PreToolUse, PostToolUse, StopFailure] differing=[] extra=[]
    /home/you/.amplihack/claude-profiles/work/settings.json: clean
```

The check fails when the home file is not `clean`, or when a profile or launch file that exists shows drift, is unparseable or is refused. Absent profile and launch files pass. The full output format is in the [hooks block reference](../reference/hook-block.md#doctor-report).

**Fix:** Run the installer:

```sh
amplihack install
```

`install` merges the full block into `~/.claude/settings.json`. The live profile's file is repaired by the next `amplihack profile rotate` cycle; other profile files are repaired by profile refresh. Re-run `doctor` to confirm every line reads `clean`.

**If the home file keeps losing its hooks,** another process is rewriting it. Sessions started with `amplihack launch` or `amplihack claude` still receive the hooks through the launch settings file; see [One source for the hooks block](../concepts/hook-block-single-source.md).

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

This check is informational and only fails if the binary cannot report its version. If you see it fail on an otherwise working install, the binary may be corrupted or compiled for the wrong architecture.

**Fix:** Reinstall from the appropriate release artifact for your platform. See [Install amplihack for the First Time](./first-install.md).

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
