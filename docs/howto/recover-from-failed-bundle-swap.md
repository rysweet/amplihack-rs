---
title: Recover from a failed bundle swap
description: Restore ~/.amplihack/amplifier-bundle when install could not swap the new bundle in and could not put the previous one back.
last_updated: 2026-11-07
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Recover from a failed bundle swap

Use this guide when `amplihack install` (or the install that `amplihack update`
runs afterwards) fails and the error line contains
`failed to swap new amplifier-bundle into`. In the terminal the line starts
with `error: failed to swap new amplifier-bundle into`.

The shell commands below are for Linux and macOS. On Windows, see
[Windows equivalents](#windows-equivalents).

## How install replaces the bundle

Install copies the new bundle into `amplifier-bundle.staging`, checks it, and
then swaps it in with two renames:

1. The current `amplifier-bundle` is renamed to `amplifier-bundle.old`.
2. `amplifier-bundle.staging` is renamed to `amplifier-bundle`.

If the second rename fails, install renames `amplifier-bundle.old` back to
`amplifier-bundle`. The error you see depends on whether that rollback worked.
The full message text is listed in the
[install command reference](../reference/install-command.md#bundle-swap-errors).

On any error, install then tries to remove `amplifier-bundle.staging`. If that
removal fails, install logs a warning and carries on; the next install removes
the leftover staging directory before it starts.

## Check which case you are in

The error names the bundle directory. In the default layout this is
`~/.amplihack/amplifier-bundle`.

**Rollback succeeded.** The error line is:

```text
error: failed to swap new amplifier-bundle into /home/dev/.amplihack/amplifier-bundle: Permission denied (os error 13)
```

The previous bundle is back in place and amplihack keeps working with it.
Fix the cause shown after the last colon, then run `amplihack install` again.
Nothing else is needed.

**Rollback failed.** The error line also says `could not restore the previous
bundle` and names `amplifier-bundle.old`:

```text
error: failed to swap new amplifier-bundle into /home/dev/.amplihack/amplifier-bundle and could not restore the previous bundle (Permission denied (os error 13)); it was left at /home/dev/.amplihack/amplifier-bundle.old and can be moved back to /home/dev/.amplihack/amplifier-bundle by hand: Permission denied (os error 13)
```

The text in brackets is why the rollback failed. The text after the last
colon is why the swap failed. In this case there is no `amplifier-bundle`
directory, and the previous bundle is intact under `amplifier-bundle.old`.
Install does not delete it on this path. Follow the steps below.

## Restore the previous bundle

1. Check the state on disk:

   ```bash
   ls -d ~/.amplihack/amplifier-bundle ~/.amplihack/amplifier-bundle.old
   # Expected:
   # ls: cannot access '/home/dev/.amplihack/amplifier-bundle': No such file or directory
   # /home/dev/.amplihack/amplifier-bundle.old
   ```

   If `amplifier-bundle` already exists, a later install, update or launch has
   probably staged a new bundle since the failure. No restore is needed. Do not
   move `amplifier-bundle.old` over it, as that would mix two bundles. You can
   keep `amplifier-bundle.old` or delete it; the next install that finds both
   directories deletes it anyway. Stop here.

2. Fix the cause given in the error. Typical causes are a parent directory that
   is not writable by your user, a full disk, or (on Windows) another process
   holding a file open inside the bundle.

3. Move the previous bundle back:

   ```bash
   mv ~/.amplihack/amplifier-bundle.old ~/.amplihack/amplifier-bundle
   ```

4. Run the install again to stage the new bundle:

   ```bash
   amplihack install
   # Expected line in the output:
   #   ✅ Staged amplifier-bundle to /home/dev/.amplihack/amplifier-bundle
   ```

   A successful install leaves no `amplifier-bundle.old` behind.

## Reinstall without restoring

If you do not need the previous bundle, skip step 3 and run
`amplihack install` once the cause is fixed. With no `amplifier-bundle`
present, install moves the new bundle straight into place and leaves
`amplifier-bundle.old` alone, so you can inspect it and then delete it:

```bash
rm -rf ~/.amplihack/amplifier-bundle.old
```

Keep a copy elsewhere if you want it. The next install that finds both
`amplifier-bundle` and `amplifier-bundle.old` deletes the `.old` directory
before it takes a new backup.

## Windows equivalents

In PowerShell, the bundle directory is under `$HOME\.amplihack`:

```powershell
# Step 1: check the state on disk
Test-Path $HOME\.amplihack\amplifier-bundle
Test-Path $HOME\.amplihack\amplifier-bundle.old

# Step 3: move the previous bundle back
Move-Item $HOME\.amplihack\amplifier-bundle.old $HOME\.amplihack\amplifier-bundle

# Reinstall without restoring: delete the old bundle
Remove-Item -Recurse -Force $HOME\.amplihack\amplifier-bundle.old
```

## Related

- [install command reference](../reference/install-command.md#bundle-swap-errors) - exact error text and what install leaves on disk
- [Repair a stale framework bundle](./repair-stale-framework-bundle.md) - when the bundle is present but out of date
