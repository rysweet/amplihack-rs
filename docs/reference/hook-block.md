---
title: amplihack hooks block
description: The single definition of amplihack's Claude Code hooks, every place that writes it, the launch settings file, the doctor drift report and the file-safety rules.
last_updated: 2026-11-10
review_schedule: quarterly
owner: amplihack-cli
related:
  - ../concepts/hook-block-single-source.md
  - ../howto/diagnose-with-doctor.md
  - ../howto/claude-profile-rotation.md
  - ../HOOK_MIGRATION_AND_DOCTOR.md
---

# Reference: amplihack hooks block

amplihack registers its Claude Code hooks in a `hooks` object inside
`settings.json`. One module, `install/hook_block.rs`, defines that object and
is the only code that merges it into a file or compares a file against it.
For the reasons behind this design, see
[One source for the hooks block](../concepts/hook-block-single-source.md).

## Contents

- [The block](#the-block)
- [Files that hold the block](#files-that-hold-the-block)
- [Writers](#writers)
- [Merge rules](#merge-rules)
- [Drift](#drift)
- [Launch settings file](#launch-settings-file)
- [Doctor report](#doctor-report)
- [File safety](#file-safety)
- [Environment variables](#environment-variables)
- [Module interface](#module-interface)

## The block

The block is built from the `AMPLIHACK_HOOK_SPECS` table. Each entry becomes
one wrapper under its event name. The command is the quoted path to the
`amplihack-hooks` binary followed by a subcommand.

| Event              | Subcommand                         | Timeout (s) | Matcher      | `asyncRewake` |
| ------------------ | ---------------------------------- | ----------- | ------------ | ------------- |
| `SessionStart`     | `session-start`                    | 10          |              |               |
| `Stop`             | `stop`                             | 120         |              |               |
| `PreToolUse`       | `pre-tool-use`                     |             | `*`          |               |
| `PostToolUse`      | `post-tool-use`                    |             | `*`          |               |
| `UserPromptSubmit` | `workflow-classification-reminder` | 5           |              |               |
| `UserPromptSubmit` | `user-prompt-submit`               | 10          |              |               |
| `PreCompact`       | `pre-compact`                      | 30          |              |               |
| `SessionEnd`       | `session-stop-event`               | 30          |              |               |
| `StopFailure`      | `limit-resume`                     | see below   | `rate_limit` | `true`        |

That is nine entries across eight events. `StopFailure` is included only when
the installed Claude Code supports it; on an older Claude Code it is left out
and any existing amplihack `StopFailure` entry is removed (#1659). Its timeout
is `LIMIT_RESUME_TIMEOUT_SECS`.

A `PreToolUse` wrapper as written to a file:

```json
{
  "matcher": "*",
  "hooks": [
    {
      "type": "command",
      "command": "\"/home/you/.local/bin/amplihack-hooks\" pre-tool-use"
    }
  ]
}
```

The binary path is absolute and wrapped in double quotes; `~` is never used,
because Claude Code does not expand it inside a quoted path.

The block never contains an XPIA hook, whether or not `tools/xpia/hooks/` is
present on disk. See [XPIA](#xpia).

## Files that hold the block

| File                                                 | Created by amplihack                    | Kept current by                                                           |
| ---------------------------------------------------- | --------------------------------------- | ------------------------------------------------------------------------  |
| `~/.claude/settings.json`                            | Yes, by `amplihack install` only        | install, update, launch repair, rotator cycle                             |
| `$CLAUDE_CONFIG_DIR/settings.json`                   | No                                      | launch repair, when `CLAUDE_CONFIG_DIR` is set and the file exists        |
| `<profile root>/<name>/settings.json`                | Copied at profile seeding               | profile refresh, profile seeding; rotator cycle for the live profile only |
| `$AMPLIHACK_HOME/runtime/launch-hooks-settings.json` | Yes, at each `claude` launch            | `amplihack launch` / `amplihack claude`                                   |

The profile root is resolved through `root_config`, as for every other
`amplihack profile` command; its default is `~/.amplihack/claude-profiles`.
A profile with no `settings.json` stays without one. No writer other than
install creates a settings file, and install always targets
`~/.claude/settings.json`; it does not read `CLAUDE_CONFIG_DIR`.

Only the launch repair reads `CLAUDE_CONFIG_DIR`. Install, the rotator cycle
and doctor work on `~/.claude/settings.json` whether or not the variable is
set, so a `$CLAUDE_CONFIG_DIR/settings.json` that has drifted is repaired only
by the next `amplihack launch` and is not shown by doctor.

## Writers

Every writer below calls the same merge. None of them has its own copy of the
hook list.

| Writer                         | When it runs                                        | Target                                                     | Skipped when                                                                                           |
| ------------------------------ | --------------------------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `amplihack install` / `update` | On request                                          | `~/.claude/settings.json` (created as `{}` if absent)      | Never                                                                                                  |
| Launch repair                  | Each `amplihack launch` / `amplihack claude`        | `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/...`   | The running binary is older than the install stamp; `AMPLIHACK_SKIP_AUTO_INSTALL` is set; no drift     |
| Profile refresh                | `amplihack profile` refresh paths                   | Each `<profile root>/<name>/settings.json`                 | The profile root is refused by `hook_path_refusal`; the file is absent                                 |
| Profile seeding                | After `settings.json` is copied into a new profile  | The new profile's `settings.json`                          | Older binary; no hooks binary; the profile root is refused. Seeding still succeeds                     |
| Rotator cycle                  | End of each `swapped` or `stayed` cycle             | `~/.claude/settings.json` and the live profile's file only | Read-only mode; older binary; no hooks binary; no drift. A refused or unidentified live profile is logged and does not stop the home heal |
| Launch settings file           | Each `claude` launch                                | `$AMPLIHACK_HOME/runtime/launch-hooks-settings.json`       | See [When the file is not used](#when-the-file-is-not-used)                                            |

Nothing else in amplihack removes amplihack's hooks from a settings file. The
session-start step in `amplihack-hooks` that used to delete the global hooks
when a project also had project-local hooks no longer does so; see
[Global hooks at session start and doctor](../HOOK_MIGRATION_AND_DOCTOR.md).

### Launch repair

The launch repair reads the target with the drift check first and writes only
when the check reports drift. An absent target is left absent. Any missing or differing event counts, so a file
that lost only `StopFailure`, or holds a stale `StopFailure` entry, is repaired
as well as a file that lost every hook.

A binary older than the install stamp never writes, because it would write an
older block. If reading the install stamp fails, the binary is treated as
older. In that case `amplihack doctor` reports the drift and
`amplihack install` repairs it.

An unreadable or refused file produces one warning line and no write. The
wording below is an example; only the path and the reason in brackets are
fixed:

```text
amplihack: warning: /home/you/.claude/settings.json could not be used (is a symlink); hook registrations were not checked or repaired. Fix the file, then run `amplihack install`.
```

### Rotator cycle

`amplihack profile rotate` (both a single run and the watch daemon) checks the
hooks after each cycle that ends `swapped`, `stayed` or `switched`. It runs
after the cycle has released the swap lock and adds no timer of its own; it
reacts to the cycle finishing.

The live profile's directory comes from the profile store and `root_config`,
never from the ledger, a status file or the environment. Profiles other than
the live one are not checked by the cycle; they are repaired only by profile
refresh. The heal never
touches `.claude.json`, `.credentials.json` or any login, and it never changes
the cycle outcome or the exit code. It logs one line per file it writes. The
wording below is an example; the path and the event names are fixed:

```text
hooks: repaired /home/you/.claude/settings.json (PreToolUse, PostToolUse, StopFailure)
```

## Merge rules

The merge changes only `settings["hooks"]`.

- **Added or corrected:** every spec entry for the current `limit_resume`
  value, with the current hooks binary path, timeout, matcher and
  `asyncRewake`.
- **Removed:** amplihack's own wrappers that are stale (an old binary path, a
  duplicate, a `StopFailure` entry when `limit_resume` is off), any wrapper
  whose command contains `tools/xpia/hooks/`, and any wrapper left empty by
  these removals. A wrapper whose `hooks` list was already empty is not
  amplihack's and is kept.
- **Kept byte-equal and in order:** every wrapper that amplihack does not own.
  A wrapper is amplihack's only when `is_own_wrapper` matches it against a spec,
  or its command contains `tools/xpia/hooks/`. A foreign hook such as
  `block-tmux-kill.py` is kept even when another field mentions amplihack.
- **Never touched:** `env`, `permissions`, `statusLine` (outside install),
  `model` and every other top-level key.

The merge is idempotent. Merging an already-current file reports `Unchanged`,
and the file is not rewritten: its bytes and modification time stay the same
and no backup is made.

### XPIA

No writer adds an XPIA hook, and every writer removes one it finds. No writer
adds, changes or removes keys under `env`. Where `env.XPIA_ENABLED` is absent
it stays absent; where it is `"false"` it stays `"false"`. XPIA asset checks
in `amplihack install` (#1266) are unchanged.

### Backups

Two writers go through `update_file_with_backup` and so back up the file
before they change it: `amplihack install` (and `update`), and the launch
repair. Each writes a backup only when the merge changes the file. The backup
is written from the bytes already read, opened with
`O_CREAT | O_EXCL | O_NOFOLLOW` and mode `0600`. No code path ever restores a
backup over a settings file.

Profile refresh, profile seeding and the rotator cycle call `merge_file` and
make no backup.

Profile refresh runs only inside `amplihack install`, which is the explicit
installer; it does not consult the install stamp. Profile seeding is implicit
and writes nothing from a binary older than the install stamp.

There is no lock between amplihack's writers and other programs. If another
process changes `hooks` between amplihack's read and its rename, that change
is lost; changes to other keys made in the same window are lost too. This is
accepted: the window is one read, merge and rename, and the next drift check
reports any result.

## Drift

The drift check compares a file's `hooks` object against the block and
returns three lists of event names.

| List        | An event is listed when                                                                                                               |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `missing`   | A spec entry for the event has no matching wrapper                                                                                    |
| `differing` | amplihack's wrapper for the event exists but its binary path, timeout, matcher or `asyncRewake` differs, or it appears more than once |
| `extra`     | The file holds an amplihack-owned wrapper not in the spec: an XPIA hook, or `StopFailure` when `limit_resume` is off                  |

One event may appear in both `missing` and `extra`. A file is clean when all
three lists are empty. The check is read-only.

`amplihack install` uses the same check for its post-install contract
validation, and the implicit-install warning uses its `missing` list. A file
that contains no amplihack hook at all reports every event as missing.

## Launch settings file

When the agent binary is `claude`, `amplihack launch` and `amplihack claude`
write the block to a private file and pass it to Claude Code as an extra
settings source:

```text
claude --settings /home/you/.amplihack/runtime/launch-hooks-settings.json <your arguments>
```

`--settings` is placed before the arguments you pass. The path is passed, never
inline JSON, so no settings text appears in the process command line (#2030).

The file holds only the hooks object:

```json
{
  "hooks": {
    "SessionStart": [ ... ],
    "Stop": [ ... ]
  }
}
```

It contains no `env`, `permissions` or other key. It is written with mode
`0600` inside `runtime/` (mode `0700`), and only when its bytes would change.

Claude Code reads `--settings` as a source separate from the user settings
file, so a session started through amplihack has amplihack's hooks even if
something has stripped `~/.claude/settings.json`.

### Duplicate hook commands

The commands in the launch file and the home file are built by the same code
from the same binary path, so they are byte-identical.

Claude Code runs a handler once when the same handler is defined in more than
one settings file (<https://code.claude.com/docs/en/hooks>), so the session
runs each amplihack hook once.

### When the file is not used

Checked in this order. In each case the launch continues without
`--settings`.

| Condition                                                                    | Behaviour                                        |
| ---------------------------------------------------------------------------- | ------------------------------------------------ |
| The agent binary is not `claude`                                             | Nothing written                                  |
| The LiteLLM proxy or safe mode is in use (it passes `--setting-sources ""`)  | Nothing written                                  |
| You passed `--settings <x>` or `--settings=<x>` before any literal `--`      | One notice line; the home-file repair still runs |
| The running binary is older than the install stamp                           | Nothing written                                  |
| No `amplihack-hooks` binary is found, or its path contains unsafe characters | Nothing written                                  |
| `$AMPLIHACK_HOME` is not owned by you, or is writable by group or others     | Nothing written                                  |
| `runtime/` is a symlink, is not owned by you, or cannot be made `0700`       | Nothing written                                  |
| Writing the file fails                                                       | One warning line                                 |

When the skip is because `$AMPLIHACK_HOME` or `runtime/` is not private, the
launch prints one warning that names the directory and says it must not be
group- or other-writable. The other skips are logged at debug level only.

A `--settings` that appears after `--` is an argument to something else and
does not suppress the injection.

## Doctor report

`amplihack doctor` checks every settings file that should hold the block and
prints one line per file, in this order:

1. `~/.claude/settings.json`;
2. each `<profile root>/<name>/settings.json`, sorted by profile name;
3. `$AMPLIHACK_HOME/runtime/launch-hooks-settings.json`.

Each line has the form `<path>: <state>`, where the state is one of:

| State                                          | Meaning                                                 |
| ---------------------------------------------- | ------------------------------------------------------- |
| `clean`                                        | The file matches the block                              |
| `absent`                                       | The file does not exist                                 |
| `unparseable (<reason>)`                       | Not valid JSON; the reason gives line and column only   |
| `refused (<reason>)`                           | Not read, for a [file-safety](#file-safety) reason      |
| `drift missing=[..] differing=[..] extra=[..]` | The file differs from the block; lists hold event names |
| `absent; repo-local install clean`             | Home file only: absent, and `<cwd>/.claude/settings.json` matches the block |

When the profile root cannot be resolved or listed, or the amplihack home is
unknown or not absolute, doctor prints one line for that source instead of
leaving its files out:

```text
profiles: cannot list (permission denied)
launch file: cannot list (amplihack home unknown or not absolute)
```

Example with one stripped profile. The `<path>: <state>` lines are fixed; the
`✗ hooks installed` heading and the indentation are an example of doctor's
existing grouping:

```text
$ amplihack doctor
✗ hooks installed
    /home/you/.claude/settings.json: clean
    /home/you/.amplihack/claude-profiles/team/settings.json: drift missing=[PreToolUse, PostToolUse, StopFailure] differing=[] extra=[]
    /home/you/.amplihack/claude-profiles/work/settings.json: clean
    /home/you/.amplihack/runtime/launch-hooks-settings.json: clean
```

The check fails when the home file is anything other than `clean`, when a
profile or launch file that exists shows drift, is unparseable or is refused,
or when a source cannot be listed. An absent profile or launch file passes.
An absent home file passes only when the project-local
`<cwd>/.claude/settings.json` is clean, which is a repo-local install (#1088).
A project-local file is read only in that case and has no line of its own. Failure uses doctor's existing exit
code; there is no new one.

The check is read-only: it changes no bytes and no modification times. Its
output never contains file content, only paths, event names, fixed phrases,
`io::ErrorKind` names and JSON line and column numbers.

Doctor does not check:

- the project-local `<cwd>/.claude/settings.json`, except as above when the
  home file is absent;
- `$CLAUDE_CONFIG_DIR/settings.json`. When `CLAUDE_CONFIG_DIR` is set, the
  file the session reads may differ from the home file doctor reports.

To fix drift, run `amplihack install`. The live profile's file is repaired by
the next rotator cycle; other profile files are repaired by profile refresh.

## File safety

Every read and write of a settings file in this module follows these rules.

| Rule                   | Detail                                                                                |
| ---------------------- | ------------------------------------------------------------------------------------- |
| No symlinks            | Opened with `O_NOFOLLOW`; a symlink is `refused (is a symlink)`                            |
| Regular files only     | A FIFO, socket or directory is refused                                                |
| Size limit             | Files over 1 MiB are refused                                                          |
| Owner                  | A file not owned by the effective uid is refused                                      |
| Hard links             | A file with more than one link is refused at replace time                             |
| Root must be an object | A file whose JSON root is not an object is refused and left unchanged                 |
| Atomic write           | Temp file with `O_EXCL` and mode `0600`, `fsync`, `rename`                            |
| Write only on change   | An unchanged file keeps its bytes and modification time                               |
| No creation            | Only `amplihack install` creates a settings file; every other writer reports `Absent` |
| No content in messages | Reasons and log lines never quote the file                                            |
| Unix only              | On other platforms every file operation returns `refused (unix-only)`                 |

The first write by this module sets the file mode to `0600`. A home
`settings.json` that was `0644` becomes `0600` after its first repair.

A `settings.json` that is a symlink (for example one managed by GNU Stow or
another dotfile manager) or has more than one hard link is refused by every
writer, including `amplihack install`, which fails. Replace the link with a
regular file to let amplihack manage its hooks.

Before any profile file is written, `hook_path_refusal` checks the profile
root, so a test or tool running with a throwaway `HOME` cannot write a real
profile (#1699).

If two processes change the `hooks` object at the same moment, the later write
wins. There is no lock or retry.

## Environment variables

| Variable                        | Effect                                                                                                                                                |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CLAUDE_CONFIG_DIR`             | Launch repair targets `$CLAUDE_CONFIG_DIR/settings.json`. Used only when set, non-empty and absolute. Install, the rotator cycle and doctor ignore it |
| `AMPLIHACK_HOME`                | Location of `runtime/launch-hooks-settings.json`. Used only when set, non-empty and absolute                                                          |
| `AMPLIHACK_SKIP_AUTO_INSTALL`   | Suppresses the launch repair                                                                                                                          |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Profile root for refresh, seeding, the rotator heal and doctor                                                                                        |

## Module interface

`crates/amplihack-cli/src/commands/install/hook_block.rs`:

```rust
pub(crate) struct HookTarget { pub bin: PathBuf, pub limit_resume: bool }

/// None when no hooks binary is found or its path has unsafe characters.
pub(crate) fn hook_target() -> Option<HookTarget>;

pub(crate) fn block(t: &HookTarget) -> serde_json::Value;
pub(crate) fn merge(settings: &mut Value, t: &HookTarget) -> Merge;   // Changed | Unchanged
pub(crate) fn drift(settings: &Value, t: &HookTarget) -> Drift;       // missing, differing, extra

pub(crate) enum FileMerge { Written, AlreadyCurrent, Absent, Unparseable(String), Refused(String) }
pub(crate) fn merge_file(path: &Path, t: &HookTarget) -> FileMerge;
pub(crate) fn update_file(path: &Path, f: impl FnOnce(&mut Value)) -> FileMerge;
pub(crate) fn update_file_with_backup(path: &Path, f: impl FnOnce(&mut Value)) -> FileMerge;

pub(crate) enum FileDrift { Clean, Absent, Unparseable(String), Refused(String), Drift(Drift) }
pub(crate) fn drift_file(path: &Path, t: &HookTarget) -> FileDrift;   // read-only
```

`FileDrift` maps to the doctor states: `Clean` to `clean`, `Absent` to
`absent`, `Unparseable(r)` to `unparseable (r)`, `Refused(r)` to
`refused (r)` and `Drift(d)` to the `drift missing=[..] differing=[..]
extra=[..]` line. A reason holds only a path, an event name, a fixed phrase or
an `io::ErrorKind` name, never text from the file.

`Drift::is_clean()` is true when all three lists are empty; `Drift::lines()`
gives the strings used by the install contract validation.

Profile refresh maps `FileMerge` to its `Refresh` result:

| `FileMerge`                    | `Refresh`    |
| ------------------------------ | ------------ |
| `Written`                      | `Updated`    |
| `AlreadyCurrent`               | `Unchanged`  |
| `Absent`                       | `Missing`    |
| `Unparseable(r)`, `Refused(r)` | `Skipped(r)` |
