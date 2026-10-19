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
amplihack profile usage [--json] [--raw] [<name>...]
amplihack profile auto-switch on [--threshold <1-100>] [--exclude <name,...>]
amplihack profile auto-switch off
amplihack profile auto-switch status
amplihack profile auto-switch threshold <1-100>
amplihack profile auto-switch exclude [<name>...]
amplihack profile auto-switch install
amplihack profile auto-switch repair
amplihack launch|claude --profile <name> [...]
```

## Subcommands

| Command | Behavior |
|---------|----------|
| `add <name>` | Copy allowlisted shared assets from `~/.claude` into a private staging directory, then atomically publish `<root>/<name>/` (mode `0700` on Unix; Windows uses its inherited ACL). Errors if a profile exists whose name matches case-insensitively, on every platform. Does not run Claude. |
| `list` | One profile per line, sorted; the default is marked `*`. Skips dot-entries, symlinks and non-directories. Reports a corrupt or dangling default pointer instead of silently omitting the marker. |
| `show` | Prints `name: <name>` and `dir: <path>`, or `none`. Paths only; never directory contents. |
| `switch <name>` | Writes the default pointer. The profile must exist. |
| `remove <name>` | Requires `--yes`. Resolves and verifies the path is inside the root, reads the default pointer, deletes the directory, then clears a matching pointer. A missing pointer means no default; a corrupt, unreadable or dangling pointer fails closed before any deletion (repair with `switch`). Never deletes the root; never follows symlinks. |
| `login <name>` | Always runs the `claude` binary (ignores binary selection), interactively, with `CLAUDE_CONFIG_DIR=<profile dir>`. |
| `run <name> -- <args>` | One-shot launch through the normal launch path. Args are forwarded as a literal argv vector; no shell is involved. |
| `usage [<name>...]` | Runs `claude -p /usage` under each profile in parallel and shows its session and weekly usage, most capacity first. `--json` for scripts, `--raw` to include Claude's text. Refreshes auto-switch readings. See [Usage command](#usage-command). |
| `auto-switch …` | Turn [auto-switch](#auto-switch) on or off, set its threshold and exclusions, show its state, or install and repair the status-line wrapper. See [Auto-switch commands](#auto-switch-commands). |

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

A login rotator outside amplihack can move one account's login into the
default directory `~/.claude`. While it is there, that profile's own
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
2. **Rotator ledger.** If `<root>/.rotator/ledger.json` exists, it decides.
   Its `live` field names the profile and `since` is used only for the
   [check after the probes](#swaps-during-the-probe); other fields are
   ignored. The checks run in this order, and the first that fails decides:

   | `live` in a trusted ledger | Result | stderr |
   |----------------------------|--------|--------|
   | Missing or `null` | No profile is live. This is the rotator's idle state. | nothing |
   | Not a string, fails the [name rules](#name-rules) (for example `"../x"`), or names a profile whose directory does not exist or fails the usual directory check | No profile is live. | the ledger warning below |
   | Names a profile that still has something at `<root>/<live>/.credentials.json` (a dangling symlink counts) | No profile is live. That profile is probed through its own directory as normal, because its own login is stronger evidence than a ledger that is probably out of date. | `warning: the login rotator's ledger names a profile that still has its own login; it is not marked live in ~/.claude` |
   | Names a profile with no `.credentials.json` | That profile is live. | nothing |

   The account match below is never tried when a ledger file exists, or
   when amplihack cannot tell whether it exists (for example, a
   `<root>/.rotator/` directory you cannot search). The second case is
   treated as an unreadable ledger.

   The ledger is taken at its word: amplihack does not compare the account
   in `~/.claude.json` with the named profile's account. If the ledger names
   the wrong profile (the rotator stopped partway through a swap, or someone
   ran `/login` in `~/.claude`), that profile's row shows the numbers of the
   login that is actually in `~/.claude`.

   The ledger is trusted only if the profile root and `<root>/.rotator` are
   real directories (not symlinks) owned by you that no other user can write
   to (group write is allowed only when the group is your own per-user
   group), and `ledger.json` is not a symlink, is owned by
   you, is not writable by group or others, and is at most 64 KiB. If any of
   these fails, or the file is not valid JSON, no profile is live and one
   warning goes to stderr; the account match below is not tried:

   ```
   warning: could not read the login rotator's ledger; no profile is marked live in ~/.claude
   ```

   No warning text is ever built from the ledger's contents.
3. **Account match**, only when there is no ledger file. A missing
   `ledger.json`, with or without a `<root>/.rotator/` directory, is the
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
`<root>/.rotator/`; the rotator owns those files.

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
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Profile root. Must be absolute and non-empty. Default `~/.amplihack/claude-profiles/`. |
| `AMPLIHACK_CLAUDE_PROFILE` | Profile name a launch passes to its children so nested launches use the same profile. Set by amplihack on the child process; may also be set by hand. Empty means unset. See [Resolution and environment](#resolution-and-environment). |
| `<root>/<name>/` | Profile directory (mode `0700` on Unix; Windows uses its inherited ACL). |
| Root directory | Created with mode `0700` on Unix; Windows uses its inherited ACL. A symlinked root is refused. An existing custom root is trusted input. |
| `<root>/.default` | Pointer file: only the profile name (mode `0600` on Unix, max 256 bytes). Windows uses its inherited ACL. Written by creating a new temp file (`create_new`, never through a symlink) and renaming it over the pointer. |
| `AMPLIHACK_CLAUDE_AUTO_SWITCH` | `1` turns auto-switch on and `0` turns it off for this process and its children, overriding `<root>/.auto-switch`. Empty means unset. See [Toggle and precedence](#toggle-and-precedence). |
| `<root>/.auto-switch` | Auto-switch settings. See [Settings file](#settings-file). |
| `<root>/.auto-switch.d/` | Auto-switch state. See [State directory](#state-directory). |
| `AMPLIHACK_USAGE_LIMIT_WAIT` | `1`, `true`, `on` or `yes` turns the [usage-limit wait](#usage-limit-wait) on, and `0`, `false`, `off` or `no` turns it off, for this process and its children (any case). Overrides the config file. Empty means unset. |
| `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | Upper bound on one usage-limit wait, in seconds (a non-negative integer). Only shortens the wait. Overrides the config file. Empty means unset. |
| `$HOME/.amplihack/config` key `usage_limit_wait` | `{"enabled": <bool>, "max_wait_secs": <u64>}`, both optional. See [Usage-limit wait settings](#usage-limit-wait-settings). |

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
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | `<root>`, only when `AMPLIHACK_CLAUDE_PROFILE_ROOT` was set in the launching process; otherwise left unset so the child uses the default root too |

Copilot and Codex children get none of these, but variables already in the
environment pass through them unchanged.

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
| `multitask` / orch workstreams | Inherited environment: each workstream's `run.sh` is started with the parent's full environment. As a backup, the generated `run.sh` also exports `AMPLIHACK_CLAUDE_PROFILE` and `AMPLIHACK_CLAUDE_PROFILE_ROOT`, single-quoted, when they are set and non-empty, so a `run.sh` re-run by hand or on resume uses the same profile. `CLAUDE_CONFIG_DIR` is not exported; the nested launch rebuilds it from the name and checks it again. If either variable is set but is not valid UTF-8, the workstream is refused before any script is written (the error names the variable, not its value), because leaving it out would send the nested launch to the default root. |

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

Before each move the file must be a regular file owned by you with exactly
one hard link.

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
- stdin, unless it is a terminal, is copied once, before the first attempt,
  to a private file in the same directory, and every attempt reopens and
  reads it from the start (see [Standard input](#standard-input)). Before
  the usage-limit wait was added, attempts inherited stdin directly, so a
  rerun could get an empty or partly read prompt; the copy replaces that
  for auto-switch reruns too;
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
[usage-limit wait](#usage-limit-wait) runs if it is enabled. Otherwise the
last attempt's output and exit code are passed back unchanged, and the
recipe runner reports the usage limit and ends the run.

Reruns happen at the step-launch layer: inside the nested
`amplihack claude -p` that each agent step runs. The recipe runner's retry
loop, its executor and its failure classifier are unchanged; the runner only
sees the final attempt.

The supervised attempt runs with `AMPLIHACK_AUTO_SWITCH_SUPERVISED=1`, which
stops it supervising itself; amplihack removes that variable before it starts
Claude. If the supervisor cannot start, the launch runs unsupervised, with a
warning.

The stdout buffer is `rerun/<pid>-<n>.out`, where `<pid>` is the supervisor
and `<n>` the attempt. It is deleted when the attempt is rerun or its output
has been printed. The stdin copy is `rerun/<pid>-<n>.in`, created once per
step and deleted when the supervisor exits, on every path including a
signal. Attempts receive it as an open file descriptor, never as a path. If
the supervisor is killed first, recovery deletes both kinds of file once no
process with that `<pid>` is running, and any of them older than 24 hours
regardless. Recovery runs at every supervisor start as well as at the
points listed in [Session end and recovery](#session-end-and-recovery).
Until then each file holds whatever the step read or printed, at mode
`0600` in a `0700` directory.

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
| `rerun/` | Buffered stdout of supervised steps, `<pid>-<n>.out`, and their copied stdin, `<pid>-<n>.in`; see [Usage-limit reruns](#usage-limit-reruns) for how long each is kept. |

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

## Usage-limit wait

When a recipe agent step fails on a usage limit and no auto-switch rerun is
left, the step's supervisor waits for the limit to reset, or for the login
to change, and then runs the step once more. It is enabled by default and
does not depend on auto-switch. Task-oriented guidance is in the
[how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

### Gate

A launch can wait when all of these hold, checked in this order:

1. no `--profile` flag;
2. Claude's arguments contain `-p`, `--print` or `--print=`;
3. `AMPLIHACK_AUTO_SWITCH_SUPERVISED` is unset (the launch is not itself a
   supervised attempt);
4. `AMPLIHACK_NONINTERACTIVE` is exactly `1`. `amplihack recipe run` sets
   it for every step. Other values, and piped stdin alone, do not count.
   The [settings](#usage-limit-wait-settings) are read only after this
   check passes, so a launch typed at a terminal never reads them and never
   warns about them;
5. the wait is enabled; and
6. the mode is not `--docker` or `--auto`. With an inherited profile these
   modes never reach this check: the launch is refused earlier with
   `not applied with --docker, --auto or Docker`. So check 6 only matters
   for a launch with no inherited profile.

A launch that fails checks 1 to 3 is left alone without a log line, exactly
as before. A launch that passes them is then handled by one of these rows:

| Inherited profile | Auto-switch | Wait (checks 4 and 5) | Supervisor |
|-------------------|-------------|-----------------------|------------|
| yes | on | applies | [Usage-limit reruns](#usage-limit-reruns), then the wait |
| yes | on | does not apply | [Usage-limit reruns](#usage-limit-reruns) only. With check 4 passed and the wait disabled, a limit with no profile left writes `skipped reason=disabled` |
| yes | off | applies | the same supervisor with no profile substitution, then the wait |
| no | either | applies | the same supervisor with no profile substitution, then the wait |
| yes or no | off, or no profile | does not apply | none: one unsupervised run, as before |

"No profile substitution" means no auto-switch rerun, no lease and no
ledger reading recorded. The retry runs under the inherited profile, or
under no override when none was inherited.

#### Scratch directory

The supervisor keeps the held-back stdout and the stdin copy in a scratch
directory:

| Auto-switch | Scratch directory |
|-------------|-------------------|
| on | `<root>/.auto-switch.d/rerun/` |
| off, or no profile inherited | `$HOME/.amplihack/rerun/` |

A profiled step with auto-switch off therefore uses `$HOME/.amplihack/rerun/`
and creates nothing under the profile root.

`$HOME/.amplihack/rerun/` is checked before anything is written to it:

- `$HOME/.amplihack` must be a real directory (not a symlink), owned by the
  current user and not writable by group or others. If it is missing it is
  created with mode `0700`. An existing directory is never `chmod`'ed: one
  that fails the check makes the launch unsupervised.
- `rerun/` must be a real directory owned by the current user with mode
  exactly `0700`. It is created that way when missing.

Files in either scratch directory are created with `create_new`, no-follow,
close-on-exec and mode `0600`. The existing `rerun/` sweep removes stale
`.out` and `.in` files whose process is gone, or that are older than 24
hours. It removes only regular files whose names it can parse, follows no
symlinks and does not recurse.

#### Standard input

When stdin is not a terminal, the supervisor copies it in full, before the
first attempt, to `rerun/<pid>-0.in`. There is no size cap. The file's
device and inode are recorded. Each attempt reopens the copy read-only and
no-follow, and is refused if the reopened file is not the same device and
inode, so a swapped file is never fed to an attempt. No descriptor is
shared between attempts, so a process left over from one attempt cannot
move another attempt's read position. The copy is deleted when the
supervisor returns, on every exit path: success, the retry's exit code,
`already_waited`, an error, or `128` plus a signal.

When stdin is a terminal, nothing is copied and every attempt inherits it.

If the copy cannot be made:

| When it fails | Result |
|---------------|--------|
| Before any input is read (no `HOME`, a scratch check fails, the file cannot be created) | `skipped reason=not_supervised`, then one unsupervised run that reads stdin directly |
| After input has been read (a read or write error part way through, a full disk) | one warning on stderr, and the launch exits `1` without running Claude. The input is already consumed, so an unsupervised run would get a truncated prompt |

#### When `not_supervised` is written

When checks 1 to 5 pass but the step cannot be supervised (a `--docker` or
`--auto` mode with no inherited profile, or a scratch or stdin failure
before input is read), the
launch writes `skipped reason=not_supervised` once, before Claude starts.
It is the only `skipped` line written without a usage-limit failure,
because an unsupervised launch cannot see its own result.

### Order of attempts

1. Run the attempt.
2. If it hit a usage limit (as defined in
   [Usage-limit reruns](#usage-limit-reruns)) and auto-switch is on, run
   the auto-switch loop.
3. When that loop has no profile left, or auto-switch is off, and the wait
   has not been used for this step, wait once.
4. Unless the wait ended with `step timeout` or `signal`, run exactly one
   more attempt with the same executable, arguments, environment and stdin
   copy, under the inherited profile, or under no
   override when none was inherited. The last auto-switch candidate is not
   reused.
5. Pass that attempt's output and exit code up.

There is at most one wait and one retry after a wait per step. A failure
that is not a usage limit never starts a wait. If the retry hits the limit
again, `skipped reason=already_waited` is written and its result is passed
up; the recipe runner then ends the run with its usual usage-limit message.

Every attempt, including the unprofiled ones and the retry, runs with
`AMPLIHACK_AUTO_SWITCH_SUPERVISED=1`, so a nested launch inside a step
cannot start a second wait.

The order of the checks after a usage limit is fixed:

1. a signal already received ends the step: the supervisor prints the
   failed attempt's held-back stdout, deletes the stdin copy and exits with
   `128` plus the signal number, exactly as after `end reason=signal`. No
   log line is written, because no wait started;
2. auto-switch on, the wait unused, and an eligible profile left: rerun
   under that profile at once, with no wait;
3. the wait disabled: `skipped reason=disabled`, and the attempt's result
   is passed up;
4. the wait already used: `skipped reason=already_waited`, and the
   attempt's result is passed up;
5. otherwise: `start`, the wait and `end`. After `end reason=signal` the
   step exits with `128` plus the signal number. After
   `end reason=step timeout` the attempt's result is passed up with no
   retry. After any other end reason, `resume` and the retry follow.

The retry runs the step **from the start**: Claude gets the same prompt and
has no memory of the failed attempt. Anything that attempt already did,
such as a commit, a push or an issue comment, may happen again. Write
recipe agent steps so that running one twice is harmless, for example by
checking whether a commit or comment already exists before creating it.

### Reset time

The reset time is read from the attempt's kept stderr tail and the last
64 KiB of its stdout. The patterns live in one table per harness
(`Harness::Claude` is the only harness), in
`crates/amplihack-cli/src/commands/recipe/run/limit_patterns.rs`. Two forms
are recognised, in this order:

| Form | Example | Rule |
|------|---------|------|
| Epoch | `Claude AI usage limit reached\|1792335000` | A line that starts with `Claude AI usage limit reached\|` followed by 1 to 19 ASCII digits. The digits are Unix seconds. A sign, a 20th digit or a value that overflows gives no reset. |
| Text | `You've hit your session limit · resets 8:30am (America/Los_Angeles)` | The text after the first `resets `, read with the grammar below. |

#### Text grammar

The fragment after the first `resets ` is cut to 128 bytes, at a character
boundary, before it is read. It must then be, in order:

1. a clock time, either `H[:MM]am` or `H[:MM]pm` (1 to 12, any case), or
   24-hour `HH:MM` (00 to 23);
2. exactly one space;
3. `(`, an IANA zone name, `)`.

The zone is looked up only in the zone table compiled into amplihack
(`chrono-tz`). No file under `/usr/share/zoneinfo` is opened and `TZ` is
not consulted.

| Fragment | Result |
|----------|--------|
| `8:30am (America/Los_Angeles)` | 08:30 Los Angeles time |
| `9pm (Europe/London)` | 21:00 London time |
| `12am (UTC)` | midnight |
| `12pm (UTC)` | noon |
| `17:00 (Asia/Tokyo)` | 17:00 Tokyo time |
| `8 (America/Los_Angeles)` | no reset: an hour with no `am`/`pm` and no minutes |
| `8:30am` | no reset: no zone |
| `8:30am (Mars/Olympus)` | no reset: unknown zone |
| `Oct 20 at 9am (UTC)` | no reset: dates are not read |
| `99:99pm (UTC)` | no reset: not a clock time |

No zone, date or day is ever guessed. A weekly-limit message that names a
date therefore gives `unparsed`, which waits for the 30-minute default.

#### Today or tomorrow

The clock time is placed on today's date in the named zone:

- If that instant is in the future, or at most 10 minutes in the past, it
  is used as is. An instant up to 10 minutes in the past is already
  reached: the wait has length zero and ends at once with
  `end reason=reset reached`, and the retry runs (see
  [Past resets](#past-resets)). This stops a message printed just after
  the reset from causing a five-hour wait.
- If it is more than 10 minutes in the past, the same clock time tomorrow
  is used.

Daylight saving changes:

- A time that falls in a spring-forward gap becomes the first valid instant
  after the gap. For `America/Los_Angeles` on 2026-03-08, `2:30am` becomes
  03:00 PDT.
- A time that occurs twice in a fall-back overlap becomes the earlier
  instant. For `America/Los_Angeles` on 2026-11-01, `1:30am` becomes 01:30
  PDT.

#### Clamping

A reset read for the wait, in either form, is capped at now plus five
hours. It is not raised to now: a reset in the past is kept as read, so the
deadline step below can tell it apart from a reset that is exactly now.
When the output has a usage-limit marker but neither form gives a reset,
the reset is `unparsed`.

#### Past resets

A reset at or before the moment the wait starts (an epoch in the past, or a
clock time inside the 10-minute grace) has already been reached. The
deadline is then now itself, not now plus the 120-second margin: `start` is
written with `until=` equal to the start time and `bound=reset+margin`,
the wait ends at once with `end reason=reset reached`, and the retry runs.
The 120-second margin applies only to a reset still in the future.

The auto-switch ledger keeps its existing rule: a limit without a readable
reset is recorded as resetting five hours from now. That default is never
used to time the wait.

### Deadline

The wait ends at the earliest candidate that applies:

| Candidate | Applies when | `bound=` |
|-----------|--------------|----------|
| reset + 120 s, or now when the reset is at or before now | a reset was read | `reset+margin` |
| now + 1800 s | no reset was read | `default` |
| now + `max_wait_secs` | a maximum is configured | `max_wait` |
| step start + `AMPLIHACK_STEP_TIMEOUT` − 60 s | `AMPLIHACK_STEP_TIMEOUT` is set and greater than 0 | `step_timeout` |

"Now" is when the wait starts. "Step start" is when the supervisor started,
which is before the first attempt. The `step_timeout` deadline is never
earlier than now: a step timeout of 60 s or less, or one already used up,
gives a deadline of now, and the step then ends without a retry (see
[Ending the wait early](#ending-the-wait-early)). All arithmetic saturates, so no input can make the wait
longer than five hours and 120 seconds. On a tie, the earlier row in the
table names the bound.

A configured maximum larger than the first two rows changes nothing.
`max_wait_secs = 0` waits 0 seconds and still retries once.

### Ending the wait early

The wait sleeps in slices of at most one second and stops for the first
of:

| `end reason=` | Cause |
|---------------|-------|
| `reset reached` | The deadline came from `reset+margin` or `default` and has passed. |
| `max reached` | The deadline came from `max_wait` and has passed. |
| `step timeout` | The deadline came from `step_timeout` and has passed. |
| `login changed` | The login fingerprint differs from the one taken when the wait began. |
| `signal` | `SIGINT`, `SIGTERM` or `SIGHUP` was received. |

The **login fingerprint** describes which account the retry would use. It
is taken when the wait starts and again every 30 seconds. It watches the
configuration directory the retry will run with:

1. `<root>/<inherited profile>` when a profile is inherited;
2. otherwise `$CLAUDE_CONFIG_DIR`;
3. otherwise `$HOME/.claude`.

A rotation into `~/.claude` does not help a step that inherited a profile,
because its retry still runs under that profile, so it is ignored.

The fingerprint has three parts:

| Part | Source |
|------|--------|
| Ledger account | For a profile directory, the account the auto-switch ledger (`<root>/.auto-switch.d/ledger.json`) records for it. For `$HOME/.claude`, the `live` field of the rotator ledger (`<root>/.rotator/ledger.json`), read with the existing rotator-ledger trust checks. |
| Account identity | `oauthAccount.accountUuid` from the directory's `.claude.json`, or `oauthAccount.emailAddress` when there is no UUID. For `$HOME/.claude` the file is `$HOME/.claude.json`. It is read without following symlinks, with the 32 MiB cap auto-switch already uses. |
| Credentials present | Whether `.credentials.json` exists in the directory, checked with `lstat` only. |

The wait ends with `login changed` when any part differs from the value
taken when the wait began. A part that appears or disappears counts as a
change.

The credentials file is **never opened**: its bytes, size, modification
time and digest are not part of the fingerprint. Claude rewrites that file
whenever it refreshes its OAuth token, and a refresh is not a new account,
so reading it would end waits early for no reason.

The ledgers and `.claude.json` must each be a regular file (not a symlink),
owned by the current user and not writable by group or others. A file that
fails these checks, is missing, cannot be parsed or is too large counts as
absent for its part, with no warning. The fingerprint is held only in
memory: it is never printed, logged or written to disk, and no account
name, UUID or e-mail address appears in any log line.

After `reset reached`, `max reached` and `login changed`, the retry runs.

After `step timeout` there is no retry. The step timeout is less than 60 s
away, too little for a fresh Claude attempt to finish, and the runner
would kill it part way. The supervisor prints the failed attempt's
held-back stdout, deletes the stdin copy and exits with that attempt's exit
code, so the runner sees the usage-limit failure and reports it with its
usual message. A step timeout of 60 s or less, or one already used up when
the wait starts, gives a deadline of now, so the wait ends at once with
`end reason=step timeout` and no retry.

After `signal`, the supervisor prints the failed attempt's held-back
stdout, deletes the stdin copy and exits with `128` plus the signal
number.

No attempt is running during the wait and no login is moved, so nothing is
left behind on any of these paths.

### Log lines

Each line goes to stderr, starts with `amplihack.recipe.usage_limit_wait`,
then the event, then `key=value` fields. Times are local, formatted
`%Y-%m-%d %H:%M:%S %:z`, so a time value contains spaces. Event and field
names are fixed; any new field is added at the end of its line.

| Event | Fields | Written |
|-------|--------|---------|
| `start` | `reason=usage_limit harness=claude reset=<time>\|unparsed until=<time> bound=reset+margin\|default\|max_wait\|step_timeout` | when the wait begins |
| `progress` | `remaining=<secs>s` | every 60 s while waiting |
| `end` | `reason=reset reached\|login changed\|max reached\|step timeout\|signal` | when the wait ends |
| `resume` | `attempt=<n>` | before the retry, so not after `end reason=step timeout` or `signal`; `<n>` counts every attempt in the step, including auto-switch reruns |
| `skipped` | `reason=disabled\|already_waited\|not_supervised` | `disabled` and `already_waited`: once, when a supervised step hits a usage limit and does not wait. `not_supervised`: once, at launch; see [Gate](#gate) |

Example of a step that waited for the login to change:

```
amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin
amplihack.recipe.usage_limit_wait progress remaining=1740s
amplihack.recipe.usage_limit_wait end reason=login changed
amplihack.recipe.usage_limit_wait resume attempt=2
```

No line contains credential bytes, digests, account names, paths inside a
configuration directory, config file content or parser error text. Times
are printed only after being parsed, so control characters in Claude's
output cannot reach the log.

### Usage-limit wait settings

| Setting | Environment variable | `~/.amplihack/config` | Default |
|---------|----------------------|-----------------------|---------|
| Enabled | `AMPLIHACK_USAGE_LIMIT_WAIT` | `usage_limit_wait.enabled` | `true` |
| Maximum wait, seconds | `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | `usage_limit_wait.max_wait_secs` | none |

```json
{
  "node_options_consent": true,
  "usage_limit_wait": { "enabled": true, "max_wait_secs": 3600 }
}
```

Each setting is resolved separately: the variable when it is set and
valid, otherwise the file, otherwise the default.

- `AMPLIHACK_USAGE_LIMIT_WAIT` accepts `1`, `true`, `on`, `yes`, `0`,
  `false`, `off` and `no`, in any case. This is wider than
  `AMPLIHACK_CLAUDE_AUTO_SWITCH`, which accepts only `1` and `0`.
- `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` accepts a non-negative integer
  with no sign.
- An empty variable is unset. Any other invalid value prints one warning
  per process, naming the variable, and the file decides.
- The file is read without following symlinks. It must be a regular file
  of at most 64 KiB, owned by the current user and not writable by group or
  others. A file that fails any of these checks gives the defaults for the
  whole key, with one warning. Fix it with `chmod go-w ~/.amplihack/config`
  or by giving it back to your user.
- A missing file or a missing key gives the defaults silently. Invalid
  JSON, a `usage_limit_wait` value that is not an object, or a field of the
  wrong type gives the defaults for the whole key, with one warning.
- Warnings name the variable or the key, never the file's content or the
  rejected value. Each is printed at most once per process.
- Settings are read when the step launches, and only when
  `AMPLIHACK_NONINTERACTIVE` is exactly `1`. Changing them affects later
  steps, not one that is already waiting.

There is no subcommand for this key. Inside the crate,
`wait_config::write(home, settings)` updates only `usage_limit_wait`, keeps
every other key, and writes the file atomically: a private temporary file
in the same directory, set to mode `0600`, then renamed over the original.
It refuses a symlinked file, a file owned by another user, and a file whose
content is not a JSON object.

`wait_config::write` takes no lock. The same file is also written by
amplihack's saved-preference code (for example the `node_options_consent`
prompt), so if both write at the same moment one of the two updates can be
lost. Check the file after changing it while other amplihack processes are
starting.

### Test seams

Two variables let tests run the real binary without waiting in real time.
They are read only in debug builds (`cfg(debug_assertions)`); release
builds ignore them.

| Variable | Effect |
|----------|--------|
| `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW` | Unix seconds used as the time the wait starts. |
| `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE` | Divides every sleep by this number. Must be a finite number from 1 to 1000000 inclusive; any other value is ignored and sleeps run in real time. |

Log lines and deadlines use virtual time; signals are still checked at
least once per real second. Unit tests do not use these variables: they
pass a `WaitDeps` implementation with their own clock, sleep, login probe
and signal check.

### Platform

The wait is Unix-only, like the rest of the supervisor. On other platforms
the settings are never read, no line is written, and a usage limit ends the
run as before.

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
| Invalid `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Use an absolute path |
| `Leased`: the profile's login is live in another profile's session | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| `HostsForeignAccount`: the profile's directory holds another account's login | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| `AutoSwitchConfig`: `.auto-switch` is a symlink or cannot be written | `` remove <root>/.auto-switch and run `amplihack profile auto-switch on` or `off` `` |
| `AutoSwitchUnsupported`: an `auto-switch` command on a platform without Unix file semantics | `auto-switch needs Linux or another Unix; leave it off` |
| `AutoSwitchInvariant`: the state files are damaged or a login is not where the state says | `see the Recovery section of docs/howto/claude-profiles.md` |
| `AutoSwitchInstall`: a profile's `settings.json` is a symlink, owned by someone else, not a JSON object, or cannot be written | `` fix or remove the reported file, then run `amplihack profile auto-switch repair` `` |

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
