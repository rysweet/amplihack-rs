# Global Hooks at Session Start & `amplihack doctor` Hook Detection

This guide explains how amplihack manages the relationship between **global**
hooks (`~/.claude/settings.json`) and **project-local** hooks
(`<project-root>/.claude/settings.json`) on session start, and how
`amplihack doctor` reports whether hooks are installed.

It states what session start does with hooks, and the conditions under
which `amplihack doctor` reports a healthy install.

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

## Session start

Session start does not read or write the hooks in either settings file
(#2279); see the [hooks block reference](reference/hook-block.md).

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

Writes use a private temporary file and an atomic rename, so a crash cannot
leave `settings.json` half-written. A writer that changes the file first saves
a backup at mode `0600`; amplihack never restores a backup. A symlinked or
hard-linked `settings.json` is refused. See
[file safety](reference/hook-block.md#file-safety).

## `amplihack doctor`: hook detection

`amplihack doctor` checks `~/.claude/settings.json`, each profile's
`settings.json` and the launch settings file against amplihack's hooks block,
and prints one line per file with its state (`clean`, `absent`, `unparseable`,
`refused` or the drifted event names). A source it cannot list gets a failing
line of its own. It reads the project-local `<cwd>/.claude/settings.json` only
when the home file is absent. The output format, pass and fail rules and
secret hygiene are described in the
[hooks block reference](reference/hook-block.md#doctor-report).

## Frequently asked questions

**Will amplihack delete my global hooks?**
No. Session start does not touch them, including when a project-local copy
exists.

**Does `doctor` pass for a project-local install?**
Yes, when there is no `~/.claude/settings.json` and the project-local
`<cwd>/.claude/settings.json` matches the hooks block. The home line then
reads `absent; repo-local install clean`. A home file that exists but lacks
amplihack's hooks fails the check whatever the project-local file holds; run
`amplihack install` to restore it.
