---
title: Repair stale amplihack wrappers and PATH conflicts
description: Use install/update to quarantine stale Python or uvx wrappers that shadow Rust, put the Rust binary first, and report unknown command conflicts.
last_updated: 2026-07-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Repair stale amplihack wrappers and PATH conflicts

Use this guide when `amplihack` resolves to an old Python or uvx wrapper, or
when `amplihack install` or `amplihack update` reports that an earlier `PATH`
entry shadows the Rust binary in `~/.local/bin`.

Current install/update is the repair path. It deploys the Rust binaries to
`~/.local/bin`, writes a managed profile block that makes future shells resolve
that directory first, quarantines only positively identified stale wrappers
that shadow Rust, and
refreshes framework assets before final verification that the selected
`amplihack` is the Rust binary.

## Repair with install

Run:

```bash
amplihack install
```

Expected successful behavior:

```text
Deployed amplihack -> /home/alice/.local/bin/amplihack
Deployed amplihack-hooks -> /home/alice/.local/bin/amplihack-hooks
Updated shell PATH profile block
Quarantined stale amplihack wrapper -> /home/alice/.amplihack/quarantine/stale-wrappers/...
Refreshed amplifier-bundle from current distribution
Verified Rust amplihack resolves first
```

Open a new shell and verify:

```bash
command -v amplihack
amplihack --version
```

The first command should print:

```text
/home/alice/.local/bin/amplihack
```

## Repair with update

Run:

```bash
amplihack update
```

After replacing the binary, update runs the new binary's install repair path:

```bash
amplihack install --force-refresh
```

`--force-refresh` is an internal flag. It bypasses stale installed bundle
contents and refreshes `~/.amplihack/amplifier-bundle` from the current Rust
distribution.

## Inspect command resolution

Show every candidate on `PATH`:

```bash
which -a amplihack
which -a amplihack-hooks
```

Healthy output starts with user-local Rust binaries:

```text
/home/alice/.local/bin/amplihack
/home/alice/.local/bin/amplihack-hooks
```

If an earlier candidate exists, install/update classifies it before taking any
action.

| Candidate | Behavior |
| --- | --- |
| Current Rust binary | Accepted. |
| Preferred Rust binary in `~/.local/bin` | Accepted and made first for future shells. |
| Stale Python wrapper | Quarantined only when it shadows Rust, is clearly identified, and is in a safe location. |
| Stale uvx wrapper | Quarantined only when it shadows Rust, is clearly identified, and is in a safe location. |
| Unknown executable | Not modified; reported as a conflict. |
| Inaccessible path | Not modified; reported with the filesystem error. |

## Review quarantined wrappers

Quarantined wrappers are stored under:

```text
~/.amplihack/quarantine/stale-wrappers/<timestamp>/
```

Each quarantine directory includes a manifest:

```bash
find ~/.amplihack/quarantine/stale-wrappers -name manifest.tsv -print
```

The manifest records the original path, quarantined path, wrapper kind, file
size, modification time, action, and reason. It does not copy full file
contents into logs.

To restore a quarantined file, move it out manually after confirming that doing
so will not shadow the Rust binary. Do not restore it to an earlier `PATH`
directory named `amplihack`.

## Handle unknown conflicts

Install/update does not delete or quarantine unknown executables named
`amplihack`.

If the unknown executable is yours and obsolete, remove or rename it manually:

```bash
mv /path/to/old/amplihack /path/to/old/amplihack.disabled
hash -r
amplihack install
```

If the unknown executable is intentionally managed by a package manager, either
update that package or put `~/.local/bin` before the package-manager directory
for shells that should use the Rust user-level install.

## Verify future shells

The managed profile block is idempotent and bounded by markers:

```bash
grep -n "amplihack path" ~/.bashrc ~/.zshrc 2>/dev/null || true
```

The block prepends `$HOME/.local/bin` only when it is not already first. It
does not remove unrelated `PATH` entries.

Open a new terminal and check:

```bash
command -v amplihack
amplihack --version
```

## Troubleshooting

### Install reports an unknown executable

**Symptom**:

```text
Unknown executable shadows Rust amplihack:
  /home/alice/bin/amplihack
```

**Fix**: Inspect that file yourself. If it is not the current Rust binary and
is not needed, rename or remove it, then run `amplihack install` again.

### A system path shadows the Rust binary

Install/update never mutates system-managed locations such as `/usr/bin`,
`/usr/local/bin`, `/opt`, or package-manager directories outside `$HOME`.

**No sudo repair:** install/update will not request elevated privileges and
will not delete system files. Do not fix this with a privileged delete; use your
normal package-manager or administrative process.

Use your normal administrative process to update or remove the system copy, or
keep the user-level install first through the managed profile block.

### The shell still runs the old command

Clear the shell command cache:

```bash
hash -r
```

Then open a new shell. The persistent repair applies to future shells, not only
the process that ran install.

### smart-orchestrator still mentions `orch_helper.py`

Run:

```bash
amplihack install --force-refresh
```

Then verify:

```bash
grep -R "orch_helper.py" ~/.amplihack/amplifier-bundle/recipes || true
```

Active recipes should not contain an executable `orch_helper.py` dependency.
Mentions in docs, tests, or compatibility rejection logic are allowed.

## See also

- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
- [Repair a stale framework bundle](repair-stale-framework-bundle.md)
- [amplihack install reference](../reference/install-command.md)
