# How to Manage Multiple Claude Code Logins with Profiles

A *Claude login profile* is a named Claude Code configuration directory. For
claude.ai logins and API-key accounts, separate profiles let you keep, for
example, `work` and `personal` configurations side by side and choose between
them manually. There is one authentication exception: Claude Code does not
isolate two Claude Console sign-ins made without an API key because those
credentials live outside the configuration directory. See Claude Code's
[multiple-account documentation](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts).

> Not to be confused with [component profiles](../PROFILE_MANAGEMENT.md),
> which filter staged files during install. Login profiles only select which
> `CLAUDE_CONFIG_DIR` Claude Code runs with.

Profiles rely on Claude Code's documented `CLAUDE_CONFIG_DIR` variable.
amplihack never modifies `~/.claude` and makes no network calls for this
feature. See [What a new profile inherits](#what-a-new-profile-inherits).

## What a new profile inherits

`amplihack profile add` seeds the new profile with an **allowlist** of shared,
non-auth assets copied from `~/.claude` (resolved from `$HOME`). Authentication
and history stored in the selected configuration directory stay with that
profile, subject to the Console sign-in exception above.

Profiles preserve the calling process environment. An inherited
`ANTHROPIC_API_KEY` can select API billing instead of a subscription login; see
the official [environment variable reference](https://code.claude.com/docs/en/env-vars).

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
  copied. On Windows, source file handles open the final component without
  following a reparse point and reject reparse-point handles; path-based
  directory traversal still leaves same-user ancestor/directory races.
- **Permissions.** On Unix, copied directories are `0700` and files `0600`.
  Windows inherits the destination's ACLs. Existing files are never overwritten.
- **Limits.** Copying is bounded (16 directory levels, 5,000 entries, and
  64 MiB per copied asset file, and 1 MiB for `settings.json`); exceeding a
  bound is an error that fails the add (it is not silently truncated).
- **Non-Unix / Windows.** On Unix, source files are opened relative to a
  no-follow root descriptor to close the check-then-open race. Non-Unix
  platforms, including Windows, have no equivalent root descriptor, so a
  same-user race in ancestor or directory traversal remains possible there.
- **Hardlinks.** A hardlink is indistinguishable from a regular file, so a
  hardlink inside `~/.claude` to another file you own is copied like any other
  file. Only files the invoking user can already read are reachable.
- **Failure is rolled back.** If seeding fails (for example invalid
  `settings.json` gives `InvalidSharedSettings`), the private staging directory
  created by that call is removed and no profile is published. If cleanup also
  fails, the error reports both the seeding and cleanup failures. A pre-existing
  profile is never deleted.
- **Root overlap.** A profile root inside `~/.claude` is rejected before the
  profile root is created, including when the shared source or profile root
  contains not-yet-existing path components.

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

`add` copies the allowlisted shared assets into a private staging directory and
publishes `~/.amplihack/claude-profiles/work/` atomically after seeding succeeds
(mode `0700` on Unix); it fails if the profile already exists. `login`
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

Resolution order: `--profile` flag, then an inherited
`AMPLIHACK_CLAUDE_PROFILE`, then the default pointer, then none. An explicit
`--profile` never reads the inherited value or the pointer, so it works even if
the pointer is dangling or corrupt. With
no profile, `CLAUDE_CONFIG_DIR` is neither set nor altered, so existing
behavior (including an ambient `CLAUDE_CONFIG_DIR` you exported yourself) is
unchanged. When a profile is selected, it overrides the ambient value for the
child process only.

### Nested sessions keep the parent's profile

Once a launch picks a profile, every amplihack launch started from it uses
the same profile: recipe agent steps, `multitask` workstreams, and any
`amplihack claude` you run inside the session. This holds for sessions
started with `amplihack claude`, `amplihack launch`, `amplihack profile run`
and `amplihack profile login`. The launch passes the name to
its children in `AMPLIHACK_CLAUDE_PROFILE` (and the root in
`AMPLIHACK_CLAUDE_PROFILE_ROOT`, if you set one), and nested launches use it
ahead of the pointer.

```bash
amplihack profile switch personal
amplihack claude --profile work
# inside the session, and in every recipe step or workstream it starts:
echo "$AMPLIHACK_CLAUDE_PROFILE"   # work
amplihack profile switch personal  # changes the default for new shells only
amplihack claude                   # still runs as "work"
```

A `profile switch` therefore never moves a running session or orchestration
to another account part way through. It affects only launches started
afterwards from a shell without `AMPLIHACK_CLAUDE_PROFILE`.

To pin a profile for a run started from a plain shell, set the variable
yourself:

```bash
AMPLIHACK_CLAUDE_PROFILE=work amplihack recipe run default-workflow
```

If the inherited name is invalid or the profile has been removed, the launch
fails and does not fall back to the pointer:

```
error: inherited AMPLIHACK_CLAUDE_PROFILE "work": not found
hint: unset AMPLIHACK_CLAUDE_PROFILE, or create the profile with `amplihack profile add <name>`
```

See the
[resolution reference](../reference/profile-command.md#resolution-and-environment)
for how the name reaches each kind of nested launch.

## List and remove

```bash
amplihack profile list
amplihack profile remove personal --yes
```

`remove` deletes only the verified profile directory, requires `--yes`, and
clears the default pointer if it named that profile. This deletes that
account's local Claude login and history.

`remove` reads the default pointer before deleting anything. A missing pointer
means "no default" and removal proceeds. A corrupt, unreadable or dangling
pointer makes `remove` fail with the pointer error and leaves every profile
directory untouched. Repair it with `amplihack profile switch <name>`, then
re-run `remove`.

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
  An inherited `AMPLIHACK_CLAUDE_PROFILE` is rejected the same way, so a
  Docker or `--auto` launch started by hand inside a profiled session fails
  with `hint: unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto`.
  These modes keep the `CLAUDE_CONFIG_DIR` they inherit, which inside a
  profiled session is still the parent's profile, so unset both to run on
  the default account:
  `env -u AMPLIHACK_CLAUDE_PROFILE -u CLAUDE_CONFIG_DIR amplihack claude --auto`.
  Recipe steps and `multitask` workstreams never pass these flags, but they
  do inherit `AMPLIHACK_USE_DOCKER`. If it is set to a true value in a
  profiled session and the Docker daemon is running, every nested launch is
  refused. Unset it before starting the run.
- `amplihack recipe run` and `amplihack multitask` started from a plain shell
  (no inherited `AMPLIHACK_CLAUDE_PROFILE`) do not pin a profile: each agent
  step reads the default pointer itself, so a `profile switch` during the run
  moves later steps to the new profile. Start the run from inside an
  `amplihack claude` session, or prefix it with
  `AMPLIHACK_CLAUDE_PROFILE=<name>`.
- Removing a profile while sessions pinned to it are running makes their later
  nested launches fail closed with the inherited-profile error.
- Fleet and remote VM launches do not pass the profile on. An
  `amplihack claude` started inside one of them uses the default pointer.
- Copilot and Codex launches don't set the variables themselves, but they
  don't remove an inherited value either. A nested `amplihack claude` started
  from a Copilot session that inherited `AMPLIHACK_CLAUDE_PROFILE` uses that
  profile and checks it again. Without it, the launch uses the pointer.
- `amplihack recipe run` refuses context keys in the `AMPLIHACK_` namespace
  (`-c amplihack_claude_profile=x` is an error), so `-c` cannot choose the
  profile. A recipe's own YAML `context:` defaults and step `output:` names
  are not checked yet: the recipe runner exports them to bash steps under
  their uppercase names, so a recipe that defines `amplihack_claude_profile`
  can still change the profile used by an `amplihack claude` in its bash
  steps. It can only pick one of your own profiles, and the directory is
  still checked before spawn.
- The profile path is re-validated just before spawn. The check is not atomic;
  a small window remains.
- On Unix, launches require the profile directory to be owned by the current
  user with no group/other permission mode bits (`mode & 0o077 == 0`, so
  `0700` passes and `0755`, `0750` and `0705` are rejected). This checks mode
  bits; filesystem ACL entries are not inspected. Profiles created by older
  versions with `0755` need `chmod go-rwx <profile-directory>`. Only the
  profile directory is checked, not its parents; the profile root is created
  `0700`. The check is Unix-only.
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
