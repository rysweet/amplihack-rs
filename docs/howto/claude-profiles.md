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
Logins stay separate, but sessions are shared: every profile's `projects/`,
`file-history/` and `session-env/` is a link to the same directory in
`~/.claude`, so `claude --resume <id>` finds a session whatever profile
started it. Apart from those three directories, amplihack never modifies
`~/.claude` for this feature, and it makes no network calls. See
[What a new profile inherits](#what-a-new-profile-inherits) and
[Share sessions across profiles](#share-sessions-across-profiles).
To see which accounts have quota left, see
[Check usage across profiles](#check-usage-across-profiles). To move work to
another account automatically when one runs out of quota, see
[Switch accounts automatically](#switch-accounts-automatically). To keep the
default `~/.claude` login on the account with the most capacity, see
[Rotate Claude logins automatically](claude-profile-rotation.md).

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

Never copied: `.credentials.json`, `.claude.json`, `history*`,
`sessions/`, `todos/`, `statsig/`, `shell-snapshots/`, `ide/`, and anything else not
on the allowlist (so plugins and MCP server settings are not carried over), and the `env` and `apiKeyHelper` settings keys (and any other
key not on the allowlist). `projects/`, `file-history/` and `session-env/` are
not copied either: they are linked, see
[Share sessions across profiles](#share-sessions-across-profiles). If
`~/.claude` does not exist, the profile is simply created empty and gets no
links.
If the path exists but is not a directory, `profile add` fails and removes the
new profile directory rather than silently creating an unseeded profile.

Caveats:

- **Copy by value.** The allowlisted assets are copies, not links. Later
  edits to `~/.claude` do not propagate to existing profiles; re-add the
  profile to pick them up. The only links are the three session directories.
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

## Share sessions across profiles

Every profile uses the same session store, so a conversation started under
one account can be resumed under another. Logins are never shared.

| Shared (one copy, in `~/.claude`) | Per profile (never shared) |
|-----------------------------------|----------------------------|
| `projects/`: session transcripts (`<session-id>.jsonl`), grouped by working directory | `.credentials.json`, `.claude.json` (login and account) |
| `file-history/`: file snapshots, one directory per session id | `settings.json`, `CLAUDE.md` and the other seeded assets |
| `session-env/`: per-session environment, one directory per session id | `history*`, `todos/`, `statsig/`, `shell-snapshots/`, `ide/` |

`file-history/` and `session-env/` are shared along with `projects/` because
both are keyed by session id and `--resume` needs them: without them a
resumed session can't rewind file edits or restore its environment.

In each profile directory the three names are symbolic links:

```bash
ls -l ~/.amplihack/claude-profiles/work/
# lrwxrwxrwx ... file-history -> /home/you/.claude/file-history
# lrwxrwxrwx ... projects -> /home/you/.claude/projects
# lrwxrwxrwx ... session-env -> /home/you/.claude/session-env
```

### Resume a session from another profile

```bash
amplihack claude --profile work      # session 0f6c…e1 is written to ~/.claude/projects
amplihack claude --profile personal -- --resume 0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1
```

Plain `claude --resume` (no profile) sees the same sessions, because the
store is `~/.claude` itself.

### When the links are made

amplihack makes or checks the links at each of these points. When there is
nothing to do it prints nothing.

| Command | What happens |
|---------|--------------|
| `profile add <name>` | Links are created in the new profile. Nothing is printed unless something went wrong. |
| `profile switch <name>`, `profile login <name>` | The profile's links are checked and fixed: by `switch` after the pointer is written, by `login` before `claude` starts. |
| `amplihack claude` / `launch` with any profile (`--profile`, inherited, default pointer), `profile run` | Checked just before spawn, after the usual directory check. |
| `profile rotate` | After a swap that actually ran, for both profiles in it. `--dry-run` and refused rotates change nothing. |

Auto-switch makes no check of its own. Its swap moves only the login files,
and the rerun after a usage limit is an ordinary launch, so the launch row
covers it.

Problems are printed to stderr as warnings, for example
`amplihack: profile work: projects is a symlink to /mnt/old/projects; left alone`.
They never stop a launch, an `add`, a `switch`, a `login` or a rotate.

### Moving an existing profile's sessions into the shared store

Profiles made before sessions were shared have real `projects/`,
`file-history/` and `session-env/` directories. The first launch, `switch` or
`login` of such a profile merges them into `~/.claude` and replaces each with
a link. The merge runs before `claude` starts. It prints
`amplihack: profile work: merging session history into ~/.claude (one-time; may take a while)`
when it begins and the results when it finishes. When the profile and `~/.claude` are on different filesystems every
file is copied, so a profile with gigabytes of history can take minutes;
let it finish rather than interrupting it (an interrupted merge is safe and
resumes on the next launch, but it starts by re-checking every file).

```
amplihack: profile work: projects: moved 41 files into ~/.claude/projects, 2 identical, 1 conflict
amplihack: profile work: projects: conflict: -home-you-src-app/0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1.jsonl kept the shared copy; this profile's copy saved as ~/.claude/projects/-home-you-src-app/0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1.jsonl.conflict-work
amplihack: profile work: projects: old directory kept as ~/.amplihack/claude-profiles/work/projects.pre-share-20261020T141503Z
```

The merge never deletes or overwrites a session:

- **Only in the profile:** the file is moved into the shared store, keeping
  its modification time. Its mode is limited to the owner (a `0644`
  transcript becomes `0600`), because `~/.claude` may be readable by other
  users where the profile directory was not.
- **Same bytes in both:** the shared copy stays. The profile's copy is left
  behind and ends up in the backup.
- **Different contents:** the shared copy always keeps the real name. The
  profile's copy is moved beside it as `<file>.conflict-<profile>`, or
  `.conflict-<profile>-2`, `-3` and so on if that name is taken. The shared
  file is never renamed, so a session another profile has open is never
  split and its name never goes missing, even for a moment. If the
  profile's copy was the one you wanted, see
  [Resume the losing copy of a conflict](#resume-the-losing-copy-of-a-conflict).
- **Claude's memory index:** Claude reads per-project memory only through
  `projects/<project>/memory/MEMORY.md`, which lists the memory files beside
  it. When the profile's `MEMORY.md` differs from the shared one, the shared
  file keeps its name and the profile's copy is saved as
  `MEMORY.md.conflict-<profile>` like any conflict, and then every entry of
  the profile's index that the shared index lacks is appended to the shared
  `MEMORY.md` (headings and blank lines are left out; a link to a memory file
  that was itself saved under a conflict name points at that name). This
  append is the only change the merge ever makes to an existing shared file.
  amplihack prints
  `projects: memory index <project>/memory/MEMORY.md: added <n> entries from this profile to the shared copy`.
  Read the shared `MEMORY.md` afterwards and tidy any entries that now say
  the same thing twice.
- **Skipped and left in the backup:** credential files (`.credentials*`,
  `.claude.json`) at any depth, symbolic links, and special files.

When the merge finishes, the old directory is renamed to
`<name>.pre-share-<UTC time>` inside the profile, holding whatever was left
behind, and the link takes its place. The merge is safe to stop and re-run;
the next launch picks up where it stopped. A second run on a merged profile
changes nothing.

If the link is put in place but the old directory can't then be renamed to
its `.pre-share-` backup name, amplihack warns
`rename failed: ...; nothing removed, the old directory is at <profile>/.<name>.share-tmp`.
That warning is printed only once: on later runs `<name>` is a correct link
and the leftover isn't mentioned again. Check its contents, move anything you
still want into `~/.claude/<name>`, then remove it by hand. While it is still
there, a later swap of the same name refuses to run.

### Resume the losing copy of a conflict

The `.conflict-<profile>` suffix stops Claude from listing the file as a
second session. To resume that copy instead, end every session that has the
shared copy open, then put the losing copy under the session's own name and
keep the shared copy aside:

```bash
cd ~/.claude/projects/-home-you-src-app
mv 0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1.jsonl 0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1.jsonl.conflict-shared
mv 0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1.jsonl.conflict-work 0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1.jsonl
amplihack claude --profile work -- --resume 0f6c9b2a-4d1e-4c7a-9a55-3b8f2f7d10e1
```

### Delete the backups

Backups are never deleted automatically. They can hold full transcripts, so
once you have checked the merge, delete them rather than copying them
anywhere:

```bash
ls -d ~/.amplihack/claude-profiles/*/*.pre-share-*
rm -rf ~/.amplihack/claude-profiles/work/projects.pre-share-20261020T141503Z
```

`profile remove` deletes a profile's backups along with its directory. It
removes only the links, never the shared sessions they point to.

### A running session delays the merge

If a Claude session is running with the profile's directory, its real
directories are not merged; you see
`amplihack: profile work: projects: migration deferred: live session`.
If amplihack can't read the list of running processes (on Linux, `/proc`), it
also waits, and says
`projects: migration deferred: could not check for running sessions (retried next launch)`
instead.
Missing names still get links straight away. The merge happens on the first
launch, `switch` or `login` after every session using that profile has ended.

On systems other than Linux, amplihack can't see which sessions are running,
so it never merges automatically there. New profiles and missing names still
get links. While a profile still has a real directory, each launch, `switch`
and `login` of it prints
`amplihack: profile work: projects: not merged: running sessions can't be checked on this system; merge by hand, see docs/howto/claude-profiles.md#merge-by-hand-on-macos`
until you merge it as shown below.

On Windows the store is not shared and each profile keeps its own
directories. `profile add` prints
`shared session dirs not supported on this platform; profiles keep separate projects/`
once; other commands log it at debug level only.

### Merge by hand on macOS

End every session that uses the profile, then run:

```bash
p=~/.amplihack/claude-profiles/work
for d in projects file-history session-env; do
  [ -d "$p/$d" ] && [ ! -L "$p/$d" ] || continue      # only real directories
  rsync -a --no-links --ignore-existing \
    --exclude='.credentials*' --exclude='.claude.json' \
    "$p/$d/" "$HOME/.claude/$d/" &&
  mv "$p/$d" "$p/$d.pre-share-manual" &&
  ln -s "$HOME/.claude/$d" "$p/$d" ||
  { echo "stopped at $d; nothing was linked for it" >&2; break; }
done
```

- `--ignore-existing` never overwrites a shared file.
- `--no-links` (after `-a`) skips symbolic links, and the two `--exclude`
  options keep credential files out of the shared store.
- A file that exists in both places with different contents is not copied:
  the profile's copy stays only in `$d.pre-share-manual`. Run
  `diff -rq "$p/projects.pre-share-manual" ~/.claude/projects` to list
  them before you delete the backup.

If the system `rsync` rejects one of these options, install rsync 3 (for
example `brew install rsync`) and run the loop again.

### Privacy: every profile can read every session

With a shared store, any profile, and anyone using it, can read every
transcript and file snapshot from every other profile. Only the logins stay
separate. [Stop sharing for one profile](#stop-sharing-for-one-profile)
keeps that profile's new sessions out of the shared store, so other profiles
don't list them, but they are still files your OS user can read. If you need
an account whose sessions the others can't read, use a different OS user
rather than a profile.

All profiles also share one `session-env/`. If you keep accounts separate
for trust reasons, for example a client's account next to your own, create
`.no-shared-sessions` in that profile before its first launch after the
upgrade (see [Stop sharing for one profile](#stop-sharing-for-one-profile)).

### When nothing is linked

amplihack leaves a profile's directories as they are, with a warning, when:

- the profile has a `.no-shared-sessions` file (no warning; see
  [Stop sharing for one profile](#stop-sharing-for-one-profile));
- `$HOME` is unset, or `~/.claude` doesn't exist (no warning in this case;
  amplihack doesn't create `~/.claude`);
- `~/.claude` is itself a symlink, as in many dotfile setups (a warning is
  printed). Point the profiles at the real directory instead, or keep
  sharing off;
- `~/.claude` is not owned by you, or is writable by group or others (a
  group-writable directory is accepted when the group is your own private
  group, as with Ubuntu's default umask of `002`);
- the profile directory is not owned by you, or has any group or other
  permission bit (`mode & 0o077` is not zero, the same rule as the
  launch check);
- `~/.claude/projects` (or one of the other two) is a symlink, not a
  directory, not owned by you, or writable by group or others (same rule as
  `~/.claude`). Only that name is skipped;
- the profile directory is inside `~/.claude` (a warning is printed; move
  the profile root elsewhere to share sessions);
- the profile's `projects/` is already a symlink to somewhere else, or is a
  file rather than a directory. amplihack never follows or replaces it.

A missing `~/.claude/projects` (or `file-history/`, `session-env/`) is created
with mode `0700`.

### Stop sharing for one profile

A `.no-shared-sessions` file in the profile directory turns sharing off for
that profile. amplihack checks for it before anything else and, when it is
there, never links or merges that profile's directories. End the profile's
sessions, then create the file and replace each link with a real directory:

```bash
p=~/.amplihack/claude-profiles/work
touch "$p/.no-shared-sessions"
for d in projects file-history session-env; do
  [ -L "$p/$d" ] && rm "$p/$d"        # removes the link only, never the shared files
  [ -e "$p/$d" ] || mkdir -m 700 "$p/$d"
done
```

Sessions the profile wrote while it was shared stay in `~/.claude`. Copy back
the ones you want, or move a `*.pre-share-*` backup into place instead of the
`mkdir`; a backup holds only what the merge left behind.

To share again, delete `.no-shared-sessions`. The next launch, `switch` or
`login` merges the profile's directories into `~/.claude` as described in
[Moving an existing profile's sessions into the shared store](#moving-an-existing-profiles-sessions-into-the-shared-store).

## Create and log in

```bash
amplihack profile add work
amplihack profile login work      # interactive claude; complete login inside it
amplihack profile add personal
amplihack profile login personal
```

`add` copies the allowlisted shared assets into a private staging directory and
publishes `~/.amplihack/claude-profiles/work/` atomically after seeding succeeds
(mode `0700` on Unix); it fails if the profile already exists. The first `add`
also saves the profile root it used; see
[Choose where profiles live](#choose-where-profiles-live). `login`
always runs the `claude` binary (not the configured launch binary) with
`CLAUDE_CONFIG_DIR` pointing at the profile so the login is stored there.
If the new login is an account another profile already has, `login` refuses
it and puts the previous login back; see
[Duplicate logins](#duplicate-logins).

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
amplihack profile switch --none   # no default again
```

See [Clearing the default profile](#clearing-the-default-profile). If the
default is the profile the rotator has moved into `~/.claude`, see
[Launching the rotator's live profile](#launching-the-rotators-live-profile).

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

## Clearing the default profile

```bash
amplihack profile switch --none
# Default profile cleared.
amplihack profile show
# none
```

After this, `amplihack claude` runs with no profile and leaves
`CLAUDE_CONFIG_DIR` alone. If no default was set, the command prints
`No default profile was set.` and still exits 0. `--none` cannot be combined
with a name, and `switch` with neither is a usage error (exit 2).

There is no `switch default` form: `default` is a valid profile name, so
`amplihack profile switch default` selects a profile called `default`.

## Launching the rotator's live profile

When [`amplihack profile rotate`](claude-profile-rotation.md) has moved a
profile's login into `~/.claude`, that profile's own directory has no login.
Launching Claude in that directory would ask you to log in, and logging in
there would create a [duplicate login](#duplicate-logins).

So when the profile a launch resolves to is the rotator's live profile,
`amplihack claude` and `amplihack launch` run against `~/.claude` instead,
with `CLAUDE_CONFIG_DIR` unset and no profile pinned. This applies whether
the profile came from `--profile`, an inherited `AMPLIHACK_CLAUDE_PROFILE`,
or the default pointer. One line goes to stderr:

```
note: profile 'team' is the rotator's live login, which is in ~/.claude; launching against ~/.claude.
```

The session uses the right account because `~/.claude` is where that
account's login is. Because no profile is pinned, nested launches inside
the session also run against `~/.claude`.

The live profile is the one shown as `live in ~/.claude` by
[`profile usage`](#a-profile-shows-live-in-claude). If amplihack cannot tell
which profile is live (no ledger and no single account match, or Windows or
another platform without Unix file semantics), it launches the profile directory as before and prints
nothing. Docker and `--auto` launches are not affected.

`amplihack profile run <name>` does not redirect; it always uses the
profile's own directory. Use `amplihack claude --profile <name>` for the
live profile.

## Choose where profiles live

Profiles live under one *profile root*. amplihack picks it in this order:

1. `AMPLIHACK_CLAUDE_PROFILE_ROOT`, if set and non-empty.
2. The root saved in `~/.amplihack/profile-root`.
3. `~/.amplihack/claude-profiles`.

Save a root once and every shell finds it, whether or not it reads
`~/.bashrc`:

```bash
amplihack profile root /home/you/.claude-profiles
```

```
saved profile root: /home/you/.claude-profiles (/home/you/.amplihack/profile-root)
```

Check which root is in use and where it came from:

```bash
amplihack profile root
```

```
root: /home/you/.claude-profiles (source: config)
```

`amplihack profile list` prints the same line first.

You may not need to run `profile root` at all. The first `amplihack profile
add` saves the root it used when nothing is saved yet, so if you export
`AMPLIHACK_CLAUDE_PROFILE_ROOT` in `~/.bashrc`, your first `add` records that
path:

```
recorded profile root /home/you/.claude-profiles in /home/you/.amplihack/profile-root
```

Later `add`s never change it.

If you created profiles before this feature existed, save the root once from a
shell that has the variable:

```bash
amplihack profile root "$AMPLIHACK_CLAUDE_PROFILE_ROOT"
```

The variable still takes precedence while it is set and non-empty; an empty
value counts as unset. When it is set, `profile root <dir>` saves anyway and
adds a note on stderr:

```
note: AMPLIHACK_CLAUDE_PROFILE_ROOT=/tmp/scratch overrides the saved root in this shell
```

To go back to the default, delete `~/.amplihack/profile-root`. Changing the
root does not move existing profiles.

`profile root <dir>` refuses a relative or empty path, a path with a `..`
component (the variable is refused for the same reasons), a symlink, and a path inside `~/.claude`, and saves nothing when it
refuses. A quoted `'~/x'` is relative; let the shell expand `~` instead. The
directory does not have to exist yet; `profile add` creates it.

### Use profiles from non-interactive shells

cron, systemd, `tmux new-session -d '<cmd>'`, `ssh host cmd` and `env -i` do
not read `~/.bashrc`, so a root exported there is missing. With a saved root
they need no extra setup:

```bash
env -i HOME="$HOME" PATH="$PATH" amplihack profile list
```

```
root: /home/you/.claude-profiles (source: config)
* personal
  work
```

```cron
*/30 * * * * amplihack profile usage --json > /tmp/claude-usage.json
```

```bash
tmux new-session -d -s work 'amplihack claude --profile work'
```

If a command reports `source: default` where you expected your own root, no
root is saved yet; run `amplihack profile root <dir>`.

A launch exports `AMPLIHACK_CLAUDE_PROFILE_ROOT` to its children only when it
was set and non-empty. Otherwise nested launches read `~/.amplihack/profile-root`
themselves, so changing the saved root while a session runs moves that
session's later nested launches to the new root. Set the variable before
starting a session if it must stay on one root. The background rotator is
always pinned to the root it started with; see
[Which root the rotator uses](claude-profile-rotation.md#which-root-the-rotator-uses).

### Repair a damaged saved root

If `~/.amplihack/profile-root` is empty, relative, a symlink, writable by
others or otherwise invalid, every command that needs the root fails rather
than quietly using another one:

```
error: saved profile root in "/home/you/.amplihack/profile-root" is invalid: not an absolute path
hint: fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>
```

Run `amplihack profile root <absolute dir>` to replace it; it never reads the
damaged file. Deleting the file returns you to the default. Setting
`AMPLIHACK_CLAUDE_PROFILE_ROOT` also works, because the file is not read while
the variable is set and non-empty.

If the file was written by `sudo` and is owned by root, remove it with
`sudo rm ~/.amplihack/profile-root` first.

See the [profile root reference](../reference/profile-command.md#profile-root)
for every rule.

## List and remove

```bash
amplihack profile list
amplihack profile remove personal --yes
```

`list` starts with the root in use, for example
`root: /home/you/.claude-profiles (source: config)`, then one profile per
line with the default marked `*`.

`remove` deletes only the verified profile directory, requires `--yes`, and
clears the default pointer if it named that profile. This deletes that
account's local Claude login, its history and any `*.pre-share-*` backups.
The shared session links are removed without being followed, so sessions in
`~/.claude/projects`, `file-history/` and `session-env/` are kept.

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

## Check usage across profiles

To see which accounts have quota left, run:

```bash
amplihack profile usage
```

```
PROFILE   MARKS             STATE              SESSION  SESSION RESETS                        WEEK  WEEK RESETS
review    -                 ok                 5%       Oct 16, 9pm (America/Los_Angeles)     10%   Oct 22, 2am (America/Los_Angeles)
work      *                 ok                 20%      Oct 16, 7pm (America/Los_Angeles)     30%   Oct 22, 2am (America/Los_Angeles)
team      -                 session_exhausted  100%     Oct 16, 4:50pm (America/Los_Angeles)  68%   Oct 22, 2am (America/Los_Angeles)
scratch   -                 not_logged_in      -        -                                     -     -
```

amplihack runs `claude -p /usage` under every profile at once, with
`CLAUDE_CONFIG_DIR` set to that profile, and reads the `Current session` and
`Current week` lines. Checking does not appear to use quota, and an
exhausted account still answers. The profiles with the most room come first. Exhausted ones
follow, soonest back first, and the reset column says when. Profiles that
could not be read come last. `*` marks the default.

To check only some profiles, name them:

```bash
amplihack profile usage work team
```

What the `STATE` column means:

| State | What to do |
|-------|------------|
| `ok` | Ready to use. |
| `session_exhausted` / `weekly_exhausted` | Wait for the reset shown, or use another profile. |
| `spend_limit` | Your organisation's spend limit was reached. |
| `not_logged_in` | `amplihack profile login <name>` |
| `timeout` | Claude did not answer in time (30 seconds by default). Try again. |
| `unparsed` | Claude printed something unexpected. Run with `--raw` to see it. |
| `error` | The probe could not run (the profile directory was refused, or `claude` could not be started, read or waited for); the reason is printed on stderr. |

Claude's own text is shown only when you ask for it, for example to report
output amplihack does not recognise:

```bash
amplihack profile usage --raw scratch
```

`--raw` text can include your e-mail address or organisation name. Read it
before you share it.

For scripts, use `--json`. This picks the profile with the most capacity:

```bash
amplihack profile usage --json | jq -r '[.[] | select(.state == "ok")][0].profile'
```

```
review
```

Each JSON row also has the per-model week lines (`week_models`) and each
reset as Unix time (`resets_at`); see the
[reference](../reference/profile-command.md#json-output).

If [auto-switch](#switch-accounts-automatically) is set up, `profile usage`
also refreshes its readings, so it can choose accounts using numbers taken
now instead of the last status-line update. While auto-switch has state
(`<root>/.auto-switch.d/`), the table also shows where logins are: `leased→work` on a profile whose login is lent
to `work`'s session, and `hosts:personal` on the profile holding it. The
numbers on a `hosts:` row belong to the hosted account. See
[Refreshing auto-switch readings](../reference/profile-command.md#refreshing-auto-switch-readings)
for when a reading is not recorded.

The command exits with status 0 whenever the probes ran, whatever their
states. Like `profile run`, it is refused under Docker.

### A profile shows `live in ~/.claude`

If you use a login rotator that moves one account's login into the default
`~/.claude` directory, such as
[`amplihack profile rotate`](claude-profile-rotation.md), that profile's own directory has no login while it is
there. `profile usage` checks that account through `~/.claude` instead and
marks the row:

```
PROFILE   MARKS              STATE  SESSION  SESSION RESETS                     WEEK  WEEK RESETS
team      live in ~/.claude  ok     40%      Oct 16, 8pm (America/Los_Angeles)  55%   Oct 22, 2am (America/Los_Angeles)
```

The numbers on that row are the account's real usage, and the row is not
`not_logged_in`. In the JSON the row has `"live_in": "~/.claude"`; every
other row has `null`:

```bash
amplihack profile usage --json | jq -r '.[] | select(.live_in != null) | .profile'
```

```
team
```

amplihack finds the profile from a rotator's ledger:
`<root>/.auto-switch.d/rotate/ledger.json` for `profile rotate`, or
`<root>/.rotator/ledger.json` for an external rotator such as the tmux
prototype. If both exist, the one updated more recently decides, so moving
from one rotator to the other needs no clean-up. Without a ledger it looks
for the one profile with no login whose account matches the account in
`~/.claude.json`; if you do not use a rotator, there is no ledger and nothing
is printed. `profile usage` only reads the rotators' files and never changes
them.

This needs Linux. On macOS Claude keeps the login in the Keychain rather
than in `~/.claude/.credentials.json`, so no profile is ever shown as
`live in ~/.claude`; on Windows the mark is never shown either.

The live profile's check runs with `CLAUDE_CONFIG_DIR` unset rather than set
to `~/.claude`. With the variable set, `claude` would look for
`~/.claude/.claude.json` instead of the real `~/.claude.json` and could read
the wrong account.

When the evidence about which profile is live is unreliable or conflicting,
amplihack marks none and prints one warning line, for example:

```
warning: the login rotator's ledger names a profile that still has its own login; it is not marked live in ~/.claude
```

That one means the ledger is out of date: the named profile still has its
own login, so amplihack checks it through its own directory. The other
warnings are listed in
[Warnings about `~/.claude`](../reference/profile-command.md#warnings-about-claude).

`live in ~/.claude` is not the same as `*`: `*` is the profile
`amplihack claude` uses by default.

If the rotator swaps logins while the check runs, the row's numbers may
belong to the new login. amplihack says so on stderr and does not record
them for auto-switch. Run the command again for current numbers. This check
cannot see a profile that became live after the command started, or a swap
away and back again when the ledger has no `since` time. See
[Logins in `~/.claude`](../reference/profile-command.md#logins-in-claude)
for the exact rules and warnings.

## Accounts, organizations and duplicate logins

### Which logins count as the same account

One Claude user can belong to several organizations, for example a team
organization and a personal one. Each organization has its own subscription
and its own usage limits, so a profile per organization is a normal setup.

amplihack reads two values from the `oauthAccount` block of each
`.claude.json`: `accountUuid` (the user) and `organizationUuid` (the
organization). Two logins are the same account only when both match:

| Login A | Login B | Same account? |
|---------|---------|---------------|
| user U, org O1 | user U, org O1 | yes |
| user U, org O1 | user U, org O2 | no |
| user U, org O1 | user U, no org | no |
| user U, no org | user U, no org | yes |
| different users | (any) | no |
| no user on either side | — | compared by e-mail address, ignoring case |

A missing value never makes two logins look the same. This one rule is used
by `profile login`, `profile usage`, `auto-switch status`, auto-switch swaps
and the rotator. No credentials file is read to decide it, and the values
are never printed.

For example, profiles `ryan-mistt-org` and `ryan-mistt-personal` share a user
but have different organizations. They are two accounts, and no command
warns about them.

### Duplicate logins

Claude's login opens claude.ai in your browser and signs in whichever
account the browser is already signed in to. If that account is already in
another profile, you get two profiles with the same login.

`amplihack profile login` checks for this after Claude exits. It compares the
new login with every other profile and with the login in `~/.claude`. If it
matches one, the login is refused:

```bash
amplihack profile login rssourcelabs
```

```
note: holding the previous login of profile 'rssourcelabs' in /home/ryan/.amplihack/claude-profiles/.rejected-logins/.pending-rssourcelabs-20261020T171502Z while you log in
...interactive claude session...
note: moving the new login of profile 'rssourcelabs' to /home/ryan/.amplihack/claude-profiles/.rejected-logins/rssourcelabs-20261020T171502Z
error: profile 'rssourcelabs' was signed in to the same account and organization as profile 'ryan-ryansweet'. The new login was moved to /home/ryan/.amplihack/claude-profiles/.rejected-logins/rssourcelabs-20261020T171502Z; 'rssourcelabs' is back to its previous login.
hint: sign out of claude.ai in your browser (or use a private/incognito window), then run 'amplihack profile login rssourcelabs' again.
```

The command exits with status 1. The profile has the login it had before
(or no login, if it had none), and its `oauthAccount` block is restored.
Other `.claude.json` keys Claude changed during the login stay as Claude
left them. The refused login is moved, not deleted.

To fix it, sign out of claude.ai in the browser, or open a private window,
and run `amplihack profile login rssourcelabs` again. Sign in to the account
this profile is meant for.

If the account is the one in `~/.claude`, the holder is named
`~/.claude (rotator live profile 'team')` when the rotator's live profile is
known, otherwise just `~/.claude`.

The rotator's live profile cannot be logged in to at all: its login is the
one in `~/.claude`, and the rotator refuses to swap it back into a profile
directory that has a login of its own. `login` refuses before Claude starts
and moves nothing:

```
error: profile 'team' is the rotator's live profile; its login is ~/.claude. Nothing was moved.
hint: log in with plain 'claude', or stop the rotator first.
```

When the login is accepted and the profile had a previous login, that login
is kept too:

```
note: the previous login of profile 'work' was moved to /home/ryan/.amplihack/claude-profiles/.rejected-logins/work-20261020T171502Z-replaced
```

If Claude exits with an error, or you press Ctrl-C, the previous login is
put back, Claude's exit status is kept, and no check is made. A login Claude
wrote anyway goes to `.rejected-logins/` like a refused one.

#### What is in `.rejected-logins`

```
<root>/.rejected-logins/                         0700
├── .pending-<name>-<time>/                       the previous login while Claude runs
├── <name>-<time>/                                a refused login
│   ├── .credentials.json                         0600, moved, not copied
│   └── oauthAccount.json                         its oauthAccount block, 0600
└── <name>-<time>-replaced/                       a previous login replaced by an accepted one
```

`<time>` is UTC, for example `20261020T171502Z`. amplihack never deletes
anything here. Each login file is in one place only: the profile directory
or one of these directories. To use a kept login again, move it back with
`mv -n` into a profile that has no `.credentials.json`, never `cp`.

If amplihack itself is killed while Claude runs, the previous login stays in
the `.pending-…` directory printed at the start. Ctrl-C is handled, but
closing the terminal (SIGHUP) or `kill` (SIGTERM) ends amplihack before it
can restore anything. The previous login is then at
`<root>/.rejected-logins/.pending-<name>-<time>/.credentials.json`, and its
`oauthAccount` block is next to it in `oauthAccount.json`. If Claude wrote a
new login, move that one aside first (or delete it if you do not want it),
then move the previous one back into the profile directory with one `mv`.
This example uses the default root; if you chose another root (`amplihack
profile root` prints it), use that path instead:

```bash
mv -n ~/.amplihack/claude-profiles/.rejected-logins/.pending-rssourcelabs-20261020T171502Z/.credentials.json \
      ~/.amplihack/claude-profiles/rssourcelabs/
```

#### When the check cannot move the login

- **macOS.** Claude keeps the login in the Keychain, not in
  `.credentials.json`. A duplicate is still reported and `login` exits 1,
  but there is no file to move aside. The profile's `oauthAccount` block is
  left as Claude wrote it, so `.claude.json` still matches the login in the
  Keychain. Sign out in the browser and log in again.
- **No login file after a previous one.** If the profile had a
  `.credentials.json` and Claude exits 0 without writing a new one, the
  login did not finish: the previous login and its `oauthAccount` block are
  both put back, and `login` exits 1.
- **The profiles cannot be read.** If the profile root cannot be listed or
  the auto-switch state cannot be opened, the new login cannot be compared,
  so it is treated like a refused one: moved to `.rejected-logins/`, the
  previous login put back, exit 1.
- **The rotator swapped the profile in during the login.** `profile rotate`
  takes no lease. If it made this profile the live one while Claude ran,
  amplihack stops without moving anything and prints where each login is.
- **A login file that is not private.** If the new `.credentials.json` is
  not a private regular file (for example mode `0644`, or a symlink),
  amplihack does not move it or change its mode. It exits 1, reports the
  mode, and leaves the previous login in the `.pending-…` directory, whose
  path the hint prints. Treat the new login as exposed: log in again.
- **A session started using the profile during the login.** If auto-switch
  lent the profile's login to a session or the profile now hosts another
  account, amplihack stops without moving anything and prints where each
  login is.

#### `/login` inside a running session

Running `/login` inside a Claude session is not checked when it happens.
`amplihack profile usage` and `amplihack profile auto-switch status` find the
duplicate afterwards:

```
note: profiles 'rssourcelabs' and 'ryan-ryansweet' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login ryan-ryansweet' with the browser signed out of claude.ai or in a private window.
```

Profile names are listed in sorted order and the fix names the second one.
If the second profile is the one with the right login, log in the first one
again instead.

`status` prints it as a `note:` line; `usage` prints the same text on stderr
as a `warning:` line, after the table. The login in `~/.claude` takes part
too, named `~/.claude`. The rotator's live profile is not compared with
`~/.claude`, since that is where its own login is.

## Switch accounts automatically

When one account is close to its five-hour or seven-day usage limit,
auto-switch moves work onto another of your profiles that has quota left.
It is **off by default**. Auto-switch acts on sessions started under a
profile; to keep the default `~/.claude` login used by plain `claude` on the
account with the most capacity, use
[`profile rotate`](claude-profile-rotation.md), which shares this toggle. With it off, and before it has ever been turned on,
amplihack behaves exactly as described in the rest of this guide.

Auto-switch does three things:

| Where | What happens |
|-------|--------------|
| A running Claude session | When the session's account reaches the threshold, the login in that session's configuration directory is swapped for the account with the most quota left. The session keeps running. |
| Nested launches (recipe agent steps, `multitask`/orch workstreams) | A launch that would inherit an exhausted profile starts under a profile with quota left instead. |
| A recipe agent step that fails with a usage limit | The step is run again under the next profile with quota left instead of ending the run. |

amplihack learns how much quota each account has used from the status line
Claude Code already computes, and from
[`profile usage`](#check-usage-across-profiles) when you run it. Auto-switch
sends no requests of its own, makes no network calls, and never reads,
parses, copies or refreshes a token.

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

With auto-switch on, `amplihack claude`, `amplihack launch` and
`amplihack profile run` also start the
[login rotator](claude-profile-rotation.md#start-the-rotator-automatically)
for `~/.claude` if it is not running, and print one line when they do. To
keep auto-switch without that, run `amplihack profile rotate autostart off`.

### 3. Check what it sees

```bash
amplihack profile auto-switch status
```

```
root: /home/ryan/.claude-profiles
switch: on (file)
autostart: on (default)
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
account and organization, `status` prints a duplicate-login note with the
fix; see [Duplicate logins](#duplicate-logins). Two profiles for the same
user in different organizations are separate accounts and get no note.

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
The recipe runner's heartbeat therefore reports such a step as idle until
it ends; that is expected, and nothing is stopped because of it.
If the step is stopped (SIGINT, SIGTERM or SIGHUP), it is not rerun: the
attempt's output is passed back and amplihack exits with `128` plus the
signal number.
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
- **Two profiles signed in to the same account and organization** are not
  supported. An account is identified by the profile that owns its login,
  so they are treated as two accounts with separate readings; `status`
  warns about it. See [Duplicate logins](#duplicate-logins).
- **`/status` can lag.** It may name the previous account after a swap
  until Claude rewrites `.claude.json`; see above.
- **Leases protect against accidents, not attacks.** Any process running as
  you, and root, can read or move these files.
- **Profiles do not isolate accounts from each other.** Any process running
  as you, including an agent following injected instructions, can feed the
  status-line wrapper a fake usage reading and so pull another profile's
  login into its session. It could already read that login file directly,
  so this adds no new exposure.
- **Held-back step output is kept on disk briefly.** A supervised step's
  standard output is in a private file under `<root>/.auto-switch.d/rerun/`
  until the step finishes. If amplihack is killed first, the next recovery
  deletes it once that process is gone, and after 24 hours at the latest.
- **A rerun repeats the step.** Anything the failed attempt already did may
  happen twice.
- **File modes are kept.** Patching `.claude.json` or `settings.json` keeps
  the file's mode; a `settings.json` that `install` creates is `0600`.
- **Readings come only from sessions with the wrapper and from
  `profile usage`.** A profile that has done neither since the switch was
  turned on has no reading. Run `amplihack profile usage` to fill them in.

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
- `profile run <name>` with the rotator's live profile still runs in the
  profile's empty directory; only `amplihack claude` and `amplihack launch`
  [redirect to `~/.claude`](#launching-the-rotators-live-profile).
- A redirected launch skips the auto-switch launch checks, as a launch with
  no profile does.
- Profiles are not a security boundary against other processes of the same OS
  user. Anyone able to read your home directory can read the profile. Every
  profile can read every other profile's sessions through the shared store;
  see [Privacy](#privacy-every-profile-can-read-every-session).
- Two profiles writing the same session at once behave as two `claude`
  processes writing it under `~/.claude` without profiles do. Session ids are
  UUIDs, so separate sessions never collide.
- Claude Code is expected to follow the `projects/` link. If a Claude version
  replaces the link with a real directory, the next launch merges it back.
- A first launch that merges a large `projects/` held on another filesystem
  copies every file, so it can take a while. One line is printed when it
  starts and the results when it finishes; free space is checked before each file, and it happens once.
- No token vault or token refresh exists by design. Usage is checked only
  when you run `amplihack profile usage`, which asks Claude itself. Automatic
  switching is opt-in and only moves logins between your own profiles; see
  [Switch accounts automatically](#switch-accounts-automatically) and the
  [ccswitch review](../reviews/ccswitch-review.md).

## Testing without touching real state

```bash
export HOME="$(mktemp -d)"
export AMPLIHACK_CLAUDE_PROFILE_ROOT="$HOME/profiles"
amplihack profile add demo
d="$(mktemp -d)"
printf '#!/bin/sh\necho "$CLAUDE_CONFIG_DIR"\n' > "$d/claude" && chmod +x "$d/claude"
PATH="$d:$PATH" amplihack profile run demo
```

The fake `claude` prints `$CLAUDE_CONFIG_DIR`, which should be the demo profile
directory. The temporary `HOME` keeps the saved root
(`$HOME/.amplihack/profile-root`, written by the first `add`), the seeding
source and the shared session store (`$HOME/.claude`) away from your real
files. The temporary `$HOME/.claude` doesn't exist, so `demo` gets no links;
run `mkdir -m 700 "$HOME/.claude"` before `profile add` to see them made.
