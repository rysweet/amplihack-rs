# Reference: `amplihack profile`

Manage Claude Code login profiles. See the
[how-to](../howto/claude-profiles.md) for a walkthrough.

## Synopsis

```
amplihack profile add <name>
amplihack profile list
amplihack profile root [<dir>]
amplihack profile show
amplihack profile switch <name> | --none
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
amplihack profile overflow add|remove|clear <name>
amplihack profile overflow list
amplihack profile rotate [--dry-run] [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1]
amplihack profile rotate start|watch [<tuning flags>] [--interval SECS]
amplihack profile rotate stop|status
amplihack profile rotate autostart [on|off]
amplihack profile kind list
amplihack profile kind set <name> <subscription|api-key|auto>
amplihack profile overflow clear --platform
amplihack launch|claude --profile <name> [...]
```

## Subcommands

| Command | Behavior |
|---------|----------|
| `add <name>` | Copy allowlisted shared assets from `~/.claude` into a private staging directory, then atomically publish `<root>/<name>/` (mode `0700` on Unix; Windows uses its inherited ACL). Errors if a profile exists whose name matches case-insensitively, on every platform. Does not run Claude. Then links `projects/`, `file-history/` and `session-env/` to `~/.claude` when it exists; see [Shared session directories](#shared-session-directories). The first `add` also saves the root it used; see [Saved on the first add](#saved-on-the-first-add). |
| `root [<dir>]` | Print the profile root in use and where it came from, or save `<dir>` as the root. See [Profile root](#profile-root). |
| `list` | First line `root: <path> (source: env\|config\|default)`; see [Profile root](#profile-root). Then one profile per line, sorted; the default is marked `*`. Skips dot-entries, symlinks and non-directories. Reports a corrupt or dangling default pointer instead of silently omitting the marker. |
| `show` | Prints `name: <name>` and `dir: <path>`, or `none`. Paths only; never directory contents. |
| `switch <name>` | Moves the profile's login into `~/.claude` with the rotator's [swap](#rotate-swap), under the rotator's lock, then pins rotation to it, writes the default pointer and records the switch in the rotate ledger, so sessions stopped on a usage limit before it are resumed. The profile must exist. Then checks the [shared session links](#shared-session-directories) of both profiles in the swap; messages go to stderr. When the live login cannot be moved (any system but Linux, a profile open in a session, no login in `~/.claude`, ...), it still writes the default pointer, holds nothing and exits 1. See [Rotate switch](#rotate-switch). |
| `switch --none` | Removes the default pointer and the [rotation pin](#rotate-pin). Prints `Default profile cleared.`, or `No default profile was set.` when there was none, plus `Rotation released; the rotator picks the best login on its next cycle.` when a pin was removed; exits 0 either way. Moves no login. Conflicts with a name; `switch` with neither is a usage error. See [Clearing the default](#clearing-the-default). |
| `remove <name>` | Requires `--yes`. Resolves and verifies the path is inside the root, reads the default pointer, deletes the directory, then clears a matching pointer. A missing pointer means no default; a corrupt, unreadable or dangling pointer fails closed before any deletion (repair with `switch`). Never deletes the root; never follows symlinks, so the shared session links are removed and the sessions in `~/.claude` are kept. `*.pre-share-*` backups inside the profile are deleted with it. |
| `login <name>` | Checks the profile's [shared session links](#shared-session-directories), then always runs the `claude` binary (ignores binary selection), interactively, with `CLAUDE_CONFIG_DIR=<profile dir>`. Then refuses a login whose account another profile or `~/.claude` already holds, and restores the previous login. See [Login check](#login-check). |
| `run <name> -- <args>` | One-shot launch through the normal launch path. Args are forwarded as a literal argv vector; no shell is involved. |
| `usage [<name>...]` | Asks `claude` for each profile's usage, a few profiles at a time, and shows its session and weekly usage, most capacity first. `--json` for scripts, `--raw` to include Claude's output. Refreshes auto-switch readings. See [Usage command](#usage-command). |
| `auto-switch …` | Turn [auto-switch](#auto-switch) on or off, set its threshold and exclusions, show its state, or install and repair the status-line wrapper. See [Auto-switch commands](#auto-switch-commands). |
| `overflow …` | Mark or unmark a credit-backed profile that the rotator may use when every plan login is exhausted, list the marks, or lift a spend-limit block. See [Overflow commands](#overflow-commands). |
| `rotate …` | Keep the login in `~/.claude` on the account with the most capacity: once, as a dry run, or in a background process with `start`, `stop` and `status`; `autostart` controls whether launches start that process. See [Rotate](#rotate). |
| `kind list`, `kind set <name> <kind>` | Show whether each profile is a subscription or an [API-key profile](#api-key-profiles), and set or remove an override. See [Kind override](#kind-override). |

`remove` and `login` are also refused while auto-switch has an account's
login away from home; see [Leases](#leases). `run`, `switch` and
`--profile` launches refuse a subscription profile with no OAuth login of
its own on Unix; see [No silent billing](#no-silent-billing).

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
| `--raw` | Also print what `claude` wrote to stdout and stderr, for each profile, from the [attempt](#probe-method) that gave the row. Without it, that output is dropped once parsed. It can include account details; see [Raw output](#raw-output). |
| `<name>...` | Probe only these profiles, in the [usual order](#ordering). |

#### How a profile is probed

For each profile, amplihack runs the `claude` binary directly, as `login`
does (the binary selection is ignored, and `AMPLIHACK_CLAUDE_BINARY_PATH` is
honoured). A probe makes at most two attempts, one after the other, within
one [deadline](#probe-deadline):

1. **Usage request.** `claude` is started as

   ```
   claude -p --input-format stream-json --output-format stream-json --verbose
   ```

   and amplihack writes exactly one line to its stdin, then closes it:

   ```
   {"type":"control_request","request_id":"amplihack-usage","request":{"subtype":"get_usage","skip_behaviors":true}}
   ```

   `claude` answers with its session and weekly figures as JSON. It does not
   appear to scan the session transcripts to answer, or to make a model
   call.
2. **`/usage` text.** Only when the usage request gave no
   [accepted answer](#probe-method), and the deadline has not passed,
   `claude` is started again as

   ```
   claude -p /usage
   ```

   with stdin from `/dev/null`, and its text is [read](#what-is-read).

Profiles marked as [overflow](#rotate-overflow) skip the first attempt:
they are always probed with `/usage` only, whatever the
[method](#probe-method), because only `/usage` reports a used-up spend
limit. Under `text`, every profile skips it.

Both children get `CLAUDE_CONFIG_DIR=<profile dir>` on their own `Command`;
the environment of amplihack itself is never changed. The one exception is a
profile whose login is [live in `~/.claude`](#logins-in-claude): its
children have `CLAUDE_CONFIG_DIR` removed instead, so `claude` reads
`~/.claude` and `~/.claude.json`. It is never set to `~/.claude`: when
`CLAUDE_CONFIG_DIR` is set, `claude` reads `.claude.json` from inside that
directory (`~/.claude/.claude.json`), not the `~/.claude.json` the default
login uses, so it would report the wrong account or none. Removing the
variable is exactly what "use the default directory" means. Those children
still get `AMPLIHACK_CLAUDE_PROFILE=<name>`, like every other probe. There is no
shell, and no arguments other than the ones above are passed. The profile
directory is checked again just before each spawn, as for every launch. A
refused directory gives `error` (`profile directory refused`), and no further
attempt is made.

For neither attempt does amplihack create, copy, link, move or write
anything in the profile directory, and it never opens or reads a login file
to probe it. `claude` reaches the login in place, as it does for any
`claude` run with that `CLAUDE_CONFIG_DIR`, and may update its own files
there, such as `.claude.json`, as any `claude` run does.

- Profiles are probed in parallel, at most the
  [parallelism bound](#probe-parallelism) at a time. The extra `~/.claude`
  row, when there is one, is probed after the others.
- The [deadline](#probe-deadline), 120 seconds by default, runs from the
  first spawn and covers both attempts. The `/usage` attempt gets only the
  time left; when none is left, the state is `timeout`. When the deadline
  passes, the running child is killed. Output from a `/usage` attempt that
  timed out is never parsed.
- A usage request still running at the deadline without an accepted answer
  gives `timeout`, and `/usage` is not tried.
- After an accepted answer, amplihack waits for `claude` to exit rather than
  kill it, so that `claude` can finish writing its own files. A `claude`
  that answers but does not exit keeps its place in the
  [parallelism bound](#probe-parallelism) until the deadline. It is then
  killed and its answer is used: the state comes from the answer, not
  `timeout`.
- On Unix each child runs in its own process group. When an attempt ends,
  whether by the deadline or a normal exit, the whole group is killed with
  `SIGKILL`, so processes `claude` started (MCP servers, for example) do not
  outlive it. A process that left the group itself is not tracked. On other
  platforms only the child is killed. A kill at the deadline can interrupt a
  write `claude` is making to its own files, such as `.claude.json`.
- stdout and stderr are each read up to 64 KiB per attempt. Anything after
  that is dropped and the rest is still parsed; a usage request answer cut
  off by the limit is not accepted.
- Neither attempt appears to use quota: the usage request does not appear
  to make a model call, and an exhausted session still answers `/usage`
  without its session percentage changing. This is `claude`'s behaviour, not something amplihack
  controls. `claude` contacts Anthropic to answer; amplihack itself makes no
  network calls.

The `/usage` attempt makes `claude` read every session transcript reachable
from the profile's `projects` directory. With large transcripts that is
over half a minute of CPU time for one probe, and one probe was measured at
71.5 seconds on a 72-CPU host at a load average of 194. The usage request
does not appear to make that scan, though it still pays for `claude`
starting up. The deadline and the number of probes run at once are still
sized for the `/usage` attempt on a loaded host, because it remains the
fallback and the only method for overflow profiles. `profile usage` and the
[rotator](#rotate-cycle) use the same three settings:
[method](#probe-method), [deadline](#probe-deadline) and
[parallelism](#probe-parallelism).

#### Probe method

`claude` prints its answer to the usage request as one JSON line. Shown over
several lines, with fields amplihack does not read left out, an answer looks
like this:

```json
{
  "type": "control_response",
  "response": {
    "subtype": "success",
    "request_id": "amplihack-usage",
    "response": {
      "rate_limits_available": true,
      "rate_limits": {
        "five_hour": { "utilization": 20.0, "resets_at": "2026-10-17T02:00:00Z" },
        "seven_day": { "utilization": 30.0, "resets_at": "2026-10-22T09:00:00Z" },
        "model_scoped": [
          { "display_name": "Fable", "utilization": 0.0, "resets_at": "2026-10-22T09:00:00Z" }
        ],
        "extra_usage": { "is_enabled": false }
      }
    }
  }
}
```

The *answer* is the first line of stdout that is a JSON object with
`"type": "control_response"` and a `response.request_id` of
`"amplihack-usage"`. Lines before it, such as the `system` line `claude`
prints when it starts, are skipped. Lines after it are ignored, even another
line with the same `request_id`. The figures are in `response.response`,
called the *payload* below; every field named in the rest of this section is
inside it. An answer that failed has `response.subtype` `"error"` and a
message in `response.error`; amplihack never parses that message or puts it
in a message of its own.

The answer is *accepted* only when all of these hold:

- `response.subtype` is `"success"`;
- the payload, `response.response`, is an object;
- `rate_limits_available` is the JSON value `true` (not `1` or `"true"`),
  and `rate_limits` is an object;
- `extra_usage` is absent, `null`, or an object whose `is_enabled` is the
  JSON value `false`, both at the top of the payload and at
  `rate_limits.extra_usage`;
- `rate_limits.five_hour.utilization` or `rate_limits.seven_day.utilization`
  is a finite number from `0` to `1000000`.

Claude Code reports whether the account has usage credits turned on at
`rate_limits.extra_usage.is_enabled`. The top-level `extra_usage` is
checked as well, in case a later version moves it. The rule is strict: an
answer with no `extra_usage`, or a `null` one, is accepted, but an
`extra_usage` of `true`, an object with no `is_enabled`, or an `is_enabled`
of `"false"` or `0` is not.

An accepted answer becomes the row like this:

| Payload field | Row field |
|---------------|-----------|
| `rate_limits.five_hour` | `session`, label `session` |
| `rate_limits.seven_day` | `week_all`, label `all models` |
| each `rate_limits.model_scoped` entry with a valid `display_name` and a valid `utilization` | one `week_models` entry, labelled with its `display_name`, in the order given; any other entry is skipped |
| `utilization` | `used_percentage`, as read; `-0` reads as `0` |
| `resets_at`, a valid string | `resets_text`, the string as received; `resets_at`, that time in Unix epoch seconds |

A `display_name` or `resets_at` string is valid only when it is 1 to 64
characters long and has no control characters (C0, DEL or C1, so no escape
sequences) and no bidirectional override or isolate characters (U+202A to
U+202E, U+2066 to U+2069). This stops an answer from putting terminal
escapes or reordered text into the table or the JSON. Letters of any script
are kept, as are the directional marks U+200E, U+200F and U+061C.

A window whose `utilization` is missing, `null`, not a number, negative or
above `1000000` is left out (`null`), never read as `0`. For a window that
is kept:

- When the window's `resets_at` is missing, `null`, not a string (a number
  of epoch seconds, for example) or not a valid string, the row's
  `resets_text` and `resets_at` are both `null`.
- When it is a valid string, `resets_text` is that string. `resets_at` is
  `null` when the string is not an RFC 3339 time, or when the time is more
  than 8 days from now: no session or weekly window resets that far ahead,
  so such a time is treated as unknown rather than trusted.

This page shows such resets as `2026-10-16T23:50:00Z`. That is an example
of the form: whether `claude` sends fractional seconds, `Z` or an offset
such as `+00:00` is up to `claude`, and `resets_text` keeps what it sent.
The other windows in the payload (`seven_day_opus`, `seven_day_sonnet`,
`seven_day_oauth_apps`) are not used.

The state of an accepted answer is `session_exhausted` when the session is
at 100% or more, else `weekly_exhausted` when the all-models week is, else
`ok`. The usage request never gives `spend_limit`, `not_logged_in` or
`unparsed`: those states come only from the `/usage` text.

`claude` can answer from figures it stored earlier, without saying how old
they are. `/usage` shows the same stored figures, so amplihack treats both
alike and adds no age check.

Any other outcome of the usage request, apart from reaching the deadline,
falls back to `/usage` in the time left:

| Usage request outcome | Why it falls back |
|-----------------------|-------------------|
| `response.subtype` is not `success`, such as an `error` with `Unsupported control request subtype` from a `claude` that does not know `get_usage` | The installed `claude` cannot answer it. |
| No line of stdout is a `control_response` for `amplihack-usage`: none was printed, the output is not JSON, or the answer was cut off at 64 KiB | The answer is missing. |
| `response.response` is missing or not an object | The answer has no payload. |
| `rate_limits_available` is not `true`, or `rate_limits` is not an object (`null`, for example) | `claude` has no figures, for example because the profile is not logged in. `/usage` can then report `not_logged_in`. |
| Neither the session nor the all-models week has a valid `utilization` | Nothing usable was read. |
| `extra_usage`, at the top of the payload or at `rate_limits.extra_usage`, is not absent, `null` or an object with `"is_enabled": false`; for example `{"is_enabled": true}` | The account has usage credits turned on, or the answer reports `extra_usage` in a form amplihack does not recognise. The payload has no spend-limit field, and `/usage` shows a used-up spend limit as `spend_limit`. |
| `claude` could not be started, written to, read or waited for | The attempt failed. |

Profiles marked as overflow are probed with `/usage` only. They never get
the usage request, under `auto` as under `text`, because the
[spend-limit block](#overflow-spend-limit-block) depends on `/usage`
reporting `spend_limit`. The marks are read from `<root>/.auto-switch`
before probing; a missing or corrupt `.auto-switch` marks no profile, so
every profile gets the usage request. A profile that is not marked but
whose account has usage credits turned on gets the usage request, its
answer is not accepted, and it falls back to `/usage`.

`get_usage` is part of an interface that Claude Code marks as experimental,
and it may change or be removed. A change that makes its answer
unrecognisable only makes every probe fall back to `/usage`. To skip the
usage request altogether:

| Setting | Values | Default |
|---------|--------|---------|
| `AMPLIHACK_PROFILE_USAGE_PROBE` | `auto`, `text` | `auto` |

`auto` sends the usage request first and falls back to `/usage`. `text` runs
only `claude -p /usage`, so every probe pays the transcript scan. Values are
matched exactly: case, leading and trailing spaces count. An empty variable
counts as unset. Any other value prints one warning to stderr,
showing the value escaped and cut to 32 characters, then uses `auto`:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_PROBE "Text" (use auto or text); using auto
```

There is no command-line flag for it. Whether a `claude` supports the usage
request is not remembered: every probe tries it again under `auto`.

To check that both methods give the same figures for your logins, see
[Compare the two probe methods](../howto/claude-profiles.md#compare-the-two-probe-methods).

#### Probe deadline

| Setting | Values | Default |
|---------|--------|---------|
| `--timeout-secs <n>` (hidden flag, `profile usage` only) | `1` to `600` | none |
| `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` | `1` to `600` | `120` |

The deadline bounds the whole probe of one profile: the usage request and,
when it falls back, the `/usage` attempt in the time left. The default of
120 seconds allows about 1.7 times the 71.5 seconds one `/usage` probe was
measured to take on a 72-CPU host at a load average of 194.

The flag wins over the variable. A flag value out of range is a usage error
(exit status 2). An empty variable counts as unset. A variable that is not a
number from `1` to `600` prints one warning to stderr, showing the value
escaped and cut to 32 characters, then uses `120`:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS "soon" (use 1 to 600); using 120
```

The rotator has no flag; see
[Probe settings in the rotator](#probe-settings-in-the-rotator).

#### Probe parallelism

Unless `AMPLIHACK_PROFILE_USAGE_PARALLEL` is set, the number of probes run at
once is derived from the host:

```
cpus  = number of CPUs available to amplihack
load1 = first field of /proc/loadavg (the one-minute load average)
free  = max(cpus - load1, 0)
bound = clamp(floor(free / 8), 2, 4)
```

Where `/proc/loadavg` is missing or cannot be read (any platform other than
Linux, for example), `free` is `cpus`. A load value that is empty, negative
or not a finite number counts as missing.

| Host | `free` | Bound |
|------|--------|-------|
| 72 CPUs, idle | 72 | `4` |
| 72 CPUs, load 38 | 34 | `4` |
| 72 CPUs, load 194 | 0 | `2` |
| 8 CPUs, idle | 8 | `2` |

The floor is 2, not 1. Under load 194 a bound of 1 would make a cycle over
14 profiles take about 16 minutes. Two probes add little to a host already
that busy, and they halve that time. The ceiling of 4 keeps an idle host
from starting a burst of transcript scans at once.

| Setting | Values | Default |
|---------|--------|---------|
| `AMPLIHACK_PROFILE_USAGE_PARALLEL` | `1` to `16` | the derived bound |

A valid value replaces the derived bound. An empty variable counts as unset,
as it does for the deadline. Any other value, including `0` and one that is
not valid UTF-8, prints one warning to stderr, showing the value escaped and
cut to 32 characters, then uses the derived bound:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL "lots" (use 1 to 16); using 2
```

The number after `using` is the derived bound from the formula above, before
the cap described next, so it can be larger than the number of profiles.

Whichever value applies is then capped at the number of profiles to probe,
and is never below 1. There is no command-line flag for it. The bound counts
probes, not attempts: a profile's `/usage` attempt starts only after its
usage request has ended, so each probe runs at most one `claude` at a time.

#### Probe settings in the rotator

The rotator uses the same [method](#probe-method),
[deadline](#probe-deadline) and [parallelism](#probe-parallelism) as
`profile usage`, from the same three variables. It has no flags for them.

- **When they are read.** At the start of every cycle, from the rotator's
  own environment. That environment is fixed when the rotator starts: a
  background rotator inherits it from the shell that ran
  `amplihack profile rotate start`, or from the launch that
  [autostarted](#rotate-autostart) it. To change a setting, set the variable
  in a shell, then run `amplihack profile rotate stop` and
  `amplihack profile rotate start` from that shell. The variables the
  background rotator removes are listed under [Rotate daemon](#rotate-daemon);
  these three are not among them.
- **Warnings.** An invalid value prints its warning once per cycle (to
  `rotate.err` for a background rotator), and the default is used.
- **The live probe counts against the bound.** When the live login is
  probed, the bound is derived for the candidates plus one. The live probe
  and the candidate probes share one pool of that size: the live probe
  starts first, and whenever a probe ends, the live one included, the next
  candidate takes its place. With a bound of 1, the live probe runs first
  and the candidates follow one at a time. Either way a cycle never runs
  more probes at once than the bound. An [API-key](#api-key-profiles) live
  login is not probed, so then the bound is derived for the candidates
  alone.
- **Load.** The bound is derived again on every cycle, so it follows the
  load as it changes.

A cycle under heavy load can take longer than `--interval`. Cycles never
overlap: the rotator waits for `--interval` seconds after a cycle ends before
it starts the next one.

#### What is read

This section covers the `/usage` attempt; an accepted usage request is read
as in [Probe method](#probe-method). These lines of the `/usage` stdout are
read. Each one has a percentage and usually a reset time:

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
| `ok` | The session or the all-models week was read, from an accepted usage request or a `/usage` line, and neither is at 100%. |
| `session_exhausted` | The session is at 100% or more. |
| `weekly_exhausted` | The all-models week is at 100% or more and the session is not. |
| `spend_limit` | The `/usage` output reported that the organisation's spend limit was reached. Any percentages read are kept. |
| `not_logged_in` | The `/usage` output had no usage line of any kind (model week lines included) and reported that the profile is not signed in. Run `amplihack profile login <name>`. A profile [live in `~/.claude`](#logins-in-claude) is probed against `~/.claude`, so its missing `.credentials.json` does not give this state. |
| `timeout` | The probe passed its [deadline](#probe-deadline), in either attempt. The account's usage is unknown. |
| `unparsed` | The `/usage` output had nothing recognisable. Run again with `--raw` to see what Claude said. |
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

#### Failed and exhausted results

A probe that timed out or could not run says nothing about the account. Its
usage is unknown, and it is never reported or treated as 100%. An exhausted
result is a reading: the account answered, and a window is at 100% or more.

| Result | `profile usage` table | `--json` | Rotator cycle line | Rotator decision | Next probe |
|--------|-----------------------|----------|--------------------|------------------|------------|
| `timeout` | state `timeout`, `-` in every percentage and reset | `"state": "timeout"`, `session`, `week_all` and `error` `null`, `week_models` empty | `<name>:failed` | Unknown: never a target, never counted as exhausted; a plan candidate in this state blocks a move onto [overflow](#overflow-decision) | Next run or cycle; no failed result is kept |
| `error` | state `error`, `-` in every percentage and reset; reason on stderr | `"state": "error"`, `session` and `week_all` `null`, `week_models` empty, reason in `error` | `<name>:failed` | As for `timeout`, except `profile directory refused`, which does not block a move onto overflow (see [Overflow decision](#overflow-decision)) | As for `timeout` |
| `session_exhausted`, `weekly_exhausted` | the percentages read | the windows read | `<name>:s=100%,w=100%`, whatever the windows read | Headroom `0`: never a target | Next run or cycle |

`spend_limit`, `not_logged_in` and `unparsed` are also shown as
`<name>:failed` in the cycle line and are never a target; see
[Decision rule](#rotate-decision-rule). Unlike `timeout` and `error`, they do
not block a move onto overflow, so a profile that is never logged in cannot
keep the rotator off credits for good. A failed live probe is handled as in
[Failed live probes](#rotate-failed-live-probes). Neither `profile usage` nor
the rotator records a `timeout` or `error` result as an auto-switch
[reading](#readings), or as exhausted anywhere.

The rotator writes one line to stderr (`rotate.err` for a background
rotator) for each `timeout` or `error` result, with `~/.claude` as the name
of the live login. Both are fixed text:

```
warning: rotate: probe of <name> timed out
warning: rotate: probe of <name> failed: <reason>
```

The reasons for `error` are fixed text, never the underlying I/O error. A
failure to start, write to, read or wait for the usage request is not an
`error`: it falls back to `/usage`. So every reason below except
`profile directory refused` comes from the `/usage` attempt.

| Reason | When |
|--------|------|
| `profile directory refused` | The profile cannot be resolved, or its directory fails the check just before spawn. |
| `could not start claude` | Spawning `claude` failed. |
| `could not read claude output` | Reading the child's stdout or stderr failed, or a pipe was still open 2 seconds after the child exited (a process that left its group still held it), so the output may be cut short. |
| `could not wait for claude` | Waiting for the child failed. It is killed and reaped first. |

#### Table output

```
PROFILE   MARKS             STATE              SESSION  SESSION RESETS        WEEK  WEEK RESETS
review    -                 ok                 5%       2026-10-17T04:00:00Z  10%   2026-10-22T09:00:00Z
work      * hosts:personal  ok                 20%      2026-10-17T02:00:00Z  30%   2026-10-22T09:00:00Z
team      -                 session_exhausted  100%     2026-10-16T23:50:00Z  68%   2026-10-22T09:00:00Z
personal  leased→work       not_logged_in      -        -                     -     -
spare     -                 timeout            -        -                     -     -
```

`SESSION` and `WEEK` are the session and all-models week. Percentages print
as a whole number when they are whole and with one decimal place otherwise
(`12.5%`). `-` means no value. A `timeout` or `error` row shows `-` in every
percentage and reset column: its usage is unknown. Model-specific week lines
are not in the table; they are in the JSON.

Reset columns show the reset as received, so their form depends on the
[attempt](#probe-method) that gave the row:

| Row from | Reset column | Example |
|----------|--------------|---------|
| The usage request | The `resets_at` string `claude` sent, an ISO 8601 time; `-` when it sent none or not a string | `2026-10-16T23:50:00Z`, an example of the form ([details](#probe-method)) |
| `/usage` | Claude's text as printed | `Oct 16, 4:50pm (America/Los_Angeles)` |

Text from Claude and from state files is cleaned before it reaches the
terminal: control characters (including escape sequences) and the
bidirectional override and isolate characters U+202A to U+202E and U+2066
to U+2069 are replaced with `?`, and reset
text longer than 64 characters is cut to 64 and followed by `…`. The table's
layout is for people and may change; scripts should use `--json`.

For each `error` row, one line goes to stderr, with or without `--json`:

```
warning: profile "review": could not start claude
```

#### Raw output

With `--raw`, each row is followed by what `claude` printed in the attempt
that gave the row, indented and cleaned of control characters other than
newlines. Each stream is shown up to the 64 KiB read for that attempt. For
a row from the usage request, that is the attempt's whole stdout and stderr:
every JSON line `claude` printed, from the `system` line it prints when it
starts, through the `control_response` answer, to any lines after it. For a
row from `/usage`, it is the text, as in this run with
`AMPLIHACK_PROFILE_USAGE_PROBE=text`:

```
team      -                 session_exhausted  100%     Oct 16, 4:50pm (America/Los_Angeles)  68%   Oct 22, 2am (America/Los_Angeles)
  raw stdout:
    You are currently using your subscription to power your Claude Code usage

    Current session: 100% used · resets Oct 16, 4:50pm (America/Los_Angeles)
    Current week (all models): 68% used · resets Oct 22, 2am (America/Los_Angeles)
  raw stderr:
    (empty)
```

Output from a usage request that fell back is not shown. Raw output can
include account details, such as your e-mail address, organisation name or
subscription type; read it before you share it. amplihack prints it only to
stdout and never writes it to `usage.json`, the rotate log or any other
file, and the rotator never collects it.

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
| `overflow` | The profile is marked as [credit-backed overflow](#rotate-overflow) in `<root>/.auto-switch`. |
| `overflow, blocked until <time>` | As `overflow`, and the rotator has [blocked](#overflow-spend-limit-block) it after a spend-limit reading until `<time>` (UTC, `YYYY-MM-DDTHH:MM:SSZ`). |

Several marks are separated by a space (`* live in ~/.claude`); none
prints `-`. A row marked `live in ~/.claude` is placed by its state and
numbers like any other row. An overflow row is placed by its state like any
other row too; its session and week numbers are its plan limits, which it can
work past on credits.

When the row marked `live in ~/.claude` is also marked `overflow`, and that
row's own probe in this run reached the session or weekly limit (state
`session_exhausted` or `weekly_exhausted`, or the session or all-models
weekly window at 100%), the table is followed by one line:

```
running on overflow: ryan-mistt-org
```

A live overflow row whose probe shows plan capacity left prints no footer:
work on it is still covered by the plan. A full per-model weekly window
(for example `Fable`) does not count, because the rotator's capacity test
reads only the session and all-models weekly windows. A live overflow row whose probe
failed prints no footer either, because nothing in the run shows the limit
was reached.

The overflow marks come from `<root>/.auto-switch`, read once before
probing; the block comes from the rotate ledger, read at the same time. A
corrupt `.auto-switch` gives no overflow marks, and a rotate ledger that
cannot be read gives no block, each with a warning on stderr. The `live in
~/.claude` mark, and so the footer, needs the live profile to be identified
by account; see [Logins in `~/.claude`](#logins-in-claude).

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
   absent. One whose `account` is not the account in `~/.claude.json`
   (someone ran `/login` in `~/.claude` and the rotator has not yet
   [recovered](#rotate-hand-change-recovery)) does not decide either; the
   account in `~/.claude.json` is matched against every profile instead,
   with or without its own login, by the
   [account identity rule](#account-identity):

   | Profiles with the account in `~/.claude.json` | Result |
   |-----------------------------------------------|--------|
   | Exactly one, `M` | `M` is live, even if it has its own `.credentials.json`. Its row is probed against `~/.claude` and marked `live in ~/.claude`. |
   | None, and no profile names were given | An extra row `~/.claude` is added, probed against `~/.claude`, with `profile`, `account` and `live_in` all `"~/.claude"` in the JSON (see the [key table](#json-output)). It is never the default and is never leased. |
   | Anything else (several, an unreadable account, or names were given) | The ledger counts as absent, as before. |

   This applies to `profile usage` only. The live-profile launch, the
   `profile login` refusal and the duplicate-login warning still go by the
   ledger, so the warning for `~/.claude` and `M` is still printed while
   both copies exist. The `~/.claude` row is never recorded as an
   auto-switch reading. Both ledgers are only read. The deciding ledger's `live` field names the profile
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

   Apart from the `profile rotate` ledger's `account` check above, the
   ledger is taken at its word: amplihack does not compare the account in
   `~/.claude.json` with the named profile's account. If the prototype
   ledger names the wrong profile (the rotator stopped partway through a
   swap, or someone ran `/login` in `~/.claude`), that profile's row shows
   the numbers of the login that is actually in `~/.claude`.

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
| `warning: profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); fix: …` | Two logins are the same account. One line per pair, after the other warnings. See [Duplicate-login warning](#duplicate-login-warning). Printed for `~/.claude` and `M` while a manual `/login` in `~/.claude` has left both with the same account; the next rotate cycle moves `M`'s copy aside when it [safely can](#rotate-hand-change-recovery). |

A manual `/login` in `~/.claude` replaces whatever login was there; the
profile that login belonged to is left with no login. `profile usage` prints
no warning of its own about that; the rotator does, in its log and in
[`rotate status`](#rotate-status).

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
    "overflow": false,
    "overflow_blocked_until": null,
    "state": "ok",
    "session": {
      "label": "session",
      "used_percentage": 20.0,
      "resets_text": "2026-10-17T02:00:00Z",
      "resets_at": 1792202400
    },
    "week_all": {
      "label": "all models",
      "used_percentage": 30.0,
      "resets_text": "2026-10-22T09:00:00Z",
      "resets_at": 1792659600
    },
    "week_models": [
      {
        "label": "Fable",
        "used_percentage": 0.0,
        "resets_text": "2026-10-22T09:00:00Z",
        "resets_at": 1792659600
      }
    ],
    "error": null
  }
]
```

| Key | Type | Meaning |
|-----|------|---------|
| `profile` | string | Profile name, or the literal `~/.claude` for the extra row added when the account in `~/.claude` [belongs to no profile](#logins-in-claude) after a manual `/login`. No profile can have that name, because it fails the [name rules](#name-rules). |
| `account` | string | The account whose login is live in the profile's directory: the profile's own name unless it [hosts](#leases) another account. The numbers belong to this account. The literal `~/.claude` on the extra `~/.claude` row. |
| `default` | bool | Whether this is the default profile. |
| `leased_to` | string or `null` | The directory this profile's login is lent to. |
| `live_in` | string or `null` | `"~/.claude"` when this profile's login is [live in `~/.claude`](#logins-in-claude) and its numbers were read from there; otherwise `null`. The key is always present, and at most one row has a non-null value. The value is always the literal `~/.claude`, never the expanded home path. |
| `overflow` | bool | Whether the profile is marked as [credit-backed overflow](#rotate-overflow). Always present. |
| `overflow_blocked_until` | integer or `null` | Unix epoch seconds until which the rotator has [blocked](#overflow-spend-limit-block) this overflow profile after a spend limit; `null` when it is not blocked or not marked. Always present. |
| `state` | string | One of the [states](#states). |
| `session` | window or `null` | The session: `rate_limits.five_hour` from the [usage request](#probe-method), or the `Current session` line from `/usage`. Always `null` for `timeout` and `error`. |
| `week_all` | window or `null` | The all-models week: `rate_limits.seven_day`, or the `Current week (all models)` line. Always `null` for `timeout` and `error`. |
| `week_models` | array of windows | Each usable `rate_limits.model_scoped` entry, or every other `Current week (<model>)` line, in the order given. Empty if none. |
| `error` | string or `null` | The [reason](#states) for an `error` row, otherwise `null`. |
| `raw` | object | Only with `--raw`: `{"stdout": "...", "stderr": "..."}` from the attempt that gave the row; see [Raw output](#raw-output). The key is absent, not `null`, without `--raw`. |

A window is `{label, used_percentage, resets_text, resets_at}`. `label` is
`session` for the session window, `all models` for the all-models week, and
the model name as given (`display_name`, or the name `/usage` printed, for
example `Fable`) for a model week. `used_percentage` is a number as read
(not clamped). `resets_text` is the reset as received, an ISO 8601 time from
the usage request or Claude's text from `/usage`, and `null` when there was
none. `resets_at` is Unix epoch seconds, or `null` when there was no reset or
it could not be resolved (see [Probe method](#probe-method) and
[What is read](#what-is-read)). JSON strings are not cleaned; they use JSON
escaping.

The output stays a single array; there is no top-level object and no
`on_credits` key. The live login is running on credits exactly when one row
has `live_in` equal to `"~/.claude"`, `overflow` equal to `true`, and
either a `state` of `"session_exhausted"` or `"weekly_exhausted"`, or a
`state` of `"ok"` with `session` or `week_all` at 100%. This is the same
test the footer uses:

```bash
amplihack profile usage --json | jq -r '.[]
  | select(.live_in == "~/.claude" and .overflow
           and (.state == "session_exhausted" or .state == "weekly_exhausted"
                or (.state == "ok"
                    and ((.session.used_percentage // 0) >= 100
                         or (.week_all.used_percentage // 0) >= 100))))
  | .profile'
```

```
ryan-mistt-org
```

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
quiet period after a swap does not apply: the probe asks `claude` under
the live login directly, rather than waiting for a status-line update. Any failure while recording prints one
`auto-switch:` warning to stderr and the exit status stays 0.

After `usage.json` is written, `profile usage` also updates the
[capacity file](#capacity-file) when the switch is on.

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
- The usage request is a fixed line with no profile name, path, e-mail
  address or token in it. The `system` line `claude` prints when it starts
  (its working directory, session id and tool paths) and any
  `response.error` message are never parsed into a message or stored; they
  appear only in `--raw` output, and the rotator never collects them.
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
| `<root>/<name>/{projects,file-history,session-env}` | Symlinks to `~/.claude/<same name>`. See [Shared session directories](#shared-session-directories). |
| `<root>/<name>/<dir>.pre-share-<YYYYMMDDTHHMMSSZ>` | What a migration of `<dir>` left behind. Never deleted automatically. |
| `<root>/<name>/.no-shared-sessions` | Opt-out marker. When present, any file type, the profile's session directories are never linked or migrated. See [Preconditions](#preconditions). |
| Root directory | Created with mode `0700` on Unix; Windows uses its inherited ACL. A symlinked root is refused. An existing custom root is trusted input. |
| `<root>/.default` | Pointer file: only the profile name (mode `0600` on Unix, max 256 bytes). Windows uses its inherited ACL. Written by creating a new temp file (`create_new`, never through a symlink) and renaming it over the pointer. |
| `AMPLIHACK_CLAUDE_AUTO_SWITCH` | `1` turns auto-switch on and `0` turns it off for this process and its children, overriding `<root>/.auto-switch`. Empty means unset. See [Toggle and precedence](#toggle-and-precedence). |
| `<root>/.auto-switch` | Auto-switch settings. See [Settings file](#settings-file). |
| `<root>/.auto-switch.d/` | Auto-switch state. See [State directory](#state-directory). |
| `AMPLIHACK_ROTATE_AUTOSTART` | `0`/`off` stops launches from starting the rotator, and `1`/`on` lets them, overriding `<root>/.auto-switch.d/rotate/autostart`. Empty means unset. See [Rotate autostart](#rotate-autostart). |
| `AMPLIHACK_USAGE_LIMIT_WAIT` | `true` or `1` turns the [usage-limit wait](#usage-limit-wait) on, `false` or `0` off. Outranks `recipe.usage_limit_wait` in `config.toml`. See [Usage-limit wait settings](#usage-limit-wait-settings). |
| `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | Upper bound on one usage-limit wait, in seconds. Only shortens the wait. Outranks `recipe.usage_limit_max_wait_secs`. |

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
- `amplihack install` (user scope) refreshes the hooks in every profile's
  existing `settings.json` (issue #1661): it applies the same hook update as
  for `~/.claude/settings.json`, so amplihack's entries are replaced and all
  other keys and hooks are kept. Only real profile directories directly in
  the root are visited; the file must be a regular file owned by the user
  (never a symlink), is rewritten atomically with mode `0600` only when it
  changes, and is never created. A file that cannot be read, parsed or
  written is reported and skipped; the install still succeeds.
- Before it opens any profile's `settings.json`, the refresh checks the hooks
  path it is about to write (issue #1699). It refuses the whole refresh when
  the `amplihack-hooks` binary does not exist or is not a regular file (a
  symlink to a regular file is accepted). It also refuses it, wherever the
  binary lies, when `$HOME` is not the account's home directory from the
  passwd database and the profile root is outside `$HOME`: such a `HOME` is a
  scratch one, whether under `/tmp` or under a work directory such as
  `~/.amplihack/work`, and the profiles belong to the account. This check is
  skipped when the account has no passwd entry, and on Windows. Otherwise it
  refuses when the binary's canonical path
  lies under a temporary directory (`/tmp`, an absolute `$TMPDIR`, or the
  platform temp dir), unless the profile root lies both under `$HOME` and
  under a temporary directory. That exception is the layout of an isolated
  test install, where `HOME` and the profile root are both temporary. Two
  cases are refused in particular: a test or scratch install with a
  throwaway `HOME` that inherited a real `AMPLIHACK_CLAUDE_PROFILE_ROOT`
  (for example `HOME=~/.amplihack/work/x/home amplihack install`), and a
  real install whose
  hooks binary is a temporary build (deploying the binary failed, so install
  fell back to a source binary such as one named by
  `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` under `/tmp`). `$HOME` unset also
  refuses a temporary hooks path. The path as written is
  checked as well, so a symlink placed in a temporary directory is refused
  even when it points elsewhere. A refusal writes nothing; each profile is
  reported with one line, and the install still succeeds:

  ```text
  Profile work: settings.json not refreshed (HOME /home/user/.amplihack/work/x/home is not the account's home /home/user; profile root /home/user/.claude-profiles is outside HOME)
  Profile work: settings.json not refreshed (hooks binary /tmp/.tmpQ3x9/home/.local/bin/amplihack-hooks is under temporary directory /tmp; profile root /home/user/.claude-profiles is outside HOME)
  Profile work: settings.json not refreshed (hooks binary /tmp/build/amplihack-hooks is under temporary directory /tmp; profile root /home/user/.claude-profiles is not under a temporary directory)
  Profile work: settings.json not refreshed (hooks binary /nonexistent/amplihack-hooks does not exist or is not a regular file)
  ```

  Paths are canonicalised and compared component-wise, so `/tmpfoo` is not
  under `/tmp`. The `~/.claude/settings.json` registration and `statusLine`
  are not affected. The second line appears when no passwd home is known;
  otherwise a temp `HOME` gets the first. If you run with `HOME` set to a
  directory other than your account's home on purpose, keep the profile root
  under that `HOME`, or set `HOME` back before `amplihack install`. If your
  real `HOME` is under `/tmp` and your profile root
  is outside it, move the root under `HOME` (see [Root command](#root-command))
  and run `amplihack install` again; changing `TMPDIR` does not help. If the
  hooks binary is a temporary build, fix the deploy failure that install
  reported, or point `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` at a binary
  outside the temporary directories, and run `amplihack install` again.
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
  `sessions`, `todos`, `statsig`, `shell-snapshots`, `ide`,
  and every other entry not on the allowlists (so plugins and MCP server
  settings are not carried over). `projects`, `file-history` and
  `session-env` are not copied; they are linked after the add (see
  [Shared session directories](#shared-session-directories)).
- Invalid `settings.json` yields `InvalidSharedSettings`. A seeding error
  removes the private staging directory so no profile is published; if cleanup
  also fails, the error reports both failures (never a pre-existing profile).
- Values are never logged; messages contain paths and counts only.
- No sync after creation: later edits to `~/.claude` do not update profiles.
  The three shared session directories are links, not copies, so they are
  always current.

## Shared session directories

Every profile shares one session store with `~/.claude`. Logins stay per
profile. See the [how-to](../howto/claude-profiles.md#share-sessions-across-profiles)
for a walkthrough.

### Shared names

| Constant | Contents |
|----------|----------|
| `SHARED_SESSION_DIRS` | `projects`, `file-history`, `session-env` |

The *canonical root* is `$HOME/.claude` (the same `shared_assets` path
seeding reads). The canonical directory for a name is
`<canonical root>/<name>`. In each profile, `<root>/<profile>/<name>` is an
absolute symbolic link to it. The link target is always the canonicalized
canonical root joined with the constant name; no other input reaches it.

`file-history/` and `session-env/` hold one entry per session id, and
`--resume` reads them, so they are shared with `projects/`. No other
directory is shared. `.credentials.json` and `.claude.json` are top-level
files of the profile directory, outside all three names, so neither rotate
nor auto-switch can reach the shared store when it moves a login.

### Entry point

The module is `profile::shared_dirs`; every item is `pub(super)`.

```rust
pub(super) const SHARED_SESSION_DIRS: [&str; 3] = ["projects", "file-history", "session-env"];

pub(super) enum Deferred { LiveSession, LivenessCheckFailed, LivenessUnknown, Raced }

pub(super) enum Mode { Migrate, LinkOnly(Deferred) }

pub(super) fn ensure_shared_session_dirs(
    profile_dir: &Path,
    profile: &ProfileName,
    canonical_root: &Path,
    mode: impl FnOnce() -> Mode,
) -> Report;

pub(super) fn ensure_for(store: &ProfileStore, name: &ProfileName);
```

- `ensure_shared_session_dirs` never returns an error. Every problem becomes
  a warning in the `Report`.
- `mode` is called at most once, and only when a name is a real directory.
  When every name is already a correct link it is not called, so no `/proc`
  scan happens.
- `ensure_for` picks the mode from liveness (below), then prints the
  report to stderr. It prints nothing when the report is a no-op. It never
  writes to stdout, so `switch` output and every `--json` output are
  unchanged.

Liveness comes from the auto-switch `/proc` scan for a process with
`CLAUDE_CONFIG_DIR=<root>/<name>`. In doubt nothing is migrated:

| Scan result | Mode |
|-------------|------|
| No such process | `Migrate` |
| A matching process, or the scan sees everything as running | `LinkOnly(LiveSession)` |
| The scan failed (Linux) | `LinkOnly(LivenessCheckFailed)`; the error is logged at debug level and the next call retries |
| `Unsupported` (every Unix except Linux) | `LinkOnly(LivenessUnknown)` |

`Deferred::Raced` is never passed in; the helper sets it itself (see the
[state table](#per-name-state-table)).

`Report` has one list per outcome: `linked`, `already_ok`, `migrated`,
`deferred` (with its `Deferred` reason), `conflicts` and `warnings`, plus
`unsupported: bool`. Each name appears in exactly one of `linked`,
`already_ok`, `migrated`, `deferred` or a warning.

### Call sites

| Caller | When | Mode |
|--------|------|------|
| `ProfileStore::add` | After the profile is published. Also covers profiles that rotate's `adopt` creates. The `linked` line is not printed; only warnings, and the not-supported warning. | `Migrate` (a new directory has nothing to migrate) |
| `profile switch <name>` | After the pointer is written. | from liveness |
| `profile login <name>` | Before `claude` is spawned. | from liveness |
| Launch with a profile (`--profile`, inherited `AMPLIHACK_CLAUDE_PROFILE`, default pointer, `profile run`, usage-limit reruns) | In the [pre-spawn check](#pre-spawn-check), after the directory check passes and while holding the launch hold. | from liveness |
| `profile rotate`, rotate cycles | After a swap that ran, for `to` and `from`, once the cycle is recorded and the auto-switch lock is released, so a long merge never stalls a launch. Not after `--dry-run` or a refused swap. | from liveness |

Auto-switch has no call site. Its [in-session swap](#in-session-swap) moves
only the top-level login files, and a [usage-limit rerun](#usage-limit-reruns)
is a launch, so `verify_launch_profile_dir` in the pre-spawn check covers
it. The swap code of rotate (`rotate/swap.rs`) and of auto-switch is
unchanged.

### Preconditions

Checked in this order. The first one that fails ends the call with nothing
changed.

| Condition | Result |
|-----------|--------|
| `<profile>/.no-shared-sessions` exists (any file type; `symlink_metadata`, not followed) | No-op, debug log only. |
| Not a Unix system | No-op with `Report.unsupported` set. `ProfileStore::add` prints the not-supported warning; every other caller logs it at debug level. |
| `$HOME` unset (`shared_assets` is `None`) | No-op, no message. |
| `~/.claude` doesn't exist | No-op, debug log only. `~/.claude` is never created. |
| `~/.claude` is a symlink or not a directory | Warning, no-op. |
| `~/.claude` is not owned by the effective user, or is writable by others, or is group-writable when the group is not your own private group (a group named like your user with no listed members, the usual layout with a umask of `002`) | Warning, no-op. |
| The profile directory is not owned by the effective user, or `mode & 0o077 != 0` (any group or other bit; the same mask as the [pre-spawn check](#pre-spawn-check)) | Warning, no-op. |
| The profile directory is the canonical root (canonicalized) | No-op, debug log only. `~/.claude/projects` is never replaced with a link. |
| The profile directory is inside the canonical root (canonicalized, compared by path components) | Warning, no-op. |

Then each name is checked on its own. If `<canonical root>/<name>` is
missing it is created with mode `0700`. If it is a symlink, not a
directory, not owned by the effective user, or writable by group or others
(the same private-group rule as the root, checked with `fstat` on the opened
directory), that name is refused with a warning and the other names go on.
Modes of existing directories are never changed.

### Per-name state table

| What exists at `<profile>/<name>` | Action |
|-----------------------------------|--------|
| Nothing | Create the link with `symlink()`, even when the profile is live. `EEXIST` means something appeared meanwhile: the name is deferred (`raced`). |
| A symlink whose canonicalized target is the canonical directory | Nothing (`already_ok`). |
| Any other symlink, dangling ones included | Warning; never followed or replaced. |
| A real directory, `Mode::Migrate` | [Migrate](#migration), then swap in the link. |
| A real directory, `Mode::LinkOnly(reason)` | Deferred with that reason. |
| Any other file type | Warning; left alone. |

### Migration

The walk is descriptor-based, like [seeding](#seeding): each directory is
opened with `openat(…, O_NOFOLLOW|O_DIRECTORY)`, file types come from `fstat`
on the opened descriptor, and every move is a `renameat2` relative to the
directory descriptors. A path swapped for a symlink during the walk is never
followed. Depth is capped at 16; there is no entry or size cap.

Skipped, and left in the source (so they end up in the backup):

- names matching `.credentials*` or `.claude.json`, at any depth, compared
  as bytes before anything is opened (so non-UTF-8 names and
  `.credentials.json.bak` are caught), with a warning;
- nested symlinks and special files;
- reserved temporary names (`.amplihack-share-tmp-*`).

For each regular file at relative path `p`:

| In canonical | Action |
|--------------|--------|
| Absent | Set the source's mode to `mode & 0o700` with `fchmod` on the opened file, then move it in with `renameat2(RENAME_NOREPLACE)`. |
| Same bytes (size compared first, then a streamed comparison) | Leave the source copy; it ends up in the backup. |
| Different bytes | Canonical always wins. Move the source to the [conflict name](#migration) with `renameat2(RENAME_NOREPLACE)`. |

Canonical always wins because renaming a canonical file could split a session
that another profile's `claude` has open, and would leave a moment with the
real name missing. Size and mtime are not compared. A user who wants the
profile's copy swaps the two by hand
([how-to](../howto/claude-profiles.md#resume-the-losing-copy-of-a-conflict)).

New directories in canonical are created `0700`. No file in canonical is ever
renamed, overwritten or deleted: every write into canonical uses
`RENAME_NOREPLACE` or `O_EXCL`, apart from the memory-index append below.

**Memory index.** In a directory at relative path `<project>/memory` of
`projects/`, `MEMORY.md` is handled after every other entry. It merges like
any file. If that made a conflict, the source's lines missing from the
canonical `MEMORY.md` (compared after trimming trailing white space; blank
lines and lines starting with `#` are left out; `](<file>)` is rewritten to
`](<saved name>)` for every file in the same directory saved under a
conflict name) are appended to the canonical file with
`O_WRONLY|O_APPEND|O_NOFOLLOW` and `fsync`ed. The conflict copy is saved
before the append. An index over 1 MiB, or one that isn't UTF-8, is not
appended to, and the "entries not added" warning says so. Every link is rewritten at most once. A repeat run appends
nothing, because every line is then present. If the append fails (the shared
index is read-only, or was replaced by a rename while the entries were
written), the merge goes on and a warning names the conflict copy to add the
entries from by hand.

**Other filesystem.** When a rename fails with `EXDEV`, the file is copied:
check free space with `statvfs` (if the file doesn't fit, stop migrating this
name, leave it a real directory, and warn), create
`.amplihack-share-tmp-<pid>-<n>` in the destination directory with `O_EXCL`
and mode `0600`, copy, `fsync`, set the mode to the source's `& 0o700` and its
modification time, `renameat2(RENAME_NOREPLACE)` into place, `fsync` the
directory, and only then unlink the source. If the source's size or
modification time changed during the copy, the temporary file is removed
before the rename and the source is kept: the merge stops with
`copy failed: <path> changed during the copy; kept`, and the next run copies
the file again. The same check runs once more before the unlink; a change
caught there keeps the source too, and the next run saves it under a
conflict name.

**Conflict names.** `<file>.conflict-<profile>`, beside the winner. If that
name already holds identical bytes nothing more is done. If it holds
different bytes the next free name of `.conflict-<profile>-2`, `-3`, … is
used. `<profile>` is always a validated profile name. The suffix breaks the
`.jsonl` extension on purpose, so Claude doesn't list the loser as a session.
Every conflict is reported. Nested symlinks and special files are counted
and reported in one line per name.

### Link swap

After a name's merge:

1. Create `<profile>/.<name>.share-tmp` as a symlink to the canonical
   directory, replacing a stale symlink of that name. If `<name>` is no
   longer a directory (another process swapped it meanwhile), stop with the
   `could not swap in the link` warning.
2. On Linux, exchange it with the real directory using
   `renameat2(RENAME_EXCHANGE)`, then rename the real directory (now at the
   temporary name) to the backup name.
3. Without an atomic exchange (`EINVAL`, `ENOSYS`, or not Linux): rename the
   real directory to the backup name, then rename the temporary link to
   `<name>`. If the second rename fails, the backup is renamed back. If that
   also fails, the warning prints the exact `mv` that restores it; nothing is
   deleted.

The backup is `<profile>/<name>.pre-share-<YYYYMMDDTHHMMSSZ>` (UTC), with
`-2`, `-3`, … if the name exists. Backups are never deleted automatically.
`profile remove` deletes them with the profile directory.

### Repeat runs and recovery

A run on a profile whose three names are correct links changes nothing on
disk. An interrupted run is resumed by the next one:

- files already moved are no longer in the source;
- `.amplihack-share-tmp-*` files are removed, but only when they are regular
  files owned by the effective user and the process whose pid is in the name
  is no longer running, so a merge running at the same time in another
  process keeps its copy (`unlinkat`);
- a stale `.<name>.share-tmp` is replaced, but only when it is a symlink;
- if both a backup and a real `<name>` exist, the real `<name>` is migrated
  again.

Those two reserved names are the only things the helper ever deletes.

### Messages

All output goes to stderr, prefixed `amplihack: profile <name>: `. Paths are
shortened with `~`; only paths, counts and OS error text are printed, never
file contents. This is the complete list; nothing else is printed at the
default log level.

| Message | Meaning |
|---------|---------|
| `merging session history into ~/.claude (one-time; may take a while)` | Printed once per call, before the first merge starts, so a long merge doesn't look hung. Every other line is printed when the call finishes. |
| `linked projects, file-history, session-env` | Links created (not printed by `add`). Lists only the names linked. |
| `projects: moved <n> files into ~/.claude/projects, <n> identical, <n> conflict` | A merge finished. |
| `projects: conflict: <path> kept the shared copy; this profile's copy saved as ~/.claude/projects/<path>.conflict-<profile>` | A conflict. Canonical always wins. |
| `projects: memory index <project>/memory/MEMORY.md: added <n> entries from this profile to the shared copy` | The profile's memory index lost a conflict; its missing entries were appended to the shared index. |
| `projects: memory index <project>/memory/MEMORY.md: entries not added (<error>); add them by hand from <conflict copy>` | The append failed, or either index is over 1 MiB or not UTF-8; the profile's index is safe under its conflict name. |
| `projects: skipped credential file <path>; left in <backup>` | A credential-named file was found in a shared directory. |
| `projects: skipped <n> symbolic links or special files; left in <backup>` | Nested entries that are never moved. |
| `projects: old directory kept as <backup>` | The backup name. |
| `projects: migration deferred: live session` | `Deferred::LiveSession`. The merge runs on a later call. |
| `projects: migration deferred: could not check for running sessions (retried next launch)` | `Deferred::LivenessCheckFailed`: on Linux the `/proc` scan failed, so no session was seen either way. The merge runs on a later call. |
| `projects: not merged: running sessions can't be checked on this system; merge by hand, see docs/howto/claude-profiles.md#merge-by-hand-on-macos` | `Deferred::LivenessUnknown`. Printed on every call while the name is a real directory. |
| `projects: link not created: the name appeared during the check; retried next launch` | `Deferred::Raced` (`EEXIST` from `symlink()`). |
| `projects is a symlink to <target>; left alone` | A foreign or dangling link at the profile's name. |
| `projects is not a directory or a link; left alone` | Any other file type at the profile's name. |
| `~/.claude/projects is a symlink or not a directory; projects not shared` | The canonical name is refused; the other names go on. |
| `~/.claude/projects is not owned by you or is writable by group or others; projects not shared` | The canonical name fails the ownership/permission check; the other names go on. |
| `~/.claude is a symlink or not a directory; sessions not shared` | Canonical root refused. |
| `~/.claude is not owned by you or is writable by group or others; sessions not shared` | Canonical root fails the ownership/permission check (group write is accepted only for your own private group, as for `install`). |
| `profile directory is not owned by you or has group or other permissions; sessions not shared` | Profile directory fails the `0o077` check. |
| `projects: not enough free space to copy <path> (<n> bytes) to ~/.claude/projects; merge stopped, nothing removed` | `EXDEV` copy refused by the `statvfs` check. The name stays a real directory and the next call retries. |
| `projects: could not swap in the link (<OS error>); <profile dir>/projects was not replaced; files already merged are in ~/.claude/projects` | The link swap failed and the rollback worked. The profile's directory is still real, but the files moved before the swap are already in the shared store. The next call merges what is left and retries the swap. |
| `projects: could not restore the old directory (<OS error>); run: mv '<backup>' '<profile dir>/projects'` | The link swap and its rollback both failed. The `mv` is printed with the real absolute paths, shell-quoted. |
| `projects: rename failed: <OS error>; nothing removed, the old directory is at <profile dir>/.projects.share-tmp` | The link is in place but the old directory could not be renamed to its backup name. Printed once; remove the leftover by hand after checking it (see the how-to). While it exists, a later swap of the same name refuses. |
| `projects: <step> failed: <OS error>; nothing removed` | Any other I/O error during the merge. `<step>` is `open`, `read`, `copy`, `rename`, `mkdir`, `chmod` or `link`. The same line is printed when creating, opening or linking the name fails before any merge. The name stays a real directory. |
| `<profile dir> is inside ~/.claude; sessions not shared` | The profile root is under `~/.claude`; move it elsewhere to share sessions. |
| `<path>: <OS error>; sessions not shared` | The opt-out marker, `~/.claude` or the profile directory could not be checked. Nothing changed. |
| `shared session dirs not supported on this platform; profiles keep separate projects/` | Not a Unix system. Printed by `add` only. |

The `projects` in each message is replaced by the name concerned. A
`moved` line is printed only when a merge completes; apart from the
`merging session history` line printed before it starts, there is no
progress output.

### Shared directories platform support

| Platform | Links | Automatic merge |
|----------|-------|-----------------|
| Linux | Yes | Yes, when the profile is not live |
| Other Unix (macOS) | Yes | No: liveness is `Unsupported`, so the mode is always `LinkOnly(LivenessUnknown)`. Each call prints the `not merged` message until the directory is merged by hand ([how-to](../howto/claude-profiles.md#merge-by-hand-on-macos)). |
| Windows and other non-Unix | No | No. `add` prints one warning; other callers log at debug level. |

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
Any failure aborts the launch. Once the check passes, the profile's
[shared session links](#shared-session-directories) are checked and fixed
while the launch hold is held; that step never aborts the launch.
This check applies only to profile launches;
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
  at most one bounded read of `<root>/.auto-switch` (4096 bytes);
- launches do not [start the rotator](#rotate-autostart) and do not read its
  autostart setting.

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
| `auto-switch exclude [names…]` | Replaces the exclusion list. With no names, clears it. Each name must pass the [name rules](#name-rules); it does not have to exist (`status` lists names that do not). Keeps `enabled`, `threshold` and `overflow`. |
| `auto-switch install` | Adds the [status-line wrapper](#status-line-wrapper) to every profile that does not have it. Leaves a stale wrapper alone and reports it. Prints the root it acts on. |
| `auto-switch repair` | If `<root>/.auto-switch.d/` exists, takes the global lock and runs [recovery](#session-end-and-recovery). Then does everything `install` does, and also rewrites stale wrappers (old or moved binary path, wrapped twice, edited by hand) to the canonical one. Prints the install lines, then one `recovery:` line per thing recovery did, or `recovery: nothing to do`. This is the only command whose purpose is recovery. |

On Unix, `status` prints an `autostart:` line right after the `switch:`
line, in the format described in [Rotate status](#rotate-status). When
auto-switch is off it ends with ` — inactive (auto-switch off)`, because
launches start the rotator only while auto-switch is on:

```
switch: off (file)
autostart: on (default) — inactive (auto-switch off)
```

When any profile is marked [overflow](#rotate-overflow), the next line is
`overflow gate:`, the gate auto-switch uses and where it came from (see
[Auto-switch and overflow profiles](#auto-switch-and-overflow-profiles)):

```
overflow gate: wait 7200s, unknown reset waits, plan capacity below 85% session and 90% week (from the rotator, pid 4242)
overflow gate: wait 1800s, unknown reset waits, plan capacity below 85% session and 90% week (no rotator runs; from AMPLIHACK_ROTATE_* or defaults)
overflow gate: closed (the running rotator (pid 4242) has not recorded its overflow settings; restart it)
```

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
| `profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login <b>' with the browser signed out of claude.ai or in a private window.` | Two logins are the [same account](#account-identity): from each profile's own `.claude.json`, from its saved block while its directory hosts another account, or `$HOME/.claude.json` (shown as `~/.claude`). See [Duplicate-login warning](#duplicate-login-warning). No uuid, organization or e-mail is printed. |

Every command takes the global state lock only when it needs it, and exits
with status 1 on error with the usual `error:`/`hint:` lines.

### Settings file

`<root>/.auto-switch` holds `key=value` lines, written in this order:

```
enabled=on
threshold=90
exclude=review,shared
overflow=ryan-mistt-org
```

| Key | Values | Default |
|-----|--------|---------|
| `enabled` | `on` or `off`. Required. | — |
| `threshold` | Integer `1` to `100`, in percent. | `90` |
| `exclude` | Comma-separated profile names, may be empty. | empty |
| `overflow` | Comma-separated profile names, in the order the rotator tries them. See [Rotate overflow](#rotate-overflow). Written only by [`profile overflow`](#overflow-commands). | empty |

`overflow` is written last, and only when the list is not empty, so a file
with no overflow profiles is byte-identical to one written before the key
existed. A missing key or an empty value means no overflow profiles. Every
command that rewrites the file keeps the `overflow` list.

It is written like `.default`: a new temp file in the same directory
(`create_new`, mode `0600`, never through a symlink), synced, renamed over the
file, and the directory synced. Rewrites produce identical bytes for an
identical setting.

Every command that changes the file (`auto-switch on`, `off`, `threshold`
and `exclude`, and `profile overflow add` and `remove`) reads it, changes
its own key and writes it back while holding the auto-switch state lock in
`<root>/.auto-switch.d/`, the lock the rotator and in-session swaps hold
while they work. Two writers therefore never lose each other's change. A
command waits up to 10 seconds for the lock and otherwise fails, leaving the
file unchanged, with `AutoSwitchConfig` and the message:

```
.auto-switch is being changed by another process (the rotator or auto-switch); retry
```

It never falls back to an unlocked write. When the change leaves every key as
it was, for example `overflow add` on a name already marked, nothing is
written. A change that would make the file larger than 4096 bytes is refused
with `AutoSwitchConfig` and the file is left as it was.

The file is *corrupt*, and the switch counts as off, if it is larger than
4096 bytes, not UTF-8, not a regular file, not owned by the current user,
writable by group or others, has an unknown or repeated key, has no `enabled`, has an out-of-range
value, or has an `overflow` list with an invalid or repeated name.
`status` shows `switch: off (corrupt)`. A corrupt file also gives no overflow
profiles, so the rotator never moves onto credits because of it.

An `amplihack` binary older than the `overflow` key counts a file that has
the key as corrupt: auto-switch counts as off and the exclusion list is
lost for that binary. Upgrade every `amplihack` binary that uses the root
before running `profile overflow add`.

What each command writes depends on the current file:

| Current `.auto-switch` | `on` / `off` | `threshold N` / `exclude …` |
|------------------------|--------------|-----------------------------|
| valid | rewrites it with the new `enabled` | rewrites it, keeping the other keys |
| missing | creates it | creates it with `enabled=off`, defaults for the key not given |
| corrupt regular file owned by you | replaces it; `threshold` and `exclude` start from their defaults unless given | refused with `AutoSwitchConfig`; run `on` or `off` first |
| owned by another user | refused with `AutoSwitchConfig` | refused with `AutoSwitchConfig` |
| owned by you, writable by group or others | replaced with mode `0600`, as for a corrupt file | refused with `AutoSwitchConfig`; run `on` or `off` first |
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
reading is not exhausted. Exhausted plan accounts are never chosen; an
[overflow](#auto-switch-and-overflow-profiles) profile's own reading does
not count.

An account is *eligible* when it is not excluded, not the current account,
not [leased](#leases), and not exhausted (overflow profiles: not
spend-blocked instead), plus the extra conditions for an
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

#### Auto-switch and overflow profiles

[Overflow](#rotate-overflow) profiles are chosen only after every plan
profile, by the rule the rotator follows, with the rotator's own gate. The
gate has four values: the overflow wait, the unknown-reset choice, and the
two thresholds that define *plan capacity*. A plan account has capacity
when its five-hour use is below the rotator's `--session-at` and its
seven-day use is below its `--week-at` (85% and 90% by default), the test
the rotator applies to its [plan pool](#plan-pool). Every selection
(in-session swaps, nested launches and
[usage-limit reruns](#usage-limit-reruns)) works like this:

1. The best plan profile, by the order above, with the overflow profiles
   left out, among those with plan capacity or no reading. From a plan
   profile, the target must also be below the auto-switch threshold, as for
   any move between plan profiles. From an overflow profile, plan capacity
   alone decides, whatever the auto-switch threshold: a session moves back
   as soon as a plan profile has plan capacity and is eligible, whether or
   not the overflow profile is near its own limit. This is the rotator's
   rule for leaving overflow. For example, with `auto-switch threshold 80`
   and the default 85%/90%, a plan account whose session has reset and
   whose week reads 85% brings the session back: 85% is over 80 but under
   the 90% week threshold. The status-line check that decides whether a
   session on an overflow profile is worth a full check uses the same rule,
   plus step 2's spend block: without either, it stops there, whatever the
   overflow profile's own reading, so a session on credits does not run
   recovery or the `/proc` scan on every update.
2. A session on an overflow profile that is not
   [spend-blocked](#overflow-spend-limit-block) stays where it is. Its own
   plan reading plays no part: a credit account reads at or near 100% of
   its plan limits while it spends credits, and that is the account
   overflow exists for. Only a spend block moves a session from one
   overflow profile to the next, as in the rotator.
3. An overflow profile, only when **both** hold:
   - no plan profile has plan capacity, the current one included, whether
     or not this session could use it (one leased to another session still
     counts as capacity, so no money is spent while it exists). A current
     plan profile at the auto-switch threshold but below the gate's
     thresholds (for example at 82% with `auto-switch threshold 80`) still
     has plan capacity: the rotator would not leave it for overflow, so the
     session stays on it rather than spend, and it cannot move onto
     overflow only to move straight back;
   - the earliest plan reset is more than the overflow wait away. The plan
     reset of an account is the reset of its window at its threshold, or
     the later of the two when both are; the earliest plan reset is the
     earliest of those over every plan account, the current one included.

   The first overflow profile by tier, then by list order within a tier,
   that is not current, not excluded, not spend-blocked and eligible is
   chosen. Its own plan reading is not checked, so a tier 1 credit account
   at 100% of its plan limits is still chosen before a tier 2 API key at
   0%.
4. Otherwise the best plan profile below the auto-switch threshold, by the
   order above. A plan account between the gate's thresholds and the
   auto-switch threshold (for example at 87% session) is used only here:
   when overflow is not allowed, it beats waiting, and it costs nothing.

The rotator decides in the same order (plan capacity, then overflow, then a
free swap onto a plan login without capacity), and both use the gate's plan
capacity test for moving onto and off overflow, so on the same readings the
two agree on when money is spent, for any auto-switch threshold. The
auto-switch threshold only decides when a session on a plan profile asks,
and which plan profiles it may move to from another plan profile. In particular, a plan account at 87%
session counts as capacity for neither: both move onto an overflow profile
when the plan reset is beyond the wait, and both use the 87% account when it
is not.

Auto-switch takes the gate from the running rotator. `rotate watch` writes
its pid and the gate it resolved from its flags to
`<root>/.auto-switch.d/rotate/gate.json` when it starts, so
`profile rotate start --overflow-wait 7200` gives every Claude session the
same two-hour wait with nothing to keep in step. Where the gate comes from:

| Rotator | Gate auto-switch uses |
|---------|-----------------------|
| Runs, and `gate.json` names its pid | The recorded gate. `AMPLIHACK_ROTATE_*` in the session's environment is ignored. |
| Runs, and `gate.json` is missing, damaged, out of range or names another pid (for example a rotator from an amplihack without this record, which never uses overflow) | None: no overflow profile is chosen. Restart the rotator. |
| Cannot be verified (no `/proc`) | None. |
| Not running | `AMPLIHACK_ROTATE_OVERFLOW_WAIT`, `AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET`, `AMPLIHACK_ROTATE_SESSION_AT` and `AMPLIHACK_ROTATE_WEEK_AT` from auto-switch's own environment, with the rotator's defaults (1800 seconds, off, 85, 90). An invalid value of one of these four means no gate; other `AMPLIHACK_ROTATE_*` variables play no part. |

`auto-switch status` prints the gate and its source on its
`overflow gate:` line. A rotate ledger that cannot be read (so spend blocks
are unknown) also means no overflow profile is chosen.

The status-line check that runs a swap normally does nothing while the
session is under the threshold. When the account in the session's directory
is an overflow profile and some plan profile reads under the threshold, it
goes on to the selection above, so the session moves back. The note then
reads `profile "<overflow>" is an overflow profile and a plan profile has
capacity again; ...`. A nested launch that inherits an overflow profile is
started under a plan profile with capacity in the same way.

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

While the [capacity notice](#capacity-notice) is due, step 3 adds one line
after the original output, and step 2 also updates the
[capacity file](#capacity-file) after recording the reading.

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

The wrapper writes to standard output itself only to add the
[capacity notice](#capacity-notice) after the original output. Its exit code is the
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

The swap does not check the [shared session links](#shared-session-directories).
It moves only the top-level login files, which are outside the shared
names. Both profiles are checked the next time they are launched, including
the usage-limit rerun, by the [pre-spawn check](#pre-spawn-check).

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
- the first attempt inherits stdin unchanged. A rerun gets a regular file
  again from where the step started reading it, a terminal unchanged, and
  anything else (a pipe, a socket) `/dev/null`, because what the first
  attempt read from it is gone. stdin is never read or copied by the
  supervisor, so a pipe nobody closes cannot hold it up (see
  [Gate](#gate));
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
has been printed. If the supervisor is killed first, recovery deletes it
once no process with that `<pid>` is running, and any buffer older than 24
hours regardless. Files whose names do not match `<pid>-<n>.out` are never
deleted, whatever their age. Recovery runs at every supervisor start as
well as at the points listed in
[Session end and recovery](#session-end-and-recovery). Until then each
buffer holds whatever the step printed, at mode `0600` in a `0700`
directory.

A rerun starts the step again from the beginning, so anything the attempt
that hit the limit had already done (a commit, a push, a comment) may be done
again.

### Capacity notice

When auto-switch is on and every eligible account is at or above the
threshold, amplihack says so in the status line, in Claude Code (through the
hooks), and when `amplihack claude` or `amplihack recipe run` starts. The
notice only informs: it makes no swap, refuses nothing and changes no exit
status. With the switch off, none of this runs and behaviour is unchanged.

#### Capacity file

The state lives in `<root>/.auto-switch.d/capacity.json`:

```json
{
  "version": 1,
  "state": "low",
  "threshold": 90,
  "over": ["personal", "work"],
  "earliest_reset": "2026-10-22T09:00:00Z",
  "written_at": "2026-10-20T14:03:11Z",
  "writer": "statusline"
}
```

| Field | Type | Meaning |
|-------|------|---------|
| `version` | integer | Always `1`. |
| `state` | `"ok"`, `"low"` or `"exhausted"` | See [Capacity states](#capacity-states). |
| `threshold` | integer `1`–`100` | The auto-switch threshold the state was computed with. |
| `over` | array of profile names | The eligible accounts at or above the threshold, sorted, no duplicates. Excluded and overflow profiles are not eligible. |
| `earliest_reset` | RFC 3339 UTC string or `null` | When the first of the `over` accounts gets capacity back. `null` when no reset time is known. |
| `written_at` | RFC 3339 UTC string | When the file was written. |
| `writer` | `"statusline"`, `"usage"` or `"rotate"` | Which command wrote it. |

The file holds profile names, the state, the threshold and times only. It
never holds account IDs, e-mail addresses, tokens or credential paths, and
no reader ever shows `over` or `writer` to you or to the model.

#### Capacity states

The *eligible* accounts are the profiles that `profile list` shows, minus the
[exclusion list](#settings-file). Listing reads the root directory; it runs
no probe.

- An account is *over* when it has a reading and the higher of its two
  adjusted windows is at or above the threshold. This is the same test that
  makes an account exhausted in [Candidate selection](#candidate-selection).
- An account is *at its limit* when the same test passes at 100%, when it has
  a `usage_limit_failure` [reading](#readings) whose window has not reset
  yet, or, for the rotator only, when its probe reported `session_exhausted`
  or `weekly_exhausted`.
- An account with no reading, or whose probe failed, is not over. Missing
  data never raises a notice.

| State | When |
|-------|------|
| `exhausted` | Every eligible account is at its limit. |
| `low` | Every eligible account is over, and at least one is not at its limit. |
| `ok` | Anything else, including no eligible profiles. |

With a single eligible profile, the state is `low` as soon as that account
crosses the threshold.

With [overflow](#rotate-overflow) profiles, a state that would be `low` or
`exhausted` is `ok` when an in-session swap would move onto an overflow
profile now ([Auto-switch and overflow profiles](#auto-switch-and-overflow-profiles)),
judged by the same functions the selection uses:

- the gate allows it: no eligible account has plan capacity (below
  `--session-at` and `--week-at`, not the auto-switch threshold), and the
  earliest plan reset is more than the overflow wait away. The plan reset
  here is the gate's, the reset of the window at the gate's threshold, or the
  later of the two when both are, not the reset of the window over the
  auto-switch threshold;
- some overflow profile is listed, not excluded, not spend-blocked and not
  over the threshold.

For example, an account at 87% session (resetting in two hours) and 95% week
(resetting in ten minutes) has plan capacity again only in two hours, so with
the default 30-minute wait the session moves onto credits and the state is
`ok`, although the week window alone would reset inside the wait.

An account's reset time is the time it gets capacity back: the later of the
two `resets_at` values when both windows are over, otherwise the `resets_at`
of the window that is over. Only reset times in the future count.
`earliest_reset` is the earliest known reset time among the `over` accounts,
or `null` when none is known.

#### When the notice clears

Readers do not trust the stored state forever. The state they act on is:

1. `ok` when the stored state is `ok`;
2. `ok` once `earliest_reset` has passed. The first account to reset means
   not every account is over any more, so the notice stops without anyone
   rewriting the file;
3. when `earliest_reset` is `null`, the stored state until 5 hours after
   `written_at`, then `ok`.

The 5-hour bound is the only time-based expiry, and it applies only when the
reset time is unknown, so a notice can never stay up indefinitely. 5 hours is
the length of the session window. A writer that keeps seeing the same state
rewrites the file at least hourly in that case, so the bound does not clear a
state that is still true.

#### Capacity writers

Only the three commands that already evaluate capacity write the file. None
of them makes an extra network call or probe for it.

| `writer` | Written | Readings used |
|----------|---------|---------------|
| `statusline` | By the [status-line wrapper](#status-line-wrapper), each time it records a reading, under the state lock it takes for that. | `usage.json` as just recorded. |
| `usage` | By [`profile usage`](#refreshing-auto-switch-readings), each time it records readings, under the state lock it takes for that. | `usage.json` as just recorded. |
| `rotate` | By the rotator, in a [cycle](#rotate-cycle) that reaches the swap, stay or ALERT, under the lock taken in step 6. | That cycle's probes. A `session_exhausted` or `weekly_exhausted` probe counts as at its limit; a failed probe, including one that timed out, counts as no reading and never as at its limit. Reset times come from the profile's `usage.json` entry while they are in the future, and are otherwise unknown. |

All three use the auto-switch threshold and exclusion list, not the
rotator's `--session-at` and `--week-at`, so they agree on the state. None
of them counts an [overflow](#rotate-overflow) profile as eligible: its
capacity is credit capacity, used only by the rotator's overflow rule. While
the rotator's last cycle left the live login on credits, its own write sets
the state to `ok`. The
rotator's own `ALERT` line still follows its own thresholds. All three
write while holding the global state lock, so writes never interleave and
the last one wins. A writer that cannot get the lock records nothing and
writes no capacity file. The three take reset times from the same
`usage.json` entries, so a later write differs from an earlier one only by
the readings taken in between.

[Usage-limit reruns](#usage-limit-reruns) do not write the file. A step that
runs out of profiles shows no notice until one of the three writers next
runs.

When the rotator's cycle ends on an [overflow](#rotate-overflow) profile
(`last_cycle.overflow` is `in` or `on`), the `rotate` writer records `ok`
whatever the readings say, because work can go on at the cost of credits.
Like any `ok`, this replaces an existing file but never creates one. The
`statusline` and `usage` writers apply only the overflow rule in
[Capacity states](#capacity-states); they do not know whether the rotator's
live login is on credits, and may write an exhausted state in between. The
rotator's next cycle replaces it.

Write rules:

- The file is written only while the [effective setting](#toggle-and-precedence)
  is on. With the switch off it is neither written nor read, and a file left
  from before is ignored. This gate applies to the rotator too: with no
  `<root>/.auto-switch` and no `AMPLIHACK_CLAUDE_AUTO_SWITCH`, the rotator
  still rotates and writes its `ALERT`, but no notice appears anywhere. Turn
  auto-switch on to get the notice.
- Writes go through a private temp file, are synced and renamed into place.
  The file is `0600` in the `0700` state directory, and is never written
  through a symlink.
- The write is skipped when `state`, `over`, `threshold` and
  `earliest_reset` are all unchanged, so the status line does not sync a file
  every turn. The exception: when `earliest_reset` is `null` and `written_at`
  is more than 1 hour old, the file is rewritten to keep the 5-hour bound
  from expiring a state that still holds.
- An `ok` result never creates the file. It only replaces an existing one.
- A failed write is logged at debug level, naming only the operation and the
  error kind. It never changes the writer's output or exit status.

#### Reading the capacity file

The hooks, the status-line wrapper, launches and `recipe run` read the file
the same way. They take no lock, start no process and use no network.

1. **Root.** Chosen in the same order as the [profile root](#profile-root):
   `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`, then
   `~/.amplihack/claude-profiles`, with the same checks on each value. A value
   that fails them means no notice; the reader never falls back to the
   default root.
2. **Gate.** The notice is shown only when the
   [effective setting](#toggle-and-precedence) is on:
   `AMPLIHACK_CLAUDE_AUTO_SWITCH=1` or `0` wins, any other value is ignored,
   and otherwise `<root>/.auto-switch` must be a valid
   [settings file](#settings-file) with `enabled=on`. Nothing is printed
   about a bad variable on this path.
3. **File.** It is opened without following symlinks and without blocking,
   so a FIFO cannot hang a hook. It must be a regular file owned by you, at
   most 16 KiB, valid JSON with `"version": 1`, a `threshold` from `1` to
   `100`, and a `written_at` no more than 5 minutes in the future. Unknown
   fields are accepted. An `earliest_reset` more than 8 days after
   `written_at` is treated as unknown.

If any step fails, the reader acts as if the state were `ok`: no notice, no
error, and the caller's output is unchanged.

#### Notice text

The line has two forms, built only from fixed text, the state, the threshold
and the reset time:

```
⚠ all Claude accounts ≥90% · next reset Oct 22 2am
⚠ all Claude accounts at limit · next reset Oct 22 2:30am
```

The first is `low` and shows the configured threshold; the second is
`exhausted`. The reset time is in local time, with minutes only when they
are not zero. When the reset time is unknown, the end reads
`next reset unknown`.

| Where | When | What is shown |
|-------|------|---------------|
| Status line | Every render while the state is `low` or `exhausted`. | The line, below the original status line. |
| Claude Code, through the `SessionStart` and `UserPromptSubmit` hooks | Once per state per session, and again when the state gets worse. | The line plus `— run 'amplihack profile usage'` as a user-visible message, and a note to the model. See [Hook specifications](hook-specifications.md#capacity-notice-in-hooks). |
| `amplihack claude` and `amplihack launch` | At start, on stderr. | The line plus ` — run 'amplihack profile usage'`. |
| `amplihack recipe run` | At start, on stderr, before the recipe runs. | `amplihack recipe run: ` + the line + ` — run 'amplihack profile usage' (continuing anyway)`. |

Examples:

```
⚠ all Claude accounts ≥90% · next reset Oct 22 2am — run 'amplihack profile usage'
amplihack recipe run: ⚠ all Claude accounts at limit · next reset Oct 22 2am — run 'amplihack profile usage' (continuing anyway)
```

`amplihack claude` and `launch` print nothing when the arguments contain
`-p` or `--print`, or when they run inside a recipe (`AMPLIHACK_SESSION_DEPTH`
is a number above 0), so recipe agent steps do not repeat the warning.
`recipe run` prints nothing when it is itself nested. Neither ever prompts,
refuses to run or changes the exit status.

**Status line.** When the state is `low` or `exhausted`, the wrapper passes
the original command's standard output through unchanged, with no size cap,
then writes the line on its own line: a newline first only if the original
output was not empty and did not end with one, then the line, then a
newline. The original command's standard error and exit status, including
death by a signal, pass through unchanged. When the switch is off or the
state is `ok`, the wrapper runs exactly as it did before this notice
existed and its output is byte-identical.

```
work · opus 4 · 5h 93%
⚠ all Claude accounts ≥90% · next reset Oct 22 2am
```

#### Notice records

The hooks remember the last state they showed for each session in
`<root>/.auto-switch.d/notice/<session id>`. The file name is the session
ID with unsafe characters replaced, cut to 128 bytes. The file holds one
word, `low` or `exhausted`, and is read up to 64 bytes; anything else counts
as no record.

| Current state | Last shown | Result |
|---------------|------------|--------|
| `low` or `exhausted` | none | Show, record the state. |
| `low` | `low` or `exhausted` | Nothing. |
| `exhausted` | `low` | Show again, record `exhausted`. |
| `exhausted` | `exhausted` | Nothing. |
| `ok` | anything | Nothing, and delete the record, so a later drop shows again. |

A hook call without a session ID shows the notice every time the state is
`low` or `exhausted`, because there is nothing to key a record on.

Records are written only when `.auto-switch.d/` and `notice/` are real
directories owned by you with no group or other permissions (`notice/` is
created `0700`). Each record is a `0600` temp file renamed into place. If the
directories fail the check, nothing is written and the notice is still
shown. When the state goes back to ok, the session's record is removed only if
both directories already pass that check; otherwise it is left alone. Each
write also deletes records older than 7 days, looking at no more
than 256 entries, never following a symlink and removing only regular files
you own. Two hooks in the same session racing on a record can at worst show
the notice twice.

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
| `capacity.json` | Whether any account has capacity left; see [Capacity file](#capacity-file). Unlike the other state files, it accepts unknown fields and is capped at 16 KiB. |
| `notice/<session>` | The last capacity state shown in a Claude session; see [Notice records](#notice-records). Unlike the other state files, it is a bare word, not JSON, and is read up to 64 bytes. |

Apart from `capacity.json` and `notice/<session>`, every state file has
`"version": 1`, rejects unknown fields and is capped at 64 KiB. Each name in them is checked with the
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

`profile rotate` keeps the login in `~/.claude` on the account with the most
capacity left by moving logins between `~/.claude` and the profiles. The
[how-to](../howto/claude-profile-rotation.md) walks through using it; this
section defines its behaviour. It is separate from [auto-switch](#auto-switch),
which swaps logins inside profiled sessions, but shares its toggle, its
exclusion list and its state lock.

### Rotate commands

```
amplihack profile rotate [--dry-run] [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1]
amplihack profile rotate start  [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1] [--interval SECS]
amplihack profile rotate watch  [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1] [--interval SECS]
amplihack profile rotate stop
amplihack profile rotate status
amplihack profile rotate autostart [on|off]
```

| Command | Behaviour |
|---------|-----------|
| `rotate` | Runs one [cycle](#rotate-cycle): recover, measure, decide, and swap if needed. Prints its log lines to stdout and appends them to the log. Runs whether the toggle is on or off. |
| `rotate --dry-run` | Measures and decides, then prints `would ...` lines and the `cycle` line to stdout. Takes no lock, writes no file, moves nothing, and skips both the [prototype check](#rotate-prototype-coexistence) and [recovery](#rotate-recovery). Reports a pending journal as `would recover an interrupted swap (phase <phase>)` and decides as if it were not there. Reads the [pin](#rotate-pin): a pin in force prints `would stay (held by switch to <name>)`, or `would stay (pin unreadable)`, and measures nothing. Reports a [hand change](#rotate-hand-change-recovery) as `would recover: <recovered line>`, or as `would alert: <ALERT line>` with its `fix` lines; either way it then stops without measuring, and the alert case ends with outcome `alerted`. An inconsistent journal is reported as `would alert: rotate recovery is inconsistent`, and a damaged one as `would alert: the rotate journal is damaged`. |
| `rotate start` | Checks for the prototype, runs recovery under the lock, warns on stderr if auto-switch has been turned off, refuses if a verified rotator is already running, then starts `rotate watch` as a detached process and writes the pid file. The check, the start and the pid file all happen under the state lock, so two `start`s cannot both start one. Fails with `rotate: lock busy` when the lock stays held for 2 seconds, and with the auto-switch error when auto-switch recovery is inconsistent. Pins the child to its root and records that root; see [Rotate root record](#rotate-root-record). Prints `rotate: started (pid <pid>), checking every <interval>s`, `log: <path>` and `root: <path>`, and returns at once. |
| `rotate watch` | Runs a cycle every `--interval` seconds in the foreground until `SIGTERM` or `SIGINT`. A signal is never acted on during a swap; see [Signals](#rotate-signals). Refuses if another verified rotator is running. Writes only to the log. |
| `rotate stop` | Finds the rotator as described in [Rotate root record](#rotate-root-record). Sends `SIGTERM` to a [verified](#rotate-daemon) rotator, waits up to 10 seconds for it to exit, then removes the pid file and prints `rotate: stopped (pid <pid>)`. |
| `rotate status` | **Read-only.** Prints the [status lines](#rotate-status) and exits 0. |
| `rotate autostart` | **Read-only.** Prints the effective [autostart](#rotate-autostart) state, for example `autostart: on (default)`. |
| `rotate autostart on\|off` | Writes the autostart setting and prints `autostart: on (setting)` or `autostart: off (setting)`. Does not start or stop a rotator. Any other value is a usage error (status 2). |

`--dry-run` belongs to the one-shot form only. Tuning flags go after `start`
or `watch`; `amplihack profile rotate --margin 3 start` is a usage error and
exits with status 2.

On platforms without Unix file semantics, such as Windows, every form fails
with `RotateUnsupported` before doing anything else. That is the only use of
`RotateUnsupported`. On a Unix system or filesystem that cannot exchange two
files atomically, the swap is refused with `RotateRefused`; see
[Rotate swap](#rotate-swap).

### Overflow commands

```
amplihack profile overflow add <name> [--tier N]
amplihack profile overflow remove <name>
amplihack profile overflow list
amplihack profile overflow clear <name>
```

These act on the resolved [profile root](#profile-root), the same root the
rotator uses. The marks are kept in the `overflow` key of
[`<root>/.auto-switch`](#settings-file); spend blocks are kept in the
[rotate ledger](#rotate-state-directory). What the rotator does with them is
in [Rotate overflow](#rotate-overflow).

The marks live in `.auto-switch`, not in the layered `config.toml`, because
the rotator and auto-switch already re-read that file on every cycle and
every check, and auto-switch takes its exclusions from it. Keeping the marks
there gives one source of truth for which profiles are skipped and when.

| Command | Behaviour |
|---------|-----------|
| `overflow add <name> [--tier N]` | Appends `<name>` to the `overflow` list in tier `N` (`1` to `9`, default `1`). The name must pass the [name rules](#name-rules) and be an existing profile directory under the root (not a symlink); otherwise the command fails, names the profile and changes nothing. Prints `overflow: added <name> (tier N)` and the cost note below. For a profile already marked, `--tier` moves it (`overflow: <name> moved from tier A to tier B`), and without `--tier` the tier is kept (`overflow: <name> is already marked (tier N)`); both exit 0. Prints a warning when `<name>` is in the `exclude` list, and another when the [default pointer](#clearing-the-default) names it. |
| `overflow remove <name>` | Removes `<name>` from the list. The name must pass the name rules; it does not have to exist. Prints `overflow: removed <name>`, or `overflow: <name> is not marked` and exits 0. A spend block for the profile is left in the ledger and expires on its own. |
| `overflow list` | **Read-only.** Prints one line per marked profile, in the order the rotator and auto-switch try them (by tier, then list order within a tier): the name, `tier N`, then `missing` when the root has no profile directory of that name, then `blocked until <time> (spend_limit)` when it has an active block. Prints `overflow: none` when the list is empty. Takes no lock. A rotate ledger that cannot be read prints a warning on stderr and the marks without blocks; the command still exits 0. |
| `overflow clear <name>` | Removes the spend block for `<name>` from the rotate ledger. Takes the rotate state lock, retrying for up to 10 seconds; if it is still held, fails with `RotateDaemon` and the message `rotator busy, retry`. Prints `overflow: cleared the spend block on <name>`, or `overflow: <name> has no spend block` and exits 0. The name must pass the name rules; it does not have to be marked or exist, so a block left after `overflow remove` can still be cleared. Never creates the ledger. |

What `add` prints for a new mark, with the cost note:

```
overflow: added ryan-mistt-org (tier 1)
note: ryan-mistt-org is used only when every plan profile is exhausted and the earliest plan reset is more than --overflow-wait away; work on it is billed to that account.
note: set a spend limit for the organization at https://claude.ai/settings/usage; the rotator is not a spending cap.
```

The warnings `add` prints, on stderr:

```
warning: ryan-mistt-org is in the auto-switch exclude list; excluded profiles are never used, even as overflow
warning: the default profile is ryan-mistt-org; launches use it directly and spend credits whatever the rotator does (run: amplihack profile switch --none)
```

`overflow list` with one blocked profile:

```
ryan-mistt-org  tier 1  blocked until 2026-11-01T00:00:00Z (spend_limit)
team-credits  tier 1
old-credits  tier 1  missing
api-key  tier 2
```

#### Overflow tiers

Each overflow mark has a tier from 1 to 9. Tier 1 is tried before tier 2,
and so on; within a tier, the order the profiles were added decides. Tiers
rank kinds of overflow by cost, for example usage credits (tier 1) before an
API key billed per token (tier 2), so the order of names in the file never
has to carry that meaning. In `.auto-switch` an entry is written
`name:tier`, and a tier-1 entry as the bare name, so a file written before
tiers existed reads the same:

```
overflow=ryan-mistt-org,team-credits,api-key:2
```

The rotator and auto-switch move onto an overflow profile only from a plan
profile, or from an overflow profile that is spend-blocked or exhausted. They
do not move from a tier-2 profile to a tier-1 profile that becomes usable
again; they move back to a plan profile as soon as one has capacity.

`add` and `remove` change only the `overflow` key of `<root>/.auto-switch`,
through the same locked write as the [auto-switch commands](#auto-switch-commands)
(see [Settings file](#settings-file)). A missing file is created with
`enabled=off`. A corrupt file is refused with `AutoSwitchConfig` and left
unchanged. Because the write holds the lock, a `remove` that reports success
is never lost to a concurrent `auto-switch exclude`, `on` or rotator cycle.
If the lock is busy for 10 seconds, the command fails with the
`.auto-switch is being changed by another process` message and changes
nothing; run it again. Messages and warnings are printed after the lock is
released.

All four commands exit 0 on success and 1 with the usual `error:`/`hint:`
lines on failure. Output holds profile names, times and fixed text only.

### Rotate settings

| Flag | Variable | Default | Range |
|------|----------|---------|-------|
| `--session-at N` | `AMPLIHACK_ROTATE_SESSION_AT` | `85` | `1`-`100` (%) |
| `--week-at N` | `AMPLIHACK_ROTATE_WEEK_AT` | `90` | `1`-`100` (%) |
| `--margin N` | `AMPLIHACK_ROTATE_MARGIN` | `5` | `0`-`100` (points) |
| `--upgrade-margin N` | `AMPLIHACK_ROTATE_UPGRADE_MARGIN` | `30` | `0`-`100` (points) |
| `--hold SECS` | `AMPLIHACK_ROTATE_HOLD` | `3600` | `0` or more (seconds); `0` disables the hold |
| `--overflow-wait SECS` | `AMPLIHACK_ROTATE_OVERFLOW_WAIT` | `1800` | `0`-`604800` (seconds); see [Rotate overflow](#rotate-overflow) |
| `--overflow-unknown-reset 0\|1` | `AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET` | `0` | `1` lets a plan reset that no reading reports count as further away than `--overflow-wait`, so it can move work onto an overflow profile and **spend money**; see [Overflow decision](#overflow-decision) |
| `--interval SECS` | `AMPLIHACK_ROTATE_INTERVAL` | `600` | `1`-`86400` (seconds); `start` and `watch` only |

A flag beats its variable, and the variable beats the default. An empty
variable counts as unset. Values are unsigned decimal integers: a sign,
whitespace, a fraction or an overflow is an error. The error names the flag or
variable and shows the value escaped and cut to 32 characters:

```
error: invalid rotate setting --session-at "eighty": expected a whole number from 1 to 100
hint: run `amplihack profile rotate --help` for the accepted values
```

`start` resolves every setting and passes them to the background process as
explicit flags, so the variables are read once, when `start` runs. A rotator
started by [autostart](#rotate-autostart) takes them from the environment of
the launch that started it. The
background process sets no `AMPLIHACK_ROTATE*` variable in the environment of
the `claude` processes it starts.

The probe method, deadline and parallelism are not rotate settings: they
have no flags and come from the `AMPLIHACK_PROFILE_USAGE_*` variables in the
rotator's environment. See
[Probe settings in the rotator](#probe-settings-in-the-rotator).

The thresholds and margins are independent of the auto-switch `threshold`.
The auto-switch `exclude` list applies to rotation, and so does the
`overflow` list in the same file; see [Rotate overflow](#rotate-overflow).
`--overflow-wait` has no effect while no profile is marked as overflow.

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
   [rotate recovery](#rotate-recovery), then
   [hand-change recovery](#rotate-hand-change-recovery), then
   [identify](#rotate-live-profile) the live profile and the
   [candidates](#rotate-candidates). If hand-change recovery could not act, it
   writes its guidance and the cycle ends as `alerted`; if it did act, the
   cycle carries on from the new state. An ALERT about the live login
   (`no-login`, `hand-change`, `home-unreadable`, `ambiguous`) is written here and ends the
   cycle. Then check the [rotation pin](#rotate-pin): if it holds rotation,
   log the `skip: held by switch ...` or `skip: pin unreadable ...` line,
   clear the stored ALERT key, record the cycle as `held` (unless the last
   cycle is the `switched` record of a [`profile switch`](#rotate-switch),
   which is kept) and end the cycle without measuring. Release the lock.
4. **Measure.** Probe the live login and every candidate, as
   [`profile usage`](#how-a-profile-is-probed) does, with the same
   [method, deadline and parallelism bound](#probe-settings-in-the-rotator).
   The live probe counts against the bound, so a cycle never runs more
   probes at once than the bound: with a bound of 2 or more the live probe
   runs beside the candidate probes, and with a bound of 1 it runs first and
   the candidates follow one at a time. The live login is probed
   with `CLAUDE_CONFIG_DIR` removed from the environment, so `claude` uses
   `~/.claude` and `~/.claude.json`. Candidates are probed with
   `CLAUDE_CONFIG_DIR=<root>/<name>`. A live login or candidate marked as
   [overflow](#rotate-overflow) is probed with `/usage` only, as
   [`profile usage`](#probe-method) does. Claude's output is parsed and dropped;
   only percentages, reset times and states are kept, and `--raw` output is
   never collected. No lock is held.
5. **Decide.** See [Decision rule](#rotate-decision-rule).
6. **Revalidate and swap.** `watch` first checks for a pending signal and,
   if there is one, ends the cycle and exits without taking the lock (see
   [Signals](#rotate-signals)). Take the lock again, identify the live
   profile and the candidates afresh, and compare them with step 3 as
   described in [Rotate revalidation](#rotate-revalidation). A change that
   affects the decision ends the cycle with
   `skip: state changed during measurement (<labels>)`. It records
   `last_cycle` as `skipped` only when the ledger's `last_cycle` is older
   than this cycle's measurement. A record written while the cycle
   measured, such as the `switched` record of a
   [`profile switch`](#rotate-switch), is newer and stays. A `switched`
   record always stays. Otherwise run the [swap](#rotate-swap), still under
   the same lock. After a swap that ran, check the
   [shared session links](#shared-session-directories) of `to` and `from`.
7. **Record.** Write the log lines and the ledger's `last_cycle`, with
   `overflow` set when the cycle moved onto, off or stayed on an
   [overflow](#rotate-overflow) profile. Add any new
   [spend blocks](#overflow-spend-limit-block) and drop expired ones. When the
   cycle stays or raises the no-capacity ALERT and the live profile is known,
   also record it as `live`, with its `account`.

The overflow list is read in step 3 with the exclusion list. A change to it
between steps 3 and 6 counts as a state change, so the cycle ends with
`skip: state changed during measurement (overflow)`. When overflow profiles
are marked, a rotate ledger that cannot be read ends the cycle before the decision with
the `RotateInconsistent` ALERT; the rotator never moves onto credits without
knowing the spend blocks. A missing ledger means no blocks.

Between steps 6 and 7, after the swap, stay or ALERT and still under the
lock taken in step 6, the cycle updates the
[capacity file](#capacity-file) from that cycle's readings. This writes
nothing under `rotate/` and does not change the decision. A cycle that ends
before then does not update it: the lock busy in step 3 or 6, `skip: state
changed during measurement`, a pending signal or stop, an inconsistent
recovery, an ALERT about the live login, or a refused swap. The update is
also skipped when the [effective setting](#toggle-and-precedence) is off,
which includes having no setting at all, even though rotation still runs
then. A swap made after a [reconsideration](#rotate-revalidation) updates
the file in the same way as any other swap.

#### Rotate revalidation

Step 4 measures without the lock and can take a minute or more, so other
processes may change profiles while it runs. Step 6 compares the state it
reads under the lock (the *fresh* state) with the state read in step 3, and
looks only at what bears on the decision from step 5. Changes to profiles
the decision does not use are ignored: another profile being parked, starting
a session, losing its login or being leased does not stop a swap to a
different target, a stay or an ALERT.

| Change since step 3 | Label | Applies to |
|---------------------|-------|------------|
| The rotate journal exists | `journal` | Every decision |
| The live profile, the account in `~/.claude.json`, the prototype ledger state, or whether `~/.claude/.credentials.json` is a readable login without OAuth (an API-key login), differs | `live` | Every decision |
| The [rotation pin](#rotate-pin) differs | `pin` | Every decision |
| The [overflow](#rotate-overflow) list or its ignored entries differ | `overflow` | Every decision |
| The swap target is no longer a candidate | `target eligibility: <reason>` | Swap only |
| The swap target is still a candidate, but the account in its `.claude.json` is not the one it had in step 3 | `target identity` | Swap only |

The last of the `live` fields is read from the login file,
`~/.claude/.credentials.json`, not from `~/.claude.json`. It holds only when
the login file is a readable JSON object without a `claudeAiOauth` key, the
[API-key](#api-key-profiles) case. A login file that is missing, cannot be
read or trusted, or holds an OAuth login does not satisfy it, so this field
changes only when the live login becomes or stops being an API-key login. A
login file that was readable in step 3 but cannot be read in step 6 leaves
no profile live, so it shows as a change in the live profile. That change
on its own is not read again: the second read described below is made only
when `~/.claude.json` has lost its account.

For the target, `<reason>` is the [skip reason](#rotate-candidates) it now
has, such as `no login`, `excluded`, `session running`, `parked` or
`not oauth`, or `gone` when the profile no longer exists. When `/proc` can no
longer be scanned, every profile is skipped as `session running`, so a swap
ends with `target eligibility: session running` and a stay or ALERT goes
ahead. Whether `/proc` can be scanned is not compared in its own right: a
change in it shows only through the target's skip reason, so it has no
label of its own and never affects a stay or an ALERT.

The target's account is the `oauthAccount` in `<root>/<name>/.claude.json`,
compared by the [account identity](#account-identity) rule that every other
account comparison in amplihack uses. A `profile login <name>` to another
account during step 4 is a `target identity` change, so the cycle never
moves in a login whose usage it did not measure. A rewrite of that file that keeps the account, such as the
probe itself updating it, is not a change. No other profile's account is
compared, and the login file's inode is not compared at all, because a
probe may refresh the OAuth token and rewrite it.

Exclusion is checked for the target only. The live profile is always
skipped as `live` before the `exclude` list is read, so adding the live
profile to `exclude` while a cycle measures has no effect on a swap away
from it.

A `journal`, `live`, `pin` or `overflow` change ends the cycle with a skip.
One of these means a person or another process has acted, for example with
[`profile switch`](#rotate-switch) or `auto-switch` settings, and the
rotator does not act against it. The next cycle measures again.

There is one exception. Running Claude Code sessions rewrite
`~/.claude.json`, and a read during a rewrite can find no account. When
`~/.claude.json` held an account in step 3 and holds none in step 6, and the
only other changes are to the target, the cycle logs
`revalidate: ~/.claude.json held no account; read again`, waits 500
milliseconds with the lock still held, and makes the comparison once more.
The cycle then acts on the second comparison alone: it goes ahead,
reconsiders the target as below, or skips with the labels the second
comparison found. There is at most one second read per cycle. A
`~/.claude.json` that names a different account is never read again: the
next cycle identifies the live login afresh, and
[hand-change recovery](#rotate-hand-change-recovery) acts when the ledger
names another account. A `journal`, `pin` or `overflow` change is never
read again either.

When the only changes are to the target's eligibility or identity, the
cycle reconsiders once:

1. The failed-live-probe count is put back to its value before this cycle,
   so a failed live probe is not counted twice (see
   [Failed live probes](#rotate-failed-live-probes)).
2. The step 4 readings are kept, minus those of profiles that are no longer
   candidates or whose account has changed since step 3. Nothing is probed
   again, so the lock is held throughout and the readings are as recent as
   those of the first decision.
3. The [decision rule](#rotate-decision-rule) runs again on the fresh state
   and those readings, recorded with the time of the original measurement.
   Damping and spend-limit blocks are judged against the current time.
4. If it gives a swap to a profile that is a candidate in the fresh state
   with its step 3 account, the cycle logs
   `revalidate: target <old> no longer eligible (<reason>); reconsidered`,
   where `<reason>` is the old target's bare skip reason, such as
   `session running`, `gone`, or `account changed` for a
   `target identity` change, without the `target eligibility: ` text used
   in the skip line. It then swaps to the new target and records the cycle
   from the fresh state and the new decision. Any other result, including
   a stay or an ALERT, ends the cycle with
   `skip: state changed during measurement (target eligibility: <reason>)`
   or `skip: state changed during measurement (target identity)`.

There is at most one reconsideration per cycle. The [swap](#rotate-swap)
still makes its own checks on the target's login and on `~/.claude` before it
moves anything, so a login that disappears after step 6 still causes a
refusal: the `revalidate:` line is then followed by
`ALERT: swap refused: <reason>` rather than a `swap` line.

For example, live login `work` is exhausted, `spare` and `third` are spare
logins at 0% session use, and `third` is parked while the cycle measures.
The decision is a swap to `spare`, `third` is not part of it, and the cycle
logs:

```
2026-10-29T20:16:25Z cycle live=work work:s=100%,w=100% spare:s=0%,w=43% third:s=0%,w=50%
2026-10-29T20:16:31Z swap work->spare live s=100% w=100% target s=0% w=43% reason=threshold
```

If instead `spare` starts a session while the cycle measures, the cycle
reconsiders and takes `third`:

```
2026-10-29T20:16:25Z cycle live=work work:s=100%,w=100% spare:s=0%,w=43% third:s=0%,w=50%
2026-10-29T20:16:31Z revalidate: target spare no longer eligible (session running); reconsidered
2026-10-29T20:16:31Z swap work->third live s=100% w=100% target s=0% w=50% reason=threshold
```

With `spare` the only candidate, the same change ends the cycle:

```
2026-10-29T20:16:31Z skip: state changed during measurement (target eligibility: session running)
```

A [held cycle](#pin-and-overflow) that records a spend-limit block makes the
same comparison with the rules for a stay: it stops only on a `journal`,
`live`, `pin` or `overflow` change, logs the same skip line, and never
reads again or reconsiders.

These related problems are known and tracked in their own issues:

- The live account and the target's account are read from `.claude.json`.
  A running session that writes the previous account back into
  `~/.claude.json` after a swap is not detected by step 6, and it can make
  [hand-change recovery](#rotate-hand-change-recovery) act on false
  evidence. Reading the account from data that moves with the login is
  [#1753](https://github.com/rysweet/amplihack-rs/issues/1753).
- Nothing checks after a swap that `~/.claude` holds the account the
  ledger records:
  [#1755](https://github.com/rysweet/amplihack-rs/issues/1755).
- An exhausted reading is logged as `s=100% w=100%` whatever the other
  window's figure, and a profile has been seen read as exhausted after its
  reset time:
  [#1754](https://github.com/rysweet/amplihack-rs/issues/1754).

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

A candidate whose probe failed, or that is exhausted, is never chosen. A
probe that timed out, or that could not start `claude`, read its output or
wait for it (`timeout`, or an `error` other than `profile directory
refused`), is *unknown*, not exhausted: it is not counted or recorded as
exhausted, does not [have capacity](#plan-pool), blocks a move onto
[overflow](#overflow-decision), and is probed again on the next cycle. See
[Failed and exhausted results](#failed-and-exhausted-results).
The *best* candidate has the
highest headroom; ties go to the name that sorts first by byte value.
Failed probes on some candidates do not stop a swap to another candidate
whose probe succeeded: with live `work` exhausted, `a` and `b` failed and
`c` at `s=0%,w=43%`, the cycle swaps to `c`, in `watch` and in the one-shot
`rotate` alike. The no-capacity ALERT means that no candidate measured with
enough headroom; its [suffix](#rotate-log) counts the candidates whose probe
failed or that were exhausted.

| Condition (all comparisons inclusive) | Outcome |
|---------------------------------------|---------|
| Live probe failed, or `session_used >= session-at`, or `week_used >= week-at`; and `best - live >= margin` | Swap to best, `reason=threshold`. A failed live probe counts as headroom `0`, and the swap is then subject to [failed-probe damping](#rotate-failed-live-probes). |
| As above, but no candidate meets the margin, or there are no candidates | ALERT `no-capacity`. No swap. |
| No threshold reached, `now - swapped_at >= hold` (or no earlier swap), and `best - live >= upgrade-margin` and `best - live > 0` | Swap to best, `reason=upgrade`. |
| Otherwise | Stay. |

`swapped_at` and `previous` (the profile swapped out) come from the
[ledger](#rotate-state-directory). During the hold no upgrade happens at
all, so `previous` cannot come straight back by upgrade. Threshold swaps on
a measured reading are never held.

#### Rotate failed live probes

When the table above gives a threshold swap only because the live probe
failed, the rotator applies these rules in order. `k` is the number of
consecutive cycles, including this one, whose live probe failed.

| Condition | Outcome | Log line |
|-----------|---------|----------|
| `k < 3` | Stay. | `stay: live probe failed (<k>/3)` |
| `k >= 3` and `now - swapped_at < hold` | Stay. | `stay: live probe failed; held after last swap (<N>s left)`, where `N = hold - (now - swapped_at)`, or `hold` if `swapped_at` is in the future |
| Otherwise | Swap to best. | `swap <from>-><to> live failed target s=<n>% w=<n>% reason=failed-probe` |

`k` is kept in memory by `watch`. It is reset to `0` by a cycle whose live
probe does not fail, by a completed swap, and by restarting the rotator. The
one-shot `rotate` always uses `k = 1`, so it never swaps on a failed live
probe alone. With `--hold 0` the second rule never applies, but the first
still limits these swaps to one every three cycles.

Nothing else changes: an ALERT when no candidate meets the margin, swaps on
a measured threshold, swaps on an exhausted live login and upgrade swaps
behave as in the table above. A damped stay counts as a stay without an
alert, so it clears the stored ALERT key.

### Rotate overflow

An *overflow profile* is one whose account has usage credits turned on at
<https://claude.ai/settings/usage>, so it keeps working after its plan's
session or weekly limit and the extra use is billed to the credits. A
*plan profile* is any other profile. Profiles are marked with
[`profile overflow add`](#overflow-commands); with no profile marked, every
rule in this section is inactive and the rotator behaves exactly as described
above.

Credits cost money and plan capacity does not, so the rotator:

- never moves onto an overflow profile while a plan profile has capacity;
- moves onto one only when the live plan login has reached a threshold,
  no plan profile has capacity,
  no plan candidate's probe in this cycle is
  [unknown](#rotate-decision-rule) (it timed out, or `claude` could not be
  started, read or waited for),
  and the next plan reset is **known** and more than `--overflow-wait`
  seconds away (an unknown reset spends money only with
  `--overflow-unknown-reset 1`);
- moves back to a plan profile in the first cycle in which one has capacity.

[Auto-switch](#auto-switch-and-overflow-profiles) follows the same rule
with the running rotator's gate: the wait, the unknown-reset choice, and
the plan-capacity test below (`--session-at`, `--week-at`). It does not
apply the rule about unknown plan probes: it decides from recorded
[readings](#readings), and an unknown probe is never recorded.

#### Plan pool

The *plan pool* is the live login, when it is not an overflow profile, plus
every [candidate](#rotate-candidates) that is not marked overflow. Excluded,
leased, hosting, parked, session-running and login-less profiles are not
candidates, so they neither offer capacity nor shorten the wait.

A plan candidate *has capacity* when all of these hold:

- its probe this cycle gave `ok`;
- its session use is below `--session-at`;
- its week use is below `--week-at`.

`--margin`, `--hold` and `--upgrade-margin` play no part in this test. A
plan login that has capacity is one the rotator would not swap away from
for reaching a threshold, which is what stops the [overflow decision](#overflow-decision)
from flapping. For example, with `--session-at 80`, a plan login at `s=85%`
does not have capacity, so the rotator stays on credits rather than moving
to it and back.

#### Overflow candidates

An overflow profile can be moved to when all of these hold:

- it is a [candidate](#rotate-candidates): the `exclude` list and every other
  skip reason apply to it as to any profile, so an excluded overflow profile
  is never used;
- it has no active [spend block](#overflow-spend-limit-block);
- its probe this cycle gave `ok`, `session_exhausted` or `weekly_exhausted`.
  Its session and week numbers are its plan limits and do not stop it being
  used. Any other state, including `timeout`, `unparsed` and
  `not_logged_in`, means it is not used this cycle.

The first eligible profile by [tier](#overflow-tiers), then by `overflow`
list order within a tier, is chosen.

#### Earliest plan reset

For each profile in the plan pool, the time it *regains capacity* is:

| Live or candidate reading | Regains capacity at |
|---------------------------|---------------------|
| Only the session window is at `--session-at` or exhausted | the session reset |
| Only the week window is at `--week-at` or exhausted | the week reset |
| Both are | the later of the two resets |
| Neither is | now |
| Probe failed, or a needed reset time is missing | unknown |

The *earliest plan reset* is the earliest of the known times; unknown ones
are left out, so a plan login whose probe failed never shortens the wait.
When **no** time is known, for example every plan login reads exhausted with
no `resets_at`, the earliest plan reset is unknown, and by default the
rotator does **not** move onto an overflow profile: it writes the
`no-capacity` ALERT and stays, because a move onto overflow costs money and
the plan login might be back in minutes. `--overflow-unknown-reset 1`
(`AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET=1`) makes an unknown reset count
as far away instead: work then moves onto an overflow profile and is billed
at once, and the `overflow: on` line reads `next plan reset unknown`. Turn
it on only if you would rather pay than wait for a reset nobody reported.

One known reset is enough. A plan candidate whose probe timed out or could
not run blocks the move anyway (see [Overflow decision](#overflow-decision)),
but one that read `not_logged_in`, `unparsed` or `spend_limit` does not. With
an expired login, for example, another login's known reset decides, and the
rotator can move onto overflow even though the expired login would have had
capacity again within `--overflow-wait`. Reset times are the `resets_at`
values that [`profile usage`](#probe-method) reads, kept for exhausted
readings too. A reset more than 8 days away in a usage request answer is
unknown, so it cannot make the wait look long.

#### Overflow decision

While the live login is a plan profile, the rotator first applies the
[decision rule](#rotate-decision-rule) and the
[failed-probe rules](#rotate-failed-live-probes) to the plan pool alone.
Only when that gives `ALERT no-capacity`, or a `reason=threshold` swap onto
a plan candidate that does not itself [have capacity](#plan-pool), on a
measured live reading does it consider overflow. A threshold swap onto a
plan candidate with capacity always stands:

| Condition | Outcome |
|-----------|---------|
| Live probe failed | As without overflow. A failed live probe never moves work onto credits. |
| A plan candidate's probe in this cycle ended in `timeout` or `error` | As without overflow: the ALERT, or the threshold swap onto the plan candidate without capacity. That login might have capacity, so a failed measurement never moves work onto credits. It is probed again on the next cycle. |
| A plan candidate has capacity (it is below both thresholds but short of `--margin`) | ALERT `no-capacity`, as without overflow. Credits are never used while a plan login has capacity. |
| `earliest plan reset - now > overflow-wait`, and an overflow candidate is eligible | Swap to the first eligible one, `reason=overflow-in`. The `--hold` does not apply. |
| `earliest plan reset - now <= overflow-wait` | As without overflow: the ALERT, or the threshold swap onto the plan candidate without capacity, which costs nothing. |
| Earliest plan reset unknown | As without overflow. With `--overflow-unknown-reset 1`, as for a reset beyond the wait. |
| No overflow candidate is eligible | As without overflow. |

For example, with the live login at 95% session and a plan candidate at 87%
session, the decision rule would swap onto the candidate, but it is at
`--session-at` itself and would be swapped away from in the next cycle. With
the plan reset beyond the wait, the rotator moves onto credits instead; with
the reset near, it makes the free swap. Auto-switch makes the same choice on
the same readings.

The comparison is strict: a reset exactly `--overflow-wait` seconds away
stays. With `--overflow-wait 0` the rotator moves onto overflow as soon as
every plan profile is out of capacity and the earliest reset is known and in
the future.

A plan candidate that reads `not_logged_in`, `unparsed` or `spend_limit`
does not block the move, so a login that has expired cannot keep the rotator
off credits for good. Nor does a plan candidate whose directory fails the
check just before the spawn: it reads `error` (`profile directory refused`)
on every cycle, not once, so it is a lasting state like those, not an
unknown probe. It is never a target, and `amplihack profile usage <name>`
shows the same `error` row; fix the directory, or exclude or remove the
profile.

An unknown plan probe stops only a move from a plan login onto an overflow
profile. It does not stop these moves:

| Move | Why it is not stopped |
|------|-----------------------|
| From a live overflow profile with a [spend block](#overflow-spend-limit-block) to the next eligible overflow profile | The blocked login cannot spend, so staying on it would only stop work. |
| From a live [API-key](#api-key-rotation) profile to an eligible credit profile | Credits cost less than the API key. |
| `overflow-out` back to a plan login | Its target is always a plan login whose probe read figures; an unknown probe is never a target. |

While the live login is an overflow profile, these rules apply in order:

| Condition | Outcome |
|-----------|---------|
| A plan candidate has capacity | Swap to the plan candidate that has capacity with the most headroom, `reason=overflow-out`. `--margin`, `--hold` and `--upgrade-margin` do not apply. |
| The live reading is `spend_limit` | [Block](#overflow-spend-limit-block) the live profile, then swap to the next eligible overflow candidate if the wait rule above still holds, else to the plan candidate with the most headroom that is not exhausted and whose probe did not fail, else ALERT `no-capacity`. |
| Live probe failed | [Failed-probe rules](#rotate-failed-live-probes), with plan candidates as the only targets; otherwise stay on credits. |
| Otherwise | Stay on credits. |

`--session-at` and `--week-at` are never applied to a live overflow profile:
reaching its plan limit is expected. An overflow profile is never swapped for
another overflow profile except after a spend limit, and never for a lower
tier that becomes usable again.

The rules do not flap. `overflow-out` moves only to a plan login that is
below both thresholds, so the next cycle does not swap away from it. The
rotator moves back onto credits only once that login reaches a threshold
again and every rule above holds again.

If an overflow profile's login is live in `~/.claude` while a plan profile
has capacity, for example after you put it there by hand, the next cycle
moves it out with `reason=overflow-out`. To keep running on it, remove the
mark, stop the rotator, or put it there with `amplihack profile switch <name>`,
which [pins](#pin-and-overflow) it.

Removing the mark from the live overflow profile does not by itself stop
credit spending. From the next cycle the login is a plan profile at or past
its limits, so the ordinary [decision rule](#rotate-decision-rule) applies:
it is swapped out only when a candidate meets `--margin`, and otherwise the
cycle writes `ALERT: no profile has capacity` and leaves the login in
`~/.claude`, where Claude keeps billing work to the credits. See
[Stopping credit use](#stopping-credit-use).

The [default pointer](#clearing-the-default) is outside this policy. A
launch that resolves to a profile through `.default` uses that profile
directly, whether it is a plan profile or an overflow profile, so leave the
pointer unset (`amplihack profile switch --none`) when using overflow.

#### Pin and overflow

A [rotation pin](#rotate-pin) from `profile switch <name>` wins over every
overflow rule, in both directions. The pin is checked before the cycle
measures, so the overflow decision never runs while a pin is in force:

| Pinned login | Readings | Outcome |
|--------------|----------|---------|
| An overflow profile | A plan login has capacity | `held`. The login stays on the overflow profile and keeps spending credits. Switching onto an overflow profile is a choice to spend. `status` shows `on overflow: yes (<name>, held by switch)`. |
| An overflow profile | Its credits run out | `held`, and still watched for the spend limit (see below). The login does not move, but the profile is blocked with one ALERT that names `profile switch --none`. |
| A plan profile | Every plan login is out of capacity and an overflow profile is eligible | `held`. The cycle never moves onto credits, so nothing is spent. |

`amplihack profile switch --none` removes the pin; the next cycle applies
the overflow rules above, so it moves off a pinned overflow profile at once
if a plan login has capacity. The rotator's own moves onto or off overflow
never write a pin.

The pin stops moves, not the spend-limit check. While the pin holds the live
login on an overflow profile that has no active block, each held cycle probes
the live login only, without the lock, and measures no candidate. If the probe
reads [`spend_limit`](#states), the cycle takes the lock again, checks that
nothing changed while it probed (with the rules for a stay in
[Rotate revalidation](#rotate-revalidation)), and blocks
the profile as in [Overflow spend-limit block](#overflow-spend-limit-block),
with this ALERT in place of the usual one:

```
ALERT: overflow profile <name> hit its spend limit; blocked until <time>; rotation is held on it by `profile switch`, so run `amplihack profile switch --none` to move off it
```

The login stays where it is, since the pin still holds. Once the block is
recorded the held cycles stop probing until it ends or is cleared. A pinned
plan login is never probed. `profile switch` refuses an overflow profile that
already has an active block (see [Rotate switch](#rotate-switch)).

The `--hold` that `overflow-in` and `overflow-out` ignore is the minimum time
after a rotator swap. It is not the pin.

#### Overflow spend-limit block

Claude's `You've hit your org's monthly spend limit …` message, and the
older `Your organization has reached its monthly spend limit` wording, mean
the account's credits are used up or not turned on. The
[usage probe](#what-is-read) matches both, without regard to case, and
classifies the profile as `spend_limit`. The probe is the only detector:
the rotator probes the live overflow profile on every cycle, including a cycle
held by a [pin](#pin-and-overflow) on it, so a block is recorded within one
cycle. (A held cycle probes only the live login, and only while it has no
active block.) The [usage-limit wait](#usage-limit-wait) and the
recipe-run classifier see the same message in an agent's output but never
write a block. A probe that reads
[`spend_limit`](#states) for an overflow profile, live or candidate, *blocks*
that profile:

1. Under the lock taken in [step 6](#rotate-cycle), the rotator adds
   `{"at": <now>, "until": <reset>, "reason": "spend_limit"}` to
   `overflow_blocked` in the [ledger](#rotate-state-directory). `<reset>` is
   00:00 UTC on the first day of the next month. The probe text, including
   any reset Claude states, is not stored.
2. It writes `ALERT: overflow profile <name> hit its spend limit; blocked until <time>`
   once for that block. The block record, not the ledger's `alert` key,
   prevents a repeat, so the ALERT does not stop the
   [hook](#capacity-for-a-resume) from seeing capacity when the cycle moves
   somewhere else.
3. A blocked profile is not an overflow candidate. If it was live, the
   cycle moves away as in the table above.

A block ends at its `until` time or with
[`profile overflow clear <name>`](#overflow-commands). Expired blocks are
removed from the ledger the next time the rotator records a cycle. The reset
time Claude prints in the spend-limit message is not read; a spend limit
raised or credits added before the 1st need `overflow clear`.

A `spend_limit` reading on a plan profile is a failed probe, as it always
was. A `spend_limit` reading on a live overflow profile does not count
towards the three failed live probes.

The block depends on the spend-limit text appearing in the output of
`claude -p /usage` for that login. The [usage request](#probe-method) has no
spend-limit field, so profiles marked as overflow are probed with `/usage`
only, never with the usage request, live or candidate, in a held cycle too.
An overflow profile needs its usage credits turned on: an account with them
off has no spend limit for `/usage` to show, and it stops at its plan limit
like a plan login. If the text does not appear, the rotator cannot see that credits have run out, and sessions on
that login keep failing. The two ways out behave differently:

- `profile overflow remove <name>` makes the login a plan profile. A
  `spend_limit` reading on it is then a failed live probe, so the
  [failed-probe rules](#rotate-failed-live-probes) swap it out only after
  three consecutive failed cycles, and only to a plan candidate. Any other
  reading is judged by the ordinary decision rule, as described above.
- `profile rotate stop` stops all swapping and leaves the login in
  `~/.claude`. Nothing moves it until you start the rotator again.

#### Stopping credit use

No `amplihack` command puts a plan login into `~/.claude` when no plan
login has capacity, so neither `overflow remove` nor `rotate stop` stops
spending on its own. To stop spending at once, turn off usage credits or
lower the spend limit at <https://claude.ai/settings/usage>. Claude then
refuses work on that login, the next probe reads `spend_limit`, and the
rotator [blocks](#overflow-spend-limit-block) the profile and moves away from
it as described above. Ending the Claude sessions that run on the login also
stops spending, but a new launch uses it again.

### Rotate live profile

The live profile is the one whose login is in `~/.claude`. Accounts are
compared by the [account identity rule](#account-identity) (user and
organization), read from `$HOME/.claude.json` and from each
`<root>/<name>/.claude.json`. The rotate ledger's `account` field is still
the user only (or the e-mail address when there is no uuid), so the
`hand-change` check does not notice a new login to the same user in
another organization. No credentials file is read. An account value
over 320 bytes or containing control characters counts as missing. Account
values are never logged or printed.

| Evidence, checked in order | Result |
|----------------------------|--------|
| `~/.claude/.credentials.json` is missing or not a regular file | ALERT `no-login`. No swap. A login is never moved into an empty `~/.claude`. |
| The rotate ledger exists and its `account` matches `~/.claude.json` | Its `live` profile. |
| The rotate ledger exists, and `~/.claude.json` has no account that can be read | ALERT `home-unreadable`: `ALERT: the account in ~/.claude.json cannot be read (live=<L>); no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace <L>'s login`. Nothing is recovered, moved or swapped, and no `fix` line is written. Claude rewrites `~/.claude.json` often, so one failed read is not taken as a hand change: if the ledger is right, the login in `~/.claude` is `<L>`'s, and a `/login` there would leave `<L>` with no login anywhere. This holds only for the first 5 minutes: once the file has had no readable account for 5 minutes or more, the cycle writes the [stuck `~/.claude.json` guidance](#stuck-claudejson) instead, with the file's problem and `fix` lines. |
| The rotate ledger exists and its `account` does not match | [Hand-change recovery](#rotate-hand-change-recovery) decides. It runs before this table in every cycle, `--dry-run` included, and either records a matching ledger or stops the cycle, so this row is never reached. |
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

### Rotate hand-change recovery

A manual `/login` in `~/.claude` replaces whatever login was there; the
profile that login belonged to is left with no login. The rotate ledger then
names a profile `L` whose account is no longer the one in `~/.claude.json`.

Hand-change recovery runs under the lock in every non-dry-run `rotate`,
`start` and cycle, after [rotate recovery](#rotate-recovery) and before the
live profile is identified. It acts only when the ledger's `account` does not
match the account read from `~/.claude.json`; when no account can be read
there, it does nothing and the cycle ends with the `home-unreadable` ALERT
from [Rotate live profile](#rotate-live-profile), until that has lasted 5
minutes; from then on it writes the
[stuck `~/.claude.json` guidance](#stuck-claudejson). `profile switch` does not
run it: a switch refuses a hand-changed login (see
[Rotate switch](#rotate-switch)) and leaves recovery to the next cycle. It reads `.claude.json` files and file metadata only,
and moves at most one login, by rename. It never opens, copies or deletes a
credentials file.

Recovery only records which profile is live; it decides no move. When the
recorded profile `M` is an [overflow](#rotate-overflow) profile, the same
cycle measures and applies the overflow policy as for any cycle that starts
on credits: with a plan login that has capacity it moves off `M`, and `M`'s
login (the one from the manual `/login`) goes back to `<root>/<M>/`, so `M`
is never added to `lost_login`.

#### Holders

The account in `~/.claude.json` is compared, by the
[account identity rule](#account-identity), with every profile that passes
the usual directory check, whether or not it has its own login. Checked in
this order, the first that holds decides:

| Holders | Condition |
|---------|-----------|
| `home-no-login` | `~/.claude/.credentials.json` is missing or not a regular file. |
| `home-unreadable` | `~/.claude.json` has no usable account. Reached only when the file was read with a different account and then, a moment later, with none (Claude rewrote it in between). |
| `unreadable` | At least one profile with its own login has no usable account. A profile with no login and no account is skipped. |
| `none` | No profile has the account. |
| `one` | Exactly one profile `M` has it. `own login` is whether `<root>/<M>/.credentials.json` exists (by `lstat`). |
| `many` | Two or more profiles have it. |

`M` may be `L` itself, when the ledger's `account` was out of date. The same
rules apply, without the lost-login line.

#### Actions

| Holders | Action | Log line |
|---------|--------|----------|
| `one`, no own login | Record `M` as live. Nothing moves. | `recovered: ~/.claude holds <M>'s login (ledger named <L>); recorded <M> as live` |
| `one`, own login, every check passes | Move `<root>/<M>/.credentials.json` to the backup, then record `M` as live. | `recovered: ~/.claude and <M> held the same account; kept ~/.claude, moved <M>'s copy to <backup>/.credentials.json; recorded <M> as live` |
| `one`, a check fails | Nothing moves. Guidance. | The refused ALERT from [Guidance](#guidance) and its `fix` line |
| Anything else | Nothing moves. Guidance. | The ALERT for those holders from [Guidance](#guidance) and its `fix` lines |

Recording `M` sets the ledger's `live` to `M`, `account` to `M`'s account,
`since` to the cycle's time, and clears `alert` and `guidance`, in one atomic
write. If `L` is not `M` and `L` has no login file, `L` is added to
`lost_login`. After a recovery the cycle continues from step 3 with the new
ledger: `L` is skipped as `no login`, and the same cycle may swap to a
candidate with capacity or raise `no-capacity`.

Checks before recording, in both `one` cases: the profile root is private, no
swap journal is pending or inconsistent, and `HOME` and `~/.claude` pass the
[swap's](#rotate-swap) home checks. Extra checks before the move, each with
its `<reason>`:

| Check | `<reason>` when it fails |
|-------|--------------------------|
| No running process has `CLAUDE_CONFIG_DIR=<root>/<M>`. A host where running sessions cannot be seen fails this check. | `a claude session is running on <M>` / `cannot see running sessions` |
| Neither `~/.claude/.oauth_refresh.lock` nor `<root>/<M>/.oauth_refresh.lock` exists. | `Claude holds its refresh lock` |
| `M`'s login is not leased and `M` does not host another login in the auto-switch ledger; an unreadable auto-switch ledger fails this check. | `<M>'s login is lent out or it hosts another login` |
| `<root>/<M>/.credentials.json` is a private regular file with one link, owned by you. | `a login file is not a private regular file with one link` |
| `~/.claude`, `<root>/<M>` and the root are on one device. | `~/.claude and the profile root are on different devices` |
| `.rejected-logins/` and the backup directory can be created. | `cannot create the backup directory: <error>` |
| `.rejected-logins/` and the backup directory are real directories owned by you with no group or other bits. | `backup directory is not private: <path>` |
| A free backup name exists. | `no free backup name` |
| `~/.claude.json` still has `M`'s account after the backup directory is created. | `~/.claude changed during recovery` |

The backup is `<root>/.rejected-logins/rotate-<M>-<YYYYMMDDTHHMMSSZ>/`, where
the time is the cycle's start in UTC. If that name exists, `-2` to `-99` is
added. `.rejected-logins/` is created `0700` if it is missing; one that
exists must be a real directory owned by you with no group or other bits, as
`profile login` requires. The backup directory is created fresh, `0700`,
never through a symlink, and must then be a directory owned by you with no
group or other bits; one that is not is removed again (it is new and empty)
and the move is refused. Its parent is synced. The login is moved into it
with `renameat2(RENAME_NOREPLACE)`.
Nothing else is moved, and `<root>/<M>/.claude.json` is not changed.

The steps run in this order: checks, create the backup directory, re-check
`~/.claude.json`, move the login, write the ledger. There is no journal.
Every interrupted state is safe:

| Stopped after | State | Next cycle |
|---------------|-------|------------|
| Creating the backup directory | An empty `rotate-<M>-<time>/`. | Uses the next free name. The empty directory is left. |
| Moving the login | `M` has no login; the ledger still names `L`. | Holders `one`, no own login: records `M`. |

If a session on `M` writes a new login there after the move, the next cycle
finds `one` with own login again and makes a new backup.

#### Guidance

When nothing can move, the cycle writes an ALERT line and numbered `fix`
lines, ends as `alerted` and does not swap. Every ALERT starts with
`ALERT: the account in ~/.claude is not the one the ledger names (live=<L>);`
then has one of these middles, then ` no swap`:

| Holders | Middle of the ALERT |
|---------|---------------------|
| `many` | `it matches more than one profile (<M1>, <M2>);` |
| `none` | `no profile has that account;` |
| `unreadable` | `the account of profile <P1>, <P2> cannot be read, so the match is uncertain;` |
| `home-unreadable` | `the account in ~/.claude.json cannot be read;` |
| `home-no-login` | `~/.claude has no login;` |
| `one`, no own login, a check failed | `profile <M> holds the same account; recovery refused: <reason>;` |
| `one`, own login, a check failed | `profile <M> holds the same account and has its own login; recovery refused: <reason>;` |

Profile lists are sorted by name and joined with `, `.

For example, with no profile holding the account:

```
ALERT: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); no profile has that account; no swap
```

The `fix` lines follow. `fix 1: amplihack profile login <L>` comes first,
and only when `L` has no login file; the lines below are numbered after it.
Each is written exactly as shown, after `fix <n>: `:

| Holders | `fix` lines, in order |
|---------|-----------------------|
| `many` | `keep this account in one of <M1>, <M2>: log the others in to another account or remove them; the next cycle then recovers` |
| `none` | `rm <ledger> (the next swap keeps the login in ~/.claude as profile default-original)`<br>`or, to set that login aside instead: close every claude session that uses ~/.claude, then run: mkdir -m 700 <dir> && mv -n <home>/.claude/.credentials.json <dir>/ && rm <ledger>; then run claude with no CLAUDE_CONFIG_DIR and /login` |
| `unreadable` | `amplihack profile login <P>; the next cycle then recovers`, one line per profile `P` in the ALERT |
| `home-unreadable` | `none needed; the next cycle reads ~/.claude.json again` (never `/login`: one failed read does not show whose login `~/.claude` holds) |
| `home-no-login` | `run claude with no CLAUDE_CONFIG_DIR and /login` |
| refused | One line, by `<reason>`; see the next table. |

`unreadable` lists only profiles with a login file, so `L` never appears both
in `fix 1` and in an `unreadable` line.

The refusal `fix` line for each `<reason>`:

| `<reason>` | `fix` line |
|------------|------------|
| `an earlier swap stopped half way` | `see the Recovery section of docs/howto/claude-profile-rotation.md` |
| `HOME is not a directory owned by you` | `make <home> owned by you and run: chmod go-w <home>` |
| `~/.claude is not a private directory owned by you` | `make <home>/.claude owned by you and run: chmod 700 <home>/.claude` |
| `profile root is owned by another user`, `profile root is writable by other users` | `make <root> owned by you and run: chmod 700 <root>` |
| `a claude session is running on <M>` | `close the claude sessions that use <M>; the next cycle retries` |
| `cannot see running sessions` | `close the claude sessions that use <M>, then run: mkdir -m 700 <dir> && mv -n <root>/<M>/.credentials.json <dir>/` |
| `Claude holds its refresh lock` | `wait; the next cycle retries` |
| `<M>'s login is lent out or it hosts another login` | `wait for the auto-switch session to end; the next cycle retries` |
| `a login file is not a private regular file with one link`, `~/.claude and the profile root are on different devices`, `cannot create the backup directory: <error>`, `backup directory is not private: <path>`, `no free backup name` | `move <M>'s copy by hand: mkdir -m 700 <dir> && mv -n <root>/<M>/.credentials.json <dir>/` |
| `~/.claude changed during recovery` | `none needed; the next cycle retries` |

The first four reasons come from the checks before recording, so they can
appear with or without an own login; the rest come from the checks before
the move and appear only with one.

`<ledger>` is `<root>/.auto-switch.d/rotate/ledger.json`, `<home>` is the
real `HOME` path (never `~`), `<root>` is the profile root, and `<dir>` is
`<root>/.rejected-logins/manual-<time>`, where `<time>` is the cycle's time
as `YYYYMMDDTHHMMSSZ`. A path made only of `A-Z a-z 0-9 / . _ + -` is
printed bare; any other path is single-quoted, with each `'` written as
`'\''`. If any path a line needs has a control or bidirectional-override
character, or the whole line would be longer than 1024 bytes, the line is
`move <what> by hand; its path cannot be printed safely` instead, where
`<what>` is `the login in ~/.claude` or `<M>'s copy`. A line that names a
path is never cut, since a cut path names another file; only lines without
a path are cut to 1024 bytes.

#### Stuck `~/.claude.json`

A `~/.claude.json` with no readable account while a login is in `~/.claude`
and the ledger names a live profile `<L>` starts a *spell*. The first cycle
that sees it records the time in the ledger's `home_unreadable_since`; the
first cycle that reads an account there again, or finds no login in
`~/.claude`, clears it. A time later than the cycle's (the clock went back)
counts as the cycle's time.

While the spell is under 5 minutes old (`HOME_UNREADABLE_GRACE`, 300
seconds), the cycle writes only the `home-unreadable` ALERT from
[Rotate live profile](#rotate-live-profile). A cycle never escalates on its
own first read, so with the default 600-second interval the second cycle in
a row is the first that can. From 5 minutes on, every cycle, `--dry-run`
included, writes this ALERT and these `fix` lines to the log and the
ledger's `guidance`, so `rotate status` shows them, and ends `alerted`
without a swap. Nothing moves.

```
ALERT: ~/.claude.json has had no account that can be read for 5 minutes or more (live=<L>): <problem>; no swap until it is fixed; do not /login in ~/.claude before the login there is set aside, it would replace <L>'s login
fix 1: run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if <home>/.claude.json then holds an account, the next cycle goes on
fix 2: or give <L> its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e <root>/<L>/.credentials.json && mv -n <home>/.claude/.credentials.json <root>/<L>/ && rm <ledger>; then run claude with no CLAUDE_CONFIG_DIR and /login
```

`<problem>` is the first that holds:

| State of `<home>/.claude.json` | `<problem>` |
|--------------------------------|-------------|
| Missing | `it does not exist` |
| A symlink, not a regular file, over 32 MiB, or not readable | `it cannot be read (a symlink, not a regular file, over 32 MiB or not readable)` |
| Empty or not JSON | `it is not valid JSON (empty or cut short)` |
| JSON, but not an object | `it is not a JSON object` |
| No `oauthAccount` key, or `null` | `it has no oauthAccount block` |
| An `oauthAccount` with no `accountUuid` or `emailAddress` that passes the [account value rules](#rotate-live-profile) | `its oauthAccount block has no usable accountUuid or emailAddress` |

Both fixes keep `<L>`'s login. `fix 1` lets Claude write the file again
from the login it already has. `fix 2` renames that login back into
`<L>`'s profile directory, where it came from, before any `/login`: the
`test` stops the line when `<L>` already has a login file, so nothing is
replaced, and `mv` fails when `<root>/<L>` is not a directory. With the
ledger gone, the next cycle identifies the new login as usual. The
`fix 1: amplihack profile login <L>` line of the hand-change guidance is not
written here, because `<L>`'s login is the one in `~/.claude`.
`amplihack profile switch` is not offered: a switch refuses while the live
login is `home-unreadable` (see [Rotate switch](#rotate-switch)). Paths are
printed by the rules above.

Separately, every cycle writes, for each profile in the ledger's
`lost_login`:

```
ALERT: profile <L> has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login <L>
```

A profile leaves `lost_login` when `<root>/<L>/.credentials.json` exists again
or `<root>/<L>` is no longer a real directory (it was removed, or replaced by
a symlink). In a cycle whose guidance already has
`fix 1: amplihack profile login <L>`, the lost-login line for `L` is left out.

`lost_login` holds at most 64 profiles, which keeps the ledger a bounded size.
A profile that would be added to a full list is not added; instead, once, at
that recovery:

```
ALERT: profile <L> has no login anywhere, and the list of such profiles is full, so this is not repeated. Run: amplihack profile login <L>
```

Guidance and lost-login lines are written on every cycle while their
condition holds, at most once each per cycle, without the
[ALERT key](#rotate-log). The one-shot command also prints them. The
ledger's `guidance` holds the current guidance lines and is cleared when the
ledger's account matches `~/.claude.json` again; [`status`](#rotate-status)
prints both lists.

`--dry-run` runs the same holder check without the lock and writes nothing.
It prints `would recover: <recovered line>` and stops before measuring, or
`would alert: <ALERT line>` followed by the `fix` lines, and then stops with
outcome `alerted`. `<recovered line>` and `<ALERT line>` are the log text
without its `recovered: ` or `ALERT: ` prefix, as in the other `would alert:`
lines. It does not go on to the
[live profile](#rotate-live-profile) table, so it never prints a second
`would alert:` line for the same mismatch.

### Rotate switch

`amplihack profile switch <name>` moves a profile's login into `~/.claude`
by hand. It uses the rotator's lock, recovery,
[candidate check](#rotate-candidates), [swap](#rotate-swap) and ledger entry,
so it refuses every profile the rotator would refuse, and the result is
exactly what a rotate swap to `<name>` would leave. Then it pins rotation to `<name>` (see [Rotate pin](#rotate-pin)) and
makes `<name>` the default for new launches. No rotator needs to be running.

The steps run in this order. A step that refuses ends the command with
status 1 and writes no pin, no ledger ALERT and no new swap. Moving the live
login and setting the default for new launches are separate: a refusal at
steps 6-8, which is about the live login or `<name>` itself, still writes
`<root>/.default` (see [Default only](#default-only)); a refusal at steps
1-5 writes nothing, `.default` included. Step 5 may already have finished or
rolled back an interrupted swap, which changes the ledger, the journal and
`~/.claude`; each such recovery is logged as a `recovery: ...` line and
stays done even if a later step refuses. When no recovery ran, a refusal
changes nothing but `.default`.

1. Check the name with the [name rules](#name-rules) and find the profile.
   An unknown profile is refused here, before any lock is taken.
2. On a platform without Unix file semantics, write `<root>/.default`,
   print the two [default-only](#default-only) lines (`rotation not held`),
   and fail with `RotateUnsupported`.
3. Run the [prototype check](#rotate-prototype-coexistence) and check that
   the profile root is private.
4. Take `<root>/.auto-switch.d/lock`, waiting up to 2 seconds. A busy lock is
   refused with `RotateRefused` (`state lock busy`).
5. Run auto-switch [recovery](#session-end-and-recovery), then
   [rotate recovery](#rotate-recovery). Either one finding an inconsistent
   state is refused.
   Then, when `<name>` is an [API-key profile](#switching-onto-an-api-key-profile) and the
   [API-key block](#api-key-block) is active, the switch is refused with
   `PlatformBlocked`. A ledger that cannot be read fails the switch.
6. [Identify](#rotate-live-profile) the live login:

   | Live login | Action |
   |------------|--------|
   | `<name>` | Nothing moves. Step 7 is skipped. |
   | Another profile | Swap from that profile to `<name>`. |
   | No profile owns it | Adopt it as `default-original` (or the next free name), then swap, as the rotator does. |
   | `no-login`, `hand-change`, `home-unreadable` or `ambiguous` | Refused with the matching ALERT text, without the `ALERT: ` prefix: [default only](#default-only). No ALERT is written to the ledger. |

   Then, whether or not `<name>` is already live: when `<name>` is an
   [overflow profile](#rotate-overflow) with an active
   [spend-limit block](#overflow-spend-limit-block), the switch is refused,
   [default only](#default-only), with
   ``overflow profile <name> is blocked for its spend limit until <time>; clear the block with `amplihack profile overflow clear <name>` once credits are added``.
   A pin stops every rotator move, so pinning a profile whose credits are
   known to be spent would hold the login where it cannot work. A ledger
   that cannot be read fails the switch, since its blocks are then unknown.

7. Unless `<name>` is already the live profile, check `<name>` against
   [Rotate candidates](#rotate-candidates), using the list built under the
   lock in step 6. A profile with any skip reason is refused, the same
   profiles the rotator never swaps to, so a switch never moves a login out
   from under a running session, an auto-switch lease or a hosted login.
   The refusal is [default only](#default-only): `<name>` still becomes the
   default for new launches.
   The refusal is
   `profile <name> is not a rotate candidate (<reason>): <detail>`, where
   `<reason>` is the skip reason the rotator logs:

   | Reason | Detail |
   |--------|--------|
   | `no login`, file missing | The whole message is ``profile <name> has no stored login; run `amplihack profile login <name>` first``. |
   | `no login`, file present | `a login file is not a private regular file with one link` |
   | `excluded` | ``it is in the auto-switch exclude list; set the list without it with `amplihack profile auto-switch exclude` first`` |
   | `session running` | `a running process has CLAUDE_CONFIG_DIR set to it; end that session first` |
   | `session running`, `/proc` cannot be scanned | `running sessions cannot be seen here, so its login may be in use`. Every profile but the live one is refused, as every profile is skipped by the rotator. This is every system but Linux, macOS included: there `switch` only sets the default for new launches. |
   | `leased` | `auto-switch has lent its login to another profile's session` |
   | `hosting` | `its directory holds another account's login for auto-switch` |
   | `parked` | `auto-switch has parked its own login` |
   | `no account` | `its .claude.json has no usable oauthAccount` |

   `excluded` and `parked` refuse too. The exclude list keeps a profile out
   of rotation, and a switch is a rotation done by hand; to switch to an
   excluded profile, take it off the list first. A parked profile's own
   login sits in `<root>/.auto-switch.d/parked/` while its directory holds
   another account, so there is nothing of its own to move.
8. Run the [swap](#rotate-swap) unchanged: the same pre-swap checks, journal,
   refresh locks and ledger entry. A refusal from the swap moves nothing and
   is passed through unchanged, [default only](#default-only). The log gets
   `switch <from>-><name>`.
9. Still under the lock, write the [pin](#rotate-pin), then the default
   pointer `<root>/.default`, then the ledger's `last_cycle`:
   `outcome` `switched`, with `at` and `measured_at` both the current time,
   and clear the ledger's ALERT key. This record is what lets the
   [limit-resume hook](#capacity-for-a-resume) wake sessions that stopped on
   a usage limit before the switch; see
   [Sessions stopped by a limit](#sessions-stopped-by-a-limit).
10. Release the lock, then check the
    [shared session links](#shared-session-directories) of `<name>` and of
    the profile swapped out. Problems there are warnings on stderr only.

`switch` reads none of the `AMPLIHACK_ROTATE_*` [settings](#rotate-settings)
and never runs `claude`: it measures nothing, so it makes no usage or OAuth
calls. It does not look for a `claude` binary either, so it works with no
`claude` on `PATH`. A login with no capacity left is still moved: `switch`
does what you asked, and the usage checks that stop a rotate swap do not
apply to it.

On success it prints three lines and exits 0:

```
Live login moved: work -> spare
Default for new launches: spare
Rotation held on spare until `amplihack profile switch --none`.
```

When `switch` adopts a `~/.claude` that no profile owned, the first line
names the adopted profile, for example
`Live login moved: default-original -> spare`, and the log gets
`switch default-original->spare`.

When `<name>` is already the live login, the first line is
`spare is already the live login; nothing moved.` and the log gets
`skip: spare is already the live login; nothing moved`.

| Result | Output | Exit |
|--------|--------|------|
| Swapped, pin and pointer written | The three lines above | 0 |
| Already live, pin and pointer written | The three lines, first line `<name> is already the live login; nothing moved.` | 0 |
| Refused at steps 1-5 (lock busy, prototype, private root, inconsistent recovery) | `error: ...` and `hint: ...`; no pin, pointer, ALERT or swap written (any step-5 recovery stays done) | 1 |
| Refused at steps 6-8, or step 2 off Unix | The two [default-only](#default-only) lines, then `error: ...` and `hint: ...`; `.default` written, no pin, ALERT or swap | 1 |
| The swap ran, then writing the pin, the pointer or the ledger failed | ``error: the live login moved to <name>, but writing the <rotation hold\|default pointer\|rotate ledger> failed: <error>; run `amplihack profile switch <name>` again`` | 1 |
| A name and `--none` together, or neither | clap usage error | 2 |

After a partial failure, `~/.claude` already holds `<name>`'s login, so
running the same `switch` again takes the already-live path and writes the
pin, the pointer and the ledger record. It is safe to repeat.

A refused `switch` writes no ledger ALERT. If step 5 ran no recovery, it
also changes no byte of the ledger, the journal, the pin, `~/.claude` or
`~/.claude.json`, and of `.default` only what a default-only refusal
writes. If a recovery ran, only that recovery's changes are made, and its
`recovery: ...` line says what it did.

#### Default only

Making `<name>` the default for new launches never depended on moving the
live login, and it still does not. When the live login cannot be moved for
a reason about the live login or `<name>` (step 6, 7 or 8, or a platform
without Unix file semantics), `switch` writes `<root>/.default` under the
rotate lock, prints two lines on stdout, and exits 1 with the refusal on
stderr:

```
Default for new launches: spare
Live login not moved; rotation not held.
```

It writes no pin and no ledger record, so the
[limit-resume hook](#capacity-for-a-resume) does not treat it as capacity,
and the rotator keeps rotating. A pin left by an earlier `switch` is not
changed; the second line then names it:
``Live login not moved; rotation still held on work until `amplihack profile switch --none`.``
The pin decides what `~/.claude` holds and `.default` decides what
amplihack launches without `--profile` use, so after a default-only refusal
they can name different profiles until the next successful `switch` or
`switch --none`. `switch --none` removes both.

This is how `switch` sets the default on macOS, where `/proc` cannot be
scanned and every profile counts as having a running session, and for a
profile that is open in a session of its own (`CLAUDE_CONFIG_DIR` set to its
directory).

If `switch` is stopped partway through a swap, the journal is left behind as
for any swap. The next `switch`, `rotate` or rotator cycle runs
[recovery](#rotate-recovery) first, and no pin or pointer has been written.

A second `switch <other>` while a pin is in force swaps again and moves the
pin to `<other>`.

#### Sessions stopped by a limit

A `switch` counts as capacity for the
[limit-resume hook](#capacity-for-a-resume) and the
[usage-limit wait](#usage-limit-wait): you chose that login, so the switch
stands in for a measurement taken at the moment of the switch. Sessions that
stopped on a usage limit before the switch are resumed on the next ledger
read, within 15 seconds in the first hour of their wait, the same way they
are after a rotator swap. This holds for every held cycle that follows,
because a held cycle keeps the `switched` record (see [Rotate pin](#rotate-pin)).

A session that stops on a limit after the switch, on the login you switched
to, is not resumed by it: the switch is older than that limit. It waits as
usual, for a later `switch`, for a `switch --none` and a rotator cycle that
finds capacity, or for its limit to reset.

`switch` to the login that is already live writes the same record, so
running `amplihack profile switch <live>` also wakes sessions stopped before
it. The hook still needs a running rotator: with none running it exits at the
limit (`rotator-down`) and never reads the ledger.

#### Warning about an older rotator

After the success lines, `switch` checks whether a running rotator was
started from another amplihack binary, the same check `amplihack install`
and one-shot `rotate` make (see
[Capacity for a resume](#capacity-for-a-resume)). Such a rotator does not
read the [pin](#rotate-pin) and may swap away on its next cycle. `switch`
then prints one more line on stderr and still exits 0, because the login
has moved:

```
amplihack: the Claude login rotator (pid 48211) runs another amplihack binary and ignores the switch hold; restart it: amplihack profile rotate stop && amplihack profile rotate start
```

With no rotator running, or one started from this binary, nothing more is
printed.

### Rotate pin

The pin is `<root>/.auto-switch.d/rotate/pin`. While it names an existing
profile, no rotate cycle swaps: not the one-shot `rotate`, not `rotate watch`
and not the background rotator. `profile switch <name>` writes it;
`profile switch --none` removes it. Nothing else writes or removes it.

The file holds one profile name and a newline. It is written atomically
(temporary file, then rename) with mode `0600`. It is read like the other
rotate state files: never through a symlink, at most 256 bytes, and only
when it is a regular file owned by you that group and others cannot write.

| Pin file | Cycle | Log line |
|----------|-------|----------|
| Missing | Runs normally. | |
| Names an existing profile | Held: after recovery and identify, before measuring. No swap and no measurement. The one probe a held cycle makes is the spend-limit check of a pinned live overflow profile with no active block (see [Pin and overflow](#pin-and-overflow)). Recorded as `held`, unless the last cycle is a `switched` record, which is kept. | ``skip: held by switch to <name>; run `amplihack profile switch --none` to release`` |
| Names a profile that no longer exists | Runs normally; the file is left for `switch --none` to remove. | `skip: pin names missing profile <name>; ignored` |
| Not a valid name, too long, a symlink, a directory, not owned by you, writable by group or others, or unreadable | Held. Rotation fails closed rather than move a login you did not expect. Recorded as `held`, unless the last cycle is a `switched` record, which is kept. | ``skip: pin unreadable; run `amplihack profile switch --none` `` |

The pin wins over every decision in the [decision rule](#rotate-decision-rule),
including a threshold reached or an exhausted live login, and over a live
login that has changed: an ALERT about the live login is still written, but
nothing is swapped. `watch` writes the `skip: pin names missing profile`
line only when it changes.

A held cycle clears the stored ALERT key, as a cycle that stays does, so
the same ALERT is written again if it is still true after the pin is
released.

A held cycle measures nothing. When the last cycle is the `switched` record
that `profile switch` writes, a held cycle leaves it in place: it still
counts as capacity measured at the switch, so sessions stopped before the
switch are resumed (see
[Sessions stopped by a limit](#sessions-stopped-by-a-limit)) and sessions
stopped after it are not. Otherwise, for example under a pin written by
hand or by an older amplihack, the held cycle records `outcome` `held` with
no `measured_at`, and `held` is never capacity.

The pin is checked before the cycle measures or picks a target, so it wins
over every rule that picks one, including any rule that prefers some
profiles over others, such as the credit overflow proposed in
[#1694](https://github.com/rysweet/amplihack-rs/issues/1694). The pin is a
file of its own, not `.default`: `switch --none` removes both, which leaves
`.default` unset as such rules expect.

The pin's contents are never printed. A pin that cannot be read gives the
fixed `pin unreadable` text.

`switch --none` takes the state lock (waiting up to 2 seconds), even when
there is no pin, and under it checks for and removes the pin and then
`.default`. A `switch` writes both under the same lock, so the two commands
cannot interleave: whichever takes the lock second wins, and the pin and
`.default` never name different profiles. It checks the path with
`lstat` and never follows a symlink: a file or a symlink is removed (a
symlink's target is left alone), an empty directory is removed, and anything
else is refused with
`pin path <path> is not a file; remove it by hand`. If the lock is busy,
`switch --none` fails with `RotateRefused` and changes neither file.

A rotator started before the pin existed does not read it. `switch` warns
when one is running (see
[Warning about an older rotator](#warning-about-an-older-rotator)). Restart
`rotate watch` (`amplihack profile rotate stop`, then `start`) after
upgrading. The ledger format did not change, so an older rotator can still
read the ledger a `switch` writes; it reads `switched` as an outcome it does
not count as capacity.

### Rotate state directory

Everything is under `<root>/.auto-switch.d/rotate/` (directory `0700`, files
`0600`). Ledger and journal are written atomically, never through a symlink,
and are capped at 64 KiB. Both have `"version": 1`, reject unknown fields, and
omit optional fields that are unset. Every profile name read from them is
checked with the [name rules](#name-rules) before it is used. A file that
fails to parse is left unchanged and reported as `RotateInconsistent`.

| Path | Contents |
|------|----------|
| `ledger.json` | `live`, `account`, `since` (epoch seconds when `live` last changed), `swapped_at`, `previous`, `alert` (the de-duplication key, `<kind>:<live profile or ->`), `last_cycle` (`at`, when the cycle was recorded; `measured_at`, when it started measuring usage, absent for a cycle that measured nothing; `outcome`; and, for an [overflow](#rotate-overflow) cycle, `overflow`), and `overflow_blocked` (the [spend blocks](#overflow-spend-limit-block)). Optionally `guidance`, the current [hand-change](#rotate-hand-change-recovery) ALERT and `fix` lines (at most 16, each at most 1024 bytes, with no control or bidirectional-override characters), and `lost_login`, the profiles that lost their login to a manual `/login` (at most 64 distinct valid names). Both are omitted when empty, and a ledger that breaks these limits is `RotateInconsistent`. Optionally `home_unreadable_since`, the epoch seconds when a [stuck `~/.claude.json`](#stuck-claudejson) spell started. While a rotator from an older amplihack binary runs, `measured_at`, `overflow`, `overflow_blocked`, `guidance`, `lost_login` and `home_unreadable_since` are left out so that rotator can read the file. |
| `journal.json` | The swap in progress: `from`, `to`, `adopt`, `phase` (`j0`, `s1`, `s2`, `s3`, `s4` or `inconsistent`), `home`, `live_ino`, `target_ino`, `home_lock_ino` and `to_lock_ino` (each present once that refresh lock is taken), `started_at` and `pid`. |
| `rotate.pid` | The background process's pid, as ASCII decimal. |
| `gate.json` | Written by `rotate watch` when it starts: `version`, `pid`, and the [overflow](#rotate-overflow) gate it resolved from its flags (`overflow_wait`, `overflow_unknown_reset`, `session_at`, `week_at`). [Auto-switch](#auto-switch-and-overflow-profiles) uses it only while the rotator with that pid runs. If it cannot be written, the log says `overflow: gate not recorded, auto-switch will not use overflow (<error>)` and auto-switch uses no overflow profile. |
| `rotate.log` | The log. |
| `rotate.log.1` | The previous log, after rotation. |
| `rotate.err` | The background process's own stdout and stderr, and one line from each launch that [started](#rotate-autostart) a rotator or failed to. Not rotated. |
| `autostart` | The [autostart](#rotate-autostart) setting: `on` or `off` and a newline. Missing means on. |
| `pin` | The [rotation pin](#rotate-pin): one profile name and a newline. Written by `profile switch <name>`, removed by `profile switch --none`. Not part of the ledger. |

The auto-switch `ledger.json` and `journal.json` one level up are separate
files and are not changed by rotation.

[Hand-change recovery](#rotate-hand-change-recovery) also writes outside this
directory:

| Path | Contents |
|------|----------|
| `<root>/.rejected-logins/rotate-<name>-<YYYYMMDDTHHMMSSZ>[-<n>]/` | `0700`. The `.credentials.json` moved out of profile `<name>` because `~/.claude` held the same account. Never deleted by amplihack. An empty one is left by a recovery that stopped before the move. |

Example ledger, with the account value replaced:

```json
{
  "version": 1,
  "live": "spare",
  "account": "<accountUuid>",
  "since": 1760791205,
  "swapped_at": 1760791205,
  "previous": "work",
  "last_cycle": { "at": 1760791805, "measured_at": 1760791774, "outcome": "stayed" }
}
```

Overflow fields:

| Field | Values | Written |
|-------|--------|---------|
| `last_cycle.overflow` | `in` (the cycle swapped onto an overflow profile), `out` (it swapped off one onto a plan profile) or `on` (it stayed on one) | Only for those cycles; absent otherwise. |
| `overflow_blocked` | Object keyed by profile name; each value is `{"at": <epoch>, "until": <epoch>, "reason": "spend_limit"}` | Only while at least one block exists; absent otherwise. |

A ledger with no overflow activity is byte-identical to one written before
these fields existed. The rotator rejects the ledger as `RotateInconsistent`,
with the message `rotate ledger: invalid overflow field` and without echoing
the value, when a block key fails the [name rules](#name-rules), `reason` is
not `spend_limit`, `at` is not above `0`, `until` is not later than `at`,
`until` is more than 62 days after `at` (a sum that would overflow counts as
more), there are more than 256 blocks, or `last_cycle.overflow` is not `in`,
`out` or `on`. The rotator writes `until` as 00:00 UTC on the 1st of the
month after `at`, which always meets this rule; the wider window lets a
later reader keep a block whose end came from elsewhere. An unknown key
inside a block record is ignored, so a newer writer's extra block fields do
not make the ledger invalid. `last_cycle` and the top level of the ledger
still reject unknown keys.

Ledger on credits, with one block on another overflow profile:

```json
{
  "version": 1,
  "live": "ryan-mistt-org",
  "account": "<accountUuid>",
  "since": 1792832405,
  "swapped_at": 1792832405,
  "previous": "work",
  "last_cycle": { "at": 1792833005, "outcome": "stayed", "overflow": "on" },
  "overflow_blocked": {
    "team-credits": { "at": 1792826100, "until": 1793491200, "reason": "spend_limit" }
  }
}
```

An `amplihack` binary older than these fields rejects a ledger that has them,
because it rejects unknown fields everywhere in the ledger. This binary
relaxes that rule only inside a block record; `last_cycle` and the top level
stay strict, so a future field there is rejected in the same way. The
rejection fails safe: the older binary leaves the ledger unchanged, reports
`RotateInconsistent` and makes no swap, so rotation stops instead of acting
on a ledger it cannot read. A version gate that names the newer writer
instead of failing on the field is a follow-up in #1687. Upgrade every
binary that uses the root before the first `overflow add`, then restart the
rotator (`rotate stop`, `rotate start`). To go back to an older binary,
follow
[Upgrading and rolling back](../howto/claude-profile-rotation.md#upgrading-and-rolling-back).

### Rotate log

Each line is `YYYY-MM-DDTHH:MM:SSZ <message>`, in UTC. The one-shot command
writes to the log and stdout, `watch` to the log only, and `--dry-run` to
stdout only, without the time.

| Message | Meaning |
|---------|---------|
| `cycle live=<name\|-> <name>:s=<n>%,w=<n>% ...` | One per cycle: every measured profile, live first, then by name. Percentages are rounded down. An exhausted profile (`session_exhausted` or `weekly_exhausted`) shows `s=100%,w=100%`, whatever the windows it read. A profile whose probe timed out or could not run shows `<name>:failed`, never a percentage: its usage is unknown, and it is probed again on the next cycle. `spend_limit`, `not_logged_in` and `unparsed` also show `<name>:failed`. A skipped profile is not listed. A login no profile owns is shown as `-`. The API-key profile is never probed and shows as `<name>:per-token`, live or not. |
| `swap <from>-><to> live s=<n>% w=<n>% target s=<n>% w=<n>% reason=threshold\|upgrade\|failed-probe\|overflow-in\|overflow-out` | A completed swap. For an adopted login, `<from>` is the new profile's name. A failed live probe shows `live failed`. `reason=failed-probe` is a swap made after three consecutive failed live probes; see [Failed live probes](#rotate-failed-live-probes). `overflow-in` and `overflow-out` are swaps onto and off an [overflow](#rotate-overflow) profile. |
| `overflow: on <name>; no plan profile has capacity, next plan reset <time\|unknown>` | Written after an `overflow-in` swap. |
| `overflow: staying on <name>; no plan profile has capacity, next plan reset <time\|unknown>` | A cycle stayed on overflow profile `<name>` and the previous recorded cycle did not: its `last_cycle.overflow` was absent or `out`, or the ledger's `live` named another profile. So it is written once when the rotator finds itself on credits without having moved there, for example after a login was put in `~/.claude` by hand, and not again while it stays. After `overflow-in` the `overflow: on` line is written instead. |
| `overflow: off <name>; back on plan profile <name>` | Written after an `overflow-out` swap. |
| `ALERT: overflow profile <name> hit its spend limit; blocked until <time>` | A [spend block](#overflow-spend-limit-block) was recorded. Written once per block. |
| `ALERT: overflow profile <name> hit its spend limit; not blocked (too many blocks\|no end time)` | A spend-limit reading that could not be stored as a block: the ledger already holds 256 active blocks, or the cycle's time is outside the range for which the next month can be computed. The profile is still not used in that cycle; the ledger is left as it was, so it still loads. |
| `stay: live probe failed (<k>/3)` | The live probe failed for the `k`th cycle in a row; no swap yet. |
| `stay: live probe failed; held after last swap (<N>s left)` | The live probe has failed three or more times in a row, but the last swap was less than `--hold` seconds ago. |
| `ALERT: no profile has capacity live=<name> s=<n>% w=<n>%` / `ALERT: no profile has capacity live=<name> s=<n>% w=<n>% (candidates: <F> failed, <E> exhausted)` | A threshold is reached and no candidate meets the margin. The suffix is added only when `F + E > 0`: `F` candidates had a failed probe and `E` were exhausted. When every candidate failed, the probes are at fault rather than the accounts. The suffix holds no profile names, and the ALERT key stays `no-capacity`, so a change in the counts does not repeat the ALERT. `--dry-run` prints it as `would alert: no profile has capacity live=<name> s=<n>% w=<n>% (candidates: <F> failed, <E> exhausted)`, without the `ALERT: ` prefix, and with the same rule for the suffix. |
| `ALERT: no login in ~/.claude; no swap` | `~/.claude/.credentials.json` is missing or not a regular file. |
| `ALERT: the account in ~/.claude is not the one the ledger names (live=<name>); <finding>; no swap` | Hand change that [hand-change recovery](#rotate-hand-change-recovery) could not resolve. Followed by its `fix <n>: <command>` lines. Written every cycle. |
| `fix <n>: <command>` | A step of the guidance above, in order. |
| `recovered: ~/.claude holds <M>'s login (ledger named <L>); recorded <M> as live` | Hand-change recovery recorded the profile whose login is in `~/.claude`. |
| `recovered: ~/.claude and <M> held the same account; kept ~/.claude, moved <M>'s copy to <path>; recorded <M> as live` | Hand-change recovery moved `M`'s duplicate login to a backup. |
| `ALERT: profile <L> has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login <L>` | `L` is in the ledger's `lost_login`. Written every cycle. |
| `ALERT: profile <L> has no login anywhere, and the list of such profiles is full, so this is not repeated. Run: amplihack profile login <L>` | `L` lost its login but `lost_login` already holds 64 profiles. Written once. |
| `ALERT: more than one profile matches the account in ~/.claude; no swap` | Two or more login-less profiles have the account in `~/.claude.json`. |
| `ALERT: swap refused: <reason>` | A [pre-swap check](#rotate-swap) failed; written once per reason. The one-shot command also prints it and exits with `RotateRefused`. |
| `ALERT: rotate recovery is inconsistent: <reason>; no swap until it is resolved` | See [Rotate recovery](#rotate-recovery). |
| `ALERT: auto-switch recovery is inconsistent; no swap` | Auto-switch state needs `auto-switch repair`. |
| `recovery: rolled back an interrupted swap <from>-><to>` / `recovery: finished an interrupted swap <from>-><to>` | Recovery acted. |
| `skip: state lock busy` / `skip: <name> (<reason>)` / `skip: cannot see running sessions; no candidates` | The cycle or a candidate was skipped. |
| `skip: state changed during measurement (<labels>)` | Step 6 found a change that affects the decision; see [Rotate revalidation](#rotate-revalidation). `<labels>` lists one or more of `journal`, `live`, `pin`, `overflow`, `target eligibility: <reason>` and `target identity`, separated by `, `. The labels are fixed text and [skip reasons](#rotate-candidates) only. Older readers that match the prefix `skip: state changed during measurement` still match. |
| `revalidate: ~/.claude.json held no account; read again` | Step 6 found no account in `~/.claude.json`, which held one in step 3, and read the state once more after 500 milliseconds; see [Rotate revalidation](#rotate-revalidation). The next line comes from the second comparison. |
| `revalidate: target <name> no longer eligible (<reason>); reconsidered` | The swap target became ineligible, or its account changed, while the cycle measured, and the cycle decided again under the lock. `<reason>` is the target's bare [skip reason](#rotate-candidates), such as `session running`, `gone`, or `account changed`; it never begins with `target eligibility: `. The next line is the new `swap` line, or `ALERT: swap refused: <reason>` when a [pre-swap check](#rotate-swap) refuses the new target. |
| `skip: the external rotator is running` / `skip: <error>` | `watch` only: the cycle failed with that error and the rotator carries on. |
| `skip: prototype ledger ignored (<reason>)` | See [Rotate live profile](#rotate-live-profile). Written only when the reason changes. |
| `stop: signal received; cycle abandoned` | `watch` stopped during measurement or before the swap; nothing was moved. |
| `idle: auto-switch is off` | `watch` with auto-switch turned off; written once per change. |
| ``skip: held by switch to <name>; run `amplihack profile switch --none` to release`` | The [pin](#rotate-pin) holds rotation; the cycle neither measured nor swapped. |
| ``skip: pin unreadable; run `amplihack profile switch --none` `` | The pin file cannot be trusted or parsed; the cycle holds. |
| `skip: pin names missing profile <name>; ignored` | The pin names a removed profile; the cycle runs normally. |
| `switch <from>-><to>` | A swap made by [`profile switch`](#rotate-switch). It has no readings, because `switch` measures nothing, and it does not start with `swap `, so a search for rotator swaps does not match it. |
| `skip: <name> is already the live login; nothing moved` | `profile switch` found `<name>` already live. |
| `would swap ...` / `would block <name> (spend limit) until <time>` / `would adopt ~/.claude as <name>` / `would stay` / `would stay (live probe failed (1/3))` / `would stay (held by switch to <name>)` / `would stay (pin unreadable)` / `would alert: ...` / `would recover an interrupted swap (phase <phase>)` / `would alert: rotate recovery is inconsistent` / `would alert: the rotate journal is damaged` / `would recover: <recovered line>` | `--dry-run` only. |

An ALERT is written only when its key (`no-capacity`, `no-login`,
`hand-change`, `home-unreadable`, `ambiguous`, `inconsistent`, `auto-switch` or `refused <reason>`
for a swap refused under [Swap](#rotate-swap), plus the live
profile) differs from the key
stored in the ledger. The key is cleared when a cycle stays without alerting
or a swap succeeds, so the same ALERT can be written again later. The
one-shot `rotate` still prints a repeated ALERT to stdout on every run; only
the log line is written once. The hand-change guidance and lost-login ALERTs
have no key: they are written on every cycle while their condition holds.

Before each write, a log over 1 MiB is renamed to `rotate.log.1`, replacing
the old one. The log is opened with `O_APPEND`, `O_NOFOLLOW` and `O_CLOEXEC`,
mode `0600`. A log that is not a regular file, is owned by another user, or is
readable or writable by group or others is not written to; a warning goes to
stderr and its mode is not changed.

Log lines and stdout contain only profile names (with control characters
removed), percentages, times, phases, paths under `HOME` and the root, and
fixed text. They never contain
account IDs, e-mail addresses, tokens, environment values or Claude's output.

### Rotate daemon

`start`, and a launch that [autostarts](#rotate-autostart) the rotator,
launch `amplihack profile rotate watch <resolved flags> --spawned`
with the same `amplihack` binary, in a new session (`setsid`) with `SIGHUP`
ignored, stdin from `/dev/null`, and stdout and stderr appended to
`rotate.err` (`0600`) beside the log. The log itself is written only by the
cycle, so it keeps its grammar; `rotate.err` holds only what the process
prints outside a cycle, such as start-up notices or a crash.
`--spawned` is hidden; it tells `watch` that `start` has already written the
pid file. No shell is involved.

The child's environment is the parent's, with every `AMPLIHACK_ROTATE*`
variable, `CLAUDE_CONFIG_DIR` and `AMPLIHACK_CLAUDE_PROFILE` removed and
`AMPLIHACK_CLAUDE_PROFILE_ROOT` set to the absolute root `start` resolved.
Removing the profile variables means a rotator started from a
`--profile` launch still probes and swaps `~/.claude`. The child validates that value again when it starts, and
never reads `~/.amplihack/profile-root`. Changing the variable or the saved
root, or closing the shell that ran `start`, does not move a running rotator.
To move it to another root, stop it and start it again.

`AMPLIHACK_PROFILE_USAGE_PROBE`, `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` and
`AMPLIHACK_PROFILE_USAGE_PARALLEL` are kept, so the rotator probes with the
values of the shell or launch that started it until it is restarted; see
[Probe settings in the rotator](#probe-settings-in-the-rotator).

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
autostart: on (default)
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
overflow: -
on overflow: no
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

On credits, with a second overflow profile blocked:

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default)
pid: 48213
live: ryan-mistt-org
last cycle: 2026-10-24T09:10:05Z stayed overflow=on
overflow: ryan-mistt-org, team-credits (blocked until 2026-11-01T00:00:00Z)
on overflow: yes (ryan-mistt-org)
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

| Key | Value |
|-----|-------|
| `root` | Always the first line. `<path> (source: env\|config\|default)` for the resolved root, or `<path> (pinned by running rotator)` when the rotator was found through the [rotate root record](#rotate-root-record). The lines after it describe that root. |
| `rotate` | `running`, `stopped`, `stale` or `unverifiable`. |
| `autostart` | `on` or `off`, then the source in brackets: `default`, `setting`, `env` or `corrupt`. See [Rotate autostart](#rotate-autostart). Unix only. |
| `pid` | The pid, or `-`. |
| `live` | The ledger's `live` profile, or `-`. |
| `last cycle` | The time and outcome (`swapped`, `stayed`, `switched`, `alerted`, `skipped`, `idle` or `held`) of the last cycle, or `-`. `skipped` is recorded when the state changed during measurement; `switched` by [`profile switch`](#rotate-switch), and kept by the held cycles after it; `held` when the [pin](#rotate-pin) held rotation with no switch record; a cycle that could not take the lock records nothing. An [overflow](#rotate-overflow) cycle adds ` overflow=in`, ` overflow=out` or ` overflow=on`. |
| `hold` | Only while something is at the [pin](#rotate-pin) path: `hold: <name>`, `hold: <name> (profile missing; ignored)` or `hold: unreadable`. |
| `overflow` | The marked overflow profiles in list order, separated by `, `, each followed by ` (blocked until <time>)` when it has an active [spend block](#overflow-spend-limit-block); `-` when none is marked or `.auto-switch` is corrupt. When the ledger cannot be read, the list ends with ` (blocks unknown)`. |
| `on overflow` | `yes (<name>, held by switch)` when a [pin](#rotate-pin) names the live overflow profile: a `profile switch` onto it is a choice to spend, and the rotator does not move off it until `profile switch --none` (see [Pin and overflow](#pin-and-overflow)). Otherwise `yes (<name>)` when the profile identified as live in `~/.claude` by account is an overflow profile and the ledger's `last_cycle.overflow` is `in` or `on`, which means the rotator found no plan capacity in its last cycle. `no (<name> is overflow; plan capacity remains)` when the live profile is an overflow profile but the last cycle carries no such tag, for example after the login was put in `~/.claude` by hand. `no` when it is a plan profile, `-` when the live profile cannot be identified. `status` runs no probe; the ledger is its only evidence. |
| `default` | Printed only when the [default pointer](#clearing-the-default) names an overflow profile: `<name> (overflow; launches use it directly)`. |
| `log` | The log path. |
| `alert` | Zero or more. A current [hand-change](#rotate-hand-change-recovery) guidance line. |
| `warning` | Zero or more. A profile that lost its login to a manual `/login` in `~/.claude`. |

Run from a shell where the root now resolves elsewhere, for example cron with
no saved root, `status` finds the running rotator through the record:

```
root: /home/you/.claude-profiles (pinned by running rotator)
rotate: running
autostart: on (default)
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
overflow: -
on overflow: no
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

A pending or inconsistent journal adds a line `journal: pending (phase <phase>)`
or `journal: inconsistent`, and one that cannot be read `journal: damaged`. A
ledger that cannot be read shows `live: - (ledger unreadable)`.

After `last cycle:`, `status` prints one `alert: <line>` for each line of the
ledger's `guidance`, then one `warning: <line>` for each profile in its
`lost_login`. It does not compare accounts itself; it shows what the last
cycle decided. Lines are cleaned of control characters again before
printing:

```
live: ryan-ryansweet
last cycle: 2026-10-27T03:17:05Z alerted
alert: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); it matches more than one profile (mistt-spare, ryan-mistt-org); no swap
alert: fix 1: amplihack profile login ryan-ryansweet
alert: fix 2: keep this account in one of mistt-spare, ryan-mistt-org: log the others in to another account or remove them; the next cycle then recovers
```

There is no `warning:` line yet: `lost_login` is filled only when a profile
is recorded in `L`'s place. Once the extra profile is fixed and a cycle
recovers, `guidance` is empty and the lost login is shown instead:

```
live: ryan-mistt-org
last cycle: 2026-10-27T03:22:05Z stayed
warning: profile ryan-ryansweet has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login ryan-ryansweet
```

### Rotate autostart

On Unix, `amplihack claude`, `amplihack launch` and `amplihack profile run`
start the background rotator when auto-switch is on and no rotator is
running. The check runs after the launch has resolved its profile and before
Claude starts. It never fails the launch, never changes its exit status and
never writes to stdout. On other platforms it does nothing.

It checks, in order, and stops silently at the first that fails. Nothing is
created on disk before step 8.

| Step | Check | When it fails |
|------|-------|---------------|
| 1 | Auto-switch is on: `<root>/.auto-switch` says `enabled=on`, or `AMPLIHACK_CLAUDE_AUTO_SWITCH=1`. A missing, damaged or `off` setting counts as off. | No effect. |
| 2 | The root exists, is a real directory (not a symlink) owned by you, and is not writable by other users. | Skipped. Nothing else under the root is read; in particular the autostart setting is not. |
| 3 | The effective uid is not `0`. | Skipped. `rotate start` still works as root. |
| 4 | Autostart is on (see the precedence table below). | No effect. |
| 5 | Not Docker: no `--docker` flag, and Docker not activated through the environment. The environment activates it only when `AMPLIHACK_USE_DOCKER` is truthy, the process is not already inside a container, and Docker is running. `--auto` launches are not skipped. | Skipped. |
| 6 | The [daemon state](#rotate-daemon) is not `running` or `unverifiable`. | Already running, or skipped. |
| 7 | The [prototype lock](#rotate-prototype-coexistence) is free and the root is private, as `start` requires. | Skipped. |
| 8 | Creates the state directory if needed and tries `<root>/.auto-switch.d/lock` once, without waiting. | Busy: another process holds it. |
| 9 | Under the lock, the daemon state is still not `running`. | Already running. |
| 10 | The `AMPLIHACK_ROTATE_*` [settings](#rotate-settings) in the launch's environment are valid. | Failed; no rotator started. |
| 11 | Starts the [daemon](#rotate-daemon), writes `rotate.pid` and the [root record](#rotate-root-record), then releases the lock. | Failed. If the pid file cannot be written, the child is sent `SIGTERM`. |

Steps 8 to 11 run under the state lock, and `start` and other launches check
under the same lock, so at most one rotator runs per root. Unlike `start`,
autostart runs no recovery; the daemon's first cycle does.

After step 11 the launch prints one line to stderr:

```
amplihack: started Claude login rotator (pid <pid>); 'amplihack profile rotate autostart off' to disable
```

and appends `autostart: started pid <pid> exe <path>` to `rotate.err`. A
failure at step 10 or 11, or an I/O error at step 8, is appended to
`rotate.err` as one `autostart: failed: <error>` line, when that file can be
written, and is otherwise ignored. Every `rotate.err` line has the
[log](#rotate-log) time prefix and its control characters removed.

Nested launches inside a recipe run take the same steps, so they start a
rotator only when none is running. The Claude Code `SessionStart` hook does
not start the rotator.

#### Autostart setting

| `AMPLIHACK_ROTATE_AUTOSTART` | `<root>/.auto-switch.d/rotate/autostart` | Effective | Status line |
|------------------------------|------------------------------------------|-----------|-------------|
| `1` or `on` | any | on | `autostart: on (env)` |
| `0` or `off` | any | off | `autostart: off (env)` |
| unset, empty or invalid | missing | on | `autostart: on (default)` |
| unset, empty or invalid | `on` | on | `autostart: on (setting)` |
| unset, empty or invalid | `off` | off | `autostart: off (setting)` |
| unset, empty or invalid | anything else | off | `autostart: off (corrupt)` |

The variable is case-insensitive. Any other value is ignored, and a process
that reads it prints `warning: ignored AMPLIHACK_ROTATE_AUTOSTART value (use
on, off, 1 or 0); the setting decides` to stderr once, without repeating the
value.

The setting file is read without following symlinks and only up to 64 bytes.
It counts as `corrupt` when it is a symlink, is not a regular file, is not
owned by you, is longer than 64 bytes, cannot be read, or holds anything but
`on` or `off` and an optional newline. `rotate autostart on|off` replaces it
atomically with mode `0600`, creating `<root>/.auto-switch.d/rotate/` first
if needed.

When the variable is set to a valid value, `rotate autostart on|off` still
writes the file, and then warns on stderr that
`AMPLIHACK_ROTATE_AUTOSTART=<value>` overrides it in this environment.

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
| `0` | No swap, a swap, a held cycle, a dry run, an ALERT, `status`, a `stop` of a stopped or stale rotator, a successful `profile switch`. |
| `1` | `RotateConfig`, `RotatePrototypeRunning`, `RotateInconsistent`, `RotateRefused`, `RotateUnsupported`, `RotateDaemon`, or any other [error](#errors). |
| `2` | A usage error from argument parsing, such as a tuning flag before `start`. |

### Rotate session resume

`amplihack-hooks limit-resume` is a Claude Code `StopFailure` hook that
restarts a session which stopped on a usage limit, once the rotator has put a
login with capacity in `~/.claude`. `amplihack install` registers it; see
[StopFailure in the hook reference](hook-specifications.md#8-stopfailure--limit-resume).
The [how-to](../howto/claude-profile-rotation.md#resume-sessions-stopped-by-a-usage-limit)
shows how to use it. The hook only reads the rotate ledger and its own
[resume state](#resume-state-directory). It never moves a login, never reads
the session's transcript, and never opens `.credentials.json`, a token file
or anything under a profile directory.

The hook exits `2` with the resume message on stderr, or `0` with nothing on
stdout or stderr. It never writes to stdout and never exits with any other
status. A panic exits `0` with nothing printed. The resume message is exactly:

```
amplihack: the login rotator reports a login with capacity. Continue the task you were working on; if the usage limit is still in place, stop and wait.
```

Claude Code shows it to the model as a system reminder and starts a new turn.
The message does not claim the limit is gone: the rotator's reading can be
wrong, so the model is told to stop again if it is not.

The hook relies on Claude Code behaviours that a live check against Claude
Code 2.1.306 confirmed. It used a stub `StopFailure` hook with
`asyncRewake: true`, triggered by an invalid API key (matcher
`authentication_failed`):

- a turn that ends on an API error runs `StopFailure`, not `Stop`;
- the payload has `session_id`, `transcript_path`, `cwd`, `hook_event_name`,
  `error` and `last_assistant_message`, plus `prompt_id` and an `effort`
  object, which the hook ignores;
- in an interactive session, exit `2` starts a new turn with stderr shown to
  the model as a system reminder. With `claude -p` the process exits first, so
  there is no new turn;
- the payload has no `stop_hook_active`, not even on the failure of the turn
  the hook started. A repeated failure is stopped by the
  [event marker](#resume-state-directory) instead: the event key leaves out
  `prompt_id`, so the same error text in the same session gives the same key.

A second live check, on Claude Code 2.1.307 with a real weekly limit, showed
the usage-limit path itself: the stop ran `StopFailure` with
`error: "rate_limit"`, `last_assistant_message` set to the limit text
("You've hit your weekly limit · resets …") and no `stop_hook_active`; the
hook's exit `2` closed the "What do you want to do?" limit menu and started a
new turn. That turn hit the same limit and sent an identical payload, which
is why a repeated event on the same login is a `duplicate` and never resumes
again.

#### Resume decision

The checks run in this order. The first one that fails decides the outcome.

| Step | Check | When it fails |
|------|-------|---------------|
| 1 | The [profile root](#profile-root) resolves (`AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`, then `~/.amplihack/claude-profiles`) to an absolute path with no `..` component. The pointer file must not be writable by group or others, and auto-switch is on by the same rule as `auto-switch status`. The pointer file is read with no-follow and at most 4 KiB. | Exit `0`. Nothing under the root is created or written, and nothing but the toggle and the pointer file is read. Nothing is logged. |
| 2 | The stop was a usage limit (see [Limit detection](#limit-detection)), and `session_id` is present and at most 256 bytes. | Exit `0`. Nothing is logged. |
| 3 | `stop_hook_active` is absent, `null` or `false` (any other non-boolean value counts as `true`). The ledger is read once here, for the `live` profile that goes into the [event key](#resume-state-directory). | Exit `0`, outcome `stop-hook-active`. |
| 4 | No marker exists for this limit event. | Exit `0`, outcome `duplicate`, or `guard-failed` if `resume/` or one of its subdirectories is unsafe (see [Resume state directory](#resume-state-directory)). |
| 5 | This session has resumed fewer than 3 times in the last 24 hours. | Exit `0`, outcome `cap`, or `guard-failed` if `resume/` or one of its subdirectories is unsafe. |
| 6 | The rotator can record a cycle: `<root>/.auto-switch.d/rotate/rotate.pid` names a running `amplihack profile rotate watch` process of yours (checked in `/proc`; skipped where there is none), and the ledger, if present, can be read as described in [Capacity for a resume](#capacity-for-a-resume). | Exit `0` without waiting, outcome `rotator-down`. |
| 7 | `resume/` and `resume/prompted/` exist or can be created safely, so a prompt during the wait can be recorded; prompt markers older than 7 days and 1 hour are removed. Then a [fresh cycle with capacity](#capacity-for-a-resume) appears within 7 days. The ledger is read every 15 seconds, and once more at the deadline. Each read also checks that auto-switch is still on and that the session has not [moved on](#moved-on). | Exit `0`, outcome `no-capacity-timeout`; `disabled` if auto-switch was turned off while waiting; `moved-on` if you sent a prompt in that session; `guard-failed` if `resume/` cannot be prepared or the prompt marker cannot be read safely. |
| 8 | No prompt was sent in the session since the hook started (checked once more). Then, under the resume lock, steps 4 and 5 still pass, the session has no resume after this hook's start, the event marker is created and the session record is updated and synced to disk. | Exit `0`, outcome `moved-on` if a prompt was sent; `duplicate` if another hook resumed this event or this session first; `cap` if the session reached its cap; `guard-failed` if the lock is not taken within 5 seconds or any write fails. |
| 9 | — | Exit `2` with the resume message, outcome `resumed`. |

Steps 4 and 5 run once without the lock, so a duplicate or capped session
exits at once instead of waiting up to 7 days, and again under the lock in
step 8. The lock is not held while waiting in step 7. The marker and session record
are on disk before the hook exits `2`, so a resume that is cut short never
repeats.

A session is woken at most once per waiting period. When two hooks wait for
the same session with different events, the first to commit wakes it; the
other finds a resume after its own start time and ends as `duplicate`. A
new limit that starts in the same second as the previous resume may still
resume.

##### Moved on

The session moved on when you sent a prompt in that session during the wait.
amplihack's `UserPromptSubmit` hook (`amplihack-hooks user-prompt-submit`)
records the time of each prompt in `resume/prompted/<sha256 hex>`; the waiting
hook reads it at every ledger read and right before the commit in step 8, and
exits `0` with outcome `moved-on` when the time is after its own start. The
hook's own wake is not a prompt, so the woken turn does not count. Background
task reports and local commands such as `/usage` are not prompts either, so
they never end the wait.

The prompt hook writes the marker only when the profile root resolves and
`resume/` already exists as a private directory. A waiting hook creates
`resume/` before it starts waiting, so even the first wait sees a prompt, and
nothing is created for a user whose sessions never hit a limit with
auto-switch on. The prompt hook takes no lock, does not sync the marker to
disk, and ignores every error, so it never fails or holds up a prompt.

#### Limit detection

The stop is a usage limit only when `error` is exactly `rate_limit`. The
installed hook has matcher `rate_limit`, so Claude Code runs it only for that
error. Any other value, an empty or missing `error`, or `unknown` is not a
limit, whatever `last_assistant_message` or `error_details` say; message text
is never matched.

Standard input is read up to 1 MiB; a larger payload or one that is not a
JSON object exits `0` with nothing logged. A known field of an unexpected
type (for example `error_details` sent as an object) is read as absent, so it
neither stops the hook nor reaches the event key; an `error` that is not a
string is therefore not a limit.

#### Capacity for a resume

The hook reads `<root>/.auto-switch.d/rotate/ledger.json` (see
[Rotate state directory](#rotate-state-directory)) and uses only `live`,
`alert` and `last_cycle`. There is capacity when all of these hold:

- `last_cycle.measured_at` is after the time the hook started, so the cycle's
  usage probes began after the limit hit;
- `last_cycle.outcome` is `swapped`, `stayed` or `switched` (written by
  [`profile switch`](#sessions-stopped-by-a-limit), with `measured_at` the
  time of the switch);
- `alert` is not set.

`last_cycle.at` is set when the cycle ends, so a cycle that was already
measuring when the limit hit can be recorded after the hook starts with
readings taken before the limit; `measured_at` rules it out whatever the
probe timeout. A ledger written by a rotator older than
[#1659](https://github.com/rysweet/amplihack-rs/issues/1659), which has no
`measured_at`, needs `last_cycle.at` more than 60 seconds after the hook
started instead.

A rotator started before an upgrade keeps running the old binary. While it
runs, `amplihack install` and a one-shot `amplihack profile rotate` print one
line naming its pid and how to restart it, and every ledger write leaves out
`measured_at`, which that binary would reject. The rotator is matched by its
`/proc/<pid>/exe` file, so a binary replaced at the same path counts as
another. From this version on the rotator ignores ledger fields it does not
know.

Going back to an amplihack older than #1659 is not covered by that
protection. The older rotator rejects a ledger that has
`last_cycle.measured_at`, never rewrites a ledger it cannot read, and logs a
failed cycle every interval without rotating again. Nothing else reports it:
a limit-resume hook still finds a running rotator and waits its full 7 days.
Before the older binary runs, including a launch that would
[start the rotator automatically](#rotate-autostart), stop the rotator with
this version (`amplihack profile rotate stop`), then delete
`<root>/.auto-switch.d/rotate/ledger.json`, then start the older rotator.
The ledger holds only bookkeeping; the next cycle works out the live profile
again from the accounts. Deleting it also clears the swap hold and the alert
de-duplication key, so the first cycle may swap or alert once more. If an
older rotator is already running and logging failures, the same three steps,
with `rotate stop` run by either binary, recover it.

A cycle that moved onto or stayed on an [overflow](#rotate-overflow)
profile records `swapped` or `stayed` with no `alert`, so it counts as
capacity and the hook resumes the session, which then runs on credits.
`last_cycle.overflow` does not change the verdict; the hook ignores
`overflow_blocked`. A spend block with nowhere to move ends the cycle on the
no-capacity ALERT, which is not capacity.

Any outcome other than `swapped`, `stayed` or `switched` (`alerted`,
`skipped`, `idle`, `held`, or one the hook does not know), a missing ledger, or a ledger that is not
trusted means no capacity yet. The hook does not apply the
rotator's thresholds itself.

The hook reads the ledger more loosely than the rotator does: it ignores
unknown fields, but still requires `"version": 1`, at most 64 KiB, a regular
file owned by you, not a symlink and not writable by group or others. A
`last_cycle.at` or `measured_at` of 0 or more than 300 seconds in the future
is ignored. A
`live` value that fails the [name rules](#name-rules) is logged as `-`.

#### Resume state directory

Everything is under `<root>/.auto-switch.d/resume/`. Each directory is created
one level at a time with mode `0700`, and the hook refuses to use any path that
is a symlink, is not owned by you, or is writable by group or others. It never
changes the mode of a path that already exists. Files hold only hashes, counts
and epoch seconds.

| Path | Contents |
|------|----------|
| `lock` | Lock file for steps 4, 5 and 7. Waited on for at most 5 seconds. |
| `events/<sha256 hex>` | One empty marker per limit event. The name is the SHA-256 of a fixed prefix followed by `session_id`, `error`, `error_details`, `last_assistant_message` and the ledger's `live` profile when the hook started, each written with its length, so no two different payloads share a name. The same login hitting its limit again is the same event; a limit after the rotator swapped to another login is a new one. Markers older than 7 days are removed. |
| `sessions/<sha256 hex>` | One file per session, named by the SHA-256 of a different fixed prefix and its `session_id`, with one epoch-seconds line per resume in the last 24 hours. A file that cannot be parsed counts as the cap being reached and is left unchanged. |
| `prompted/<sha256 hex>` | One file per session, named like `sessions/`, holding the epoch seconds of your last prompt in it. Written by the `UserPromptSubmit` hook, replaced atomically. Removed after 7 days and 1 hour (the longest wait plus 1 hour) when a hook starts waiting or commits. A file that cannot be parsed or is not private makes a waiting hook end as `guard-failed`. |
| `resume.log` | One line per decision from step 3 on, and for auto-switch turned off while waiting. |
| `resume.log.1` | The previous log. `resume.log` is renamed to it when it passes 64 KiB. |

#### Resume log

```
2026-10-21T14:12:46Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=resumed live=spare cycle=swapped
2026-10-21T16:40:02Z resume session=3f9a1c2b7e04 event=a0c55e1f9d38 outcome=no-capacity-timeout live=work cycle=alerted
2026-10-21T16:40:09Z resume session=91be04d2aa70 event=a0c55e1f9d38 outcome=duplicate live=- cycle=-
```

`session` and `event` are the first 12 hex digits of the hashes above. `live`
and `cycle` are the ledger's `live` profile and `last_cycle.outcome` at the
decision, or `-` when the decision was made without reading it. Every
field holds only `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` and `-`; any other
character is written as `-`. The log never holds message text, error text,
account IDs or tokens. Failing to write it does not change the outcome.

| Outcome | Meaning |
|---------|---------|
| `resumed` | The hook exited `2` and the session was told to continue. |
| `no-capacity-timeout` | No fresh cycle with capacity within 7 days. |
| `moved-on` | You sent a prompt in that session during the wait, so the hook did not wake it. |
| `disabled` | Auto-switch was turned off while waiting. |
| `duplicate` | This limit event, or this session since the hook started, was already resumed. |
| `cap` | The session already resumed 3 times in 24 hours. |
| `stop-hook-active` | Claude Code reported a stop hook already running. |
| `session-gone` | The Claude Code session that started the hook exited during the wait (the hook's parent process changed). |
| `rotator-down` | When the hook started, no rotator was running, or its ledger could not be read (for example a newer version). The hook did not wait. |
| `guard-failed` | `resume/`, the lock, a marker or the session record could not be used safely. |

#### Resume limits

| Constant | Value |
|----------|-------|
| Wait for capacity | 604800 s (7 days) |
| Ledger read interval | 15 s for the first hour, then 300 s |
| Freshness margin without `measured_at` | 60 s |
| Registered hook timeout | 608400 s |
| Resumes per session | 3 per 24 hours |
| Resumes per limit event | 1 |
| Resume lock wait | 5 s |
| Event marker retention | 7 days |

The wait plus one read interval is less than the hook timeout, so the hook
always decides before Claude Code stops it.

## Usage-limit wait

When a recipe agent step fails on a usage limit and no auto-switch rerun is
left, the step's supervisor waits for the limit to reset, or for a login
with capacity to become live, and then runs the step once more. It is on by
default and does not need auto-switch or profiles. Task-oriented guidance is
in the [how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

### Gate

A launch is supervised for the wait when all of these hold:

1. no `--profile` flag;
2. Claude's arguments set `-p` or `--print` (read as Claude reads them, so
   an option's value such as `--model -p` does not count);
3. `AMPLIHACK_AUTO_SWITCH_SUPERVISED` is unset (the launch is not itself a
   supervised attempt);
4. `AMPLIHACK_NONINTERACTIVE` is exactly `1` (`amplihack recipe run` sets
   it for every step). The [settings](#usage-limit-wait-settings) are read
   only after this check;
5. the wait is enabled; and
6. the mode is not `--docker` or `--auto`. With an inherited profile those
   modes are refused earlier, so this matters only without one.

When checks 1 to 5 pass but the step cannot be supervised (check 6, or a
scratch directory failure), the launch writes `skipped reason=not_supervised`
once and runs unsupervised, as before.

With an inherited profile and auto-switch on, the
[auto-switch reruns](#usage-limit-reruns) come first and the wait follows.
The retry after the wait always runs under the inherited profile, not under
the last profile auto-switch tried, so the wait is for the **inherited
profile's** reset (see [Order of attempts](#order-of-attempts)).
Otherwise the same supervisor runs with no profile substitution: the retry
runs under the inherited profile, or with no override when none was
inherited. With no inherited profile the launch may still apply the
`.default` pointer's profile; the wait then watches that profile's login,
unless it is the rotator's live profile, whose login is in `~/.claude`.

The supervisor keeps the held-back stdout in `<root>/.auto-switch.d/rerun/`
when auto-switch is on, otherwise in `$HOME/.amplihack/rerun/`.
`$HOME/.amplihack` must be a real directory owned by the current user that
group and others cannot write (created at `0700` when missing, never
`chmod`ed), and `rerun/` must be mode `0700`.

**Stdin.** The supervisor never reads stdin. The first attempt inherits it
unchanged. A rerun gets a regular file again from the offset the step
started at (on Linux through its own open of `/proc/self/fd/0`, so no read
offset is shared with other processes), a terminal unchanged, and anything
else (a pipe, a socket, a closed descriptor) `/dev/null`: a pipe's contents
went to the first attempt. Recipe agent steps get their prompt as `-p`
argv, so this changes nothing for them, and a step that inherited a pipe
the recipe run never closes no longer hangs. When stdin is a pipe and
Claude's arguments carry no prompt, the prompt is on the pipe, so the step
is neither rerun by auto-switch (`auto-switch: no rerun under profile …`)
nor waited for (`skipped reason=piped_prompt`). The prompt is any
positional argument, wherever it is, read with Claude's option table so
that the values of options such as `--model`, `--output-format`,
`--max-turns`, `--allowedTools` and `--append-system-prompt` are not taken
for it; a prompt that starts with `-` counts only after `--`. The table
includes Claude's hidden value-taking options (such as
`--max-thinking-tokens`), which `claude --help` does not list. An option
in no table, such as one a newer Claude added, is read as taking the next
non-option word as its value. That errs toward "no prompt in the
arguments", which with a piped stdin only means the step runs once as it
did before the wait existed. The other way round would be worse: a wait of
hours, then a rerun with no prompt.

### Order of attempts

After an attempt hits a usage limit, in this order:

1. a stop signal already received ends the step with `128` plus the signal;
2. auto-switch on, the wait unused, and an eligible profile left: rerun
   under that profile at once;
3. auto-switch moved the step off the inherited profile before the first
   attempt only because that profile is over the threshold (no window at
   100%), and no eligible profile is left: rerun under the inherited
   profile at once. This is an auto-switch rerun, made at most once per
   step: no `start` or `end` is written and the wait stays unused, so a
   limit on that rerun still goes through steps 4 to 7. With a piped
   prompt this rerun is refused (`auto-switch: no rerun under profile …`)
   and the step goes on to step 4: the log gets `skipped reason=disabled`
   (or `ci`) from step 4 when the wait is off, else
   `skipped reason=piped_prompt` from step 6;
4. the wait disabled: `skipped reason=disabled` (or `ci`), and the result
   is passed up;
5. the wait already used: `skipped reason=already_waited`, and the result
   is passed up;
6. the limit is not one a wait can outlast (see [Reset time](#reset-time)):
   `skipped reason=<why>`, and the result is passed up;
7. otherwise `start`, the wait, and `end`. After `end reason=signal` the
   step exits with `128` plus the signal; after `step timeout` or
   `session ended` the failed attempt's result is passed up; after any
   other reason `resume` is written and the step runs once more.

Steps 6 and 7 read the inherited profile's limit, not the last attempt's.
The retry runs under the inherited profile, so only that profile's reset
says when it can work again. When auto-switch ran the step under other
profiles before running out, their limits only steer auto-switch's choice
of the next profile: a candidate that resets later does not stretch the
wait, and a candidate's weekly limit or spend limit does not cancel it.
When auto-switch moved the step off the inherited profile before the first
attempt, the inherited profile's limit comes from the usage readings: the
latest reset among its windows at 100%. A window over the auto-switch
threshold but under 100% is a reason to prefer another profile, not a
limit, so it neither times nor cancels the wait. If no window is at 100%,
the inherited profile still has capacity and no wait is built: when the
candidates are limited, the step goes back to the inherited profile through
step 3, with no `start` or `end` event, and the wait is kept for a limit on
that profile itself. A candidate's limit never times it. If that move is
refused (a piped prompt), step 4 or 6 still writes the `skipped` event.

There is at most one wait per step. The retry runs the step **from the
start** with the same prompt, so anything the failed attempt already did (a
commit, a push, a comment) may happen again. Write agent steps so that
running one twice is harmless.

### Reset time

The wait reads only the harness's own limit message: the last stderr line
in its shape, else the last non-empty line of the stdout tail when that
whole line has it. In `-p` mode stdout is the model's answer, so prose that
mentions a usage limit never starts a wait (the recipe runner's classifier
still sees the limit and ends the run). For Claude the shape is a line that
starts with `Claude AI usage limit reached|` and Unix seconds, or with
`You've hit your ` and also contains `limit` and `·`. The patterns live in
`crates/amplihack-cli/src/commands/recipe/run/limit_patterns.rs`.

| Message | Example | Result |
|---------|---------|--------|
| Session limit | `You've hit your session limit · resets 1pm (America/Los_Angeles)` | wait, when the reset is at most five hours away |
| Weekly limit | `You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)` | `skipped reason=reset_too_far` |
| Spend limit with a session reset | `You've hit your org's monthly spend limit · run /usage-credits … · your session limit resets 1pm (America/Los_Angeles)` | wait for that reset: the session limit is what stops work |
| Spend limit with a weekly reset | `… monthly spend limit · … · your weekly limit resets Oct 8, 12am (America/Los_Angeles)` | `skipped reason=reset_too_far` |
| Spend limit, no reset | `You've hit your org's monthly spend limit · run /usage-credits …` | `skipped reason=spend_limit`: no reset lifts it |
| Epoch | `Claude AI usage limit reached\|1792335000` | wait, when at most five hours away |
| Shape, no readable reset | `You've hit your session limit · try again later` | `skipped reason=no_reset` |
| No message in the shape | model prose, `credit balance is too low` | `skipped reason=no_limit_message`, or not a usage limit at all |

The wait never records an overflow block. Only the rotator's own
`claude -p /usage` probe does; see
[Overflow spend-limit block](#overflow-spend-limit-block).

The text after `resets ` is an optional date (`Mon D, `, three-letter
English month), a clock time (`H[:MM]am`/`pm` or 24-hour `HH:MM`), one space
and an IANA zone in parentheses, looked up in the compiled-in `chrono-tz`
table. No year, day or zone is guessed. Without a date the time is placed
today in that zone, with one in this year; more than 10 minutes in the past
means tomorrow, or next year. A spring-forward gap moves to the first valid
instant, a fall-back overlap takes the earlier one. A reset more than five
hours away, or none that parses, is not waited for: waiting would hold the
run for hours with nothing known to change.

### Deadline

The wait ends at the earliest candidate that applies (a tie is named by the
earlier row):

| Candidate | Applies when | `bound=` |
|-----------|--------------|----------|
| reset + 120 s, or now when the reset has passed | always | `reset+margin` |
| now + `max_wait_secs` | a maximum is set | `max_wait` |
| step start + `AMPLIHACK_STEP_TIMEOUT` − 60 s, never before now | the step timeout is set and above 0 | `step_timeout` |

No input can make a wait longer than five hours and 120 seconds.
`max_wait_secs = 0` does not wait but still retries once.

### Ending the wait early

The wait sleeps in slices of at most one second and stops at the first of:

| `end reason=` | Cause | Retry |
|---------------|-------|-------|
| `signal` | `SIGINT`, `SIGTERM` or `SIGHUP` was received | no; exits `128` + signal |
| `session ended` | the process that started the step exited (the supervisor's parent changed) | no |
| `reset reached` | the `reset+margin` deadline passed | yes |
| `max reached` | the `max_wait` deadline passed | yes |
| `step timeout` | the `step_timeout` deadline passed; a fresh attempt would be killed part way | no |
| `login changed` | two probes in a row, 30 s apart, agree on a fingerprint that differs from the baseline | yes |
| `rotator capacity` | the step uses `$HOME/.claude` and the [rotate ledger](#rotate-state-directory) records a cycle with capacity (`swapped`, `stayed` or `switched`, no alert) whose `measured_at` is after the wait began, or, from a rotator that writes no `measured_at`, recorded more than 60 s after it | yes |

`rotator capacity` uses the rule the [limit-resume hook](#capacity-for-a-resume)
uses (`CycleStatus::fresh_capacity` in `amplihack_utils::rotator_status`), so
the two never disagree. It is checked with the fingerprint every 30 s. A
cycle that moved onto a [credit-backed overflow profile](#rotate-overflow)
(`swapped`, `overflow=in`) or stayed on one (`stayed`, `overflow=on`) counts
as capacity, so the step runs again on credits.

The **login fingerprint** watches the directory the retry will use:
`<root>/<inherited profile>`, else `$CLAUDE_CONFIG_DIR`, else
`$HOME/.claude`. It has three parts:

- the ledger account: for a profile, the account the auto-switch ledger
  records for it; for `$HOME/.claude`, the rotate ledger's `live` profile;
- the account in the `.claude.json` Claude reads for that directory, as
  [Account identity](#account-identity) defines it;
- whether `.credentials.json` exists (`lstat` only; it is never opened, so
  an OAuth token refresh is not a change).

A file that is missing, a symlink, not owned by the current user, writable
by others, too large or unparseable counts as absent, with no warning. One
differing probe followed by a matching one is ignored, so a torn read while
another session rewrites `.claude.json` does not end the wait. The
fingerprint is never printed, logged or written.

### Log lines

Each line goes to stderr and starts with `amplihack.recipe.usage_limit_wait`,
then the event and `key=value` fields. Times are local,
`%Y-%m-%d %H:%M:%S %:z`. New fields are only ever added at the end.

| Event | Fields |
|-------|--------|
| `start` | `reason=usage_limit harness=claude reset=<time> until=<time> bound=<bound>` |
| `progress` | `remaining=<secs>s`, every 60 s |
| `end` | `reason=<end reason>` from the table above |
| `resume` | `attempt=<n>`, counting every attempt in the step |
| `skipped` | `reason=disabled\|ci\|already_waited\|not_supervised\|spend_limit\|reset_too_far\|no_reset\|no_limit_message\|piped_prompt` |

```
amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin
amplihack.recipe.usage_limit_wait progress remaining=1740s
amplihack.recipe.usage_limit_wait end reason=login changed
amplihack.recipe.usage_limit_wait resume attempt=2
```

No line contains credentials, account names, config content or text copied
from Claude's output.

### Usage-limit wait settings

| Setting | `config.toml` key | Environment variable | Default |
|---------|-------------------|----------------------|---------|
| Enabled | `recipe.usage_limit_wait` | `AMPLIHACK_USAGE_LIMIT_WAIT` | `true`; `false` in CI |
| Maximum wait, seconds | `recipe.usage_limit_max_wait_secs` | `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | none |

Both are ordinary [user-config](./user-config.md#recipeusage_limit_wait)
keys: set them with `amplihack config set recipe.usage_limit_wait false`, in
the user or project `config.toml`, or with the variable, which outranks the
files. The variable takes `true`, `false`, `1` or `0`; an invalid value is
reported by the config loader and ignored. A project file may only lower
the maximum. With no setting in the environment or a file, the wait is off
in CI, meaning any `CI` that is not, after trimming, empty, `0` or `false`
in any case: a CI job has no
second login to wait for, and its own timeout bounds it better. A limit
then writes `skipped reason=ci` where a supervisor runs. Setting
`recipe.usage_limit_wait = true` in the user or project `config.toml`, or
the variable, turns it back on in CI. Settings are read only when
`AMPLIHACK_NONINTERACTIVE` is exactly `1`.

### Limit-resume hook

The [limit-resume hook](#rotate-session-resume) wakes an interactive Claude
session stopped by a limit (`StopFailure`, with `asyncRewake`). A recipe
step's `claude -p` instead exits on the limit, and this wait reruns it. The
hook's session checks mean it never reruns a `-p` session that has already
exited, so the two never both act on one limit.

### Test seams

In debug builds only, `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW` (Unix seconds)
sets the wait's start time and `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE` (1 to
1000000) divides every sleep. Signals and the session check still run at
least once per real second. Release builds ignore both.

### Platform

The wait is Unix-only. Elsewhere the settings are never read and a usage
limit ends the run as before.

## Errors

Errors print one line to stderr as `error: <message>` plus `hint: <next step>`
and exit with status 1. Messages never include file contents.

`profile usage` reports a profile it cannot read as a
[state](#states) on that profile's row, not as an error; only its setup
failures use this format.

| Condition | Hint |
|-----------|------|
| Invalid name | Lists the allowed pattern |
| Profile exists / not found (including `overflow add` on a name with no profile directory) | `amplihack profile list` |
| Symlinked or out-of-root profile | Remove the symlink and re-add |
| Pointer dangling (names a missing profile) | `amplihack profile switch <name>` |
| Pointer corrupt or oversized | Re-run `profile switch <name>` |
| `remove` without `--yes` | Re-run with `--yes` |
| `login` produced a duplicate login | `sign out of claude.ai in your browser (or use a private/incognito window), then run 'amplihack profile login <name>' again.` See [Login check](#login-check). |
| `login` found a new or previous login that is not a private regular file, or a `.rejected-logins` entry that already exists | Nothing was moved; follow the paths in the message. See [Login check](#login-check). |
| `login` named the rotator's live profile | `log in with plain 'claude', or stop the rotator first.` See [Login check](#login-check). |
| `--profile` with `--docker`/`--auto` | Launch without the flag combination |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` with `--docker`, `--auto` or env-activated Docker | `unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto` |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` is an invalid name or not found | Unset `AMPLIHACK_CLAUDE_PROFILE`, or create the profile with `amplihack profile add <name>` |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` names a profile that exists but cannot be used | The hint for the underlying cause, e.g. `check permissions on the profile root` for an I/O error or the `chmod go-rwx` hint for unsafe permissions |
| Recipe context key in the `AMPLIHACK_` namespace (`recipe run -c amplihack_…=…`) | Remove the key; the namespace is reserved |
| `InvalidRoot`: a non-empty `AMPLIHACK_CLAUDE_PROFILE_ROOT`, or the `profile root` argument, is empty, relative, not UTF-8, holds control characters or has a `..` component; or `HOME` is unset | `use an absolute path: set AMPLIHACK_CLAUDE_PROFILE_ROOT to one, or run: amplihack profile root <absolute dir>` |
| `InvalidSavedRoot`: `~/.amplihack/profile-root` is invalid (the message names the file and the [reason](#saved-root-file-format)), or `profile root <dir>` cannot write it safely | `fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>` |
| `Leased`: the profile's login is live in another profile's session | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| `HostsForeignAccount`: the profile's directory holds another account's login | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| `AutoSwitchConfig`: `.auto-switch` is a symlink or cannot be written, is corrupt when `threshold`, `exclude`, `overflow add` or `overflow remove` needs to keep its other keys, the change would exceed 4096 bytes, or the [settings lock](#settings-file) stayed busy for 10 seconds (the message ends in `retry`; run the command again) | `` remove <root>/.auto-switch and run `amplihack profile auto-switch on` or `off` `` |
| `AutoSwitchUnsupported`: an `auto-switch` command on a platform without Unix file semantics | `auto-switch needs Linux or another Unix; leave it off` |
| `AutoSwitchInvariant`: the state files are damaged or a login is not where the state says | `see the Recovery section of docs/howto/claude-profiles.md` |
| `AutoSwitchInstall`: a profile's `settings.json` is a symlink, owned by someone else, not a JSON object, or cannot be written | `` fix or remove the reported file, then run `amplihack profile auto-switch repair` `` |
| `RotateConfig`: a rotate flag or `AMPLIHACK_ROTATE_*` value is not a whole number in range | `` run `amplihack profile rotate --help` for the accepted values `` |
| `RotatePrototypeRunning`: the external rotator holds `<root>/.rotator/lock` | `stop the external rotator (tmux session claude-rotator) first` |
| `RotateRefused`: a check before the swap failed, or the atomic exchange failed; nothing was moved. Also every [`profile switch`](#rotate-switch) refusal (no stored login, lock busy, recovery inconsistent, live login unusable) and a `switch --none` that found the lock busy | `nothing was moved; see "Swap refused" in docs/howto/claude-profile-rotation.md` |
| `profile switch` moved the live login, then could not write the pin or the default pointer. Not a `RotateRefused`: the login did move | The message itself says to run `amplihack profile switch <name>` again; see [Rotate switch](#rotate-switch) |
| `RotateInconsistent`: the rotate ledger or journal is damaged, or recovery cannot tell which way is safe | `see the Recovery section of docs/howto/claude-profile-rotation.md` |
| `RotateUnsupported`: `profile rotate` on a platform without Unix file semantics | `rotate needs Linux or another Unix` |
| `RotateDaemon`: `start` found a rotator running, `stop` timed out, the process cannot be verified, or `overflow clear` could not take the state lock within 10 seconds (`rotator busy, retry`) | `` run `amplihack profile rotate status` `` |

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

Autostart reads only the auto-switch setting, its own setting file and
`/proc`; it never opens a login file. It prints only the pid, and its status
line only `on`, `off` and the source. It refuses a root writable by others
and runs nothing as root, so another user cannot use the setting or the pid
file to start a process for you. The rotator it starts keeps the
`AMPLIHACK_ROTATE_*` values and the binary of the launch that started it;
`rotate.err` records which binary that was.

The [usage probe](#how-a-profile-is-probed) never opens, reads, logs or
prints a login file or token, and amplihack writes nothing to the profile
directory for it; `claude` may update its own files there, as any `claude`
run does. Both attempts start `claude` through these command builders:
`build_profile_command`, which
`profile login` also uses; `build_default_dir_command` for a profile whose
login is [live in `~/.claude`](#logins-in-claude); and `build_home_command`
for a login in `~/.claude` that no profile owns. So the profile directory
check and the `CLAUDE_CONFIG_DIR` rules apply to both attempts.
Its arguments and the one request line it writes are fixed text, with no
value from the environment, a profile or a state file in them, and no
permission or tool flags. The answer is matched strictly: `type`,
`response.subtype` and `response.request_id` must be exact,
`rate_limits_available` and `is_enabled` must be JSON booleans, an answer
whose `extra_usage` is anything other than absent, `null` or
`{"is_enabled": false}` is not accepted, and a missing or
non-numeric `utilization` is no value rather than `0`. A `display_name` or
`resets_at` string longer than 64 characters, or with a control character
or a character in U+202A to U+202E or U+2066 to U+2069 in it, is dropped,
so it reaches neither the table nor the JSON. The answer is parsed only
from the 64 KiB of stdout read, with `serde_json`'s nesting limit. A hook or plugin that writes to `claude`'s
stdout could print a forged answer before `claude`'s own; the first match
decides, so that probe's figures would be wrong, as forged `/usage` text
would make them. `claude` itself still runs the project's hooks, settings and
MCP servers for the working directory amplihack was started from, as any
`claude -p` does. With `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` or
`CLAUDE_CODE_OAUTH_TOKEN` set, whichever attempt answered, the usage is not
recorded; see [Refreshing auto-switch readings](#refreshing-auto-switch-readings).

Overflow spends real money, so every uncertain case fails towards not
spending: a corrupt `.auto-switch` gives no overflow profiles, an unreadable
rotate ledger stops the cycle, a failed live probe never moves onto credits,
a plan candidate whose probe timed out or could not run blocks the move onto
credits (a profile whose directory is refused does so on every cycle, so fix
or remove it), a reset more than 8 days away counts as unknown, an answer to
the usage request that reports credits on, or reports them in a form
amplihack does not recognise, is not accepted,
profiles marked as overflow are probed with `/usage` only so a spend limit
is always seen, an overflow profile whose probe failed is not used, `exclude` beats
`overflow`, an unknown plan reset never moves onto overflow without
`--overflow-unknown-reset 1`, and auto-switch uses an overflow profile only
under the running rotator's own gate (none when that rotator has not
recorded one), with spend blocks unknown meaning none. The rotator is
not a spending cap: the organization spend limit at
<https://claude.ai/settings/usage> is the only hard limit, and a session
resumed by the [hook](#capacity-for-a-resume) while on credits spends them.
Spend-block records are checked when the ledger is loaded, so a hand-edited
block cannot last more than 62 days after its `at`. A group- or
other-writable `.auto-switch` is corrupt and gives no overflow profiles, and
every change to it goes through one locked writer that fails rather than
write unlocked, so a `remove` that reports success is never silently undone. Overflow output holds
profile names, times and the fixed usage URL; Claude's output and account
e-mail addresses are never stored or printed.

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

## Identity and duplicate logins

How amplihack decides that two logins are the same account, how
`profile login` refuses a duplicate, how a launch of the rotator's live
profile is redirected, and how the default is cleared. The walkthrough is
in [Accounts, organizations and duplicate logins](../howto/claude-profiles.md#accounts-organizations-and-duplicate-logins).

### Account identity

An account is read from the `oauthAccount` object of a `.claude.json`:
`accountUuid` (the user), `organizationUuid` (the organization) and
`emailAddress`. A value that is not a string, is over 320 bytes or contains
control characters counts as missing. Only these fields are read.

| Left | Right | Same account |
|------|-------|--------------|
| uuid `u`, org `o1` | uuid `u`, org `o1` | yes |
| uuid `u`, org `o1` | uuid `u`, org `o2` | no |
| uuid `u`, org `o1` | uuid `u`, no org | no |
| uuid `u`, no org | uuid `u`, no org | yes |
| different uuids | (any) | no |
| uuid on one side only | (any) | no |
| no uuid on either side | — | the e-mail addresses match, ignoring case |

Every account-to-account comparison uses this rule: the `login` check, the
[duplicate-login warning](#duplicate-login-warning), the
[rotate live profile](#rotate-live-profile) match, the
[hand-change recovery](#rotate-hand-change-recovery) holder check, auto-switch's account
checks and the rotator's candidate checks. Two profiles with the same uuid
and different organizations are two accounts, so they do not make the live
profile `ambiguous`.

The rotate ledger and auto-switch state files are unchanged. They still
record the user only, and checks against them (the rotate ledger match,
swap verification and auto-switch's observed-account check) still compare
the user only. Where a full account is needed, the organization is read
from the profile's `.claude.json`.

### Login check

`amplihack profile login <name>` runs in three steps. The usual
[lease](#leases) refusals apply first. A login to the rotator's
[live profile](#logins-in-claude) is refused before step 1, with nothing
moved, because its login is `~/.claude` and the rotator refuses to swap back
into a profile that has a login of its own:

```
error: profile '<name>' is the rotator's live profile; its login is ~/.claude. Nothing was moved.
hint: log in with plain 'claude', or stop the rotator first.
```

1. **Before Claude starts.** If `<root>/<name>/.credentials.json` exists, it
   is renamed into a new directory
   `<root>/.rejected-logins/.pending-<name>-<time>/`, and the profile's
   current `oauthAccount` block is written there as `oauthAccount.json`.
   The path is printed first:
   `note: holding the previous login of profile '<name>' in <path> while you log in`.
   A previous login that is a symlink, has more than one link or is not
   private stops the command here, before Claude starts, with nothing moved.
2. **Claude runs.** amplihack ignores SIGINT while it waits, so Ctrl-C ends
   Claude and the restore in step 3 still runs. No lock is held.
3. **After Claude exits.** amplihack takes the state lock again and checks
   leases again; if the profile is now leased or hosting another account,
   or the rotator made it the live profile while Claude ran (the rotator
   takes no lease), it stops, moves nothing, and prints where each login
   is. If cleanup fails after Claude itself failed, the message starts with
   `claude exited with an error first: ` and Claude's error. Otherwise:

| Outcome | What happens | Exit |
|---------|--------------|------|
| Claude exited non-zero | Any new `.credentials.json` goes to `<name>-<time>/`. The previous login and `oauthAccount` are restored. No duplicate check. | Claude's status |
| Claude exited 0, no new `.credentials.json`, a previous login was held | The login did not finish. The previous login and `oauthAccount` are restored: `Claude exited without writing a login file; the previous login of profile '<name>' was restored`. | 1 |
| Claude exited 0, no `.credentials.json` before or after | The macOS Keychain case. `oauthAccount` is left as Claude wrote it, even for a duplicate: restoring the old block would make `.claude.json` disagree with the login in the Keychain. If `.claude.json` names a duplicate account, the refusal is printed but there is no file to move. | 0, or 1 for a duplicate |
| New login is not a private regular file | Nothing is moved and its mode is not changed. The error reports the mode and says the login may be exposed; the previous login stays in the `.pending-…` directory, and the hint prints that path. | 1 |
| New login duplicates another login | Refused; see below. | 1 |
| The profiles cannot be listed, or the auto-switch state cannot be opened | The check fails closed: the new login is set aside and the previous one restored as for a refusal, with `could not check profile '<name>' for duplicate logins: <reason>`. | 1 |
| New login is accepted | The previous login, if any, moves to `<name>-<time>-replaced/` and its path is printed. The empty `.pending-…` directory is removed. | 0 |

The new login is compared with every other profile (its own `.claude.json`,
or its saved block while it hosts another account) and with
`$HOME/.claude.json`. The rotator's live profile is never compared with
`~/.claude`, because it is refused before Claude starts.

A refusal:

1. prints `note: moving the new login of profile '<name>' to <path>`;
2. renames the new `.credentials.json` into `<root>/.rejected-logins/<name>-<time>/`
   and writes its `oauthAccount` block there as `oauthAccount.json`;
3. renames the previous login back, if there was one;
4. sets only the `oauthAccount` key of `<root>/<name>/.claude.json` back to
   the saved block, or removes the key if there was none, keeping the file's
   mode. Other keys stay as Claude left them. `.claude.json` is never
   created. If a block has to go back but the file is missing, unparsable
   or not a JSON object, amplihack stops and keeps the `.pending-…`
   directory, whose `oauthAccount.json` is then the only copy of the block;
5. removes the empty `.pending-…` directory and prints:

```
error: profile '<name>' was signed in to the same account and organization as <holder>. The new login was moved to <path>; '<name>' is back to its previous login.
hint: sign out of claude.ai in your browser (or use a private/incognito window), then run 'amplihack profile login <name>' again.
```

`<holder>` is `profile '<other>'`, `~/.claude (rotator live profile '<live>')`
when the live profile is known, or `~/.claude`.

If a step fails part-way, amplihack stops and prints the path of every
login it has moved. It never copies or deletes one.

`<time>` is UTC as `YYYYMMDDTHHMMSSZ`. `.rejected-logins/` is created `0700`
under a profile root that is not a symlink; each entry directory is created
new (an existing one is an error, nothing moves) and is `0700`; files are
`0600`. amplihack never deletes anything under `.rejected-logins/` except
its own empty `.pending-…` directory and that directory's
`oauthAccount.json`. Logins there are not listed by `profile list` and are
never rotation or auto-switch candidates.

The check never opens, reads, copies, changes the mode of or deletes a
`.credentials.json`; it only checks its metadata without following symlinks
and renames it with `renameat2(RENAME_NOREPLACE)`, syncing both directories.
Messages contain profile names and paths only, never uuids, organization IDs
or e-mail addresses. On platforms without Unix file semantics, `login` runs
Claude as before, with no check.

### Duplicate-login warning

```
profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login <b>' with the browser signed out of claude.ai or in a private window.
```

`auto-switch status` prints it as a [status note](#status-notes) on stdout.
`profile usage` prints it on stderr prefixed with `warning: `, after its
other warnings, and it does not change the table, the JSON or the exit
status. Both cover every profile under the root, whatever names `usage` was
given.

One line is printed per pair. Pairs of profiles follow `profile list` order;
a pair with the login in `~/.claude` comes last, with `'~/.claude'` as
`<a>` and the profile as `<b>`, so the fix always names a profile. The
rotator's live profile is never paired with `~/.claude`. Names are cleaned
of control characters.

### Live profile launch

When `amplihack claude` or `amplihack launch` resolves a profile (from
`--profile`, an inherited `AMPLIHACK_CLAUDE_PROFILE`, or the default
pointer) and that profile is the one [rotate live profile](#rotate-live-profile)
detection names, the launch runs with no profile: `CLAUDE_CONFIG_DIR` and
`AMPLIHACK_CLAUDE_PROFILE` are not set for the child, so Claude uses
`~/.claude`. One line goes to stderr:

```
note: profile '<name>' is the rotator's live login, which is in ~/.claude; launching against ~/.claude.
```

| Case | Result |
|------|--------|
| The resolved profile is the live profile | Redirected, one note. The pre-spawn check, the auto-switch launch checks and the launch hold do not run, as for a launch with no profile. |
| Detection finds no live profile, is ambiguous, or fails | Not redirected, no note. |
| Windows or another platform without Unix file semantics | Not redirected, no note. |
| `--docker`, `--auto`, env-activated Docker | Unchanged; these are handled before this check. |
| `profile run <name>` | Not redirected. It uses `<root>/<name>/`. |

A plain `amplihack claude` with an ambient `CLAUDE_CONFIG_DIR` you exported
yourself and no profile selected is unchanged.

### Clearing the default

```
amplihack profile switch --none
```

| State before | Output | Exit |
|--------------|--------|------|
| `.default` exists (valid, corrupt or dangling) | `Default profile cleared.` | 0 |
| `.default` missing | `No default profile was set.` | 0 |
| a name and `--none` together, or neither | clap usage error | 2 |
| a [rotation pin](#rotate-pin) was removed too | the line above, then `Rotation released; the rotator picks the best login on its next cycle.` | 0 |
| the rotate state lock stays busy for 2 seconds | `RotateRefused`; neither `.default` nor the pin changes | 1 |

`--none` removes the [rotation pin](#rotate-pin) and then `<root>/.default`,
both under the rotate state lock, which it takes even when there is no pin. It moves no login: the login in `~/.claude`
stays where it is until the rotator's next cycle decides otherwise. It does
not need the named profile to exist. On a platform without Unix file
semantics there is no pin, and `--none` removes only `.default`.

```
$ amplihack profile switch --none
Default profile cleared.
Rotation released; the rotator picks the best login on its next cycle.
```

There is no `switch default` alias: `default` is a
valid [profile name](#name-rules), so `switch default` selects the profile
called `default`.

## API-key profiles

An *API-key profile* runs Claude Code on the Claude Platform API, which is
billed per token, instead of on a claude.ai subscription login, which is
billed by plan. Every other profile is a *subscription profile*. The code
calls the API-key kind `platform`; every command prints it as `api-key`. The
how-tos explain when to use one:
[Use an API key as the last resort](../howto/claude-profile-rotation.md#use-an-api-key-as-the-last-resort)
and [API-key profiles and usage limits](../howto/claude-profiles.md#api-key-profiles-and-usage-limits).

### Where the platform credential lives

An API-key profile's login file, `<root>/<name>/.credentials.json`, holds
`{}` or another JSON object with no `claudeAiOauth` key. With no OAuth login
in its config directory, Claude Code falls back to the Claude Platform
credential stored under the real home directory, for example in
`~/.config/anthropic/`. That credential is per home directory, not per
profile, so:

- every API-key profile on a machine uses the same Platform account;
- moving the profile's `{}` login file into `~/.claude` makes plain `claude`
  use that account, which is how the [rotator](#api-key-rotation) makes an
  API-key profile live;
- any config directory with no OAuth login falls back to the same paid
  credential, which is why launches of a subscription profile with no login
  are [refused](#no-silent-billing).

`amplihack` never opens, reads, lists, copies or names a file under
`~/.config/anthropic/`. It does not create, change or test API keys, never
runs `apiKeyHelper`, and never prints, logs or stores any part of a key.

### Kind detection

The kind is worked out again on every read and never stored. Only key names
and file presence are used; no value is read. All inspection of
`.credentials.json`, `settings.json` and `.claude.json` happens in
`rotate/identify.rs`.

The first rule that matches wins. `kind list` prints the source shown:

| Order | Evidence | Kind | Source shown |
|-------|----------|------|--------------|
| 1 | A `kind=` entry for the profile in `<root>/.auto-switch` | as set | `override` |
| 2 | The login file is a readable JSON object with a `claudeAiOauth` key | `subscription` | `detected: oauth login` |
| 2 | The login file is a readable JSON object that is `{}` or has no `claudeAiOauth` key | `api-key` | `detected: login file has no oauth login` |
| 2 | The login file exists but is [unreadable](#unreadable-login-file) | `subscription`, with no login | `detected: login file unreadable` |
| 3 | The login file is missing, the rotate ledger names the profile as `live`, and `~/.claude/.credentials.json` has a `claudeAiOauth` key | `subscription` | `detected: live in ~/.claude (oauth login)` |
| 3 | As above, and `~/.claude/.credentials.json` is a readable object with no `claudeAiOauth` key | `api-key` | `detected: live in ~/.claude (no oauth login)` |
| 3 | As above, and `~/.claude/.credentials.json` is missing or unreadable | `subscription`, with no login | `detected: live in ~/.claude (login file unreadable)` |
| 4 | The login file is missing and the profile names an API-key setting | `api-key` | `detected: api key setting` |
| 5 | None of the above | `subscription`, with no login | `detected: no login` |

Only a readable `{}` file, or an object without `claudeAiOauth`, is evidence
of the platform credential. A missing login file is not: on its own it
resolves to `detected: no login`, a subscription profile with no login,
which `run`, `launch` and `switch` [refuse](#no-silent-billing). A damaged or
unreadable file is not either. Every doubtful case therefore refuses a launch
instead of billing per token.

An *API-key setting* is any of these names, checked for presence only:

- `ANTHROPIC_API_KEY` under `env` in `settings.json`;
- `apiKeyHelper` in `settings.json`;
- `primaryApiKey` in `.claude.json`.

**Key settings count only when the profile has no login file** (rule 4).
They give one source, `detected: api key setting`, however many of them are
present. A profile whose login file has a `claudeAiOauth` key stays a
subscription profile whatever settings it names; `list` and `show` then
print one warning on stderr, because Claude may prefer the key:

```
warning: profile ryan-mistt has an oauth login and an API-key setting is also configured; Claude may bill per token. Set the kind to confirm: amplihack profile kind set ryan-mistt subscription
```

An override silences the warning. Nothing is refused.

What counts as evidence:

- <a id="unreadable-login-file"></a>A login file is *unreadable* when it
  exists but cannot be read, is a symlink, is not a regular file owned by
  you, is over 64 KiB, is not valid JSON, or is valid JSON that is not an
  object.
- A `settings.json` or `.claude.json` that fails the same checks, or is
  writable by group or others, counts as naming no setting.
- The `oauthAccount` block in `.claude.json` plays no part.
- Rule 3 reads `~/.claude/.credentials.json` only when the rotate ledger
  names the profile as `live`. An unreadable ledger names no profile.
- When `.auto-switch` is corrupt, no override is trusted and only detection
  is used.

### Kind override

```
amplihack profile kind list
amplihack profile kind set <name> <subscription|api-key|auto>
```

The override is one `kind` line in `<root>/.auto-switch`, after the
`overflow` line, and absent when no profile has an override:

```
kind=api-key:platform,spare:subscription
```

An override wins over detection in both directions. `api-key` and `platform`
are both accepted, on the command line and in the file; the file is always
written with `platform`. `auto` removes the entry. A file with no override
is byte-identical to one written before kinds existed.

A `kind` line with an invalid profile name or a kind other than
`subscription`, `platform` or `api-key` makes the whole file corrupt, as does
a second `kind` line. Overrides are read only from a trusted `.auto-switch`,
under the same rules as the other [settings](#settings-file).

| Command | Behaviour |
|---------|-----------|
| `kind list` | **Read-only.** One line per profile, sorted by name: the name, the kind (`subscription` or `api-key`) and, in brackets, the source from the [detection table](#kind-detection). When `.auto-switch` is corrupt, first prints `warning: .auto-switch is unreadable or invalid; no kind override is used` on stderr. Takes no lock. |
| `kind set <name> api-key` | Writes `kind=<name>:platform`. Prints `kind: <name> is api-key (override)` and the cost note below. |
| `kind set <name> subscription` | Writes `kind=<name>:subscription`. Prints `kind: <name> is subscription (override)`. |
| `kind set <name> auto` | Removes the override. Prints `kind: removed the override on <name>; detected <kind> (<source>)`, or `kind: <name> has no override` when there was none, and exits 0. |

The name must pass the [name rules](#name-rules) and, except for `auto`, be
an existing profile directory under the root; `auto` accepts any valid name
so that a stale override can be removed. Any other kind is a usage error
(status 2). `kind set` uses the same locked write as
[`overflow add`](#overflow-commands), with the same `AutoSwitchConfig`
errors.

```
$ amplihack profile kind list
api-key         api-key       (detected: login file has no oauth login)
ryan-mistt      subscription  (detected: oauth login)
ryan-mistt-org  subscription  (detected: live in ~/.claude (oauth login))
ryan-ryansweet  subscription  (detected: oauth login)
```

Setting the API-key kind prints a cost note:

```
$ amplihack profile kind set api-key api-key
kind: api-key is api-key (override)
note: api-key is billed per token on the Claude Platform API key, which is stored outside the profile directory; the rotator uses it only after every plan and credit profile.
```

When the running rotator comes from an older `amplihack`, `kind set` also
prints, on stderr:

```
warning: the running rotator (pid 48213) is from an older amplihack and will read <root>/.auto-switch as corrupt; upgrade it, then run 'amplihack profile rotate stop' and 'amplihack profile rotate start'
```

`profile login` on an API-key profile works as before and first prints:

```
note: this profile uses a Claude Platform credential stored outside the profile directory, which amplihack does not manage.
```

A completed OAuth login gives the profile a `claudeAiOauth` key, so with no
override it is detected as `subscription` from then on. Run
`kind set <name> auto` after a login if an old override should no longer
apply.

### Held logins

A subscription profile's login can be away from its directory without
anything being wrong. The login is *held* for the profile when either:

- rotate [live profile](#rotate-live-profile) detection names the profile:
  the ledger and the account in `~/.claude.json` agree; or
- auto-switch records a [lease](#leases) or hosting record for it.

A `hand-change`, `ambiguous` or adopt result does not count. A held profile
keeps the `subscription` kind and its launches are allowed.

### No silent billing

`profile run`, `claude --profile`, `launch --profile` (including a profile
taken from `AMPLIHACK_CLAUDE_PROFILE` or the default pointer) and
`profile switch` refuse a profile when all of these hold:

- its kind is `subscription`;
- its own login file has no OAuth login (missing, unreadable, or without a
  `claudeAiOauth` key);
- its login is not [held](#held-logins) elsewhere.

While `.auto-switch` is corrupt, every profile whose own login file has no
OAuth login is checked as a subscription profile. The refusal comes before
any variable is set, any process is spawned, or any pointer, journal or
rename is written. The command exits 1:

```
$ amplihack profile run old-team -- -p "reply with OK"
error: profile old-team is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login old-team', or run 'amplihack profile kind set old-team api-key' if per-token billing is intended.
hint: amplihack profile login <name>, or: amplihack profile kind set <name> api-key
```

The error is `SubscriptionNoLogin`.

| Case | Result |
|------|--------|
| API-key profile | Allowed. Work is billed per token. |
| Subscription profile with its own OAuth login | Allowed, as before. |
| Subscription profile whose login is the rotator's live login | `launch` and `claude` are [redirected](#live-profile-launch) to `~/.claude`; `switch` is allowed. |
| Subscription profile whose login is leased to another session | Refused with `Leased`, as before. |
| Subscription profile with no login held anywhere | Refused with `SubscriptionNoLogin`. |

Plain `amplihack launch` with no profile, and plain `claude`, are not
checked. For those the rotator's [swap check](#api-key-rotation) is the
safeguard: it never leaves `~/.claude` without an OAuth login unless an
API-key profile is moved there.

The refusal exists only on Unix; see
[API-key platform support](#api-key-platform-support).

### API-key platform support

Kind detection, `profile kind`, the `SubscriptionNoLogin` refusal and the
rotator's swap check exist only on Unix. They are compiled out on Windows
and other non-Unix platforms, as are the rotator and auto-switch they build
on.

On those platforms `profile run`, `claude --profile` and `launch --profile`
start Claude with `CLAUDE_CONFIG_DIR=<root>/<name>` and no kind check, and
`profile kind` is not a command. Claude Code's own fallback still applies:
a profile with no OAuth login runs on the Claude Platform credential when
one is configured, and that work is billed per token. On Windows, run
`amplihack profile login <name>` before launching a profile, and do not keep
a Platform credential on a machine where per-token billing is not wanted.

### API-key profiles in commands

| Command | Behaviour for an API-key profile |
|---------|----------------------------------|
| `list`, `show` | As for any profile. |
| `run`, `claude --profile`, `launch --profile` | Launch with `CLAUDE_CONFIG_DIR=<root>/<name>`, so Claude uses the platform credential. No refusal. |
| `usage` | Runs no `/usage` probe. See [API-key usage](#api-key-usage). |
| `switch` | See [Switching onto an API-key profile](#switching-onto-an-api-key-profile). |
| `login` | Unchanged apart from the note in [Kind override](#kind-override). |
| `overflow add` | Refused; see [API-key errors](#api-key-errors). |
| `overflow list` | After the marked profiles, one line per API-key profile: the name and `api-key tier`, then `blocked until <time> (<reason>)` while the [API-key block](#api-key-block) is active, or `not used (<first> comes first)` for each API-key profile after the first by name. |
| `overflow clear --platform` | Clears the [API-key block](#api-key-block). |
| Duplicate-login check | Skips every API-key profile: they all share one credential by design. |

### API-key usage

`profile usage` gives an API-key profile a row with no reading. Its `STATE`
is `api_key` and its `SESSION` cell is `api-key (billed per token)`; the
other usage cells are `-`. The `MARKS` cell adds `live in ~/.claude` when
its login file is live, and `blocked until <time> (<reason>)` while the
[API-key block](#api-key-block) is active. The row never shows `unparsed`,
`timeout` or `failed`. API-key rows sort after every usable and exhausted
plan row and before the rows with no reading.

Below the table, `usage` prints
`live: work is running on the API key (billed per token)` when an API-key
profile is live in `~/.claude`, and `blocked: the API key until <time>
(<reason>)` while a block is active:

```
$ amplihack profile usage
PROFILE         MARKS              STATE              SESSION                     SESSION RESETS                  WEEK  WEEK RESETS
ryan-mistt      -                  session_exhausted  100%                        Oct 28, 4:50pm (Europe/London)  64%   Nov 1, 2am (Europe/London)
ryan-ryansweet  -                  session_exhausted  100%                        Oct 28, 5:20pm (Europe/London)  71%   Nov 1, 2am (Europe/London)
api-key         live in ~/.claude  api_key            api-key (billed per token)  -                               -     -
live: work is running on the API key (billed per token)
```

In `--json` output every row gains `"kind"`, `"subscription"` or `"api-key"`,
and `"api_key_blocked"`, an object with `reason` and `until` (epoch seconds)
while the block is active and `null` otherwise. An API-key row has
`"state": "api_key"` and `null` usage windows.

### Switching onto an API-key profile

`profile switch <name>` onto an API-key profile is allowed, because it is an
explicit choice to pay per token. It runs the kind check, then the normal
[switch steps](#rotate-switch): the `{}` login file is moved into `~/.claude`
by the journalled rename swap, rotation is [pinned](#rotate-pin) to the
profile and the default pointer is written. Then it prints:

```
note: api-key is an API-key profile; work in ~/.claude is billed per token until you run: amplihack profile switch --none
```

While the pin stands, the rotator does not move off the API-key profile even
when a plan login has capacity, as with any [pinned overflow
profile](#pin-and-overflow). `profile switch` refuses an API-key profile
while the [API-key block](#api-key-block) is active, with `PlatformBlocked`.
The check runs after recovery and before the live login is identified, so it
also applies when `~/.claude` cannot be used. The refusal writes nothing,
`.default` included, since a launch of the default would otherwise use the
blocked key directly:

```
error: profile api-key: the API key is blocked (key_exhausted) until 2026-12-29T10:14:03Z
hint: amplihack profile overflow clear --platform
```

### API-key rotation

API-key profiles are the last tier, below every credit
[overflow tier](#overflow-tiers). Nothing has to be marked: every profile
whose kind is `api-key` belongs to the tier, and `overflow list` shows it as
`api-key tier`. The full order is:

1. plan (subscription) logins;
2. credit overflow profiles, by tier;
3. the API-key profile.

An API-key profile listed in `overflow=` is ignored there and still belongs
to the API-key tier. The rotator logs one line per process:

```
skip: API-key profile api-key ignored in overflow=; API-key profiles are always the last tier
```

An API-key profile is never probed with `/usage`. Its reading is usable
unless the API key is blocked. It never counts as plan capacity and never
takes part in the headroom comparison. The `exclude` list and the
[candidate](#rotate-candidates) skip reasons apply to it, except
`no account`.

**Moving onto the API key.** The rotator swaps `~/.claude` onto the
API-key profile, with `reason=overflow-in`, only when all of these hold:

- no plan login has [plan capacity](#plan-pool);
- no credit overflow profile can be used: each is blocked, excluded, not a
  candidate or failed its probe;
- the [earliest plan reset](#earliest-plan-reset) is further away than
  `--overflow-wait`, or unknown with `--overflow-unknown-reset 1`;
- the API key is not [blocked](#api-key-block).

If the reset falls inside the wait, the rotator waits, as for credits.

**Moving off the API key.** While the API-key profile is live, these rules
apply in order on every cycle. `--hold`, `--margin` and `--upgrade-margin`
do not apply to them:

| Condition | Outcome |
|-----------|---------|
| A plan candidate has capacity | Swap to the plan candidate with capacity that has the most headroom, `reason=overflow-out`. A failed plan probe is not capacity. |
| No plan candidate has capacity, and a credit overflow candidate can be used | Swap to the first by tier, `reason=overflow-out`. Credits cost less than the API key. |
| The API key is blocked, and a plan or credit candidate can be used | Swap to the best one: a plan candidate that is not exhausted and whose probe did not fail, then a credit candidate. |
| The API key is blocked, and nothing can be used | One ALERT and stay. No swap is attempted. |
| Otherwise | Stay on the API key. |

From a credit profile the [overflow rules](#rotate-overflow) apply
unchanged; the rotator never moves from credits to the API key while a
credit profile can be used, or from a plan login to credits or the API key
while a plan login has capacity.

**How the swap works.** The API-key profile's `{}` login file is moved into
`~/.claude` by the same [journalled rename swap](#rotate-swap) as any login,
and the outgoing login goes back to its own profile. Nothing is copied, an
interrupted swap is recovered from the journal, and `~/.claude` always holds
a login file. A profile with no login file cannot be swapped in; no empty
file is created for it.

Before the journal is written, the swap is refused when it would leave
`~/.claude` without an OAuth login and the incoming profile does not resolve
as `api-key`. A refusal leaves no journal, renames nothing and logs:

```
ALERT: swap refused: ryan-ryansweet has no oauth login and is not an API-key profile
```

**Log lines.** A swap onto or off the API key logs the ordinary
[swap line](#rotate-log), then one of:

```
moved ~/.claude onto API key profile api-key (billed per token)
moved ~/.claude off API key profile api-key to ryan-mistt
```

### Several API-key profiles

All API-key profiles share one credential, so they form one pool: the
rotator and auto-switch use the first by name and never move from one
API-key profile to another. Keep one API-key profile per root. One
[block](#api-key-block) covers every API-key profile, including ones added
later.

When the live login in `~/.claude` has no OAuth login, more than one
API-key profile exists and the ledger names none of them, the live profile
is ambiguous. The rotator logs once, and makes no swap until the ledger
names one:

```
ALERT: more than one API-key profile could be live in ~/.claude; no swap
```

### API-key block

When an agent step run under the [usage-limit wait](#usage-limit-wait)
fails because the key cannot be used, the whole API-key tier is blocked.
The output is checked only when the attempt ran under an API-key profile,
or under `~/.claude` while an API-key profile was live there. Matching
ignores case. The text is matched and dropped: it is never stored, logged
or printed.

| Reason | Matching text | Block lasts |
|--------|---------------|-------------|
| `key_exhausted` | `credit balance is too low`, `insufficient funds` | until cleared by hand |
| `key_revoked` | `invalid x-api-key`, `authentication_error` | until cleared by hand |
| `key_rate_limited` | `rate_limit_error`, or `429` with `rate limit` | until the reset or `retry-after` time the error gives, at most 62 days; one hour when it gives none or one in the past |

*Until cleared by hand* is stored as 62 days after the block, the longest
block a ledger may hold. When that passes, the key is tried once more and,
if it still fails, blocked again.

`authentication_error` means "not logged in" for a subscription profile. It
blocks the API key only for attempts under an API-key profile.

A block is one `platform_block` record in the
[ledger](#rotate-state-directory), written under the rotate state lock and
only when no block is active, so two sessions failing at once record one
block. Only a new block writes an ALERT:

```
ALERT: the API key is blocked (key_exhausted) until 2026-12-29T10:14:03Z; API-key profiles are not used
```

When the API-key profile is live, blocked, and no plan or credit profile can
be used, the rotator stays and logs once:

```
ALERT: the API key is blocked (key_exhausted) and no plan or credit profile can be used; staying on api-key
```

If the state lock cannot be taken, the session logs
`platform block not recorded: <error kind>` and carries on.

Clear a block once the key is funded, replaced or no longer limited:

```
$ amplihack profile overflow clear --platform
overflow: cleared the API-key block (key_exhausted)
```

With no block it prints `overflow: the API key is not blocked` and exits 0.
`overflow clear --platform` cannot be combined with a profile name.

Bare `claude` sessions that amplihack does not supervise are not read. The
rotator does not probe the key, because a probe costs tokens.

### API-key status

`rotate status` and `auto-switch status` print one line while the ledger's
`live_kind` is `platform`:

```
~/.claude is on API key profile api-key: work is billed per token (since 2026-10-28T14:02:11Z)
```

The ledger is the only evidence; nothing is probed. The `on overflow` line
reads `yes (<name>)` while an API-key profile is live. A block is shown by
`profile usage` and `overflow list`.

### API-key log lines

Every line the rotator writes to the [rotate log](#rotate-log) about the
API-key tier. Each ALERT is written once per change of state.

| Line | When |
|------|------|
| `moved ~/.claude onto API key profile <name> (billed per token)` | After a swap onto the API-key profile. |
| `moved ~/.claude off API key profile <name> to <target>` | After a swap off it. |
| `ALERT: the API key is blocked (<reason>) until <time>; API-key profiles are not used` | A new [API-key block](#api-key-block). |
| `ALERT: the API key is blocked (<reason>) and no plan or credit profile can be used; staying on <name>` | The live API-key profile is blocked and there is nowhere to go. |
| `ALERT: more than one API-key profile could be live in ~/.claude; no swap` | The live API-key profile is [ambiguous](#several-api-key-profiles). |
| `ALERT: swap refused: <name> has no oauth login and is not an API-key profile` | The swap check refused a target. |
| `skip: API-key profile <name> ignored in overflow=; API-key profiles are always the last tier` | An API-key profile is listed in `overflow=`; once per rotator process. |
| `platform block not recorded: <error kind>` | A session saw a key error but could not take the state lock. |

### API-key ledger fields

The [rotate ledger](#rotate-state-directory) keeps `version: 1` and gains
these optional fields. Older ledgers load unchanged.

| Field | Contents |
|-------|----------|
| `live_kind` | `"platform"` while `live` is an API-key profile; absent means subscription. Only present with `live`. The rotator checks and repairs it on every cycle; it is kept for readers, such as the limit-resume hook, that cannot detect kinds. |
| `platform_block` | `{"reason", "at", "until"}`, with `reason` one of `key_exhausted`, `key_revoked` or `key_rate_limited`, and `0 < at < until <= at + 62 days`; absent when there is no block. Expired blocks are removed the next time the rotator records a cycle. |

The limit-resume hook reads both fields without failing on a missing or
wrongly typed field, and treats a live API-key profile as on the API key,
not as plan capacity.

### API-key profiles and auto-switch

Auto-switch uses the same tier order, wait gate and move-back rules as the
rotator, taken from the same function, so the two agree on which profile is
the API-key candidate. It never chooses a credit or API-key profile while any
plan profile has capacity.

The platform login is never lent to another profile's session, and an
API-key profile's directory never hosts another account's login, because
the credential is shared across the home directory. Auto-switch therefore
uses an API-key profile only for a new nested launch or a usage-limit rerun,
which runs in that profile's own directory, and never swaps it into a
running session. A session started in the API-key profile's directory is
not moved mid-session; once plan capacity returns, its next nested launch or
rerun goes back to a plan profile. Sessions in `~/.claude` leave the API key
when the rotator moves the login.

### API-key errors

| Condition | Message | Hint |
|-----------|---------|------|
| `SubscriptionNoLogin`: `run`, `claude`, `launch` or `switch` named a subscription profile with no OAuth login of its own that is not held | `profile <name> is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. ...` (in full under [No silent billing](#no-silent-billing)) | `amplihack profile login <name>, or: amplihack profile kind set <name> api-key` |
| `PlatformNotMarkable`: `overflow add` named an API-key profile | `profile <name> is an API-key profile; API-key profiles are always the last overflow tier and cannot be marked` | `` run `amplihack profile kind list` `` |
| `PlatformBlocked`: `switch` named an API-key profile while the API-key block is active | `profile <name>: the API key is blocked (<reason>) until <time>` | `amplihack profile overflow clear --platform` |
| `AutoSwitchConfig` from `kind set`: `.auto-switch` is corrupt, a symlink, untrusted or busy | as for the other [settings commands](#settings-file) | as for the other settings commands |

Messages hold profile names, kinds, reasons and times only. No message, log
line, ALERT or ledger field holds a byte read from a login, settings or
`.claude.json` file.
