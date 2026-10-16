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
To move work to another account automatically when one runs out of quota,
see [Switch accounts automatically](#switch-accounts-automatically).

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

`remove` also refuses a profile that [auto-switch](#switch-accounts-automatically)
is still using: one whose account is live in another profile's directory,
whose own login is parked, or whose directory is hosting another account.
End the sessions involved and run `amplihack profile auto-switch status`
until it shows `hosted directories: none`.

## Switch accounts automatically

When one account is close to its five-hour or seven-day usage limit,
auto-switch moves work onto another of your profiles that has quota left.
It is **off by default**. With it off, and before it has ever been turned on,
amplihack behaves exactly as described in the rest of this guide.

Auto-switch does three things:

| Where | What happens |
|-------|--------------|
| A running Claude session | When the session's account reaches the threshold, the login in that session's configuration directory is swapped for the account with the most quota left. The session keeps running. |
| Nested launches (recipe agent steps, `multitask`/orch workstreams) | A launch that would inherit an exhausted profile starts under a profile with quota left instead. |
| A recipe agent step that fails with a usage limit | The step is run again under the next profile with quota left instead of ending the run. |

amplihack learns how much quota each account has used from the status line
Claude Code already computes. It sends no requests of its own, makes no
network calls, and never reads, parses, copies or refreshes a token.

### 1. Install the usage meter

Claude Code versions that pass `rate_limits` to the status line give it the
account's `five_hour` and `seven_day` usage. amplihack wraps your existing status line
so it can record those numbers, then runs your original command and prints
its output unchanged.

Install the wrapper into every profile. Run it with the same profile root
your launches use:

```bash
export AMPLIHACK_CLAUDE_PROFILE_ROOT="$HOME/.claude-profiles"   # if your profiles live here
amplihack profile auto-switch install
```

```
root: /home/ryan/.claude-profiles
installed  personal (no previous status line)
unchanged  review
installed  work     (wrapped existing status line)
```

The command prints the root it acts on. If that root has no profiles, it
warns that `AMPLIHACK_CLAUDE_PROFILE_ROOT` may be unset. Running it again
changes nothing: every `settings.json` stays byte-identical. It covers every
profile under the root, including ones you created before auto-switch
existed, edits only each profile's own `settings.json`, and never touches
`~/.claude`. New profiles do not get the wrapper automatically: run
`install` again after `profile add`. `auto-switch on` and `status` list any
profile that is still missing it.

The wrapper calls amplihack by the absolute path of the binary that ran
`install`. If you later move or reinstall amplihack, `status` shows those
profiles as `stale`, and `install` reports them as `stale` without changing
them. Fix them with `repair`:

```bash
amplihack profile auto-switch repair
```

```
root: /home/ryan/.claude-profiles
unchanged  personal
repaired   review   (wrapper pointed at a different binary)
unchanged  work
recovery: nothing to do
```

`repair` finishes any interrupted swap and returns logins whose session has
ended (see [Recovery](#recovery)), then does what `install` does and also
rewrites wrappers that are stale, wrapped twice or edited by hand. The last
lines say what recovery did, for example
`recovery: returned the login of personal hosted in work (session ended)`.
`install` refuses a binary in a directory that other users can write to,
and warns if the binary is inside a Cargo `target/` directory.

### 2. Turn it on

```bash
amplihack profile auto-switch on
amplihack profile auto-switch status
```

The setting file is read again on every check, so turning it on or off takes
effect in sessions that are already running, from their next status-line
update. The exception is a session started with
`AMPLIHACK_CLAUDE_AUTO_SWITCH` set: the variable overrides the file and cannot
be changed once the session is running.

The default threshold is 90%: an account counts as exhausted when **either**
its five-hour or its seven-day window is at or above it. To change it, or to
stop some profiles from ever being chosen:

```bash
amplihack profile auto-switch threshold 80
amplihack profile auto-switch exclude review shared   # never switch *to* these
amplihack profile auto-switch exclude                 # clear the list
amplihack profile auto-switch on --threshold 85 --exclude review
```

An excluded profile is never chosen as a target, but a session running on it
can still be switched away from it.

To force the setting for one shell, which takes precedence over the file:

```bash
export AMPLIHACK_CLAUDE_AUTO_SWITCH=0   # off in this shell, whatever the file says
export AMPLIHACK_CLAUDE_AUTO_SWITCH=1   # on in this shell
```

An empty value means unset. Any other value prints one warning, `status`
notes it, and the file decides.

### 3. Check what it sees

```bash
amplihack profile auto-switch status
```

```
root: /home/ryan/.claude-profiles
switch: on (file)
threshold: 90%
exclude: review
in-session swap: available (Linux)
profile    5h    7d    observed   login   wrapper
personal   12%   40%   1m ago     home    installed
review     -     -     never      home    installed
work       93%   61%   2m ago     home    installed
hosted directories: none
invariant: ok (every login is in exactly one place)
```

Notes follow the table when they apply, for example:

```
note: profiles without the wrapper: scratch — run: amplihack profile auto-switch install
note: excluded but missing: shared
```

While a session in `work` is using `personal`'s login, `work` shows
`parked`, `personal` shows `live in work`, and the line
`hosted: work holds the login of personal (swapped 3m ago)` replaces
`hosted directories: none`.

`5h` and `7d` are the latest readings. A window whose reset time has passed
is shown and treated as 0%. `-` means no reading yet: the wrapper records
nothing while the switch is off, so readings start at the first status-line
update after you turn it on. A profile with no reading is treated as having
quota left, but is chosen only after every profile with a known reading.

`status` only reads. It never moves a login or finishes a swap, so looking
at the state cannot change it. If a swap was interrupted or a session ended
without returning its login, `status` says so and tells you to run
`amplihack profile auto-switch repair`; the next launch or status-line
update fixes it too.

Usage is tracked per profile. If two profiles are signed in to the same
Anthropic account, `status` warns that their readings are not reliable;
this setup is not supported.

### What happens inside a running session

After each Claude turn, the status-line wrapper records the reading, prints
your status line, and then checks the session's account. If another
amplihack process holds the state lock for more than 50 ms, the wrapper
skips this turn's recording, recovery and check rather than slow your
status line; the next turn tries again. If that account is
at or above the threshold, the wrapper swaps the login in the session's
configuration directory for the eligible account with the lowest usage (or
one whose limit has reset). The session continues; Claude picks up the new
login on its next request.

Each account's login lives in exactly one place at a time, because Claude
rotates refresh tokens and a stored copy would go stale. A swap is therefore
a move, never a copy:

1. The live login, which Claude may have just refreshed, moves back to the
   account it belongs to.
2. The chosen account's login moves into the session's directory.
3. amplihack records which account is now live there and updates the
   account name Claude shows.

While a login is live in another profile's directory, that account is
*leased*: launching or logging into its own profile directly is refused
until the session ends:

```
error: profile "personal": its login is in use by a session in profile "work"
hint: wait for that session to end, or run `amplihack profile auto-switch status`
```

While a profile's directory is hosting another account's login, that
profile cannot be launched directly either. From a second terminal,
`amplihack claude --profile work` is refused until the session in `work`
ends:

```
error: profile "work": its directory holds the login of profile "personal" for a running session
hint: wait for that session to end, or run `amplihack profile auto-switch status`
```

When the session ends, every login goes back to its own profile. A nested
launch inside the session (a recipe step, a workstream) shares the session's
directory and its current login; it does not trigger a lease refusal. It
counts as nested only if it inherited the session's `CLAUDE_CONFIG_DIR`:
exporting `AMPLIHACK_CLAUDE_PROFILE=work` in a new shell is not enough.

amplihack also updates `oauthAccount` in the session's `.claude.json`, the
account name `/status` shows. Claude keeps that file in memory and rewrites
it, so `/status` may keep showing the previous account until Claude next
rewrites the file or the session restarts. The login itself has moved:
`.credentials.json` decides which account is used, and the
`auto-switch status` table is the source of truth.

In-session swaps only happen in sessions that show a status line, because
the status line is the only place Claude reports usage. Non-interactive
`claude -p` runs are covered by the next section instead.

### What happens for nested launches and recipe steps

When the switch is on, a nested launch that inherits a profile whose account
is exhausted starts under the eligible profile with the most quota left
instead, and prints one line to stderr:

```
amplihack: profile "work" is at 93% of its five-hour limit; auto-switch is using profile "personal" for this launch
```

The candidate must hold its own login and must not be hosting another
account. Nested launches never move logins. An explicit `--profile` and the
default pointer are never redirected; only an inherited profile is.
`multitask` workstreams make the same choice when their `run.sh` is
written, and again when each nested launch starts.

When a recipe agent step (a nested `amplihack claude -p …`) fails with a
usage limit, and the switch is on, the step is run again under the next
eligible profile:

```
amplihack: profile "work" hit its usage limit; rerunning this step under profile "personal" (auto-switch)
```

A step counts as having hit its limit when it exits non-zero and the end of
its output matches the same usage-limit check the recipe runner uses to end
a run (`usage limit`, `session limit`, `quota exceeded` and the like). So a
step the runner would stop for is always tried on another profile first.
Output that also names a work or environment problem, such as a failed test
or `permission denied`, is not treated as a usage limit and is not rerun.
`credit balance is too low` and `disk quota exceeded` count as environment
problems, so they are not rerun either.
Each profile is tried at most once per step. When none is left, the last
attempt's output and exit code are passed back, and the recipe run stops
with today's usage-limit message. With the switch off, a usage limit ends
the run exactly as before.

The rerun happens inside the step's own `amplihack claude -p` launch, not
in the recipe runner, so only the failed step runs again; finished steps
are not repeated.

While a step is supervised this way, its error output streams as usual but
its standard output is held back and printed when the last attempt finishes.
A rerun starts the step from the beginning, so anything the failed attempt
had already done (a push, a comment) may be done again.

### Turn it off

```bash
amplihack profile auto-switch off
```

`off` keeps your threshold and exclusions. It stops new swaps,
substitutions and reruns immediately, including in running sessions, but it
does not pull a login out of a session that is running: those logins go back
to their own profiles when the session ends. Until then the lease refusals
above still apply.

To stop using a version of amplihack that has auto-switch, for example to
downgrade:

1. `amplihack profile auto-switch off`
2. End every Claude session started under a profile.
3. Run `amplihack profile auto-switch repair`, then
   `amplihack profile auto-switch status`, and check that it shows
   `hosted directories: none` and `invariant: ok`.

An older amplihack does not know about parked logins and could launch a
profile whose login is elsewhere.

### Recovery

amplihack recovers on its own after a crash or a killed session. Every
launch, `profile login`, status-line update and `auto-switch repair` first
finishes or rolls back any interrupted swap, then returns every login whose
session has ended. This happens even with the switch off, so turning it off
never strands a login. `auto-switch status` only reports what needs doing.

To recover now rather than at the next launch:

```bash
amplihack profile auto-switch repair
```

If the state files are damaged in a way it cannot interpret, it moves
nothing and refuses launches and logins for the profiles involved:

```
error: auto-switch state is inconsistent: .auto-switch.d/journal.json: unknown version or invalid name
hint: see the Recovery section of docs/howto/claude-profiles.md
```

To repair it by hand:

1. End every Claude session started under a profile.
2. Find each login. Every account's `.credentials.json` is in exactly one of
   two places: `<root>/<name>/.credentials.json`, or
   `<root>/.auto-switch.d/parked/<name>/.credentials.json`.
   ```bash
   ls -l "$AMPLIHACK_CLAUDE_PROFILE_ROOT"/*/.credentials.json \
         "$AMPLIHACK_CLAUDE_PROFILE_ROOT"/.auto-switch.d/parked/*/.credentials.json
   ```
3. Use `<root>/.auto-switch.d/ledger.json` to see which account is in which
   directory. Move each login home with `mv -n`; never copy it.
4. Delete `ledger.json` and `journal.json`, then run
   `amplihack profile auto-switch repair` and
   `amplihack profile auto-switch status`, and check that `status` reports
   `invariant: ok`.

After a hand repair, `/status` may show the wrong account name for a profile
until its next launch or `amplihack profile login <name>`, because the name
is kept in `.claude.json` separately from the login.

If you cannot tell which account a file belongs to, run
`amplihack profile login <name>` for that profile and sign in again.

### Check it with two real accounts

The in-session swap relies on two things Claude Code is expected to do,
which have not yet been confirmed by a live test:
Claude reloads `.credentials.json` when its modification time changes, and
it re-reads the file after taking its own refresh lock
(`.oauth_refresh.lock`). Before relying on in-session swaps, check them once:

1. Set up two profiles with different logins, run `install`, and turn the
   switch on with a low threshold, for example `threshold 1`.
2. Start `amplihack claude --profile work` and send one prompt.
3. After the reply, `auto-switch status` shows `personal` as live in `work`.
4. Send another prompt. It succeeds, and `/status` (after a restart, if
   needed) names the `personal` account.
5. Leave the session idle for more than an hour so Claude refreshes the
   token, then send another prompt. It still succeeds.
6. Exit. `status` shows `hosted directories: none`, and
   `amplihack profile run personal` and `amplihack profile run work` both
   work without logging in again.

If any step fails, turn the switch off. Nested substitution and step reruns
do not depend on these two behaviours.

### Limits of auto-switch

- **In-session swaps need Linux.** A session is detected as running by
  looking for its `CLAUDE_CONFIG_DIR` in `/proc`. Without `/proc`, or when
  amplihack cannot read `/proc/self/environ` (for example with `hidepid`),
  swaps are disabled and `status` says so; nested substitution and step reruns still
  work.
- **A credentials file is required.** Logins kept in the macOS Keychain, or
  profiles without `.credentials.json`, are never swapped; `status` notes it.
- **Unix only.** On Windows `auto-switch on` is refused and nothing else
  happens.
- **Interactive sessions only, for in-session swaps.** The trigger is the
  status line. `claude -p` runs rely on nested substitution and step reruns.
- **`/logout` inside a swapped session** deletes the borrowed login. The
  owning profile has to log in again.
- **Helper processes keep a session alive.** A login returns home only when
  no process (including MCP servers Claude started) still has the directory
  in its environment.
- **Two profiles signed in to the same account** are not supported. An
  account is identified by the profile that owns its login, so they are
  treated as two accounts with separate readings; `status` warns about it.
- **`/status` can lag.** It may name the previous account after a swap
  until Claude rewrites `.claude.json`; see above.
- **Leases protect against accidents, not attacks.** Any process running as
  you, and root, can read or move these files.
- **Held-back step output is kept on disk briefly.** A supervised step's
  standard output is in a private file under `<root>/.auto-switch.d/rerun/`
  until the step finishes. If amplihack is killed first, the next recovery
  deletes it once that process is gone, and after 24 hours at the latest.
- **A rerun repeats the step.** Anything the failed attempt already did may
  happen twice.
- **File modes are kept.** Patching `.claude.json` or `settings.json` keeps
  the file's mode; a `settings.json` that `install` creates is `0600`.
- **Readings come only from sessions with the wrapper.** A profile that has
  never run a session since the switch was turned on has no reading.

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
- No token vault, token refresh or usage probing exists by design. Automatic
  switching is opt-in and only moves logins between your own profiles; see
  [Switch accounts automatically](#switch-accounts-automatically) and the
  [ccswitch review](../reviews/ccswitch-review.md).

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
