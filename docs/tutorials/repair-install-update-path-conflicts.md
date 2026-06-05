---
title: Tutorial: understand install/update PATH conflicts
description: Learn how stale system-wide amplihack binaries shadow current user-local binaries and how the repair guidance works.
last_updated: 2026-06-05
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: tutorial
---

# Tutorial: understand install/update PATH conflicts

This tutorial uses temporary fixture binaries to show how `PATH` order affects
`amplihack install` and `amplihack update`. It does not modify your real
`/usr/local/bin` or `~/.local/bin`.

## What you will learn

You will create two fake install roots:

- a stale system-style root that appears first on `PATH`
- a current user-style root that appears later on `PATH`

Then you will change `PATH` order and see why `amplihack` prefers safe
user-local repair instead of writing to system-managed directories.

## 1. Create temporary fake binaries

```bash
tmpdir="$(mktemp -d)"
mkdir -p "$tmpdir/system-bin" "$tmpdir/user-bin"

printf '#!/bin/sh\necho stale-system-amplihack\n' > "$tmpdir/system-bin/amplihack"
printf '#!/bin/sh\necho stale-system-hooks\n' > "$tmpdir/system-bin/amplihack-hooks"

printf '#!/bin/sh\necho current-user-amplihack\n' > "$tmpdir/user-bin/amplihack"
printf '#!/bin/sh\necho current-user-hooks\n' > "$tmpdir/user-bin/amplihack-hooks"

chmod +x "$tmpdir/system-bin/amplihack" \
  "$tmpdir/system-bin/amplihack-hooks" \
  "$tmpdir/user-bin/amplihack" \
  "$tmpdir/user-bin/amplihack-hooks"
```

## 2. Put the stale system root first

```bash
PATH="$tmpdir/system-bin:$tmpdir/user-bin:$PATH" which -a amplihack
PATH="$tmpdir/system-bin:$tmpdir/user-bin:$PATH" which -a amplihack-hooks
```

You should see the system-style root before the user-style root:

```text
/tmp/.../system-bin/amplihack
/tmp/.../user-bin/amplihack
/tmp/.../system-bin/amplihack-hooks
/tmp/.../user-bin/amplihack-hooks
```

That is the same shape as a real machine where `/usr/local/bin/amplihack`
appears before `/home/alice/.local/bin/amplihack`.

Run the fake command:

```bash
PATH="$tmpdir/system-bin:$tmpdir/user-bin:$PATH" amplihack
```

Expected output:

```text
stale-system-amplihack
```

The shell ran the first candidate, even though a newer user-local candidate
also exists.

## 3. Put the user root first

```bash
PATH="$tmpdir/user-bin:$tmpdir/system-bin:$PATH" which -a amplihack
PATH="$tmpdir/user-bin:$tmpdir/system-bin:$PATH" amplihack
```

Expected output:

```text
current-user-amplihack
```

This is the preferred real configuration:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

With that order, `amplihack update` can update the user-local binary and your
shell will run the updated binary first.

## 4. Map the tutorial to the real repair

In a real conflict, inspect your actual command resolution:

```bash
which -a amplihack
which -a amplihack-hooks
```

If `/usr/local/bin` appears before `~/.local/bin`, repair it by reordering
`PATH`:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
hash -r
```

Or remove stale system copies when they are not managed by a package manager:

```bash
sudo rm /usr/local/bin/amplihack /usr/local/bin/amplihack-hooks
hash -r
```

After repair:

```bash
amplihack update
amplihack install
```

## 5. Clean up the fixture

```bash
rm -rf "$tmpdir"
```

## What clean output means

Clean install/update output contains phase summaries and actionable warnings. It
does not contain stale hook-file checks such as:

```text
session_start.sh ❌
post_tool_use.sh ❌
pre_tool_use.sh ❌
```

Those files are not part of the Rust-native hook path. Hook registrations call
`amplihack-hooks` binary subcommands.

## See also

- [Repair install/update PATH conflicts](../howto/repair-install-update-path-conflicts.md)
- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
- [amplihack install reference](../reference/install-command.md)
