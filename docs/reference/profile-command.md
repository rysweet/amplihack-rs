# Reference: `amplihack profile`

Manage Claude Code login profiles. See the
[how-to](../howto/claude-profiles.md) for a walkthrough.

## Synopsis

```
amplihack profile add <name>
amplihack profile list
amplihack profile show
amplihack profile switch <name>
amplihack profile remove <name> --yes
amplihack profile login <name>
amplihack profile run <name> -- <claude-args>...
amplihack launch|claude --profile <name> [...]
```

## Subcommands

| Command | Behavior |
|---------|----------|
| `add <name>` | Create `<root>/<name>/` with mode `0700`. Errors if a profile exists whose name matches case-insensitively, on every platform. Does not run Claude. |
| `list` | One profile per line, sorted; the default is marked `*`. Skips dot-entries, symlinks and non-directories. |
| `show` | Prints `name: <name>` and `dir: <path>`, or `none`. Paths only; never directory contents. |
| `switch <name>` | Writes the default pointer. The profile must exist. |
| `remove <name>` | Requires `--yes`. Resolves and verifies the path is inside the root, deletes it, clears a matching pointer. Never deletes the root; never follows symlinks. |
| `login <name>` | Always runs the `claude` binary (ignores binary selection), interactively, with `CLAUDE_CONFIG_DIR=<profile dir>`. |
| `run <name> -- <args>` | One-shot launch through the normal launch path. Args are forwarded as a literal argv vector; no shell is involved. |

## Configuration

| Item | Value |
|------|-------|
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Profile root. Must be absolute and non-empty. Default `~/.amplihack/claude-profiles/`. |
| `<root>/<name>/` | Profile directory (mode `0700`). |
| Root directory | Created and set to mode `0700`. A symlinked root, or one not owned by the current user, is refused (best effort on Unix). |
| `<root>/.default` | Pointer file: only the profile name (mode `0600`, max 256 bytes). Written by creating a new temp file (`create_new`, never through a symlink) and renaming it over the pointer. |

## Name rules

Pattern `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`. Additionally rejected: `..`
anywhere, trailing `.`, path separators, NUL, and Windows-reserved names
(`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`, also with an extension,
case-insensitive). `add` rejects names that differ only by case from an existing profile on
every platform, so `Work` and `work` can never coexist.

## Resolution and environment

1. `--profile <name>` (explicit; the pointer is not read, so a dangling or
   corrupt pointer does not block it)
2. `<root>/.default`
3. none — `CLAUDE_CONFIG_DIR` is left exactly as inherited.

When a profile resolves, `CLAUDE_CONFIG_DIR` is set on the child `Command`
only (never `set_var`), overriding any ambient value, and only for
Claude-compatible binaries. The canonical path is re-verified immediately
before spawn.

`--profile` together with `--docker` or `--auto` fails with an
unsupported-mode error before anything is spawned. This includes Docker
activated through the environment (`AMPLIHACK_USE_DOCKER`). With Docker
activated that way and only a default pointer, the pointer is not applied and
a warning is printed.

A symlinked profile root is refused by every subcommand and by the pointer
reads/writes; a missing root is treated as empty.

## Literal argument forwarding

`run <name> -- <args>` passes args as an argv vector with no shell, so
`amplihack profile run work -- "; rm -rf ~"` hands that string to Claude
as one literal argument and executes nothing.

## Errors

Errors print one line to stderr as `error: <message>` plus `hint: <next step>`
and exit with status 1. Messages never include file contents.

| Condition | Hint |
|-----------|------|
| Invalid name | Lists the allowed pattern |
| Profile exists / not found | `amplihack profile list` |
| Symlinked or out-of-root profile | Remove the symlink and re-add |
| Pointer dangling (names a missing profile) | `amplihack profile switch <name>` |
| Pointer corrupt or oversized | Re-run `profile switch <name>` |
| `remove` without `--yes` | Re-run with `--yes` |
| `--profile` with `--docker`/`--auto` | Launch without the flag combination |
| Invalid `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Use an absolute path |

## Security notes

amplihack never opens, copies, logs or lists profile contents. Profiles are
not a security boundary against the same OS user. A same-user race between
path verification and spawn is accepted and mitigated by re-checking before
spawn.
