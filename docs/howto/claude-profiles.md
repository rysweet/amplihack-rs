# How to Manage Multiple Claude Code Logins with Profiles

A *Claude login profile* is a named, isolated Claude Code configuration
directory. Each profile has its own login, settings and history, so you can
keep, for example, a `work` and a `personal` account side by side and choose
between them manually.

> Not to be confused with [component profiles](../PROFILE_MANAGEMENT.md),
> which filter staged files during install. Login profiles only select which
> `CLAUDE_CONFIG_DIR` Claude Code runs with.

Profiles rely on Claude Code's documented `CLAUDE_CONFIG_DIR` variable.
amplihack never modifies `~/.claude` and makes no network calls for this
feature. See [What a new profile inherits](#what-a-new-profile-inherits).

## What a new profile inherits

`amplihack profile add` seeds the new profile with an **allowlist** of shared,
non-auth assets copied from `~/.claude` (resolved from `$HOME`). Login
credentials and history stay isolated per profile.

| Copied | Details |
|--------|---------|
| `CLAUDE.md` | Your global instructions |
| `agents/`, `commands/`, `output-styles/`, `rules/`, `skills/` | Copied recursively |
| `settings.json` (filtered) | Only allowlisted top-level keys: `alwaysThinkingEnabled`, `attribution`, `cleanupPeriodDays`, `hooks`, `includeCoAuthoredBy`, `language`, `model`, `outputStyle`, `permissions`, `respectGitignore`, `spinnerVerbs`, `statusLine` |

Never copied: `.credentials.json`, `.claude.json`, `history*`, `projects/`,
`sessions/`, `todos/`, `statsig/`, `shell-snapshots/`, `ide/`, and anything else not
on the allowlist (so plugins and MCP server settings are not carried over), and the `env` and `apiKeyHelper` settings keys (and any other
key not on the allowlist). If `~/.claude` does not exist, the profile is simply
created empty.
If the path exists but is not a directory, `profile add` fails and removes the
new profile directory rather than silently creating an unseeded profile.

Caveats:

- **Copy by value.** Nothing is linked. Later edits to `~/.claude` do not
  propagate to existing profiles; re-add the profile to pick them up.
- **Allowlisted setting values are copied verbatim.** Review hooks and
  status-line commands for embedded secrets or machine-specific paths before
  adding a profile; what they contain is your responsibility. Values are never
  logged.
- **Symlinks.** A symlinked `~/.claude` is refused (`SymlinkRefused`); symlinked
  ancestors of it are resolved normally. A missing `CLAUDE.md` or `settings.json`
  is skipped. Symlinks
  and special files (FIFOs, sockets, devices) inside the shared directories are
  skipped, never followed or copied. On Unix the copy is descriptor-based:
  `~/.claude` is opened once with `O_NOFOLLOW` and everything else is read with
  `openat` relative to that descriptor, so swapping `~/.claude` or a
  directory for a symlink mid-copy is not followed and no outside data is
  copied; non-Unix platforms lack this race hardening.
- **Permissions.** Copied directories are `0700`, files `0600`. Existing files
  are never overwritten.
- **Limits.** Copying is bounded (16 directory levels, 5,000 entries); exceeding a bound
  is an error that fails the add (it is not silently truncated).
- **Non-Unix / Windows.** On Unix, source files are opened relative to a
  no-follow root descriptor to close the check-then-open race. Non-Unix
  platforms, including Windows, have no equivalent, so a same-user race remains
  possible there.
- **Hardlinks.** A hardlink is indistinguishable from a regular file, so a
  hardlink inside `~/.claude` to another file you own is copied like any other
  file. Only files the invoking user can already read are reachable.
- **Failure is rolled back.** If seeding fails (for example invalid
  `settings.json` gives `InvalidSharedSettings`), the profile directory created
  by that call is removed. If cleanup also fails, a warning is printed and the
  original seeding error is returned. A pre-existing profile is never deleted.

Example:

```bash
amplihack profile add work
# error: InvalidSharedSettings: ~/.claude/settings.json is not valid JSON
# (exact wording may vary; no profile directory is left behind)
```

## Create and log in

```bash
amplihack profile add work
amplihack profile login work      # interactive claude; complete login inside it
amplihack profile add personal
amplihack profile login personal
```

`add` creates `~/.amplihack/claude-profiles/work/` (mode `0700`), copies the
allowlisted shared assets, and fails if the profile already exists. `login`
always runs the `claude` binary (not the configured launch binary) with
`CLAUDE_CONFIG_DIR` pointing at the profile so the login is stored there.

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
- Docker is unsupported with profiles. `profile run`, `launch --profile`,
  `--docker`, `--auto` and env-activated Docker (`AMPLIHACK_USE_DOCKER`) are all
  rejected with an error rather than partially applied. A default pointer under
  env-activated Docker is skipped with a warning.
- The profile path is re-validated just before spawn. The check is not atomic;
  a small window remains.
- The seeded `hooks`, `statusLine` and `permissions` keys are code-execution
  configuration: hooks and the status line run commands, and permissions
  control what Claude may run. Review them before sharing a profile, and treat
  `~/.claude/settings.json` as trusted input.
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

The fake `claude` prints `$CLAUDE_CONFIG_DIR`, which should be the demo profile
directory. Profile creation copies only allowlisted assets from `$HOME/.claude`
when that directory exists; use a temporary HOME to test that behavior safely.
