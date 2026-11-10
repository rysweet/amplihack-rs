# Statusline Reference

Real-time session information bar displayed at the bottom of Claude Code interface.

## Overview

The statusline shows progress, costs, context usage, and active features for your Claude Code session. When a login rotator manages `~/.claude`, a second line shows the live login and its capacity.

## Indicators Reference

| Indicator             | Shows                     | Format                                      | Notes                                                    |
| --------------------- | ------------------------- | ------------------------------------------- | -------------------------------------------------------- |
| **Directory**         | Current working directory | `~/path`                                    | `~` = home directory                                     |
| **Git Branch**        | Branch name and status    | `(branch → remote)` or `(branch* → remote)` | `*` = uncommitted changes, Cyan = clean, Yellow = dirty  |
| **Repository URI**    | Git remote repository URL | `[github.com/user/repo]`                    | Shortened format (cyan), only if remote exists           |
| **Model**             | Active Claude model       | `Opus`, `Sonnet`, `Haiku`                   | Red=Opus, Green=Sonnet, Blue=Haiku                       |
| **Tokens** 🎫         | Total token usage         | `234K`, `1.2M`, or raw number               | M=millions, K=thousands; see [Token count](#token-count) |
| **Cost** 💰           | Total session cost        | `$1.23`                                     | USD                                                      |
| **Duration** ⏱        | Session elapsed time      | `15m`, `1h`, `30s`                          | s/m/h format                                             |
| **Power-Steering** 🚦 | Redirect count            | `🚦×3`                                      | Only when active (purple)                                |
| **Lock Mode** 🔒      | Lock invocation count     | `🔒×5`                                      | Only when active (yellow)                                |

## Color Coding

### Git Status

- **Cyan**: Clean working tree (no uncommitted changes)
- **Yellow with `*`**: Dirty working tree (uncommitted changes)

### Model Type

- **Red**: Opus models
- **Green**: Sonnet models
- **Blue**: Haiku models
- **Gray**: Unknown/other models

### Feature Indicators

- **Purple (🚦)**: Power-steering active
- **Yellow (🔒)**: Lock mode active

## Examples

### Example 1: Clean Development Session

```
~/src/amplihack4 (main → origin) [github.com/rysweet/amplihack] Sonnet 🎫 234K 💰$1.23 ⏱12m
```

**Breakdown:**

- **Directory**: `~/src/amplihack4` (~= home shorthand)
- **Git**: `(main → origin)` cyan = clean branch
- **Repository**: `[github.com/rysweet/amplihack]` cyan = repository URL
- **Model**: `Sonnet` green = Sonnet family
- **Tokens**: `🎫 234K` 234,000 tokens
- **Cost**: `💰$1.23` $1.23 USD
- **Duration**: `⏱12m` 12 minutes

### Example 2: Active Development with Features

```
~/projects/api (feature/auth* → origin) [github.com/org/api-service] Opus 🎫 1.2M 💰$15.67 ⏱1h 🚦×3 🔒×5
```

**Breakdown:**

- **Directory**: `~/projects/api`
- **Git**: `(feature/auth* → origin)` yellow = dirty, `*` = uncommitted changes
- **Repository**: `[github.com/org/api-service]` cyan = repository URL
- **Model**: `Opus` red = Opus family
- **Tokens**: `🎫 1.2M` 1.2 million tokens
- **Cost**: `💰$15.67` $15.67 USD
- **Duration**: `⏱1h` 1 hour
- **Power-Steering**: `🚦×3` 3 redirects (purple indicator)
- **Lock Mode**: `🔒×5` 5 lock invocations (yellow indicator)

## Configuration

`amplihack install` with global scope sets the `statusLine` entry in
`~/.claude/settings.json` to the amplihack command:

```json
{
  "statusLine": {
    "type": "command",
    "command": "'/usr/local/bin/amplihack' profile statusline"
  }
}
```

That one command prints the session line from
`~/.amplihack/.claude/tools/statusline.sh` described above, then the
[login capacity line](#login-capacity-line). See
[Install wiring](#install-wiring) for when the entry is written, updated or
kept.

## Token count

The `🎫` segment shows the sum of four usage fields over the session
transcript that Claude Code passes in `transcript_path`. The function
`calculate_tokens()` in `amplifier-bundle/tools/statusline.sh` computes it.
It reads each transcript incrementally: on every refresh it reads only the
bytes appended since the previous refresh, and keeps a running total in a
small per-transcript cache file.

### What is counted

For each of these keys, every match of
`"KEY"[[:space:]]*:[[:space:]]*[0-9]*` adds its trailing digit run to the
total. An empty digit run counts as 0.

| Key                           | Meaning                       |
| ----------------------------- | ----------------------------- |
| `input_tokens`                | Uncached input tokens         |
| `output_tokens`               | Output tokens                 |
| `cache_read_input_tokens`     | Input tokens read from cache  |
| `cache_creation_input_tokens` | Input tokens written to cache |

The leading quote in each pattern stops `"input_tokens"` from matching inside
the two `cache_*_input_tokens` keys. All four keys are matched in one `awk`
pass under `LC_ALL=C`.

Only complete lines are counted. A last line with no trailing newline is left
out until Claude Code finishes writing it, and is then counted once. The file
size is read once with `stat` at the start of each call, so bytes written
while a call is running are left for the next call.

### Function contract

```bash
calculate_tokens "$transcript_path"
```

- Prints a single decimal integer and a newline. The output is always digits
  only, because `format_tokens` places the value inside an `awk` program.
- Writes nothing to stderr and always returns status 0.
- Prints `0` and writes no cache entry when the path is unset, empty, not a
  regular file, or a file of size 0.
- Totals above 2^32 are printed exactly. `awk` prints the token sum of the
  new bytes as `ADD BYTES`, and the shell adds `ADD` to the cached total with
  `$(( ))`, which is 64-bit in bash.

`format_tokens` and the rest of the status line are unchanged: `1234567`
still displays as `🎫 1.2M` and `234000` as `🎫 234K`.

### Cache location and format

| Item           | Value                                                          |
| -------------- | -------------------------------------------------------------- |
| Directory      | `${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/statusline-tokens/` |
| Directory mode | `0700`, created on first use                                   |
| Entry name     | Hex SHA-256 of the transcript path string                      |
| Entry mode     | `0600`                                                         |
| Entry content  | One line: `INODE OFFSET TOTAL`                                 |

The entry name comes from `sha256sum`, then `shasum -a 256` if that is
missing. On a host with neither, `cksum` is used and the name has the form
`digits-digits`. The path is passed to the hash on stdin, and neither the
path nor any transcript text is written to the cache.

`INODE OFFSET TOTAL` are three decimal integers separated by single spaces:

- `INODE` is the transcript's inode number, from `stat -L`.
- `OFFSET` is the byte offset just after the last complete line counted.
- `TOTAL` is the running token total up to `OFFSET`.

Example, where `$t` is the transcript path, for a transcript of 482113 bytes
whose complete lines sum to 1234567 tokens:

```console
$ key=$(printf '%s' "$t" | sha256sum | cut -d' ' -f1)
$ cat "${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/statusline-tokens/$key"
2753091 482113 1234567
```

Each number must be `0` or a digit run with no leading zero, of at most 18
digits. Values are checked against a `case` pattern before any arithmetic or
comparison, so a cache field such as `a[$(cmd)]` is never evaluated.

### When the count starts again from zero

The function discards the cached offset and total, and counts the transcript
from byte 0, in these cases:

| Condition                                                                         | Cause                                |
| --------------------------------------------------------------------------------- | ------------------------------------ |
| No cache entry exists                                                             | First refresh for this transcript    |
| The entry is unreadable, has other than three fields, or a field fails validation | Corrupt or hand-edited entry         |
| The inode differs from `INODE`                                                    | File replaced, for example by rename |
| The file size is below `OFFSET`                                                   | File truncated or rewritten in place |

When the file size equals `OFFSET`, the cached total is printed without
reading the transcript.

### Writes and failure handling

The new entry is written to `"$dir/.$key.$$"` inside a subshell with
`umask 077` and `set -C`, then moved over `"$dir/$key"` with `mv -f`. `set -C`
opens the temporary file with `O_EXCL`, so a planted symlink makes the write
fail instead of being followed. If two panes refresh the same transcript at
once, the last `mv` wins, and each entry stays self-consistent.

The cache is used only when the directory is a directory, not a symlink, and
owned by the current user, and an entry is read only when it is a regular,
user-owned file that is not a symlink. At most 128 bytes of an entry are
read. If any check fails, or `stat` gives no usable inode and size, the
function counts the whole file in one pass and writes nothing.

If the write or the `mv` fails, the function removes any temporary file that
this call created, prints the correct total and shows no error. A temporary
file that the call did not create is left in place: for example a stale or
planted `.$key.$$` that makes the `set -C` write fail, or one left by a
refresh that was killed part-way through.

If `mkdir` fails, no temporary file is created. The function counts the whole
file in one pass, prints the total and writes nothing.

### Platform support

| Need           | Used                                                                |
| -------------- | ------------------------------------------------------------------- |
| Inode and size | `stat -L -c '%i %s'`, else `stat -L -f '%i %z'` (BSD, macOS)        |
| Byte slice     | `tail -c +N \| head -c M`                                           |
| Counting       | POSIX `awk` with `[[:space:]]` (gawk, mawk 1.3.4, BSD awk, busybox) |
| Hash           | `sha256sum`, `shasum -a 256` or `cksum`                             |

No `jq`, `python` or `dd` is needed.

### Limits

- The first refresh of a transcript still reads the whole file, once, in a
  single pass.
- An inode reused by a new file that has already grown past the cached
  offset is not detected. The only effect is a wrong figure in the display.
- Cache entries are about 40 bytes each and are not removed. Deleting the
  directory is safe; the next refresh recounts each transcript from zero:

  ```bash
  rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/statusline-tokens"
  ```

- mawk 1.3.3, the default `awk` on some older Debian and Ubuntu releases,
  does not support `[[:space:]]` as a character class. On such a host the
  compact form `"input_tokens":123` does not match and the total shows as
  `0`. This is an accepted risk.
- The Rust status-line path and `docs/claude/tools/statusline.sh` do not use
  this cache.

### Checking the count by hand

The total for a transcript can be checked against a full recount over its
complete lines. The newest transcript is usually still being written, so the
example copies it first and runs the installed function with a temporary
cache directory. Running the function with the default cache directory
writes a real cache entry for the path it is given.

```bash
# Copy of the most recently written transcript, and a temporary cache
work=$(mktemp -d)
cp -- "$(ls -t ~/.claude/projects/*/*.jsonl | head -n 1)" "$work/session.jsonl"
t=$work/session.jsonl

# Bytes up to and including the last newline
size=$(stat -L -c %s -- "$t" 2>/dev/null || stat -L -f %z -- "$t")
if [ "$(tail -c 1 -- "$t" | wc -l)" -eq 0 ]; then
  size=$((size - $(tail -n 1 -- "$t" | wc -c)))
fi

# Full recount over complete lines
LC_ALL=C head -c "$size" -- "$t" \
  | grep -oE '"(input_tokens|output_tokens|cache_read_input_tokens|cache_creation_input_tokens)"[[:space:]]*:[[:space:]]*[0-9]*' \
  | grep -oE '[0-9]*$' | awk '{s+=$1} END {printf "%.0f\n", s}'

# Incremental count from the installed script
XDG_CACHE_HOME=$work/cache bash -c "$(sed -n '/^calculate_tokens()/,/^}/p' ~/.amplihack/.claude/tools/statusline.sh)
calculate_tokens \"\$1\"" _ "$t"

rm -rf -- "$work"
```

The two commands print the same number. Run against the live transcript
instead of a copy, the figures can differ, because the file can grow between
the two commands. The tests in
`crates/amplihack-cli/tests/issue_1921_statusline_tokens.rs` extract the
function from the shipped script in the same way and compare it with a Rust
oracle, using temporary `HOME` and `XDG_CACHE_HOME` directories only.

## Login capacity line

The capacity line shows which login is live and how much of its capacity is
used. It is printed below the session line:

```
~/src/amplihack-rs (main → origin) [github.com/rysweet/amplihack-rs] Opus 🎫 1.2M 💰$15.67 ⏱1h
🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%  · 3 spare
```

### Command

```
amplihack profile statusline [--root <path>] [--owner <name>]
```

Claude Code runs the command on every status-line refresh and sends a JSON
object on standard input. Both arguments are optional.

| Argument         | Effect                                                                                                                                                          |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--root <path>`  | Uses this profile root. It must be absolute.                                                                                                                    |
| (no `--root`)    | Resolves the root as other profile commands do: `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then the saved `profile root` setting, then `~/.amplihack/claude-profiles`. |
| `--owner <name>` | Runs as the [status-line wrapper](./profile-command.md#status-line-wrapper) of profile `<name>`.                                                               |

`--owner` no longer needs `--root`. When the root cannot be resolved, the
reason goes to stderr and nothing about it is printed on standard output.
The root is never created.

### Modes

Without `--owner`, the mode is chosen from `CLAUDE_CONFIG_DIR`. The
variable is canonicalised first: symlinks are resolved and a trailing slash
is ignored.

| `CLAUDE_CONFIG_DIR`                                                                    | Mode        | Behaviour                                                                                |
| -------------------------------------------------------------------------------------- | ----------- | ---------------------------------------------------------------------------------------- |
| Unset, empty, or `$HOME/.claude`                                                       | Home        | Runs `statusline.sh`, then prints the capacity line for the login live in `~/.claude` and the capacity notice when it is due. |
| `<root>/<p>`, where the canonical parent is the canonical root and `<p>` is a valid profile name | Profile     | Behaves like `--owner <p>`, without recovery, the swap check or the capacity-file update. See [Inside profiles](#inside-profiles). |
| Anything else                                                                          | Passthrough | Runs `statusline.sh` only. No capacity line, no capacity notice and nothing recorded.   |

A directory reached through a symlink is matched on its canonical path, so a
symlink to a profile from outside the root is treated as passthrough unless
it resolves into the root.

With `--owner`, the command always runs in profile mode for that profile.

### Output order

The order is the same in home and profile mode. Passthrough mode prints only
item 1.

1. the original command's output, copied through unchanged; a newline is
   added only when the output did not end with one;
2. the capacity line, when there is one;
3. the [capacity notice](./profile-command.md#capacity-notice), when it is
   due.

Nothing else is written to standard output.

### Line format

```
🔑 <login>  5h <bar> <pct>%  7d <bar> <pct>%  · <n> spare
```

Segments are separated by two spaces and appear in this order. Each one is
left out on its own when its value is unknown.

| Segment           | Shows                                     | Left out when                                                       |
| ----------------- | ----------------------------------------- | ------------------------------------------------------------------- |
| `🔑 <login>`      | The live login                            | No login can be resolved.                                           |
| `5h <bar> <pct>%` | The five-hour session window              | The input has no usable `five_hour` window.                         |
| `7d <bar> <pct>%` | The seven-day window                      | The input has no usable `seven_day` window.                         |
| `· <n> spare`     | Profiles with room in both windows        | The profile list, auto-switch state or a readings file cannot be read. |

`· 0 spare` is printed when the count is known to be zero; an unknown count
is never shown as 0. A line holding only some segments, even only the spare
count, is printed. When no segment remains, nothing is printed, not even a
blank line.

Control characters are removed from the login name before it is printed.

#### Percentages and bars

The percentage is `used_percentage` from the input, rounded half away from
zero. The line prints the value it is given: a value over 100 is printed as
reported, with a full bar, and a negative value as `0`. Before that, each
window passes the same bounds check that `profile usage` applies to its
readings, which limits `used_percentage` to 0 to 100; Claude Code's input
therefore never shows more than `100%`. A window is treated as missing when
its value is not a finite number, or when its `resets_at` is earlier than
one day before now or later than eight days after now.

The bar has 10 cells. The number of filled cells (`█`) is the percentage
divided by 10 and rounded half away from zero, limited to 0 to 10. The rest
are empty (`░`).

| Used | Printed           |
| ---- | ----------------- |
| 0    | `░░░░░░░░░░ 0%`   |
| 34   | `███░░░░░░░ 34%`  |
| 35   | `████░░░░░░ 35%`  |
| 42.5 | `████░░░░░░ 43%`  |
| 43   | `████░░░░░░ 43%`  |
| 96   | `██████████ 96%`  |
| 112  | `██████████ 112%` |
| -3   | `░░░░░░░░░░ 0%`   |

The input is read once, up to 16 MiB. Only the first 1 MiB is parsed for
`rate_limits`; larger input is still passed to the original command in full,
but gives no bars and records nothing. Missing windows do not hide the login
or the spare count.

### Data sources

| Value       | Source                                                                                                                                                         |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Login name  | Home mode: the rotator ledgers. Profile mode: the auto-switch ledger entry for that profile.                                                                  |
| Percentages | The `rate_limits` object in the JSON Claude Code sends on standard input. Nothing is fetched from the network.                                                |
| Spare count | The profile list, the auto-switch settings and ledger, the overflow marks and the readings in `<root>/.auto-switch.d/usage.json` and `<root>/.auto-switch.d/home-usage.json` |

`<root>` is the profile root (see
[Profile root](./profile-command.md#profile-root)).

#### Credentials

The capacity line, home mode and profile mode without `--owner` never read
credentials. None of them opens any `.credentials.json` file, and the
capacity line and home mode also never test or name one, or read
`~/.claude.json` or any profile's `.claude.json`. Tests watch the credentials
files with inotify while the binary runs, and fail if any is opened: a home
tick that records a reading with auto-switch on, and a profile tick without
`--owner` that is over the threshold with an overflow profile marked.

The installed `--owner` wrapper has one exception. Auto-switch has to tell
an API-key profile from a plan login, because an API-key profile is never
swapped into a running session (issue #1804) and does not count towards the
capacity state, and a login's kind is read from its login file. So the
wrapper resolves profile kinds in two steps, and only when that step has
work to do. A third step, recovery, moves login files without reading them:

- the [capacity file](./profile-command.md#capacity-file) update, when this
  reading is at or over the threshold, or the file reports low or no
  capacity;
- the [in-session swap](./profile-command.md#in-session-swap) check, when
  this reading is at or over the threshold, or on every tick while an
  overflow profile is marked;
- recovery, on every tick, which does nothing unless a swap journal is
  waiting or a hosted session has ended. It then renames `.credentials.json`
  files into place and updates the `.claude.json` account fields.

Profile mode without `--owner` runs none of the three, so a session with no
wrapper moves no login. A swap journal stops it recording a reading, because
which login is live is not settled; the journal stays for the wrapper,
`repair` or the next command that recovers. Tests check that such a tick
leaves a stale `hosts` entry, a journal and the login files as they were, and
that the wrapper does return the login.

An ordinary tick, under the threshold with no overflow profile marked, no
capacity notice in force and nothing to recover, opens no login file; a test
checks this too.
Another test checks that a tick at the threshold still updates the capacity
file, and that a later tick clears it. The file is also kept current by
`profile usage` and the rotator.

#### Live login in home mode

The live login comes from the two rotator ledgers only:

- `<root>/.auto-switch.d/rotate/ledger.json`, written by `profile rotate`;
- `<root>/.rotator/ledger.json`, written by an external rotator.

Each ledger is read with the trust rules in
[Logins in `~/.claude`](./profile-command.md#logins-in-claude): owned by you,
not writable by other users, not a symlink and within the size limit. An
empty `live` counts as no ledger. When both ledgers name a login, the one
with the later `since` wins; on a tie the rotate ledger wins, and a ledger
with no `since` counts as older.

The name must be a valid profile name, and `<root>/<name>` must be a
directory itself, not a symlink to one. Otherwise there is no login: the
`🔑` segment is left out and nothing is recorded.

The ledger is not compared with the account in `~/.claude.json`. If someone
runs `claude /login` by hand in `~/.claude`, the `🔑` segment can name the
previous login until the rotator next writes its ledger, and a reading
recorded in that time is filed under that previous login. The same happens
when a session started before a swap keeps its old token for more than 300
seconds. Nothing in home mode can detect either case, so its readings go
into a file that nothing acts on; see
[Recording readings](#recording-readings).

### Recording readings

In home mode the command records the input's reading in
`<root>/.auto-switch.d/home-usage.json`, never in `usage.json`, and only
when all of these hold:

1. the profile root is private (a real directory owned by you and not
   writable by other users);
2. the input has at least one usable window;
3. the winning ledger has a `since`, and at least 300 seconds have passed
   since it (the auto-switch swap cool-down);
4. auto-switch attribution for that profile gives an account to record
   against: the account auto-switch has live in that profile, or the profile
   itself when auto-switch has no entry for it. Attribution refuses a
   reading taken within 300 seconds of an auto-switch swap, or one that
   matches the figures recorded before that swap.

The reading is stored under the attributed account, with `source` set to
`statusline`, merged with any existing reading in `home-usage.json` in the
same way as the
[status-line wrapper](./profile-command.md#status-line-wrapper).
`home-usage.json` has the same format and the same file checks as
`usage.json`. The command
waits at most 50 ms for the state lock; when the lock is busy, it records
nothing on that run. Recording happens whether auto-switch is enabled or
not. Home mode reads the
[capacity file](./profile-command.md#capacity-file) to print the notice and
does not update it: working out which
profiles are overflow profiles means reading their login files, which this
path never does. The status-line wrapper and `profile rotate` keep that file
current.

When any condition fails, nothing is recorded. The capacity line is still
printed, with the bars from this run's input.

Home mode never runs auto-switch recovery or an in-session swap, because the
rotator owns `~/.claude`. It writes nothing other than `home-usage.json`:
`usage.json`, the capacity file, the journal, the ledgers and `hosts` are
left untouched.

#### Why the readings are kept apart

Conditions 3 and 4 do not check which account `~/.claude` holds. Rotator
logins have no auto-switch `hosts` entry, so attribution has no fingerprint
to compare and files any reading taken more than 300 seconds after the
ledger's `since` under the ledger's login. A hand-run `claude /login`, or a
session that kept its old token past a swap, therefore gives a reading for
the wrong login. Comparing the login with the profile's
[recorded account](./profile-command.md#recorded-account) would catch the
first case only: after a swap `~/.claude.json` already names the new
account while the old session goes on reporting the old one's figures.

`usage.json` is read by every part of amplihack that acts on readings:

- `profile usage` and the rotator's measurement use a fresh entry as a
  [passive reading](./profile-command.md#passive-readings) in place of a
  real one;
- `profile rotate` reads it for the
  [exhausted-while-off](./profile-command.md#exhausted-while-off) check and
  for reset times;
- auto-switch reads it to pick a login, and the capacity writers read it.

So home mode never writes it. `home-usage.json` is read only by the spare
count below, which a person reads and nothing acts on. A wrong reading
there can make the count off by one for a while; it cannot change which
login is measured, picked or rotated to.

### Spare rule

Candidates are the profiles that `amplihack profile list` shows, other than:

- the live login;
- profiles in the auto-switch `exclude` list;
- profiles marked for [overflow](./profile-command.md#overflow-commands).

For each candidate, the reading is looked up in `usage.json` and
`home-usage.json`, the newer of the two winning, under the
account the auto-switch ledger records for that profile, or under the
profile's own name when it has no entry. A candidate counts as spare when
all of these hold:

- it has a reading;
- it is not exhausted: its reading is not a usage-limit failure whose reset
  time is still in the future. A later reading in `home-usage.json` does not
  replace such a failure;
- its five-hour window is under 50% used, or the reading has no five-hour
  window;
- its seven-day window is present and under 90% used.

Both comparisons are strict, so a profile at exactly 50% or 90% is not
spare. A window whose reset time has passed counts as 0%. Run
`amplihack profile usage` to record readings for every profile.

When home mode has just recorded a reading, the count uses the
`home-usage.json` it saved under the state lock; if the save failed, the
spare segment is left out. Otherwise the count reads its files without
taking the state lock. A damaged or half-written `usage.json` or
`home-usage.json` leaves the spare segment out for that run. A missing file
holds no readings; with neither file, every candidate counts as not spare.

### Inside profiles

In profile mode, for profile `<p>`, the command:

1. applies the [status-line wrapper](./profile-command.md#status-line-wrapper)
   checks: the root is private and `CLAUDE_CONFIG_DIR` resolves to
   `<root>/<p>`;
2. waits at most 50 ms for the state lock and records the reading when
   auto-switch is on, as the wrapper always has. With `--owner` only, it runs
   recovery first, updates the capacity file when the reading can change it,
   and checks for an in-session swap (see [Credentials](#credentials));
3. runs the saved original from `<root>/.auto-switch.d/statusline/<p>.json`.
   An original that is exactly the home command is never run; `statusline.sh`
   is run in its place. When the command was started without `--owner` and
   there is no saved original, `statusline.sh` is run too. A user's original
   can itself run `amplihack profile statusline`, as in `... | my-filter`: the
   wrapper sets `AMPLIHACK_STATUSLINE_NESTED` for what it runs, and that inner
   run runs `statusline.sh` only, so it cannot run the original again;
4. prints the capacity line, when the step 1 checks passed and the line
   stands in for the home command's: the command was started without
   `--owner`, or the saved original is the home command or the
   `statusline.sh` saved in its place. A wrapper around any other original
   copies that original's output through unchanged and adds no capacity
   line, and a wrapper with no saved original prints nothing. The line shows the account the auto-switch ledger records for `<p>`
   as the login, falling back to `<p>`, the bars from this run's input, and
   the spare count excluding `<p>`;
5. prints the capacity notice, when it is due.

New profiles receive the `~/.claude` entry through the shared settings keys,
so a profile without a wrapper still gets the capacity line for its own
login. Seeding is unchanged. Such a profile gets no in-session swap:
auto-switch moves a login only for a profile with the wrapper installed,
which `profile auto-switch status` reports and `install` adds.

When `profile auto-switch install` or `repair` finds the `~/.claude` entry
in a profile, it saves `{"type": "command", "command": "'<home>/.amplihack/.claude/tools/statusline.sh'"}`
as the original and installs the wrapper. The script path is saved only when
it passes the [script checks](#script-checks); otherwise no original is
saved and the wrapper prints nothing: neither `statusline.sh` nor the capacity
line. Fixing the script does not change that, and neither does `repair`, which
leaves a current wrapper alone. To get both lines back, copy the `statusLine`
entry from `~/.claude/settings.json` into the profile's `settings.json` and run
`profile auto-switch install`. Only the exact commands listed under
[Install wiring](#install-wiring) as the `profile statusline` forms are
treated this way; the same check decides for `install`, `uninstall` and the
wrapper. A user's command that does more, such as
`amplihack profile statusline | my-filter`, or only shares the words, such as
`my-profile statusline-helper.sh`, is saved as the original like any other.
A wrapper around such a filter prints that filter's output and adds no
capacity line.

### Script checks

`~/.amplihack/.claude/tools/statusline.sh` is run as `sh -c '<path>'`, with
the path single-quoted, the same standard input, working directory and
environment. It is run only if it is a regular file, not a symlink, owned by
you or root and not writable by other users, and each directory above it up
to `$HOME` passes the same checks. The script's `#!` line chooses its
interpreter, so it must be executable.

When the script is missing or fails the checks, nothing is run and only the
capacity line is printed.

### Failure behaviour

Every failure gives less output, never an error on standard output.

| Condition                                                                 | Result                                                                    |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `statusline.sh` missing or failing its checks                             | No first line; capacity line printed.                                     |
| Home mode: no root resolves, or the root is relative, missing, a symlink or not private | Bars only: no `🔑` or spare segment. Nothing is recorded. The reason is on stderr unless the root does not exist. |
| Profile mode: the root is not private, or `CLAUDE_CONFIG_DIR` does not resolve to `<root>/<p>` | First line only. Nothing is recorded. A root that is not private is named on stderr. |
| No ledger, an untrusted ledger, or a name with no profile directory       | No `🔑` segment. Nothing is recorded.                                     |
| Ledger `since` missing or less than 300 seconds old                       | Capacity line printed. Nothing is recorded.                               |
| Input has no `rate_limits`, or no usable window                           | No bars. Nothing is recorded.                                             |
| Input larger than 1 MiB                                                   | No bars. Nothing is recorded.                                             |
| `usage.json`, `home-usage.json` or auto-switch state damaged or unreadable | No spare segment.                                                       |
| State lock held for more than 50 ms                                       | Capacity line printed. The reading is not recorded on that run.          |
| Internal panic                                                            | The original command's output only, and one generic line on stderr.      |

The exit code is the original command's when it ran (`1` if a signal ended
it), and `0` otherwise. Logs name operations and errors only, never input,
file contents or environment values. On non-Unix platforms the command
prints nothing and exits `0`.

The capacity work (resolving the login, counting spares, rendering and
recording) takes under 200 ms with 20 or more profiles. Process start-up and
the original command are not counted. The test that checks this takes the
fastest of five runs, so a single run delayed by a loaded host does not fail
it.

### Install wiring

Only global-scope installs write the entry: an explicit `amplihack install`
and the launch-time repair that runs when amplihack's hooks are missing from
`~/.claude/settings.json`. Implicit installs and repo-local
`.claude/settings.json` files never add, change or remove `statusLine`, and
neither does seeding or refreshing profile settings.

| Existing `statusLine` in `~/.claude/settings.json` | What install does                                                         | Printed                                |
| -------------------------------------------------- | ------------------------------------------------------------------------- | -------------------------------------- |
| None                                               | Writes `{"type": "command", "command": "<cmd>"}`                          | `statusLine: installed`                |
| amplihack's, with a different command              | Replaces `type` and `command`; `padding` and every other key are kept     | `statusLine: updated`                  |
| amplihack's, with the current command              | Nothing                                                                   | nothing                                |
| Anything else                                      | Nothing; the entry is kept byte for byte                                  | `statusLine: kept your existing entry` |

An entry is amplihack's only when it is an object whose `command`, with
leading and trailing spaces removed, is exactly one of:

- the command this install writes;
- a single-quoted absolute path to a binary named `amplihack`, followed by
  ` profile statusline`, as an older install wrote it;
- `amplihack profile statusline`;
- the legacy script path and nothing else:
  `~/.amplihack/.claude/tools/statusline.sh`, the same after `$HOME` or
  `${HOME}`, or after an absolute path, optionally inside one pair of single
  or double quotes.

Everything else is kept as yours, byte for byte, including:

- a command that runs the script and does more, such as
  `bash ~/.amplihack/.claude/tools/statusline.sh | my-filter`;
- a command that only contains the words, such as
  `my-profile statusline-helper.sh`;
- the older example `$CLAUDE_PROJECT_DIR/.claude/tools/statusline.sh`;
- a profile wrapper (`profile statusline ... --owner ...`) copied in by hand;
- a value that is not an object, or a `settings.json` whose top level is not
  an object.

To switch a kept entry to amplihack's, delete the `statusLine` key and run
`amplihack install` again.

`amplihack uninstall` removes the `statusLine` key from
`~/.claude/settings.json` when its `command` is one of the `profile
statusline` forms or the legacy script above, for a binary named `amplihack`.
Any other entry is kept byte for byte. It changes no other key.
Uninstall does not touch profile directories: a wrapper in a profile's
`settings.json` stays and names a binary that is gone, so remove that
`statusLine` key by hand if you keep the profile.

`<cmd>` is the installed amplihack binary followed by `profile statusline`,
with the path single-quoted for `sh`. The installed binary is the copy in
`~/.local/bin`, which install has just deployed; when there is none, it is
the running binary, with a warning if that is a build output, because the
status line stops working when that directory is cleaned. When the binary cannot be located or
is refused (it or a directory above it can be changed by other users), or
its path cannot be quoted safely (for example, it contains a newline),
install prints a warning and writes no `statusLine` entry. It never falls
back to a bare `amplihack` found through `PATH`. The `statusLine` decision
never makes install fail.

Running install again gives the same file, with one `statusLine` key. Other
keys keep their values; their order may change. `settings.json` is written
in place after the usual timestamped backup, not through a temporary file.

The launch repair is tested through `ensure_framework_installed_with`
(`install/tests/issue_1745_launch_repair_statusline.rs`): it writes the
entry when there is none, keeps a user's own entry and records the deployed
binary over the running one. The repo-local
path, which passes no command, is tested in `install/tests/settings_tests.rs`
(`a_repo_local_install_*`).

### Differences from issue #1745

Three parts of the build differ from what was asked for on issue #1745. All
are recorded on the issue.

1. **Readings from `~/.claude` do not reach the rotator.** The issue asked
   that home mode record its reading "as the wrapper does today (so readings
   from ~/.claude sessions also feed the rotator)". Home mode writes
   `home-usage.json` instead, which only the spare count reads. `profile
   usage`, `profile rotate` and auto-switch read `usage.json` alone. The
   reason is in [Why the readings are kept apart](#why-the-readings-are-kept-apart):
   nothing in home mode can confirm which account `~/.claude` holds, and a
   reading filed under the wrong login would change which login is
   measured, picked or rotated to. Under rotation most sessions run in
   `~/.claude`, so the rotator gets no readings from them and relies on its
   own measurements. A way to attribute those readings safely is left to a
   follow-up issue.
2. **The profile wrapper adds the capacity line only where it stands in for
   the home command.** The issue said the wrapper "prints the same line after
   the original command". It does so only when started without `--owner`,
   or when the saved original is the home command or `statusline.sh` (see
   [Inside profiles](#inside-profiles), step 4). A wrapper with no saved
   original prints nothing, as the #1569 wrapper tests require. A wrapper around a user's
   own status line copies its output through byte for byte, as it did
   before #1745 and as the #1569 wrapper tests require, so a user's own line
   is never changed by amplihack.
3. **The `--owner` wrapper can still open login files.** The requirement was
   that the status-line path never reads credentials. That holds for the
   capacity line, home mode and profile mode without `--owner`. The installed
   wrapper keeps the exception in [Credentials](#credentials), because
   auto-switch cannot choose a swap target safely without each profile's
   kind, and the kind is read from the login file. Removing the reads would
   mean removing the in-session swap, or letting it swap an API-key profile
   into a running session.

## Project Structure

The statusline integrates with amplihack's structure:

```
.claude/
├── agents/     # Agent definitions (core + specialized)
├── context/    # Philosophy and patterns
├── workflow/   # Development processes
└── commands/   # Slash commands
```

## Power-Steering

Power-steering is AI-powered session guidance that prevents premature session termination by analyzing your work against 21 completion criteria.

### Purpose

Power-steering analyzes your session using **21 distinct considerations** to determine if work is truly finished, blocking stop attempts when tasks remain incomplete, unfinished TODOs exist, or workflow steps were skipped.

### What It Does

When you try to stop a session, power-steering:

1. **Reads the transcript** - Analyzes the entire conversation history
2. **Evaluates 21 considerations** - Checks completion against multiple criteria:
   - Session Completion & Progress (8 checks)
   - Workflow Process Adherence (3 checks)
   - Code Quality & Philosophy Compliance (3 checks)
   - Testing & Local Validation (2 checks)
   - PR Content & Quality (3 checks)
   - CI/CD & Mergeability Status (2 checks)
3. **Makes a decision** - Either approves the stop or blocks it with specific reasons
4. **Provides continuation prompt** - Tells you exactly what needs completion

### The 21 Considerations

Power-steering evaluates your session against these considerations:

#### Session Completion & Progress (8 checks)

1. Session type matches requirements - Investigation vs development work
2. TODOs completed - All planned tasks finished
3. User objectives achieved - Original goals met
4. Documentation updated - Changes reflected in docs
5. No pending work - Nothing left unfinished
6. Questions asked appropriately - Clarified when needed
7. Session logs complete - All interactions documented
8. Natural stopping point - Sensible place to pause

#### Workflow Process Adherence (3 checks)

9. Workflow steps followed - Proper sequence maintained
10. Required steps completed - No skipped stages
11. Step order correct - Logical progression

#### Code Quality & Philosophy Compliance (3 checks)

12. Code quality verified - Standards met
13. Philosophy alignment checked - Principles followed
14. Module boundaries respected - Clean architecture

#### Testing & Local Validation (2 checks)

15. Tests executed - Validation performed
16. Local testing done - End-to-end verification

#### PR Content & Quality (3 checks)

17. PR created properly - Complete information
18. PR reviewed - Quality check performed
19. Review feedback addressed - Changes incorporated

#### CI/CD & Mergeability Status (2 checks)

20. CI passing - All checks green
21. PR mergeable - Ready for integration

### Indicator Display

When power-steering redirects a stop attempt, the statusline shows:

```
🚦×N
```

Where `N` is the number of times power-steering has intervened in **this session**. The indicator appears in **purple** color.

### Configuration

Power-steering can be toggled via `~/.amplihack/.claude/tools/amplihack/.power_steering_config`:

```json
{
  "enabled": true // false to disable
}
```

### Examples

#### Example 1: Blocked Stop - Incomplete TODOs

```
User attempts to stop mid-workflow...

🚦 POWER-STEERING REDIRECT 🚦

The session cannot be stopped because:
- 5 TODOs remain uncompleted
- Workflow Step 12 (Run Tests) not executed
- PR not created yet

Continue with: Complete remaining TODOs and execute workflow steps.
```

#### Example 2: Approved Stop - Work Complete

```
User attempts to stop after successful merge...

✅ Power-steering approved stop request
All 21 considerations satisfied:
- All TODOs completed
- All workflow steps executed
- Tests passing
- PR merged
- CI green

Session can safely end.
```

### Session Counter

The counter (`N` in `🚦×N`) tracks interventions **per session only**:

- Resets to 0 at session start
- Increments each time power-steering blocks a stop
- Does NOT persist across sessions
- Helps you see how many times you tried to stop prematurely

## Lock Mode

Lock mode is continuous work mode that forces Claude to keep working until you give explicit permission to stop.

### Purpose

Lock mode enables **uninterrupted autonomous work** by blocking all stop attempts and forcing Claude to continue pursuing the user's objective until explicitly unlocked.

### Commands

#### Enable Lock Mode

```bash
/amplihack:lock [optional custom instruction]
```

**Basic usage (default continuation prompt)**:

```bash
/amplihack:lock
```

When enabled with no custom message, Claude receives the default continuation prompt:

> "Continue working autonomously and independently. Do not stop. Pursue the user's objective with the highest quality and attention to detail."

**With custom instruction** (up to 1000 characters):

```bash
/amplihack:lock Focus on security fixes first, then performance optimizations
```

The custom instruction becomes the continuation prompt that Claude receives whenever it tries to stop.

#### Disable Lock Mode

```bash
/amplihack:unlock
```

Immediately disables lock mode and allows normal stop behavior.

### How It Works

When lock mode is active:

1. **All stop attempts blocked** - Claude cannot stop under any circumstances
2. **Custom prompt delivered** - Your instruction (or default) is sent to Claude
3. **Autonomous continuation** - Claude resumes work based on the prompt
4. **Counter increments** - Tracks how many times stop was blocked
5. **Remains active until unlocked** - Persists across multiple stop attempts

### Indicator Display

When lock mode is active, the statusline shows:

```
🔒×N
```

Where `N` is the number of times the lock has **blocked a stop attempt** in **this session**. The indicator appears in **yellow** color.

### Custom Continuation Messages

**Message Constraints**:

- **Maximum length**: 1000 characters (hard limit)
- **Warning threshold**: 500 characters (shows warning but allows)
- **Empty message**: Falls back to default continuation prompt

**Examples of effective custom messages:**

```bash
# Focus directive
/amplihack:lock Complete all testing before moving to documentation

# Priority order
/amplihack:lock Fix bugs first, then refactor, then add features

# Quality emphasis
/amplihack:lock Every function must have comprehensive tests and error handling

# Workflow enforcement
/amplihack:lock Follow the default-workflow skill/recipe exactly - do not skip steps
```

### Examples

#### Example 1: Enable Lock Mode (Default)

```bash
/amplihack:lock
```

**Result:**

```
🔒 LOCK MODE ENABLED 🔒

All stop attempts will be blocked.
Continuation prompt: [default autonomous work instruction]

To disable: /amplihack:unlock
```

#### Example 2: Stop Attempt While Locked

```
Claude attempts to stop...

🔒 LOCK MODE ACTIVE 🔒

Stop attempt blocked. Counter: 3

Continuation prompt delivered:
"Complete the authentication feature including all tests and documentation"

Resuming work...
```

### Interaction with Power-Steering

When **both** lock mode and power-steering are active:

1. **Lock mode takes precedence** - Blocks all stops unconditionally
2. **Power-steering is bypassed** - 21 considerations not evaluated
3. **Counter tracks both** - Shows lock mode indicator in statusline
4. **Different purposes**:
   - **Lock mode** = "Never stop, regardless of completion"
   - **Power-steering** = "Only stop when work is actually complete"

**Recommendation**: Use lock mode for extended autonomous work sessions. Use power-steering for quality-gated completions.

### Session Counter

The counter (`N` in `🔒×N`) tracks blocked stops **per session only**:

- Resets to 0 at session start
- Increments each time lock mode blocks a stop
- Does NOT persist across sessions
- Helps you see how determined Claude was to stop

### Use Cases

**When to use lock mode:**

1. **Long autonomous tasks** - Multi-hour implementations
2. **Workflow completion** - Must finish all workflow steps
3. **Testing marathons** - Comprehensive test suites
4. **Documentation sprints** - Complete doc overhaul
5. **Debugging sessions** - Won't stop until bug is fixed

**When NOT to use lock mode:**

1. **Exploratory work** - Need flexibility to pivot
2. **User collaboration** - Frequent back-and-forth expected
3. **Uncertain requirements** - May need to stop and clarify
4. **Short tasks** - Overhead not worth it

## See Also

- [Power-Steering](#power-steering) - AI-powered session guidance
- [Lock Mode](#lock-mode) - Continuous work mode
- [Token count](#token-count) - How the `🎫` total is counted and cached
- [Login capacity line](#login-capacity-line) - Live login and capacity in `~/.claude`
- [Profile command](./profile-command.md#status-line-wrapper) - Per-profile status-line wrapper and readings
- [Configuration Guide](../howto/configure-hooks.md) - Session hooks and settings
- [Development Workflow](../concepts/default-workflow.md) - Process customization
