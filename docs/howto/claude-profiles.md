# How to Manage Multiple Claude Code Logins with Profiles

A *Claude login profile* is a named, isolated Claude Code configuration
directory. Each profile has its own login, settings and history, so you can
keep, for example, a `work` and a `personal` account side by side and choose
between them manually.

> Not to be confused with [component profiles](../PROFILE_MANAGEMENT.md),
> which filter staged files during install. Login profiles only select which
> `CLAUDE_CONFIG_DIR` Claude Code runs with.

Profiles rely on Claude Code's documented `CLAUDE_CONFIG_DIR` variable.
amplihack never touches `~/.claude`, never reads credentials, and makes no
network calls for this feature.

## Create and log in

```bash
amplihack profile add work
amplihack profile login work      # interactive claude; complete login inside it
amplihack profile add personal
amplihack profile login personal
```

`add` creates `~/.amplihack/claude-profiles/work/` (mode `0700`) and fails if
it already exists. `login` always runs the `claude` binary (not the configured launch binary) with `CLAUDE_CONFIG_DIR` pointing
at the profile so the login is stored there.

## Choose a profile

Per launch:

```bash
amplihack claude --profile work
amplihack profile run personal -- --model sonnet
```

As a persistent default:

```bash
amplihack profile switch work
amplihack claude              # uses "work"
amplihack profile show        # prints name and directory, or "none"
```

Resolution order: `--profile` flag, then the default pointer, then none. An
explicit `--profile` never reads the pointer, so it works even if the pointer
is dangling or corrupt. With
no profile, `CLAUDE_CONFIG_DIR` is neither set nor altered, so existing
behavior (including an ambient `CLAUDE_CONFIG_DIR` you exported yourself) is
unchanged. When a profile is selected, it overrides the ambient value for the
child process only.

## List and remove

```bash
amplihack profile list
amplihack profile remove personal --yes
```

`remove` deletes only the verified profile directory, requires `--yes`, and
clears the default pointer if it named that profile. This deletes that
account's local Claude login and history.

## Limits

- `run <name> -- args` forwards args literally with no shell; `-- "; rm -rf ~"`
  is passed to Claude as a single string.
- Names differing only by case (`Work` vs `work`) are rejected on every platform.
- Profiles apply only to Claude-compatible binaries (`claude`, `rusty`,
  `rustyclawd`, `amplifier`). Copilot and Codex launches are unaffected.
- `--profile` with `--docker` or `--auto` is rejected with an error rather
  than being partially applied.
- If the default pointer names a deleted profile, launch fails with a hint
  (`amplihack profile switch <name>`); it does not fall back silently.
- Profiles are not a security boundary against other processes of the same OS
  user. Anyone able to read your home directory can read the profile.
- No token vault, token refresh, usage probing or automatic rotation exists
  by design; see the [ccswitch review](../reviews/ccswitch-review.md).

## Testing without touching real state

```bash
export AMPLIHACK_CLAUDE_PROFILE_ROOT="$(mktemp -d)"
amplihack profile add demo
d="$(mktemp -d)"
printf '#!/bin/sh\necho "$CLAUDE_CONFIG_DIR"\n' > "$d/claude" && chmod +x "$d/claude"
PATH="$d:$PATH" amplihack profile run demo
```

The fake `claude` prints `$CLAUDE_CONFIG_DIR`, which should be the demo profile directory.
