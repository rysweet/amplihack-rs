---
title: Repair install/update PATH conflicts
description: Diagnose and repair stale system-wide amplihack binaries that shadow current user-local binaries.
last_updated: 2026-06-05
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Repair install/update PATH conflicts

Use this guide when `amplihack update` or `amplihack install` reports that a
stale system-wide binary, usually `/usr/local/bin/amplihack` or
`/usr/local/bin/amplihack-hooks`, is shadowing the current user-local binary in
`~/.local/bin`.

`amplihack` never runs `sudo`, deletes system files, or writes to
system-managed locations automatically. It repairs only user-level writable
install targets and gives explicit commands when system files need
administrator action.

## Check command resolution order

Show every `amplihack` and `amplihack-hooks` candidate on `PATH`:

```bash
which -a amplihack
which -a amplihack-hooks
```

A healthy user-local install resolves `~/.local/bin` first:

```text
/home/alice/.local/bin/amplihack
/home/alice/.local/bin/amplihack-hooks
```

A conflicting install has an earlier system candidate:

```text
/usr/local/bin/amplihack
/home/alice/.local/bin/amplihack
/usr/local/bin/amplihack-hooks
/home/alice/.local/bin/amplihack-hooks
```

In that case the shell runs `/usr/local/bin/amplihack` even though the current
user-level binaries are present.

## Prefer `~/.local/bin`

Put `~/.local/bin` before `/usr/local/bin` in your shell profile:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
hash -r
```

For zsh, use `~/.zshrc`:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
hash -r
```

Verify the result:

```bash
which -a amplihack
which -a amplihack-hooks
amplihack --version
amplihack-hooks --version
```

The first result for both binaries should be under `~/.local/bin`.

## Remove stale system binaries

If `/usr/local/bin` still appears first, remove the stale system copies with
administrator privileges:

```bash
sudo rm /usr/local/bin/amplihack /usr/local/bin/amplihack-hooks
hash -r
```

Then reinstall or update using the user-local binaries:

```bash
amplihack update
amplihack install
```

Only remove the files when they are stale amplihack binaries. If your team
intentionally manages `/usr/local/bin/amplihack` through a package manager,
update that package or change `PATH` order instead.

## Run a safe update

When `~/.local/bin/amplihack` and `~/.local/bin/amplihack-hooks` already exist
and are writable, `amplihack update` uses them as the preferred replacement
target even if stale copies also exist in system-managed prefixes such as
`/usr/local/bin`, `/usr/bin`, `/bin`, or `/opt`.

```bash
amplihack update
```

If a system-managed binary blocks automatic repair, the command fails before
attempting a temporary copy into that location and prints manual guidance:

```text
Cannot update /usr/local/bin/amplihack automatically.

/usr/local/bin/amplihack appears before /home/alice/.local/bin/amplihack on PATH
and is not writable by the current user.

Run one of:
  sudo rm /usr/local/bin/amplihack /usr/local/bin/amplihack-hooks
  export PATH="$HOME/.local/bin:$PATH"

Then run:
  hash -r
  amplihack update
```

This is intentional. The updater avoids misleading errors such as:

```text
Permission denied (os error 13)
```

from trying to copy temporary replacement files into system-managed prefixes.

## Expected clean install/update output

Successful install and update output should not contain stale hook-file or
profile warnings. Treat these strings as regressions:

```text
session_start.sh ❌
post_tool_use.sh ❌
pre_tool_use.sh ❌
profile_management
Skipping symlink
```

Known-safe bundled symlinks are skipped silently or reported only when
diagnostic verbosity explicitly asks for file-copy details. Normal user-facing
install/update output remains focused on actionable results.

## Troubleshooting

### `amplihack update` still runs the old binary

Clear your shell's command lookup cache:

```bash
hash -r
```

Open a new terminal and check again:

```bash
which -a amplihack
amplihack --version
```

### Hooks still point at an old binary

Re-run install after fixing `PATH`:

```bash
amplihack install
```

Hook registrations use the compiled `amplihack-hooks` binary. They do not call
Python hook scripts and do not require `session_start.sh`,
`post_tool_use.sh`, or `pre_tool_use.sh` files.

### You need a system-wide install

Install both binaries consistently in the same system-managed location and keep
them writable only by the administrator:

```bash
sudo install -m 0755 amplihack /usr/local/bin/amplihack
sudo install -m 0755 amplihack-hooks /usr/local/bin/amplihack-hooks
```

After that, keep `/usr/local/bin` first on `PATH` and update the system copy
through the same administrative process. Do not mix a system-wide `amplihack`
with a user-local `amplihack-hooks`.

## See also

- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
- [amplihack install reference](../reference/install-command.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
- [First-time install](first-install.md)
