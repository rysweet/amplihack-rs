---
title: Install/update PATH conflict reference
description: Reference for amplihack install/update target selection, PATH conflict detection, safe repair behavior, and output regression guarantees.
last_updated: 2026-06-05
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# Install/update PATH conflict reference

`amplihack install` and `amplihack update` analyze command resolution order
before reporting binary deployment status or replacing an existing binary. The
analysis is deterministic, side-effect free, and based on `PATH` order rather
than only `std::env::current_exe()`.

## Scope

The conflict detector covers these binary names:

| Binary | Purpose |
| --- | --- |
| `amplihack` | Main CLI and update target |
| `amplihack-hooks` | Rust-native hook dispatcher used by registered hooks |

It does not execute discovered binaries. It inspects candidate paths, metadata,
canonical forms when available, and user-writable target locations.

## Preferred install target

`~/.local/bin` is the preferred user-local target when it already contains
current or writable `amplihack` and `amplihack-hooks` binaries.

Target selection uses this priority:

| Priority | Target | Used when |
| --- | --- | --- |
| 1 | Existing current executable directory | The running binary directory is user-writable and not a known unsafe system target. |
| 2 | `~/.local/bin` | User-local binaries are present or the directory is creatable/writable. |
| 3 | Manual repair required | The only viable target is system-owned, root-owned, or otherwise unwritable. |

`/usr/local/bin` is never written automatically unless it is already writable by
the current user. The updater does not invoke `sudo`, change ownership, delete
files, or attempt privileged temporary-file copies.

## PATH conflict categories

| Category | Detection | User-facing behavior |
| --- | --- | --- |
| User-local first | `~/.local/bin/<binary>` is the first candidate for both binaries. | No warning. Install/update proceeds normally. |
| System shadowing user-local | A candidate such as `/usr/local/bin/<binary>` appears before `~/.local/bin/<binary>`. | Warn with the shadowing path and the preferred user-local path. |
| Duplicate candidates | More than one distinct candidate exists for a binary. | Warn when the order is ambiguous or could run a stale binary. |
| Root-owned stale candidate | Earlier candidate is not writable by the current user and user-local candidate exists. | Prefer safe user-local update when possible; otherwise fail with manual repair guidance. |
| Mixed binary locations | `amplihack` and `amplihack-hooks` resolve from different install roots. | Warn because hooks may be updated independently from the CLI. |

Canonicalization is best effort. If a path cannot be canonicalized because it
does not exist or metadata cannot be read, the raw path is still included in the
report and filesystem errors from later install operations are preserved.

## Update behavior

`amplihack update` downloads the new release, verifies it, and replaces only the
selected safe target.

If the current executable is `/usr/local/bin/amplihack` but
`~/.local/bin/amplihack` is present and writable, the updater selects the
user-local target and reports the system conflict instead of trying to write a
temporary file into `/usr/local/bin`.

Example:

```text
⚠️  PATH conflict: /usr/local/bin/amplihack appears before /home/alice/.local/bin/amplihack
    Updating user-local target: /home/alice/.local/bin/amplihack
    To run the updated binary first, move ~/.local/bin earlier in PATH or remove the stale system copy.
```

When no safe user-writable target exists, update exits non-zero before
replacement:

```text
Cannot update amplihack automatically because the resolved target is not writable:
  /usr/local/bin/amplihack

amplihack does not perform privileged writes.

Repair options:
  1. Move ~/.local/bin before /usr/local/bin in PATH.
  2. Remove stale system binaries with sudo:
     sudo rm /usr/local/bin/amplihack /usr/local/bin/amplihack-hooks
  3. Re-run:
     hash -r
     amplihack update
```

The error is explicit and actionable. It replaces generic permission-denied
failures caused by blind temporary-file copies into privileged directories.

## Install behavior

`amplihack install` deploys native binaries to `~/.local/bin` and then analyzes
`PATH` resolution.

When a system binary shadows the deployed user-local binary, install succeeds
but prints a warning:

```text
✓ Deployed amplihack → /home/alice/.local/bin/amplihack
✓ Deployed amplihack-hooks → /home/alice/.local/bin/amplihack-hooks
⚠️  PATH conflict: /usr/local/bin/amplihack appears before /home/alice/.local/bin/amplihack
    Your shell may continue to run the stale system binary.
    Fix: export PATH="$HOME/.local/bin:$PATH" or remove the stale system copy with sudo.
```

This is an advisory because the install completed successfully and the repair
requires either a shell profile change or administrator action.

## Hook behavior

Hook registrations use Rust-native binary subcommands:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks post-tool-use"
          }
        ]
      }
    ]
  }
}
```

The installer does not deploy or verify Python hook files. The following missing
file lines are not valid install/update diagnostics:

```text
session_start.sh ❌
post_tool_use.sh ❌
pre_tool_use.sh ❌
```

Their presence in install/update output is a regression.

## Configuration

No new configuration file is required.

| Input | Role |
| --- | --- |
| `PATH` | Source of ordered command candidates. |
| `HOME` | Used to resolve `~/.local/bin`. |
| Current executable path | Used as a candidate replacement target, not as the sole source of truth. |
| Filesystem metadata | Used for existence, file type, ownership, and writability checks. |
| `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | Still applies to `amplihack-hooks` lookup during install. It does not override PATH conflict reporting. |

Recommended shell configuration:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Contributor API

The shared resolver lives in `crates/amplihack-cli/src/path_conflicts.rs`.

### `PathAnalysisInput`

Inputs supplied by install/update callers and tests.

| Field | Type | Description |
| --- | --- | --- |
| `path_env` | string | Raw `PATH` value to scan in order. |
| `home_dir` | `PathBuf` | Home directory used to derive preferred user bin. |
| `current_exe` | `PathBuf` | Running executable path. |
| `binary_names` | list | Fixed set: `amplihack`, `amplihack-hooks`. |
| `filesystem` | trait-backed adapter | Metadata, canonicalization, and writability checks. |

Tests inject temporary directories and filesystem adapters so transition
coverage is deterministic and does not depend on host `/usr/local/bin`
permissions.

### `BinaryResolution`

Ordered candidate list for one binary.

| Field | Description |
| --- | --- |
| `name` | Binary name. |
| `candidates` | Existing executable candidates in PATH order. |
| `first` | First candidate that the shell resolves. |
| `preferred_user_bin` | Expected `~/.local/bin/<name>` path. |
| `shadowed_user_bin` | User-local candidate when another path appears earlier. |

### `PathConflictReport`

Side-effect-free analysis result consumed by install and update.

| Field | Description |
| --- | --- |
| `resolutions` | Per-binary command resolution details. |
| `warnings` | User-facing advisory messages for shadowing, duplicates, and mixed roots. |
| `has_shadowing` | True when user-local binaries are shadowed by earlier candidates. |
| `has_ambiguity` | True when multiple candidates create an ambiguous command resolution. |

### `InstallTargetDecision`

Decision used by update replacement logic.

| Variant | Meaning |
| --- | --- |
| `CurrentExeDir { target }` | Replace the current executable path. |
| `PreferredUserBin { target, warnings }` | Replace the safe user-local target and report conflicts. |
| `ManualRepairRequired { attempted_target, guidance }` | Do not replace anything; return actionable guidance. |

Callers must propagate `ManualRepairRequired` as a user-visible error and must
not fall back to privileged writes.

## Regression output contract

Install/update smoke tests assert that normal user-facing output does not
contain:

| Forbidden output | Reason |
| --- | --- |
| `session_start.sh ❌` | Obsolete Python/shell hook verification. |
| `post_tool_use.sh ❌` | Obsolete Python/shell hook verification. |
| `pre_tool_use.sh ❌` | Obsolete Python/shell hook verification. |
| `profile_management` warning | Old noisy profile-management staging warning. |
| Known-safe bundled symlink skip warnings | Non-actionable copy noise during normal install/update. |

Diagnostic modes may include explicit file-copy details, but normal
install/update output must stay free of these known regressions.

## See also

- [Repair install/update PATH conflicts](../howto/repair-install-update-path-conflicts.md)
- [amplihack install reference](install-command.md)
- [Binary resolution reference](binary-resolution.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
