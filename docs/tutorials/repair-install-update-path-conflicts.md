---
title: Tutorial: repair stale amplihack wrappers and assets
description: Learn how install/update keeps the Rust amplihack binary first and refreshes stale framework assets.
last_updated: 2026-07-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: tutorial
---

# Tutorial: repair stale amplihack wrappers and assets

This tutorial shows the finished install/update behavior using temporary files.
It does not modify your real shell profile, `~/.local/bin`, or
`~/.amplihack`.

You will create:

1. a stale Python-style `amplihack` wrapper
2. a user-local Rust-style `amplihack` binary
3. a stale installed `amplifier-bundle`
4. the shell profile block that makes future shells resolve the Rust binary first

## Prerequisites

- A POSIX shell
- `mktemp`, `mkdir`, `chmod`, `grep`, and `find`

## 1. Create a fake stale wrapper and Rust binary

```bash
tmpdir="$(mktemp -d)"
mkdir -p "$tmpdir/stale-bin" "$tmpdir/home/.local/bin" "$tmpdir/home/.amplihack"

cat > "$tmpdir/stale-bin/amplihack" <<'SH'
#!/usr/bin/env python3
# stale amplihack uvx wrapper
import sys
print("stale-python-wrapper")
SH

cat > "$tmpdir/home/.local/bin/amplihack" <<'SH'
#!/bin/sh
echo "rust-amplihack"
SH

chmod +x "$tmpdir/stale-bin/amplihack" "$tmpdir/home/.local/bin/amplihack"
```

Put the stale wrapper first:

```bash
PATH="$tmpdir/stale-bin:$tmpdir/home/.local/bin:$PATH" amplihack
```

Expected output:

```text
stale-python-wrapper
```

That is the failure shape the installer repairs: a Python or uvx wrapper named
exactly `amplihack` appears earlier on `PATH` than the Rust binary in
`~/.local/bin`.

## 2. See the persistent PATH repair

`amplihack install` writes an idempotent managed block to the user's shell
profile so future shells put `$HOME/.local/bin` first:

```bash
cat > "$tmpdir/home/.bashrc" <<'SH'
# >>> amplihack managed PATH >>>
# Added by amplihack install
export PATH="$HOME/.local/bin:$PATH"
# <<< amplihack managed PATH <<<
SH
```

Source the profile with the temporary home:

```bash
HOME="$tmpdir/home" PATH="$tmpdir/stale-bin:$tmpdir/home/.local/bin:$PATH" \
  sh -c '. "$HOME/.bashrc"; command -v amplihack; amplihack'
```

Expected output:

```text
/tmp/.../home/.local/bin/amplihack
rust-amplihack
```

The block prepends `$HOME/.local/bin` so the Rust binary wins in new shells. A
later duplicate is harmless because shells resolve the first matching
executable. Fish profiles use `fish_add_path --prepend $HOME/.local/bin`
instead of POSIX `export PATH=...`.

## 3. See how stale wrappers are quarantined

When install/update positively identifies a stale Python or uvx wrapper that
shadows the Rust binary and is in a user-controlled or amplihack-managed
location, it moves the wrapper under:

```text
~/.amplihack/quarantine/stale-wrappers/<timestamp>/
```

The quarantine uses sanitized path names and a manifest instead of deleting the
file. Simulate the result:

```bash
stamp="20260710T183440Z"
quarantine="$tmpdir/home/.amplihack/quarantine/stale-wrappers/$stamp"
mkdir -p "$quarantine"
quarantined="$quarantine/stale-bin__amplihack"
mv "$tmpdir/stale-bin/amplihack" "$quarantined"

cat > "$quarantine/manifest.json" <<EOF
{
  "generated_at_unix_secs": 1783737280,
  "entries": [
    {
      "original_path": "$tmpdir/stale-bin/amplihack",
      "quarantine_path": "$quarantined",
      "kind": "stale-python-wrapper",
      "size": 91,
      "modified_unix_secs": 1783737280,
      "action": "quarantined"
    }
  ]
}
EOF
```

Check that only the Rust binary remains on the fake `PATH`:

```bash
PATH="$tmpdir/stale-bin:$tmpdir/home/.local/bin:$PATH" command -v amplihack
PATH="$tmpdir/stale-bin:$tmpdir/home/.local/bin:$PATH" amplihack
```

Expected output:

```text
/tmp/.../home/.local/bin/amplihack
rust-amplihack
```

Unknown executables are not quarantined. If a file named `amplihack` cannot be
classified as the current Rust binary, the preferred Rust binary, or a stale
Python/uvx wrapper, install/update reports it as an unknown conflict and fails
if it would still shadow the Rust binary.

## 4. See whole-bundle replacement

Create a stale installed bundle:

```bash
mkdir -p "$tmpdir/home/.amplihack/amplifier-bundle/recipes"
cat > "$tmpdir/home/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml" <<'YAML'
name: smart-orchestrator
steps:
  - run: python orch_helper.py
YAML
```

Create a current distribution bundle:

```bash
mkdir -p "$tmpdir/current/amplifier-bundle/recipes"
cat > "$tmpdir/current/amplifier-bundle/recipes/smart-orchestrator.yaml" <<'YAML'
name: "smart-orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
YAML
```

Install/update validates a staged copy from the current Rust distribution and
then activates the staged bundle as the installed bundle. The real installer
uses a same-install-root activation step; the temporary commands below only
build the expected post-repair tree so you can inspect the result. They are not
the installer algorithm.

```bash
cp -R "$tmpdir/current/amplifier-bundle" \
  "$tmpdir/home/.amplihack/amplifier-bundle.after"
```

Verify the stale active dependency is gone:

```bash
grep -R "orch_helper.py" "$tmpdir/home/.amplihack/amplifier-bundle.after" || true
```

Expected output: no matches.

## 5. Clean up

```bash
rm -rf "$tmpdir"
```

## See also

- [Repair install/update PATH conflicts](../howto/repair-install-update-path-conflicts.md)
- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
