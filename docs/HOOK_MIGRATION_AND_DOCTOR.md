# Global Hooks at Session Start & `amplihack doctor` Hook Detection

This guide explains how amplihack manages the relationship between **global**
hooks (`~/.claude/settings.json`) and **project-local** hooks
(`<project-root>/.claude/settings.json`) on session start, and how
`amplihack doctor` reports whether hooks are installed.

It documents why the session-start step never removes amplihack's global
hooks, and the conditions under which `amplihack doctor` reports a healthy
install.

> Related: [Hook Configuration Guide](HOOK_CONFIGURATION_GUIDE.md),
> [amplihack hooks block](reference/hook-block.md),
> [Hook Payload Compatibility](HOOK_PAYLOAD_COMPATIBILITY.md).

## Background: two places hooks can live

Claude Code (and amplihack's native hook runner) resolves hooks from two
locations:

| Location      | Path                                   | Scope                     |
| ------------- | -------------------------------------- | ------------------------- |
| Global        | `~/.claude/settings.json`              | Applies to every project  |
| Project-local | `<project-root>/.claude/settings.json` | Applies to one repository |

Amplihack hooks are identified inside a `settings.json` `hooks` section by a
hook `command` string that references `amplihack-hooks` or
`tools/amplihack/`. Either location is a **valid, fully working** install.

## Session-start step: global hooks are kept

On every session start, amplihack runs a small compatibility step
(`migrate_global_hooks` in `crates/amplihack-hooks/src/session_start/migration.rs`)
that looks at the global and project-local settings files. The step never
removes amplihack's hooks from `~/.claude/settings.json`. The governing
invariant is:

> **The session-start step never deletes amplihack's global hooks, and never
> reports a cleanup or migration.**

### Decision table

The step reads global settings and probes the project-local
`.claude/settings.json`. It writes neither file.

| Global has amplihack hooks | Project-local has amplihack hooks | Action                         | Notice shown |
| :------------------------: | :-------------------------------: | ------------------------------ | ------------ |
|    No (or file absent)     |              Either               | **Nothing.**                   | _(none)_     |
|            Yes             |        No (or file absent)        | **Nothing.**                   | _(none)_     |
|            Yes             |                Yes                | **Nothing.** Both copies stay. | _(none)_     |

Key points:

- **Global hooks are kept even when a project-local copy exists.** Earlier
  versions removed the global copy in this case. Every session started in a
  checkout or worktree with project-local hooks then stripped the home file,
  and sessions elsewhere ran without hooks. Install, the launch repair, the
  rotator cycle and `amplihack doctor` all expect the home file to hold the
  full hooks block, so the step no longer removes it. See
  [One source for the hooks block](concepts/hook-block-single-source.md).
- **No "removed" or "migrated" notice is printed.** The step makes no change,
  so it claims none.
- If the global `~/.claude/settings.json` **exists but cannot be read or
  parsed**, the step makes no changes and shows a cautionary notice
  (`⚠️ Global amplihack hooks may exist in ~/.claude/settings.json. Failed to
read the file for migration.`).
- When both files hold amplihack hooks, Claude Code reads both. Commands that
  differ, for example an older `tools/amplihack/` command in the project file,
  run as separate hooks.

### What you will see

Session start is silent about hooks whether or not the project has
project-local hooks, and `~/.claude/settings.json` keeps its hooks:

```text
$ amplihack   # start a session
# (no hook cleanup notice; global hooks remain in ~/.claude/settings.json)
```

To confirm the home file still has the resume hook after starting a session in
a project with project-local hooks:

```sh
jq '.hooks.StopFailure' ~/.claude/settings.json
```

The output is the `StopFailure` entry, not `null`.

## Data-loss protection for malformed settings

amplihack's settings writers are defensive about malformed configuration files.
If `~/.claude/settings.json` does not parse to a JSON **object** (for example, a
truncated write left an array, a bare string, a number or `null`), the writer
**leaves the value untouched** and logs a warning:

```text
WARN  Global settings.json was not a JSON object; left unchanged
```

A writer of amplihack's hooks will **never** overwrite an unexpected value
with an empty object (`{}`), so a partially written or hand-edited settings file
is never zeroed out.

All reads and writes use amplihack's atomic JSON file mechanism (temp-write +
atomic rename with a consistent backup), so a crash or a concurrent writer
cannot leave `settings.json` half-written.

## `amplihack doctor`: hook detection

`amplihack doctor` checks `~/.claude/settings.json`, each profile's
`settings.json` and the launch settings file against amplihack's hooks block,
and prints one line per file with its state (`clean`, `absent`, `unparseable`,
`refused` or the drifted event names). It does not check the project-local
`<cwd>/.claude/settings.json`. The output format, pass and fail rules and
secret hygiene are described in the
[hooks block reference](reference/hook-block.md#doctor-report).

## Frequently asked questions

**Will amplihack delete my global hooks?**
No. The session-start step no longer removes global hooks, including when a
project-local copy exists.

**Does `doctor` pass for a project-local install?**
No. `doctor` checks the global file, profile files and the launch settings
file. A global file without amplihack's hooks fails the check even when a
project-local copy exists; run `amplihack install` to restore it.

**Does this step write project-local hooks for me?**
No. This compatibility step only **reads** the project-local file. Writing project-local hooks (for example
choosing an install scope during the install wizard) is a separate feature.
