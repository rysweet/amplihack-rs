# Reference: `amplihack profile`

Manage Claude Code login profiles. See the
[how-to](../howto/claude-profiles.md) for a walkthrough.

## Synopsis

```
amplihack profile add <name>
amplihack profile list
amplihack profile root [<dir>]
amplihack profile show
amplihack profile switch <name>
amplihack profile remove <name> --yes
amplihack profile login <name>
amplihack profile run <name> -- <claude-args>...
amplihack profile usage [--json] [--raw] [<name>...]
amplihack profile auto-switch on [--threshold <1-100>] [--exclude <name,...>]
amplihack profile auto-switch off
amplihack profile auto-switch status
amplihack profile auto-switch threshold <1-100>
amplihack profile auto-switch exclude [<name>...]
amplihack profile auto-switch install
amplihack profile auto-switch repair
amplihack profile rotate [--dry-run] [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS]
amplihack profile rotate start|watch [<tuning flags>] [--interval SECS]
amplihack profile rotate stop|status
amplihack launch|claude --profile <name> [...]
```

## Subcommands

| Command | Behavior |
|---------|----------|
| `add <name>` | Copy allowlisted shared assets from `~/.claude` into a private staging directory, then atomically publish `<root>/<name>/` (mode `0700` on Unix; Windows uses its inherited ACL). Errors if a profile exists whose name matches case-insensitively, on every platform. Does not run Claude. The first `add` also saves the root it used; see [Saved on the first add](#saved-on-the-first-add). |
| `root [<dir>]` | Print the profile root in use and where it came from, or save `<dir>` as the root. See [Profile root](#profile-root). |
| `list` | First line `root: <path> (source: env\|config\|default)`; see [Profile root](#profile-root). Then one profile per line, sorted; the default is marked `*`. Skips dot-entries, symlinks and non-directories. Reports a corrupt or dangling default pointer instead of silently omitting the marker. |
| `show` | Prints `name: <name>` and `dir: <path>`, or `none`. Paths only; never directory contents. |
| `switch <name>` | Writes the default pointer. The profile must exist. |
| `remove <name>` | Requires `--yes`. Resolves and verifies the path is inside the root, reads the default pointer, deletes the directory, then clears a matching pointer. A missing pointer means no default; a corrupt, unreadable or dangling pointer fails closed before any deletion (repair with `switch`). Never deletes the root; never follows symlinks. |
| `login <name>` | Always runs the `claude` binary (ignores binary selection), interactively, with `CLAUDE_CONFIG_DIR=<profile dir>`. |
| `run <name> -- <args>` | One-shot launch through the normal launch path. Args are forwarded as a literal argv vector; no shell is involved. |
| `usage [<name>...]` | Runs `claude -p /usage` under each profile in parallel and shows its session and weekly usage, most capacity first. `--json` for scripts, `--raw` to include Claude's text. Refreshes auto-switch readings. See [Usage command](#usage-command). |
| `auto-switch …` | Turn [auto-switch](#auto-switch) on or off, set its threshold and exclusions, show its state, or install and repair the status-line wrapper. See [Auto-switch commands](#auto-switch-commands). |
| `rotate …` | Keep the login in `~/.claude` on the account with the most capacity: once, as a dry run, or in a background process with `start`, `stop` and `status`. See [Rotate](#rotate). |

`remove` and `login` are also refused while auto-switch has an account's
login away from home; see [Leases](#leases).

### Usage command

```
amplihack profile usage [--json] [--raw] [<name>...]
```

Shows how much of each profile's current session and weekly quota is used
and when each resets, so you can see which accounts have capacity. With no
names it covers every profile under the root, as `list` does. Each name must
pass the [name rules](#name-rules) and must exist. A name given twice is
probed once.

| Flag | Effect |
|------|--------|
| `--json` | Print a JSON array instead of the table. See [JSON output](#json-output). |
| `--raw` | Also print what `claude` wrote to stdout and stderr, for each profile. Without it, that text is dropped once parsed. |
| `<name>...` | Probe only these profiles, in the [usual order](#ordering). |

#### How a profile is probed

For each profile, amplihack runs the `claude` binary directly, as `login`
does (the binary selection is ignored, and `AMPLIHACK_CLAUDE_BINARY_PATH` is
honoured):

```
claude -p /usage
```

The child gets `CLAUDE_CONFIG_DIR=<profile dir>` on its own `Command`; the
environment of amplihack itself is never changed. The one exception is a
profile whose login is [live in `~/.claude`](#logins-in-claude): its child
has `CLAUDE_CONFIG_DIR` removed instead, so `claude` reads `~/.claude` and
`~/.claude.json`. It is never set to `~/.claude`: when `CLAUDE_CONFIG_DIR`
is set, `claude` reads `.claude.json` from inside that directory
(`~/.claude/.claude.json`), not the `~/.claude.json` the default login uses,
so it would report the wrong account or none. Removing the variable is
exactly what "use the default directory" means. That child still gets
`AMPLIHACK_CLAUDE_PROFILE=<name>`, like every other probe. There is no
shell, stdin is `/dev/null`, and no other arguments are passed. The profile directory is
checked again just before spawn, as for every launch.

- Profiles are probed in parallel, at most 8 at a time.
- Each probe has a 30-second deadline from spawn. When it passes, the child is
  killed and the profile's state is `timeout`.
- On Unix each child runs in its own process group. When the probe ends,
  whether by the deadline or a normal exit, the whole group is killed, so
  processes `claude` started (MCP servers, for example) do not outlive it.
  A process that left the group itself is not tracked. On other platforms
  only the child is killed.
- stdout and stderr are each read up to 64 KiB. Anything after that is
  dropped and the rest is still parsed.
- Running `/usage` does not appear to use quota: an exhausted session still
  answers, and its session percentage does not change. This is `claude`'s
  behaviour, not something amplihack controls. `claude` contacts Anthropic to
  answer it; amplihack itself makes no network calls.

The deadline can be changed for tests and slow machines:

| Setting | Values | Default |
|---------|--------|---------|
| `--timeout-secs <n>` (hidden flag) | `1` to `600` | — |
| `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` | `1` to `600` | `30` |

The flag wins over the variable. A flag value out of range is a usage error
(exit status 2). An empty variable counts as unset. A variable that is not a
number from `1` to `600` prints one warning showing the value escaped and cut to 32 characters, then uses `30`:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS "soon" (use 1 to 600); using 30
```

#### What is read

These lines of stdout are read. Each one has a percentage and usually a
reset time:

```
Current session: 100% used · resets Oct 16, 4:50pm (America/Los_Angeles)
Current week (all models): 68% used · resets Oct 22, 2am (America/Los_Angeles)
Current week (Fable): 0% used · resets Oct 22, 2am (America/Los_Angeles)
```

Claude leaves out the reset when the window has not started, for example
`Current session: 0% used` when no session is open. Such a line is still
read: `resets_text` and `resets_at` are `null`, and the table shows `-` for
the reset.

| Line | Field |
|------|-------|
| `Current session` | `session` |
| `Current week (all models)` | `week_all` |
| `Current week (<anything else>)` | one entry in `week_models`, in the order printed |

The separator before `resets` may be `·`, `•`, `-` or missing. Anything
after `used` other than a reset clause makes the line unreadable. Extra spaces
and decimal percentages (`12.5%`) are accepted. A line whose percentage is
not a finite number, or is above 1,000,000, is ignored. Every other line is
ignored. The parser never guesses a number: a value it cannot read is left
out.

The reset text is kept exactly as Claude printed it (`resets_text`). amplihack
also turns it into a time (`resets_at`, Unix epoch seconds) when it can:

| Form | Example | Meaning |
|------|---------|---------|
| Month, day, time, zone | `Oct 22, 2am (America/Los_Angeles)` | The year that puts the time between one day before now and 366 days after now: this year, then next year, then last year. |
| Month, day, year, time, zone | `Jan 2, 2027, 9:30pm (UTC)` | That date. |
| Time, zone | `4:50pm (America/Los_Angeles)` | The first time of day at or after 10 minutes ago in that zone: usually today, or tomorrow if that time has passed. Claude prints whole minutes, so a reset due within the current minute can print as a time a few seconds ago, just before midnight included. |

Minutes are optional, `am`/`pm` may be in any case, and spacing is tolerant.
The zone must be an IANA name; the host's own time zone is never used. If
the zone is unknown, the time does not exist in that zone (a daylight-saving
gap) or the text has another form, `resets_at` is `null` and only the text is
shown. A time that occurs twice (the hour repeated when daylight saving
ends) resolves to the earlier one.

#### States

Every profile gets exactly one state:

| State | Meaning |
|-------|---------|
| `ok` | The session line or the all-models week line was read, and neither is at 100%. |
| `session_exhausted` | The session is at 100% or more. |
| `weekly_exhausted` | The all-models week is at 100% or more and the session is not. |
| `spend_limit` | Claude reported that the organisation's spend limit was reached. Any percentages read are kept. |
| `not_logged_in` | No usage line of any kind (model week lines included) was read and Claude reported that the profile is not signed in. Run `amplihack profile login <name>`. A profile [live in `~/.claude`](#logins-in-claude) is probed against `~/.claude`, so its missing `.credentials.json` does not give this state. |
| `timeout` | The probe passed its deadline. |
| `unparsed` | Nothing recognisable was printed. Run again with `--raw` to see what Claude said. |
| `error` | The probe could not run. The reason goes to stderr in both modes, and the JSON also has it in `error`. |

When several apply, the first in this order wins: `error`, `timeout`,
`spend_limit`, `session_exhausted`, `weekly_exhausted`, `ok`,
`not_logged_in`, `unparsed`. A model-specific week line at 100% (for example
`Current week (Opus)`) does not make a profile exhausted; the profile can
still be used with other models. The exit status of `claude` does not decide
the state: output that parses counts even after a non-zero exit.

The spend-limit and not-logged-in states are recognised by phrases, matched
without regard to case in stdout and stderr:

| State | Phrases |
|-------|---------|
| `spend_limit` | `spend limit`, `spending limit`, `usage limit for your organization`, `organization has reached` |
| `not_logged_in` | `not logged in`, `run /login`, `invalid api key`, `login required`, `authentication_error`, `oauth token has expired` |

A spend-limit phrase wins even when usage lines were read, and such a row is
not recorded: the session and weekly resets do not lift a spend limit, so
recording its percentages would make auto-switch treat the profile as
usable. A normal `/usage` reading contains none of these phrases.

The not-logged-in phrases count only when no usage line of any kind was
read; a model week line alone is enough to rule them out. New
wording from Claude that matches none of these gives `unparsed`, never a
guessed state. Scripts should treat a state they do not know as `unparsed`.

The reasons for `error` are fixed text, never the underlying I/O error:

| Reason | When |
|--------|------|
| `profile directory refused` | The profile cannot be resolved, or its directory fails the check just before spawn. |
| `could not start claude` | Spawning `claude` failed. |
| `could not read claude output` | Reading the child's stdout or stderr failed, or a pipe was still open 2 seconds after the child exited (a process that left its group still held it), so the output may be cut short. |
| `could not wait for claude` | Waiting for the child failed. It is killed and reaped first. |

#### Table output

```
PROFILE   MARKS             STATE              SESSION  SESSION RESETS                        WEEK  WEEK RESETS
review    -                 ok                 5%       Oct 16, 9pm (America/Los_Angeles)     10%   Oct 22, 2am (America/Los_Angeles)
work      * hosts:personal  ok                 20%      Oct 16, 7pm (America/Los_Angeles)     30%   Oct 22, 2am (America/Los_Angeles)
team      -                 session_exhausted  100%     Oct 16, 4:50pm (America/Los_Angeles)  68%   Oct 22, 2am (America/Los_Angeles)
personal  leased→work       not_logged_in      -        -                                     -     -
```

`SESSION` and `WEEK` are the session and all-models week. Percentages print
as a whole number when they are whole and with one decimal place otherwise
(`12.5%`). `-` means no value. Reset columns show Claude's text as printed.
Model-specific week lines are not in the table; they are in the JSON.

Text from Claude and from state files is cleaned before it reaches the
terminal: control characters (including escape sequences) are replaced, and reset
text longer than 64 characters is cut to 64 and followed by `…`. The table's layout is
for people and may change; scripts should use `--json`.

For each `error` row, one line goes to stderr, with or without `--json`:

```
warning: profile "review": could not start claude
```

With `--raw`, each row is followed by what Claude printed, indented and
cleaned of control characters other than newlines:

```
team      -                 session_exhausted  100%     Oct 16, 4:50pm (America/Los_Angeles)  68%   Oct 22, 2am (America/Los_Angeles)
  raw stdout:
    You are currently using your subscription to power your Claude Code usage

    Current session: 100% used · resets Oct 16, 4:50pm (America/Los_Angeles)
    Current week (all models): 68% used · resets Oct 22, 2am (America/Los_Angeles)
  raw stderr:
    (empty)
```

#### Ordering

Rows are in three groups, and the JSON uses the same order:

1. **Usable** (`ok`): least used first, by the higher of the session and
   all-models week percentages. A missing window, or one whose reset time has
   passed, counts as 0%. Then by name.
2. **Exhausted** (`session_exhausted`, `weekly_exhausted`, `spend_limit`):
   soonest back first. A profile is back when every window at 100% or more
   has reset, so "back at" is the latest of those reset times. If any of
   those windows has no `resets_at`, the back-at time is unknown, and the
   profile comes after the others. A `spend_limit` row's back-at time is
   always unknown, because the window resets do not lift a spend limit.
   Then by name.
3. **Unavailable**: `not_logged_in`, then `timeout`, then `unparsed`, then
   `error`, each by name.

#### Marks

| Mark | Meaning |
|------|---------|
| `*` | The default profile, as in `list`. |
| `leased→<dir>` | This profile's login is live in profile `<dir>`'s directory, lent by [auto-switch](#leases). |
| `hosts:<account>` | This profile's directory currently holds the login of profile `<account>`, so the numbers on this row belong to `<account>`. |
| `live in ~/.claude` | This profile's login is in the default directory `~/.claude`, and the numbers on this row were read from there. See [Logins in `~/.claude`](#logins-in-claude). |

Several marks are separated by a space (`* live in ~/.claude`); none
prints `-`. A row marked `live in ~/.claude` is placed by its state and
numbers like any other row.

While a profile's login is lent out, its own directory has no login, so its
row usually shows `not_logged_in`. That account's numbers are on the row of
the profile hosting it (`work` above, whose `account` is `personal`). Lease marks come from
the auto-switch ledger, read once before probing without taking the lock; the
default mark is read at the same time. The account a row is recorded under
is the one from this read, so a swap during the probe is caught when the
reading is recorded. If
there is no `<root>/.auto-switch.d/`, there are no lease marks. If the
default pointer or the ledger cannot be read, a warning goes to stderr and
the marks it would give are left out; the probes still run.

#### Logins in `~/.claude`

A login rotator, either [`profile rotate`](#rotate) or an external one such
as the tmux prototype, can move one account's login into the default
directory `~/.claude`. While it is there, that profile's own
directory has no `.credentials.json`. `profile usage` does not report that
profile as `not_logged_in`: it probes it against `~/.claude` instead, marks
the row `live in ~/.claude`, and sets `live_in` to `"~/.claude"` in the JSON.

The default profile (`*`) is unrelated to `~/.claude`. The default profile
is the one `amplihack claude` uses when no profile is named; `~/.claude` is
the directory `claude` uses when `CLAUDE_CONFIG_DIR` is not set. A row can
have either mark, both, or neither.

The live profile is found once, before probing, and nothing is written:

1. `~/.claude/.credentials.json` must be a regular file. If it is missing or
   is a symlink, no profile is live.
2. **Rotator ledger.** There are two possible ledgers:
   `<root>/.auto-switch.d/rotate/ledger.json`, written by
   [`profile rotate`](#rotate-state-directory), and
   `<root>/.rotator/ledger.json`, written by an external rotator. If only one
   exists, it decides. If both exist and both are trusted, the one with the
   later `since` decides; a ledger with no `since` counts as older than one
   with it, and on a tie the `profile rotate` ledger decides. `since` may be
   epoch seconds (the `profile rotate` ledger) or an ISO-8601 time with an
   offset or in local time (the external rotator writes
   `datetime.now().isoformat()`). If one of them
   cannot be trusted or read, its warning is printed and the other decides.
   A `profile rotate` ledger with no `live` value (written by a cycle that
   only raised an ALERT or found the rotator idle) names nobody and counts as
   absent, and so does one whose `account` is not the account in
   `~/.claude.json` (someone ran `/login` in `~/.claude`; the rotator raises a
   hand-change ALERT and does not update its ledger). Both are only read. The deciding ledger's `live` field names the profile
   and its `since` is used only for the
   [check after the probes](#swaps-during-the-probe); other fields, apart from
   the `profile rotate` ledger's `account`, are ignored. The checks run in this order, and the first that fails decides:

   | `live` in a trusted ledger | Result | stderr |
   |----------------------------|--------|--------|
   | Missing or `null` | No profile is live. This is the rotator's idle state. | nothing |
   | Not a string, fails the [name rules](#name-rules) (for example `"../x"`), or names a profile whose directory does not exist or fails the usual directory check | No profile is live. | the ledger warning below |
   | Names a profile that still has something at `<root>/<live>/.credentials.json` (a dangling symlink counts) | No profile is live. That profile is probed through its own directory as normal, because its own login is stronger evidence than a ledger that is probably out of date. | `warning: the login rotator's ledger names a profile that still has its own login; it is not marked live in ~/.claude` |
   | Names a profile with no `.credentials.json` | That profile is live. | nothing |

   The account match below is never tried when either ledger file exists,
   or when amplihack cannot tell whether one exists (for example, a
   `<root>/.rotator/` directory you cannot search). The second case is
   treated as an unreadable ledger.

   The ledger is taken at its word: amplihack does not compare the account
   in `~/.claude.json` with the named profile's account. If the ledger names
   the wrong profile (the rotator stopped partway through a swap, or someone
   ran `/login` in `~/.claude`), that profile's row shows the numbers of the
   login that is actually in `~/.claude`.

   A ledger is trusted only if the profile root and the directories leading
   to it (`<root>/.rotator`, or `<root>/.auto-switch.d` and its `rotate`) are
   real directories (not symlinks) owned by you that no other user can write
   to (group write is allowed only when the group is your own per-user
   group), and `ledger.json` is not a symlink, is owned by
   you, is not writable by group or others, and is at most 64 KiB. The
   `profile rotate` ledger must also be `"version": 1`. If any of
   these fails, or the file is not valid JSON, that ledger is ignored and one
   warning goes to stderr. If no other ledger decides, no profile is live and
   the account match below is not tried:

   ```
   warning: could not read the login rotator's ledger; no profile is marked live in ~/.claude
   ```

   No warning text is ever built from the ledger's contents.
3. **Account match**, only when there is no ledger file. A missing
   `ledger.json`, with or without its directory, is the
   normal case when no rotator is installed, and prints nothing. A profile counts as
   live if its directory passes the usual check, it has no
   `.credentials.json`, and the `oauthAccount` in its `.claude.json` matches
   the one in `~/.claude.json`:
   - when both have an `accountUuid`, the uuids decide;
   - otherwise both `emailAddress` values are compared without regard to
     case;
   - an empty or non-string value counts as missing and never matches.

   If the profile root cannot be listed, none is marked and
   `warning: could not list profiles; no profile is marked live in ~/.claude`
   goes to stderr. If more than one profile matches, none is marked and one
   warning goes to stderr:

   ```
   warning: more than one profile matches the account in ~/.claude; none is marked live
   ```

At most one row is marked. If `HOME` is unset or empty, no row is marked.
The ownership and mode checks above exist only on Unix, so on Windows and
other non-Unix platforms no row is marked and no warning is printed. In
practice this works on Linux only: on macOS Claude keeps the login in the
Keychain and writes no `~/.claude/.credentials.json`, so step 1 finds no
login and no row is marked. When names are given, the mark appears only if
the live profile is one of them, and the warnings above are printed only if
at least one named profile has no `.credentials.json` of its own (a profile
with its own login is never live, so the warning could not concern it). The
own-login warning is the exception and is always printed, because the profile
it is about has a login.

##### Swaps during the probe

After every probe has finished, the same evidence is checked again: the
ledger's `live` and `since` fields, or the matched account. `since` is only
compared for equality; it is never printed or stored. If the evidence
changed or can no longer be read, the login in `~/.claude` may have been
swapped during the probe. The row is still shown, with its numbers and its
mark, but its usage is [not recorded](#refreshing-auto-switch-readings)
(the other rows are recorded as usual), and one line goes to stderr. The
profile name in it is cleaned of control characters:

```
warning: the login in ~/.claude changed while profile "work" was probed; its usage was not recorded
```

On such a row, the numbers and any `--raw` text may belong to the login that
replaced it. Run the command again for current numbers.

The check has two known gaps. Closing them would need a lock on the
rotator's ledger, which amplihack does not take:

- A profile that becomes live only after the run started is not detected.
  Its row is probed through its own directory, which by then has no login.
- A swap from A to B and back to A during the run is missed when the ledger
  has no `since` field, because the evidence before and after is the same.

`profile usage` never writes, creates or locks anything under
`<root>/.rotator/` or `<root>/.auto-switch.d/rotate/`; the rotators own those
files.

##### Warnings about `~/.claude`

Each of these is one fixed line on stderr. None of them changes the exit
status, and none marks a profile live when there is doubt.

| Warning | Cause |
|---------|-------|
| `warning: could not read the login rotator's ledger; no profile is marked live in ~/.claude` | `ledger.json` exists but is not trusted (wrong owner, writable by group or others, a symlink, over 64 KiB, or `.rotator` is not a real directory you own), is not valid JSON, or its `live` value is not a usable profile name or names a profile that does not exist. |
| `warning: more than one profile matches the account in ~/.claude; none is marked live` | No ledger, and more than one profile with no login matches the account in `~/.claude.json`. |
| `warning: the login rotator's ledger names a profile that still has its own login; it is not marked live in ~/.claude` | The ledger names a profile whose directory still has a `.credentials.json`. |
| `warning: could not list profiles; no profile is marked live in ~/.claude` | No ledger, and the profile root could not be listed (for example, it is not readable), so no account match was tried. |
| `warning: the login in ~/.claude changed while profile "<name>" was probed; its usage was not recorded` | The evidence for the live profile changed during the probe. See [Swaps during the probe](#swaps-during-the-probe). |

#### JSON output

`--json` prints one JSON array of row objects, in [display order](#ordering).
stdout holds only the JSON; warnings go to stderr. With no profiles it prints
`[]`. This is the stable interface for scripts, and the key table below is
its specification. Keys are only ever added, never renamed or removed, and
there is no version field.

```json
[
  {
    "profile": "work",
    "account": "personal",
    "default": true,
    "leased_to": null,
    "live_in": null,
    "state": "ok",
    "session": {
      "label": "session",
      "used_percentage": 20.0,
      "resets_text": "Oct 16, 7pm (America/Los_Angeles)",
      "resets_at": 1792202400
    },
    "week_all": {
      "label": "all models",
      "used_percentage": 30.0,
      "resets_text": "Oct 22, 2am (America/Los_Angeles)",
      "resets_at": 1792659600
    },
    "week_models": [
      {
        "label": "Fable",
        "used_percentage": 0.0,
        "resets_text": "Oct 22, 2am (America/Los_Angeles)",
        "resets_at": 1792659600
      }
    ],
    "error": null
  }
]
```

| Key | Type | Meaning |
|-----|------|---------|
| `profile` | string | Profile name. |
| `account` | string | The account whose login is live in the profile's directory: the profile's own name unless it [hosts](#leases) another account. The numbers belong to this account. |
| `default` | bool | Whether this is the default profile. |
| `leased_to` | string or `null` | The directory this profile's login is lent to. |
| `live_in` | string or `null` | `"~/.claude"` when this profile's login is [live in `~/.claude`](#logins-in-claude) and its numbers were read from there; otherwise `null`. The key is always present, and at most one row has a non-null value. The value is always the literal `~/.claude`, never the expanded home path. |
| `state` | string | One of the [states](#states). |
| `session` | window or `null` | The `Current session` line. |
| `week_all` | window or `null` | The `Current week (all models)` line. |
| `week_models` | array of windows | Every other `Current week (<model>)` line, in the order printed. Empty if none. |
| `error` | string or `null` | The [reason](#states) for an `error` row, otherwise `null`. |
| `raw` | object | Only with `--raw`: `{"stdout": "...", "stderr": "..."}`. The key is absent, not `null`, without `--raw`. |

A window is `{label, used_percentage, resets_text, resets_at}`. `label` is
`session` for the session window, `all models` for the all-models week, and
the model name as printed (for example `Fable`) for a model week.
`used_percentage` is a number as read (not clamped). `resets_text` is
`null` when the line had no reset. `resets_at` is Unix epoch seconds, or
`null` when there was no [reset text](#what-is-read) or it could not be
resolved. JSON strings are not cleaned; they use JSON escaping.

#### Refreshing auto-switch readings

After printing, `profile usage` records what it read as auto-switch
[readings](#readings), so candidate selection uses fresh numbers rather than
only the last status-line update. The format of `usage.json` does not
change. Nothing is recorded, and the output is the same, unless all of these
hold:

1. Auto-switch is set up: its [effective setting](#toggle-and-precedence) is
   on, or `<root>/.auto-switch.d/` exists. Otherwise nothing is created, and
   without `--json` one line goes to stderr:
   `note: auto-switch not set up; usage record not updated`.
2. None of `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` or
   `CLAUDE_CODE_OAUTH_TOKEN` is set to a non-empty value. With one set,
   `claude` may report the usage of that credential rather than the
   profile's login. The warning names the variable, never its value:
   `auto-switch: ANTHROPIC_API_KEY is set; usage record not updated`.
   Whether or not auto-switch is set up, and with or without `--json`, one
   more line goes to stderr before probing, because every row may show that
   credential's numbers:
   `warning: ANTHROPIC_API_KEY is set; claude may report that credential's usage, not each profile's`.
3. The global state lock can be taken within the usual wait. Otherwise:
   `auto-switch: state is busy; usage record not updated`.
4. No swap is in progress (no `journal.json`). `profile usage` never runs
   recovery. Otherwise:
   `auto-switch: swap in progress; usage record not updated`.

Then, under the lock, for each row:

- Only `ok`, `session_exhausted` and `weekly_exhausted` rows are recorded.
  `spend_limit`, `not_logged_in`, `timeout`, `unparsed` and `error` rows are
  not.
- A row [live in `~/.claude`](#logins-in-claude) is recorded against its own
  profile and account. It is not recorded if the login in `~/.claude`
  changed during the probe.
- The ledger is read again. If the account live in the profile's directory
  changed while the probe ran, the row is not recorded:
  `auto-switch: profile "work" changed accounts during the probe; its usage was not recorded`.
- The reading goes to the account live in the directory (the row's
  `account`), not necessarily the profile with that name.
- `session` becomes `five_hour` and `week_all` becomes `seven_day`.
  Model-specific lines are not recorded. Each window must pass the same
  checks as a status-line reading: a finite percentage, clamped to `0`–`100`,
  and a `resets_at` from one day before to eight days after now. A window
  with no `resets_at` is not recorded.
- If neither window passes, nothing is recorded for that account. If only
  one passes, the other is kept from the stored reading while its reset time
  is still in the future.
- The reading has `observed_at` set to now and `source` set to `statusline`,
  and is merged with the stored one the usual way.

`usage.json` is written once, atomically, at the end. The 300-second
quiet period after a swap does not apply: `/usage` asks the live login
directly, so its answer is current. Any failure while recording prints one
`auto-switch:` warning to stderr and the exit status stays 0.

On platforms without auto-switch (Windows and other non-Unix), there are no
lease marks, `account` is always the profile's own name, and nothing is
recorded or noted.

#### Exit status and refusals

| Status | When |
|--------|------|
| `0` | The probes ran, whatever states they produced, or there are no profiles. Recording failures do not change it. |
| `1` | Setup failed: Docker is active, the profile root is invalid, a name is invalid or does not exist, or `claude` cannot be found. Printed as `error:`/`hint:` like other [errors](#errors). Nothing is probed. |
| `2` | Command-line usage error, such as `--timeout-secs 0`. |

With no profiles, the table form prints
`` no profiles; create one with `amplihack profile add <name>` `` and the JSON
form prints `[]`.

Like `profile run`, `usage` is refused when Docker is activated through the
environment (`AMPLIHACK_USE_DOCKER`), before anything is read or spawned.

#### Privacy

- Credential files are never opened, read or printed.
- Claude's text appears only with `--raw`. Without it, it is dropped after
  parsing and never stored, logged or recorded. `--raw` prints Claude's text
  as it is, so it can include account details such as an e-mail address or
  organisation name. Read it before you paste it into an issue or chat.
- Error reasons are fixed text. Messages contain profile names, states,
  percentages and times only, never token values, e-mail addresses or
  organisation IDs. `account` and `leased_to` are profile names.
- For [logins in `~/.claude`](#logins-in-claude), the only new text is the
  literal label `~/.claude`. Account uuids, e-mail addresses, the rotator
  ledger's other fields and credential paths are never printed. Credential
  files are only checked for existence, without following symlinks.


## Configuration

| Item | Value |
|------|-------|
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Profile root. When set and non-empty it must be absolute, and it overrides the saved root. An empty value counts as unset. See [Profile root](#profile-root). |
| `~/.amplihack/profile-root` | The saved profile root: one absolute path and a trailing newline (mode `0600`). Written by `amplihack profile root <dir>` and by the first `profile add`. Used when `AMPLIHACK_CLAUDE_PROFILE_ROOT` is unset or empty. |
| `~/.amplihack/rotate-root` | The root the most recent `profile rotate start` used, in the same format. Read by `rotate status` and `rotate stop`. See [Rotate root record](#rotate-root-record). |
| `<root>/.auto-switch.d/rotate/root` | The root a rotator started with, written by `rotate start` next to the pid file. Informational. |
| Default root | `~/.amplihack/claude-profiles/`, used when neither the variable nor the saved root is set. |
| `AMPLIHACK_CLAUDE_PROFILE` | Profile name a launch passes to its children so nested launches use the same profile. Set by amplihack on the child process; may also be set by hand. Empty means unset. See [Resolution and environment](#resolution-and-environment). |
| `<root>/<name>/` | Profile directory (mode `0700` on Unix; Windows uses its inherited ACL). |
| Root directory | Created with mode `0700` on Unix; Windows uses its inherited ACL. A symlinked root is refused. An existing custom root is trusted input. |
| `<root>/.default` | Pointer file: only the profile name (mode `0600` on Unix, max 256 bytes). Windows uses its inherited ACL. Written by creating a new temp file (`create_new`, never through a symlink) and renaming it over the pointer. |
| `AMPLIHACK_CLAUDE_AUTO_SWITCH` | `1` turns auto-switch on and `0` turns it off for this process and its children, overriding `<root>/.auto-switch`. Empty means unset. See [Toggle and precedence](#toggle-and-precedence). |
| `<root>/.auto-switch` | Auto-switch settings. See [Settings file](#settings-file). |
| `<root>/.auto-switch.d/` | Auto-switch state. See [State directory](#state-directory). |

### Seeding

`add` calls the store's `seed_shared_assets`, which copies from the source
`$HOME/.claude` into the new profile by value. The allowlists are fixed
constants:

| Constant | Contents |
|----------|----------|
| `SHARED_ASSET_FILES` | `CLAUDE.md` |
| `SHARED_ASSET_DIRS` | `agents`, `commands`, `output-styles`, `rules`, `skills` |
| `SHARED_SETTINGS_KEYS` | top-level `settings.json` keys: `alwaysThinkingEnabled`, `attribution`, `cleanupPeriodDays`, `hooks`, `includeCoAuthoredBy`, `language`, `model`, `outputStyle`, `permissions`, `respectGitignore`, `spinnerVerbs`, `statusLine` (12 keys) |

Rules:

- `settings.json` is parsed and only `SHARED_SETTINGS_KEYS` are written to the
  profile's `settings.json`. If nothing survives the filter, no file is
  written. `env`, `apiKeyHelper` and all unknown keys are dropped.
- On Unix, directories are created `0700` and files `0600`; Windows uses
  inherited ACLs. Files use `create_new` (`O_EXCL` on Unix), so nothing is
  overwritten and no destination symlink is followed.
- The 12-key list is kept in sync with the code by `tests_docs_sync.rs`, which
  checks both directions (every constant key is documented here and in the
  how-to guide, and no extra key is documented).
- A symlinked source root fails with `SymlinkRefused`. Nested symlinks and
  special files (FIFOs, sockets, devices) are skipped. On Unix the source root
  is opened exactly once with `O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC` and
  that descriptor anchors the whole copy. The five allowlisted directories,
  `CLAUDE.md` and `settings.json` are opened with `openat(root_fd, name, ...)`
  and `O_NOFOLLOW`, never by absolute path, so replacing `~/.claude` (or any
  ancestor) with a symlink after the anchor is taken cannot redirect the copy:
  the descriptor still pins the original directory. Ancestors of the root are
  resolved once, at anchor time. Each directory is opened with
  `O_NOFOLLOW|O_DIRECTORY` and entries are read and stat'ed relative to it
  (`openat`/`fstatat` with `AT_SYMLINK_NOFOLLOW`), so a directory swapped for a
  symlink during the copy is never followed. Files are opened `O_NOFOLLOW`
  and `O_NONBLOCK`, verified as regular files with `fstat` on the opened
  descriptor, and read through that same descriptor with a bounded read. Entry
  names must be a single path component. A missing `CLAUDE.md` or
  `settings.json` is skipped. Only the final component of the root is
  no-follow: a symlinked ancestor of `~/.claude` is resolved normally, once.
  Windows opens source files with `FILE_FLAG_OPEN_REPARSE_POINT` and rejects
  reparse-point handles; its path-based directory traversal still permits
  same-user ancestor/directory races. Other non-Unix platforms keep the
  path-based copy without descriptor-relative hardening.
- A missing `~/.claude` source produces an empty profile. An existing source
  that is not a directory fails the add and rolls back the new profile.
- The profile root cannot be inside the shared source tree. This is checked
  before the root is created, including for missing path suffixes, to prevent
  recursive seeding.
- Caps: nesting is limited to 16 levels, a seed to 5,000 entries, each file to
  64 MiB, and `settings.json` to 1 MiB; exceeding a cap is an error. Special
  files are skipped without being opened, so a FIFO cannot block the copy.
- Excluded entirely: `.credentials.json`, `.claude.json`, `history*`,
  `projects`, `sessions`, `todos`, `statsig`, `shell-snapshots`, `ide`,
  and every other entry not on the allowlists (so plugins and MCP server
  settings are not carried over).
- Invalid `settings.json` yields `InvalidSharedSettings`. A seeding error
  removes the private staging directory so no profile is published; if cleanup
  also fails, the error reports both failures (never a pre-existing profile).
- Values are never logged; messages contain paths and counts only.
- No sync after creation: later edits to `~/.claude` do not update profiles.

## Name rules

Pattern `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$` (validated before any path join). Additionally rejected: `..`
anywhere, trailing `.`, path separators, NUL, and Windows-reserved names
(`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`, also with an extension,
case-insensitive). `add` rejects names that differ only by case from an existing profile on
every platform, so `Work` and `work` can never coexist.

## Profile root

The *profile root* is the directory that holds every profile, the default
pointer and the auto-switch and rotate state. Every `amplihack profile`
command, every launch with a profile and the background rotator choose one
root, in this order:

| Order | Source | Shown as |
|-------|--------|----------|
| 1 | `AMPLIHACK_CLAUDE_PROFILE_ROOT`, when set and non-empty | `source: env` |
| 2 | `~/.amplihack/profile-root`, when the file exists | `source: config` |
| 3 | `~/.amplihack/claude-profiles` | `source: default` |

`~` is `$HOME`. If `HOME` is unset or empty and the variable is not set,
every profile command fails with `InvalidRoot`.

Rules:

- An empty `AMPLIHACK_CLAUDE_PROFILE_ROOT` counts as unset, so a wrapper
  script that leaves `AMPLIHACK_CLAUDE_PROFILE_ROOT=` behind still gets the
  saved root.
- A non-empty variable always wins. The saved file is not opened at all, so a
  damaged `profile-root` never blocks a command run with the variable set.
- A non-empty variable that is relative, holds control characters or has a
  `..` component is an error, `InvalidRoot`. amplihack does not fall back to
  the saved root or the default. Earlier releases accepted an absolute
  variable with a `..` component, such as `/home/you/x/../profiles`; set the
  path without the `..` instead.
- A saved file that exists but is invalid is an error, `InvalidSavedRoot`.
  amplihack does not fall back to the default. This applies to every command
  that resolves the root, including `amplihack claude --profile` and the
  other launch paths.
- A missing saved file means no saved root, and the default is used. Any other
  error reading it is `InvalidSavedRoot`.

The saved root is what makes profiles work in shells that do not read
`~/.bashrc`: cron jobs, systemd units, `tmux new-session -d` commands,
`ssh host cmd` and `env -i`. See
[Use profiles from non-interactive shells](../howto/claude-profiles.md#use-profiles-from-non-interactive-shells).

### Root command

```
amplihack profile root
amplihack profile root <dir>
```

`root` runs before the root is resolved for any other purpose, so it works
when the saved file is damaged.

With no argument, `root` prints the root in use and its source, and exits 0:

```
$ amplihack profile root
root: /home/you/.claude-profiles (source: config)
```

If the saved file is invalid, it exits 1 with the `InvalidSavedRoot` error
and hint instead.

With `<dir>`, it saves `<dir>` to `~/.amplihack/profile-root`, prints where it
saved it, and exits 0:

```
$ amplihack profile root /home/you/.claude-profiles
saved profile root: /home/you/.claude-profiles (/home/you/.amplihack/profile-root)
```

`root <dir>` makes these checks in order, and writes nothing if any of them
fails:

| Check | Error |
|-------|-------|
| `<dir>` is non-empty, absolute and valid UTF-8, holds no control characters and has no `..` component. `~` is not expanded: a quoted `'~/x'` is relative and is refused. | `InvalidRoot` |
| One trailing `/` is removed (`/` itself is kept). | - |
| If `<dir>` exists, it is not a symlink. | `SymlinkRefused` |
| `<dir>` is not `~/.claude` or inside it. | `SharedSourceContainsRoot` |
| The file can be written safely (see [Saved root file format](#saved-root-file-format)). | `InvalidSavedRoot` |

More behaviour:

- `<dir>` is saved as given, apart from the trailing `/`. It is not
  canonicalized, and the directory is not created; `profile add` creates it.
- A `<dir>` with trailing whitespace, such as `"/srv/x "`, is refused with
  `InvalidSavedRoot` (`has leading or trailing whitespace`) and nothing is
  written. A leading space makes the path relative, so it gives `InvalidRoot`.
- A valid `<dir>` replaces any saved value, including an invalid one. The old
  file is never read, so `root <dir>` is how you repair a damaged
  `profile-root`.
- If `AMPLIHACK_CLAUDE_PROFILE_ROOT` is set and non-empty, `root <dir>` still
  saves and exits 0, and also prints to stderr:

  ```
  note: AMPLIHACK_CLAUDE_PROFILE_ROOT=/tmp/scratch overrides the saved root in this shell
  ```

- There is no option to clear the saved root. Delete
  `~/.amplihack/profile-root` to go back to the default.
- Profiles under the old root are not moved. Move them yourself, or point the
  root back.

### Saved on the first add

`profile add <name>` resolves the root first, so a damaged saved file makes
`add` fail before it creates anything. After the profile is created, if
`~/.amplihack/profile-root` does not exist, `add` saves the root it used,
whether that came from the variable or the default, and prints:

```
recorded profile root /home/you/.claude-profiles in /home/you/.amplihack/profile-root
```

If the file already exists, `add` leaves it alone and prints nothing about
it. Later `add`s therefore never change the saved root.

```bash
AMPLIHACK_CLAUDE_PROFILE_ROOT=/home/you/.claude-profiles amplihack profile add work
env -u AMPLIHACK_CLAUDE_PROFILE_ROOT amplihack profile list
```

```
root: /home/you/.claude-profiles (source: config)
  work
```

If the save fails, `add` prints
`warning: could not record profile root: <reason>` to stderr and still exits
0, because the profile was created. No other command saves the root on its
own.

### Saved root file format

`~/.amplihack/profile-root` and `~/.amplihack/rotate-root` each hold one
absolute path followed by a single `\n`.

| Rule | Detail |
|------|--------|
| Write | Atomic: a new temporary file in `~/.amplihack`, flushed to disk, then renamed over the target. Mode `0600`. The temporary file is created without following symlinks. A value that is not valid UTF-8, that fails the content checks, or that has leading or trailing whitespace is refused before anything is written, because the reader trims whitespace and would return a different root. |
| `~/.amplihack` | Created with mode `0700` when missing. An existing directory keeps its mode; it also holds other amplihack settings such as `~/.amplihack/config`. |
| Read | Never through a symlink, at most 4096 bytes. Leading and trailing whitespace is trimmed. |
| Valid content | Non-empty, UTF-8, absolute, no control characters, no `..` component. |
| Permissions | The file must be a regular file that is not writable by group or others. Ownership is not checked. |

An invalid `profile-root` gives `InvalidSavedRoot`. The message names the file
and one of these reasons, or the operating system's error text:

| Reason | Cause |
|--------|-------|
| `empty` | The file holds only whitespace. |
| `not an absolute path` | The path is relative, including `~/...`. |
| `not UTF-8` | The bytes are not valid UTF-8. |
| `larger than 4096 bytes` | The file is too large to be a path. |
| `contains control characters` | The path holds a control character, such as an escape. |
| `contains a .. component` | The path has a `..` component. |
| `is a symlink` | The file is a symlink. It is not followed. |
| `not a regular file` | The file is a directory or a special file. |
| `writable by others` | The file's mode grants write to group or others. |

The message never repeats the file's contents:

```
error: saved profile root in "/home/you/.amplihack/profile-root" is invalid: not an absolute path
hint: fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>
```

A `profile-root` left behind by `sudo amplihack profile add` with `HOME` kept
is owned by root and cannot be replaced by `profile root <dir>`. Remove it
with `sudo rm ~/.amplihack/profile-root`, then save the root again.

On Windows the file is written through a temporary file and a rename, and read
with the same size limit. The mode check does not apply; Windows ACLs are out
of scope.

### Root line

`profile root`, `profile list` and `profile rotate status` print the root on a
line of its own:

```
root: <path> (source: env|config|default)
```

`rotate status` prints `root: <path> (pinned by running rotator)` instead when
it found the rotator through the [rotate root record](#rotate-root-record).
The path is escaped for the terminal, so control characters cannot write
escape sequences.

`profile list` prints the root line first, then the profiles:

```
$ amplihack profile list
root: /home/you/.claude-profiles (source: config)
* personal
  work
```

With no profiles, the root line is followed by
`` no profiles; create one with `amplihack profile add <name>` ``. Scripts
that read `list` should skip the first line, which always starts `root: `.

## Resolution and environment

`amplihack claude`, `amplihack launch` and `amplihack profile run` pick a
profile in this order:

1. `--profile <name>` (or the `<name>` given to `profile run`). The inherited
   variable and the pointer are not read, so a dangling or corrupt pointer
   does not block it.
2. An inherited, non-empty `AMPLIHACK_CLAUDE_PROFILE`. The pointer is not
   read, even if it is dangling.
3. `<root>/.default`
4. none — `CLAUDE_CONFIG_DIR` is left exactly as inherited.

When a profile resolves, three variables are set on the child `Command` only
(never `set_var`), and only for Claude-compatible binaries. This applies to
every command that starts Claude with a profile: `amplihack claude`,
`amplihack launch`, `amplihack profile run` and `amplihack profile login`.

| Variable | Value |
|----------|-------|
| `CLAUDE_CONFIG_DIR` | `<root>/<name>`, overriding any ambient value |
| `AMPLIHACK_CLAUDE_PROFILE` | `<name>` |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | `<root>`, only when `AMPLIHACK_CLAUDE_PROFILE_ROOT` was set and non-empty in the launching process (that is, when the root's source is `env`); otherwise the child's environment is left as inherited, so the child reads the same saved root or uses the same default. An inherited empty value passes through unchanged and still counts as unset. |

Copilot and Codex children get none of these, but variables already in the
environment pass through them unchanged.

A root that came from the saved file or the default is not exported, so a nested launch reads
`~/.amplihack/profile-root` again, and fails with `InvalidSavedRoot` if the
file has since been damaged. If you change the saved root while a
session runs, launches nested in that session use the new root. Set
`AMPLIHACK_CLAUDE_PROFILE_ROOT` before starting the session to pin it.

### Nested launches keep the parent's profile

Every `amplihack claude` started inside a profiled session (recipe agent
steps, `multitask`/orch workstreams, a nested `amplihack claude` run by hand)
inherits `AMPLIHACK_CLAUDE_PROFILE` and so resolves the same profile at tier 2.
The pin holds for the whole tree of sessions:

- A later `amplihack profile switch` only rewrites the pointer. Running
  sessions, and every launch started from them, keep their profile.
- A parent started with `--profile work` while the pointer names `personal`
  runs every descendant as `work`.

How the name reaches each kind of descendant:

| Launch path | How the variables arrive |
|-------------|--------------------------|
| Direct child, `amplihack recipe run` | Inherited environment (the recipe runner is detached with `setsid` but keeps the environment). |
| Recipe agent steps | Inherited environment. `recipe-runner-rs` builds each step's environment from its own, and never drops `AMPLIHACK_*` variables when it trims the environment to fit its budget. The only `AMPLIHACK_*` value it removes is one whose `KEY=value` string is over the kernel's 131,072-byte per-string limit, which no child could receive anyway; a profile name and root are far smaller. In a bash step, values the recipe itself supplies (context keys and their uppercase aliases) that are too large for the environment become shell-local variables and are not exported (checked at the revision pinned in `claude-plugin/recipe-runner.rev`, `4d5187e`). Agent steps take no per-step environment. In bash steps, a step's `env:` block can set any variable, including these, and so can the runner's uppercase export of context keys (see [Recipe context cannot set the profile](#recipe-context-cannot-set-the-profile)). |
| `multitask` / orch workstreams | Inherited environment: each workstream's `run.sh` is started with the parent's full environment. As a backup, the generated `run.sh` also exports `AMPLIHACK_CLAUDE_PROFILE` and `AMPLIHACK_CLAUDE_PROFILE_ROOT`, single-quoted, when they are set and non-empty, so a `run.sh` re-run by hand or on resume uses the same profile. `CLAUDE_CONFIG_DIR` is not exported; the nested launch rebuilds it from the name and checks it again. If either variable is set but is not valid UTF-8, the workstream is refused before any script is written (the error names the variable, not its value), because leaving it out would send the nested launch to the saved or default root. |

Only the name and the root are passed on. Each nested launch resolves the
name against the root and runs the full check below again, so an inherited
`CLAUDE_CONFIG_DIR` is never trusted on its own.

An inherited value is checked exactly like a flag: it is parsed with the
[name rules](#name-rules) without trimming (so `" work"` is invalid), resolved
inside the root, and re-validated before spawn. Any failure — an invalid
name, a value that is not valid UTF-8, a profile that no longer exists, a
symlinked or out-of-root directory — aborts the launch with the
inherited-profile error. It never falls back to the pointer or to no profile.

You can set the variable by hand to pin a profile for one shell or command:

```bash
AMPLIHACK_CLAUDE_PROFILE=work amplihack recipe run default-workflow
```

A stale export in a shell startup file acts as a per-shell default that beats
the pointer; `unset AMPLIHACK_CLAUDE_PROFILE` removes it.

`amplihack recipe run` and `amplihack multitask` do not resolve a profile
themselves. Started from a plain shell with no inherited
`AMPLIHACK_CLAUDE_PROFILE`, each agent step resolves the pointer on its own,
so a `profile switch` part way through moves later steps to the new profile.
Start them from inside an `amplihack claude` session, or set
`AMPLIHACK_CLAUDE_PROFILE` as above, to pin one profile for the whole run.

Fleet and remote VM launches do not pass the profile on, so nested launches
inside them use the pointer.

Copilot and Codex launches don't set the variables themselves, but they don't
remove an inherited value either. A nested `amplihack claude` started from a
Copilot session that inherited `AMPLIHACK_CLAUDE_PROFILE` uses that profile
and checks it again. Without it, the launch uses the pointer.

### Recipe context cannot set the profile

`amplihack recipe run` refuses any context key whose uppercase form starts
with `AMPLIHACK_`, on both the `--set` and the `--context-file` path, before
the runner starts:

```text
recipe context key "amplihack_claude_profile" is reserved: the AMPLIHACK_ namespace cannot be set from recipe context
```

Only the key is printed, never its value. Keys that merely contain
`amplihack` (such as `use_amplihack`) are allowed.

Known limitation: the recipe runner itself still exports every top-level
context key to bash steps under its uppercase name. A recipe's own YAML
`context:` default or a step `output:` named `amplihack_claude_profile` (or
`amplihack_claude_profile_root`) is not checked by amplihack and can override
the pinned value for an `amplihack claude` run inside a bash step. The name
and directory are still validated, so this can only select one of your own
profiles. The runner fix is tracked in
[rysweet/amplihack-recipe-runner#146](https://github.com/rysweet/amplihack-recipe-runner/issues/146).

### Pre-spawn check

Immediately before spawn, the profile path is
re-resolved and re-validated: it must be a real directory (not a symlink),
its canonical path must equal the expected path under the profile root, and
it must be owned by the current user with no group/other permission mode bits
(`mode & 0o077 == 0`: `0700` passes; `0755`, `0750` and `0705` fail). ACLs are
not inspected. Only the profile directory is checked, not its parents.
If the check fails, run `chmod go-rwx <profile-directory>` on Unix.
Any failure aborts the launch. This check applies only to profile launches;
Claude and Copilot launches without a profile are unchanged.
Ownership and group/other mode-bit checks are available on Unix. On other
platforms, profile launches still require a real, contained directory, but
the operating system does not expose the same checks through this command.

The profile selects a configuration directory; it does not change Claude's
authentication environment. Claude Code documents separate directories for
claude.ai logins and API-key accounts, but two Claude Console sign-ins without
API keys are not isolated because their credentials live outside the
configuration directory. An inherited `ANTHROPIC_API_KEY` can also select API
billing instead of a subscription login. See the official
[multiple-account](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts)
and [environment variable](https://code.claude.com/docs/en/env-vars)
documentation.

`profile run` applies the same check as `launch --profile`: if Docker is
active (including `AMPLIHACK_USE_DOCKER`), it fails with the unsupported-mode
error before resolving or creating anything. It never silently drops the
profile.

`--profile` together with `--docker` or `--auto` fails with an
unsupported-mode error before anything is spawned. This includes Docker
activated through the environment (`AMPLIHACK_USE_DOCKER`). With Docker
activated that way and only a default pointer, the pointer is not applied and
a warning is printed.

An inherited, non-empty `AMPLIHACK_CLAUDE_PROFILE` is refused the same way.
With `--docker`, `--auto` or Docker activated through the environment, the
launch fails with the inherited-profile error and reason `not applied with
--docker, --auto or Docker`:

```
error: inherited AMPLIHACK_CLAUDE_PROFILE "work": not applied with --docker, --auto or Docker
hint: unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto
```

The refusal happens before the value is parsed and before the profile root
is opened, so the value drives no filesystem reads. A value that is not
valid UTF-8 gets the same refusal, not an invalid-name error. An empty value
counts as unset and does not trigger it.

These modes never set, change or re-check `CLAUDE_CONFIG_DIR`. A launch
that gets past the refusal keeps whatever `CLAUDE_CONFIG_DIR` it inherited,
with no pre-spawn check. Inside a profiled session that is still the
parent's profile directory, so unsetting `AMPLIHACK_CLAUDE_PROFILE` alone
would run an `--auto` launch on the parent's account, not the default one.
Unset both variables for that one command:

```bash
env -u AMPLIHACK_CLAUDE_PROFILE -u CLAUDE_CONFIG_DIR amplihack claude --auto -- -p "summarise the README"
```

A `--docker` launch does not forward `CLAUDE_CONFIG_DIR` into the
container, but unsetting both is still the safe form to use.

Amplihack's own orchestration (recipe agent steps, `multitask`/orch
workstreams) never passes `--docker` or `--auto` to nested launches. It does
not clear `AMPLIHACK_USE_DOCKER`, though. A profiled session can carry a
truthy `AMPLIHACK_USE_DOCKER` if you export it inside the session, or if it
was exported when the session started but the Docker daemon was not running
(env-activated Docker only applies when the daemon is up). Once the daemon is
running, every nested launch of a recipe or `multitask` run started from that
session is refused with the error above. Unset `AMPLIHACK_USE_DOCKER` before
starting the run. Inside a container `AMPLIHACK_USE_DOCKER` has no effect,
so nested launches there are not refused.

A default pointer on its own is never an error in these modes: with
`--docker` or `--auto` it is not read, and with Docker activated through the
environment it is skipped with the warning above.

A symlinked profile root is refused by every subcommand and by the pointer
reads/writes; a missing root is treated as empty.

## Literal argument forwarding

`run <name> -- <args>` passes args as an argv vector with no shell, so
`amplihack profile run work -- "; rm -rf ~"` hands that string to Claude
as one literal argument and executes nothing.

## Auto-switch

Auto-switch moves work from an account that is close to its usage limit to
another profile with quota left. It is off by default. The
[how-to](../howto/claude-profiles.md#switch-accounts-automatically) walks
through using it; this section defines its behaviour.

### What "off" means

With the switch off and no `<root>/.auto-switch.d/` directory:

- the lease and recovery guards make one metadata check (does
  `<root>/.auto-switch.d/` exist?) and stop;
- launches, nested substitution and the rerun supervisor read
  `AMPLIHACK_CLAUDE_AUTO_SWITCH` once and, unless it decides the setting, make
  at most one bounded read of `<root>/.auto-switch` (4096 bytes).

Nothing else happens. No directory is created, no lock is taken, no usage is
recorded, no profile is substituted, no output is buffered and no step is
rerun. Behaviour is the same as without the feature. The status-line wrapper,
if installed, runs the original status line and records nothing.

If `<root>/.auto-switch.d/` exists, the [lease](#leases) and
[recovery](#session-end-and-recovery) checks run whatever the toggle says,
so a login moved while the switch was on stays protected after `off`.

### Auto-switch commands

| Command | Behavior |
|---------|----------|
| `auto-switch on [--threshold N] [--exclude a,b]` | Writes `enabled=on`, plus the threshold and exclusions if given, and prints `auto-switch: on (threshold N%)`. If any profile's [status-line wrapper](#status-line-wrapper) is `missing` or `stale`, prints the `profiles without the wrapper` [note](#status-notes) to stderr. |
| `auto-switch off` | Writes `enabled=off`. The threshold and exclusions are kept. Logins in running sessions are not moved; they return when each session ends. |
| `auto-switch status` | **Read-only.** Takes no lock, runs no recovery and changes nothing on disk. Prints the effective setting and where it comes from (`env`, `file`, `default` or `corrupt`), the threshold, the exclusions, whether in-session swaps are available, one row per profile (latest readings, where its login is, wrapper state), the hosted directories, an invariant line, and then any [notes](#status-notes). Prints profile names, percentages and times only. |
| `auto-switch threshold N` | Sets the threshold, `1` to `100`. Keeps `enabled` and `exclude`. |
| `auto-switch exclude [names…]` | Replaces the exclusion list. With no names, clears it. Each name must pass the [name rules](#name-rules); it does not have to exist (`status` lists names that do not). Keeps `enabled` and `threshold`. |
| `auto-switch install` | Adds the [status-line wrapper](#status-line-wrapper) to every profile that does not have it. Leaves a stale wrapper alone and reports it. Prints the root it acts on. |
| `auto-switch repair` | If `<root>/.auto-switch.d/` exists, takes the global lock and runs [recovery](#session-end-and-recovery). Then does everything `install` does, and also rewrites stale wrappers (old or moved binary path, wrapped twice, edited by hand) to the canonical one. Prints the install lines, then one `recovery:` line per thing recovery did, or `recovery: nothing to do`. This is the only command whose purpose is recovery. |

`status` prints `wrapper` as `installed`, `missing`, `stale` (an older or
edited wrapper command, one wrapped twice, or one pointing at a different
binary) or `unsafe` (the binary is in a location `install` would refuse).
The `in-session swap` line is `available (Linux)` or
`unavailable (<reason>)`; see [Platform support](#platform-support).
The invariant line is `invariant: ok (every login is in exactly one place)`,
`invariant: pending recovery` when a swap journal is waiting, or
`invariant: PROBLEM: <problem>` naming the profile and the problem (a
damaged ledger or journal, a journal marked `inconsistent`, or a login that
is not where the ledger says).

`install` and `repair` print one line per profile: the outcome, the name and
a detail.

| Outcome | Detail |
|---------|--------|
| `unchanged` | (none) |
| `installed` | `(no previous status line)`, `(wrapped existing status line)` or `(restored the saved status line)` |
| `stale` (`install` only) | `(wrapper pointed at a different binary)` or `(wrapper command differs from the canonical one)`, followed by `— run: amplihack profile auto-switch repair` |
| `repaired` (`repair` only) | `(wrapper pointed at a different binary)` or `(wrapper command differs from the canonical one)` |

The `recovery:` lines `repair` prints are
`recovery: rolled back an interrupted swap in <dir>`,
`recovery: finished an interrupted swap in <dir>` and
`recovery: returned the login of <account> hosted in <dir> (session ended)`.

Both `install` and `repair` are idempotent: a second run leaves every
`settings.json` byte-identical. Neither resolves or writes `~/.claude`.
`profile add` does not install the wrapper; run `install` after it.

#### Status notes

`status` prints these notes after the invariant line, each as
`note: <text>` on its own line, only when they apply:

| Note | When |
|------|------|
| `interrupted swap pending — run: amplihack profile auto-switch repair` | `journal.json` exists. `status` never acts on it. |
| `stale lease: <dir> (session ended) — run: amplihack profile auto-switch repair` | A hosted directory is no longer live. `status` does not return it. |
| `ignored AMPLIHACK_CLAUDE_AUTO_SWITCH value (use 1 or 0); the file decides` | The variable is set to something other than `1`, `0` or empty. |
| `profiles without the wrapper: <names> — run: amplihack profile auto-switch install` | A profile's wrapper is `missing` or `stale`. |
| `excluded but missing: <names>` | An exclusion names no existing profile. |
| `profiles <a> and <b> are signed in to the same account; their readings are not reliable` | Two profiles' `oauthAccount.accountUuid` values are equal: from the profile's own `.claude.json`, or from its saved block while its directory hosts another account. The UUID itself is not printed. |

Every command takes the global state lock only when it needs it, and exits
with status 1 on error with the usual `error:`/`hint:` lines.

### Settings file

`<root>/.auto-switch` holds `key=value` lines, written in this order:

```
enabled=on
threshold=90
exclude=review,shared
```

| Key | Values | Default |
|-----|--------|---------|
| `enabled` | `on` or `off`. Required. | — |
| `threshold` | Integer `1` to `100`, in percent. | `90` |
| `exclude` | Comma-separated profile names, may be empty. | empty |

It is written like `.default`: a new temp file in the same directory
(`create_new`, mode `0600`, never through a symlink), synced, renamed over the
file, and the directory synced. Rewrites produce identical bytes for an
identical setting.

The file is *corrupt*, and the switch counts as off, if it is larger than
4096 bytes, not UTF-8, not a regular file, not owned by the current user,
has an unknown or repeated key, has no `enabled`, or has an out-of-range
value. `status` shows `switch: off (corrupt)`.

What each command writes depends on the current file:

| Current `.auto-switch` | `on` / `off` | `threshold N` / `exclude …` |
|------------------------|--------------|-----------------------------|
| valid | rewrites it with the new `enabled` | rewrites it, keeping the other keys |
| missing | creates it | creates it with `enabled=off`, defaults for the key not given |
| corrupt regular file owned by you | replaces it; `threshold` and `exclude` start from their defaults unless given | refused with `AutoSwitchConfig`; run `on` or `off` first |
| owned by another user | refused with `AutoSwitchConfig` | refused with `AutoSwitchConfig` |
| symlink or not a regular file | refused with `AutoSwitchConfig` | refused with `AutoSwitchConfig` |

A file is never written through a symlink, and a file owned by someone else
is never replaced.

The file is read again on every check and never cached, so a change applies
to running sessions from their next check.

### Toggle and precedence

| `AMPLIHACK_CLAUDE_AUTO_SWITCH` | Effective setting |
|--------------------------------|-------------------|
| `1` | on |
| `0` | off |
| unset or empty | the file's `enabled`; off if the file is missing or corrupt |
| anything else | the file decides; one warning per process goes to stderr, and `status` shows the [note](#status-notes) |

Only `1` and `0` are recognised. The file is read on every check, so a
change to it applies to running sessions from their next check. The variable
is inherited when a session starts and cannot be changed afterwards; while it
is set, it overrides the file, so `=1` keeps switching on even after
`auto-switch off`. The threshold and exclusions always come from the file.

### Readings

A *reading* is the latest usage seen for one account:

```json
{
  "version": 1,
  "accounts": {
    "work": {
      "five_hour": { "used_percentage": 93.0, "resets_at": 1760630400 },
      "seven_day": { "used_percentage": 61.0, "resets_at": 1761040800 },
      "observed_at": 1760621000,
      "source": "statusline"
    }
  }
}
```

That is `<root>/.auto-switch.d/usage.json`. Times are Unix epoch seconds.
`source` is `statusline` for readings from the wrapper or from
[`profile usage`](#refreshing-auto-switch-readings), and
`usage_limit_failure` for a reading recorded from a step that hit its usage
limit (see [Usage-limit reruns](#usage-limit-reruns)).

- **Parsing.** Status-line input is capped at 1 MiB and only its
  `rate_limits` object is read. `resets_at` is accepted as epoch seconds
  (integer or float) or RFC 3339. A window is dropped if its percentage is
  not a finite number or its `resets_at` is not within one day before to
  eight days after now. Percentages are clamped to `0`–`100`. Input without
  `rate_limits` records nothing.
- **Reset.** A window whose `resets_at` is at or before now counts as 0%.
- **Attribution.** A reading belongs to the account whose login is live in
  that directory (per the ledger), not to the directory's owner.
- **Identity.** An account is identified by the name of the profile that
  owns its login. Leases, the ledger and readings all use that name;
  `oauthAccount` is only used to patch what Claude displays. Two profiles
  signed in to the same Anthropic account are not supported: they are
  treated as two accounts, and `status` warns about them.
- **After a swap.** For 300 seconds after a swap, readings from that
  directory are not recorded and do not trigger another swap. After that, a
  reading identical to the last reading before the swap (same percentages and
  reset times) is still ignored, because it describes the previous account.
  Readings recorded by `profile usage` skip this quiet period; see
  [Refreshing auto-switch readings](#refreshing-auto-switch-readings).
- **Merging.** The reading with the newer `observed_at` wins; on a tie, the
  one with the higher usage wins.
- **Errors.** A damaged `usage.json` is treated as empty and overwritten by
  the next reading.

### Candidate selection

An account is *exhausted* when the higher of its two adjusted windows is
at or above the threshold (`max(five_hour, seven_day) >= threshold`). An
*adjusted* window is one whose `resets_at` is at or before now, which counts
as 0%. A window without `resets_at` is used as read. An account with no
reading is not exhausted. Exhausted accounts are never chosen.

An account is *eligible* when it is not excluded, not the current account,
not [leased](#leases), and not exhausted, plus the extra conditions for an
[in-session swap](#in-session-swap) or a
[nested launch](#nested-launches-and-auto-switch).

Eligible accounts are ordered:

1. accounts with a reading, by the higher of their two adjusted windows,
   lowest first, then by name;
2. accounts with no reading, by name.

The first one is chosen. A known reading below the threshold always beats
no reading. If none is eligible, nothing is switched. The
exclusion list only stops an account being chosen; an excluded account that
is current can still be switched away from.

### Status-line wrapper

`install` and `repair` set each profile's `statusLine` to a hidden command:

```json
"statusLine": {
  "type": "command",
  "command": "'/home/ryan/.local/bin/amplihack' profile statusline --root '/home/ryan/.claude-profiles' --owner 'work'",
  "padding": 0
}
```

The binary path is the one that ran `install`. Each value is single-quoted
for `sh`; a path containing NUL, a newline or non-UTF-8 bytes is refused.
The original `statusLine` object is saved in
`<root>/.auto-switch.d/statusline/<name>.json` and run by the wrapper.

| The profile's `statusLine` is… | `install` | `repair` |
|--------------------------------|-----------|----------|
| the canonical wrapper | does not write | does not write |
| missing | installs the wrapper; uses a saved original if one exists | same as `install` |
| another command | saves it as the original, then installs the wrapper | same as `install` |
| an old, moved, edited or doubly wrapped wrapper (contains `profile statusline`) | does not write; reports `stale` | rewrites it to the canonical wrapper, keeping the saved original (never saving a wrapper as the original) |

The original `padding` is kept. A missing `settings.json` is created at
`0600`. Writes use a temp file, `rename` and a directory sync. Other keys and
their values are kept; key order may change. `install` checks each profile
directory like a launch does, refuses a symlinked `settings.json`, and never
resolves or writes `~/.claude`. It refuses to install when the amplihack
binary, or any directory above it, is writable by group or others or is owned
by someone other than you or root. Group write is accepted only for your own
private group (a group named like your user with no listed members, the
usual layout with a umask of `002`). It warns when the binary is inside a
`target/` directory. There is no uninstall command; restore the saved
original by hand if you need to.

When Claude runs the wrapper, it:

1. checks that `--root` is absolute and private, that `--owner` is a valid
   name, and that `CLAUDE_CONFIG_DIR` resolves to `<root>/<owner>`. If any
   check fails, it only runs the original status line;
2. waits at most 50 ms for the state lock, then runs
   [recovery](#session-end-and-recovery) if `<root>/.auto-switch.d/`
   exists and, if the switch is on, records the reading. If the lock is
   still busy after 50 ms, it skips recovery, the update and the swap check
   for this run; the next wrapper run, launch, login or `repair` runs them.
   It never delays or changes the output;
3. runs the original command with `sh -c`, the same standard-input bytes,
   working directory and environment, and copies its standard output through
   byte for byte;
4. runs the [in-session swap](#in-session-swap) check.

The saved original in `statusline/<name>.json` is run only if it passes the
[state directory](#state-directory) checks (a regular file owned by you,
mode `0600`, not a symlink, at most 64 KiB, valid JSON). If it fails them, the
wrapper does not run it, prints nothing to standard output (Claude shows an
empty status line), and writes one stderr line naming the path and telling
you to run `amplihack profile auto-switch repair`.

If the `--root` it was installed with is not an absolute path, cannot be
resolved, or fails the profile-root checks (a real directory owned by you and
not writable by other users), the wrapper does nothing else: it runs neither
the original status line nor any bookkeeping, prints nothing to standard
output, and writes one stderr line, `auto-switch: status line disabled:
profile root <path>: <reason>`.

#### Trigger point and coverage

The wrapper is the only passive source of `rate_limits`: hooks receive no
usage data. It is therefore the trigger for in-session swaps, which happen
only in sessions that render a status line (interactive sessions).
Non-interactive `claude -p` launches may never run the status line. They are
covered instead by [nested substitution](#nested-launches-and-auto-switch)
when they start and by [usage-limit reruns](#usage-limit-reruns) when they
fail.

The wrapper never writes to standard output itself. Its exit code is the
original command's, or `0` if there is none or it was not run. Its own errors, including a
panic, are one line on stderr naming paths only.

### In-session swap

Runs from the wrapper, on Linux only, when the switch is on and the session's
account is exhausted. The target must be eligible and, in addition, its login
must be in its home slot, and if that home slot is its own profile directory,
no running process may be using that directory.

The *home slot* of account `X` is `<root>/X/.credentials.json` when directory
`X` holds `X`'s own login, and `<root>/.auto-switch.d/parked/X/.credentials.json`
when directory `X` is hosting another account. At any moment each account's
login is in exactly one of: its home slot, or one directory where it is live.

The swap holds two locks: the global lock `<root>/.auto-switch.d/lock`, and
Claude's own refresh lock `<live dir>/.oauth_refresh.lock`, taken with a
non-blocking `mkdir`. If either is busy, the swap is skipped until the next
update. The refresh lock is never broken, and is removed afterwards only if
it is still the one amplihack created (recovery also removes it after a
crash, using the inode recorded in the journal).

1. Finish any interrupted swap, then write `journal.json` and sync it.
2. Move `<live dir>/.credentials.json` to the outgoing account's home slot
   with `renameat2(RENAME_NOREPLACE)`. Sync both directories.
3. Move the target's home slot to `<live dir>/.credentials.json` the same way,
   set its modification time to now so Claude reloads it, and sync.
4. Update `ledger.json`, set `oauthAccount` in `<live dir>/.claude.json` to the
   target's saved block, delete the journal and sync.

Before each move the file must be a regular file owned by you, readable by
nobody else (no group or other mode bits), with exactly one hard link.

After a swap the wrapper writes one line to stderr (never to the status
line):

```
amplihack: profile "work" reached the auto-switch threshold; the session in profile "work" now uses the login of profile "personal"
```

If either move fails because the destination exists (`EEXIST`), a file is
where the state says none should be, and amplihack cannot tell which copy is
current. The swap stops at once: nothing else moves and nothing is rolled
back, so a move already done in step 2 stays done. The journal is kept with
`phase` set to `inconsistent`. Recovery never acts on an `inconsistent`
journal, so launches and logins of both profiles fail with
`AutoSwitchInvariant` until it is [repaired by hand](../howto/claude-profiles.md#recovery).

If the filesystem cannot do the move (`EXDEV`, `EINVAL`, `ENOSYS`), the swap
rolls back any move already done, deletes the journal, and disables swaps for
that directory; `status` says so. A login is never copied as a fallback.

#### `oauthAccount`

`oauthAccount` is the account name and organisation Claude shows in
`/status`. amplihack saves each profile's block to `accounts/<name>.json`
while that profile holds its own login. In step 4 it patches only that key
in `<live dir>/.claude.json`:

- the file is opened without following symlinks and read up to 32 MiB;
- if the file is missing, larger than that, a symlink, or not a JSON
  object, the patch is skipped silently and only the login moves;
- if no saved block exists for the incoming account, the patch is skipped
  with a warning that ends `/status may name the previous account`;
- if the saved block is damaged, the patch is skipped with the warning
  `auto-switch ignored damaged account details: <error>`;
- if `oauthAccount` already equals the saved block, nothing is written;
- otherwise the new content goes to a temp file in the same directory, which
  is synced and renamed over the file, keeping the file's existing mode; if
  that write fails, it warns `auto-switch could not update
  <dir>/.claude.json: <error>`;
- if the file changes while it is being patched (inode, size or mtime
  differ), it retries once and then skips with the warning
  `<dir>/.claude.json kept changing; account details not updated`.

`.credentials.json` decides which account is live; `oauthAccount` is only a
display. Claude keeps `.claude.json` in memory and rewrites it, so a write
by Claude can bring back the previous account's name in `/status` until
Claude next rewrites the file or the session restarts. Requests still use
the moved login. Other cached account fields are left alone.

### Leases

A directory is *live* when a process of yours, other than amplihack itself
and not a zombie, has a `CLAUDE_CONFIG_DIR` in `/proc/<pid>/environ` that
resolves to it. Relative values are resolved against that process's working
directory. A process whose environment cannot be read but whose command line
mentions `claude` or `amplihack` makes every hosted directory count as live.
Any doubt counts as live.

Account `X` is *leased* to directory `Y` when `Y` is not `X` and the ledger
says `X` is live in `Y`. These are refused before `claude` is spawned:

| Action | Refused when | Error |
|--------|--------------|-------|
| Launch, `run` or `login` of profile `X` | `X` is leased elsewhere | `Leased` |
| Launch or `run` of profile `X` | directory `X` is hosting another account, unless the launch [joins](#joining-a-hosted-session) the session using it | `HostsForeignAccount` |
| `login X` | directory `X` is hosting another account (no exception) | `HostsForeignAccount` |
| `remove X` | `X` is in the ledger, has a parked login, or hosts another account | `Leased` or `HostsForeignAccount` |

#### Joining a hosted session

A launch *joins* the session in directory `X` only when all of these hold:

- the profile was inherited (tier 2 of
  [resolution](#resolution-and-environment)), not given by `--profile`,
  `profile run` or the default pointer;
- the launcher's own `CLAUDE_CONFIG_DIR` is set; and
- that value, canonicalised, is equal to the canonical path of `<root>/X`.

The comparison is between whole canonical paths. It never uses the value of
`AMPLIHACK_CLAUDE_PROFILE`, a profile name, or a string prefix. A fresh shell
that runs `export AMPLIHACK_CLAUDE_PROFILE=work` without the session's
`CLAUDE_CONFIG_DIR` does not join and is refused with `HostsForeignAccount`.

A launch that joins shares the session's current login. The checks run under
the global lock and again just before spawn.

Leases are interlocks against accidents. They are not a security boundary
against other processes running as you.

### Session end and recovery

These steps run at the start of every launch, `profile login`, wrapper run
and `auto-switch repair`, and when a launched Claude exits (before amplihack
exits, on success and on error). `auto-switch status` does not run them; it
only reports what they would do. They run whether the switch is on or off,
as long as `<root>/.auto-switch.d/` exists:

1. **Interrupted swap.** If `journal.json` exists, the state is worked out
   from which of the two known paths each login is at. If the incoming login
   has not reached the live directory, the swap is rolled back; if it has,
   step 4 of the swap is finished. A journal whose `phase` is `inconsistent`,
   or a layout that matches neither case, is reported as inconsistent and
   nothing is moved. Recovery replays forward or back by moving files; it
   never copies one.
2. **Return.** Every hosted directory that is no longer live (no running
   process of yours uses it; see [Leases](#leases)) is returned:
   the borrowed login goes back to its home slot, the owner's parked login
   goes back into its directory, the owner's `oauthAccount` is restored, and
   the ledger entry is removed. If the borrowed login is missing (for example
   after `/logout`), the owner's login is still returned.

A damaged, unknown-version or unknown-field `ledger.json` or `journal.json`
is never overwritten. Launches and logins for the profiles involved fail
with `AutoSwitchInvariant` and nothing is moved. The how-to's
[Recovery](../howto/claude-profiles.md#recovery) section explains how to
repair it by hand.

### Nested launches and auto-switch

Applies only when the switch is on and the profile was inherited
(tier 2 of [resolution](#resolution-and-environment)). An explicit
`--profile` and the default pointer are never redirected.

If the account live in the inherited directory is exhausted, the launch uses
the first eligible candidate that holds its own login at home and is not
hosting another account. The child gets `AMPLIHACK_CLAUDE_PROFILE=<candidate>`
and `CLAUDE_CONFIG_DIR=<root>/<candidate>`, the candidate gets the normal
pre-spawn check, and one line naming both profiles goes to stderr. Nested
launches never move logins. With no candidate, the inherited profile is used
as today. `--docker` and `--auto` are still refused with an inherited profile.

`multitask` workstreams apply the same choice to the `AMPLIHACK_CLAUDE_PROFILE`
line their `run.sh` exports, and each nested launch checks again when it
starts.

### Usage-limit reruns

Applies when the switch is on, the profile was inherited, there is no
`--profile`, Claude's arguments contain `-p` or `--print`, and the launch is
not `--docker` or `--auto`. That is how recipe agent steps run.

The launch runs Claude under a supervising amplihack process:

- stderr streams live; its last 64 KiB is kept;
- stdout is written to a private temp file under
  `<root>/.auto-switch.d/rerun/` and printed once, after the final attempt;
- `SIGTERM` and `SIGHUP` are forwarded to the attempt; `SIGINT` reaches it
  through the terminal.

An attempt counts as hitting a usage limit only when all of these hold:

1. it was not killed by a signal;
2. it exited with a non-zero status; and
3. the recipe runner's own classifier, applied to the kept stderr tail
   followed by the last 64 KiB of buffered stdout, returns its
   `usage_limit` class. That is the same classifier, with the same markers
   (`session limit`, `usage limit`, `usage_limit_reached`, `quota exceeded`,
   `monthly limit`) and the same precedence, the runner uses to end a run
   with a usage-limit message. Work markers are checked first, then
   environmental ones, then usage-limit ones, so a tail that also matches a
   work or environmental marker is not a usage limit. For example, a
   `claude -p` stdout tail that contains `permission denied` anywhere is
   classed as environmental and the step is not rerun, even if it also says
   `usage limit`. Two usage-limit-like messages are environmental for the
   same reason and never cause a rerun: `credit balance is too low` (an
   environmental marker, checked first) and `disk quota exceeded` (which
   matches the environmental marker before `quota exceeded`).

Using one classifier means the supervisor and the runner cannot disagree: a
step the runner would end the run for is always offered a rerun first. A
false match costs one extra attempt, which is no worse than today's
termination.

The reset time comes from Claude's own message: if a line starts with
`Claude AI usage limit reached|` followed by digits, those digits are the
reset epoch. The last such line in the stderr tail is used, else the last in
the stdout tail. The text must start the line: a line such as
`model: Claude AI usage limit reached|1` does not supply a reset time.
Otherwise it is five hours from now. The
account's five-hour window is recorded as 100% used until that time, and
never later than five hours from now. A note naming both profiles goes to
stderr, and the step runs again under the next eligible profile. Each
remaining profile is tried at most once per step. When none is left, the
last attempt's output and exit code are passed back unchanged, and the
recipe runner reports the usage limit and ends the run as it does today.

Reruns happen at the step-launch layer: inside the nested
`amplihack claude -p` that each agent step runs. The recipe runner's retry
loop, its executor and its failure classifier are unchanged; the runner only
sees the final attempt.

The supervised attempt runs with `AMPLIHACK_AUTO_SWITCH_SUPERVISED=1`, which
stops it supervising itself; amplihack removes that variable before it starts
Claude. If the supervisor cannot start, the launch runs unsupervised, with a
warning.

The buffer is `rerun/<pid>-<n>.out`, where `<pid>` is the supervisor and `<n>`
the attempt. It is deleted when the attempt is rerun or its output has been
printed. If the supervisor is killed first, the next recovery run deletes the
buffer once no process with that `<pid>` is running, and any buffer older
than 24 hours regardless. Until then it holds whatever the step printed,
at mode `0600` in a `0700` directory.

A rerun starts the step again from the beginning, so anything the attempt
that hit the limit had already done (a commit, a push, a comment) may be done
again.

### State directory

Everything is under `<root>/.auto-switch.d/` (directories `0700`, files
`0600`). Profile names cannot start with `.`, so it never clashes with a
profile and never appears in `profile list`.

| Path | Contents |
|------|----------|
| `lock` | Global exclusive file lock for every ledger, journal, usage and login change. |
| `ledger.json` | `{"version":1,"hosts":{"<dir>":{"live":"<account>","swapped_at":<epoch>,"pre_swap_fingerprint":"<sha256 hex>"}}}`. `pre_swap_fingerprint` is `null` when there was no reading before the swap. Lists only directories hosting another account. |
| `journal.json` | The swap or return in progress: `op`, `dir`, `from`, `to`, `phase` (a step name, or `inconsistent` after `EEXIST`), plus `started_at` and `pid` for diagnosis. |
| `usage.json` | Readings, as shown in [Readings](#readings). |
| `parked/<name>/.credentials.json` | The home slot of a profile whose directory is hosting another account. |
| `accounts/<name>.json` | The profile's saved `oauthAccount` block. No tokens. |
| `statusline/<name>.json` | The profile's original `statusLine` object. |
| `rerun/` | Buffered stdout of supervised steps, `<pid>-<n>.out`; see [Usage-limit reruns](#usage-limit-reruns) for how long each is kept. |

Every state file has `"version": 1` and rejects unknown fields. State files
are capped at 64 KiB. Each name in them is checked with the
[name rules](#name-rules) before it is used in a path. A damaged
`accounts/` or `statusline/` file is ignored with a warning and not
overwritten.

Before any state write or login move, the root must be owned by you and not
writable by group or others (with the same private-group exception as
`install`), and every path must be inside the root, owned by
you and not a symlink.

### Platform support

| Platform | In-session swap | Nested substitution and reruns |
|----------|-----------------|-------------------------------|
| Linux, `/proc/self/environ` readable, login in `.credentials.json` | yes | yes |
| Linux where `/proc/self/environ` cannot be read (for example `hidepid`) | no; `status` says so and nothing is moved | yes |
| Other Unix (no `/proc`) | no; `status` says so and nothing is moved | yes |
| macOS with logins in the Keychain | no; no credentials file to move, skipped silently with a `status` note | yes |
| Windows and other non-Unix | no | no; every `auto-switch` command is refused with `AutoSwitchUnsupported`, and every other auto-switch path does nothing |

An in-session swap only happens when `<live dir>/.credentials.json` exists.
Moving a file is the only primitive that keeps one copy of a login, so there
is no fallback for other storage.

## Rotate

`profile rotate` keeps the login in `~/.claude` on an account with capacity,
preferring weekly capacity that expires soonest, by moving logins between
`~/.claude` and the profiles. The
[how-to](../howto/claude-profile-rotation.md) walks through using it; this
section defines its behaviour. It is separate from [auto-switch](#auto-switch),
which swaps logins inside profiled sessions, but shares its toggle, its
exclusion list and its state lock.

### Rotate commands

```
amplihack profile rotate [--dry-run] [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--min-headroom N]
amplihack profile rotate start  [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--min-headroom N] [--interval SECS] [--fast-at N] [--fast-interval SECS]
amplihack profile rotate watch  [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--min-headroom N] [--interval SECS] [--fast-at N] [--fast-interval SECS]
amplihack profile rotate stop
amplihack profile rotate status
```

| Command | Behaviour |
|---------|-----------|
| `rotate` | Runs one [cycle](#rotate-cycle): recover, measure, decide, and swap if needed. Prints its log lines to stdout and appends them to the log. Runs whether the toggle is on or off. |
| `rotate --dry-run` | Measures and decides, then prints the `cycle` line, one [candidate line](#rotate-candidate-lines) per measured candidate, and the `would ...` line to stdout. Takes no lock, writes no file, moves nothing, and skips both the [prototype check](#rotate-prototype-coexistence) and [recovery](#rotate-recovery). Reports a pending journal as `would recover an interrupted swap (phase <phase>)` and decides as if it were not there. An inconsistent journal is reported as `would alert: rotate recovery is inconsistent`, and a damaged one as `would alert: the rotate journal is damaged`. |
| `rotate start` | Checks for the prototype, runs recovery under the lock, warns on stderr if auto-switch has been turned off, refuses if a verified rotator is already running, then starts `rotate watch` as a detached process and writes the pid file. The check, the start and the pid file all happen under the state lock, so two `start`s cannot both start one. Fails with `rotate: lock busy` when the lock stays held for 2 seconds, and with the auto-switch error when auto-switch recovery is inconsistent. Pins the child to its root and records that root; see [Rotate root record](#rotate-root-record). Prints `rotate: started (pid <pid>), checking every <interval>s`, `log: <path>` and `root: <path>`, and returns at once. |
| `rotate watch` | Runs a cycle every `--interval` seconds, or every `--fast-interval` seconds while the live login is near its limit (see [Rotate pacing](#rotate-pacing)), in the foreground until `SIGTERM` or `SIGINT`. A signal is never acted on during a swap; see [Signals](#rotate-signals). Refuses if another verified rotator is running. Writes only to the log. |
| `rotate stop` | Finds the rotator as described in [Rotate root record](#rotate-root-record). Sends `SIGTERM` to a [verified](#rotate-daemon) rotator, waits up to 10 seconds for it to exit, then removes the pid file and prints `rotate: stopped (pid <pid>)`. |
| `rotate status` | **Read-only.** Prints the [status lines](#rotate-status) and exits 0. |

`--dry-run` belongs to the one-shot form only. Tuning flags go after `start`
or `watch`; `amplihack profile rotate --margin 3 start` is a usage error and
exits with status 2.

On platforms without Unix file semantics, such as Windows, every form fails
with `RotateUnsupported` before doing anything else. That is the only use of
`RotateUnsupported`. On a Unix system or filesystem that cannot exchange two
files atomically, the swap is refused with `RotateRefused`; see
[Rotate swap](#rotate-swap).

### Rotate settings

| Flag | Variable | Default | Range |
|------|----------|---------|-------|
| `--session-at N` | `AMPLIHACK_ROTATE_SESSION_AT` | `85` | `1`-`100` (%) |
| `--week-at N` | `AMPLIHACK_ROTATE_WEEK_AT` | `90` | `1`-`100` (%) |
| `--margin N` | `AMPLIHACK_ROTATE_MARGIN` | `5` | `0`-`100` (points) |
| `--upgrade-margin N` | `AMPLIHACK_ROTATE_UPGRADE_MARGIN` | `30` | `0`-`100` (points) |
| `--hold SECS` | `AMPLIHACK_ROTATE_HOLD` | `3600` | `0` or more (seconds); `0` disables the hold |
| `--min-headroom N` | `AMPLIHACK_ROTATE_MIN_HEADROOM` | `50` | `0`-`100` (points of session headroom); `0` turns the floor off |
| `--interval SECS` | `AMPLIHACK_ROTATE_INTERVAL` | `600` | `1`-`86400` (seconds); `start` and `watch` only |
| `--fast-at N` | `AMPLIHACK_ROTATE_FAST_AT` | `70` | `1`-`100` (%). The flag is for `start` and `watch` only; the variable also sets when the [on-limit cycle](#rotate-on-a-usage-limit) treats the live login as near its limit. |
| `--fast-interval SECS` | `AMPLIHACK_ROTATE_FAST_INTERVAL` | `60` | `1`-`86400` (seconds); `start` and `watch` only. A value above `--interval` is lowered to `--interval` without an error. |

A flag beats its variable, and the variable beats the default. An empty
variable counts as unset. Values are unsigned decimal integers: a sign,
whitespace, a fraction or an overflow is an error. The error names the flag or
variable and shows the value escaped and cut to 32 characters:

```
error: invalid rotate setting --session-at "eighty": expected a whole number from 1 to 100
hint: run `amplihack profile rotate --help` for the accepted values
```

`start` resolves every setting and passes them to the background process as
explicit flags, so the variables are read once, when `start` runs. The
background process sets no `AMPLIHACK_ROTATE*` variable in the environment of
the `claude` processes it starts.

The thresholds and margins are independent of the auto-switch `threshold`.
The auto-switch `exclude` list applies to rotation.

The [rotation that `amplihack recipe run` starts after a usage limit](#rotate-on-a-usage-limit)
runs in the recipe process, not in the background rotator. It resolves
every setting in the table above from the variables in that process and the
defaults; flags given to `start` or `watch` do not reach it. Three settings
change what it does:

- `min-headroom` and `margin` decide which candidates are eligible;
- `fast-at` decides whether the live login is near enough to its limit for
  the failure to force a swap.

The thresholds, the upgrade margin, the hold and both intervals have no
effect on it. An invalid value in any of the variables in the table still
stops it, with `no swap` and reason `invalid rotate settings: <error>`.

### Rotate cycle

Every non-dry-run cycle runs these steps in order:

1. **Prototype check.** See [Prototype coexistence](#rotate-prototype-coexistence).
2. **Toggle.** `watch` only: if auto-switch has been turned off, by
   `auto-switch off` or `AMPLIHACK_CLAUDE_AUTO_SWITCH=0`, or its settings
   file is damaged (see [Toggle and precedence](#toggle-and-precedence)),
   write `idle: auto-switch is off` once per change from on to off, and skip
   the rest. With no setting at all (no `<root>/.auto-switch` and no
   variable), rotation runs.
3. **Lock and recover.** Take `<root>/.auto-switch.d/lock`, waiting up to
   2 seconds. If it is busy, log `skip: state lock busy` and end the cycle.
   Under the lock, run auto-switch [recovery](#session-end-and-recovery), then
   [rotate recovery](#rotate-recovery), then [identify](#rotate-live-profile)
   the live profile and the [candidates](#rotate-candidates). Release the
   lock.
4. **Measure.** Probe the live login and every candidate in parallel, as
   [`profile usage`](#how-a-profile-is-probed) does. The live login is probed
   with `CLAUDE_CONFIG_DIR` removed from the environment, so `claude` uses
   `~/.claude` and `~/.claude.json`. Candidates are probed with
   `CLAUDE_CONFIG_DIR=<root>/<name>`. Claude's text is parsed and dropped;
   only percentages, reset times and states are kept. No lock is held.
5. **Decide.** See [Decision rule](#rotate-decision-rule).
6. **Revalidate and swap.** `watch` first checks for a pending signal and,
   if there is one, ends the cycle and exits without taking the lock (see
   [Signals](#rotate-signals)). Take the lock again. If the live account, the
   candidate list, the running sessions, the auto-switch ledger or the parked
   logins changed since step 3, log `skip: state changed during measurement`
   and end the cycle. Otherwise run the [swap](#rotate-swap).
7. **Record.** Write the log lines and the ledger's `last_cycle`. When the
   cycle stays or raises the no-capacity ALERT and the live profile is known,
   also record it as `live`, with its `account`. The
   [candidate lines](#rotate-candidate-lines) are written before the `swap`,
   `ALERT` or stay outcome on every swap, every no-capacity ALERT and every
   [on-limit](#rotate-on-a-usage-limit) cycle. On a cycle that stays they are
   written only when they differ from the previous cycle's, so `watch` does
   not repeat the same block every minute while it polls fast.

`watch` then sleeps for the delay [Rotate pacing](#rotate-pacing) gives.

### Rotate decision rule

*Headroom* is `100 - max(session_used, week_used)`, rounded down to a whole
point.

| Probe result | Session used | Week used | Headroom |
|--------------|--------------|-----------|----------|
| `ok` with both windows | as reported | as reported | as computed |
| `ok` with no session window | `0` | as reported | as computed |
| `ok` with no week window | | | probe failed |
| `session_exhausted` or `weekly_exhausted` | | | `0` |
| any other [state](#states) | | | probe failed |

A candidate's *gain* is its headroom minus the live login's headroom. A
failed live probe counts as headroom `0`.

The decision first sets the mode:

- **Threshold mode** when the live probe failed, the live login is
  exhausted, `session_used >= session-at`, or `week_used >= week-at`.
- **Upgrade mode** otherwise.

#### Eligibility

Each candidate is checked in this order. The first check it fails is its
verdict; a candidate that passes all of them is *eligible*.

| Check | Verdict when it fails |
|-------|-----------------------|
| It is not `session_exhausted` or `weekly_exhausted`. | `rejected: exhausted` |
| Its probe did not fail. | `rejected: probe failed` |
| Its session headroom is at least `min-headroom`. | `rejected: session headroom <h> < min <m>` |
| Threshold mode: `gain >= margin`. Upgrade mode: `gain > 0` and `gain >= upgrade-margin`. | `rejected: gain <g> < margin <m>` or `rejected: gain <g> < upgrade margin <m>` |
| Upgrade mode only: `now - swapped_at >= hold`, or there is no earlier swap. | `rejected: held until <time>` |

The gain is checked for each candidate on its own. A candidate with a lower
score that passes every check is still chosen when a higher-scoring one
fails the gain check.

#### Score

Among the eligible candidates the one with the highest *score* wins. Ties go
to the name that sorts first by byte value. Every other eligible candidate
gets the verdict `rejected: lower score than <winner>`, and the winner gets
`chosen: highest expiry-weighted score`.

| Value | Definition |
|-------|------------|
| `session_headroom` | `100 - session_used`, rounded down, kept in `0`-`100`. |
| `week_left` | `100 - week_used`, kept in `0`-`100`. |
| `days_to_reset` | Days until the week window resets: `(resets_at - now) / 86400`, but never less than `1/24` (one hour). When the probe gave no weekly reset time, `7.0`, marked `(assumed)`. |
| `score` | `min(week_left / days_to_reset, session_headroom)`. |

The score prefers weekly capacity that will be lost soonest if it is not
used, as long as the session has room for it. Its two sides have different
units (points per day against points); the session side is a cap, not a
conversion. With no weekly reset time a score is at most about `14.3`
(`100 / 7`).

Example, with the live login at `s=92% w=41%` (headroom `8`) in threshold
mode and the default settings:

| Candidate | Session used | Week used | Week resets in | `session_headroom` | `week_left` | `score` | Gain |
|-----------|--------------|-----------|----------------|--------------------|-------------|---------|------|
| `personal` | `5%` | `60%` | 1 day | `95` | `40` | `40.0` | `40 - 8 = 32` |
| `spare` | `0%` | `20%` | 6 days | `100` | `80` | `13.3` | `80 - 8 = 72` |

Both are eligible, and `personal` wins with `40.0`. `spare` has more
headroom, but its weekly capacity will still be there in six days;
`personal`'s is gone tomorrow.

#### Outcome

| Mode | An eligible candidate exists | Outcome |
|------|------------------------------|---------|
| Threshold | yes | Swap to the winner, `reason=threshold`. |
| Threshold | no, or there are no candidates | ALERT `no-capacity`. No swap. |
| Upgrade | yes | Swap to the winner, `reason=upgrade`. |
| Upgrade | no | Stay. |

`swapped_at` and `previous` (the profile swapped out) come from the
[ledger](#rotate-state-directory). During the hold no upgrade happens at
all, so `previous` cannot come straight back by upgrade. Threshold swaps are
never held.

The `min-headroom` floor stops a swap to a profile whose session is nearly
used up, such as one at `77%` or `86%` session use. It applies in both
modes. `--min-headroom 0` turns it off and restores ordering by gain and
score alone.

### Rotate candidate lines

The cycle explains its choice with one line per measured candidate, sorted by
name. The log and `--dry-run` use the same text:

```
candidate <name>: score=<n.n|-> session_headroom=<n|-> week_left=<n|-> days_to_reset=<n.nn>[ (assumed)] -> <verdict>
```

A value the probe could not give, as for an exhausted or failed candidate,
is shown as `-`. `days_to_reset` is always shown, with two decimal places,
so the one-hour floor prints as `0.04`. The verdict is one of:

| Verdict | Meaning |
|---------|---------|
| `chosen: highest expiry-weighted score` | The swap target. |
| `rejected: exhausted` | The candidate is `session_exhausted` or `weekly_exhausted`. |
| `rejected: probe failed` | The candidate could not be measured. |
| `rejected: session headroom <h> < min <m>` | Below the `min-headroom` floor. |
| `rejected: gain <g> < margin <m>` | Threshold mode: not enough better than the live login. |
| `rejected: gain <g> < upgrade margin <m>` | Upgrade mode: not enough better than the live login. |
| `rejected: held until <time>` | Upgrade mode: inside the hold after the last swap. `<time>` is `YYYY-MM-DDTHH:MM:SSZ`. |
| `rejected: lower score than <name>` | Eligible, but `<name>` scored higher. |

The dry run for the example above prints:

```
cycle live=work work:s=92%,w=41% personal:s=5%,w=60% spare:s=0%,w=20%
candidate personal: score=40.0 session_headroom=95 week_left=40 days_to_reset=1.00 -> chosen: highest expiry-weighted score
candidate spare: score=13.3 session_headroom=100 week_left=80 days_to_reset=6.00 -> rejected: lower score than personal
would swap work->personal live s=92% w=41% target s=5% w=60% reason=threshold
```

A profile skipped before measurement, such as one with a running session, has
no candidate line; its `skip:` line gives the reason.

### Rotate pacing

After each cycle `watch` sleeps for:

| Live reading after the cycle | Delay |
|------------------------------|-------|
| `session_exhausted` or `weekly_exhausted` | `--fast-interval` |
| Measured with `max(session_used, week_used) >= fast-at` | `--fast-interval` |
| Anything else, including a failed probe or a cycle that measured nothing | `--interval` |

After a swap, the live reading is the new live login's reading from that
cycle, usually low, so `watch` returns to `--interval`. With the defaults,
`watch` checks every 60 seconds once either window of the live login reaches
`70%`, and every 600 seconds while both are below it. The
[on-limit cycle](#rotate-on-a-usage-limit) uses the same test to decide
whether the live login is near its limit. `--fast-interval` is never longer than
`--interval`: a larger value is lowered to `--interval` without an error.

### Rotate on a usage limit

When an agent step of `amplihack recipe run` fails on a login session or
weekly limit, the recipe process runs one rotate cycle at once instead of
waiting for the next poll, and retries the run once if the login changed.
[Retry after rotating the login](transient-failure-retry.md#retry-after-rotating-the-login)
describes the recipe side. This section describes the cycle.

#### When it is enabled

| `AMPLIHACK_ROTATE_ON_LIMIT` | Enabled |
|-----------------------------|---------|
| `1` | Yes. |
| `0` | No. |
| unset or empty | Only when `rotate status` would show `rotate: running`. `stale` and `unverifiable` count as not running. |
| any other value | As if unset, after one warning on stderr: `warning: ignoring AMPLIHACK_ROTATE_ON_LIMIT="<value>": expected 1 or 0`. The value is escaped and cut to 32 characters. |

The variable is read only from the process environment. A recipe context key
cannot set it; see [Recipe context cannot set the profile](#recipe-context-cannot-set-the-profile).
On platforms without Unix file semantics, such as Windows, it is never
enabled, even with `AMPLIHACK_ROTATE_ON_LIMIT=1`, and the value is not
checked, so no warning is printed.

Even when enabled, nothing is swapped and the run is not retried when:

- auto-switch is off, as `watch` judges it in [Rotate cycle](#rotate-cycle)
  step 2: turned off by `auto-switch off` or `AMPLIHACK_CLAUDE_AUTO_SWITCH=0`,
  or `<root>/.auto-switch` is damaged and `AMPLIHACK_CLAUDE_AUTO_SWITCH` is
  not `1`. A missing `<root>/.auto-switch` does not stop it;
- the [prototype rotator](#rotate-prototype-coexistence) holds its lock;
- the profile root has no profiles.

#### What the cycle does

1. **Already swapped.** Read the ledger. If its `swapped_at` is at or after
   the time the failed attempt started, another run or the background
   rotator has already moved off the login that failed. Return
   `already swapped` with no probe and no swap. The same check runs again
   under the lock, after recovery, and writes
   `skip: already swapped since the failed attempt started (live=<name>)`.
   A ledger that cannot be read gives `no swap`, never a swap.
2. **Settings.** Resolve every [setting](#rotate-settings) from the
   variables and defaults. An invalid value in any of them, including one
   the cycle does not use, gives `no swap` with reason
   `invalid rotate settings: <error>`.
3. **Cycle.** Run the [cycle](#rotate-cycle) with the live login treated as
   exhausted, which puts the decision in threshold mode. Probes are still run.
   The live probe decides whether the forced exhaustion stands:

   | Live probe | Result |
   |------------|--------|
   | Exhausted, failed, or `max(session_used, week_used) >= fast-at` | The live reading is replaced by exhausted (headroom `0`) and the cycle decides as usual. |
   | Measured with both windows below `fast-at` | No swap. The cycle writes `skip: live login is not near a limit (s=<n>% w=<n>%); no forced swap`. |

   The second row stops a step's error text from forcing a swap away from a
   login that is not near its limit.
4. **Busy.** If the lock is busy or the state changed during measurement,
   wait 2 seconds and try again, for at most 30 seconds in total. Each try
   rereads the ledger as in step 1. When time runs out, return `no swap`
   with reason `rotator busy`.

The hold does not apply, because the decision is in threshold mode. Only
`min-headroom` and `margin` decide which candidates are eligible; `fast-at`
is used only in step 3.

#### Results

| Result | Recipe retries the run | Meaning |
|--------|------------------------|---------|
| `swapped` | Yes | This cycle swapped `<from>-><to>`. |
| `already swapped` | Yes | The ledger shows a swap since the failed attempt started. |
| `no swap` | No | One of the reasons below. |
| `disabled` | No | Not enabled; the recipe prints exactly what it printed before this feature. |

| `no swap` reason | When |
|------------------|------|
| `auto-switch is off` | Auto-switch is turned off, or `<root>/.auto-switch` is damaged and `AMPLIHACK_CLAUDE_AUTO_SWITCH` is not `1`. |
| `the external rotator is running` | The prototype holds its lock. |
| `no profiles are configured` | `AMPLIHACK_ROTATE_ON_LIMIT=1`, but the root has no profiles. |
| `the rotate ledger cannot be read` | The ledger fails the hardened read. |
| `invalid rotate settings: <error>` | A variable from [Rotate settings](#rotate-settings) is invalid, including one the cycle does not use. `AMPLIHACK_ROTATE_ON_LIMIT` is not one of them; an invalid value there only gives its warning. |
| `live login not near a limit` | Step 3, second row. |
| `no profile has capacity` | No candidate is eligible. |
| `rotator busy` | Step 4 ran out of time. |
| `<ALERT text>` | Any other ALERT, such as `no login in ~/.claude; no swap`, without the `ALERT: ` prefix. |
| `<error>` | Any other cycle error, such as `swap refused: <reason>`. |

The cycle writes to the [log](#rotate-log) as a `watch` cycle does, and never
to stdout, which is reserved for the recipe's result JSON. Its results and
reasons hold profile names only, never account IDs or e-mail addresses.

### Rotate live profile

The live profile is the one whose login is in `~/.claude`. Accounts are
compared by `oauthAccount.accountUuid`, or by `emailAddress` (case-insensitive)
when either side has no uuid, read from `$HOME/.claude.json` and from each
`<root>/<name>/.claude.json`. No credentials file is read. An account value
over 320 bytes or containing control characters counts as missing. Account
values are never logged or printed.

| Evidence, checked in order | Result |
|----------------------------|--------|
| `~/.claude/.credentials.json` is missing or not a regular file | ALERT `no-login`. No swap. A login is never moved into an empty `~/.claude`. |
| The rotate ledger exists and its `account` matches `~/.claude.json` | Its `live` profile. |
| The rotate ledger exists and its `account` does not match | ALERT `hand-change`. No swap. |
| No rotate ledger; the prototype ledger `<root>/.rotator/ledger.json` is trusted (the trust rules in [Logins in `~/.claude`](#logins-in-claude)), names a profile that has nothing at `<root>/<name>/.credentials.json`, and that profile's `.claude.json` either has no account or has the account in `~/.claude.json` | That profile. |
| No rotate ledger; the prototype ledger exists but fails the row above | The prototype ledger is ignored and logged as `skip: prototype ledger ignored (<reason>)`; the next two rows decide. |
| No usable ledger; exactly one profile with no login matches the account in `~/.claude.json` | That profile. |
| No usable ledger; more than one profile with no login matches | ALERT `ambiguous`. No swap. |
| None of the above | *Adopt*: the login has no profile. |

The prototype ledger has no account field, so these checks are what stop an
old prototype ledger from naming the wrong profile. That can happen when
someone ran `/login` in `~/.claude` and then deleted the rotate ledger, as
the [hand-change steps](../howto/claude-profile-rotation.md#swap-refused)
direct: once the named profile has been logged in again, it has a login and
the prototype ledger is ignored. The `<reason>` is `profile has a login`,
`account does not match`, `untrusted` or `invalid name`.

When the live login is to be adopted, the swap first creates a profile with
`profile add` under the first free name of `default-original`,
`default-original-2`, `default-original-3` and so on, and writes the
`oauthAccount` block from `~/.claude.json` into its `.claude.json`. `add`
copies no credentials file. Both steps run under the lock, after the
pre-swap checks and before `j0`; the outgoing login is then moved into that
profile by the swap. Adoption happens only as part of a swap, never on a
cycle that stays.

The profile is created before `j0`, so creating it is not journalled; a journal that recovery rolls back removes an adopted profile that still holds no login. If the process stops after `add` but before the
`oauthAccount` block is written, the new profile exists with no login and no
account. Nothing has moved, so no recovery is needed, but the account match
cannot find that profile, and the next swap adopts again under the next free
name, such as `default-original-2`. Remove the empty profile with
`amplihack profile remove default-original --yes`. If the process stops
after the block is written, the profile has no login and the right account,
so the account match finds it and the next swap uses it.

### Rotate candidates

A profile is a candidate unless any of these holds, checked in this order.
Every reason but `live` is logged as a `skip:` line: by `watch` only when the
set of skipped profiles changes, and by the one-shot command on every run.

| Skip reason | Condition |
|-------------|-----------|
| `live` | It is the live profile. |
| `no login` | `<root>/<name>/.credentials.json` is missing, a symlink, not a regular file, not owned by you, readable by other users, or has more than one link. |
| `excluded` | It is in the auto-switch `exclude` list. |
| `session running` | A running process has `CLAUDE_CONFIG_DIR=<root>/<name>` (the same `/proc` scan auto-switch uses). If `/proc` cannot be scanned, every profile is skipped and `skip: cannot see running sessions; no candidates` is logged once. |
| `leased` | Its login is on loan to another profile's session in the auto-switch ledger. |
| `hosting` | Its directory hosts another account's login in the auto-switch ledger. |
| `parked` | `<root>/.auto-switch.d/parked/<name>` exists. |
| `no account` | Its `.claude.json` has no usable `oauthAccount`, so `~/.claude.json` could not be updated after a swap. |

### Rotate swap

`L` is `~/.claude/.credentials.json`, `T` is `<root>/<to>/.credentials.json`
and `S` is `<root>/<from>/.credentials.json`, where `<from>` is the live
profile (or the adopted one).

Before anything is written, every check below must pass; otherwise the swap
fails with `RotateRefused` and nothing changes:

- the profile root is private (as for auto-switch state writes); `$HOME` is
  absolute, a directory rather than a symlink, and owned by you; `~/.claude` is owned by you and
  not writable by group or others; both profile directories pass the usual
  directory check;
- `L` and `T` are regular files owned by you, with no group or other mode
  bits and exactly one link (checked
  with `lstat`, never opened);
- `S` does not exist;
- `~/.claude` and the profile root are on the same device;
- the platform has an atomic exchange. Only Linux
  (`renameat2(RENAME_EXCHANGE)`) qualifies. macOS has `renamex_np`, but Claude
  keeps macOS logins in the Keychain, so there is no `~/.claude/.credentials.json`
  to exchange and a cycle stops earlier with the `no-login` ALERT. Any system
  other than Linux, macOS included if a credentials file is there, is refused
  with `atomic exchange is not supported here`.

Then the steps run in order. The step names are the `phase` values in the
journal.

| Step | Action |
|------|--------|
| `j0` | Write `journal.json` with `from`, `to`, `adopt`, the inode numbers of `L` (`live_ino`) and `T` (`target_ino`), and `$HOME`. Then take Claude's refresh lock (`.oauth_refresh.lock`, a directory made with `mkdir`, as auto-switch does) in `~/.claude`, and in `<root>/<to>` when it is free, and rewrite the journal with each lock's inode (`home_lock_ino`, `to_lock_ino`) as soon as it is taken. A lock held by someone else is never broken. |
| `s1` | Atomically exchange `T` and `L`. `~/.claude` now holds the incoming login; `<root>/<to>` holds the outgoing one. |
| `s2` | Rename the outgoing login from `<root>/<to>` to `S`, refusing to replace an existing file. Release the refresh locks: each is removed with `rmdir` only if its inode is the one recorded. |
| `s3` | Replace the `oauthAccount` block in `$HOME/.claude.json` with the one from `<root>/<to>/.claude.json`, keeping every other key and the file's mode. |
| `s4` | Write the ledger: `live=<to>`, `previous=<from>`, `account`, `since` and `swapped_at`. Clear any ALERT key. |
| | Delete `journal.json`. |

Credentials files are touched only by `lstat`, the exchange and the rename,
and each parent directory is synced after each rename. At every point each
login is in exactly one place, and `~/.claude` always has a login.

If the exchange fails with `EXDEV`, `EINVAL` or `ENOSYS`, the swap fails with
`RotateRefused` (`atomic exchange is not supported here`), the refresh locks
are released and the journal is removed; nothing was moved. Any other error
from the exchange also fails with `RotateRefused`, after the same clean-up.
The exchange never produces `RotateUnsupported`. There is no copy-and-delete
fallback.

### Rotate recovery

Recovery runs under the lock at the start of every non-dry-run `rotate`,
`start` and cycle, before anything else is decided. `--dry-run` never runs
it. Recovery uses the journal and file metadata only, and only renames
logins: it never deletes, overwrites or opens a credentials file.

Recovery decides by where the recorded inodes are, found with `lstat`. The
journal's `phase` is a hint for the log and for `status`; it never decides.
This matters most for the commonest crash point, just after the exchange and
before the journal is rewritten as `s1`, where the journal still says `j0`
but the logins have already moved.

`L` and `T` are not tested in `~/.claude`. Claude may refresh the token in
`~/.claude` at any time, which replaces the file there with a new inode, so
neither recorded inode can be expected in `~/.claude` after the exchange.
`~/.claude/.credentials.json` need only be a regular file.

| Where the logins are (any phase) | Action | Log line |
|----------------------------------|--------|----------|
| No journal | Nothing. | |
| `target_ino` is at `<root>/<to>/.credentials.json`, `S` does not exist, and `~/.claude/.credentials.json` is a regular file | The exchange did not happen. Roll back: release the recorded refresh locks, then delete the journal. | `recovery: rolled back an interrupted swap work->spare` |
| `live_ino` is at `<root>/<to>/.credentials.json`, `S` does not exist, and `~/.claude/.credentials.json` is a regular file | The exchange happened; `s2` did not. Roll forward: run `s2`, `s3` and `s4`, then delete the journal. | `recovery: finished an interrupted swap work->spare` |
| `live_ino` is at `S`, nothing is at `<root>/<to>/.credentials.json`, and `~/.claude/.credentials.json` is a regular file | `s2` happened. Roll forward: release the recorded refresh locks, run `s3` and `s4`, then delete the journal. `s3` and `s4` are safe to repeat. | `recovery: finished an interrupted swap work->spare` |
| Anything else, a `home` that differs from `$HOME`, an invalid name, or an unknown version | Mark the journal `inconsistent`. The refresh locks are left in place. | `ALERT: rotate recovery is inconsistent: <reason>; no swap until it is resolved` |

Releasing a refresh lock means removing `<dir>/.oauth_refresh.lock` with
`rmdir` only when its inode equals the recorded `home_lock_ino` or
`to_lock_ino`. A lock with another inode belongs to Claude and is left alone.
If the process stopped after making a lock but before recording its inode,
that lock is not released; see
[Recovery](../howto/claude-profile-rotation.md#recovery) for the manual step.

`inconsistent` is sticky. While the journal says so, no swap happens, the
one-shot command exits with `RotateInconsistent`, and `watch` writes the ALERT
(once) and keeps running. A person resolves it as described in
[Recovery](../howto/claude-profile-rotation.md#recovery) and deletes the
journal.

If auto-switch recovery reports an inconsistent state, the cycle writes
`ALERT: auto-switch recovery is inconsistent; no swap` and does not swap.

The journal's `home` is only compared with `$HOME`, and its inode numbers are
only compared with `lstat` results; neither is ever used as a path.

### Rotate state directory

Everything is under `<root>/.auto-switch.d/rotate/` (directory `0700`, files
`0600`). Ledger and journal are written atomically, never through a symlink,
and are capped at 64 KiB. Both have `"version": 1`, reject unknown fields, and
omit optional fields that are unset. Every profile name read from them is
checked with the [name rules](#name-rules) before it is used. A file that
fails to parse is left unchanged and reported as `RotateInconsistent`.

| Path | Contents |
|------|----------|
| `ledger.json` | `live`, `account`, `since` (epoch seconds when `live` last changed), `swapped_at`, `previous`, `alert` (the de-duplication key, `<kind>:<live profile or ->`), and `last_cycle` (`at` and `outcome`). |
| `journal.json` | The swap in progress: `from`, `to`, `adopt`, `phase` (`j0`, `s1`, `s2`, `s3`, `s4` or `inconsistent`), `home`, `live_ino`, `target_ino`, `home_lock_ino` and `to_lock_ino` (each present once that refresh lock is taken), `started_at` and `pid`. |
| `rotate.pid` | The background process's pid, as ASCII decimal. |
| `rotate.log` | The log. |
| `rotate.log.1` | The previous log, after rotation. |
| `rotate.err` | The background process's own stdout and stderr. Not rotated. |

The auto-switch `ledger.json` and `journal.json` one level up are separate
files and are not changed by rotation.

Example ledger, with the account value replaced:

```json
{
  "version": 1,
  "live": "spare",
  "account": "<accountUuid>",
  "since": 1760791205,
  "swapped_at": 1760791205,
  "previous": "work",
  "last_cycle": { "at": 1760791805, "outcome": "stayed" }
}
```

### Rotate log

Each line is `YYYY-MM-DDTHH:MM:SSZ <message>`, in UTC. The one-shot command
writes to the log and stdout, `watch` to the log only, and `--dry-run` to
stdout only, without the time.

| Message | Meaning |
|---------|---------|
| `cycle live=<name\|-> <name>:s=<n>%,w=<n>% ...` | One per cycle: every measured profile, live first, then by name. Percentages are rounded down. A profile whose probe failed shows `<name>:failed`, an exhausted one `s=100%,w=100%`; a skipped one is not listed. A login no profile owns is shown as `-`. |
| `candidate <name>: score=... -> <verdict>` | One per measured candidate, explaining the choice; see [Rotate candidate lines](#rotate-candidate-lines) for the format and when they are written. |
| `swap <from>-><to> live s=<n>% w=<n>% target s=<n>% w=<n>% reason=threshold\|upgrade` | A completed swap. For an adopted login, `<from>` is the new profile's name. A failed live probe shows `live failed`. |
| `ALERT: no profile has capacity live=<name> s=<n>% w=<n>%` | A threshold is reached and no candidate meets the margin. |
| `ALERT: no login in ~/.claude; no swap` | `~/.claude/.credentials.json` is missing or not a regular file. |
| `ALERT: the account in ~/.claude is not the one the ledger names (live=<name>); no swap` | Hand change. |
| `ALERT: more than one profile matches the account in ~/.claude; no swap` | Two or more login-less profiles have the account in `~/.claude.json`. |
| `ALERT: swap refused: <reason>` | A [pre-swap check](#rotate-swap) failed; written once per reason. The one-shot command also prints it and exits with `RotateRefused`. |
| `ALERT: rotate recovery is inconsistent: <reason>; no swap until it is resolved` | See [Rotate recovery](#rotate-recovery). |
| `ALERT: auto-switch recovery is inconsistent; no swap` | Auto-switch state needs `auto-switch repair`. |
| `recovery: rolled back an interrupted swap <from>-><to>` / `recovery: finished an interrupted swap <from>-><to>` | Recovery acted. |
| `skip: state lock busy` / `skip: state changed during measurement` / `skip: <name> (<reason>)` / `skip: cannot see running sessions; no candidates` | The cycle or a candidate was skipped. |
| `skip: the external rotator is running` / `skip: <error>` | `watch` only: the cycle failed with that error and the rotator carries on. |
| `skip: already swapped since the failed attempt started (live=<name>)` | [On-limit](#rotate-on-a-usage-limit) cycle: another run or the background rotator swapped after the failed attempt started. Nothing is probed or moved. |
| `skip: live login is not near a limit (s=<n>% w=<n>%); no forced swap` | On-limit cycle: the live login measured below `fast-at` (`AMPLIHACK_ROTATE_FAST_AT`) in both windows, so the failure was not taken as a reason to swap. |
| `skip: prototype ledger ignored (<reason>)` | See [Rotate live profile](#rotate-live-profile). Written only when the reason changes. |
| `stop: signal received; cycle abandoned` | `watch` stopped during measurement or before the swap; nothing was moved. |
| `idle: auto-switch is off` | `watch` with auto-switch turned off; written once per change. |
| `would swap ...` / `would adopt ~/.claude as <name>` / `would stay` / `would alert: ...` / `would recover an interrupted swap (phase <phase>)` / `would alert: rotate recovery is inconsistent` / `would alert: the rotate journal is damaged` | `--dry-run` only. |

An ALERT is written only when its key (`no-capacity`, `no-login`,
`hand-change`, `ambiguous`, `inconsistent`, `auto-switch` or `refused <reason>`
for a swap refused under [Swap](#rotate-swap), plus the live
profile) differs from the key
stored in the ledger. The key is cleared when a cycle stays without alerting
or a swap succeeds, so the same ALERT can be written again later. The
one-shot `rotate` still prints a repeated ALERT to stdout on every run; only
the log line is written once.

Before each write, a log over 1 MiB is renamed to `rotate.log.1`, replacing
the old one. The log is opened with `O_APPEND`, `O_NOFOLLOW` and `O_CLOEXEC`,
mode `0600`. A log that is not a regular file, is owned by another user, or is
readable or writable by group or others is not written to; a warning goes to
stderr and its mode is not changed.

Log lines and stdout contain only profile names (with control characters
removed), percentages, scores, times, phases and fixed text. They never contain
account IDs, e-mail addresses, tokens, environment values or Claude's output.

### Rotate daemon

`start` launches `amplihack profile rotate watch <resolved flags> --spawned`
with the same `amplihack` binary, in a new session (`setsid`) with `SIGHUP`
ignored, stdin from `/dev/null`, and stdout and stderr appended to
`rotate.err` (`0600`) beside the log. The log itself is written only by the
cycle, so it keeps its grammar; `rotate.err` holds only what the process
prints outside a cycle, such as start-up notices or a crash.
`--spawned` is hidden; it tells `watch` that `start` has already written the
pid file. No shell is involved.

The child's environment is the parent's, with every `AMPLIHACK_ROTATE*`
variable removed and `AMPLIHACK_CLAUDE_PROFILE_ROOT` set to the absolute root
`start` resolved. The child validates that value again when it starts, and
never reads `~/.amplihack/profile-root`. Changing the variable or the saved
root, or closing the shell that ran `start`, does not move a running rotator.
To move it to another root, stop it and start it again.

### Rotate root record

After writing the pid file, `start` writes two files that hold the absolute
root it started with, in the
[saved root file format](#saved-root-file-format):

| File | Purpose |
|------|---------|
| `<root>/.auto-switch.d/rotate/root` | Next to the pid file. Shows which root a rotator's state belongs to. |
| `~/.amplihack/rotate-root` | Lets `status` and `stop` find the rotator from a shell whose root now resolves elsewhere. Only the most recent `start` is recorded. |

If either file cannot be written, `start` prints a warning to stderr and the
rotator keeps running.

`rotate status` and `rotate stop` locate the rotator as follows:

1. If a rotator under the resolved root is `running`, they use the resolved
   root.
2. Otherwise, if `~/.amplihack/rotate-root` names a different root and the
   rotator under that root is `running`, they use that root.
3. Otherwise they use the resolved root, and report what they find there as
   they do without a record (`rotate: not running` for `stop`).

A record that cannot be read, is invalid, or names a root whose rotator is
`stopped`, `stale` or `unverifiable` is ignored. A stale record is therefore
harmless: it can only choose which directory's pid file is read, and `stop`
still makes the [verified](#rotate-daemon) checks before it signals anything.

`stop` leaves the record in place; the next `start` replaces it. Rotators
running under several roots at once are not supported; the record names only
the latest.

A pid file is *valid* when it holds at most 32 bytes of ASCII digits for a pid
greater than 1 that is not the caller's own. A process is a *verified
rotator* when `/proc/<pid>` is owned by your effective uid, its command line
contains `profile`, `rotate` and `watch`, and its `argv[0]` has the same file
name as the running `amplihack`.

| State | Condition | `stop` |
|-------|-----------|--------|
| `running` | Valid pid file naming a verified rotator. | Sends `SIGTERM` (through a pidfd on Linux), polls for 10 seconds, then removes the pid file. If it is still running, keeps the pid file and fails with `RotateDaemon`. |
| `stopped` | No pid file. | Prints `rotate: not running` and exits 0. |
| `stale` | The pid file is invalid, or names no process, or a process that is not a verified rotator. | Removes the pid file, signals nothing, prints `rotate: removed a stale pid file`. |
| `unverifiable` | No `/proc` on this host. | Signals nothing and fails with `RotateDaemon`. |

`watch` removes the pid file on exit only if it still holds its own pid.

### Rotate signals

`watch` acts on `SIGTERM` and `SIGINT` at these points:

| When the signal arrives | What `watch` does |
|-------------------------|-------------------|
| Between cycles | Exits at once. |
| During measurement | Stops its probes as it does at the probe deadline (`SIGKILL` to each probe's process group), writes `stop: signal received; cycle abandoned` and exits. Nothing is swapped. |
| After measurement, before step 6 takes the lock | Writes the same line and exits without taking the lock. |
| While it holds the lock (revalidation and swap) | Finishes the swap, writes the log lines, releases the lock, then exits. |

The swap holds no probe and waits on nothing but local file operations, so
`stop`'s 10-second wait covers it. A `stop` that times out therefore means
the process is not responding, not that a cycle was slow.

### Rotate status

`status` takes no lock and changes nothing. It prints `key: value` lines:

```
root: /home/you/.claude-profiles (source: config)
rotate: running
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

| Key | Value |
|-----|-------|
| `root` | Always the first line. `<path> (source: env\|config\|default)` for the resolved root, or `<path> (pinned by running rotator)` when the rotator was found through the [rotate root record](#rotate-root-record). The lines after it describe that root. |
| `rotate` | `running`, `stopped`, `stale` or `unverifiable`. |
| `pid` | The pid, or `-`. |
| `live` | The ledger's `live` profile, or `-`. |
| `last cycle` | The time and outcome (`swapped`, `stayed`, `alerted`, `skipped` or `idle`) of the last cycle, or `-`. `skipped` is recorded when the state changed during measurement; a cycle that could not take the lock records nothing. |
| `log` | The log path. |

Run from a shell where the root now resolves elsewhere, for example cron with
no saved root, `status` finds the running rotator through the record:

```
root: /home/you/.claude-profiles (pinned by running rotator)
rotate: running
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

A pending or inconsistent journal adds a line `journal: pending (phase <phase>)`
or `journal: inconsistent`, and one that cannot be read `journal: damaged`. A
ledger that cannot be read shows `live: - (ledger unreadable)`.

### Rotate prototype coexistence

Before a non-dry-run `rotate`, `start` or cycle, the rotator opens
`<root>/.rotator/lock` read-only (`O_NOFOLLOW`, `O_NONBLOCK`, never
`O_CREAT`), checks that it is a regular file, tries a non-blocking exclusive
lock, and releases it at once. A missing file passes the check and is not
created. If the lock is held, the command fails with
`RotatePrototypeRunning`. `--dry-run` skips this check. The rotator never
creates, writes or deletes anything under `<root>/.rotator/`.

### Rotate exit status

| Status | When |
|--------|------|
| `0` | No swap, a swap, a dry run, an ALERT, `status`, a `stop` of a stopped or stale rotator. |
| `1` | `RotateConfig`, `RotatePrototypeRunning`, `RotateInconsistent`, `RotateRefused`, `RotateUnsupported`, `RotateDaemon`, or any other [error](#errors). |
| `2` | A usage error from argument parsing, such as a tuning flag before `start`. |

## Errors

Errors print one line to stderr as `error: <message>` plus `hint: <next step>`
and exit with status 1. Messages never include file contents.

`profile usage` reports a profile it cannot read as a
[state](#states) on that profile's row, not as an error; only its setup
failures use this format.

| Condition | Hint |
|-----------|------|
| Invalid name | Lists the allowed pattern |
| Profile exists / not found | `amplihack profile list` |
| Symlinked or out-of-root profile | Remove the symlink and re-add |
| Pointer dangling (names a missing profile) | `amplihack profile switch <name>` |
| Pointer corrupt or oversized | Re-run `profile switch <name>` |
| `remove` without `--yes` | Re-run with `--yes` |
| `--profile` with `--docker`/`--auto` | Launch without the flag combination |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` with `--docker`, `--auto` or env-activated Docker | `unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto` |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` is an invalid name or not found | Unset `AMPLIHACK_CLAUDE_PROFILE`, or create the profile with `amplihack profile add <name>` |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` names a profile that exists but cannot be used | The hint for the underlying cause, e.g. `check permissions on the profile root` for an I/O error or the `chmod go-rwx` hint for unsafe permissions |
| Recipe context key in the `AMPLIHACK_` namespace (`recipe run -c amplihack_…=…`) | Remove the key; the namespace is reserved |
| `InvalidRoot`: a non-empty `AMPLIHACK_CLAUDE_PROFILE_ROOT`, or the `profile root` argument, is empty, relative, not UTF-8, holds control characters or has a `..` component; or `HOME` is unset | `use an absolute path: set AMPLIHACK_CLAUDE_PROFILE_ROOT to one, or run: amplihack profile root <absolute dir>` |
| `InvalidSavedRoot`: `~/.amplihack/profile-root` is invalid (the message names the file and the [reason](#saved-root-file-format)), or `profile root <dir>` cannot write it safely | `fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>` |
| `Leased`: the profile's login is live in another profile's session | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| `HostsForeignAccount`: the profile's directory holds another account's login | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| `AutoSwitchConfig`: `.auto-switch` is a symlink or cannot be written | `` remove <root>/.auto-switch and run `amplihack profile auto-switch on` or `off` `` |
| `AutoSwitchUnsupported`: an `auto-switch` command on a platform without Unix file semantics | `auto-switch needs Linux or another Unix; leave it off` |
| `AutoSwitchInvariant`: the state files are damaged or a login is not where the state says | `see the Recovery section of docs/howto/claude-profiles.md` |
| `AutoSwitchInstall`: a profile's `settings.json` is a symlink, owned by someone else, not a JSON object, or cannot be written | `` fix or remove the reported file, then run `amplihack profile auto-switch repair` `` |
| `RotateConfig`: a rotate flag or `AMPLIHACK_ROTATE_*` value is not a whole number in range | `` run `amplihack profile rotate --help` for the accepted values `` |
| `RotatePrototypeRunning`: the external rotator holds `<root>/.rotator/lock` | `stop the external rotator (tmux session claude-rotator) first` |
| `RotateRefused`: a check before the swap failed, or the atomic exchange failed; nothing was moved | `nothing was moved; see "Swap refused" in docs/howto/claude-profile-rotation.md` |
| `RotateInconsistent`: the rotate ledger or journal is damaged, or recovery cannot tell which way is safe | `see the Recovery section of docs/howto/claude-profile-rotation.md` |
| `RotateUnsupported`: `profile rotate` on a platform without Unix file semantics | `rotate needs Linux or another Unix` |
| `RotateDaemon`: `start` found a rotator running, `stop` timed out, or the process cannot be verified | `` run `amplihack profile rotate status` `` |

Auto-switch errors name profiles in the message; hints are fixed text. For
example:

```
error: profile "personal": its login is in use by a session in profile "work"
hint: wait for that session to end, or run `amplihack profile auto-switch status`
```

```
error: profile "work": its directory holds the login of profile "personal" for a running session
hint: wait for that session to end, or run `amplihack profile auto-switch status`
```

Auto-switch notes on stderr, errors and `status` contain only profile names,
paths relative to the root, phases, percentages and times. They never contain
token values, e-mail addresses or organisation IDs.

The inherited-profile message names `AMPLIHACK_CLAUDE_PROFILE`, shows the
value escaped (Rust `{:?}` form) and cut to 64 characters, and gives a fixed
reason: `invalid name`, `not found`, `refused path`, `unsafe permissions`,
`i/o error on profile root (<kind>)`, `not usable` or
`not applied with --docker, --auto or Docker`. `<kind>` is the I/O error
kind, such as `permission denied`; it never contains the value. For
example, as printed to stderr:

```
error: inherited AMPLIHACK_CLAUDE_PROFILE "work": not found
hint: unset AMPLIHACK_CLAUDE_PROFILE, or create the profile with `amplihack profile add <name>`
```

Control characters in the value are escaped, so a malformed value cannot
write terminal escape sequences.

## Security notes

When creating a profile amplihack reads the allowlisted assets and parses
`settings.json`; it never modifies the source directory and never copies
credential or history files. Allowlisted setting values and asset contents are
copied verbatim, so any secrets embedded in hooks, status-line commands or
skills remain the user's responsibility. Unix source reads are anchored to a
no-follow root descriptor; non-Unix platforms have a residual same-user
check-then-open race. Profiles are not a security boundary against the same
OS user.
The pre-spawn re-validation narrows but does not close the window between the
check and `exec`; it is not atomic. This is defense in depth: anyone who can
write the profile store already has the user's privileges.

Auto-switch moves `.credentials.json` files and nothing else touches them:
they are never opened, read, parsed, copied or refreshed. The only operations
on them are `renameat2(RENAME_NOREPLACE)` between two directories under the
root, `utimensat` (without following symlinks) to set the modification time,
metadata checks, and syncing the directories that contain them. Each account's
login is in exactly one place at any moment. `.claude.json` and
`settings.json` may contain secrets (`primaryApiKey`, `env`, `apiKeyHelper`):
only the `oauthAccount` key is copied out of `.claude.json`, rewrites of
`.claude.json` and `settings.json` keep the file's existing permission bits
(setuid, setgid and sticky are dropped; a new `settings.json` is `0600`), and parse errors report the path, line and
column only.
State directories are `0700` and state files `0600`, and symlinks are
refused, as for the profile store. Auto-switch
makes no network calls. The status-line wrapper runs the original command
with `sh -c`, which is what Claude itself does with it; the supervisor for
reruns starts amplihack directly with no shell.

Residual risks of auto-switch:

- Processes running as you, and root, are out of scope. They can read,
  move or replace every file involved, including the state directory and
  the saved status line; the ownership, mode and symlink checks stop
  accidents and other users, not them.
- Rerun buffers hold a step's stdout until the step finishes, or after a
  crash until the next recovery run (at most 24 hours); see
  [Usage-limit reruns](#usage-limit-reruns).
- A rerun repeats the whole step, including side effects of the attempt
  that hit its limit.
- The trigger is the runner's own text classifier. Output an agent was led
  to print (for example by prompt injection) can cost one extra attempt per
  profile, but only for a step that already failed with a non-zero exit and
  was not killed.
