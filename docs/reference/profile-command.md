---
title: "Reference: amplihack profile"
description: Commands, configuration, ownership checks, and recovery for Claude profiles.
doc_type: reference
review_schedule: as-needed
---

# Reference: `amplihack profile`

Manage Claude Code login profiles. See the
[how-to](../howto/claude-profiles.md) for a walkthrough.

Automatic selection also follows [organisation affinity](profile-organisation-affinity.md):
same-organisation capacity takes precedence over headroom ranking; upgrades and
plan-room never make a confirmed crossing, and an unknown organisation is not one. The reference describes the
crossing and unresolved-affinity warnings and how a resumed session receives them.

See also the [capacity governor](./capacity-governor.md), which reads the
usage readings described here to decide when new work may start.

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
amplihack profile usage [--json | --summary-json | --table] [--utc] [--raw] [--spend] [--usable-session-pct <1-100>] [<name>...]
amplihack profile auto-switch on [--threshold <1-100>] [--exclude <name,...>]
amplihack profile auto-switch off [--exclude <name,...>]
amplihack profile auto-switch status
amplihack profile auto-switch threshold <1-100>
amplihack profile auto-switch exclude [<name>...]
amplihack profile auto-switch install
amplihack profile auto-switch repair
amplihack profile overflow add|remove|clear <name>
amplihack profile overflow list
amplihack profile rotate [--dry-run] [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1]
amplihack profile rotate start|watch [<tuning flags>] [--interval SECS]
amplihack profile rotate watch [<tuning flags>] [--interval SECS] --supervised[=UNIT]
amplihack profile rotate stop|status
amplihack profile rotate autostart [on|off]
amplihack profile rotate owner [clear]
amplihack profile kind list
amplihack profile kind set <name> <subscription|api-key|auto>
amplihack profile verify [--fix]
amplihack profile seed-record <name> [--write]
amplihack profile prune-probe-logs [--dir <dir>] [--dry-run]
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
| `usage [<name>...]` | Reads each profile's usage from Anthropic's OAuth usage endpoint with the profile's own token, several profiles at a time, and starts `claude` only for a profile whose direct read fails, or, with `--spend`, to read the spend limit of an account with [usage credits on](#usage-credits-on); then shows its weekly and session usage as bars and its spend status as a mark, sorted by the rotator's headroom, followed by a [capacity summary](#capacity-summary) and, with no names, a live reading of each [Codex account](#codex-accounts). `--table` for the text table, `--utc` for UTC reset times, `--json` or `--summary-json` for scripts, `--raw` to include Claude's output. Refreshes auto-switch readings and adds to the [usage history](#usage-history). See [Usage command](#usage-command). |
| `auto-switch …` | Turn [auto-switch](#auto-switch) on or off, set its threshold and exclusions, show its state, or install and repair the status-line wrapper. See [Auto-switch commands](#auto-switch-commands). |
| `overflow …` | Mark or unmark a credit-backed profile that the rotator may use when every plan login is exhausted, list the marks, or lift a spend-limit block. See [Overflow commands](#overflow-commands). |
| `rotate …` | Keep the login in `~/.claude` on the account with the most capacity: once, as a dry run, or in a background process with `start`, `stop` and `status`; `autostart` controls whether launches start that process. See [Rotate](#rotate). |
| `kind list`, `kind set <name> <kind>` | Show whether each profile is a subscription or an [API-key profile](#api-key-profiles), and set or remove an override. See [Kind override](#kind-override). |
| `verify [--fix]` | Unix only. For `~/.claude` and each profile folder, print the account Anthropic's OAuth profile endpoint reports for the folder's own token, against the account [recorded](#recorded-account) for it. `--fix` moves a profile's login that is another profile's recorded account into that profile's empty folder; do not use it until [#1960](https://github.com/rysweet/amplihack-rs/issues/1960) is closed. See [profile verify](#profile-verify). |
| `seed-record <name> [--write]` | Unix only. Shows the account a profile with no [record](#recorded-account) would be recorded with, from one fresh reading. `--write` saves it. Moves no login and writes no credentials file. See [profile seed-record](#profile-seed-record). |
| `prune-probe-logs [--dir <dir>] [--dry-run]` | Deletes the session logs that usage probes from older amplihack versions left in `~/.claude/projects/<encoded home>`, and nothing else. `--dry-run` lists them without deleting. Never run automatically. See [Prune probe logs](#prune-probe-logs). |

`remove` and `login` are also refused while auto-switch has an account's
login away from home; see [Leases](#leases). `run`, `switch` and
`--profile` launches refuse a subscription profile with no OAuth login of
its own on Unix; see [No silent billing](#no-silent-billing).

### Usage command

```
amplihack profile usage [--json | --summary-json | --table] [--utc] [--raw] [--spend] [--usable-session-pct <1-100>] [<name>...]
```

Shows how much of each profile's current session and weekly quota is used
and when each resets, so you can see which accounts have capacity. Below the
bar view or table, a [capacity summary](#capacity-summary) adds the rows up: weekly
headroom across all logins, how many can take work now, the next session
reset, how long the weekly capacity lasts at the recent rate, and which
weekly limits reset in the next 24 hours. With no
names it covers every profile under the root, as `list` does, and then
reads each [Codex account](#codex-accounts) live and lists it after the
Claude output, marking the one `codex-pick`'s rule picks from these live
readings. Each name
must pass the [name rules](#name-rules) and must exist. A name given twice
is probed once. Names select Claude profiles only; with names, Codex is not
read.

| Flag | Effect |
|------|--------|
| `--json` | Print the profile rows as one JSON array instead of the bar view. Claude rows only: Codex is not read, and the Codex rows are in `--summary-json`. No summary. See [JSON output](#json-output). Conflicts with `--table` and `--utc`. |
| `--summary-json` | Print one JSON object holding the same Claude rows, the summary and the Codex rows. Cannot be combined with `--json`, `--raw`, `--table` or `--utc` (exit status 2). See [Summary JSON](#summary-json). |
| `--table` | Print the eight-column text table instead of the bar view. The capacity summary follows it as it follows the bar view. See [Table output](#table-output). Conflicts with `--json`, `--summary-json` and `--utc`. |
| `--utc` | Show reset times in the bar view in UTC, as `MM-DD HH:MMZ`, instead of local time. See [Reset times](#reset-times). Conflicts with `--json`, `--summary-json` and `--table`. |
| `--usable-session-pct <n>` | Session percentage below which a login counts as usable now in the summary, `1` to `100`; default `95`. Overrides `AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT`. A value out of range is a usage error (exit status 2). See [Usable now](#usable-now). |
| `--spend` | Also run one `/usage` probe for each row whose account has [usage credits on](#usage-credits-on), to fill its [spend status](#spend-status): the `spend:` mark in the bar view, the `SPEND` cell with `--table`. Without it no `claude` is started for those rows and their spend is `not_probed`. See [Spend probe](#spend-probe). |
| `--raw` | Also print what `claude` wrote to stdout and stderr, for each profile, from the [attempt](#probe-method) that gave the row. Without it, that output is dropped once parsed. Rows read without `claude` print a fixed line instead. Also prints the `note:` lines for rows that fell back or whose account has [usage credits on](#usage-credits-on). It can include account details; see [Raw output](#raw-output). With `--raw` there is no summary and nothing is added to the [usage history](#usage-history). |
| `<name>...` | Probe only these profiles, in the [usual order](#ordering). |

#### How a profile is probed

Each OAuth profile is read in this order, and the first step that gives a
result decides the row:

1. **Passive reading.** A [reading](#passive-readings) already stored in
   `usage.json` for the account in the profile's directory, if it is no
   older than `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` (300 seconds
   by default).
2. **Direct read.** amplihack asks Anthropic's OAuth usage endpoint itself,
   with the login stored in the profile's `.credentials.json`. No `claude`
   process is started. See [Direct read](#direct-read).
3. **Fallback.** Only when the direct read fails: the `claude` probe
   described below, which sends the `get_usage` request and then, if
   needed, runs `claude -p /usage`.

Every row records which step gave it as its [source](#reading-source):
`passive`, `direct` or `fallback`. Some profiles skip steps:

| Profile | Steps |
|---------|-------|
| In the auto-switch `exclude` list | None. It is not probed, read or refreshed. See [Excluded profiles](#excluded-profiles). This wins over every row below. |
| [API-key profile](#api-key-usage) | None. It is not probed, as before. |
| Marked as [overflow](#rotate-overflow) | Fallback only, as `/usage` text. The spend limit is visible only in `/usage`, so the [spend-limit block](#overflow-spend-limit-block) keeps working. Source `fallback`. Under `AMPLIHACK_PROFILE_USAGE_PROBE=direct` the row is not probed and reads `error`; see [Probe method](#probe-method). |
| A direct answer shows usage credits turned on (`extra_usage` is not absent, `null` or `{"is_enabled": false}`) | The windows and state come from the direct answer. Source `direct`. Spend `not_probed`, unless `--spend` asks for one `/usage` text probe, under the normal per-probe timeout (`--timeout-secs`, 120 s by default), that reads only the spend status; see [Usage credits on](#usage-credits-on). If neither window in the answer is usable, the direct read fails with `usage credits on` and the profile goes to the fallback. |
| Any profile, with `AMPLIHACK_PROFILE_USAGE_PROBE` set to `auto` or `text` | Fallback only, with the method named. See [Probe method](#probe-method). |
| Any profile, with `AMPLIHACK_PROFILE_USAGE_PROBE=direct` | Passive, then direct. A failed direct read gives state `error`, and the reading starts no `claude`. An account with usage credits on gives its direct windows, with spend `not_probed`, because no spend probe runs, even under `--spend`. Overflow profiles read `error` without a direct read. The one exception is the rotator's [held-cycle spend-limit check](#pin-and-overflow). |

When every row is passive or direct, no `claude` process is started, and
amplihack does not look for the `claude` binary at all, unless `--spend`
asks for a spend probe of a row with usage credits on. It looks for it the first time a row needs the
fallback or a [spend probe](#spend-probe), and keeps the result for
the rest of the command (or the rest of the rotator cycle). If `claude`
cannot be found then, only the rows that needed it for a reading read
`error`, with the reason `claude not found`; a row that needed it only for
a `--spend` probe keeps its windows and shows spend `unknown`. The other rows
are unaffected.

##### Passive readings

A passive reading is the entry in
`<root>/.auto-switch.d/usage.json` for the account whose login is in the
profile's directory, as the auto-switch ledger names it. For a profile
[live in `~/.claude`](#logins-in-claude), that is the profile's own
account. The entry is used only when all of these hold:

- auto-switch state exists and the ledger names an account for the
  directory;
- the entry's `observed_at` is no later than now and no more than
  `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` seconds ago;
- at least one of its windows passes the same plausibility check as a
  status-line reading: a finite percentage, and a reset from one day before
  to eight days after now.

Otherwise the profile is read directly. The live profile is no exception:
a stale reading for it is read directly like any other.

| Setting | Values | Default |
|---------|--------|---------|
| `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` | `0` to `3600` | `300` |

`0` turns passive readings off. An empty variable counts as unset. Any
other value prints one warning to stderr, showing the value escaped and cut
to 32 characters, then uses `300`:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS "ten" (use 0 to 3600); using 300
```

A passive row is built from the stored windows: `five_hour` becomes
`session` and `seven_day` becomes `week_all`. `resets_text` is the reset
time written as `YYYY-MM-DDTHH:MM:SSZ`, and `week_models` is empty. An
entry with a plausible `seven_day` and no `five_hour` reads as
[no session limit](#no-session-limit): `session` is `null` and
`session_limit` is `false`, as a direct read of the same answer gives,
and as auto-switch counts a login with no session window.
So a login with weekly figures only reads the same whether it was read
directly or passively. An entry whose `five_hour` was stored but fails the
plausibility check keeps `session_limit` `true`, with the session unknown.
The state follows the usual [rule](#states).

The status-line wrapper writes these readings during live sessions. Rows
that `profile usage` and the rotator [record](#refreshing-auto-switch-readings)
are stored in the same way, tagged `statusline`, so they also count as
passive while they are fresh: a second `profile usage` within 300 seconds
reads most rows passively. A passive row is never recorded again, so a
reading does not stay fresh by being read.

`amplihack profile usage --help` states the window: a stored reading
younger than 300 seconds (`AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS`,
at most 3600) is used without starting `claude`. The same help says that
the `~/.claude` row and `AMPLIHACK_PROFILE_USAGE_PROBE=auto` or `text`
always probe.

This applies to the rotator too. A rotator cycle records the rows it
measured, so with `--interval 120` the next two cycles mostly use those
readings rather than asking again. A candidate can be ranked on a reading
up to `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` seconds old (300 by
default), but the rotator never swaps to it on that reading alone: a target
whose reading is more than 300 seconds old, or older than the live login's,
is read again first, by the [probe method](#probe-method) with passive
readings off; see
[Rotate reading freshness](#rotate-reading-freshness-and-swap-back-damping).
Lower the value in the rotator's environment for it to react
sooner; see
[Readings that take longer than a cycle](../howto/claude-profile-rotation.md#readings-that-take-longer-than-a-cycle). To force a new reading of every
profile, run with `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS=0`.

##### Direct read

amplihack reads the profile's OAuth login and sends one request:

```
GET https://api.anthropic.com/api/oauth/usage
Authorization: Bearer <access token>
anthropic-beta: oauth-2025-04-20
Accept: application/json
User-Agent: amplihack-profile-usage
```

This is the endpoint Claude Code's `/usage` reads. The endpoint, the beta
header, the default token URL and the OAuth client id used for
[refreshes](#token-refresh) are the values Claude Code 2.1.316 uses, and are
constants in `usage_direct.rs`; a change on Anthropic's
side makes direct reads fail, and every profile then falls back to
`claude`, more slowly but with the same figures.

**Which login is read.** The credentials file follows the directory
`claude` would use for that row:

| Row | File |
|-----|------|
| A profile in its own directory | `<root>/<name>/.credentials.json` |
| A profile [live in `~/.claude`](#logins-in-claude) | `$HOME/.claude/.credentials.json` |

The file is opened without following symlinks and read up to 64 KiB. It
must be a regular file owned by you and not writable by group or others.
The access token is `claudeAiOauth.accessToken`; `claudeAiOauth.expiresAt`
is its expiry in Unix epoch milliseconds. A token is used only if it is
1 to 4096 bytes long and every byte is a printable ASCII character other
than a space (`0x21` to `0x7E`). A token is used only for the row of the
directory it was read from.

**The answer.** The body is read up to 64 KiB and parsed as JSON. It has
the same `five_hour`, `seven_day` and `extra_usage` objects as the
`rate_limits` object of a [`get_usage` answer](#probe-method), and is read
by the same code, so the rules for `utilization`, `resets_at`, what counts
as credits on, and [no session limit](#no-session-limit) are the same. The
one difference is what happens when credits are on: the usage request's
answer is rejected, but a direct answer still gives its windows; see
[Usage credits on](#usage-credits-on). For example:

```json
{
  "five_hour": { "utilization": 100.0, "resets_at": "2026-11-01T14:00:00Z" },
  "seven_day": { "utilization": 55.0, "resets_at": "2026-11-05T09:00:00Z" },
  "extra_usage": { "is_enabled": false }
}
```

gives a `session_exhausted` row with `session` at 100% and `week_all` at
55%. The weekly figure is always the one reported, whatever the session
reads. Unknown fields are ignored. A body that is not a JSON object, or in
which neither `five_hour` nor `seven_day` has a valid `utilization`, is
`unparseable`. A direct read never gives `spend_limit`, `not_logged_in` or
`unparsed`.

**Timing.** The connect and read timeouts of each `GET` are
`min(10 seconds, deadline - 1 second)`, at least 500 milliseconds, and the
whole read, refresh included, must finish within the
[deadline](#probe-deadline). The deadline is fixed when the read starts,
and each request is given only the time left of it, so a `GET` that answers
`401`, the refresh `POST` and the second `GET` together take no longer than
one deadline. No request starts once the deadline has passed.

The DNS lookup is inside that deadline too. The HTTP client puts no time
limit on a lookup of its own, so each lookup is bounded by amplihack: for a
`GET`, by the read's deadline; for the refresh `POST`, by its connect
timeout less 500 milliseconds. A lookup that misses its bound ends the
request as a DNS failure, before any of the request is sent.

A refresh `POST` is sent only when at least 3 seconds of the deadline are
left; otherwise none is sent and the read is `timeout`. A refresh can
retire the old refresh token, so one given up on before its answer arrives
could leave the profile logged out, while a skipped refresh costs only this
read. The `POST`'s connect and read timeouts are `min(10 seconds, time
left)`, so never below 3 seconds, and it too ends by the deadline. With a
deadline under 3 seconds, a direct read never refreshes.
Redirects are not followed, no proxy is used (`HTTPS_PROXY` and similar are
ignored), and a `429` is not retried. One read makes at most two `GET`s and
one refresh `POST`.

**Endpoint overrides.** Two variables point the direct read at another
server, for tests or a local relay:

| Setting | Default |
|---------|---------|
| `AMPLIHACK_PROFILE_USAGE_API_BASE` | `https://api.anthropic.com` |
| `AMPLIHACK_PROFILE_USAGE_TOKEN_URL` | `https://platform.claude.com/v1/oauth/token` |

A value is accepted only if it starts with `https://`, or starts with
`http://` and its host is exactly `127.0.0.1`, `[::1]` or `localhost`. The
scheme is matched exactly, in lower case. A value with whitespace, a
control character, `@`, `?` or `#` in it is refused, and the host is checked
again after the URL is parsed, so `http://localhost.example.com` and
`http://127.0.0.1@example.com` are refused. HTTPS is required for both
requests unless both values are loopback addresses. A refused value prints
one fixed warning, which never repeats the value, and the default is used:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_API_BASE (use https:// or a loopback http:// address); using the default
```

An empty variable counts as unset.

##### Direct read failures

A direct read that fails gives one of these fixed reasons. None of them
holds a token, a URL, a response body or an underlying error message; only
the numeric HTTP status class is kept.

| Reason | When |
|--------|------|
| `no credentials file` | The credentials file is missing. |
| `credentials refused` | The file is a symlink, not a regular file, owned by someone else, writable by group or others, over 64 KiB, or could not be read. |
| `no oauth token` | The file is not JSON, or has no usable `claudeAiOauth.accessToken`. |
| `token expired` | The token has expired and amplihack may not [refresh](#token-refresh) it in this directory. |
| `http 401`, `http 403`, `http 429` | The endpoint answered with that status (a `401` after any allowed refresh). |
| `http 3xx`, `http 4xx`, `http 5xx` | Any other status of that class. |
| `timeout` | A timeout or the deadline was reached. |
| `transport` | The connection or TLS failed, or the server closed it. |
| `unparseable` | The body is over 64 KiB, not a JSON object, or has no usable window. |
| `usage credits on` | The answer shows usage credits on (`extra_usage.is_enabled` as `true`, or an `extra_usage` amplihack does not recognise), and neither `five_hour` nor `seven_day` has a valid `utilization`. When either window is usable, the read succeeds instead; see [Usage credits on](#usage-credits-on). |
| `refresh busy` | The token needs a refresh, but the refresh lock is held or a swap is in progress. |
| `refresh refused` | The checks before a refresh failed (see [Token refresh](#token-refresh)). |
| `refresh failed` | The refresh request did not reach the token server (a DNS lookup that failed or ran out of time, or a failed connection or TLS handshake), or the server turned it away with a `3xx` or `4xx` status. The old refresh token is still on file. |
| `refresh not saved` | The refresh request may have reached the token server, but its new login was not written: the answer was a `5xx` or a `2xx` other than `200`, the answer did not arrive before the deadline, the connection failed after the request was sent, a `200` answer was not usable, or the write failed. |

Except under `AMPLIHACK_PROFILE_USAGE_PROBE=direct`, a failed direct read
moves on to the fallback. The table marks the row `(fallback)` and the JSON
gives `source` `fallback`; the reason is printed only with `--raw`, as one
line to stderr for that row:

```
note: profile "spare": direct read gave http 5xx; using claude
```

Without `--raw`, nothing is printed for it, so an endpoint that fails for
every profile does not add a line per profile to each run. The rotator never
prints this line, to its log or to `rotate.err`. To see why a login fell
back, run `amplihack profile usage --raw <name>`.

`usage credits on` leads to a `/usage` text probe instead of `get_usage`.
Under `direct`, the row's state is `error` and its reason is
`direct read: <reason>`, for example `direct read: http 5xx`. An overflow
profile under `direct` is not read at all and reads `error` with
`direct read: overflow needs /usage`, because it can only be read with
`/usage`.

##### Usage credits on

A direct answer shows usage credits on when its `extra_usage` is anything
other than absent, `null` or an object with `"is_enabled": false`. An
`is_enabled` of `true`, an object with no `is_enabled`, an `is_enabled` of
`"false"` or `0`, and an `extra_usage` that is not an object all count as
on. A profile does not need to be marked as [overflow](#rotate-overflow)
for this.

When credits are on and either window in the answer is usable, the direct
answer gives the row as for any other account:

- `session`, `week_all`, `session_limit` and `state` come from `five_hour`
  and `seven_day`;
- `source` is `direct`, and the row has no `(fallback)` mark;
- the [ordering](#ordering) uses that state and those windows.

Only `/usage` shows a used-up spend limit. Without `--spend`, no
`claude` is started for such a row and its
[spend status](#spend-status) is `not_probed`. With `--spend`, in the
default [reading order](#how-a-profile-is-probed), `profile usage` runs a
*spend probe* for each such row. Its result goes only to the row's spend
status. It never changes the row's state, windows, source or error, so a
row can be `ok` with spend `limit_reached`.

| Mode | Credits-on account with a usable window |
|------|-----------------------------------------|
| unset (default) | Direct windows; spend `not_probed`. With `--spend`, then a spend probe. |
| `direct` | Direct windows. No spend probe and no `claude`, even with `--spend`; spend `not_probed`. |
| `auto`, `text` | No direct read. The [fallback](#fallback-probe) reads the row, and its spend status follows from how it was read; see [Spend status](#spend-status). |
| Marked as overflow, any mode | Unchanged: `/usage` only, or under `direct` the error `direct read: overflow needs /usage`. |

When neither window is usable, the direct read fails with
`usage credits on` and the row goes to the fallback, as described under
[Direct read failures](#direct-read-failures).

##### Spend probe

**Why it is opt-in.** The point of reading credits-on accounts directly
(issue #1856) is that the session and weekly figures no longer wait on
`claude /usage`, which was reported to take 10 to 20 seconds per profile on
a loaded host; one probe has taken 71.5 seconds at load 194 (see
[Probe deadline](#probe-deadline)). A spend probe is that same `/usage`
run. Started on every run, it would either hold the table up for that long
or, under a short deadline, be killed before it answered on the hosts that
need it most, starting and killing `claude` processes for a column that
read `unknown`. So the default run starts none, and `--spend` asks for
them when the spend limit is what you want to know.

**What `--spend` runs.** One `claude -p /usage` in the profile's directory
for each direct row whose account has usage credits on, with the same
command line and environment as the fallback's
[text attempt](#fallback-probe), so no token is passed to it. Spend probes
start only after every passive and direct read has finished, at most the
[`claude` bound](#probe-parallelism) run at once, and each runs under the
same [probe deadline](#probe-deadline) as a fallback: 120 seconds by
default, set by `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS`. A probe still
running at its deadline has its process group killed, and its row shows
spend `unknown`. No `claude` started for a spend probe outlives the
command.

The spend probe's output is parsed for the spend status and then dropped.
It is not shown by `--raw`: the row's raw block stays
`(direct: no claude output)`.

**Notes with `--raw`.** With `--raw`, each such row prints two `note:`
lines to stderr:

```
note: profile "mistt": direct read gave usage credits on; windows direct, spend from claude
note: profile "mistt": extra_usage: object; keys: currency, is_enabled, monthly_limit, org_id, used_credits; is_enabled: bool true
```

The first line ends in one of three ways:

| Ending | When | Spend |
|--------|------|-------|
| `windows direct, spend from claude` | The spend probe finished and its output gave a usage reading. | `ok` or `limit_reached` |
| `windows direct, spend probe failed` | `--spend` asked for a spend probe, and it was killed at the [probe deadline](#probe-deadline), could not run (including `claude` not found), or gave output that parsed as `not_logged_in` or `unparsed`. | `unknown` |
| `windows direct, spend not probed` | No spend probe was asked for: the run had no `--spend`, or `AMPLIHACK_PROFILE_USAGE_PROBE` is `direct`. | `not_probed` |

The second line shows the *shape* of `extra_usage`, so you can see why
amplihack counts the account as having credits on. It never shows a token,
an account or organisation identifier, an e-mail address, a currency or an
amount. It is built as follows.

- **Type.** `extra_usage: object` for an object. Any other value shows only
  its type, `extra_usage: string`, `extra_usage: number` or
  `extra_usage: array`, except a bool, which also shows its value:
  `extra_usage: bool false`. Only an object is followed by keys.
- **Keys.** `keys: ` and the object's top-level key names, separated by
  `, `, in byte order. For an object with no keys, `keys: none`.
  - At most 32 keys are listed. More are counted as `(+N more)` after the
    32nd.
  - Every character other than `A` to `Z`, `a` to `z`, `0` to `9`, `_`,
    `.` and `-` is shown as `?`. This covers spaces, punctuation, control
    and formatting characters, and letters outside ASCII, so a key name
    cannot add a field to the line or reorder it on the terminal.
  - A key longer than 64 characters is cut to 63 and followed by `…`.
  - An empty key is shown as `""`.
- **`is_enabled`.** `is_enabled: ` and one of the forms below. The value is
  printed only when it is one of a fixed set, and then the fixed text is
  printed, not the bytes received.

  | Value of `is_enabled` | Shown as |
  |-----------------------|----------|
  | Absent | `missing` |
  | `null` | `null` |
  | `true` or `false` | `bool true`, `bool false` |
  | A string equal, ignoring ASCII case, to `true`, `false`, `on`, `off`, `yes`, `no`, `enabled`, `disabled`, `0` or `1` | `string "<word>"`, in lower case, for example `string "false"` |
  | Any other string | `string (<n> chars)`, its length in characters |
  | The number `0` or `1`, as an integer or as a float exactly equal to it (`0.0`, `1.0`) | `number 0`, `number 1` |
  | Any other number | `number` |
  | An object or array | `object`, `array` |

- **Other flags.** Each other member among the 32 listed keys whose value
  is a bool or `null` is added, in key order, as `; <key>: bool <value>` or
  `; <key>: null`, with the key shown as in the key list. Members holding a
  string, a number, an object or an array are shown by key name only. Nested
  objects and arrays are not looked into.

Two more examples:

```
note: profile "spare2": extra_usage: object; keys: is_enabled; is_enabled: string "false"
note: profile "team2": extra_usage: object; keys: limit_reached, overage; is_enabled: missing; limit_reached: bool false; overage: null
```

The key names are sent by Anthropic's server and printed after the
replacement and cut above; a key name could itself look like an
identifier. Without `--raw`, nothing about the shape is printed. The shape
is never in `--json`, `usage.json`, the rotate log or `rotate.err`, and the
rotator neither runs spend probes nor prints these lines.

##### Token refresh

A refresh is tried when `expiresAt` is no more than 60 seconds away, or when
the first `GET` returns `401`. There is at most one refresh per profile per
read. It is tried only for a profile read in its own directory, and never
for:

- `~/.claude`, for the live profile;
- a profile whose login is [lent](#leases) to another directory, or whose
  directory [hosts](#leases) another profile's login;
- a profile in the auto-switch `exclude` list. Such a profile is never
  probed, so it never reaches this step; see
  [Excluded profiles](#excluded-profiles). The same holds for the login in
  `~/.claude` when local files show that it belongs to an excluded profile;
  see [Local proof of an excluded live login](#local-proof-of-an-excluded-live-login).

In the first two cases a session may be using the login, and `claude` refreshes it
itself, so the row falls back with `token expired` or `http 401`. The one
exception is outside the usage read: a rotate cycle refreshes the login in
`~/.claude` once, by these steps, when its token has already expired, so
that its identity can be read (see
[An expired live login](#an-expired-live-login)).

The refresh runs as follows:

1. Take Claude's own refresh lock, `<dir>/.oauth_refresh.lock` (a directory
   made with `mkdir`), in the directory that holds the credentials file. If
   it is held, an auto-switch or rotate `journal.json` exists, or the
   rotator is stopping, nothing is refreshed and the reason is
   `refresh busy`. A lock held by someone else
   is never broken.
2. Under the lock, read the file again. If `accessToken` differs from the
   token the `GET` was sent with, `claude` has refreshed it already: the new
   token is used and no refresh request is sent. Only `accessToken` is
   compared; a refresh always replaces it, and a changed `refreshToken`
   alone does not give a token to send.
3. Check that the directory passes the
   [login directory rule](#login-directory-rule), that the file can be replaced safely, and that a private
   temporary file can be created beside it. If any check fails, the reason
   is `refresh refused` and no request is sent.
4. If less than 3 seconds of the [deadline](#probe-deadline) are left,
   stop: no request is sent, nothing is changed and the reason is
   `timeout` (see **Timing** above). Otherwise send `POST` to the token URL
   with a JSON body holding `grant_type` (`refresh_token`), `refresh_token`
   and `client_id` (Claude Code's public client id).
5. On a `200` with a usable answer, set `accessToken`, `expiresAt` (now plus
   `expires_in` seconds, in milliseconds) and, only when one is returned,
   `refreshToken`. Every other field of the file is kept as it was. The new
   token must pass the same check as a stored one, and `expires_in` must be
   a whole number from `1` to `31536000`.
6. Write the file through the temporary file (mode `0600`, synced, renamed
   over the old file, and the directory synced), release the lock, and send
   the `GET` once more with the new token.

The file is never touched unless the write in step 6 succeeds. The `POST`
ends at the read's deadline like every other request, so the 3-second floor
in step 4 makes an unanswered refresh less likely but cannot rule it out.
The token server may retire the old refresh token as soon as it handles the
request, so when the request failed before it could reach the server, or the
server turned it away with a `3xx` or `4xx` status, the reason is
`refresh failed`; in every other case the reason is `refresh not saved`.
Those cases are a `5xx` answer (a `502`, `504`, `520` or `524` from a proxy
in front of the token server can follow a request the server received and
handled), a `2xx` other than `200`, no answer before the deadline, a
connection lost after the request was sent, a `200` that cannot be used,
and a failed write. The old refresh token may no longer work, so one
line goes to stderr in every mode, with or without `--raw` (`rotate.err`
for a background rotator):

```
warning: profile "spare": token refresh may not have been saved; run claude /login for this profile
```

The row then falls back as usual, or reads `error` with
`direct read: refresh not saved` under `AMPLIHACK_PROFILE_USAGE_PROBE=direct`.

The lock is held only for steps 2 to 6. If amplihack is killed with
`SIGKILL` while it holds the lock, the lock stays behind and `claude`
waits on it. Remove it with `rmdir <dir>/.oauth_refresh.lock` when no
`amplihack` or `claude` process is running for that profile.

##### Reading source

| Source | Meaning |
|--------|---------|
| `passive` | A fresh stored reading. Nothing was asked. |
| `direct` | The OAuth usage endpoint answered. |
| `fallback` | `claude` gave the row: the direct read failed (including `usage credits on` with no usable window), the profile is marked as overflow, or `AMPLIHACK_PROFILE_USAGE_PROBE` is `auto` or `text`. A [spend probe](#spend-probe) never makes a row `fallback`. |

The table shows `(fallback)` among a fallback row's [marks](#marks). The
JSON has the `source` key. A row that failed on every step keeps the source
of its last attempt; a row that was never probed has `source` `null`.

##### Fallback probe

For a fallback, amplihack runs the `claude` binary directly, as `login`
does (the binary selection is ignored, and `AMPLIHACK_CLAUDE_BINARY_PATH` is
honoured). A fallback makes at most two attempts, one after the other,
within one [deadline](#probe-deadline) of its own:

1. **Usage request.** `claude` is started as

   ```
   claude -p --input-format stream-json --output-format stream-json --verbose --no-session-persistence
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
   claude -p /usage --no-session-persistence
   ```

   with stdin from `/dev/null`, and its text is [read](#what-is-read).

Both command lines end with `--no-session-persistence`, so `claude` writes
no session log for the probe. On Linux, each one is also run inside the
[probe sandbox](#probe-sandbox), which hides the shared `projects` folder
from the probe.

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
shell, and no arguments other than the ones above are passed; the
[probe sandbox](#probe-sandbox) only puts `bwrap` and its own arguments in
front of them. The profile
directory is checked again just before each spawn, as for every launch. A
refused directory gives `error` (`profile directory refused`), and no further
attempt is made.

For neither fallback attempt does amplihack create, copy, link, move or
write anything in the profile directory, and the fallback never opens or
reads a login file. `claude` reaches the login in place, as it does for any
`claude` run with that `CLAUDE_CONFIG_DIR`, and may update its own files
there, such as `.claude.json`, as any `claude` run does.

- At most the [direct-read bound](#probe-parallelism) of profiles are read
  at a time. A row that falls back waits for a free `claude` slot, and at
  most the [`claude` bound](#probe-parallelism) of `claude` processes run
  at a time, however many reads are running. With `auto` or `text`, every
  row starts `claude`, so the `claude` bound is the only one.
- The fallback's [deadline](#probe-deadline), 120 seconds by default, runs
  from its first spawn and covers both attempts. The `/usage` attempt gets only the
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
  controls. `claude` contacts Anthropic to answer; in the fallback,
  amplihack itself makes no network calls.

Without the [probe sandbox](#probe-sandbox), the `/usage` attempt makes
`claude` read every session transcript reachable from the profile's
`projects` directory. With large transcripts that is over half a minute of
CPU time for one probe: one probe was measured at 71.5 seconds on a 72-CPU
host at a load average of 194, and another opened 10,371 session logs in
47.5 seconds. On 10 November 2026, on the same host at a load average of
about 60, with Claude Code 2.1.322 and 9,084 transcripts in the shared
folder, `claude -p /usage --no-session-persistence` run directly opened
8,918 `.jsonl` files and took 68 seconds. Through `amplihack profile usage`
with the sandbox, the same probe opened 3 `.jsonl` files, none of them under
`projects`, and took 4.5 seconds. In the default mode, where that login's answer to
the usage request was not used and the probe went on to `/usage`, both
attempts together took 7.5 seconds. Neither run created a file in the shared folder, while the same
probe from a build without this change wrote one session log there. The
usage request opened 5 paths under `projects` with or without the sandbox,
so it does not make that scan, though it still pays for `claude` starting
up. This cost is why the
[direct read](#direct-read) comes first. The deadline and the `claude`
bound are still sized for the `/usage` attempt on a loaded host, because it
remains the fallback and the only method that can read overflow profiles.
`profile usage` and the [rotator](#rotate-cycle) use the same settings:
[method](#probe-method), [deadline](#probe-deadline),
[parallelism](#probe-parallelism), the
[passive age](#passive-readings) and the
[endpoint overrides](#direct-read).

##### Probe sandbox

Every `claude` that `profile usage` and the rotator start to read usage
gets two protections: both [fallback](#fallback-probe) attempts, the
[spend probe](#spend-probe) and the rotator's
[held-cycle spend-limit check](#pin-and-overflow).

1. **No session log.** The probe's arguments end with
   `--no-session-persistence`, so `claude` writes no transcript for it.
   Older versions wrote one log of about 5 KB per probe into the shared
   `projects` folder; see [Prune probe logs](#prune-probe-logs) to remove
   them.
2. **No shared history.** On Linux, when `bwrap` (bubblewrap) is usable,
   the probe runs in its own mount namespace with an empty `tmpfs` over the
   resolved `projects` folder:

   ```
   bwrap --dev-bind / / --tmpfs <canonical projects> -- claude -p /usage --no-session-persistence
   ```

   Inside, `projects` is empty, whether `claude` reaches it directly or
   through the profile's `projects` link, so `/usage` has no transcripts to
   read. The login files are seen in place. Nothing outside the probe
   changes: the profile's `projects` link is not repointed, no folder is
   created on the host, and a session started on the same profile during
   the probe writes its transcript to the shared folder as before.

**Which `projects` folder is hidden.** The folder is
`<config dir>/projects`, with every link resolved. The config dir is read
from the probe's own command, not from amplihack's environment:

| `CLAUDE_CONFIG_DIR` on the probe's command | Config dir |
|---|---|
| Set to a value (a profile directory) | That value |
| Removed (a [login live in `~/.claude`](#logins-in-claude)) | `$HOME/.claude` |
| Not mentioned | amplihack's `CLAUDE_CONFIG_DIR` if set, else `$HOME/.claude` |

A profile's `projects` is a link to `~/.claude/projects`, so the folder
hidden is normally `~/.claude/projects` itself. The probe runs without
`bwrap`, with `--no-session-persistence` alone, when the folder does not
exist, a link in its path is broken, or the resolved path is `/`, `$HOME`
or the config dir itself.

**When `bwrap` is used.** The first probe that needs the sandbox looks for
`bwrap` once for the life of the process:

1. Each absolute directory on `PATH` is checked, in order, for a regular,
   executable file named `bwrap`. Empty and relative entries are skipped.
2. That file is run once as `bwrap --dev-bind / / -- true`, with no shell
   and its output discarded.
3. Exit status `0` means `bwrap` is usable, and its absolute path is kept
   for every later probe. A missing file, a failed start or any other exit
   status means it is not. AppArmor on Ubuntu 24.04, for example, can refuse
   unprivileged namespaces to an installed `bwrap`.

The answer is not checked again until the process ends, so the rotator
checks once when it starts. There is no timeout and no retry on the check.
On any system but Linux, `bwrap` is never looked for.

**The note.** When a probe runs without the sandbox, amplihack prints one
line to stderr, at most once per process, naming the reason:

```
note: usage probes run claude with --no-session-persistence only: bwrap not found
note: usage probes run claude with --no-session-persistence only: bwrap refused
note: usage probes run claude with --no-session-persistence only: projects folder not resolved
```

`bwrap refused` means the check in step 2 failed. The note contains no
path and no environment value. It goes to stderr, so stdout stays clean for
`--json` and `--summary-json`; the rotator's copy goes to `rotate.err`.

**What the sandbox is not.** `--dev-bind / /` gives the probe the whole
filesystem, read and write, as before. The sandbox hides one folder; it
does not confine `claude` and is not a security boundary. `bwrap` runs with
no extra privilege: the user id is unchanged.

**Process handling.** The command keeps the probe's environment changes
(values set and values removed) and its working directory. `bwrap` is
started in place of `claude`, so it leads the probe's process group, and
the group kill at the end of each attempt ends `bwrap`, `claude` and what
`claude` started. `--die-with-parent` and `--new-session` are not passed:
the group kill already ends the probe, and the probe's stdio is pipes or
`/dev/null`, never a terminal. If amplihack itself is killed with
`SIGKILL`, the probe can outlive it, as a plain `claude` probe can.

#### Probe method

This section covers the `get_usage` request of the
[fallback probe](#fallback-probe). `claude` prints its answer to the usage request as one JSON line. Shown over
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

A `five_hour` window at `0` whose `resets_at` key is absent or `null`, or no
`five_hour` at all, means the reading has
[no session limit](#no-session-limit) when `seven_day` was read: `session`
is then `null` and `session_limit` is `false`. A `five_hour` above `0` with
no reset keeps its window and the session limit.

This page shows such resets as `2026-10-16T23:50:00Z`. That is an example
of the form: whether `claude` sends fractional seconds, `Z` or an offset
such as `+00:00` is up to `claude`, and `resets_text` keeps what it sent.
The other windows in the payload (`seven_day_opus`, `seven_day_sonnet`,
`seven_day_oauth_apps`) are not used.

The state of an accepted answer is `session_exhausted` when the session is
at 100% or more and a session limit applies, else `weekly_exhausted` when
the all-models week is, else `ok`. The usage request never gives
`spend_limit`, `not_logged_in` or `unparsed`: those states come only from
the `/usage` text.

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
| `extra_usage`, at the top of the payload or at `rate_limits.extra_usage`, is not absent, `null` or an object with `"is_enabled": false`; for example `{"is_enabled": true}` | The account has usage credits turned on, or the answer reports `extra_usage` in a form amplihack does not recognise. amplihack does not read the payload's spend fields (Claude Code 2.1.314 sends `rate_limits.extra_usage.spend_limit_reached` and `rate_limits.spend`), because they can disagree with `/usage`; `/usage` shows a used-up spend limit as `spend_limit`. |
| `claude` could not be started, written to, read or waited for | The attempt failed. |

Profiles marked as overflow are probed with `/usage` only. They never get
a direct read or the usage request, whatever the mode, because the
[spend-limit block](#overflow-spend-limit-block) depends on `/usage`
reporting `spend_limit`. Under `direct`, which starts no `claude` for a
reading, an overflow profile is therefore not probed and reads `error` with
`direct read: overflow needs /usage`, so the rotator does not move onto
it. The rotator's [held-cycle spend-limit check](#pin-and-overflow) of a
pinned live overflow profile is separate from these readings and still
runs `claude -p /usage` under `direct`. The marks are read from `<root>/.auto-switch`
before probing; a missing or corrupt `.auto-switch` marks no profile. A
profile that is not marked but whose account has usage credits turned on is
read directly, and its spend status is `not_probed` unless `--spend` asks
for a [spend probe](#spend-probe), which runs under the normal per-probe
timeout (`--timeout-secs`, 120 s by default); when the direct read failed instead, its
usage request answer is not accepted, and it falls back to `/usage`.

`get_usage` is part of an interface that Claude Code marks as experimental,
and it may change or be removed. A change that makes its answer
unrecognisable only makes every fallback go on to `/usage`.

| Setting | Values | Default |
|---------|--------|---------|
| `AMPLIHACK_PROFILE_USAGE_PROBE` | `direct`, `auto`, `text` | unset: passive, direct, then `claude` |

| Value | Behaviour |
|-------|-----------|
| unset | The [reading order](#how-a-profile-is-probed): passive, direct, then the fallback. With `--spend`, also a [spend probe](#spend-probe) for each direct row whose account has usage credits on. |
| `direct` | Passive, then direct only. A failed direct read gives state `error`, and no `claude` is started for a reading or a [spend probe](#spend-probe); an account with usage credits on gives its direct windows with spend `not_probed`, even under `--spend`. Overflow profiles read `error` with `direct read: overflow needs /usage`. The rotator's held-cycle spend-limit check still starts `claude`. |
| `auto` | The fallback probe only, as before the direct read existed: the usage request, then `/usage`. No passive or direct reads. |
| `text` | `claude -p /usage` only, so every probe pays the transcript scan. No passive or direct reads. |

The rotator's [freshness re-reads](#re-reads) follow the same setting, with
passive readings off: under `auto` or `text` a re-read is a `claude` probe
in that mode, never a direct read.

To go back to the behaviour before direct reads, set
`AMPLIHACK_PROFILE_USAGE_PROBE=auto`. Values are matched exactly: case,
leading and trailing spaces count. An empty variable counts as unset. Any
other value prints one warning to stderr, showing the value escaped and cut
to 32 characters, then uses the default order:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_PROBE "Text" (use direct, auto or text); using the default order
```

There is no command-line flag for it. Whether a `claude` supports the usage
request is not remembered: every fallback tries it again.

To check that both methods give the same figures for your logins, see
[Compare the two probe methods](../howto/claude-profiles.md#compare-the-two-probe-methods).

#### Probe deadline

| Setting | Values | Default |
|---------|--------|---------|
| `--timeout-secs <n>` (hidden flag, `profile usage` only) | `1` to `600` | none |
| `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` | `1` to `600` | `120` |

The deadline bounds each read of one profile. A [direct read](#direct-read),
any refresh included, must finish within it, and its connect and read
timeouts are each `min(10, deadline - 1)` seconds. A fallback that follows
gets a deadline of its own, which covers the usage request and, when it
falls back, the `/usage` attempt in the time left. The default of
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

Direct reads and `claude` fallbacks have separate bounds, because a direct
read is one small HTTP request while a fallback is a `claude` process.

| Setting | Values | Default | Bounds |
|---------|--------|---------|--------|
| `AMPLIHACK_PROFILE_USAGE_DIRECT_PARALLEL` | `1` to `16` | `8` | Passive and direct reads run at once. |
| `AMPLIHACK_PROFILE_USAGE_PARALLEL` | `1` to `16` | the derived bound | `claude` fallbacks run at once. |

For the direct-read bound, an empty variable counts as unset, and any other
value prints one warning to stderr, showing the value escaped and cut to 32
characters, then uses `8`:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_DIRECT_PARALLEL "lots" (use 1 to 16); using 8
```

Unless `AMPLIHACK_PROFILE_USAGE_PARALLEL` is set, the number of `claude`
fallbacks run at once is derived from the host:

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

For `AMPLIHACK_PROFILE_USAGE_PARALLEL`, a valid value replaces the derived bound. An empty variable counts as unset,
as it does for the deadline. Any other value, including `0` and one that is
not valid UTF-8, prints one warning to stderr, showing the value escaped and
cut to 32 characters, then uses the derived bound:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL "lots" (use 1 to 16); using 2
```

The number after `using` is the derived bound from the formula above, before
the cap described next, so it can be larger than the number of profiles.

Whichever value applies is then capped at the number of profiles that need
a fallback, and is never below 1. The direct-read bound is likewise capped
at the number of profiles. There is no command-line flag for either. The
`claude` bound counts probes, not attempts: a profile's `/usage` attempt starts only after its
usage request has ended, so each probe runs at most one `claude` at a time.

#### Probe settings in the rotator

The rotator uses the same [reading order](#how-a-profile-is-probed),
[method](#probe-method), [deadline](#probe-deadline),
[parallelism](#probe-parallelism), [passive age](#passive-readings) and
[endpoint overrides](#direct-read) as `profile usage`, from the same
variables. It has no flags for them.

- **When they are read.** At the start of every cycle, from the rotator's
  own environment. That environment is fixed when the rotator starts: a
  background rotator inherits it from the shell that ran
  `amplihack profile rotate start`, or from the launch that
  [autostarted](#rotate-autostart) it. To change a setting, set the variable
  in a shell, then run `amplihack profile rotate stop` and
  `amplihack profile rotate start` from that shell. The variables the
  background rotator removes are listed under [Rotate daemon](#rotate-daemon);
  the `AMPLIHACK_PROFILE_USAGE_*` variables are not among them.
- **Warnings.** An invalid value prints its warning once per cycle (to
  `rotate.err` for a background rotator), and the default is used.
- **The live probe counts against the bound.** This applies to both bounds:
  each is capped for the candidates plus one. When the live login needs a
  `claude` fallback, the `claude` bound is derived for the candidates plus
  one. The live probe
  and the candidate probes share one pool of that size: the live probe
  starts first, and whenever a probe ends, the live one included, the next
  candidate takes its place. With a bound of 1, the live probe runs first
  and the candidates follow one at a time. Either way a cycle never runs
  more probes at once than the bound. An [API-key](#api-key-profiles) live
  login is not probed, so then the bound is derived for the candidates
  alone.
- **No spend probe.** The rotator does not run [spend probes](#spend-probe).
  A profile that is not marked as overflow and whose account has usage
  credits on is read directly and judged by its windows, with spend
  `not_probed`. Before, such a profile was read with `/usage`, so a used-up
  spend limit made it `spend_limit`; the rotator no longer sees that.
  Profiles marked as overflow are still read with `/usage`, so the
  [spend-limit block](#overflow-spend-limit-block) works as before, and the
  [held-cycle spend-limit check](#pin-and-overflow) of a pinned live
  overflow profile is unchanged.
- **Load.** The bound is derived again on every cycle, so it follows the
  load as it changes.

- **The `claude` binary.** It is looked up only when a cycle first needs a
  fallback. A `claude` that cannot be found makes only those rows fail.
- **Freshness re-reads.** A [re-read](#re-reads) of a stale swap target uses
  the same method, with passive readings off. Under `auto` or `text` it is a
  `claude` probe, never a direct read.

##### Reads that outlast a cycle

A cycle does not wait for every read to finish. The reads run on their own
threads, and the cycle waits only until `--interval` minus 2 seconds after
the cycle started, and at least 1 second. Then:

- A login whose read has not finished reads as unknown. The
  [cycle line](#rotate-log) shows it as `<name>:unknown, still running`. For
  the decision it counts as a failed probe: it is never a target, it blocks
  a move onto [overflow](#overflow-decision) as `timeout` does, and for the
  live login it counts towards [failed live probes](#rotate-failed-live-probes).
- The read keeps running. The next cycle collects its result and uses it,
  and starts no second read for that login while one is outstanding. Only
  logins with no result and no read in flight get a new read. A result
  collected this way has no
  [reading time](#rotate-reading-freshness-and-swap-back-damping), so if
  the decision chooses that login as a swap target it is read again
  directly before the swap.
- A read that has been working for `2 × deadline + 5` seconds (245
  seconds by default) is dropped, and the next cycle starts a new one. Its
  thread ends on its own HTTP or `claude` timeout. A read is working from
  when it leaves the queue of the [parallelism bound](#probe-parallelism),
  and its clock starts again when a `claude` fallback takes its slot; time
  spent queued or waiting for a slot does not count, and a read in either
  is never dropped. The direct read, any refresh included, takes at most
  one deadline, and the fallback one more, so a read still working past
  this limit has hung.

`--dry-run` waits for every read in full. When the rotator stops, it waits
up to the deadline plus 1 second for any [token refresh](#token-refresh) in
progress, so that it does not leave Claude's refresh lock behind. Once
the rotator is stopping, no new refresh starts: a read that needs one
gives `refresh busy` and takes no lock. Reads still running after that
are abandoned, and reads not yet started are not run.

Cycles never overlap: the rotator waits for `--interval` seconds after a
cycle ends before it starts the next one.

#### What is read

This section covers the `/usage` attempt; an accepted usage request is read
as in [Probe method](#probe-method). These lines of the `/usage` stdout are
read. Each one has a percentage and usually a reset time:

```
Current session: 100% used · resets Oct 16, 4:50pm (America/Los_Angeles)
Current week (all models): 68% used · resets Oct 22, 2am (America/Los_Angeles)
Current week (Fable): 0% used · resets Oct 22, 2am (America/Los_Angeles)
```

Claude leaves out the reset when no session window applies, for example
`Current session: 0% used`. Such a line at 0% means the reading has
[no session limit](#no-session-limit) when the all-models week was read. A
session line above 0% with no reset is read as a session window with no
known reset.

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

#### No session limit

Claude sometimes reports the all-models week without a running five-hour
session window. A reading then has *no session limit*: no active session
window was reported, and the profile is judged on its all-models week alone.
The report has `session_limit` set to `false` and `session` set to `null`.

The label describes the reading, not the plan. Claude also leaves out the
session reset when a session-limited login has not started a session, so an
idle Pro or Max login reads as having no session limit until a session
starts. This does not change which login is chosen: the session is at 0%,
which is not exhausted, and the next reading that has a reset restores the
session window.

A `/usage` reading has no session limit when the all-models week line was
read and either of these holds:

- there is no `Current session` line;
- the `Current session` line is at 0% and has no reset clause, for example
  `Current session: 0% used`.

```
Current week (all models): 47% used · resets Oct 22, 2am (America/Los_Angeles)
```

```
Current session: 0% used
Current week (all models): 47% used · resets Oct 22, 2am (America/Los_Angeles)
```

Both give state `ok`, `session_limit` `false` and `session` `null`. With
the week at 100% or more the state is `weekly_exhausted`.

A plan with no session limit has nothing to count in the session
percentage, so a value above 0% shows that a session window exists. The same
line at `47% used` keeps the session limit with state `ok`, and at
`100% used` gives `session_exhausted`, as before issue #1818. In both cases
`resets_text` and `resets_at` are `null`.

The rule fails closed. A session limit still applies, and the session line
is read as before, in each of these cases:

| Case | Example | Why |
|------|---------|-----|
| The session line is above 0%, with or without a reset clause | `Current session: 100% used` | A non-zero percentage shows a session window. With no reset clause `resets_text` and `resets_at` are `null`. |
| The session line has a reset clause, even one whose time cannot be resolved | `Current session: 3% used · resets 3am (Etc/Unknown)` | A printed reset shows that a session window exists. `resets_at` is `null` and `resets_text` keeps the text. |
| A line contains `Current session` but does not have the form above, for example because of a prefix | `Current session: unavailable` | A change in Claude's wording must not make an exhausted login look usable. |
| No all-models week line was read | `Current session: 0% used` alone | Nothing else can rank the profile. The state follows the usual [rules](#states). A session line with no reset clause is still read: `resets_text` and `resets_at` are `null`, and the bar view shows `-` in `S-RESET` (`SESSION RESETS` in `--table`). |
| The row is `spend_limit` or any failed state | | `session_limit` is `true` on every row that is not `ok` or `weekly_exhausted`. |

An accepted [usage request](#probe-method) has no session limit when
`rate_limits.seven_day` was read and either `rate_limits.five_hour` is
absent, or its `utilization` is `0` and its `resets_at` key is absent or
`null`. A `five_hour` above `0` with no reset keeps the session window and
the limit, and is `session_exhausted` at 100 or more. The check is on the raw
key: a `resets_at` that is present but not a valid string, not an RFC 3339
time or more than 8 days ahead still means a session limit applies, even
though the row's `resets_at` is then `null`. A `five_hour` whose
`utilization` is unusable is left out as before and also keeps the limit.

| `rate_limits` | `session_limit` | `session` |
|---------------|-----------------|-----------|
| `{"seven_day": {"utilization": 47.0, "resets_at": "2026-10-22T09:00:00Z"}}` | `false` | `null` |
| `{"five_hour": {"utilization": 0.0, "resets_at": null}, "seven_day": {…}}` | `false` | `null` |
| `{"five_hour": {"utilization": 0.0}, "seven_day": {…}}` | `false` | `null` |
| `{"five_hour": {"utilization": 100.0, "resets_at": null}, "seven_day": {…}}` | `true` | the window, with both reset fields `null` |
| `{"five_hour": {"utilization": 20.0, "resets_at": "2026-10-17T02:00:00Z"}, "seven_day": {…}}` | `true` | the window |
| `{"five_hour": {"utilization": 20.0, "resets_at": 1792202400}, "seven_day": {…}}` | `true` | the window, with both reset fields `null` |

A profile with no session limit is not exempt from limits: it is
`weekly_exhausted`, and never chosen, once its all-models week reaches 100%,
and the rotator and auto-switch apply their usual week thresholds to it. See
[Rotate decision rule](#rotate-decision-rule) and
[Candidate selection](#candidate-selection).

Known limitations:

- An idle session-limited login at 0% is labelled `no session limit`, as
  described above, in `profile usage` and `profile usage --json`.
- `rotate status` and `auto-switch status` read the stored auto-switch
  [readings](#readings), which have no field for "a session limit applies
  but its window is unknown". They therefore print `no session window`, not
  `no session limit`: a stored reading with `seven_day` and no `five_hour`.
  That happens for a reading with no session limit, and also when:
  - a status-line input had no `five_hour`, or one that was rejected (no
    `resets_at`, an unusable percentage, or a reset outside the bounds in
    [Readings](#readings));
  - a `profile usage` row that has a session limit had its session window
    rejected by the same bounds (for example no reset time), and the stored
    `five_hour` had already reset.

  Ranking is the same in every case: a missing `five_hour` counts as 0%.
  These views can also be as old as the last recorded reading.
- When `profile usage` records a reading with no session limit for a login
  whose stored auto-switch reading has a session window that has not reset,
  that window is kept until its reset passes. The new reading may come from
  figures `claude` stored earlier, and a live window contradicts it. The
  rotator and auto-switch therefore go on treating the login as it was
  (for example session-exhausted) for at most five hours. See
  [Refreshing auto-switch readings](#refreshing-auto-switch-readings).

#### Excluded profiles

A profile in the auto-switch [`exclude` list](#settings-file) is never
probed or refreshed by `profile usage`. Such a login is often shared with
another host. A token refresh from this host rotates the login's refresh
token and signs the other host out, so the only safe action is none.
Issue [#2218](https://github.com/rysweet/amplihack-rs/issues/2218) made
this rule apply to every reader.

The list is read once per command from `<root>/.auto-switch`, the file the
rotator reads, whether auto-switch is on or off. There is no second list
and no other setting. A name in the list that is not a profile under the
root is ignored, as before.

For an excluded profile, `profile usage` makes no usage request, no token
request and no identity request, does not open its `.credentials.json` or
API-key file, starts no `claude`, and runs no [spend probe](#spend-probe),
even under `--spend`.

`usage.json` is updated by loading it, changing the entries for the profiles
read and saving it again, so an entry for the excluded profile written
before it was excluded stays in the file. `profile usage` neither shows,
updates nor deletes that entry. No reader counts it as room: the
[capacity file](#capacity-writers) writers, auto-switch
[candidate selection](#candidate-selection) and the rotator apply the same
`exclude` list, and the [usage-limit wait](#usage-limit-wait) never counts an
excluded profile as a login that could take over.

The same holds for the [usage history](#usage-history). Lines written before
the profile was excluded, or before
[#2218](https://github.com/rysweet/amplihack-rs/issues/2218), can still hold
a reading for it. Those lines are neither removed nor filtered: the history
is read as written. The figures leave the profile out instead. The
[capacity summary](#capacity-summary) of `profile usage` is built from this
run's rows, which never hold a reading for it, and the capacity line of
[`rotate status`](#rotate-status) leaves the profiles in the current
`exclude` list out of `usable now`, `login-weeks of N` and `unknown` when it
builds its figures.

With no names, the excluded profile is still listed, so that the table
shows every profile under the root:

| Output | Excluded row |
|--------|--------------|
| Bar view | `excluded (not probed)` from `WEEK` to the end of `S-RESET`; `STATE` is `excluded`; no marks other than `(live)` after the name. |
| `--table` | `STATE` is `excluded`, `SESSION` is `excluded (not probed)`, every other usage cell is `-`, `SPEND` is `n/a`. |
| `--json` and `--summary-json` | `"state": "excluded"`. The keys `source`, `account`, `session`, `week_all` and `error` are present with the value `null`, `week_models` is `[]` and `spend` is `"n/a"`. No e-mail, account name or cached figure appears anywhere in the row. |
| [Capacity summary](#capacity-summary) | Not counted at all: neither in `N` nor on the `unknown` line. |
| [Usage history](#usage-history) and `usage.json` | No new reading is written for it. An older `usage.json` entry is left as it is, and is ignored as above. |

Excluded rows come after every other row; see [Ordering](#ordering). The
command exits `0` when the only difference from a full run is the excluded
rows.

The login in `~/.claude` is not read either when local files show that it
belongs to an excluded profile, by the rules under
[Local proof of an excluded live login](#local-proof-of-an-excluded-live-login).
That profile's row is the `excluded (not probed)` row, marked `(live)` in
the bar view and `live in ~/.claude` in `--table`, and its `live_in` is
`"~/.claude"`. No other row is marked live. A profile that is both live
and excluded is always treated as excluded.

With names, each excluded name is refused on stderr, and the other names are
probed, recorded and reported as usual:

```
amplihack profile usage work shared
```

```
shared: excluded (in auto-switch exclude list); not probed (amplihack profile auto-switch exclude)
PROFILE  WEEK             W-RESET      SESSION           S-RESET     STATE
work     ███░░░░░░░  30%  11-05 09:00  ██░░░░░░░░  20%   11-01 12:00 ok
shared   excluded (not probed)                                       excluded
...
```

The refused name is also shown as an `excluded (not probed)` row, as with no
names. The command exits `1` once every other name has been probed, recorded
and printed, so `profile usage work shared` reads `work`, refuses `shared` and
exits `1`. A script that wants only the readable profiles can name them, or
run with no names. A name that is not a profile under the root is
still refused as an unknown profile before anything is read, and that error
takes precedence over the exclusion.

A profile that is both excluded and an [API-key](#api-key-usage) or
[overflow](#rotate-overflow) profile is shown as excluded. Neither kind is
probed.

When `<root>/.auto-switch` is a private file of yours that parses but is
invalid for its other settings (an unknown kind, say), its exclude list
still applies, and the command prints once on stderr:

```
warning: auto-switch config invalid; exclude list applied, other settings ignored
```

When the file exists but cannot be read, does not parse, or is unsafe
(owned by another user, or writable by group or others), the command
cannot tell which profiles are excluded, so it treats every profile as
excluded: none is probed or refreshed. It prints once on stderr:

```
warning: auto-switch config unreadable or unsafe; every profile treated as excluded, none probed or refreshed
```

The rotator and [`profile verify`](#profile-verify) do the same; see
[Unreadable exclude list](#unreadable-exclude-list), which also says how to
fix the file without losing the exclusions.

#### States

Every profile gets exactly one state:

| State | Meaning |
|-------|---------|
| `excluded` | The profile is in the auto-switch `exclude` list and was not probed. See [Excluded profiles](#excluded-profiles). |
| `ok` | The session or the all-models week was read, from a passive reading, a direct read, an accepted usage request or a `/usage` line, and neither is at 100%. |
| `session_exhausted` | The session is at 100% or more and a session limit applies. A reading with [no session limit](#no-session-limit) never has this state. |
| `weekly_exhausted` | The all-models week is at 100% or more and the session is not. |
| `spend_limit` | The `/usage` output reported that the organisation's spend limit was reached. Any percentages read are kept. |
| `not_logged_in` | The `/usage` output had no usage line of any kind (model week lines included) and reported that the profile is not signed in. Run `amplihack profile login <name>`. A profile [live in `~/.claude`](#logins-in-claude) is probed against `~/.claude`, so its missing `.credentials.json` does not give this state. |
| `timeout` | The fallback passed its [deadline](#probe-deadline), in either attempt. The account's usage is unknown. A direct read that times out falls back instead, except under `direct`, where it gives `error`. |
| `unparsed` | The `/usage` output had nothing recognisable. Run again with `--raw` to see what Claude said. |
| `error` | The probe could not run. The reason goes to stderr in both modes, and the JSON also has it in `error`. |

When several apply, the first in this order wins: `excluded`, `error`, `timeout`,
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

#### Spend status

Every row has a spend status, shown in the `SPEND` column of the
[table](#table-output), as the `spend` key of the [JSON](#json-output), and,
when it is not `n/a`, as a `spend:<value>` mark in the
[bar view](#bar-output).
It says whether `/usage` reported a used-up spend limit for the row. It is
for reading only: [ordering](#ordering), the [states](#states) and the
rotator's decisions do not use it.

| Value | Rows |
|-------|------|
| `n/a` | No `/usage` text was read for spend, and none was needed: a direct row whose account has usage credits off, a [passive](#passive-readings) row, an [API-key](#api-key-usage) row, and a [fallback](#fallback-probe) row given by an accepted `get_usage` answer, which is accepted only when credits are off. |
| `ok` | `/usage` text was read, gave a usage reading, and did not report a used-up spend limit: a [spend probe](#spend-probe) whose output parsed as `ok`, `session_exhausted` or `weekly_exhausted`, or a fallback row from `/usage` in one of those states. |
| `limit_reached` | `/usage` text reported a used-up spend limit: a spend probe whose output parsed as `spend_limit`, or a fallback row in state `spend_limit`. |
| `unknown` | A probe ran, or was asked for, and the spend status could not be read: a direct row with usage credits on whose `--spend` probe was killed at the [probe deadline](#probe-deadline), could not run (including `claude` not found), or gave output that parsed as `not_logged_in` or `unparsed`; and every fallback row in state `not_logged_in`, `unparsed`, `timeout` or `error`. |
| `not_probed` | Usage credits are on and no spend probe was asked for: a direct row with usage credits on in a run without `--spend`, or under `AMPLIHACK_PROFILE_USAGE_PROBE=direct`. The rotator's rows are always `not_probed`. |

`n/a` does not mean that usage credits are off. A passive row says nothing
about credits. `ok` is never guessed: output that `claude` printed but that
gave no usage reading, such as a login prompt, gives `unknown`, not `ok`.
A row whose state is `ok` and whose spend is
`limit_reached` has room in its plan windows, but the account's spend limit
is used up, so it cannot work past them on credits.

#### Failed and exhausted results

A probe that timed out or could not run says nothing about the account. Its
usage is unknown, and it is never reported or treated as 100%. An exhausted
result is a reading: the account answered, and a window is at 100% or more.

| Result | `profile usage` table | `--json` | Rotator cycle line | Rotator decision | Next probe |
|--------|-----------------------|----------|--------------------|------------------|------------|
| `timeout` | state `timeout`, `-` in every percentage and reset | `"state": "timeout"`, `session`, `week_all` and `error` `null`, `week_models` empty | `<name>:failed` | Unknown: never a target, never counted as exhausted; a plan candidate in this state blocks a move onto [overflow](#overflow-decision) | Next run or cycle; no failed result is kept |
| `error` | state `error`, `-` in every percentage and reset; reason on stderr | `"state": "error"`, `session` and `week_all` `null`, `week_models` empty, reason in `error` | `<name>:failed` | As for `timeout`, `profile directory refused` included (see [Overflow decision](#overflow-decision)) | As for `timeout` |
| `session_exhausted`, `weekly_exhausted` | the percentages read | the windows read | `<name>:s=<n>%,w=<n>%`, the windows read; see [Exhausted figures](#rotate-exhausted-figures) | Headroom `0`: never a target | Next run or cycle |
| Still running (rotator only) | - | - | `<name>:unknown, still running` | As for `timeout` | Its result is used by the next cycle; see [Reads that outlast a cycle](#reads-that-outlast-a-cycle) |

`spend_limit`, `not_logged_in` and `unparsed` are also shown as
`<name>:failed` in the cycle line and are never a target; see
[Decision rule](#rotate-decision-rule). Unlike `timeout` and `error`, they do
not block a move onto overflow, so a profile that is never logged in cannot
keep the rotator off credits for good. A failed live probe is handled as in
[Failed live probes](#rotate-failed-live-probes). Neither `profile usage` nor
the rotator records a `timeout` or `error` result as an auto-switch
[reading](#readings), or as exhausted anywhere.

<a id="rotate-exhausted-figures"></a>

#### Exhausted figures

An exhausted reading keeps both windows the probe read. The rotator does not
use them to decide: an exhausted profile has headroom `0` and is never a
target, whatever the figures are. They are kept so that the rotate log, and
anything that reads weekly room from it, sees the weekly use read for a login
that has only reached its session limit.

| Window | Shown as |
|--------|----------|
| The session | The value read, rounded down; for `session_exhausted` never below `100%`. A missing window shows `100%` for `session_exhausted`, the window that ran out, and `0%` for `weekly_exhausted`, as in the [decision rule](#rotate-decision-rule), so a login with [no session limit](#no-session-limit) shows `s=0%` whether or not its week has run out. |
| The all-models week | The value read, rounded down; for `weekly_exhausted` never below `100%`. A missing window shows `w=?` for `session_exhausted` and `100%` for `weekly_exhausted`. |

| Probe result | Session read | Week read | Cycle line |
|--------------|--------------|-----------|------------|
| `session_exhausted` | 100% | 62% | `<name>:s=100%,w=62%` |
| `session_exhausted` | 100% | none | `<name>:s=100%,w=?` |
| `weekly_exhausted` | 30% | 99.6% | `<name>:s=30%,w=100%` |
| `weekly_exhausted`, [no session limit](#no-session-limit) | none | 100% | `<name>:s=0%,w=100%` |
| `session_exhausted` | 98% | 40% | `<name>:s=100%,w=40%` |

The same figures are used in the `live` and `target` parts of a `swap` line
and in the `live=` part of an `ALERT: no profile has capacity` line. A
login at its weekly limit with no session window shows `s=0%`, the same
figure it shows before the week runs out; `s=100%` always means a session
limit was reached.

The rotator writes one line to stderr (`rotate.err` for a background
rotator) for each `timeout` or `error` result, with `~/.claude` as the name
of the live login. Both are fixed text:

```
warning: rotate: probe of <name> timed out
warning: rotate: probe of <name> failed: <reason>
```

A read that panics, which is a bug, gives no result and reads failed for
that cycle; the rotator writes one line for it and reads that login again
in the next cycle:

```
warning: rotate: read of <name> panicked; no result
```

The reasons for `error` are fixed text, never the underlying I/O error. A
failure to start, write to, read or wait for the usage request is not an
`error`: it falls back to `/usage`. So the `claude` reasons below come from
the `/usage` attempt.

| Reason | When |
|--------|------|
| `profile directory refused` | The profile cannot be resolved, or its directory fails the check just before a read or a spawn. |
| `direct read: <reason>` | Only under `AMPLIHACK_PROFILE_USAGE_PROBE=direct`: the direct read failed with one of the [fixed reasons](#direct-read-failures). |
| `claude not found` | A fallback was needed and the `claude` binary could not be found. |
| `could not start claude` | Spawning `claude` failed. |
| `could not read claude output` | Reading the child's stdout or stderr failed, or a pipe was still open 2 seconds after the child exited (a process that left its group still held it), so the output may be cut short. |
| `could not wait for claude` | Waiting for the child failed. It is killed and reaped first. |

#### Bar output

By default, `profile usage` prints one row per profile with the weekly and
session usage drawn as bars:

```
PROFILE      WEEK             W-RESET      SESSION           S-RESET     STATE
review       █░░░░░░░░░  10%  11-05 09:00  ░░░░░░░░░░   5%   11-01 14:00 ok
work (live)  ███░░░░░░░  30%  11-05 09:00  ██░░░░░░░░  20%   11-01 12:00 ok                 * hosts:personal
weekly       ████░░░░░░  47%  11-05 09:00  no session limit  -           ok
credit       ██████░░░░  61%  11-06 02:00  ████░░░░░░  40%   11-01 21:00 ok                 overflow (fallback) spend:ok
team         █████░░░░░  55%  11-05 09:00  ██████████ 100%   11-01 23:50 session_exhausted
api          api-key (billed per token)                                  api_key
personal     no reading (not_logged_in)                                  not_logged_in      leased→work spend:unknown
spare        no reading (timeout)                                        timeout            (fallback) spend:unknown
times: Europe/London
order: rotator pick (most room in the worse of session and week, then name)
```

The columns are:

| Column | Content |
|--------|---------|
| `PROFILE` | The profile name, followed by `(live)` when the profile's login is the one in `~/.claude` (the condition for the `live in ~/.claude` [mark](#marks)). |
| `WEEK` | The all-models week as a [bar](#bars), or `-` when the row has no week reading. |
| `W-RESET` | When the all-models week resets, as a [reset time](#reset-times). |
| `SESSION` | The session as a bar; `no session limit` for a reading with [no session limit](#no-session-limit); `-` when the session window is missing. |
| `S-RESET` | When the session resets; `-` for a row with no session limit. |
| `STATE` | The [state](#states). |
| (no heading) | Every other [mark](#marks) (`*`, `leased→<dir>`, `hosts:<account>`, `overflow`, `overflow, blocked until <time>`, `(fallback)`), then `spend:<value>` when the row's [spend status](#spend-status) is not `n/a`, separated by spaces. Empty when there are none. |

Rows without a reading have no bars and no reset cells. From `WEEK` to the
end of `S-RESET` they show one message:

| Row | Message |
|-----|---------|
| [API-key](#api-key-usage) (`api_key`) | `api-key (billed per token)` |
| `not_logged_in`, `timeout`, `unparsed`, `error` | `no reading (<state>)`, for example `no reading (timeout)` |
| [Excluded](#excluded-profiles) (`excluded`) | `excluded (not probed)` |

The `STATE` cell and the marks are still shown on these rows. Rows in state
`ok`, `session_exhausted`, `weekly_exhausted` and `spend_limit` show their
bars as read, so a window at its limit shows a full red bar.

The `PROFILE` column is as wide as the longest label, up to 31 characters.
A name longer than 24 characters is cut to its first 23 characters followed
by `…`, and `(live)` follows the cut name. The header line, and a row in
state `ok` with no marks, fit in 100 columns for any name, in both local and
UTC time. Longer state
names and the marks column can take a row past 100 columns. Names, accounts
and other text from files are [cleaned](#table-output) before they are
printed, as in the table.

After the rows come, in this order: the `--raw` blocks, the
`running on overflow:`, `live:` and `blocked:` lines described under
[Marks](#marks) and [API-key usage](#api-key-usage), then `times:` and
`order:`. With no profiles there are no rows and no footer; the command
prints the same line as `--table`.

The layout is for people and may change; scripts should use `--json`.

##### Bars

A bar is 10 cells of `█` (used) and `░` (free), one space and the
percentage, right-aligned in 4 characters:

| Used | Cell |
|------|------|
| 0% | `░░░░░░░░░░   0%` |
| 13% | `█░░░░░░░░░  13%` |
| 41.9% | `████░░░░░░  41%` |
| 55% | `█████░░░░░  55%` |
| 100% | `██████████ 100%` |
| 130% | `██████████ 130%` |

The percentage is printed as a whole number, rounded down, so every bar
cell is 15 characters wide and the columns stay aligned. 41.2% and 41.9%
both read `41%`, and 99.6% reads `99%`, so `100%` appears only when the
window has reached its limit. `--table` and `--json` keep the decimal
place (`12.5%`). The number of filled cells is the percentage divided by
10 and rounded down, between 0 and 10. A percentage above 100 fills the
bar and is printed rounded down in the same way.

The bars and the percentage show the reading as it was probed, even when
its reset time has passed. [Ordering](#ordering) counts such a window as 0%.

##### Colour

When stdout is a terminal, each bar and its percentage are coloured by the
percentage as read, before rounding. Because the printed figure is rounded
down, it always falls in the same band as the colour:

| Used | Colour |
|------|--------|
| Below 70% | Green |
| 70% to below 90% | Yellow |
| 90% and above | Red |

Only the bar and the percentage are coloured; the header, labels, states,
marks and footer lines never are. Set `NO_COLOR` to any non-empty value to
turn colour off, as described at [no-color.org](https://no-color.org/):

```bash
NO_COLOR=1 amplihack profile usage
```

When stdout is a pipe or a file, no colour is used and the output contains
no escape sequences. Column widths are the same with and without colour.
`--table` and `--json` are never coloured.

##### Reset times

A reset cell reads `MM-DD HH:MM` in local time, the same width on every
row. A window with no reset time, or one whose reset amplihack could not
read as a time, shows `-`. The time zone is named once, in the `times:`
line at the end:

| Source of the zone | Used when |
|--------------------|-----------|
| `TZ` | It is set, not empty, and names an IANA time zone, such as `TZ=Asia/Tokyo` (a leading `:` is ignored). |
| The system time zone | `TZ` is unset or empty and the system zone has an IANA name. |
| The UTC offset, as `UTC+HH:MM` or `UTC-HH:MM` | Neither has a usable name, or `TZ` is set to something other than an IANA name, such as a POSIX rule (`TZ=JST-9`) or a file path. The times follow such a `TZ`, so the system zone's name would not match them. |

Each time is converted with its own offset, so a reset after a change to or
from summer time is shown in the offset that applies on that day.

With `--utc`, the cells read `MM-DD HH:MMZ` and the footer reads
`times: UTC`. A reset at 23:30 UTC on 5 November reads `11-05 23:30Z` with
`--utc`, `11-06 01:30` at UTC+02:00 and `11-05 22:30` at UTC-01:00:

```bash
amplihack profile usage --utc
```

```
PROFILE      WEEK             W-RESET       SESSION           S-RESET      STATE
review       █░░░░░░░░░  10%  11-05 09:00Z  ░░░░░░░░░░   5%   11-01 14:00Z ok
work (live)  ███░░░░░░░  30%  11-05 09:00Z  ██░░░░░░░░  20%   11-01 12:00Z ok                 * hosts:personal
weekly       ████░░░░░░  47%  11-05 09:00Z  no session limit  -            ok
credit       ██████░░░░  61%  11-06 02:00Z  ████░░░░░░  40%   11-01 21:00Z ok                 overflow (fallback) spend:ok
team         █████░░░░░  55%  11-05 09:00Z  ██████████ 100%   11-01 23:50Z session_exhausted
api          api-key (billed per token)                                    api_key
personal     no reading (not_logged_in)                                    not_logged_in      leased→work spend:unknown
spare        no reading (timeout)                                          timeout            (fallback) spend:unknown
times: UTC
order: rotator pick (most room in the worse of session and week, then name)
```

Reset times taken from Claude's `/usage` text, such as
`Oct 16, 4:50pm (America/Los_Angeles)`, are shown in the same numeric form
when amplihack could resolve them to a time, as it does for
[`resets_at` in the JSON](#json-output). `--table` still shows the text as
Claude printed it.

#### Table output

`--table` prints the eight-column table that was the default output before
the [bar view](#bar-output). Its columns and cells are unchanged; only the
row order follows the [ordering](#ordering) rule, and the table ends with
the `order:` line. It has no `times:` line, because its reset columns show
the reset as received.

```bash
amplihack profile usage --table --spend
```

```
PROFILE   MARKS                STATE              SESSION           SESSION RESETS                     WEEK  WEEK RESETS                        SPEND
review    -                    ok                 5%                2026-10-17T04:00:00Z               10%   2026-10-22T09:00:00Z               n/a
work      * hosts:personal     ok                 20%               2026-10-17T02:00:00Z               30%   2026-10-22T09:00:00Z               n/a
weekly    -                    ok                 no session limit  -                                  47%   2026-10-22T09:00:00Z               n/a
mistt     -                    ok                 35%               2026-10-17T03:00:00Z               52%   2026-10-22T09:00:00Z               ok
credit    overflow (fallback)  ok                 40%               Oct 16, 9pm (America/Los_Angeles)  61%   Oct 22, 2am (America/Los_Angeles)  ok
org2      -                    session_exhausted  100%              2026-10-16T22:10:00Z               71%   2026-10-22T09:00:00Z               limit_reached
team      -                    session_exhausted  100%              2026-10-16T23:50:00Z               55%   2026-10-22T09:00:00Z               n/a
personal  leased→work          not_logged_in      -                 -                                  -     -                                  unknown
spare     (fallback)           timeout            -                 -                                  -     -                                  unknown
order: rotator pick (most room in the worse of session and week, then name)
```

`credit` is marked as overflow, so it was probed with `/usage` and shows
Claude's own reset text. `team` is at its session limit and has used 55%
of its week; the weekly figure is shown as reported. `spare` failed its
direct read and its `claude` fallback timed out. The other rows came from
passive or direct reads. `mistt` and `org2` have
[usage credits on](#usage-credits-on): their windows come from the direct
read, and, because this run used `--spend`, the `SPEND` column comes from a
[spend probe](#spend-probe) under the normal per-probe timeout, which found `org2`'s spend limit used up.
Without `--spend` both would show `not_probed`. `SPEND` is the
[spend status](#spend-status).

`SESSION` and `WEEK` are the session and all-models week. Percentages print
as a whole number when they are whole and with one decimal place otherwise
(`12.5%`). `-` means no value. A `timeout` or `error` row shows `-` in every
percentage and reset column: its usage is unknown. Model-specific week lines
are not in the table; they are in the JSON.

A row in state `ok` or `weekly_exhausted` with
[no session limit](#no-session-limit) shows `no session limit` in full in
`SESSION`, never shortened, and `-` in `SESSION RESETS`. The label means
that no active session window was reported; the profile is ranked on its
week. Every other row keeps its usual cells: `timeout`, `error`,
`unparsed`, `not_logged_in`, `spend_limit` and
[API-key](#api-key-usage) rows never show the label.

Reset columns show the reset as received, so their form depends on the
[attempt](#probe-method) that gave the row:

| Row from | Reset column | Example |
|----------|--------------|---------|
| A passive reading | The stored reset time, as `YYYY-MM-DDTHH:MM:SSZ` | `2026-10-16T23:50:00Z` |
| A direct read | The `resets_at` string the endpoint sent, as for the usage request | `2026-10-16T23:50:00Z` |
| The usage request | The `resets_at` string `claude` sent, an ISO 8601 time; `-` when it sent none or not a string | `2026-10-16T23:50:00Z`, an example of the form ([details](#probe-method)) |
| `/usage` | Claude's text as printed | `Oct 16, 4:50pm (America/Los_Angeles)` |

Text from Claude and from state files is cleaned before it reaches the
terminal: control characters (including escape sequences) and the
bidirectional override and isolate characters U+202A to U+202E and U+2066
to U+2069 are replaced with `?`, and reset
text longer than 64 characters is cut to 64 and followed by `…`. The table's
layout is for people and may change; scripts should use `--json`. The
table has eight columns; `SPEND` was added as the last. A script that
splits rows on whitespace and reads the last field now gets the spend
status instead of `WEEK RESETS`, and the reset columns can themselves
contain spaces. Use `--json` instead.

For each `error` row, one line goes to stderr, with or without `--json`:

```
warning: profile "review": could not start claude
```

With `--raw`, each row whose direct read failed and that went on to
`claude` also prints one `note:` line to stderr, as shown under
[Direct read failures](#direct-read-failures), and each direct row whose
account has usage credits on prints two, as shown under
[Usage credits on](#usage-credits-on).

#### Raw output

With `--raw`, a row read passively or directly is followed by one fixed
line, because no `claude` ran for it. The examples here use `--table`; the
bar view prints the same lines under its rows:

```
review    -                 ok                 5%                2026-10-17T04:00:00Z  10%   2026-10-22T09:00:00Z
  (direct: no claude output)
```

A passive row prints `(passive: no claude output)`. The body of a direct
answer is never printed. A direct row whose account has usage credits on
prints the same `(direct: no claude output)` line: the output of its
[spend probe](#spend-probe) is not shown. In the JSON, such a row's `raw` is
`{"stdout": "", "stderr": ""}`.

A fallback row is followed by what `claude` printed in the attempt
that gave the row, indented and cleaned of control characters other than
newlines. Each stream is shown up to the 64 KiB read for that attempt. For
a row from the usage request, that is the attempt's whole stdout and stderr:
every JSON line `claude` printed, from the `system` line it prints when it
starts, through the `control_response` answer, to any lines after it. For a
row from `/usage`, it is the text, as in this run with
`AMPLIHACK_PROFILE_USAGE_PROBE=text`:

```
team      (fallback)        session_exhausted  100%     Oct 16, 4:50pm (America/Los_Angeles)  68%   Oct 22, 2am (America/Los_Angeles)
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

Rows are sorted by the [rotator's headroom](#rotate-decision-rule): the
room left in the worse of the session and the all-models week. The bar
view, `--table` and `--json` use the same order, and the bar view and
`--table` end with a line that states the rule:

```
order: rotator pick (most room in the worse of session and week, then name)
```

Rows are in five groups:

1. **Usable** (`ok`): most headroom first, then by name. Headroom is the
   rotator's: `100 - max(session, week)` for the all-models week, rounded
   down to a whole point. A missing session window, including
   [no session limit](#no-session-limit), counts as 0%, so a weekly-only
   profile is ranked by its weekly room. Reset times break no ties: two
   profiles at 41.2% and 41.9% both have headroom 58 and are sorted by name.
   After every row with a headroom come the `ok` rows with no all-models
   week reading, by name. The rotator treats such a reading as a failed
   probe and never picks it.
2. **Exhausted** (`session_exhausted`, `weekly_exhausted`, `spend_limit`):
   soonest back first. A profile is back when every window at 100% or more
   has reset, so "back at" is the latest of those reset times. If any of
   those windows has no `resets_at`, the back-at time is unknown, and the
   profile comes after the others. A `spend_limit` row's back-at time is
   always unknown, because the window resets do not lift a spend limit.
   Then by name.
3. **API key** (`api_key`): by name.
4. **Unavailable**: `not_logged_in`, then `timeout`, then `unparsed`, then
   `error`, each by name.
5. **Excluded** (`excluded`): by name. See
   [Excluded profiles](#excluded-profiles).

Only the headroom rule is applied. The rules that decide which profiles
the rotator may pick at all are not: the [overflow tiers](#overflow-tiers),
the auto-switch `exclude` list and the other skip reasons under
[Rotate candidates](#rotate-candidates), and the thresholds in
[Rotate settings](#rotate-settings) do not change a row's place. The first
row is therefore not always the login the rotator would pick next. An
overflow profile at 20% of its session and 30% of its week (headroom 70) is
listed above a plan login at 50% of its week (headroom 50), although the
rotator moves to the plan login first and uses the overflow profile only as
described under [Overflow decision](#overflow-decision). An excluded
profile is not probed, so it has no headroom and is listed last.

The headroom used for sorting also differs from the rotator's in one
respect. A window whose reset time has already passed counts as 0% used
when the rows are sorted, because the quota it reports has since been
restored; the bars and percentages still show the reading as probed.
`profile usage` reads every profile afresh, so this happens only for a
reading taken just before a reset.

The `live` row, the default profile and overflow profiles are sorted by
their state and numbers like any other row.

The [spend status](#spend-status) plays no part in the order.

#### Marks

| Mark | Meaning |
|------|---------|
| `*` | The default profile, as in `list`. |
| `(fallback)` | The row came from `claude`, not from a passive or direct read. See [Reading source](#reading-source). |
| `leased→<dir>` | This profile's login is live in profile `<dir>`'s directory, lent by [auto-switch](#leases). |
| `hosts:<account>` | This profile's directory currently holds the login of profile `<account>`, so the numbers on this row belong to `<account>`. |
| `live in ~/.claude` | This profile's login is in the default directory `~/.claude`, and the numbers on this row were read from there. See [Logins in `~/.claude`](#logins-in-claude). |
| `overflow` | The profile is marked as [credit-backed overflow](#rotate-overflow) in `<root>/.auto-switch`. |
| `overflow, blocked until <time>` | As `overflow`, and the rotator has [blocked](#overflow-spend-limit-block) it after a spend-limit reading until `<time>` (UTC, `YYYY-MM-DDTHH:MM:SSZ`). |

Several marks are separated by a space (`* live in ~/.claude`,
`overflow (fallback)`); none prints `-`. This is the `MARKS` column of
`--table`. The [bar view](#bar-output) shows `live in ~/.claude` as
`(live)` after the profile name instead, and the other marks in the
unlabelled column after `STATE`, which is empty when there are none. A row marked `live in ~/.claude` is placed by its state and
numbers like any other row. An overflow row is placed by its state like any
other row too; its session and week numbers are its plan limits, which it can
work past on credits.

When the row marked `live in ~/.claude` is also marked `overflow`, and that
row's own probe in this run reached the session or weekly limit (state
`session_exhausted` or `weekly_exhausted`, or the session or all-models
weekly window at 100%), the rows are followed by one line, in the bar view
and in `--table`:

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
   absent. An open [hand-change suspicion](#two-readings) does not change
   which profile the ledger names here: `profile usage` makes no identity
   reads and does not compare `~/.claude.json` with the ledger. The
   duplicate-login warning for `~/.claude` and a profile `M` is still
   printed while both logins are the same account. Both ledgers are only read. The deciding ledger's `live` field names the profile
   and its `since` is used only for the
   [check after the probes](#swaps-during-the-probe); other fields are ignored. The checks run in this order, and the first that fails decides:

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

   The ledger is taken at its word: amplihack does not compare the account in
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

After every window reading has finished, and before any `--spend`
[spend probe](#spend-probe) starts, the same evidence is checked again: the
ledger's `live` and `since` fields, or the matched account. A long
`--spend` pass therefore never widens the time in which a swap stops the
live profile's windows from being recorded. `since` is only
compared for equality; it is never printed or stored. If the evidence
changed or can no longer be read, the login in `~/.claude` may have been
swapped during the probe. The row is still shown, with its numbers and its
mark, but its usage is [not recorded](#refreshing-auto-switch-readings)
(the other rows are recorded as usual), and one note goes to stderr. The
profile name in it is cleaned of control characters:

```
note: the login in ~/.claude changed while profile "work" was probed; its usage was not recorded this run
```

This is a note, not a warning. The command only sees that `~/.claude` no
longer holds the live profile's login; the change may be a rotator swap or a
`/login` in `~/.claude`, and the note does not say which. A rotator swap
needs no fix; a login that is not the profile's
[recorded account](#recorded-account) is reported by its own warning. The command does not retry the probe, and the exit
status is unchanged. A run with no swap during the probes prints no note.

On such a row, the numbers and any `--raw` text may belong to the login that
replaced it. Run the command again for current numbers.

The live profile's spend probe reads `~/.claude`, so with `--spend` the
evidence is checked again before it starts and once it has finished. If
either check fails, the probe's result is dropped, the row's spend is
`unknown` rather than the spend of the login that replaced it, and one note
goes to stderr. The row's windows were read before the change and are
recorded as usual:

```
note: the login in ~/.claude changed while profile "work" was probed for its spend; its spend is unknown this run
```

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
| `note: the login in ~/.claude changed while profile "<name>" was probed; its usage was not recorded this run` | The evidence for the live profile changed during the probe. A note rather than a warning. See [Swaps during the probe](#swaps-during-the-probe). |
| `note: the login in ~/.claude changed while profile "<name>" was probed for its spend; its spend is unknown this run` | With `--spend`, the evidence for the live profile changed before or during its spend probe. Its spend is `unknown`; its windows are recorded. See [Swaps during the probe](#swaps-during-the-probe). |
| `warning: profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); …` | Two logins are the same account. One line per pair, after the other warnings. The line names the profile whose login is not its [recorded account](#recorded-account), or `~/.claude` when it is not the live profile's recorded account, or says amplihack cannot tell which one is wrong. See [Duplicate-login warning](#duplicate-login-warning). Printed for `~/.claude` and `M` while a manual `/login` in `~/.claude` has left both with the same account. The rotator moves `M`'s copy aside only after [two matching credential readings](#rotate-hand-change-recovery), and only when no profile is left with no login. |

A manual `/login` in `~/.claude` replaces whatever login was there; the
profile that login belonged to is left with no login. `profile usage` prints
no warning of its own about that; the rotator does, in its log and in
[`rotate status`](#rotate-status).

#### Codex accounts

When no profile names are given, `profile usage` also reads every Codex
account and prints one row for each after the Claude output. Each account
is read live, on every run, by asking Codex itself; nothing is taken from
session files or any earlier reading. An account that cannot be read shows
why, never an older value.

```bash
amplihack profile usage
```

```
PROFILE      WEEK             W-RESET      SESSION           S-RESET     STATE
review       █░░░░░░░░░  10%  11-05 09:00  ░░░░░░░░░░   5%   11-01 14:00 ok
work (live)  ███░░░░░░░  30%  11-05 09:00  ██░░░░░░░░  20%   11-01 12:00 ok                 * hosts:personal
times: Europe/London
order: rotator pick (most room in the worse of session and week, then name)

Codex accounts under /home/ryan/.codex-profiles
ffarmer   signed in      5h █░░░░░░░░░  12% 11-06 21:40 in 2h 31m  7d ████░░░░░░  41% 11-12 08:00 in 132h 51m  plan plus  read 19:09:00
ggrant    signed in      7d ███░░░░░░░  34% 11-10 14:30 in 91h 21m  plan pro  read 19:09:00  <- next
hhale     signed in      credits 4200  plan team  read 19:09:00
jjones    not signed in  read 19:09:00
kkim      timed out      after 20s  read 19:09:00
mmoss     error: codex app-server exited (exit status: 1): error: unrecognized subcommand 'app-server'  read 19:09:00
~/.codex  signed in      5h ████████░░  88% 11-06 20:15 in 1h 6m  7d ███████░░░  70% 11-09 03:00 in 55h 51m  plan plus  read 19:09:00
```

Codex rows never change the Claude rows, the
[capacity summary](#capacity-summary), the [usage history](#usage-history),
the [auto-switch readings](#refreshing-auto-switch-readings) or the
[exit status](#exit-status-and-refusals). With profile names given
(`amplihack profile usage work review`), Codex is not read and no Codex
block is printed; `--summary-json` still has the `codex` key, as an empty
array. [`--json`](#json-output) never reads Codex and has no Codex rows.

##### Which accounts are read

| Account | Label | When it is read |
|---------|-------|-----------------|
| Each subdirectory of the Codex profiles root, in byte order of its name | The directory name | Always. Names starting with `.` are skipped, as `codex-pick`'s `*` glob skips them. A symlink to a directory is followed. |
| `$HOME/.codex` | `~/.codex` | When it is a directory and its canonical path is neither the root nor any profile directory's canonical path. It is read with the others and shown last, and only when it is signed in: a `not signed in` result drops the row. A `timed out` or `error` result is shown. |

The Codex profiles root is `CODEX_PROFILES_ROOT` when it is set and not
empty, otherwise `$HOME/.codex-profiles`. A root that is missing, is not a
directory or has no subdirectories is not an error. The block is then one
line, and `~/.codex` is still read:

```
Codex: no accounts under /home/ryan/.codex-profiles
```

When `~/.codex` is signed in and the root has no accounts, the header line
is printed with the `~/.codex` row under it.

A root that exists but cannot be listed (for example, permission denied)
prints one line on stderr. `~/.codex` is then not read either, and the block
is the single `Codex: no accounts under` line:

```
warning: cannot list Codex accounts under /home/ryan/.codex-profiles: Permission denied (os error 13)
```

The exit status does not change, and `codex` in `--summary-json` is an
empty array.

##### Live read

On Unix, for each account, amplihack starts one Codex app server with the
account's directory as its home and asks it for the account's rate limits.
The program is `CODEX_BIN` when it is set and not empty, otherwise `codex`
found through `PATH`:

```
CODEX_HOME=<account directory> codex app-server
```

and writes three JSON-RPC lines to its stdin:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"amplihack","version":"<amplihack version>"}}}
{"jsonrpc":"2.0","method":"initialized"}
{"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read","params":{}}
```

No model call is made, so the read uses no quota. Answers are matched by
`id` only; lines without an `id`, such as the `remoteControl/status/changed`
notification Codex sends first, are skipped, and a `"jsonrpc"` field is not
required. An answer to `account/rateLimits/read` reads, for example:

```json
{
  "id": 2,
  "result": {
    "rateLimits": {
      "primary": { "usedPercent": 12, "windowDurationMins": 300, "resetsAt": 1794001200 },
      "secondary": { "usedPercent": 41, "windowDurationMins": 10080, "resetsAt": 1794470400 },
      "credits": null,
      "planType": "plus",
      "rateLimitReachedType": null,
      "ordinaryUsageAllowed": true
    }
  }
}
```

| Field | Read from | Rule |
|-------|-----------|------|
| Windows | `primary`, `secondary` | Each that is an object. A missing or `null` one is not shown. |
| Used percentage | `usedPercent` | A JSON number, or a string of digits with an optional decimal point, that is finite and not negative. Anything else counts as absent. Not clamped. |
| Window length | `windowDurationMins` | A non-negative whole number of minutes. |
| Reset | `resetsAt` | Unix epoch seconds; see [Codex reset times](#codex-reset-times). |
| Credits | `credits` | `hasCredits` counts only as the JSON value `true`; `balance` follows the same number rule as the used percentage. |
| Plan | `planType` | A string, cut to 200 characters. |
| Limit reached | `rateLimitReachedType` | A string, cut to 200 characters; JSON only. |
| Ordinary usage allowed | `ordinaryUsageAllowed` | A JSON bool; JSON only. |

The figures may sit in `result.rateLimits`, `result.rate_limits` or in
`result` itself. Every field is also accepted in snake_case
(`used_percent`, `window_duration_mins`, `resets_at`, `has_credits`,
`plan_type`, `rate_limit_reached_type`, `ordinary_usage_allowed`). A
`result` that is not an object gives `error: unexpected response from codex
app-server`, so a reply amplihack does not understand never appears as a
reading.

All accounts are read at once, one thread each, alongside the Claude
probes, so the whole Codex read normally takes about as long as the slowest
account. There is no cap on the number read at once.

##### Codex read deadline

Each account has one deadline that covers starting the program, the
handshake and the answer: 20 seconds, or
`AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS`, a whole number from `1` to `300`. Any
other value prints one warning on stderr and uses 20:

```
warning: AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS="0" is not a whole number from 1 to 300; using 20
```

The last 3 seconds of the deadline (or the last quarter, for a deadline
under 12 seconds) are kept for stopping the app server. The answer must
arrive before that part starts; if it does not, the row reads `timed out`.

The app server may be renewing the account's login when the read ends. A
renewal stopped after the old refresh token is retired but before the new
one is written leaves the account signed out. So on every outcome,
including success and `timed out`, the app server is stopped in this order:

1. Its stdin is closed. The app server exits on its own when its stdin
   ends (`codex` 0.166.0 exits with status 0 about half a second later),
   so a renewal under way can finish and be written.
2. If it has not exited by the middle of the stop time, its process group
   gets `SIGTERM`.
3. If it has still not exited when the deadline passes, the group gets
   `SIGKILL`.

Each step waits for the exit itself (a blocking `waitid` reported on a
channel), not on a polling loop. After the exit the whole process group is
killed and the process is reaped, so no Codex process outlives the command.
A read that got its answer early waits at most the 3-second stop time for
the exit, not the rest of the deadline.

Only an app server that ignores both the end of its stdin and `SIGTERM`
until the deadline is killed outright. If that one was renewing the login
at that moment, the account may have to sign in again; run `codex login`
with `CODEX_HOME` set to that account directory.

The live read needs process groups, so it runs only on Unix. On Windows and
other non-Unix platforms no Codex program is started: the accounts are still
listed as described in [Which accounts are read](#which-accounts-are-read),
and every row reads `error: codex usage read is supported only on Unix`.
The JSON rows keep the same keys, with `state` set to `error`.

##### Codex states

| Text state | JSON `state` | When |
|------------|--------------|------|
| `signed in` | `ok` | `account/rateLimits/read` answered with a `result`. |
| `not signed in` | `not_signed_in` | It answered with an error whose message contains, ignoring case, `authentication required`, `not logged in`, `not signed in` or `login required`. Codex 0.166.0 answers `codex account authentication required to read rate limits`. |
| `timed out` | `timeout` | No answer before the stop time at the end of the [deadline](#codex-read-deadline). |
| `error: <reason>` | `error` | Anything else; see the reasons below. |

| Reason | Cause |
|--------|-------|
| `codex binary not found: <bin>` | The program does not exist. |
| `cannot start <bin>: <error>` | It exists but could not be started, for example for lack of permission. |
| `codex app-server exited (<status>): <last stderr line>` | It exited before answering. The stderr line is the last non-empty one, cut to 200 characters. |
| `codex usage read is supported only on Unix` | The platform is not Unix; see [Codex read deadline](#codex-read-deadline). No program is started. |
| `unexpected response from codex app-server` | The `result` is not an object, or a line of stdout is longer than 1 MiB. |
| The server's message | Any other JSON-RPC error, cut to 200 characters. An error answer to `initialize` counts too. |

##### Codex row layout

Each row is the label, padded to the longest label (up to 24 characters; a
longer label is cut to 23 followed by `…`), then the state padded to 13
characters, then these fields, separated by two spaces:

| Field | Shown | Form |
|-------|-------|------|
| Windows | For each window reported, primary first | `<length> <bar> <pct> <reset> in <time>`; see below. |
| Credits | When `credits` was reported | `credits <balance>`, or `credits yes` / `credits no` from `hasCredits` when there is no balance. The balance is printed with no trailing zeros, as `4200` or `12.5`. |
| Plan | When `planType` was reported | `plan <type>` |
| No figures | A `signed in` row with no window and no credits | `no windows reported` |
| Detail | `timed out` rows | `after <n>s`, the deadline that applied. |
| Read time | Always | `read HH:MM:SS`, the time the answer arrived or, for a row with no reading, the time the read was given up. |
| Marker | The [next choice](#codex-next-choice) only | `<- next` |

The window length is `5h` for 300 minutes and `7d` for 10080. Any other
length is `<n>d` when it is a whole number of days, `<n>h` when it is a
whole number of hours, and `<n>m` otherwise. A window with no length is
labelled `primary` or `secondary`.

The bar and the percentage follow the Claude [bars](#bars): 10 cells and a
whole-number percentage, rounded down, coloured the same way. With
`--table`, the bar is left out and the percentage keeps one decimal place
where there is one, as `12.5%`. A window with no percentage shows `-` in
place of both.

Rows keep the order in [Which accounts are read](#which-accounts-are-read);
they are not sorted by room. `--raw`, `--spend` and
`--usable-session-pct` do not change the Codex block. The block is printed
after the whole Claude output, including its footer, separated by one empty
line, and also after the `no profiles` line when there are no Claude
profiles.

Labels, the root path, plan types and error reasons are
[cleaned](#table-output) before they are printed, so a directory name or a
server string cannot send terminal escapes. The layout is for people and
may change; scripts should use `--json`.

##### Codex reset times

A reset is shown as `MM-DD HH:MM` in the same zone as the Claude reset
cells, and `MM-DD HH:MMZ` with `--utc`, followed by `in <h>h <m>m` from the
read time, rounded down to the minute: `in 2h 31m`, `in 132h 51m`. A reset
at or before the read time shows `due` instead of `in …`. With `--utc` the
read time is `HH:MM:SSZ`.

`resetsAt` is used as a time only when it is above `0` and no more than
400 days after the read. Any other value is shown as `reset <value>`, as
given, with no `in …`, and its `resets_at_rfc3339` in the JSON is `null`. A
window with no reset shows `-`.

##### Codex next choice

The `<- next` marker goes on the account `codex-pick`'s rule picks from
these live readings. It is not always the account `codex-pick` itself
would choose: `codex-pick` reads the rate limits stored in each account's
session files, which can be days old, while this view reads them live, so
the two can disagree. The rule:

1. Only `signed in` rows are candidates. A row that timed out, failed or is
   not signed in is never chosen.
2. Each candidate's room is `100 - max(used percentage)` over its windows;
   a row with no window percentage has room `0`.
3. A candidate with room `0` or less gets `0.5` instead when it reports
   `hasCredits` true and a balance above the credit floor, 3000, and `-1`
   otherwise.
4. The candidates are sorted by room, then by path, both descending. The
   path is the account directory as built (`<root>/<name>` or
   `$HOME/.codex`), compared as bytes, not resolved through symlinks.
5. The first candidate with room above `0` is marked. When there is none,
   no row is marked.

In the example, `ggrant` (room 66) is chosen over `ffarmer` (59), `hhale`
(credits only, 0.5) and `~/.codex` (12). Two accounts with the same room
are taken in descending path order, so `ggrant` would come before
`ffarmer`. `codex-pick` also counts a signed-in account with no stored
reading as full; with live reads that case cannot arise, because a
signed-in account always answers with its figures.

##### What the Codex read does not do

- It never opens `auth.json`, `config.toml` or any other file inside an
  account directory. Directories are only listed and checked for type.
  Codex reads its own login.
- It makes no network request itself; the Codex app server does, as it
  would for `codex` in that account.
- `CODEX_HOME` is set only in the child's environment, never in
  amplihack's own. `OPENAI_API_KEY` and `CODEX_API_KEY` are removed from
  the child's environment, so an API key cannot replace the account's login
  in the reading, and `RUST_LOG` is removed, so verbose logging cannot put
  login detail into an error reason.
- No shell is used. The account path reaches the child only through
  `CODEX_HOME`.
- Readings are not stored anywhere. amplihack writes nothing into an
  account directory; the Codex app server itself keeps its state files
  there, as any `codex` run does.
- It does not stop the Codex app server from refreshing the account's
  login. The server may refresh that account's tokens and rewrite its login
  file, as any `codex` run in that directory can. This includes `~/.codex`
  and any account a live Codex session is using at the time; a session that
  still holds the old refresh token may then have to sign in again. How the
  read stops the app server so that a renewal under way can finish is in
  [Codex read deadline](#codex-read-deadline).

#### JSON output

`--json` prints one JSON array of Claude profile rows, in
[display order](#ordering). stdout holds only the JSON; warnings go to
stderr. With no profiles it prints `[]`. This is the stable interface for
scripts, and the key table below is its specification. Keys are only ever
added, never renamed or removed, and string values that name a state or kind
may gain new values. There is no version field.

`--json` has no Codex rows, and does not read Codex at all. #2001 left it
the bare array it was before, because scripts read it with `.[]` and `.[0]`,
often with stderr sent to `/dev/null`: a top-level object would make `jq`
fail in those scripts and leave them reporting an empty result with no
message. The [Codex rows](#codex-json-rows) are in
[`--summary-json`](#summary-json), under `codex`. The same Claude rows are
named `profiles` there; the arrays are identical.

With one profile:

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
    "source": "direct",
    "spend": "n/a",
    "session": {
      "label": "session",
      "used_percentage": 20.0,
      "resets_text": "2026-10-17T02:00:00Z",
      "resets_at": 1792202400
    },
    "session_limit": true,
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

Each row has these keys:

| Key | Type | Meaning |
|-----|------|---------|
| `profile` | string | Profile name. |
| `account` | string or `null` | The account whose login is live in the profile's directory: the profile's own name unless it [hosts](#leases) another account. The numbers belong to this account. `null` only for an [excluded](#excluded-profiles) row. |
| `default` | bool | Whether this is the default profile. |
| `leased_to` | string or `null` | The directory this profile's login is lent to. |
| `live_in` | string or `null` | `"~/.claude"` when this profile's login is [live in `~/.claude`](#logins-in-claude) and its numbers were read from there; otherwise `null`. The key is always present, and at most one row has a non-null value. The value is always the literal `~/.claude`, never the expanded home path. |
| `overflow` | bool | Whether the profile is marked as [credit-backed overflow](#rotate-overflow). Always present. |
| `overflow_blocked_until` | integer or `null` | Unix epoch seconds until which the rotator has [blocked](#overflow-spend-limit-block) this overflow profile after a spend limit; `null` when it is not blocked or not marked. Always present. |
| `state` | string | One of the [states](#states). `"excluded"` for a profile in the auto-switch `exclude` list; such a row has `source`, `account`, `session`, `week_all` and `error` `null`. See [Excluded profiles](#excluded-profiles). |
| `source` | string or `null` | How the row was read: `"passive"`, `"direct"` or `"fallback"`; see [Reading source](#reading-source). A row that failed on every step has the source of its last attempt. `null` for a row that was never probed, such as an [API-key](#api-key-usage) row or a `profile directory refused` row. Always present, immediately after `state`. |
| `spend` | string | The [spend status](#spend-status): `"n/a"`, `"ok"`, `"limit_reached"`, `"unknown"` or `"not_probed"`. Always present, immediately after `source`. `"unknown"` means a probe was asked for or needed and gave no reading; `"not_probed"` means usage credits are on and no spend probe was asked for (no `--spend`). Neither says anything about the spend limit. `"n/a"` does not mean that usage credits are off. Scripts should treat a value they do not know as `"unknown"`. |
| `session` | window or `null` | The session: `five_hour` from a [passive reading](#passive-readings), the [direct read](#direct-read) or the [usage request](#probe-method), or the `Current session` line from `/usage`. Always `null` for `timeout` and `error`, and when `session_limit` is `false`. |
| `session_limit` | bool | `false` only when a reading showed no session window; `true` otherwise, including rows that could not be read. See [No session limit](#no-session-limit). Always present, immediately after `session`. |
| `week_all` | window or `null` | The all-models week: `seven_day`, or the `Current week (all models)` line. Kept as reported when the session is exhausted. Always `null` for `timeout` and `error`. |
| `week_models` | array of windows | Each usable `rate_limits.model_scoped` entry, or every other `Current week (<model>)` line, in the order given. Empty if none, and always empty for passive and direct rows. |
| `error` | string or `null` | The [reason](#states) for an `error` row, otherwise `null`. |
| `raw` | object | Only with `--raw`: `{"stdout": "...", "stderr": "..."}` from the attempt that gave the row, both empty for passive and direct rows; see [Raw output](#raw-output). The key is absent, not `null`, without `--raw`. |

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

##### Claude rows in scripts

There is no `on_credits` key. The live login is running on credits exactly when one row
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

The weekly-only row from the [table example](#table-output) reads:

```json
{
  "profile": "weekly",
  "state": "ok",
  "session": null,
  "session_limit": false,
  "week_all": {
    "label": "all models",
    "used_percentage": 47.0,
    "resets_text": "2026-10-22T09:00:00Z",
    "resets_at": 1792659600
  }
}
```

Its other keys are left out here. To list the profiles with
[no session limit](#no-session-limit):

```bash
amplihack profile usage --json | jq -r '.[] | select(.session_limit == false) | .profile'
```

```
weekly
```

##### Codex JSON rows

Each row of `codex` in [`--summary-json`](#summary-json) is one
[Codex account](#codex-accounts), in the same order as the text rows.
[`--json`](#json-output) has no Codex rows. For the `ggrant`, `hhale` and `kkim` rows of the
[example](#codex-accounts):

```json
[
  {
    "account": "ggrant",
    "path": "/home/ryan/.codex-profiles/ggrant",
    "state": "ok",
    "error": null,
    "windows": [
      {
        "name": "secondary",
        "used_percent": 34.0,
        "window_minutes": 10080,
        "resets_at": 1794321000,
        "resets_at_rfc3339": "2026-11-10T14:30:00Z"
      }
    ],
    "credits": null,
    "plan_type": "pro",
    "rate_limit_reached_type": null,
    "ordinary_usage_allowed": true,
    "read_at": "2026-11-06T19:09:00Z",
    "next_choice": true
  },
  {
    "account": "hhale",
    "path": "/home/ryan/.codex-profiles/hhale",
    "state": "ok",
    "error": null,
    "windows": [],
    "credits": { "has_credits": true, "balance": 4200.0 },
    "plan_type": "team",
    "rate_limit_reached_type": null,
    "ordinary_usage_allowed": null,
    "read_at": "2026-11-06T19:09:00Z",
    "next_choice": false
  },
  {
    "account": "kkim",
    "path": "/home/ryan/.codex-profiles/kkim",
    "state": "timeout",
    "error": "timed out after 20s",
    "windows": [],
    "credits": null,
    "plan_type": null,
    "rate_limit_reached_type": null,
    "ordinary_usage_allowed": null,
    "read_at": "2026-11-06T19:09:00Z",
    "next_choice": false
  }
]
```

| Key | Type | Meaning |
|-----|------|---------|
| `account` | string | The directory name, or `~/.codex`. Not cleaned; JSON escaping only. |
| `path` | string | The account directory as built (`<root>/<name>` or `$HOME/.codex`), not resolved through symlinks. Not cleaned. |
| `state` | string | `"ok"`, `"not_signed_in"`, `"timeout"` or `"error"`; see [Codex states](#codex-states). Scripts should treat a value they do not know as `"error"`. |
| `error` | string or `null` | For `timeout`, `"timed out after <n>s"`; for `error`, the [reason](#codex-states); otherwise `null`. |
| `windows` | array | The windows reported, primary first. Always `[]` unless `state` is `"ok"`. |
| `windows[].name` | string | `"primary"` or `"secondary"`, the field it came from. |
| `windows[].used_percent` | number or `null` | As read, not clamped. |
| `windows[].window_minutes` | integer or `null` | The window length in minutes: `300` for five hours, `10080` for a week. |
| `windows[].resets_at` | integer or `null` | The reset as Unix epoch seconds, as received, even when it is out of [range](#codex-reset-times). |
| `windows[].resets_at_rfc3339` | string or `null` | The same reset as an RFC 3339 UTC time ending in `Z`; `null` when there is no reset or it is out of range. |
| `credits` | object or `null` | `{"has_credits": bool, "balance": number or null}` when Codex reported credits; otherwise `null`. Always `null` unless `state` is `"ok"`. |
| `plan_type` | string or `null` | The plan, cut to 200 characters and cleaned. |
| `rate_limit_reached_type` | string or `null` | Which limit Codex says is reached, cut to 200 characters and cleaned. |
| `ordinary_usage_allowed` | bool or `null` | As Codex reported it. |
| `read_at` | string | When the answer arrived, or when the read was given up, as an RFC 3339 UTC time ending in `Z`. Present on every row. |
| `next_choice` | bool | `true` on the one row with the [`<- next` marker](#codex-next-choice), `false` on the others. |

Every key is present on every row; a value that was not reported is
`null`. Percentages and balances are JSON numbers with a fractional part,
such as `34.0`; read them as floats. Server strings (`plan_type`,
`rate_limit_reached_type` and the server's part of `error`) are cleaned
before they are stored, so `jq -r` cannot print terminal escapes from
them.

To print the account `codex-pick`'s rule picks from the live readings:

```bash
amplihack profile usage --summary-json | jq -r '.codex[] | select(.next_choice) | .account'
```

```
ggrant
```

To list the Codex accounts that could not be read:

```bash
amplihack profile usage --summary-json | jq -r '.codex[] | select(.state == "timeout" or .state == "error") | "\(.account): \(.error)"'
```

```
kkim: timed out after 20s
mmoss: codex app-server exited (exit status: 1): error: unrecognized subcommand 'app-server'
```

#### Capacity summary

After the bar view or the table and their footer lines (`running on
overflow:` and the others, then `times:` in the bar view and `order:`),
`profile usage` prints a blank line and six summary lines. The summary
is worked out from the rows of this run and the [usage history](#usage-history);
it runs no extra probe and makes no network call. With these rows, and a
history going back six hours:

```
PROFILE  MARKS  STATE              SESSION  SESSION RESETS        WEEK  WEEK RESETS
review   -      ok                 5%       2026-10-30T19:00:00Z  10%   2026-11-03T09:00:00Z
work     *      ok                 20%      2026-10-30T20:00:00Z  30%   2026-10-31T09:00:00Z
team     -      session_exhausted  100%     2026-10-30T18:00:00Z  68%   2026-11-04T09:00:00Z
night    -      session_exhausted  100%     2026-10-30T18:00:40Z  85%   2026-10-31T02:00:00Z
scratch  -      not_logged_in      -        -                     -     -
spare    -      timeout            -        -                     -     -

weekly headroom      2.07 login-weeks of 4 (52%)
usable now           2 of 4 logins (session < 95% and weekly < 100%)
next session reset   2026-10-30T18:00:00Z  (night, team: +2 usable)
runway               ~9 h at the last 6 h rate (0.230 login-weeks/h)
resets in next 24 h  2026-10-31T02:00:00Z night +0.85; 2026-10-31T09:00:00Z work +0.30
unknown              2 profiles (no valid reading; excluded from all figures)
```

| Line | Meaning |
|------|---------|
| `weekly headroom` | The weekly capacity left across all logins with a [valid reading](#valid-and-unknown-readings), in login-weeks: one login-week is one login's whole weekly allowance. `X login-weeks of N (P%)`, where `N` is the number of valid readings, `X` is the sum of `(100 − week%) / 100` over them, and `P` is `X / N` as a percentage. Followed by `   [weighted: X' of T]` only when some login with a valid reading has a [weight](#capacity-weights) other than `1.0`; a weight for a profile that is not among them, such as a deleted one, does not count. With no valid reading: `unknown (no valid readings)`. |
| `usable now` | `U of N logins (session < S% and weekly < 100%)`: how many logins with a valid reading can take work now, and the session threshold `S` in force. See [Usable now](#usable-now). |
| `next session reset` | The earliest session reset among logins stopped only by their session limit, and every such login whose reset falls within 60 seconds of it: `<time>  (<names>: +K usable)`, where `K` is the number of names. `none` when there is no such login. See [Next session reset](#next-session-reset). |
| `runway` | How long the weekly headroom lasts at the rate it was used over the last six hours: `~H h at the last 6 h rate (R login-weeks/h)`, or `unknown`. The `6 h` is fixed text; the rate is per hour whatever the length of history used, needs at least 4.5 h of history, and is weighted. See [Rate and runway](#rate-and-runway). |
| `resets in next 24 h` | Weekly resets due in the next 24 hours, each `<time> <name> +<login-weeks restored>`, soonest first, separated by `; `. `none` when there are none. |
| `unknown` | `M profiles (no valid reading; excluded from all figures)`: the number of subscription profiles without a valid reading, `1 profile (…)` for one and `0 profiles (…)` for none. Their names are in the table and in [`summary.unknown`](#summary-json). They are left out of every other line. |

Times are UTC, `YYYY-MM-DDTHH:MM:SSZ`, whichever [attempt](#probe-method)
gave the row. Percentages are whole numbers, login-weeks have two decimal
places, the rate three, and the runway whole hours, rounded half away from
zero. `~0 h` means less than half an hour is left at that rate. Each label
is padded with spaces to 21 characters, so the values start in one column. Names are cleaned the same way as in the table. The summary's
layout is for people and may change; scripts should use
[`--summary-json`](#summary-json).

There is no summary with `--json` or `--raw`, or when no row is a
subscription profile (for example when every profile is an
[API-key profile](#api-key-profiles)). With no profiles, the table form
prints only the `no profiles` line.

##### Valid and unknown readings

The summary counts subscription profiles only. [API-key profile](#api-key-profiles)
rows are not logins in
the summary and are not listed as unknown. An overflow profile is a
subscription profile and is counted. A profile in the auto-switch `exclude`
list is left out entirely: it is not in `N`, not in any sum and not on the
`unknown` line, because it was [not probed](#excluded-profiles).

A row's reading is valid only when its weekly figure is known:

| Row | Reading |
|-----|---------|
| `ok` or `session_exhausted` with an all-models week | Valid, at that week percentage. |
| `weekly_exhausted` | Valid, at 100% even when no week percentage was read. |
| `ok` or `session_exhausted` without an all-models week | Unknown. |
| A rotator probe that is exhausted but read no all-models week | Unknown: the week is not taken to be full. |
| `spend_limit`, `not_logged_in`, `timeout`, `unparsed`, `error` | Unknown. |
| The profile [live in `~/.claude`](#logins-in-claude), when the login in `~/.claude` changed during the probe | Unknown: the numbers may belong to another account. |

An unknown reading is never counted as 0% or 100%. It is left out of `N`,
of every sum and of the history, and counted on the `unknown` line.
Percentages are clamped to `0`–`100` first, and sums are kept in whole
percentage points, so the figures do not drift: 16 valid readings whose week
percentages add up to 1228 give `(1600 − 1228) / 100 = 3.72` login-weeks and
exactly 23.25%, shown as `23%` and given in JSON as `23.3`.

##### Usable now

A login with a valid reading is usable now when its week is below 100% and
either its session is below the threshold or it has no session limit. A login
has no session limit when its reading showed no session window, that is, when
its [`session_limit`](#no-session-limit) is `false`; it is usable on its week
alone. An idle session-limited login at 0% with no reset reads the same way,
and is usable too. A valid reading whose session was not read for another
reason, with `session_limit` still `true`, counts towards the headroom but not
as usable, because nothing shows its session is below the threshold. The
rotator applies the same rule to the readings it records.

| Setting | Values | Default |
|---------|--------|---------|
| `--usable-session-pct <n>` | `1` to `100` | none |
| `AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT` | `1` to `100` | `95` |

The flag wins over the variable. A flag value out of range is a usage error
(exit status 2). An empty variable counts as unset. A variable that is not a
whole number from `1` to `100` prints one warning to stderr, showing the
value escaped and cut to 32 characters, then uses `95`:

```
warning: ignored AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT "most" (use 1 to 100); using 95
```

With the default, a session at 94% is usable and one at 95% is not. The
line prints the threshold in force, for example
`2 of 4 logins (session < 80% and weekly < 100%)` with `--usable-session-pct 80`.
The threshold affects only this line, the `usable_now` key and the usable
count on the `rotate status` [capacity line](#capacity-line). It does not change
the rotator's or auto-switch's thresholds.

##### Next session reset

The candidates are logins with a valid reading that are stopped by their
session limit only: state `session_exhausted`, or a session at 100%, with a
week below 100% and a known session reset time. A candidate without a reset
time is left out. The line gives the earliest reset time and every candidate
whose reset is within 60 seconds of it, by name, because probes of logins
that reset together report times a few seconds apart. A reset time that has
already passed is still shown; the next run reads the login again.

##### Rate and runway

The rate is how many login-weeks were used per hour over the last six hours.
It is read from the [usage history](#usage-history):

1. The newest end is this run's readings. The oldest end is the oldest
   history entry no more than six hours older than this run that covers
   every login with a valid reading now: an entry written by `profile
   usage` (or an older entry without a writer), or a rotator entry that read
   all of those logins. A rotator entry that skipped a login, for example
   one running a session, is never the oldest end. Those busy logins are
   the ones being used, and leaving them out of the rate while their
   headroom still counts would make the runway far too long. Entries before
   the oldest end are not used.
2. If there is no such entry, or it is less than four and a half hours
   older, there is too little history: `runway unknown`, and the rate and
   runway are `null` in JSON.
3. Only logins with a valid reading at both ends count. A login `profile
   usage` could not read at the start, or one that is unknown now, is left
   out. Rotator entries between the two ends are used for the logins they
   read; a login missing from one of them only skips that step.
4. For each login, the use is its net rise in week percentage over the
   window. Its readings are taken in time order: the start, each later
   entry that read it, and this run. They are split into weeks at each
   weekly reset. A reset lies between two consecutive readings when the
   earlier reading's `w_reset` is at or before the later reading's time, or
   when the week percentage fell by more than 10 points. A fall of 10 points
   or less is not a reset: within a week the figure only rises, so such a
   fall comes from a [stale reading](#stale-readings) and adds nothing. A
   fall of more than 10 points is a reset even while `w_reset` is still in
   the future, because a week can reset early. In each week the use is the
   highest percentage read in that week minus the percentage the week
   started at, or zero if that is negative. The first week starts at the
   login's percentage at the start and each later week starts at `0`. The
   login's use is the sum over its weeks. A reading whose own `w_reset` is
   at or before the time it was taken belongs to a week that has ended and
   is not used, and neither is this run's reading of a login when it is a
   stale passive reading by the rule under [Stale readings](#stale-readings).
5. The rate is the sum of each login's use times its [weight](#capacity-weights),
   divided by 100 and by the hours between the two ends. It is weighted;
   when every weight is `1.0` it equals the unweighted rate. It is never
   negative.

For example, on 5 November 2026 one login read 34% at 16:52 and 50% at
18:55. In between, a stale status-line figure of 38% reappeared five times
between rising ones (40, 38, 40, 41, 38, 42, and so on), and once 46
appeared between 48 and 49. Each fall is 5 points or less, so none is a
reset, and the login's use over those two hours is 50 − 34 = 16 points.
Release 1.2.97 counted every fall as a reset and each fall's whole lower
figure as use: 269 points. Had the login read 90% at the start, reached
100%, reset to 0% and then read 12% now, its use would be
(100 − 90) + 12 = 22 points.

On the same day, `profile usage` at 16:52 reported a rate of `0.483` and a
runway of `7.5` hours. The start was a rotator entry at 10:53:20 that held
every login read at 16:52. Under the rule above, three logins used 59
points over those 5.99 hours, a rate of `0.099` and a runway of `36.5`
hours. Release 1.2.97 counted 289 points; most came from falls of 1 to 3
points counted as resets.

The runway is the weighted headroom of this run divided by the rate. When the
rate is zero, nothing was used over the window and the runway is `unknown`;
JSON still gives the rate. Weights are `1.0` unless set, in which case the
weighted and unweighted figures are the same.

History written before stale readings were filtered (see
[Stale readings](#stale-readings)) may still hold them, and they affect the
rate until they are more than six hours old. A fall of 10 points or less
has no effect. A stale reading at the start shifts that login's use by the
difference between the stale and the true figure, and a stale fall of more
than 10 points counts as a reset.

`amplihack profile usage --help` states the six-hour window, the
four-and-a-half-hour minimum and the weighting.

##### Capacity weights

By default every login counts as one login-week. When accounts have different
weekly allowances, for example a Max 20x plan next to Pro plans, set a weight
for each login in `<root>/.auto-switch.d/capacity-weights.json`:

```json
{ "team": 2.0, "scratch": 0.5 }
```

With that file, the first summary line above becomes:

```
weekly headroom      2.07 login-weeks of 4 (52%)   [weighted: 2.39 of 5.00]
```

The weighted headroom is the sum of `weight × (100 − week%) / 100` over the
valid readings, out of the sum of their weights `T`, which has two decimal
places. The first figure stays unweighted. The rate, the runway and
`restores` on the `resets in next 24 h` line
are always weighted.

A profile has no settings file of its own, so weights are kept together in
this one file rather than as a per-profile setting. There is no command to
set weights; edit the file by hand. It is read once
per run and never written by amplihack. The rules:

| Case | Result |
|------|--------|
| No file, or no profile root | Every weight is `1.0`, with no message. |
| The profile root is a symlink, is not a directory owned by you, or is writable by other users | Every weight is `1.0`, with the warning below and the reason `unsafe file`. This is the same root rule as the [usage history](#usage-history). |
| The file is a symlink, larger than 64 KiB, not owned by you, writable by group or others, not valid JSON, or not a JSON object | Every weight is `1.0`, and one warning goes to stderr: `warning: capacity-weights.json ignored (<reason>); every weight is 1.0`. |
| A value that is not a number, or not above `0` and at most `1000` | That weight is `1.0`, with one warning: `warning: capacity-weights.json: weight for "<name>" ignored (use a number above 0 and at most 1000); using 1.0`. |
| A name that matches no profile | Ignored. |

Weights are applied when the summary is worked out. They are never written
to the history, so a change takes effect on the next run, for past entries
too.

#### Summary JSON

`--summary-json` prints one JSON object instead of the bar view and summary.
stdout holds only the JSON; warnings go to stderr. `profiles` is exactly the
array [`--json`](#json-output) would print in the same run. `codex`, after
`summary`, holds the [Codex rows](#codex-json-rows); this is the only JSON
form that has them. For the run above, with only the first of the six rows
shown and no Codex accounts:

```json
{
  "at": "2026-10-30T16:15:00Z",
  "profiles": [
    {
      "profile": "review",
      "account": "review",
      "default": false,
      "leased_to": null,
      "live_in": null,
      "overflow": false,
      "overflow_blocked_until": null,
      "state": "ok",
      "session": {
        "label": "session",
        "used_percentage": 5.0,
        "resets_text": "2026-10-30T19:00:00Z",
        "resets_at": 1793386800
      },
      "week_all": {
        "label": "all models",
        "used_percentage": 10.0,
        "resets_text": "2026-11-03T09:00:00Z",
        "resets_at": 1793696400
      },
      "week_models": [],
      "error": null
    }
  ],
  "summary": {
    "weekly_headroom_login_weeks": 2.07,
    "weekly_headroom_pct": 51.8,
    "weighted_headroom_login_weeks": 2.07,
    "usable_now": 2,
    "valid_readings": 4,
    "unknown": ["scratch", "spare"],
    "next_session_reset": { "at": "2026-10-30T18:00:00Z", "profiles": ["night", "team"] },
    "rate_login_weeks_per_hour": 0.23,
    "runway_hours": 9.0,
    "weekly_resets_next_24h": [
      { "at": "2026-10-31T02:00:00Z", "profile": "night", "restores": 0.85 },
      { "at": "2026-10-31T09:00:00Z", "profile": "work", "restores": 0.3 }
    ]
  },
  "codex": []
}
```

| Key | Type | Meaning |
|-----|------|---------|
| `at` | string | When the summary was worked out: after the window readings and before any [spend probe](#spend-probe), so `--spend` never makes it later. |
| `profiles` | array | The array of [`--json`](#json-output), unchanged. |
| `summary` | object or `null` | `null` when there are no profiles or no row is a subscription profile. |
| `summary.weekly_headroom_login_weeks` | number | Unweighted headroom, two decimal places. `0.0` with no valid reading. |
| `summary.weekly_headroom_pct` | number or `null` | Unweighted headroom as a percentage of `valid_readings`, one decimal place. `null` with no valid reading. |
| `summary.weighted_headroom_login_weeks` | number | Weighted headroom, two decimal places. Equal to the unweighted figure when every weight is `1.0`. |
| `summary.usable_now` | integer | Logins [usable now](#usable-now). |
| `summary.valid_readings` | integer | `N`, the logins with a [valid reading](#valid-and-unknown-readings). |
| `summary.unknown` | array of strings | Subscription profiles without a valid reading, by name. |
| `summary.next_session_reset` | object or `null` | `{"at": <time>, "profiles": [<names>]}`, as on the [text line](#next-session-reset); `null` for `none`. |
| `summary.rate_login_weeks_per_hour` | number or `null` | Weighted [rate](#rate-and-runway), three decimal places: the net rise of each login's week percentage over the last 6 h, counted from the oldest entry in that window that covers every login valid now (step 1), times its weight, divided by 100 and by the hours covered. `null` when that entry is less than 4.5 h old, when there is none, or when no login has a valid reading at both ends. `0.0` or above. Unweighted when every weight is `1.0`. |
| `summary.runway_hours` | number or `null` | Hours of weighted headroom at that rate, one decimal place; `null` when the rate is `null` or zero. |
| `summary.weekly_resets_next_24h` | array | Valid logins whose weekly reset is after now and no more than 24 hours away, and whose weighted `restores` (`weight × week% / 100`, two decimal places) is above `0`, soonest first, then by name. Each is `{"at", "profile", "restores"}`. |
| `codex` | array | The [Codex JSON rows](#codex-json-rows), unchanged; `[]` with profile names given. Codex rows never count towards `summary`. |

Numbers are rounded half away from zero only when printed; a value that is
not finite is printed as `null`. Keys are only ever added, as for `--json`.

Times differ by place, on purpose: inside `profiles`, `resets_at` keeps its
`--json` form, Unix epoch seconds, so the array matches `--json` byte for
byte; `at` and every time in `summary` are ISO 8601 UTC strings ending in
`Z`. The times in `codex` are as described under
[Codex JSON rows](#codex-json-rows). To read the runway in a script:

```bash
amplihack profile usage --summary-json | jq '.summary.runway_hours'
```

```
9.0
```

With no profiles it prints
`{"at":"<time>","profiles":[],"summary":null,"codex":[...]}`, with the
Codex rows read as usual.

#### Usage history

The rate needs earlier readings, so `profile usage` and the rotator append
one line per measured cycle to `<root>/.auto-switch.d/usage-history.jsonl`
(mode `0600`). Each line is one JSON object:

```json
{"at":"2026-10-30T16:15:00Z","by":"usage","profiles":{"night":{"s":100,"w":85,"w_reset":"2026-10-31T02:00:00Z"},"review":{"s":5,"w":10,"w_reset":"2026-11-03T09:00:00Z"},"team":{"s":100,"w":68,"w_reset":"2026-11-04T09:00:00Z"},"work":{"s":20,"w":30,"w_reset":"2026-10-31T09:00:00Z"}}}
```

| Key | Meaning |
|-----|---------|
| `at` | When the cycle was measured, UTC. |
| `by` | Which command wrote the line: `usage` for `profile usage`, which reads every subscription login, or `rotate` for a rotator cycle, which records only the profiles it probed. Absent in lines written before this key existed; such a line, or one with another value, has no known writer. |
| `profiles` | Profile name to reading, for valid readings only. Unknown readings are left out, not written as 0 or 100. [Excluded profiles](#excluded-profiles) are never written, here or under `unknown`, and a name in the current `exclude` list is dropped from both on read, so older lines never count it. |
| `s` | Session percentage, a whole number `0`–`100`, or `null` when not read. `session_exhausted` is written as `100`. |
| `session_limit` | Written only as `false`, beside `"s":null`, when the reading showed no session window (see [Usable now](#usable-now)). Absent otherwise. A missing key, `null`, or `false` beside a session figure, reads as "not read". |
| `w` | All-models week percentage, a whole number `0`–`100`. `weekly_exhausted` is written as `100`. |
| `w_reset` | When the week resets, UTC, or `null` when not known. |
| `unknown` | Names of the logins the writer tried and could not read, or whose reading was a [stale reading](#stale-readings), sorted. Absent when there are none. Never counted in a figure; a name that also has a reading is ignored on read. |

##### Reading times in the history

Every reading in a line is taken as read at the line's `at`. A row read
directly or by `claude` was read during the run, so that holds. A
[passive reading](#passive-readings) was stored by Claude's status line
earlier: up to `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` before the
run started, `300` seconds by default and at most `3600`.

A passive reading's age is measured from the start of the run, the time
it was accepted at. `profile usage` takes `at` later, once every row's
windows are in and before any `--spend` [spend probe](#spend-probe) starts,
so a slow `claude` fallback for one row never turns another row the
table shows as read into an unknown one. The
[rotator](#rotate-cycle) takes one time for both, so for it the start of
the run and `at` are the same. The rule is:

- A passive reading at most 300 seconds older than the start of the run
  is recorded at the line's `at`. The recorded time is therefore later
  than the real one by at most 300 seconds plus the time the run took.
  For the rotator that is 300 seconds. For `profile usage` there is no
  fixed bound: each read of a profile may take up to its
  [deadline](#probe-deadline) (`AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS`,
  `120` seconds by default, `600` at most), a `claude` fallback gets a
  deadline of its own after a failed direct read, and fallbacks beyond
  the [parallel limit](#probe-parallelism) wait for a free slot. A run of
  several minutes is ordinary. A run of 10 minutes gives an error of at
  most 15 minutes, about 6% of the 4.5 hours the
  [rate](#rate-and-runway) needs at least.
- A passive reading more than 300 seconds older than the start of the run
  is unknown, in the history and in the
  [capacity summary](#capacity-summary) alike: it is in no figure and no
  rate, it is listed under `unknown`, and the table still shows its
  numbers. This happens only when
  `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` is set above `300`.

A row that could not be read is unknown in the same way, whatever step
failed. That includes a row whose [direct read](#direct-read) failed, with
or without a `refresh not saved` warning, and which reads `error` or
`timeout`: it is never counted as 0% or 100% used. A row whose direct read
failed but whose `claude` fallback gave a reading is that reading.

A line holds only times, profile names and whole-number percentages. It
never holds an account, e-mail address, organisation, plan, credential,
error text or anything Claude printed.

Who writes it:

| Writer | When |
|--------|------|
| `profile usage` | After the probes, when no profile names were given and `--raw` was not given. With or without `--json` and `--summary-json`. |
| The [rotator](#rotate-cycle) | After every cycle that measured usage, once its state lock is released. It records the profiles it measured, and lists as unknown the plan logins it did not probe: those skipped as busy (a session running, leased, hosting or parked) and those whose login has no usable trusted account evidence (`no account`). The entry therefore says how many it did not read. Such an entry is not used as the start of the [rate](#rate-and-runway), and `rotate status` does not take a runway from it (see [Capacity line](#capacity-line)). The [live login](#rotate-live-profile) is recorded under its profile, and a login in `~/.claude` that belongs to no profile is not recorded. A cycle that measured nothing writes nothing. |

Both writers record only when all of these hold, and otherwise write nothing
and print nothing about the history:

- The profile root is a real directory, not a symlink, owned by you and not
  writable by other users.
- [Auto-switch is set up](#refreshing-auto-switch-readings):
  `<root>/.auto-switch.d/` exists and is a private directory of yours. It is
  never created by recording.
- None of `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` or
  `CLAUDE_CODE_OAUTH_TOKEN` is set, because every reading might then show
  that credential's numbers. The rotator measures anyway and warns about the
  variable, but does not record.

A filtered `profile usage` run is not recorded either, because the gaps
would make the rate wrong.

Writers take an exclusive lock on `usage-history.lock` beside it, trying
every 50 ms for up to one second; the rotator's state lock is never held at
the same time. Each line is written in one append; when the file's last line
was cut short, for example by a full disk, a newline is written first so the
new line stays whole. Under the lock, entries
older than eight days are dropped: when the first line is more than an hour
older than that or cannot be read, or the file is over 4 MiB, the file is
rewritten atomically without entries older than eight days or lines that
cannot be read, dropping the oldest entries until it is under 3 MiB. The
hour's slack means a full history is rewritten at most once an hour rather
than on every cycle. If the rewrite fails while the file is over 4 MiB,
nothing is appended. If it fails on a smaller file, the line is still
appended, the trim is tried again on the next cycle, and one warning is
printed: `warning: usage history not trimmed`.

Readers take the last 4 MiB of the file, and skip any line longer than
64 KiB, any line that is not valid JSON, any profile name that fails the
[name rules](#name-rules), any percentage outside `0`–`100` and any time
more than 100 years from now. A missing `s` counts as `null`, and times with
a UTC offset are read as the same instant in UTC. Unknown keys are ignored,
so a newer writer's lines still read. When any line is skipped, one warning
gives the count, so a corrupted history can be told apart from an empty one.
Blank lines are not counted:

```
warning: 3 unreadable usage history lines ignored
```

Readers check the profile root as writers do. A missing history, or a
missing `.auto-switch.d/`, is an empty history with no warning. A history
that exists but is refused, because the root is not trusted, or the file is
a symlink, is not a regular file, has another owner, has more than one link,
has any group or other permission (its mode is not `0600`), or cannot be
opened or read, is also treated as empty, and one
warning goes to stderr, from `profile usage` and from `rotate status`:

```
warning: usage history ignored (unsafe file)
```

Readers and writers accept exactly the same files, so a history that is
read is also appended to. A history whose mode was changed by hand, to
`0644` for example, is refused by both until it is set back to `0600`.

The history never decides anything and never fails a command. When it cannot
be read, the rate is unknown. When a line cannot be written (the lock stays
busy for a second, the file or lock is a symlink, has another owner, more
than one link or group or other permissions, or the write fails), one line
goes to stderr, or to the [rotate log](#rotate-log) for the rotator, and the
exit status does not change:

```
warning: usage history not updated (lock busy)
```

The reason is one of `lock busy`, `unsafe file`, `write failed` or
`too large`. Paths and system error text are not printed.

To clear the history, delete `usage-history.jsonl` and `usage-history.lock`.
Nothing else reads them, and both are created again on the next recorded
cycle. Until the history is four and a half hours old again, the runway is
`unknown`.

##### Stale readings

A [passive reading](#passive-readings), from `profile usage` or the
rotator, is the figure Claude's status line stored, and it can lag behind
the account. A cached figure that reappears after a higher one would show
the week falling and rising again. So before a line is written, under the
writers' lock, each passive reading in it is compared with that login's
newest reading in the history, and is stale when any of these holds:

| Rule | Example |
|------|---------|
| Its `w_reset` is at or before the line's `at`, so its week has ended. This applies even when the history holds no reading of the login. | Line at `2026-11-05T18:02:00Z`, passive `w_reset` `2026-11-05T18:00:00Z`. |
| Its `w_reset` and the recorded one are both known and within one hour of each other, so it is the same week, and its `w` is lower than the recorded `w`. | Recorded `w` 50, passive `w` 38, both resetting at `2026-11-10T09:00:00Z`. |
| Its `w_reset` is more than one hour earlier than the recorded one, so it belongs to an earlier week. | Recorded `w_reset` `2026-11-10T09:00:00Z`, passive `w_reset` `2026-11-03T09:00:00Z`. |

A stale reading is not written. In its place the line holds that login's
newest recorded reading (its `s`, `w` and `w_reset`), so the login stays in
the line. Passive readings come from Claude's status line, so a stale one
belongs to a login in use, and leaving it out would hide the login being
used up: a `profile usage` line that is the start of the
[rate](#rate-and-runway) would leave it out of the rate, and
[`rotate status`](#rotate-status) would say the rotator cycle left a login
out and leave its headroom out of the `capacity:` line. The recorded figure
adds nothing to the rate, since the week has not risen above it, and the
true figure is at least that high, because within a week it only rises.

The recorded reading is used only while its own week has not ended at the
line's `at`. When it has, or when the history holds no reading of the
login, nothing known is left to write: the login is listed under `unknown`,
as for a reading that failed. This happens only under the first rule.

A passive reading equal to or higher than the recorded one in the same week
is written. A reading taken during the run, directly or by `claude`, is
never treated as stale, and nor is a passive reading whose own `w_reset`
is unknown. When only the recorded `w_reset` is unknown, or the history
holds no reading of the login, only the first rule applies. The reset rule
of the rate covers the rest.

The newest reading of each login is looked for in the last 256 KiB of the
history only. When that part cannot be read, the line is not written and the
`write failed` warning above is given; the reading is never written
unchecked. The filter changes what is recorded. In the same run, the table,
the headroom and usable count of the summary, and the auto-switch readings
still use the reading; the rate does not (step 4 of
[Rate and runway](#rate-and-runway)). No message is given when a reading is
replaced or left out. Lines already in the history are not rewritten.

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
- Rows with source `passive` are not recorded: they came from `usage.json`,
  and recording them again would keep an old reading fresh.
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
- A row with [no session limit](#no-session-limit) follows the same rule:
  its stored `five_hour` is kept while its reset time is still in the
  future, and dropped once it has passed. A 0% session line with no reset
  may come from figures `claude` stored earlier, so it does not override a
  live session window. A login that has just stopped reporting a session
  window can therefore stay session-exhausted until the old reset passes,
  which is at most five hours. No new `five_hour` is recorded for such a
  row.
- The reading has `observed_at` set to now and `source` set to `statusline`,
  and is merged with the stored one the usual way.

`usage.json` is written once, atomically, at the end. The 300-second
quiet period after a swap does not apply: a direct read or fallback asks
under the live login directly, rather than waiting for a status-line update. Any failure while recording prints one
`auto-switch:` warning to stderr and the exit status stays 0.

After `usage.json` is written, `profile usage` also updates the
[capacity file](#capacity-file) when the switch is on.

On platforms without auto-switch (Windows and other non-Unix), there are no
lease marks, `account` is always the profile's own name, and nothing is
recorded or noted.

#### Exit status and refusals

| Status | When |
|--------|------|
| `0` | The probes ran, whatever states they produced, or there are no profiles. Rows in state `excluded`, listed when no names are given, do not change it. Codex rows never change the status, whatever their states. Recording failures, [usage history](#usage-history) failures and [weight](#capacity-weights) warnings do not change it. |
| `1` | Setup failed: Docker is active, the profile root is invalid, or a name is invalid or does not exist. Printed as `error:`/`hint:` like other [errors](#errors). Nothing is probed. A `claude` that cannot be found is not a setup failure: only rows that need a fallback read `error` with `claude not found`. Also `1` when any name given on the command line is in the auto-switch `exclude` list: each is refused with `<name>: excluded (in auto-switch exclude list); not probed (amplihack profile auto-switch exclude)`, the other names are probed, recorded and printed first, and the status is then `1`. See [Excluded profiles](#excluded-profiles). |
| `2` | Command-line usage error, such as `--timeout-secs 0`, `--usable-session-pct 101`, `--summary-json` with `--json`, `--raw`, `--table` or `--utc`, or `--table` with `--json`. |

With no profiles, the bar view and `--table` print
`` no profiles; create one with `amplihack profile add <name>` `` followed by
the [Codex block](#codex-accounts), the JSON form prints `[]`, and the
summary JSON form prints
`{"at":"<time>","profiles":[],"summary":null,"codex":[...]}`.

`--table` with `--json`, and `--utc` with `--json` or `--table`, are usage
errors (status 2): the second flag would have no effect.

Like `profile run`, `usage` is refused when Docker is activated through the
environment (`AMPLIHACK_USE_DOCKER`), before anything is read or spawned.

#### Privacy

- No request is ever made with an [excluded](#excluded-profiles) profile's
  token. `profile usage` and [`profile verify`](#profile-verify) never open
  its `.credentials.json`. The rotator reads that file locally, in memory,
  only for the byte comparison of
  [rule 3](#local-proof-of-an-excluded-live-login), and writes it only for
  the [write-back](#write-back-into-an-excluded-folder) of a move off an
  excluded live profile.
- Identity checks for an excluded folder use its local account and stored
  [record](#recorded-account) only. The rotator's
  [local proof](#local-proof-of-an-excluded-live-login) of an excluded live
  login reads the ledger, `~/.claude.json` and the record, and may compare
  two credential files byte for byte in memory. It never sends the token. The reason
  given when an excluded folder is skipped names no path, e-mail address or
  account identifier.
- A profile's `.credentials.json` is read, in memory only, for its
  [direct read](#direct-read), and written only after a
  [token refresh](#token-refresh). Its contents are never printed, logged,
  stored elsewhere or passed to a child process.
- Tokens appear only in the `Authorization` header of the usage request and
  in the body of the refresh request. They are never in a URL, a command
  line, the environment of a `claude` child, the table, the JSON, `--raw`
  output, `usage.json`, the rotate ledger or log, a journal, a tracing
  event or a panic message. The types that hold them print `<redacted>` when
  debug-formatted.
- [Direct read failures](#direct-read-failures) are fixed text. A response
  body other than a `200`'s is never read, and no body is ever printed.
- For an account with [usage credits on](#usage-credits-on), `--raw` prints
  only the shape of `extra_usage`: its type, its top-level key names with
  every character outside `A-Za-z0-9_.-` replaced by `?`, and bool and
  `null` values. A string or number from the answer is never printed, apart
  from the fixed words and the numbers `0` and `1` listed for `is_enabled`,
  so no e-mail address, account or organisation identifier, currency or
  amount appears. The shape is kept in memory only, and is never in the
  JSON, a file or the rotator's output.
- A [spend probe](#spend-probe), run only with `--spend`, is a `claude -p /usage` run like the
  fallback's: no token is in its command line or environment, and its
  output is never printed or stored.
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
  ledger's other fields and credential paths are never printed. To find the
  live profile, credential files are only checked for existence, without
  following symlinks.
- The [capacity summary](#capacity-summary), `--summary-json`'s `summary`
  object and the [usage history](#usage-history) contain profile names,
  whole-number percentages, counts and times only. They never use the
  row's `account`, and nothing Claude printed reaches them, even when its
  output includes an e-mail address.
- The [probe sandbox](#probe-sandbox) never reads, copies or links
  `.credentials.json`, and puts no credential in `bwrap`'s arguments or
  environment. Its note names a reason only, never a path or a value.
- [`prune-probe-logs`](#prune-probe-logs) prints paths and counts only,
  never what a log contains.
- The [Codex read](#what-the-codex-read-does-not-do) opens no file inside
  a Codex account directory, including `auth.json` and `config.toml`, and
  passes no API key to the Codex app server. Its rows hold directory
  names, paths, percentages, times, plan types and error reasons cut to
  200 characters; nothing from them is stored.


### Prune probe logs

```
amplihack profile prune-probe-logs [--dir <dir>] [--dry-run]
```

Usage probes from amplihack versions before the
[probe sandbox](#probe-sandbox) wrote one session log per probe into the
shared `projects` folder, under the folder for the directory amplihack ran
in. On a busy host that is thousands of files, and every later `/usage`
read them all. This command removes those logs. It is never run
automatically.

| Flag | Effect |
|------|--------|
| `--dir <dir>` | Folder to clean. Default `$HOME/.claude/projects/<encoded home>`. |
| `--dry-run` | Print each matching file and delete nothing. |

The encoded home is `$HOME` with every character outside `A-Z`, `a-z`,
`0-9` and `-` replaced by `-`: `/home/ryan` becomes `-home-ryan`. The
default needs `HOME` to be set to an absolute path.

**What is read.** Only files directly in the folder whose names end in
`.jsonl`. Subfolders are not entered. A symlink is never followed and is
kept. A file larger than 1 MiB is kept without being read.

**What is removed.** A file is removed only when all of these hold:

1. Every non-empty line is a JSON object with a string `type`.
2. Every `type` is `user` or one of the entries a `/usage` run writes
   around it: `session-owner`, `queue-operation`, `attachment`, `system`,
   `last-prompt` and `cost-state`. Any other type keeps the file:
   `assistant`, and the `mode`, `permission-mode` and
   `file-history-snapshot` entries that `/login` and typed sessions write.
   A `queue-operation` or `last-prompt` entry can hold a queued or last
   typed prompt, so it counts as probe-only only when its text field
   (`content` or `lastPrompt`) is missing or is the string `/usage`, after
   trimming whitespace. Any other value keeps the file.
3. Exactly one `user` entry is the `/usage` command: its `message.content`
   is a string made of `<command-name>/usage</command-name>`, then
   optionally `<command-message>usage</command-message>`, then optionally
   an empty `<command-args></command-args>`, with only whitespace between
   and around them.
4. Every other `user` entry is one of:
   - the caveat: `isMeta` is the JSON value `true` and the content is one
     `<local-command-caveat>…</local-command-caveat>` element;
   - the command's output: the content is one
     `<local-command-stdout>…</local-command-stdout>` or
     `<local-command-stderr>…</local-command-stderr>` element.

A file is kept when it has a typed prompt, content that is an array (a tool
result, for example), a second command, any other command such as
`/login`, an `isMeta` that is not the boolean `true`, a line that is not a
JSON object, or when it cannot be read. When in doubt, the file is kept.

A log written by a probe looks like this, one JSON object per line, with
fields the rule does not read left out:

```json
{"type":"session-owner"}
{"type":"queue-operation"}
{"type":"attachment"}
{"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>The command below was run directly in Claude Code…</local-command-caveat>"}}
{"type":"user","message":{"content":"<command-name>/usage</command-name>\n            <command-message>usage</command-message>\n            <command-args></command-args>"}}
{"type":"system"}
{"type":"last-prompt"}
{"type":"cost-state"}
```

**Output.** With `--dry-run`, one matching path per line on stdout:

```
"/home/ryan/.claude/projects/-home-ryan/0b7c….jsonl"
"/home/ryan/.claude/projects/-home-ryan/1f3a….jsonl"
```

Without it, one line:

```
removed 2467, kept 312
```

`kept` counts every `.jsonl` file in the folder that was not removed,
including files that could not be removed.
Every path, in `--dry-run` lines, warnings and errors, is printed in double
quotes, with control characters escaped. No
log content is ever printed, in output or in errors. A file that cannot be
removed is reported on stderr and counted as kept:

```
warning: could not remove "/home/ryan/.claude/projects/-home-ryan/0b7c….jsonl": Permission denied (os error 13)
```

| Exit status | Meaning |
|-------------|---------|
| `0` | The folder was read. Files that could not be removed do not change it. |
| `1` | No `--dir` and `HOME` is unset, empty or relative; or the folder does not exist or cannot be read. |
| `2` | Usage error. |

```
error: prune-probe-logs needs an absolute HOME, or --dir
error: no such directory: "/home/ryan/.claude/projects/-home-ryan"
```

A session that has so far run only `/usage` matches the rule, including an
interactive one still open. Run `--dry-run` first, and prune while no
interactive `claude` session is running in your home directory. See
[Remove logs left by old usage probes](../howto/claude-profile-rotation.md#remove-logs-left-by-old-usage-probes).

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
| `AMPLIHACK_RECIPE_RUN_ID` | Set by `amplihack recipe run` for its steps. When it is set and non-empty, a launch never starts the rotator. See [Rotate autostart](#rotate-autostart). |
| `INVOCATION_ID` | Set by systemd for the processes of a unit. A `rotate watch` that sees it non-empty, and runs directly in a service as described in step 2 of [Owner unit name](#owner-unit-name), records itself as the [rotation owner](#rotate-owner). A shell in a `.scope` that inherited it does not. |
| `<root>/.auto-switch.d/rotate/owner` | The [owner marker](#rotate-owner). While it exists, launches do not start a rotator. Removed only by `rotate owner clear`. |
| `AMPLIHACK_USAGE_LIMIT_WAIT` | `true` or `1` turns the [usage-limit wait](#usage-limit-wait) on, `false` or `0` off. Outranks `recipe.usage_limit_wait` in `config.toml`. See [Usage-limit wait settings](#usage-limit-wait-settings). |
| `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | Upper bound on one usage-limit wait, in seconds; default `21600` (6 hours). It shortens a [reset wait](#reset-time) and sets the length of a [capacity wait](#capacity-wait). Outranks `recipe.usage_limit_max_wait_secs`. See [Usage-limit wait settings](#usage-limit-wait-settings). |
| `CODEX_PROFILES_ROOT` | Directory whose subdirectories are the Codex accounts `profile usage` reads; default `$HOME/.codex-profiles`. Empty means unset. See [Which accounts are read](#which-accounts-are-read). |
| `CODEX_BIN` | The Codex program `profile usage` runs as `<bin> app-server`; default `codex`, found through `PATH`. Like `PATH`, it decides which program runs. Empty means unset. See [Live read](#live-read). |
| `AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS` | Seconds each Codex account's live read may take, `1` to `300`; default `20`. See [Codex read deadline](#codex-read-deadline). |

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

Only a tier-3 selection whose directory hosts another login under an active
lease can use the [default-profile lease fallback](#default-profile-lease-fallback).
Explicit and inherited selections retain their existing behaviour.

When a profile resolves, its name and directory are set on the child `Command` only
(never `set_var`), and only for Claude-compatible binaries. This applies to
every command that starts Claude with a profile: `amplihack claude`,
`amplihack launch`, `amplihack profile run` and `amplihack profile login`.

| Variable | Value |
|----------|-------|
| `CLAUDE_CONFIG_DIR` | `<root>/<name>`, overriding any ambient value |
| `AMPLIHACK_CLAUDE_PROFILE` | `<name>` |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Ordinary named launches keep an absent or empty inherited root as-is, and pass a resolved root when the inherited value is non-empty. A fallback exports its resolved `<root>` so descendants and supervised attempts keep that destination even if the saved root changes. |

For a [default-profile lease fallback](#default-profile-lease-fallback), the
child receives the selected profile's name, directory and root, including an
explicit root when it came from the saved file or default location. This pins
the fallback's root for descendants and supervised attempts. These values are
applied after other environment builders, so an ambient `CLAUDE_CONFIG_DIR`
cannot replace the selected destination.

Copilot and Codex children get none of these, but variables already in the
environment pass through them unchanged.

Outside the lease fallback, a root that came from the saved file or the default
is not exported, so a nested launch reads
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

An ordinary profile launch selects a configuration directory; it does not change Claude's
authentication environment. Claude Code documents separate directories for
claude.ai logins and API-key accounts, but two Claude Console sign-ins without
API keys are not isolated because their credentials live outside the
configuration directory. An inherited `ANTHROPIC_API_KEY` can also select API
billing instead of a subscription login. See the official
[multiple-account](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts)
and [environment variable](https://code.claude.com/docs/en/env-vars)
documentation.

The [lease fallback](#default-profile-lease-fallback) additionally refuses
authentication and billing-route substitutions that cannot be bound to the
validated login. Ordinary explicit and inherited launches retain the above
environment behaviour.

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
| `auto-switch off [--exclude a,b]` | Writes `enabled=off`. The threshold and exclusions are kept; `--exclude` replaces the exclusions, and is needed only when `.auto-switch` does not parse or others can write it ([Unreadable exclude list](#unreadable-exclude-list)). Logins in running sessions are not moved; they return when each session ends. Like `auto-switch on`, appends a [`change:` line](#rotate-change-log) to the rotate log when `enabled` changes. |
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

The `5h` cell shows `no session window` when the account's reading has
`seven_day` and no `five_hour`: no session window was recorded, and the
account is ranked on its week (see [Readings](#readings)). This is not the
same as [no session limit](#no-session-limit): the stored reading cannot
tell a plan with no session limit from a session window that was not
recorded (see the known limitations there). An account with neither window,
or no reading, shows `-`.

```
profile   5h                 7d    observed   login   wrapper
weekly    no session window  47%   2m ago     home    installed
work      93%                61%   5m ago     home    installed
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
| `profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); …` | Two logins are the [same account](#account-identity): from each profile's own `.claude.json`, from its saved block while its directory hosts another account, or `$HOME/.claude.json` (shown as `~/.claude`). The fix names the profile whose login is not its [recorded account](#recorded-account), or `~/.claude` when it is not the live profile's recorded account, or the line says amplihack cannot tell which one is wrong. See [Duplicate-login warning](#duplicate-login-warning) for every form. No uuid, organization or e-mail is printed. |

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
| `exclude` | Comma-separated profile names, may be empty. An excluded profile is never chosen, and no amplihack command probes it or refreshes its token; see [Excluded profiles](#excluded-profiles) and [Rotate excluded live profile](#rotate-excluded-live-profile). | empty |
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
| corrupt regular file owned by you | replaces it; `threshold` starts from its default unless given. `exclude` keeps the file's list when it parses and is private; when it does not parse, `on` and `off` are refused with `AutoSwitchExcludeAtRisk` unless `--exclude` names the list (`on --exclude …` or `off --exclude …`; see [Unreadable exclude list](#unreadable-exclude-list)) | refused with `AutoSwitchExcludeAtRisk`, naming the recovery: `on` or `off` first when it parses, `on --exclude …` or `off --exclude …` when it does not |
| owned by another user | refused with `AutoSwitchExcludeAtRisk`, even with `--exclude`: while it is there every profile counts as excluded, so the message says to fix the owner with `chown`, not to delete it | refused with `AutoSwitchExcludeAtRisk`, as for `on` / `off` |
| owned by you, writable by group or others | its list is not trusted (anyone who can write the file could have removed an entry), so `on` and `off` are refused with `AutoSwitchExcludeAtRisk` unless `--exclude` names the list; with it, the file is replaced with mode `0600` | refused with `AutoSwitchExcludeAtRisk`; run `on --exclude …` or `off --exclude …` first |
| symlink or not a regular file | refused with `AutoSwitchExcludeAtRisk`, even with `--exclude`: the message says to replace it with a regular file of yours holding the same `exclude=` line, not to delete it | refused with `AutoSwitchExcludeAtRisk`, as for `on` / `off` |

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
- **No session window.** A reading may hold `seven_day` with no
  `five_hour`: Claude reported no session window for that login (see
  [No session limit](#no-session-limit)), or the status line gave none. It
  is judged on `seven_day` alone. When `profile usage` records such a
  reading, a stored `five_hour` is kept until its reset time passes; see
  [Refreshing auto-switch readings](#refreshing-auto-switch-readings).
  `usage.json` has no `session_limit` field and its format is unchanged.
- **Merging.** The reading with the newer `observed_at` wins; on a tie, the
  one with the higher usage wins.
- **Errors.** A damaged `usage.json` is treated as empty and overwritten by
  the next reading.
- **Readings from `~/.claude`.** The
  [`~/.claude` status line](./STATUSLINE.md#recording-readings) never writes
  `usage.json`. It records into `<root>/.auto-switch.d/home-usage.json`,
  which has the same format and is read only by the status line's spare
  count: nothing checks which account `~/.claude` held when such a reading
  was taken (see
  [Why the readings are kept apart](./STATUSLINE.md#why-the-readings-are-kept-apart)).
  Passive readings, candidate selection, `profile rotate` and the capacity
  file never read it.

### Candidate selection

An account is *exhausted* when the higher of its two adjusted windows is
at or above the threshold (`max(five_hour, seven_day) >= threshold`). An
*adjusted* window is one whose `resets_at` is at or before now, which counts
as 0%. A window without `resets_at` is used as read. A reading with no
`five_hour` is judged and ranked on `seven_day` alone, so a login with
[no session limit](#no-session-limit) is eligible while its week is below
the threshold. An account with no
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
no reading. If none is eligible, nothing is switched. An excluded
account is never chosen, and its stored reading is never refreshed by a
probe. When an excluded account is the live one, the rotator moves off it
without probing or refreshing it; see
[Rotate excluded live profile](#rotate-excluded-live-profile).

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
2. A session on an overflow profile moves to the plan profile with the most
   [room](#rotate-plan-room-swap), by the rotator's plan-room ranking, when
   any plan profile is below 100% on both windows. Otherwise, a session on an
   overflow profile that is not
   [spend-blocked](#overflow-spend-limit-block) stays where it is. Its own
   plan reading plays no part: a credit account reads at or near 100% of
   its plan limits while it spends credits, and that is the account
   overflow exists for. Only a spend block moves a session from one
   overflow profile to the next, as in the rotator.
3. An overflow profile, only when **both** hold:
   - no plan profile has [room](#rotate-plan-room-swap) below 100% on both
     windows, the current one included, whether or not this session could
     use it (one leased to another session still counts, so no money is
     spent while it exists). A plan profile with no reading, or whose probe
     failed, counts as having no room. The API-key profile also waits for
     every plan profile with no reading, which may have room: it is passed
     over, and a launch or rerun writes `ALERT: API key not used
     session=<account>: …; auto-switch from <profile>` to stderr every time
     and to the rotate log once per set of unmeasured profiles (again once
     the set changes or every plan profile has a reading). `session=` is the
     account of the session that ran out; `live=`, in the rotator's form of
     the line, is the login in `~/.claude`. Excluded profiles do not count,
     and neither do those the rotator skips as `no login` or `not oauth`,
     which nothing can measure. Nor does a profile the rotator's last
     measuring cycle found with no room, while the ledger's
     [`no_room`](#api-key-ledger-fields) record holds: auto-switch has no
     reading of its own for the login in `~/.claude`, and the rotator
     measures it every cycle. The record ends at the first reset the
     rotator read for a login it names, since that login has room again.
     It names every login the cycle measured, so a login the rotator has just moved off is covered too.
     Without a record in force, as when no rotator runs or the rotator
     found room on the login, it counts until a reading is recorded for it
     (a session on it, or `profile usage`) or it is added to `exclude`. The
     capacity file plays no part here: the rotator writes it as `ok` when
     it can use the key, and the status line and `profile usage` rewrite
     it. A current plan profile
     at 82%, or at
     97%, still has room: the rotator would not leave it for overflow, so
     the session stays on it rather than spend, and it cannot move onto
     overflow only to move straight back;
   - the earliest plan reset is more than the overflow wait away. The plan
     reset of an account is the reset of its window at its threshold, or
     the later of the two when both are; the earliest plan reset is the
     earliest of those over every plan account, the current one included.

   The first overflow profile by tier, then by list order within a tier,
   that is not current, not excluded, not spend-blocked and eligible is
   chosen. Its own plan reading is not checked, so a tier 1 credit account
   at 100% of its plan limits is still chosen before the API key. The
   API-key profile is tier 10, after every credit profile, and is a
   candidate only when it is [marked](#api-key-rotation) and passes its
   [login check](#api-key-login-check).
4. Otherwise the best plan profile below the auto-switch threshold, by the
   order above. A plan account between the gate's thresholds and the
   auto-switch threshold (for example at 87% session) is used only here:
   when overflow is not allowed, it beats waiting, and it costs nothing.

The rotator decides in the same order (plan capacity, then plan room, then
per-token profiles by tier; see [Rotate decision order](#rotate-decision-order)),
and both use the same room test before spending, so on the same readings the
two agree on when money is spent, for any auto-switch threshold. The
auto-switch threshold only decides when a session on a plan profile asks,
and which plan profiles it may move to from another plan profile. In
particular, a plan account at 87% session has room for both: neither moves
onto an overflow profile while it exists, whatever the plan reset.

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
| exactly `amplihack profile statusline`, or a quoted path to `amplihack` followed by it, with no `--owner` (the `~/.claude` entry) | saves `~/.amplihack/.claude/tools/statusline.sh` as the original when that script passes the [script checks](./STATUSLINE.md#script-checks), otherwise saves no original; then installs the wrapper | same as `install` |
| an old, moved, edited or doubly wrapped wrapper (has the words `profile statusline` and `--owner`) | does not write; reports `stale` | rewrites it to the canonical wrapper, keeping the saved original (never saving a wrapper as the original) |

The `~/.claude` entry is never saved as an original, and a saved original
that is exactly that entry, left by an older install, is never run:
`~/.amplihack/.claude/tools/statusline.sh` runs in its place. A user's command
that does more, such as `amplihack profile statusline | my-filter`, is another
command: it is saved and run, and the inner `profile statusline` runs only
`statusline.sh` ([details](./STATUSLINE.md#inside-profiles)).

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

After the original output, the wrapper prints the
[login capacity line](./STATUSLINE.md#login-capacity-line) when the step 1
checks passed and the saved original is `statusline.sh`. A wrapper around
your own command adds no capacity line, and a wrapper with no saved original
prints none (see [Inside profiles](./STATUSLINE.md#inside-profiles)). The
[capacity notice](#capacity-notice) follows while it is due. Step 2 also
updates the [capacity file](#capacity-file) when the recorded reading is at or
over the threshold, or the file reports low or no capacity; any other reading
cannot change the file.

`--root` may be left out; the root is then resolved as for other profile
commands (see [Profile root](#profile-root)). Run without `--owner` inside
`<root>/<name>`, `amplihack profile statusline` behaves as
`--owner <name>`, except that it never swaps or updates the capacity file;
see [Modes](./STATUSLINE.md#modes).

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

The wrapper writes to standard output itself only to add the capacity line
and the [capacity notice](#capacity-notice) after the original output. Its exit code is the
original command's, or `0` if there is none or it was not run. Its own errors, including a
panic, are one line on stderr naming paths only.

The wrapper makes no network request and never opens a profile's
`.credentials.json`. It records the `rate_limits` Claude passes it and reads
`usage.json`, `home-usage.json` and the ledgers. A profile in the
auto-switch `exclude` list is therefore never probed or refreshed by it,
and an in-session swap never chooses one (see
[Candidate selection](#candidate-selection)).

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
| Launch or `run` of profile `X` | directory `X` is hosting another account, unless the launch [joins](#joining-a-hosted-session) the session using it; a saved default first tries the [lease fallback](#default-profile-lease-fallback) | `HostsForeignAccount`, or default-conflict failure if no fallback is usable |
| `login X` | directory `X` is hosting another account (no exception) | `HostsForeignAccount` |
| `remove X` | `X` is in the ledger, has a parked login, or hosts another account | `Leased` or `HostsForeignAccount` |

#### Joining a hosted session

A launch *joins* the session in directory `X` only when all of these hold:

- the profile was inherited (tier 2 of
  [resolution](#resolution-and-environment)), not given by `--profile`,
  `profile run` or the default pointer;
- the launcher's own `CLAUDE_CONFIG_DIR` is set; and
- that value, canonicalised, is equal to the canonical path of `<root>/X`.

A lease-fallback launch and its descendants cannot join a hosted session,
even when their inherited name and directory match. Inherited environment
values and supervisor context do not grant permission to join.

The comparison is between whole canonical paths. It never uses the value of
`AMPLIHACK_CLAUDE_PROFILE`, a profile name, or a string prefix. A fresh shell
that runs `export AMPLIHACK_CLAUDE_PROFILE=work` without the session's
`CLAUDE_CONFIG_DIR` does not join and is refused with `HostsForeignAccount`.

A launch that joins shares the session's current login. The checks run under
the global lock and again just before spawn.

Leases are interlocks against accidents. They are not a security boundary
against other processes running as you.

#### Default-profile lease fallback

The fallback supports named profiles with file-backed credentials or macOS
Keychain storage. Supervisor files carry intent only; each process creating
a Claude child authenticates the login afresh. Final policy validation uses
the full relevant candidate pool.

The Linux acceptance regression launches an actual fallback child, releases
state and refresh protection, then uses normal `/proc` discovery to register
that child's PID. Removal must then refuse the live directory through normal
session detection. Socket events order these steps; no synthetic process
table is used for that regression.

After [lease recovery](#session-end-and-recovery), `amplihack claude` and
`amplihack launch` choose an alternative when all of these hold:

- Selection came from `<root>/.default`, without an explicit or inherited
  profile.
- The selected directory hosts another profile's login under an active lease
  (`HostsForeignAccount`).
- The mode supports Claude profiles; Docker and `--auto` are unchanged.

Lease protection remains active when auto-switch is off. This fallback does
not handle a default's login being leased elsewhere (`Leased`), ordinary
quota exhaustion, a corrupt pointer or unrelated validation errors. Explicit
`--profile` and `profile run` retain their existing refusals. Inherited
selection retains [session joining](#joining-a-hosted-session) and
[capacity substitution](#nested-launches-and-auto-switch).

**Eligible destinations.** Only named profiles under the selected root that
hold their own usable login qualify. The conflicting directory, the login it
hosts, logins lent elsewhere, directories hosting another account and parked
credentials are excluded. The rotator's `~/.claude` login is not a destination
for this fallback; ordinary [live-profile routing](#live-profile-launch)
remains separate.

Selection reuses [candidate ordering](#candidate-selection), exclusions,
capacity checks, [organisation affinity](profile-organisation-affinity.md),
login-kind checks, [overflow gates](#auto-switch-and-overflow-profiles) and
billing safeguards. Affinity uses the original host and the ledger-resolved
hosted login, rather than assuming the default's name identifies the active
account. Exclusions apply before credential requests or refreshes. A
credential file's presence alone does not establish identity or usability.
Missing usage readings keep their existing treatment. Unavailable plan
logins still count towards overflow gates where existing policy requires
it; a lease does not authorise paid overflow on its own.

**Credential storage and identity.** A missing `.credentials.json` does not
disqualify a profile with a usable Keychain login. On macOS the read uses
Claude Code's directory-specific Keychain service, with canonical Unicode
composition of the configuration path. An inaccessible Keychain entry is
refused without prompting or silently substituting another login. The login must belong to the selected
configuration directory, and credential authentication establishes the
account and organisation against the profile's expected-account record.
Directory names, displayed account metadata and usage readings are not
identity proof. If identity or storage continuity cannot be established,
the destination is refused without weakening the billing safeguards.

Network identity verification happens before launch locks are acquired.
Authenticated credential evidence stays in the verifying process and is
checked again under launch protection for continuity with the authenticated login.
Tokens, credential contents, token hashes and sensitive continuity evidence
are absent from diagnostics and supervisor files. Keychain payloads are
compared in memory during local continuity checks and are never exported.

**Effective authentication.** Before identity requests and again against the
final child command, fallback refuses non-empty `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, token/key descriptor
overrides, custom authentication headers, API routing overrides, alternate
cloud providers and LiteLLM gateway configuration. Console profile and
federation credential selectors also refuse, as do implicit Console profiles
in `~/.config/anthropic/active_config` or `configs/default.json` (also checked
under a non-empty inherited `XDG_CONFIG_HOME/anthropic`).
User, profile, project and local settings are checked before identity requests;
CLI settings are also checked against the final command. The system
`managed-settings.json` and every visible `managed-settings.d/*.json` drop-in
are checked for authentication helpers and environment substitutions at both
boundaries, including when the main managed file is absent.
Drop-ins use bounded directory enumeration and
descriptor-relative, bounded no-follow reads; unreadable, linked, malformed,
non-regular or oversized sources refuse. Policy helpers, native macOS managed
preferences (device or per-user), WSL's inherited Windows policy, and an existing profile `remote-settings.json`
cache conservatively refuse because their effective output cannot be bound
by this local validator. Administrator policy is never modified. The native
preference check also refuses another user's deployment for this domain.
The supported boundary is local startup configuration for standalone Linux
and macOS CLI launches: `/etc/claude-code` on Linux, and
`/Library/Application Support/ClaudeCode` plus device and per-user
`com.anthropic.claudecode.plist` files under `/Library/Managed Preferences`
on macOS. Other operating systems refuse fallback; Windows registry policy
is not inspected. Policy fetched or changed later by Claude is outside this
boundary; these checks do not establish complete remote-policy coverage. See
[managed delivery mechanisms](https://code.claude.com/docs/en/managed-settings).
Unsupported substitutions produce a fixed,
credential-free refusal; they are never silently cleared. An external
API-key login without a profile-owned authenticated OAuth identity cannot
qualify as a fallback. Ordinary named launches keep their existing behaviour.

On Linux and macOS, a non-empty `CLAUDE_CODE_CUSTOM_OAUTH_URL` must match the
identity client's validated API base before any credential-bearing request.
Comparison uses the resolved endpoint, including rejection of unsafe endpoint
overrides, rather than trusting the raw environment variable. A compatible
custom namespace remains supported. Each supervised launching process repeats
authentication and the final effective-configuration check; context format
`2` contains restricted intent, with no credentials or authentication proof.

**Directory continuity.** The state lock is acquired before the credential
refresh lock and retained through shared-directory preparation, private
context creation, final command validation and child creation. Removal takes
the same state protection before deleting the directory and holds it through
default-pointer updates. A concurrent removal refuses while launch protection
is held; on Linux, normal process discovery protects the directory afterwards.
On platforms where liveness is unavailable, removal with auto-switch state
fails closed. Profile creation cannot overwrite an existing directory, and
rotator and login filesystem changes already participate in state protection.
These are cooperative interlocks for amplihack commands. Arbitrary replacement
by an external process running as the same user is outside this contract;
path checks and cooperative locks do not prevent such replacement.

**Configuration.** Fallback uses the selected profile root and existing
[auto-switch exclusions](#auto-switch-commands),
[capacity rules](#candidate-selection),
[organisation affinity](profile-organisation-affinity.md) and
[overflow settings](#auto-switch-and-overflow-profiles). There is no new
flag, preference, environment setting or public API. Turning auto-switch off
does not disable lease protection or make a borrowed login available.

**Launch and supervision.** Claude receives the selected profile's name,
directory and root through the [launch environment](#resolution-and-environment),
with the requested arguments preserved. Descendants inherit that destination
and run the usual authoritative checks. Supervision watches the selected
login from its first attempt.

**Supervisor authentication.** The supervisor carries destination intent only.
The process creating each actual Claude child obtains fresh authenticated
evidence before taking launch locks. The supervisor context cannot serve as
credential proof. Further policy-approved attempts validate their own
destinations.
Resolving a fallback does not enable reruns that the original request would
not permit, or grant permission to join a hosted session.

Arguments retain their order and literal values, including `--resume` and
session identifiers containing spaces. Ordinary fallback launches preserve
the child's exit status: a child exiting 37 makes amplihack exit 37. Child
creation failures remain launch errors. Existing SIGINT, SIGTERM and SIGHUP
handling is preserved; cancellation does not trigger another attempt.

**Final validation.** Selection is provisional. Immediately before the actual
Claude spawn, amplihack rechecks the root, directory, credentials, identity,
login kind, lease state and applicable eligibility and billing policy against
the full relevant candidate pool, including both usage windows and unknown
readings. Changed local candidate evidence refuses the hand-off; refreshed
usage is evaluated against the whole pool, so returning peer capacity can
withdraw permission to cross organisations or use paid overflow.

State protection is acquired before credential-refresh protection. Both stay
held through validation, environment application, reporting and actual Claude
child creation, including supervised launches. Starting a supervisor wrapper
alone does not end protection. Final validation makes no network requests.
Unavailable protection or changed evidence stops the launch promptly, without
timed retries or silent reselection. Only protection owned by the launch is
released.

Supervisor files use bounded reads, private ownership and permissions,
no-follow access and validation of the opened file. Unsupported formats,
inconsistent destinations and malformed contexts are refused before Claude
starts. Cleanup removes only context owned by that launch. The fallback and
its descendants cannot join another session's hosted directory.

A success notice names the conflicting default, its hosted login and the
selected profile only after validation succeeds. Fallback does not rewrite
`.default`, move active or parked credentials, acquire a new session lease or
change the existing lease. The running session and its valid nested launches
retain access to the borrowed login.

**Failure and recovery.** If no destination qualifies, launch exits with
status 1 before Claude starts. The diagnostic identifies the default and
hosted login and includes all of these. The same recovery commands accompany
a fallback refused during final validation:

- `amplihack profile run <other>` to select an available profile with a usable
  login explicitly;
- `amplihack profile switch --none` to clear the default, with an explanation
  that the next launch still requires a usable ambient login;
- `amplihack profile auto-switch status` to inspect leases.

Clearing the default supplies neither credentials nor capacity. Without an
inherited profile, the next launch keeps an ambient `CLAUDE_CONFIG_DIR`, or
uses `~/.claude` if it is unset. Corrupt settings or state, or failed
validation, retain their underlying cause instead of appearing as an empty
candidate pool. See the [recovery examples](../howto/claude-profiles.md#launch-while-the-default-directory-is-leased).

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
(tier 2 of [resolution](#resolution-and-environment)). Explicit selections
and exhausted defaults are not redirected by this capacity rule. Defaults
have separate [lease-conflict handling](#default-profile-lease-fallback).

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
   (`session limit`, `5-hour limit`, `usage limit`, `usage_limit_reached`,
   `quota exceeded`, `monthly limit`, `weekly limit`,
   `hit your opus limit`, `hit your sonnet limit`, `hit your fable limit`,
   `spend limit`) and the same precedence,
   the runner uses to end a run with a usage-limit message. Work markers
   are checked first, then environmental ones, then usage-limit ones, so a
   tail that also matches a work or environmental marker is not a usage
   limit. For example, a `claude -p` stdout tail that contains
   `permission denied` anywhere is classed as environmental and the step
   is not rerun, even if it also says `usage limit`. Two usage-limit-like
   messages are environmental for the same reason and never cause a rerun:
   `credit balance is too low` (an environmental marker, checked first) and
   `disk quota exceeded` (which matches the environmental marker before
   `quota exceeded`).

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

- the gate allows it: no eligible plan account has room (both windows
  below 100%; an API-key profile is never a plan account; the API key also
  needs a reading for each of them), and the
  earliest plan reset is more than the overflow wait away. The plan reset
  here is the gate's, the reset of the window at the gate's threshold, or the
  later of the two when both are, not the reset of the window over the
  auto-switch threshold;
- some overflow profile is listed, not excluded and not spend-blocked. Its
  own plan reading does not count.

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
| `statusline` | By the [status-line wrapper](#status-line-wrapper) each time it records a reading, under the state lock it takes for that. The [`~/.claude` status line](./STATUSLINE.md#recording-readings) never writes the file. | `usage.json` as just recorded. |
| `usage` | By [`profile usage`](#refreshing-auto-switch-readings), each time it records readings, under the state lock it takes for that. | `usage.json` as just recorded. |
| `rotate` | By the rotator, in a [cycle](#rotate-cycle) that reaches the swap, stay or ALERT, under the lock taken in step 6. | That cycle's probes. A `session_exhausted` or `weekly_exhausted` probe counts as at its limit; a failed probe, including one that timed out, counts as no reading and never as at its limit. Reset times come from the profile's `usage.json` entry while they are in the future, and are otherwise unknown. |

All three use the auto-switch threshold and exclusion list, not the
rotator's `--session-at` and `--week-at`, so they agree on the state. None of
them probes or refreshes an excluded profile to write the file: the
`statusline` writer reads only files, the `usage` writer gets no reading for
one (see [Excluded profiles](#excluded-profiles)), and the `rotate` writer
never counts an excluded live login as room (see
[Rotate excluded live profile](#rotate-excluded-live-profile)). None
of them counts an [overflow](#rotate-overflow) profile as eligible: its
capacity is credit capacity, used only by the rotator's overflow rule. While
the rotator's last cycle left the live login on credits, its own write sets
the state to `ok`. The API-key profile counts only while its
[login check](#api-key-login-check) passes: `reachable`, the rotator's check,
in the rotator's writes and in any write while a rotator runs, and otherwise
auto-switch's check of the profile's own files. A live API-key profile that
fails the check does not set the state to `ok`. The
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
| `expected/<name>.json` | The profile's [recorded account](#recorded-account): `{"account":"<key>","org":"<uuid>"|null}`, both keys required (see [Recorded account](#recorded-account)). No `version` field. A damaged file counts as no record; a swap from that profile prints one warning. |
| `statusline/<name>.json` | The profile's original `statusLine` object. |
| `rerun/` | Buffered stdout of supervised steps, `<pid>-<n>.out`; see [Usage-limit reruns](#usage-limit-reruns) for how long each is kept. |
| `capacity.json` | Whether any account has capacity left; see [Capacity file](#capacity-file). Unlike the other state files, it accepts unknown fields and is capped at 16 KiB. |
| `capacity-weights.json` | Optional, written by you. Each login's weight in the capacity summary; see [Capacity weights](#capacity-weights). A JSON object, capped at 64 KiB, with no `version`. |
| `usage-history.jsonl` | One line per measured cycle from `profile usage` and the rotator, kept for eight days; see [Usage history](#usage-history). JSON lines, not one JSON document; read from the last 4 MiB. |
| `usage-history.lock` | Lock for writers of `usage-history.jsonl`. Separate from `lock`. |
| `key-held` | The unmeasured plan profiles named by the last `ALERT: API key not used session=…` that auto-switch wrote to the rotate log, one name per line, so the same set is logged once. Removed once every plan profile has a reading. Not JSON. |
| `notice/<session>` | The last capacity state shown in a Claude session; see [Notice records](#notice-records). Unlike the other state files, it is a bare word, not JSON, and is read up to 64 bytes. |

Apart from `capacity.json`, `capacity-weights.json`, `usage-history.jsonl`
and `notice/<session>`, every state file has
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

The rotator also runs a [daily session archive](#rotate-daily-session-archive)
that moves old session logs out of `~/.claude/projects`.

### Rotate commands

```
amplihack profile rotate [--dry-run] [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1]
amplihack profile rotate start  [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1] [--interval SECS]
amplihack profile rotate watch  [--session-at N] [--week-at N] [--margin N] [--upgrade-margin N] [--hold SECS] [--overflow-wait SECS] [--overflow-unknown-reset 0|1] [--interval SECS] [--supervised[=UNIT]]
amplihack profile rotate stop
amplihack profile rotate status
amplihack profile rotate autostart [on|off]
amplihack profile rotate owner [clear]
```

| Command | Behaviour |
|---------|-----------|
| `rotate` | Runs one [cycle](#rotate-cycle): recover, measure, decide, and swap if needed. Prints its log lines to stdout and appends them to the log. Runs whether the toggle is on or off. |
| `rotate --dry-run` | Measures and decides, then prints `would ...` lines and the `cycle` line to stdout. Takes no lock, writes no rotate state, moves nothing, and never refreshes a token or starts `claude`: every read is a direct read with refresh off, so an expired login shows as unmeasured with its [cause](#identity-read-causes), and the dry run can pick a different target than a real cycle that would have refreshed that login. It prints `would refresh: expired login in ~/.claude` when a real cycle would refresh the live login, and the `would` lines of [seeding](#seeding-a-record) for a live profile with no record. It skips both the [prototype check](#rotate-prototype-coexistence) and [journal recovery](#rotate-recovery). Reports a pending journal as `would recover an interrupted swap (phase <phase>)` and decides as if it were not there. Reads the [pin](#rotate-pin): a pin in force prints `would stay (held by switch to <name>)`, or `would stay (pin unreadable)`, and measures nothing. Makes the same [hand-change](#rotate-hand-change-recovery) identity reads as a real cycle, never writes `hand_suspect`, and reports the result as `would recover: <recovered line>`, `would note: <note>`, or `would alert: <ALERT line>` with its `fix` lines; see the dry-run table there. An inconsistent journal is reported as `would alert: rotate recovery is inconsistent`, and a damaged one as `would alert: the rotate journal is damaged`. |
| `rotate start` | Checks for the prototype, runs recovery under the lock, warns on stderr if auto-switch has been turned off, refuses if a verified rotator is already running, then starts `rotate watch` as a detached process and writes the pid file. The check, the start and the pid file all happen under the state lock, so two `start`s cannot both start one. Fails with `rotate: lock busy` when the lock stays held for 2 seconds, and with the auto-switch error when auto-switch recovery is inconsistent. Pins the child to its root and records that root; see [Rotate root record](#rotate-root-record). Prints `rotate: started (pid <pid>), checking every <interval>s`, `log: <path>` and `root: <path>`, and returns at once. |
| `rotate watch` | Runs a cycle every `--interval` seconds in the foreground until `SIGTERM` or `SIGINT`. A signal is never acted on during a swap; see [Signals](#rotate-signals). Refuses if another verified rotator is running. Writes only to the log, apart from the [owner marker](#rotate-owner) when it runs supervised. |
| `rotate watch --supervised[=UNIT]` | As `rotate watch`, and records itself as the [rotation owner](#rotate-owner) so that launches do not start a second rotator. `UNIT` names the service manager unit, for example `--supervised=amplihack-rotator.service`; the `=` is required. Without `UNIT` the name is found as described in [Owner unit name](#owner-unit-name). A `watch` that systemd runs as a service records itself without the flag: `INVOCATION_ID` is set and the unit is found in `/proc/self/cgroup` as in step 2 of [Owner unit name](#owner-unit-name). Like any `watch`, it refuses with status 1 when another verified rotator is running, and then writes no marker; stop that rotator with `rotate stop` before starting the unit. `rotate start --supervised` is refused with `RotateRefused` (`--supervised applies to rotate watch only`, status 1). |
| `rotate stop` | Finds the rotator as described in [Rotate root record](#rotate-root-record). Sends `SIGTERM` to a [verified](#rotate-daemon) rotator, waits up to 10 seconds for it to exit, then removes the pid file and prints `rotate: stopped (pid <pid>)`. |
| `rotate status` | **Read-only.** Prints the [status lines](#rotate-status) and exits 0. |
| `rotate autostart` | **Read-only.** Prints the effective [autostart](#rotate-autostart) state, for example `autostart: on (default)`. |
| `rotate autostart on\|off` | Writes the autostart setting and prints `autostart: on (setting)` or `autostart: off (setting)`. Does not start or stop a rotator. When the value changes, appends a [`change:` line](#rotate-change-log) to the log. Any other value is a usage error (status 2). |
| `rotate owner` | **Read-only.** Prints the [owner](#rotate-owner) line, as in `rotate status`, for example `owner: none`. |
| `rotate owner clear` | Removes the [owner marker](#rotate-owner) and prints `owner: cleared (was <unit>)`, or `owner: none` when there was no marker. Does not stop a running rotator. Under a profile root that is not private, it reads and removes nothing and fails with `RotateRefused` (`profile root <reason>; the owner marker was left in place`, status 1). |

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
| `overflow add <name> [--tier N]` | Appends `<name>` to the `overflow` list in tier `N` (`1` to `9`, default `1`). For an [API-key profile](#api-key-rotation) the mark is the consent to per-token billing: it is stored as the bare name and always read as tier 10, after every credit tier, and `--tier` is refused with `PlatformTierFixed` before anything is written. The name must pass the [name rules](#name-rules) and be an existing profile directory under the root (not a symlink); otherwise the command fails, names the profile and changes nothing. Prints `overflow: added <name> (tier N)` and the cost note below. For a profile already marked, `--tier` moves it (`overflow: <name> moved from tier A to tier B`), and without `--tier` the tier is kept (`overflow: <name> is already marked (tier N)`); both exit 0. Prints a warning when `<name>` is in the `exclude` list, and another when the [default pointer](#clearing-the-default) names it. |
| `overflow remove <name>` | Removes `<name>` from the list. The name must pass the name rules; it does not have to exist. Prints `overflow: removed <name>`, or `overflow: <name> is not marked` and exits 0; `overflow list` agrees with it, showing the profile as `marked` before a successful `remove` and not at all (or, for an API-key profile, as `not marked (not used)`) after it. A spend block for the profile is left in the ledger and expires on its own. |
| `overflow list` | **Read-only.** Prints one line per marked profile, in the order the rotator and auto-switch try them (by tier, then list order within a tier): the name, `tier N`, `marked`, then `missing` when the root has no profile directory of that name, then `blocked until <time> (spend_limit)` when it has an active block. After them comes one line per [API-key profile](#api-key-profiles-in-commands), marked or not: `<name>  api-key tier  marked` or `<name>  api-key tier  not marked (not used)`. Prints `overflow: none` when there is no line to print. Takes no lock. A rotate ledger that cannot be read prints a warning on stderr and the marks without blocks; the command still exits 0. |
| `overflow clear <name>` | Removes the spend block for `<name>` from the rotate ledger. Takes the rotate state lock, retrying for up to 10 seconds; if it is still held, fails with `RotateDaemon` and the message `rotator busy, retry`. Prints `overflow: cleared the spend block on <name>`, or `overflow: <name> has no spend block` and exits 0. The name must pass the name rules; it does not have to be marked or exist, so a block left after `overflow remove` can still be cleared. Never creates the ledger. |

What `add` prints for a new mark, with the cost note:

```
overflow: added ryan-mistt-org (tier 1)
note: ryan-mistt-org is used only when no measured plan login is below 100% on both windows and the earliest plan reset is more than --overflow-wait away; work on it is billed to that account.
note: set a spend limit for the organization at https://claude.ai/settings/usage; the rotator is not a spending cap.
```

The warnings `add` prints, on stderr:

```
warning: ryan-mistt-org is in the auto-switch exclude list; excluded profiles are never used, even as overflow
warning: the default profile is ryan-mistt-org; launches use it directly and spend credits whatever the rotator does (run: amplihack profile switch --none)
```

`overflow list` with one blocked profile:

```
ryan-mistt-org  tier 1  marked  blocked until 2026-11-01T00:00:00Z (spend_limit)
team-credits  tier 1  marked
old-credits  tier 1  marked  missing
api-key  api-key tier  not marked (not used)
```

What `add` prints for an API-key profile:

```
overflow: added api-key (api-key tier, tier 10)
note: api-key is billed per token; it is used only when every plan login is measured and none is below 100% on both windows, no credit profile can be used, the earliest plan reset is more than --overflow-wait away and its login check passes.
```

`overflow list` after that `add`:

```
ryan-mistt-org  tier 1  marked  blocked until 2026-11-01T00:00:00Z (spend_limit)
team-credits  tier 1  marked
old-credits  tier 1  marked  missing
api-key  api-key tier  marked
```

#### Overflow tiers

A credit profile's mark has a tier from 1 to 9. Tier 1 is tried before
tier 2, and so on; within a tier, the order the profiles were added decides.
Tiers rank credit profiles by cost, so the order of names in the file never
has to carry that meaning. A marked [API-key profile](#api-key-rotation) is
always tier 10, after every credit tier, whatever tier the file gives it.
Only marked profiles have a tier: an API-key profile that is not marked has
none and is never used. In `.auto-switch` an entry is written
`name:tier`, and a tier-1 entry as the bare name, so a file written before
tiers existed reads the same:

```
overflow=ryan-mistt-org,team-credits:2,api-key
```

Here `team-credits` is tier 2 and `api-key`, an API-key profile, is tier 10
whatever its position or written tier.

Every tier, credit and API key alike, is billed per token or per credit, so
none of them is used while a measured plan login has room below 100% on both
windows. The API key also waits for every plan login to be measured.
See [Rotate decision order](#rotate-decision-order).

The rotator and auto-switch move onto an overflow profile only from a plan
profile, or from an overflow profile that is spend-blocked or exhausted. They
do not move from a tier-2 profile to a tier-1 profile that becomes usable
again; they move back to a plan profile as soon as one has capacity, or has
[room](#rotate-plan-room-swap).

`add` and `remove` change only the `overflow` key of `<root>/.auto-switch`,
through the same locked write as the [auto-switch commands](#auto-switch-commands)
(see [Settings file](#settings-file)). A missing file is created with
`enabled=off`. A corrupt file is refused with `AutoSwitchExcludeAtRisk` and left
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
   read the `exclude` list (an [unreadable list](#unreadable-exclude-list)
   counts every profile as excluded and pauses the cycle before it
   measures), then [rotate recovery](#rotate-recovery), then the one refresh of an
   [expired live login](#an-expired-live-login), then the
   [unreadable-ledger stop](#unreadable-ledger-stop), then
   [hand-change recovery](#rotate-hand-change-recovery), then
   [identify](#rotate-live-profile) the live profile, [seed](#seeding-a-record)
   its record if one is missing and allowed, and find the
   [candidates](#rotate-candidates). Hand-change recovery makes its
   identity reads here, under the lock, unless the last swap is under
   `SWAP_GRACE` old. Identification and swap authorization have their own
   required trusted readings, including during that recovery grace. If
   recovery refused or holds an open suspicion, it writes its note or guidance and the cycle ends as `alerted`; if it moved a login, or
   found nothing, the cycle carries on from the resulting state. An ALERT about the live login
   (`no-login`, `hand-change`, an identity refusal, or `ambiguous`) is written here and ends the
   cycle. Then check the [rotation pin](#rotate-pin): if it holds rotation,
   log the `skip: held by switch ...` or `skip: pin unreadable ...` line,
   clear the stored ALERT key, record the cycle as `held` (unless the last
   cycle is the `switched` record of a [`profile switch`](#rotate-switch),
   which is kept) and end the cycle without measuring. Release the lock.
4. **Measure.** Read the live login and every candidate, as
   [`profile usage`](#how-a-profile-is-probed) does (passive, direct, then
   `claude`). A live profile in the `exclude` list is not read; see
   [Rotate excluded live profile](#rotate-excluded-live-profile). Reads use the same
   [method, deadline and parallelism bounds](#probe-settings-in-the-rotator).
   The cycle waits for the reads only until `--interval` minus 2 seconds
   after it started; a read still running then is used by a later cycle,
   as described in [Reads that outlast a cycle](#reads-that-outlast-a-cycle).
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
5. **Decide.** See [Decision rule](#rotate-decision-rule). A swap target
   chosen by its reading then passes the
   [freshness gate](#rotate-reading-freshness-and-swap-back-damping), which
   may read it again and decide again. No lock is held.
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
   [spend blocks](#overflow-spend-limit-block) and drop expired ones. A stay
   records `stayed` or `stayed-unread` and a no-capacity ALERT is hard or
   advisory, by the live reading; see
   [Stays and capacity](#rotate-stays-and-capacity). When the
   cycle stays or raises the no-capacity ALERT and the live profile is known,
   also record it as `live`, with its `account`. Update the ledger's
   [`swapped_away`](#rotate-reading-freshness-and-swap-back-damping) entries:
   lift those that a later reading shows to have room, add the login just
   left when the swap moved off a full one, and prune old entries.

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
| The live profile, full trusted live account, material ownership evidence, prototype ledger state, display-file safety of `~/.claude.json`, or login kind in `~/.claude/.credentials.json` differs | `live` | Every decision |
| The [rotation pin](#rotate-pin) differs | `pin` | Every decision |
| The [overflow](#rotate-overflow) list or its ignored entries differ | `overflow` | Every decision |
| The swap target is no longer a candidate | `target eligibility: <reason>` | Swap only |
| The swap target is still a candidate, but its full trusted account differs from step 3 or cannot be bound to the current login | `target identity` | Swap only |

The live and target accounts come from [login-bound trusted
readings](#rotation-login-identity). Session JSON rewrites do not change
these fields. A missing or unsafe login, or unavailable identity evidence,
cannot retain an earlier authorization.

For the target, `<reason>` is the [skip reason](#rotate-candidates) it now
has, such as `no login`, `excluded`, `session running`, `parked` or
`not oauth`, or `gone` when the profile no longer exists. When `/proc` can no
longer be scanned, every profile is skipped as `session running`, so a swap
ends with `target eligibility: session running` and a stay or ALERT goes
ahead. Whether `/proc` can be scanned is not compared in its own right: a
change in it shows only through the target's skip reason, so it has no
label of its own and never affects a stay or an ALERT.

The target's full trusted account is compared with the snapshot using the
[account identity](#account-identity) rule, including organization. A changed
login invalidates its binding. If usage measurement refreshed the token,
fresh matching identity evidence is required before the old measurement
can support a swap. An account key alone cannot prove continuity.
Material records and duplicate claims are revalidated when they support
the selected placement, adoption or backfill; unrelated profile changes
remain irrelevant.

Every usage-based swap also passes the
[freshness gate](#rotate-reading-freshness-and-swap-back-damping) under the
lock, using the current clock and ledger, even when none of the snapshot
dependencies changed. Unchanged identity and eligibility do not prove that
usage evidence is still fresh. This check uses `Reread::Never`: it makes
stale targets unknown and decides again on the retained readings. Any
replacement target must pass the same freshness, identity and eligibility
checks. No external usage read runs under the lock. The unchanged-state
path applies this gate before proceeding, as does target-change reconsideration.

When the snapshot is unchanged but the swap target's reading is no longer
fresh under the lock, for example because it has aged past the age limit
while the cycle measured, the cycle decides once again from the retained
readings, with no read, as in steps 1 to 3 below. If that gives the same
swap, the cycle proceeds. If it gives a swap to a different target that is
a candidate in the fresh state with its step 3 account, the cycle logs
`revalidate: target <old> no longer eligible (stale reading); reconsidered`
and swaps to the new target. Any other result, including a stay or an
ALERT, ends the cycle with the bare `skip: state changed during measurement`,
with no labels, since nothing in the snapshot changed.

Exclusion is checked for the target only. Exclusion of the live
profile is decided earlier, when the cycle reads the `exclude` list under
the lock, and is not compared again here: a live profile excluded when the
cycle loaded the `exclude` list under the lock is never probed and never [re-read](#re-reads) in that cycle, and is swapped away from as described
under [Rotate excluded live profile](#rotate-excluded-live-profile). A change
to `exclude` made while a cycle measures takes effect on the next cycle.

A `journal`, `live`, `pin` or `overflow` change ends the cycle with a skip.
One of these means a person or another process has acted, for example with
[`profile switch`](#rotate-switch) or `auto-switch` settings, and the
rotator does not act against it. The next cycle measures again.

A readable session block naming the previous account does not require an
identity retry or invalidate a trusted reading. The existing readability
gate remains: a missing/unusable `oauthAccount` pauses movement and uses
the documented readability retry, even when trusted identity is available.
A changed login does require fresh bound evidence.
If that evidence is unavailable or conflicting, the operation refuses
before movement and remains visible on subsequent affected attempts.

When the only changes are to the target's eligibility or identity, the
cycle reconsiders once. The unchanged-state case above follows the same
steps:

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
   The new target passes the
   [freshness gate](#rotate-reading-freshness-and-swap-back-damping) without
   any read: a target that is stale counts as unknown, and the decision
   runs again without it. That includes a target with a
   [`swapped_away`](#swap-back-damping) entry in the ledger read under the
   lock whose reading is not strictly later than that entry. No `reread:`
   or `damp:` line is written.
4. If it gives a swap to a profile that is a candidate in the fresh state
   with its step 3 account, the cycle logs
   `revalidate: target <old> no longer eligible (<reason>); reconsidered`,
   where `<reason>` is the old target's bare skip reason, such as
   `session running`, `gone`, `account changed` for a
   `target identity` change, or `stale reading` for the unchanged-state
   case, without the `target eligibility: ` text used
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
2026-10-29T20:16:25Z cycle live=work work:s=100%,w=72% spare:s=0%,w=43% third:s=0%,w=50%
2026-10-29T20:16:31Z swap work->spare live s=100% w=72% target s=0% w=43% reason=threshold
```

If instead `spare` starts a session while the cycle measures, the cycle
reconsiders and takes `third`:

```
2026-10-29T20:16:25Z cycle live=work work:s=100%,w=72% spare:s=0%,w=43% third:s=0%,w=50%
2026-10-29T20:16:31Z revalidate: target spare no longer eligible (session running); reconsidered
2026-10-29T20:16:31Z swap work->third live s=100% w=72% target s=0% w=50% reason=threshold
```

With `spare` the only candidate, the same change ends the cycle:

```
2026-10-29T20:16:31Z skip: state changed during measurement (target eligibility: session running)
```

A [held cycle](#pin-and-overflow) that records a spend-limit block makes the
same comparison with the rules for a stay: it stops only on a `journal`,
`live`, `pin` or `overflow` change, logs the same skip line, and never
reads again or reconsiders.

The liveness scan behind `live`, `session running` and the target's
eligibility reads the system process table, `/proc`. In the test suite, a
fixture can supply a synthetic process table instead, so that sessions
running elsewhere on the host do not change a test's result (#1860). The
table is the one named by `AMPLIHACK_TEST_PROC_ROOT`, selected through
`procfs::proc_root()`. The override is honored only when
`AMPLIHACK_TEST_HOME_GUARD` is non-empty and the canonical path is a
directory strictly inside the canonical temporary directory, owned by the
current user and not writable by group or others. Otherwise the scan uses
`/proc`. The shared rotate fixture
(`tests/support/profile_rotate.rs`) gives every fixture an empty synthetic
table by default. Tests add sessions with `FakeProc` and select their tree
with `with_proc()` (#2042), including the session-running case in
`issue_1691_switch_live.rs`; they do not read the host's sessions. The
rotation-owner tests (#1803) use synthetic process tables too. Checks that
precede signalling a rotator still read the real `/proc`, so a synthetic
table cannot select a host process to signal.

Revalidation and swap completion use trusted identity; later session
rewrites cannot restore the previous account in the ledger. These checks
do not change the separate usage-selection and stale-reading policies.

A profile has been seen read as exhausted after its reset time:
[#1754](https://github.com/rysweet/amplihack-rs/issues/1754).

### Rotate decision rule

*Headroom* is `100 - max(session_used, week_used)`, rounded down to a whole
point.

| Probe result | Session used | Week used | Headroom |
|--------------|--------------|-----------|----------|
| `ok` with both windows | as reported | as reported | as computed |
| `ok` with no session window, including [no session limit](#no-session-limit) | `0` | as reported | as computed |
| `ok` with no week window | | | probe failed |
| `session_exhausted` or `weekly_exhausted` | | | `0` |
| any other [state](#states) | | | probe failed |

A profile with no session limit is ranked on its week alone: its headroom
is `100 - week_used`, and `session-at` never triggers for it. When every
other login is session-exhausted, such a profile with weekly room is the
best candidate. Once its week reaches `week-at` it is treated like any
other profile over the week threshold, and at 100% it is
`weekly_exhausted`.

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
| The live profile is in the `exclude` list, and `best >= margin` | Swap to best, `reason=threshold`. The live login counts as headroom `0`, and no damping, hold or home-key stay applies. See [Rotate excluded live profile](#rotate-excluded-live-profile). |
| The live profile is in the `exclude` list, no candidate meets the margin, and no [plan-room](#rotate-plan-room-swap) or [overflow](#overflow-decision) move is possible | ALERT `excluded-live`. No swap. |
| Live probe failed, or `session_used >= session-at`, or `week_used >= week-at`; and `best - live >= margin` | Swap to best, `reason=threshold`. A failed live probe counts as headroom `0`, and the swap is then subject to [failed-probe damping](#rotate-failed-live-probes). |
| As above, but no candidate meets the margin, or there are no candidates | ALERT `no-capacity`. No swap. |
| No threshold reached on the live reading, but its [projected session use](#rotate-live-reading-freshness) one interval ahead is at or above `session-at`; and `best - live >= margin`, with `live` taken on the projected figures and `best` the best login [organisation affinity](#outlook-and-organisation-affinity) permits | Swap to best, `reason=projected`. No candidate meets the margin: ALERT `no-capacity`, as above. Only a login in another organisation meets it: stay. |
| No threshold reached, `now - swapped_at >= hold` (or no earlier swap), and `best - live >= upgrade-margin` and `best - live > 0` | Swap to best, `reason=upgrade`. |
| Otherwise | Stay. |

`swapped_at` and `previous` (the profile swapped out) come from the
[ledger](#rotate-state-directory). During the hold no upgrade happens at
all, so `previous` cannot come straight back by upgrade. Threshold swaps on
a measured reading are never held, and nor are projected swaps.

The table ranks every candidate on the reading it has, which may be minutes
old. Before a swap stands, its target passes the
[freshness gate](#rotate-reading-freshness-and-swap-back-damping): a target
read more than 300 seconds ago, earlier than the live login, or not since the
rotator last swapped away from it while it was full, is read again (by the
[probe method](#probe-method), passive readings off) and the decision is made again on the new figures.

#### Rotate failed live probes

When the table above gives a threshold swap only because the live probe
failed, the rotator applies these rules in order. An excluded live profile
is not probed, so its swap is not a failed-probe swap and none of these
rules applies to it. `k` is the number of
consecutive cycles, including this one, whose live probe failed.

| Condition | Outcome | Log line |
|-----------|---------|----------|
| `k < 3` | Stay. | `stay: live probe failed (<k>/3)` |
| `k >= 3` and `now - swapped_at < hold` | Stay. | `stay: live probe failed; held after last swap (<N>s left)`, where `N = hold - (now - swapped_at)`, or `hold` if `swapped_at` is in the future |
| The [API-key login check](#api-key-login-check) passes on the `$HOME` files and the API key is not live | Stay. | `stay: live probe failed; home key (<detail>) applies to every login once live, a swap changes nothing`; see [API-key login check](#api-key-login-check) |
| Otherwise | Swap to best. | `swap <from>-><to> live failed target s=<n>% w=<n>% reason=failed-probe` |

`k` is kept in memory by `watch`. It is reset to `0` by a cycle whose live
probe does not fail, by a completed swap, and by restarting the rotator. The
one-shot `rotate` always uses `k = 1`, so it never swaps on a failed live
probe alone. With `--hold 0` the second rule never applies, but the first
still limits these swaps to one every three cycles.

Nothing else changes: an ALERT when no candidate meets the margin, swaps on
a measured threshold, swaps on an exhausted live login and upgrade swaps
behave as in the table above.

One exception changes only the timing of these rules. When the live login
was near `--session-at` on a last known reading whose session window can
still be open, and the swap target has at least `--margin` more headroom
than the larger of `--session-at` and that last known session use, and is
not in another organisation, the failed-probe swap is made on the first
failure, with `reason=projected`:
neither the count nor `--hold` delays it, as for any projected swap. The
home-key rule still applies, so with a key in the `$HOME` files the early
swap is a stay, as the failed-probe swap would be. Otherwise these rules
apply unchanged: the exception never changes the target and never turns the
failed-probe swap into an ALERT. See
[Failed or stale reading near the threshold](#failed-or-stale-reading-near-the-threshold).

A damped stay is recorded as `stayed-unread`, not `stayed`: the cycle did
not read the live login, so it cannot say that login has room. A
`stayed-unread` cycle is not capacity by the [rotator capacity rule](#rotator-capacity-rule),
and it leaves the stored ALERT key and the ledger's `alert` as they were.
A `no-capacity` ALERT written before the probe failed therefore stays in
place, and a recipe step in the usage-limit wait keeps waiting. The next cycle whose
live probe succeeds with the live login below 100% on both windows records
`stayed` again and clears the key; see
[Stays and capacity](#rotate-stays-and-capacity).

#### Rotate stays and capacity

A cycle that stays on the live login records one of two outcomes. Which one
depends only on the reading of the live login taken in this cycle.

The live reading *qualifies* when all of these hold:

- the probe succeeded in this cycle with measured figures: it is not failed,
  not `unknown, still running`, not a result
  [collected from a read started in an earlier cycle](#reads-that-outlast-a-cycle),
  and not unknown;
- it is not a [spend limit](#spend-status), and the live login has no
  active [spend block](#overflow-spend-limit-block): no unexpired entry
  for it in the ledger's `overflow_blocked`, and none added in this cycle;
- `session_used` is below `100` and `week_used` is below `100`, as numbers
  (a value of exactly `100.0`, or one that is not a number, does not
  qualify).

The [reading freshness](#rotate-reading-freshness-and-swap-back-damping)
gate does not apply here, as it does not apply to any stay or ALERT. A
passive reading of the live login that is measured and below `100` on both
windows qualifies, but its age still matters. A `stayed` cycle records, as
`last_cycle.measured_at`, the time the live reading was taken: its
`observed_at` for a passive reading, otherwise the time the cycle started
measuring, whichever is earlier. The usage-limit wait counts only a
`measured_at` strictly later than the limit hit, so a passive reading taken
before the limit never reruns a step that the limit stopped; the next
reading taken after the limit does. A stay never resumes a session through
the limit-resume hook, which acts only on a [swap record](#swap-record).

A live login with an active spend block cannot spend, whatever its plan
windows read. A stay on it records `stayed-unread`, and its
`no-capacity` ALERT is never [advisory](#rotate-advisory-alert): the key is
stored in `alert` and the hook does not resume.

A live login on the [platform tier](#overflow-tiers) has no usage windows.
Its reading counts as qualifying for a stay, so a platform stay records
`stayed` as before, but it never makes an ALERT
[advisory](#rotate-advisory-alert): `platform-blocked` always blocks.

A stay on an [overflow](#rotate-overflow) profile (`overflow=on`) runs on
credits, so its plan windows may read `100`. When its probe read the login
in this cycle, the stay records `stayed`, whatever the figures, and the
hook resumes as before. Like a platform stay, it never makes an ALERT
advisory. A failed, carried, unknown or spend-limited probe of the overflow
profile, or a stay on one with an active
[spend block](#overflow-spend-limit-block), records `stayed-unread`.

| Live reading | Stay outcome | Capacity | ALERT key |
|--------------|--------------|----------|-----------|
| Qualifies | `stayed` | yes | `alert` and `advisory_alert` cleared |
| Read in this cycle on an overflow profile (`overflow=on`) with no active spend block, at any figures | `stayed` | yes | `alert` and `advisory_alert` cleared |
| Qualifies, but a passive reading taken before the limit hit | `stayed`, recorded at the reading's time | not for that limit | `alert` and `advisory_alert` cleared |
| Failed, carried, unknown, still running, spend-limited, or at 100% on either window (and not a read overflow stay) | `stayed-unread` | no | unchanged |
| Live login with an active spend block, at any figures | `stayed-unread` | no | unchanged |

Every stay that does not read the live login records `stayed-unread`: a
damped stay after a failed live probe, a stay because the
[home key](#api-key-login-check) applies to every login, a stay on an
[overflow](#rotate-overflow) profile whose live probe failed, and a stay
on a carried result.

A threshold reached on a candidate never changes this. Which candidate is
best, and whether any candidate has room, does not decide whether a stay
is capacity; only the live reading does.

#### Rotate advisory alert

When every candidate has passed a threshold, or none meets the margin, the
[decision rule](#rotate-decision-rule) gives the `no-capacity` ALERT. If the
live login's reading [qualifies](#rotate-stays-and-capacity) at the same
time (below 100% on both windows, with no active spend block), the login
in `~/.claude` can still be used, so the ALERT is *advisory*:

- the `ALERT: no profile has capacity ...` line is written to the
  [log](#rotate-log) once per key, exactly as for any ALERT;
- the key is stored in the ledger's `advisory_alert` field, and `alert` is
  set to `null`;
- the cycle records `stayed`, which is capacity by the
  [rotator capacity rule](#rotator-capacity-rule), so a recipe step in the
  usage-limit wait reruns. It is a stay, so it writes no swap record and the
  limit-resume hook does not act on it.

For example, with `--session-at 90` and three logins:

```
2026-11-03T10:02:05Z ALERT: no profile has capacity live=work s=92% w=60% (candidates: 0 failed, 2 exhausted)
```

The ledger after this cycle and the next, which writes no second ALERT
line:

```json
{
  "version": 1,
  "live": "work",
  "account": "<accountUuid>",
  "since": 1793696405,
  "advisory_alert": "no-capacity:work",
  "last_cycle": { "at": 1793701445, "measured_at": 1793701325, "outcome": "stayed" }
}
```

Every other ALERT is *hard*: `alert` is set, `advisory_alert` is cleared,
and the cycle records `alerted`, which is not capacity. That covers
`no-capacity` when the live reading does not qualify (live at 100%, a spend
limit, or a failed probe), `platform-blocked`, `key-held`, the swap-refused
ALERTs and every ALERT written before measuring, such as `no-login` and
`hand-change`.

`alert` and `advisory_alert` are never both set; a hand-edited ledger that
sets both is refused as `RotateInconsistent`. The two share one
de-duplication key: when the same key moves from advisory to hard, or back,
no second ALERT line is written, but `last_cycle.outcome` changes. Every
step that clears the ALERT key (a completed swap, a stay recorded as `stayed`, a held
cycle, [hand-change recovery](#rotate-hand-change-recovery)) clears both
fields.

The one-shot `amplihack profile rotate` exits `0` after an advisory ALERT,
as after a hard one; only the ledger differs. `--dry-run` writes no ledger
and prints `would alert:` in both cases.

#### Rotate reading freshness and swap-back damping

A candidate's figures can be several minutes old: a
[passive reading](#passive-readings) of up to 300 seconds, or the result of
a read that [outlasted an earlier cycle](#reads-that-outlast-a-cycle). Near a
limit, such a reading can show room the login no longer has, and the rotator
would swap to a login that is already full and then swap away again on the
next cycle. Before a swap stands, the rotator therefore checks that the
target's reading is fresh, and reads the target again when it is not, the
way [`AMPLIHACK_PROFILE_USAGE_PROBE`](#probe-method) says.

The gate applies to swaps with `reason=threshold`, `projected`, `upgrade`,
`failed-probe`, `plan-room` and `overflow-out`, in the one-shot `rotate`,
in `rotate watch` and in `--dry-run`. It does not apply to `overflow-in`,
whose target is chosen by [tier](#overflow-tiers), not by a reading, or to
a stay or an ALERT. It checks only the target the decision chose, after
[failed-probe damping](#rotate-failed-live-probes) and the
[home-key check](#api-key-login-check), not every candidate.

##### Reading time

| Reading | Reading time |
|---------|--------------|
| [Passive](#passive-readings) | Its `observed_at`. |
| A direct or `claude` read started and finished in this cycle | Its evidence timestamp when supplied; otherwise when this cycle started measuring (`last_cycle.measured_at`). |
| A target re-read by the gate | Its evidence timestamp when supplied; otherwise when the re-read finished. |
| A live re-read | Its evidence timestamp when supplied; otherwise the cycle clock captured before the re-read. |
| A result collected from a read started in an earlier cycle | None. |
| `unknown, still running` or failed | None. |

A re-read preserves a supplied `observed_at`, including an older
timestamp. Completion or collection alone cannot turn old evidence into a
new observation. Only a re-read without an evidence timestamp receives the
fallback time in the table. Target and live re-reads retain supplied timestamps.
The replacement evidence must then
pass the applicable target or live freshness checks. A failed, unknown,
still-running or carried result has no usable reading time.

A target's reading is *fresh* when all of these hold:

- it has a reading time;
- the reading time is no more than 300 seconds before now;
- the reading time is not earlier than the live login's reading time. Equal
  times count as fresh. A live reading time later than when this cycle
  started measuring (`last_cycle.measured_at`) counts as that start time.
  This covers a [live re-read](#stale-live-reading), which runs after the
  measurement and is dated then, and a passive live reading observed during
  the measurement. So a target read directly in the same measurement is
  never older than the live login, however long the measurement took, and a
  live re-read adds no target re-read. A target reading taken before the
  measurement started is still older than a live reading taken after it.
  When the live probe failed, the live login has no
  reading time, so this comparison is omitted; the other checks still apply;
- when the target is the ledger's `previous` login, the reading time is
  strictly later than `swapped_at`, whether or not that login was exhausted;
- when the target has a retained [`swapped_away`](#swap-back-damping) entry,
  the reading time is strictly later than that entry. This applies even
  when the target is no longer `previous`.

Otherwise the reading is *stale*.

When both recorded swap times apply, the later one is used. A reading from
the current cycle still has to pass these checks; collecting an older
cached result in this cycle does not give it a new reading time. These
checks use the existing ledger and its retained damping entries, not a
complete history of every login's use.

Freshness and eligibility are separate checks. A fresh exhausted reading
cannot authorize a swap to that plan login. A fresh reading showing room
remains subject to the existing margin, hold, plan-room and overflow rules;
freshness adds no new exhaustion threshold or candidate ordering.

The 300-second limit is fixed; there is no flag or environment variable for
it. It equals the default of
[`AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS`](#passive-readings) and half
the default `--interval` of 600 seconds, so a passive reading accepted under
the default setting is never too old on age alone. Raising the passive
setting above 300 lets older passive readings rank candidates, but a target
chosen on one is still read again.

When this cycle read the live login directly and a candidate passively, the
candidate's reading is older than the live one, so it is read again if it is
chosen. A cycle that swaps on the first re-read makes one read. When that
read shows the target full, the decision chooses the next candidate, which
may also need a re-read, so a near-limit cycle can make up to 3 (see
[Re-reads](#re-reads) for the limits on their number and their total time).

##### Re-reads

A stale target is read again the way
[`AMPLIHACK_PROFILE_USAGE_PROBE`](#probe-method) says, as the cycle's own
measurement reads it, except that [passive readings](#passive-readings) are
not used: the stored reading is what made the target stale.

| `AMPLIHACK_PROFILE_USAGE_PROBE` | Re-read |
|---------------------------------|---------|
| unset | A [direct read](#direct-read); when it fails, the [`claude` fallback](#fallback-probe), with `/usage` text when the direct read failed with `usage credits on`. |
| `direct` | A direct read only. A failed one makes the target unknown. |
| `auto`, `text` | The `claude` fallback only, as named. No direct read is made, as the mode promises. |

A profile marked as [overflow](#overflow-tiers) is read with `/usage` in
every mode but `direct`, as in the measurement. So setting `text` because
direct reads fail or are unwanted keeps them off for re-reads too, and a
login whose direct read fails because usage credits are on is still read by
`/usage` under the default order rather than made unknown.

The read is bounded by what is left of the gate's time limit (below), and
at most one `claude` runs for it. It holds no rotate lock; the only lock it
may take is Claude's own `.oauth_refresh.lock`, when a direct read refreshes
a token (see the `--dry-run` paragraph below). Its figures replace the stale ones for
the rest of the cycle, and the `reread:` line gives both. The decision then
runs again from the start on the new figures, and the target it chooses
passes the same gate. A re-read showing the target still has the most room
lets the swap go ahead.

A cycle makes at most 3 target re-reads. A successful re-read is checked
against the same freshness requirements. A result still stale, including
one not strictly after the applicable swap time, becomes unknown; the old
figures are not restored. Each target is read at most once. A target
that is stale once 3 re-reads have been made is treated as unknown, with
`reread: <name> limit reached; treated as unknown`, and the decision runs
once more.

All target re-reads of one cycle share one time limit: a single
[probe deadline](#probe-deadline), 120 seconds by default. The target re-reads
run one after another, and each gets only the time the earlier targets left.
A target chosen once the limit is spent is not read: it
is treated as unknown, with
`reread: <name> time limit reached; treated as unknown`, and the decision
runs once more. The optional [live re-read](#stale-live-reading) runs before
the target gate and has a separate budget of one probe deadline; it uses
neither the target budget nor the three-target-read allowance. Re-reads can
therefore add up to two probe deadlines after measurement: one for the live
login and one shared by all targets. With `--interval 120` and the default
deadline, that is about two intervals, excluding other cycle work.

Each candidate is read again at most once and made unknown at most once, so
for `n` candidates the decision runs at most `2n` more times. The decision
rule never chooses an unknown target, so the gate settles well within that
bound. If a decision still names a stale target once the bound is reached,
the cycle stops with the error `rotation gate did not settle` and makes no
swap. `rotate watch` logs the error and decides again on the next cycle.
When `rotate stop` arrives before a re-read, the gate makes no further read,
which could take up to a probe timeout, logs `stop: signal received; cycle abandoned`
and makes no swap.

A re-read that times out or fails makes the target *unknown* for this
cycle, as a [failed probe](#rotate-decision-rule) does, with
`reread: <name> failed; treated as unknown`. It is not chosen and is not
counted or recorded as exhausted: nothing about it is written to the ledger,
the [capacity file](#capacity-file) or the
[usage history](#usage-history). Like any unknown plan login, it keeps the
cycle off [overflow](#rotate-decision-order). The decision falls to the next
candidate, which is gated in turn, or to a stay or an ALERT under the
[decision rule](#rotate-decision-rule). A target made unknown by a failed
re-read, by the limit or by the time limit counts as failed, in `F`, in the
`(candidates: <F> failed, <E> exhausted)` suffix of the
[no-capacity ALERT](#rotate-log). A failed live probe is counted once per
cycle however many times the decision runs. A successful re-read that
remains stale is also replaced with unknown evidence before deciding again;
it cannot leave the rejected figures selectable.

`--dry-run` makes the same re-reads, since they write no rotate state, and
prints their `reread:` lines and any
`damp: <name> swapped away exhausted; no later reading shows room` line. It
never prints `damp: <name> later reading shows room; swap-back allowed`,
because that line is written when the ledger is updated, and it never
changes `swapped_away` or any other part of the ledger. Dry runs use direct
reads only, never refresh tokens and never start `claude`. Outside a dry
run, a stored target's direct re-read may refresh its token under the
existing [token-refresh rules](#token-refresh). A live usage re-read never
refreshes a token.
[Usage revalidation](#rotate-revalidation) under the lock never measures
usage. A stale target there counts as unknown. The
[unchanged-state check](#rotate-revalidation) applies the target-change
gate to every usage-based swap, using the current clock and ledger. Any
replacement target must pass the same freshness, identity and eligibility
checks without an external usage re-read under the lock.

For example, live `work` reaches 100% session use and the rotator swaps
from `work` to `spare`, which reads 90%. On a later cycle the stored
reading for `work` is `s=0%,w=0%`, taken before the swap. That reading
cannot authorize a swap back to `work`: it is not strictly later than
`swapped_at`, so the gate reads `work` again first. If the re-read reports
exhaustion, `work` is not eligible. If it fails or is still stale, `work`
is unknown for this cycle and is not chosen. Only a reading taken after
the swap away can make `work` a target again, under the existing decision
rules. A different target that the decision chooses passes the same gate,
within the same limits.

For example, live `ryan-ryansweet`
is past `--week-at`, and `ggrant` has a stored reading from before it was
last live:

```
2026-11-02T13:09:09Z cycle live=ryan-ryansweet ryan-ryansweet:s=0%,w=97% ggrant:s=11%,w=95% hharkness:unknown, still running
2026-11-02T13:09:10Z reread: ggrant reading stale s=11% w=95%; fresh s=43% w=100%
2026-11-02T13:09:10Z ALERT: no profile has capacity live=ryan-ryansweet s=0% w=97% (candidates: 1 failed, 1 exhausted)
```

Without the gate this cycle would swap to `ggrant`, read it at `w=100%` on
the next cycle, and swap away again.

##### Swap-back damping

After a swap completes, the rotator records the login it left in the
ledger's `swapped_away` map, with the swap time in epoch seconds, when that
login's reading at decision time was exhausted, or showed session use at or
above `--session-at` or week use at or above `--week-at`. This applies
whatever the swap's reason. Nothing is recorded when the live probe failed,
in `--dry-run`, or when the swap was refused or skipped.

A login in `swapped_away` is *damped*: its reading counts as stale unless it
was taken strictly after the recorded time, so the gate reads it again
before any swap back to it.

- When a reading taken after the recorded time shows room (not exhausted,
  and below both `--session-at` and `--week-at`), the entry is removed with
  `damp: <name> later reading shows room; swap-back allowed`. That reading
  may come from a re-read or from an ordinary probe in a later cycle, of the
  login as a candidate or as the live login.
- When the re-read shows no room, the entry stays, the log says
  `damp: <name> swapped away exhausted; no later reading shows room`, and
  the login is not chosen on its new figures.

A damped login remains a plan login: it counts towards the
[plan pool](#plan-pool) and the rules that keep the rotator off
[overflow](#rotate-decision-order) as before.

For example, in a separate case where `ggrant` and `hharkness` are the
only plan logins, the rotator leaves `ggrant` past `--session-at`. Sessions
started while `ggrant` was live keep using its login, so its use goes on
rising:

```
2026-11-02T15:40:10Z swap ggrant->hharkness live s=86% w=70% target s=70% w=62% reason=threshold
```

On the next cycle, `ggrant`'s stored reading is the one taken at 15:40:09,
before the swap, and on those figures it beats `hharkness` by `--margin`.
That reading is not later than the `swapped_away` entry, so it is read
again:

```
2026-11-02T15:42:21Z cycle live=hharkness hharkness:s=92%,w=63% ggrant:s=86%,w=70%
2026-11-02T15:42:22Z reread: ggrant reading stale s=86% w=70%; fresh s=93% w=71%
2026-11-02T15:42:22Z damp: ggrant swapped away exhausted; no later reading shows room
2026-11-02T15:42:22Z ALERT: no profile has capacity live=hharkness s=92% w=63%
```

The ledger then holds:

```json
"swapped_away": { "ggrant": 1793634010 }
```

The entry is kept between one-shot runs and across restarts. It holds only
profile names and whole epoch seconds, never an e-mail address, account ID
or token. An entry is removed when it is lifted, or when it is more than
7 days old, the length of the weekly window. The map holds at most 16
entries; when a new one would exceed that, the oldest is dropped. On load,
an entry whose name fails the [name rules](#name-rules), or whose time is
not above `0` or is more than 300 seconds in the future, is dropped, and
the rest of the ledger is used. A bad entry is dropped rather than
reported as `RotateInconsistent` so that it cannot stop rotation.

#### Rotate live reading freshness

The [freshness gate](#rotate-reading-freshness-and-swap-back-damping) checks
the swap target. This step checks the live login. A live reading can stop
moving while sessions go on using the login: a passive figure that is no
longer refreshed, or a reading from several minutes before. A live login
that rises by about 10 points a cycle can also go from below `--session-at`
to its limit between two cycles, so a rule that waits for the threshold
swaps too late.

The step runs once per cycle, after the `cycle` line and before the
decision. It applies only when the live login is a known profile and is not
the [platform tier](#overflow-tiers). An excluded live login, one in the
auto-switch `exclude` list, is never re-read: the step checks the same
`exclude` rule as every other reader before it reads anything, and the cycle
moves off that login as described under
[Rotate excluded live profile](#rotate-excluded-live-profile). It writes nothing to the ledger, the
[capacity file](#capacity-file) or the [usage history](#usage-history), and
it holds no lock.

##### Live reading series

`rotate watch` keeps, in memory, a short series of the live login's
readings: up to 4 points, each a reading time, a session figure and the
session window's reset time (none when the reading gave no reset time). A
point is added only when its reading time is later than the last point's
and not later than now. Accepted evidence is kept separately from the
measured cache figures used to count frozen cycles:

- the *frozen count* is kept on the measured figures only, never a
  re-read's. It rises by one on each cycle in which a cached (passive)
  reading equals the previous cycle's figures, and restarts from `0` on a
  direct reading, which was read from the usage source in that cycle;
- while a live re-read has established that the cache is behind, the
  series takes only the re-read figures. A cached figure below the last
  point is neither added to the series nor treated as a new session window.

Session use only rises within one session window, so the rotator tells a
new window from a stale figure by the session reset time, never by the
observation time. The status line rewrites old figures with new
observation times, and many sessions write the same cache, so a lower
figure with a newer time is expected and proves nothing. Two reset times
at most one hour apart are the same window: a direct read prints the reset
to the minute or the hour, and the status line stores it to the second. The
next window starts no earlier than the current one resets and lasts five
hours, so its reset is hours later.

| The new reading's session reset | A lower session figure | A higher or equal one |
|---------------------------------|------------------------|-----------------------|
| More than one hour later (a new window) | Clears the series, then is added | Clears the series, then is added |
| Within one hour (the same window) | Stale: not added; the series is kept | Added |
| More than one hour earlier (an ended window) | Stale: not added | Stale: not added |
| Unknown on either side | A cached figure is [read again](#stale-live-reading) first; a direct read clears the series, then is added | Added |

The series is cleared when:

- an accepted reading is from a later session window by its reset time, or,
  with no reset time to compare, a direct read gives a lower session figure
  (a new session window). A cached figure cannot establish that reset by
  its observation time;
- a different login becomes live, or a swap completes;
- the live login cannot be identified.

The series is not read from or written to disk. Restarting the rotator
starts it empty.

##### Stale live reading

The live reading is *stale* when any of these holds:

- **Frozen.** It is a cached (passive) reading, its session and week
  figures are the same as on the previous two cycles for the same live
  login, with no swap and no direct reading in between, and its session
  figure is above `0`. The third identical cached reading in a row is
  stale. A session figure of `0` is never frozen, because a login with no
  use in its window reads `0` truthfully. A direct reading is never frozen:
  when the status line is not being written, the cache ages out and each
  cycle reads the login directly, and identical direct figures mean an
  idle login. A second direct read seconds later could only return the
  same figures, or fail and discard the reading just taken, so it is not
  made, and a direct reading is never substituted (see
  [Failed or stale reading near the threshold](#failed-or-stale-reading-near-the-threshold)).
- **Old.** Its [reading time](#reading-time) is more than one `--interval`
  before now.
- **Lagging cache.** A live re-read has established that the cache is
  behind, and the cached observation is older than the accepted direct
  evidence, or its figure has not caught up with that evidence in the same
  session window. Changing cached percentages, a newer observation time or
  resetting the frozen count does not make that figure current.
- **Contradicted cache.** A cached (passive) figure is lower than the last
  point in the same session window, or in a window whose reset time is
  unknown, or comes from an earlier window. Its observation time does not
  matter: use never falls within one window.

A stale live reading is read once more in the same way as a
[re-read](#re-reads) of a target: by the [probe method](#probe-method),
passive readings off, within its own
[probe deadline](#probe-deadline), separate from the target gate's shared
deadline. Under `auto` or `text`, this uses the
configured `claude` fallback, without a direct read. The log gets:

```
2026-11-09T14:12:05Z stale live reading; re-read
```

The direct live re-read loads credentials from the supplied home's
`.claude` directory, even when the named profile's stored credentials are
different or absent. It never reads that stored copy as a substitute and
never refreshes a token or changes credential bytes. The separate
[expired-live-login prelude](#an-expired-live-login) retains its existing
behavior. Target re-reads continue to use stored-profile credentials.

The decision is then made on the new figures, with the reading time given
by the [reading-time rules](#reading-time). The re-read is dated after the
measurement, but it does not make the cycle's own target readings stale:
the [target gate](#reading-time) treats a live reading time later than the
start of the measurement as that start time. A re-read that confirms the
measured figures restarts the frozen count from `0` and clears the lag. A
disagreeing re-read marks the cache as behind and leaves the frozen count
running, so the distinction between the cache baseline and accepted
evidence is kept. A failed re-read, or one that could not be made, changes
neither: a failure says nothing about the cache, so a frozen reading stays
frozen and is read again on the next identical cycle, as a lagging one is.

| Re-read result | Frozen count | Cache behind | Series |
|----------------|--------------|--------------|--------|
| Agrees with the cached figures | Restarts from `0` | Cleared | Takes the re-read figure (equal to the cached one) |
| Disagrees | Kept; rises on the next identical cycle | Set | Takes the re-read figure |
| Fails | Kept; rises on the next identical cycle, which is read again | Unchanged | Unchanged (no point added) |

The count saturates at its maximum value rather than wrapping, so a cache
frozen for a very long run cannot return to a count below the frozen limit.

Once lag is established, a cached observation must have a usable reading
time, no later than now and no more than one interval old, to clear it.
Its time must be at least that of the latest accepted live evidence, and
its session figure must have caught up to that evidence within the same
session window. A lower session figure can instead establish a new session
window only with a session reset time more than one hour later than the
last point's. That reset clears the old series. A newer observation time
alone is not a reset: a lower figure stamped later in the same window, or
in a window whose reset time is unknown, keeps the lag and the series, and
is read again. An older observation cannot clear lag, overwrite accepted
evidence or establish a reset, even when its percentages change or equal
the accepted figures.

For example, the cache reads 63%, 63%, 63%, 64%, 65%, each newly stamped,
in the same window as direct re-reads of 66%, 69% and 72%. The third 63% is
frozen and is read again at 66%. The 64% is lower than 66% in the same
window, so it is read again at 69%, and the series keeps 63, 63, 66 and 69.
Over those points the rate projects 71% one interval ahead, so that cycle
swaps with `reason=projected`.

A cycle makes at most one live re-read, separately from the target gate's
three-read limit. None is made once `rotate stop` has arrived, or when the
reading is current and neither frozen, lagging nor contradicted. A reading
time later than now counts as old.

When the re-read fails, or its reading is itself more than one interval
old or later than now, the log gets
`reread: <name> failed; treated as unknown` after the `stale live reading`
line, and the rules in
[Failed or stale reading near the threshold](#failed-or-stale-reading-near-the-threshold)
apply.

Failure does not make a known lagging cache, or a frozen one, trustworthy
again. The accepted history remains available for the near-threshold rule,
and the next cycle retries while the cache remains behind or the reading
stays frozen: at most one live re-read per cycle, each within its own probe
deadline. A successful later re-read replaces the failed attempt with
current evidence.

Under the recovery rule, a cache can remain at 63% while successive
live re-reads return 65%, 68% and 71%. Each successful re-read supplies that cycle's live
evidence while the login remains live. The cached 63% cannot replace 65%
or 68%, including after a failed re-read; an older cache moving to 64%
does not prove recovery either. Projection can trigger a swap before 71%;
otherwise 71% reaches `--session-at 70`. A login change clears this series
and lag tracking, so the new login inherits neither.

Cycle by cycle, with `--session-at 70` and the cache frozen at 63%:

| Cycle | Cached session | Frozen count | Re-read | Cache behind | Decision input |
|-------|----------------|--------------|---------|--------------|----------------|
| 1 | 63% | 0 | none | no | 63% |
| 2 | 63% | 1 | none | no | 63% |
| 3 | 63% | 2 (frozen) | 65% | yes | 65% |
| 4 | 63% | 3 | 68% | yes | 68% |
| 5 | 63% | 4 | 71% | yes | 71%: swap, `reason=threshold` |

On cycles 3 to 5 the series never shortens, the decision never uses the
cached 63%, and by cycle 4 the rate is positive. Each of those cycles
writes one `stale live reading; re-read` line, three in all.

##### Projected swap

When the first and last points of the series are at least one
`--interval` apart, the rotator works out the live login's session rate, in
points per second, from those two points:

```
rate      = (last session - first session) / (last time - first time)
projected = session + rate * interval
```

Two points closer together give no rate. A direct read and a status line
written a few seconds after it say little about the rate: one point over
five seconds would project 24 points over a 120-second interval. Over at
least one interval, `rate * interval` is never more than the rise the series
has seen, so the projection adds no more than that.

`interval` is `--interval`. The projection is limited to the range `0` to
`100`. When `projected >= --session-at` and the live reading itself is below
`--session-at`, the decision runs again on the projected session figure and
the live week figure, under [organisation affinity](#outlook-and-organisation-affinity),
and a threshold swap it gives is logged with `reason=projected`. It uses the
same `--margin` as a threshold swap, passes the same freshness gate and
[revalidation](#rotate-revalidation), and is not stopped by `--hold`. When
that decision finds nowhere to go, the cycle writes the `no-capacity` ALERT,
so an ALERT can follow a live reading still below `--session-at`.

A projection below `--session-at`, a rate of `0` or less, fewer than two
points, or points less than one interval apart leaves the decision as it
would be without this step. Only session use is projected; week use is
decided on the reading as before.

The projected figure can add only a threshold swap onto a login with
capacity, or the ALERT. The [plan-room swap](#rotate-plan-room-swap) and the
[overflow](#rotate-overflow) moves always run on the reading as measured, so
a projection never moves work onto a plan login without capacity, onto
credits or onto the API key. Where those rules would make such a move on
the projected figure, no login has capacity on it, so the cycle writes the
ALERT. The one stay is organisation affinity's: when only a login in
another organisation has capacity, the projection adds nothing (see
[Outlook and organisation affinity](#outlook-and-organisation-affinity)).
When the measured reading is itself at `--session-at` or `--week-at`, or
gives a swap on its own, the cycle is decided on it as measured, as without
this step.

The projection affects only the decision. The ledger, the `cycle` line, the
`swap` line's `live s=<n>% w=<n>%` figures and the usage history all show
the reading as measured.

##### Failed or stale reading near the threshold

When the live probe fails, or the live re-read fails, is refused or is
itself still more than one interval old, the rotator looks at the last
point in the series for the same live login. That point counts only while
the session window it was read in can still be open: before its session
reset time when it has one, and less than five hours (one session window)
after it was read in any case. After that its figure says nothing about
the login's use now, and the rule does not apply. The limit is not the
300-second [reading age](#reading-time): the default `--interval` is 600
seconds, so the previous cycle's point would always be too old. The login
is *near the threshold* when either holds:

- `last session >= --session-at - --margin`;
- with a rate: `last session + rate * interval >= --session-at`.

A rate only widens the margin band; it never narrows it. When the reading
is frozen, the series holds the frozen points: identical figures whose
observation time keeps moving, as from a status line that keeps rewriting an
old figure, give a rate of `0`, or dilute an earlier rise. Such a rate says
nothing about the login, so a last figure inside the band is near whatever
the rate, and the outcome does not depend on whether the cache's timestamp
moves.

When it is near, the *substituted figure* is the larger of `--session-at`
and the last known session use: the last point, and the stale reading
itself when there is one. The last known week figure is used with it. The
substituted figure is never lower than the last known figure.

**A stale reading** (the re-read failed, was refused or is still old) is
decided on the substituted figure. The outcome is a swap, logged with
`reason=projected`, or the `no-capacity` ALERT. It is never a stay, except
where only a login in another organisation has capacity (see
[Outlook and organisation affinity](#outlook-and-organisation-affinity)). Without
this rule the stale reading, below `--session-at`, would be a stay, so the
rule can only make a swap more likely: an ALERT is a stay with a warning.
As with a projection, the [plan-room swap](#rotate-plan-room-swap) and the
[overflow](#rotate-overflow) moves never run on the substituted figure.
When the stale reading is itself at `--session-at` or `--week-at`, the
cycle is decided on it as measured, as without this rule, so a threshold
swap on it is logged with `reason=threshold`.

**A failed probe** changes only the timing of a swap, never its target. The
[failed-probe rules](#rotate-failed-live-probes) count a failed probe as
headroom `0`, so the candidate with the most headroom that
[organisation affinity](#outlook-and-organisation-affinity) permits, when it
has `--margin` of headroom, is their target, and damping delays that swap
for three failures. The rule then asks one thing of that same target:
whether it has `--margin` more headroom than the substituted figure leaves
the live login. Another candidate's headroom never counts, since a login
that beats the substituted figure says nothing of the target:

| The failed-probe rules' target | Outcome |
|--------------------------------|---------|
| At least `--margin` more headroom than the substituted figure leaves, in the live login's organisation or one not known | The failed-probe rules' swap, made now with `reason=projected`: neither damping nor `--hold` delays it. With a [home key](#api-key-login-check) it is a stay, as the failed-probe swap would be |
| Less than `--margin` more headroom than that, whatever other candidates have | The failed-probe rules as without this rule: a damped stay, then the `failed-probe` swap after three failures |
| In another organisation (a confirmed crossing) | The same: a substituted figure never makes a crossing sooner |
| None: no candidate even against headroom `0` | Their `no-capacity` ALERT |

So for a failed probe the rule makes the same swap or makes it sooner. It
never turns the failed-probe swap into an ALERT, which would keep a dead
live login live for as long as its probe fails. An early swap needs a
reading of the live login near the threshold since the last swap, because
a swap clears the series, so it cannot repeat while the probe keeps failing.

When the login is not near, its last point is too old, or the series is
empty, the failed-probe rules apply as before. The series is cleared on a
swap, so a failed probe just after a swap is damped as usual.

For example, with `--session-at 70 --margin 5`, a status line keeps
rewriting `s=66%` with a new observation time on every cycle. The third
identical reading is frozen and is read again. When that read fails, the
cycle is decided on 70%: a swap to a candidate with at least `--margin` more
headroom, or the ALERT. The failure keeps the frozen count, so the fourth
identical reading is read again rather than trusted. The same holds when
the cached observation time does not move, where the reading is also old.

With the same settings, live `work` read 66% on the last cycle and its
probe now fails, and `spare` reads 68%. `spare` has 32 points of headroom,
only 2 more than the substituted 70% leaves `work`, so it does not beat the
substituted figure. The failed-probe rules apply as without this rule: two
damped stays, then, on the third failure:

```
2026-11-09T14:16:05Z swap work->spare live failed target s=68% w=40% reason=failed-probe
```

With `spare` at 60% instead (40 points, 10 more than 70% leaves), the first
failure swaps with `reason=projected`. With `spare` at 68% and another
stored login, `third`, at 0%, the outcome is the 68% one: `third` beats 70%
easily, but the failed-probe rules chose `spare`, and only `spare` is
tested against the substituted figure.

##### Outlook and organisation affinity

A projected or substituted figure is decided under
[organisation affinity](profile-organisation-affinity.md), with the cycle's
one need (`Need::of`), worked out on the live reading as measured, never on
the projection. So a projected or substituted swap never makes a confirmed
crossing, nor makes one sooner. A crossing waits for the reading as
measured: a measured threshold, which needs capacity relief, or the
`failed-probe` swap after three failures.

- **A measured reading below both thresholds.** The live login has
  capacity, so the need is an upgrade's, and a crossing is never a target.
  A login in the live login's organisation with `--margin` more headroom
  than the projection leaves is taken first, then one whose organisation is
  not known. When only a login in another organisation could take the
  session, the projection adds nothing: no swap, and no `no-capacity` ALERT,
  since there is capacity, across. The reading as measured makes that
  crossing once it reaches `--session-at`, with the crossing warning.
- **A failed probe.** The failed-probe rules choose the target under
  capacity relief, as without this rule, so after three failures they may
  cross. The substituted figure only makes their swap sooner, and never
  when its target is in another organisation.

For example, with `--interval 120 --session-at 70 --margin 5`, live `work`
(organisation A) reads 63% and is projected to 76%. `spare` (A) reads 40%
and `third` (B) reads 0%. The projected swap goes to `spare`, although
`third` has more headroom. With `spare` exhausted, the cycle stays at 63%
with no ALERT, and the next cycle, measured at 76%, swaps to `third`:

```
2026-11-09T14:12:06Z swap work->third live s=76% w=41% target s=0% w=0% reason=threshold
2026-11-09T14:12:06Z affinity: crossing work->third: Cross-organisation swap: earlier thinking becomes unavailable on the next turn. The visible conversation remains and the session can continue.
```

##### Example: a login rising 10 points a cycle

With `--interval 120 --session-at 70 --margin 5`, live `work` reads one
point every two minutes:

| Cycle | `work` session | Points used | Projected | Outcome |
|-------|----------------|-------------|-----------|---------|
| 1 | 22% | 22 | none | Stay |
| 2 | 32% | 22, 32 | 42.0 | Stay |
| 3 | 42% | 22, 32, 42 | 52.0 | Stay |
| 4 | 50% | 22, 32, 42, 50 | 59.3 | Stay |
| 5 | 63% | 32, 42, 50, 63 | 73.3 | Swap, `reason=projected` |

```
2026-11-09T14:10:05Z cycle live=work work:s=63%,w=41% spare:s=0%,w=30%
2026-11-09T14:10:06Z swap work->spare live s=63% w=41% target s=0% w=30% reason=projected
```

Without the projection the rotator would stay at 63% and find `work` at its
limit on the next cycle. With no candidate that meets the margin, cycle 5
writes `ALERT: no profile has capacity live=work s=63% w=41%` instead. If
`work` then reads `s=63%,w=41%` on two more cycles, the third identical
reading is re-read with `stale live reading; re-read`.

A login rising slowly, from 20% to 22% in one interval, projects to 24% and
stays.

##### One-shot and dry run

The one-shot `amplihack profile rotate` and `--dry-run` run the step, but
start with an empty series and run one cycle, so neither ever sees a frozen
reading or makes a projection. Only the old-reading rule applies. Neither
takes `--interval`, so the limit is `AMPLIHACK_ROTATE_INTERVAL` when it is
set, and 600 seconds otherwise.
`--dry-run` makes the live re-read as it makes target re-reads: direct only,
without token refresh, `claude` fallback or rotate-state writes.

The one-shot command, `--dry-run` and `rotate watch` (including a watch
started with `start`) build their cycle with the same two re-read paths:
the live re-read, which reads the home's `.claude` directory with token
refresh off, and the target re-read described in [Re-reads](#re-reads).
Both build their reader through the cycle's own measurement settings, so a
dry run stays read-only and direct-only after the re-read paths are added,
and no mode has a second way to read usage. These
freshness and projection rules need no additional configuration or public
API; they use the existing interval, thresholds, margin and probe settings.

#### Rotate plan-room swap

The table above swaps on a threshold only when a candidate beats the live
login by `--margin`. When every plan login is near its limits, none may, and
the cycle would end in `ALERT no-capacity` although some plan logins still
have room below 100%. The plan-room swap moves `~/.claude` to the plan login
with the most room, because a plan login costs nothing while it has room.

A plan login *has room* when its probe this cycle gave `ok` and both its
session and week use are below 100%. A failed probe, a login that was not
measured, an `exhausted` reading and every API-key profile have no room.

While the live login is a plan profile, the rotator considers a plan-room
swap only when both hold:

- the live login has reached `--session-at` or `--week-at` on a measured
  reading (a failed live probe is left to the
  [failed-probe rules](#rotate-failed-live-probes), whose swap the
  [near-threshold rule](#failed-or-stale-reading-near-the-threshold) can
  only make sooner;
  a [projected](#projected-swap) or substituted figure never counts);
- the table above gives `ALERT no-capacity`.

It never fires while the live login is below both thresholds, so it adds no
upgrade swaps. `--margin`, `--hold` and `--upgrade-margin` do not apply.
While the live login is an [overflow profile](#overflow-decision), the same
ranking chooses the plan login to move back to.

Plan logins with room are ranked by these keys, in order:

1. a login whose session use is below `--session-at` comes before one whose
   session use is not;
2. the most weekly room: the lowest week use;
3. the most session room: the lowest session use;
4. the profile name, ascending by byte value.

The first key keeps a login whose session window is nearly spent from
winning on weekly room alone. Ranking by week next keeps the login with the
most weekly room, which lasts longest.

The live login is ranked by the same keys. A live login that is exhausted,
or whose probe failed, ranks last. The rotator swaps only when the best
candidate ranks strictly ahead of the live login; otherwise the cycle ends in
`ALERT no-capacity` as before, and the ALERT is written once, not every
cycle. This stops two logins from swapping back and forth: live `ddrumpf` at
`s=0%,w=92%` stays put rather than moving to `ccapstone` at `s=11%,w=92%`,
and `ccapstone` would not move back either.

The swap is logged with `reason=plan-room`.

The chosen plan login passes the
[freshness gate](#rotate-reading-freshness-and-swap-back-damping) like any
other target. A login whose stored reading shows room but whose re-read
shows it full is not chosen; the ranking runs again on the new figures.
For example, a threshold decision that chooses `hharkness` on a stored
reading of `s=74%` reads it again before the swap:

```
2026-11-02T19:52:37Z reread: hharkness reading stale s=74% w=73%; fresh s=100% w=73%
```

`hharkness` is then session-exhausted and is not chosen.

**Example.** With the default `--session-at 85`, this cycle has a live
login past its session threshold and no candidate that beats it by
`--margin 5` (the best headroom is 8 against the live login's 13):

```
2026-10-30T19:42:56Z cycle live=iivanovich iivanovich:s=87%,w=61% aanderson:s=25%,w=95% bbuck:s=50%,w=97% ccapstone:s=11%,w=92% ddrumpf:s=0%,w=92% eelsworth:s=98%,w=17% ggrant:s=100%,w=100% ryan-mistt-personal:s=0%,w=96%
2026-10-30T19:42:56Z swap iivanovich->ddrumpf live s=87% w=61% target s=0% w=92% reason=plan-room
```

`aanderson`, `bbuck`, `ccapstone`, `ddrumpf` and `ryan-mistt-personal` have
session use below 85%, so they rank first. `eelsworth` has the lowest week
use but ranks after them because its session (98%) is at `--session-at`;
the live `iivanovich` (87%) ranks after them for the same reason; `ggrant`
has no room. `ccapstone` and `ddrumpf` tie on week at 92%, and `ddrumpf`
wins on session. A marked credit or API-key profile is never chosen here,
and is not used while any of these logins has room.

**Unmeasured plan logins.** A plan login skipped as `session running`,
`leased`, `hosting`, `parked`, `no account` or `directory refused`, or whose
probe failed, has no reading, so it cannot be chosen for room. It is unknown
rather than empty.
The per-token rules below name what each does with such a login. Excluded,
login-less and `not oauth` profiles do not count, nor do overflow or API-key
profiles.

**Example with no session room.** With every plan login at or above
`--session-at`, the first key ties and weekly room decides. Live `aanderson`
at `s=90%,w=80%` and `bbuck` at `s=95%,w=60%`:

```
2026-10-31T08:10:05Z swap aanderson->bbuck live s=90% w=80% target s=95% w=60% reason=plan-room
```

On the next cycle, with the same readings, `bbuck` is live and ranks ahead
of `aanderson`, so there is no swap back; the cycle writes
`ALERT no-capacity` once.

#### Rotate decision order

While the live login is a plan profile, one cycle decides in this order. The
first step that gives a swap ends the decision:

1. The [decision rule](#rotate-decision-rule) above: a threshold, projected,
   upgrade or failed-probe swap onto a plan candidate, or a stay. The live
   reading used here is the one the
   [live freshness step](#rotate-live-reading-freshness) left: a fresh
   re-read where one was made, and the projected session use where a
   projection applies. A swap onto a plan
   login with room, or a stay, stands.
2. When step 1 gives `ALERT no-capacity`, or a threshold swap onto a plan
   login without capacity: the [plan-room swap](#rotate-plan-room-swap), which
   ranks the live login with the others. When the live login ranks first, it
   is `ALERT no-capacity`.
3. Only when no measured plan login, the live one included, has room, and the
   [overflow gate](#earliest-plan-reset) allows it: the first usable marked
   per-token profile in tier order. That is a credit
   [overflow profile](#overflow-decision) in tiers 1 to 9, then the marked
   [API-key profile](#moving-onto-the-api-key) at tier 10, which must also
   pass its [login check](#api-key-login-check). `reason=overflow-in`.
   Credit tiers do not wait for a skipped plan login or one whose probe read
   a state such as `not_logged_in`: they have a spend limit, and the
   `per-token` line gives the count as `N not measured`. Neither tier is used
   while a plan candidate's probe is [unknown](#rotate-decision-rule) or a
   plan profile is skipped as `directory refused`. The API key waits for
   every unmeasured plan login.
4. Otherwise `ALERT no-capacity`, or, when only unmeasured plan logins kept
   the API key out, `ALERT: API key not used live=<name>: no measured plan
   login has room, but these were not measured and may have: <names>`.

While the live login is a per-token profile (credit or API key), the order
is:

1. a plan login with [capacity](#plan-pool), `reason=overflow-out`;
2. a plan login with room, chosen by the plan-room ranking,
   `reason=overflow-out`;
3. a usable marked per-token profile in tier order, under the same gate,
   when either of these holds:
   - the live profile is the API key and no plan login has room: the first
     usable credit profile, `reason=overflow-out`, because credits cost
     less than the API key;
   - the live profile is blocked by a spend limit or an
     [API-key block](#api-key-block): the next usable one after it;
4. otherwise a stay, or the ALERTs described under
   [Overflow decision](#overflow-decision) and
   [Moving off the API key](#moving-off-the-api-key).

Credit profiles are billed too, so they wait for the same condition as the
API key: no plan login below 100% on both windows. A credit tier is never
used ahead of a plan login with room, whatever its tier.

In both orders, a step that chooses a target by its reading (`threshold`,
`projected`, `upgrade`, `failed-probe`, `plan-room` or `overflow-out`) ends the decision
only once that target passes the
[freshness gate](#rotate-reading-freshness-and-swap-back-damping). When the
gate reads it again, the whole order runs again from step 1 on the new
figures. An `overflow-in` target is chosen by tier, not by a reading, and is
not gated.

### Rotate overflow

An *overflow profile* is one whose account has usage credits turned on at
<https://claude.ai/settings/usage>, so it keeps working after its plan's
session or weekly limit and the extra use is billed to the credits. A
*plan profile* is any other profile. Profiles are marked with
[`profile overflow add`](#overflow-commands); with no profile marked, every
rule in this section is inactive and the rotator behaves exactly as described
above, [plan-room swap](#rotate-plan-room-swap) included. The full order of a
cycle is in [Rotate decision order](#rotate-decision-order).

Credits cost money and plan capacity does not, so the rotator:

- never moves onto an overflow profile while any measured plan login, the
  live one included, has [room](#rotate-plan-room-swap) below 100% on both windows;
- moves onto one only when the live plan login has reached a threshold, no
  measured plan login has room, no plan candidate's probe in this cycle is
  [unknown](#rotate-decision-rule) (it timed out, or `claude` could not be
  started, read or waited for), and the next plan reset is **known** and more
  than `--overflow-wait` seconds away (an unknown reset spends money only with
  `--overflow-unknown-reset 1`);
- moves back to a plan profile in the first cycle in which one has capacity
  or room.

Reaching `--session-at` or `--week-at` on every plan login is not enough on
its own: while a plan login is below 100% on both windows, the
[plan-room swap](#rotate-plan-room-swap) uses it first.

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
for reaching a threshold. A plan login without capacity can still have
[room](#rotate-plan-room-swap): with `--session-at 80`, a plan login at
`s=85%,w=40%` has no capacity but has room, so the rotator moves off credits
onto it. It does not move back onto credits from there while that login, or
any other, still has room.

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
not run, or a plan profile skipped as `directory refused`, blocks the move
anyway (see [Overflow decision](#overflow-decision)), but one that read
`not_logged_in`, `unparsed` or `spend_limit` does not. With
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
A threshold or upgrade swap onto a plan login with room always stands. Only
when that gives `ALERT no-capacity` on a measured live reading does the
[plan-room swap](#rotate-plan-room-swap) run, and only when no measured plan
login, the live one included, has room does it consider overflow:

| Condition | Outcome |
|-----------|---------|
| Live probe failed | As without overflow. A failed live probe never moves work onto credits. |
| A plan login, the live one included, has room below 100% on both windows | No overflow. The [plan-room swap](#rotate-plan-room-swap) moves to the best-ranked plan login, or the cycle stays with `ALERT no-capacity` when the live login ranks first. Neither credits nor the API key are used while a plan login has room. |
| A plan candidate's probe in this cycle ended in `timeout` or `error` (`claude` could not be started, written to, read or waited for, or the profile directory was refused just before the spawn) | As without overflow: the ALERT, or the threshold swap onto the plan candidate without capacity. That login might have capacity, so such a probe never moves work onto credits or the API key. It is probed again on the next cycle. The [plan-room swap](#rotate-plan-room-swap) is not held: it costs nothing. |
| A plan profile is skipped as [`directory refused`](#rotate-candidates) | As without overflow. Its login cannot be checked and might have capacity. The skip, and so the hold, comes back on every cycle until the directory is fixed. |
| An [unmeasured plan login](#rotate-plan-room-swap), and the only usable overflow profile is the API key | No move onto the key. `ALERT: API key not used` names the logins. |
| No plan login has room, `earliest plan reset - now > overflow-wait`, and an overflow candidate is eligible | Swap to the first eligible one by tier, `reason=overflow-in`: credit tiers 1 to 9, then the marked API-key profile at tier 10 if its [login check](#api-key-login-check) passes. The `--hold` does not apply. |
| `earliest plan reset - now <= overflow-wait` | As without overflow: the ALERT, or the threshold swap onto the plan candidate without capacity, which costs nothing. |
| Earliest plan reset unknown | As without overflow. With `--overflow-unknown-reset 1`, as for a reset beyond the wait. |
| No overflow candidate is eligible | As without overflow: `ALERT no-capacity`. |

For example, with the live login at `s=95%,w=60%` and a plan candidate at
`s=87%,w=50%`, both have room, so the rotator makes the free swap onto the
candidate whatever the plan reset, and no credits are spent. Only once every
plan login reads 100% on one of its windows, and the reset is beyond the
wait, does it move onto credits. Auto-switch makes the same choice on the
same readings.

The comparison is strict: a reset exactly `--overflow-wait` seconds away
stays. With `--overflow-wait 0` the rotator moves onto overflow as soon as
no plan login has room and the earliest reset is known and in
the future.

A plan candidate that reads `not_logged_in`, `unparsed` or `spend_limit`,
or `ok` with no week figure, does not block the move, so a login that has
expired cannot keep the rotator off credits for good. A plan profile whose
directory fails the ownership and mode check is different: its login cannot
be checked at all. The [candidate list](#rotate-candidates) skips it as
`directory refused` before any probe, so it is never chosen, and the skip
blocks the move on every cycle until the directory is fixed. A directory
that changes between that check and the spawn reads `error` (`profile
directory refused`) in the cycle and blocks the move in the same way.
`amplihack profile usage <name>` shows the `error` row whenever the
directory is refused. An overflow profile skipped as `directory refused`
blocks nothing, and an excluded profile is skipped as `excluded` whatever
its directory.

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
| A plan candidate has room | Swap to the plan login the [plan-room ranking](#rotate-plan-room-swap) puts first, `reason=overflow-out`. |
| The live reading is `spend_limit` | [Block](#overflow-spend-limit-block) the live profile, then swap to the next eligible overflow candidate if the wait rule above still holds, else to the plan candidate with the most headroom that is not exhausted and whose probe did not fail, else ALERT `no-capacity`. |
| Live probe failed | [Failed-probe rules](#rotate-failed-live-probes), with plan candidates as the only targets; otherwise stay on credits. |
| Otherwise | Stay on credits. |

`--session-at` and `--week-at` are never applied to a live overflow profile:
reaching its plan limit is expected. An overflow profile is never swapped for
another overflow profile except after a spend limit, and never for a lower
tier that becomes usable again.

The rules do not flap. `overflow-out` moves only to a plan login with room,
and the rotator moves back onto credits only once no plan login, that one
included, has room and every rule above holds again.

If an overflow profile's login is live in `~/.claude` while a plan profile
has capacity or room, for example after you put it there by hand, the next cycle
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

The [default pointer](#clearing-the-default) normally selects its profile
directly, outside the rotator's overflow policy, whether it is a plan profile
or an overflow profile. If the default directory hosts another login under
an active lease, the [lease fallback](#default-profile-lease-fallback) can
select an eligible alternative. That fallback retains the existing overflow
gates and billing safeguards; a lease conflict alone does not authorise paid
overflow. Leave the pointer unset (`amplihack profile switch --none`) when
using overflow to avoid selecting a credit-spending default directly.

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
plan login is never probed. A pinned profile in the auto-switch `exclude`
list is never probed either: each held cycle logs
`skip: pinned profile <name> is excluded; spend not probed` and the pin
holds as before. `profile switch` refuses an overflow profile that
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
   prevents a repeat, so the ALERT does not stop the usage-limit wait from
   seeing [capacity](#rotator-capacity-rule) when the cycle moves somewhere
   else.
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
login has room, so neither `overflow remove` nor `rotate stop` stops
spending on its own. To stop spending at once, turn off usage credits or
lower the spend limit at <https://claude.ai/settings/usage>. Claude then
refuses work on that login, the next probe reads `spend_limit`, and the
rotator [blocks](#overflow-spend-limit-block) the profile and moves away from
it as described above. Ending the Claude sessions that run on the login also
stops spending, but a new launch uses it again.

### Rotation login identity

Automatic rotation and manual `profile switch` share credential-bound
ownership checks. The checks cover live identification, candidate
identity, duplicate claims, adoption, measurement snapshots, locked
revalidation, swap completion and interrupted-swap recovery.

| Evidence | Role |
|----------|------|
| OAuth account returned by the existing bounded credential identity reader | Identifies the login used for that reading, including its organization. |
| Positively identified platform login | Uses the existing API-key rules; an unreadable OAuth login does not become a platform login. |
| Expected-account record | States which account a profile is meant to own. See [Recorded account](#recorded-account). |
| Rotate ledger and swap journal | Track placement and transaction state, with sanitized full-account provenance from authorized trusted readings. A legacy account key or an inode alone cannot prove ownership. |
| `.claude.json` and its `oauthAccount` | Display state. Cannot establish ownership, authorize movement, backfill a record or choose the ledger account. |

Trusted readings are bound to validated non-secret metadata for the login
file and its directory. The reader rejects symlinks and unsafe files,
and validates the binding before and after the read. It does not hash
credential contents or use access times. Replacement or refresh
invalidates an earlier binding. Continuation requires fresh evidence
that identifies the current login and satisfies the same full-account
comparison; a legitimate token refresh alone is not an account change.

Account comparisons preserve [account identity](#account-identity),
including organization, and the existing legacy expected-record rules.
Comparing the account key alone is insufficient. Missing fields make
evidence insufficient only when the applicable ownership check needs
them; supported legacy email identities do not acquire a UUID requirement.

Ownership scans distinguish claimed, unclaimed and uncertain logins. No request is
sent for an [excluded](#rotate-excluded-live-profile) folder during a
scan: with a record, the record alone makes it claimed; without one, it is
unclaimed. The duplicate-login check still uses its local `oauthAccount`,
read from disk.
Excluded folders are never swap sources or targets, so an unclaimed one
authorises nothing.
An unclaimed result requires a complete scan of the relevant logins and
records. A failed duplicate read, unusable record, changed dependency or
exhausted budget cannot prove absence. Missing records may follow the
existing adoption/backfill rules only with trusted evidence. Existing
unusable records block the affected authorization.

Immediately before exchange, both logins must still identify the full
accounts in the accepted measurement snapshot. Logins and material ownership
dependencies are validated under the state and refresh locks. Required
missing, insufficient, stale or contradictory evidence causes an
actionable refusal on each manual attempt and an ALERT on each affected
rotation cycle, with no login movement or ledger relabeling. Reasons are redacted and name validated
profiles only. Unrelated alert deduplication is unchanged.

Refresh-lock ownership is journalled in the `checking` preflight phase
before final identity requests. Recovery of that phase releases only the
recorded locks and moves no login. Before exchange, the journal retains
the authorized incoming and outgoing full identities. Recovery compares
fresh credential evidence with them, including after S4 replaced the
ledger. A different unrecorded incoming account cannot replace the
authorized one. Legacy journals without sufficient expected-account
evidence refuse conservatively.

The ledger's optional `login_identity` is written only by authorized swap
completion or confirmed credential-based recovery. A block that fails the
load-time rule in [the ledger fields](#account-identity) stops the rotator;
it is never repaired silently. Missing-record
backfill and [cycle seeding](#seeding-a-record) require that full provenance;
backfill also accepts the full outgoing authorization
in a pending journal. A UUID-only legacy ledger cannot establish an
organization. A normal observation does not upgrade it into ownership proof.
Only a definite filesystem `NotFound` proves that another login is absent;
permission and other metadata errors make scans and revalidation uncertain.

The reader reuses the existing OAuth profile endpoint, endpoint validation,
response limits, redirect prohibition and five-second timeout. Reads share
one bounded identity-work budget, excluding time spent measuring usage;
identity checks do not refresh
tokens, start Claude, use usage/reset-time guesses or add another service.
There are no new flags, configuration files or identity databases. The
existing `AMPLIHACK_PROFILE_USAGE_API_BASE` endpoint setting and its safety
checks apply. Usage measurement keeps its existing refresh behavior.

A successful swap writes `ledger.account` from the validated incoming
account and repairs display fields from trusted identity while preserving
unrelated JSON keys and file modes. A later session rewrite cannot change
ownership or ledger identity, during or after recovery grace.

These rules apply to the rotation/switch paths listed above. Standalone
`profile login`, general display warnings and other auto-switch callers
retain their separately documented behavior.

#### Login directory rule

An identity read and a [token refresh](#token-refresh) use a login only when
its directory is a real directory owned by you that no other user can write.
The one rule governs every identity read, through `stamp()` in
`rotate/login_identity.rs` (the rotate cycle and its swap, recovery and
revalidation, the dry run, `profile seed-record` and `profile usage`), and
every refresh made by a direct usage read, through `dir_private()` in
`usage_direct.rs` (rotate measurement and `profile usage`). Issue #1962 widened it for all of
these callers, not only for seeding: before, any group write bit was refused.
This is a change to which credential directories are trusted. The reason is
[the cause on candide](#cause-on-candide-issue-1962).

| Directory | Result |
|-----------|--------|
| No group or other write bit, such as mode `700` or `755` | Accepted |
| World-writable | Refused |
| Group-writable, in your own private group (your primary group, named like your user, with no listed members), with no POSIX access ACL. This is the mode `775` that a umask of `002` leaves | Accepted |
| Group-writable, in a shared group, or the group lookup failed | Refused |
| Group-writable with an access ACL, or the ACL cannot be checked | Refused |

The ACL is checked only when the group write bit is set: an ACL mask that lets
anyone write always sets that bit. A filesystem with no ACL support counts as
having no ACL.

A refused directory gives an identity read the cause
[`directory not private`](#identity-read-causes), and a refresh the reason
`refresh refused`. Changing the directory's group or mode after a reading
invalidates that reading.

The private-group check sees only a group's listed members. It does not notice
another user whose primary group is the same group. If your system has one, run
`chmod 700 ~/.claude`.

This rule applies to the directory only. A login file stays refused when group
or others can write it, whatever its group. The
[swap's](#rotate-swap) `~/.claude` check and the
[shared session](#shared-session-directories) checks do not read ACLs.

#### Cause on candide (issue #1962)

1.2.99 refused every rotate cycle on candide with `~/.claude could not be
read` and the identity alert. The cause was the directory mode, not an
expired token, a 401 or a 429.

- On candide the umask is `002`. `~/.claude` is mode `775`, owned by
  `ryan:ryan`, where `ryan` is the user's own private group (gid 1000, no
  listed members) and the directory has no access ACL.
- 1.2.99 refused any login directory with `mode & 0o022 != 0`. The identity
  read stopped in `stamp()` before any request was sent. A live refresh would
  have been refused by `dir_private()` in the same way.
- That matches the candide log: the refusal and the alert, with no refresh,
  401, 429 or expiry line. With an expired token, 1.2.99 also logs
  `refresh: expired login in ~/.claude not refreshed (refresh refused)`.
  Candide's log has no such line, so the token was still valid and the
  directory check alone explains the refusal. `profile verify` and 1.2.97 do
  not apply this check, which is why they read the same login.

The reproduction is `crates/amplihack-cli/tests/issue_1962_candide_cause.rs`.
It uses fake logins and a loopback endpoint, with `~/.claude` set to candide's
mode. Run against the 1.2.99 release binary (`AMPLIHACK_TEST_BINARY`), the two
`775` tests fail with candide's lines, and the fake endpoint sees requests for
the other profiles only, none for the live login. The `700` control passes.
Against this build all three pass.

The fix is the private-group case of the [rule above](#login-directory-rule).
`chmod 700 ~/.claude` also clears the refusal on 1.2.99.

The acceptance dry run on candide (#1962 item 5) is tracked in issue #1988.
Candide stays on 1.2.97 until that run names the live login with no identity
alert.

### Rotate live profile

The live profile is the one whose login is in `~/.claude`. The ledger may
name it, but authorization to return that login to a profile requires
[trusted identity](#rotation-login-identity) and the applicable ownership
checks. `~/.claude.json` cannot confirm or contradict the owner.

| Evidence | Result |
|----------|--------|
| The live login is missing or unsafe | No swap; report the login/file-safety failure. Never fill an empty `~/.claude`. |
| A pending journal or hand-change suspicion is unresolved | Finish authorized recovery or report the refusal; no new swap. |
| A usable ledger names a live profile | Retain that placement label and validate the login against applicable expected ownership evidence before movement. |
| No usable ledger; trusted evidence establishes a unique login-less owner | Use that profile, subject to the same ownership and structural checks. |
| No usable ledger; a prototype ledger names a profile | Accept only with the existing prototype trust/placement checks and trusted ownership evidence. Its name alone cannot establish identity. |
| More than one owner, or required evidence is uncertain or conflicting | Alert and move nothing; do not relabel the ledger. |
| A complete trusted scan establishes no owner | Adopt only as part of an authorized swap. |

Adoption creates `default-original`, `default-original-2` and so on under
the existing lock. Its display data and ownership record come from the
trusted outgoing identity; a session block cannot claim an empty profile.
No credentials are copied. Failure before exchange leaves the login in
place. An unfinished empty adoption is not evidence of ownership; retry
must establish ownership again. Existing profile creation, journal and
cleanup rules still apply.

### Rotate excluded live profile

When the login in `~/.claude` belongs to a profile in the auto-switch
`exclude` list, the rotator moves off it without probing or refreshing it.
An excluded login is often shared with another host, and a token refresh
from this host would rotate its refresh token and sign that host out
([#2218](https://github.com/rysweet/amplihack-rs/issues/2218)).

The `exclude` list is read once per cycle, under the rotate lock and before
any recovery, refresh or identity read. A cycle that cannot read the list
treats every profile as excluded and pauses; see
[Unreadable exclude list](#unreadable-exclude-list).
Call the excluded live profile `C`. The rotator identifies `C` from local
files only, as described under
[Local proof of an excluded live login](#local-proof-of-an-excluded-live-login).
In every cycle that finds `C` live, the rotator:

- sends no request of any kind with `C`'s token: no usage request, no token
  refresh and no `GET /api/oauth/profile` identity read, on the first cycle
  and every later one;
- starts no `claude` for `C`;
- skips the one refresh of an [expired live login](#an-expired-live-login),
  and, when `C`'s token has expired, logs
  `refresh: live profile C is excluded; token not refreshed`;
- writes nothing to `C`'s credential files, except the
  [write-back](#write-back-into-an-excluded-folder) of a move off `C`;
- records no reading for `C` in the [usage history](#usage-history),
  `usage.json`, the [capacity file](#capacity-file), the ledger's
  `swapped_away` list or the no-room record.

The swap protections of
[#1845](https://github.com/rysweet/amplihack-rs/issues/1845) and
[#1886](https://github.com/rysweet/amplihack-rs/issues/1886) still need a
[trusted reading](#rotation-login-identity) of the login in `~/.claude`
before any move. For an excluded live login that reading comes from local
files instead of the server. The incoming target is still read over the
network, as for any swap.

#### Local proof of an excluded live login

Once per cycle, after the rotate lock is taken and auto-switch recovery has
run, and before [swap recovery](#rotate-recovery), the
[expired-login refresh](#an-expired-live-login),
[hand-change recovery](#rotate-hand-change-recovery) and the snapshot, the
rotator decides from local files alone which logins it must never send to
the server. It reads no token and sends no request to decide.

The login in `~/.claude` is taken to be the excluded profile `C` by the
first of these rules that matches:

| Rule | Condition | Match |
|------|-----------|-------|
| 1 | The rotate ledger's `live` field names `C`, and `C` is in the `exclude` list. This rule is enough on its own. | `ledger` |
| 2 | The `oauthAccount` in `~/.claude.json` matches `C`'s [recorded account](#recorded-account) by full [account identity](#account-identity), organisation included. | `local account` |
| 3 | `~/.claude/.credentials.json` is byte for byte the same as `<root>/C/.credentials.json`. | `same bytes` |

Rules 2 and 3 apply whatever profile the ledger names, because a `claude
login` or a copy made by hand leaves the ledger naming the profile that was
live before it. There is one exception to rule 2. After a swap off `C`,
sessions still running on `C` keep writing `C`'s account into
`~/.claude.json`. Rule 2 is therefore skipped while all of these hold:

- the ledger's `live` names a profile that is not excluded;
- the ledger has no `hand_suspect`;
- the change time of `~/.claude/.credentials.json`, read by `lstat`, is no
  later than the ledger's `swapped_at`, and `swapped_at` is not in the
  future.

Any write to the login file after the swap, by a token refresh or a
`claude login`, ends the exception, and rule 2 applies again. Rule 3 is
never skipped. Outside the exception, a stale `~/.claude.json` that still
names `C` also marks `~/.claude`: its identity is then taken from local
files and the check fails closed. A match by rule 2 alone is never
proof enough to [write `~/.claude` back](#write-back-into-an-excluded-folder)
into `C`'s folder, so nothing is moved and the cycle raises the ALERT shown
under [Write-back into an excluded folder](#write-back-into-an-excluded-folder)
until the file is corrected or the ledger names `C`.

An e-mail or a prefix alone never matches in rule 2. `~/.claude.json` is
read without following a symlink, up to the usual size limit, and must be
owned by you and not writable by group or other users. A file that fails
any of these checks counts as missing. Any read error makes that rule give
no match; it never sends a request in its place.

Every profile folder named in the `exclude` list is also marked, for the
whole cycle, so that no identity read is sent for it. When a rule matches,
`~/.claude` is marked as well. For a marked folder the identity of the login
is taken from its local account (`.claude.json`) in place of
`GET /api/oauth/profile`. When no local account is found, the identity read
fails with `unavailable` and no request is sent. The other checks a trusted
reading needs still apply: the shared identity-work budget, the
[login directory rule](#login-directory-rule), the login kind, and the
metadata of `.credentials.json` before and after the read. A `/login`, or
any other change to that file between the proof and its use, gives
`login changed during read` and nothing is moved.

Paths are compared both as given and after resolving symlinks, so a
profile root reached through a symlinked parent folder is still marked.
[`profile seed-record`](#profile-seed-record) and the swap recovery run when
the rotator starts use the same marked set. Swap recovery also marks the
folder that holds the outgoing login of an interrupted swap when the
outgoing profile is excluded, wherever the swap left it.

The marked set is rebuilt at the start of every cycle from parsed profile
names and `~/.claude` only. A change to `exclude` made while a cycle runs
takes effect on the next cycle.

[`rotate status`](#rotate-status) uses the same rules to name the live
profile. When the ledger has no `live` field and a rule names `C`, the
status shows `C` as live and sends no request.

#### Moving off an excluded live profile

The decision treats `C` as a live login that must be left in this cycle:

| Case | Outcome |
|------|---------|
| A candidate meets the margin | Swap to the best candidate by the [decision rule](#rotate-decision-rule), `reason=threshold`, on the first cycle. `C` counts as headroom `0`, as an exhausted live login does. [Failed-probe damping](#rotate-failed-live-probes), `--hold`, the trusted-identity stop and the [home-key stay](#api-key-login-check) do not apply. |
| No candidate meets the margin | The [plan-room](#rotate-plan-room-swap) and then the [overflow](#overflow-decision) rules choose a target as they do for an exhausted live login. |
| No move is possible | ALERT `excluded-live`. `C` stays in `~/.claude`, unprobed and unrefreshed. The cycle records `alerted`, which is not capacity by the [rotator capacity rule](#rotator-capacity-rule), and the ALERT is never [advisory](#rotate-advisory-alert). No retry, wait or loop is added: the next normal cycle decides again. `~/.claude` is never left empty. |

`C`'s reading is never counted as room: it is not capacity for a stay, for
the [capacity file](#capacity-writers) or for the usage-limit wait. After the
swap, `C` is a profile in its own folder again and is skipped as `excluded`
by every later cycle, so the rotator never swaps it back in, however much
room `C` may have.

A swap off `C` writes:

```
2026-11-10T08:00:05Z cycle live=shared shared:excluded (not probed) review:s=5%,w=10% work:s=20%,w=30%
2026-11-10T08:00:06Z note: live profile shared is excluded; moving off it
2026-11-10T08:00:06Z swap shared->review live excluded (not probed) target s=5% w=10% reason=threshold
```

With no eligible candidate:

```
2026-11-10T08:00:05Z cycle live=shared shared:excluded (not probed)
2026-11-10T08:00:05Z ALERT: live profile shared is excluded; no eligible candidate
```

The ALERT is written once per key, like any other.

#### Write-back into an excluded folder

A normal swap moves the outgoing login from `~/.claude` back into its
profile folder. For `C` this move is the only write the rotator ever makes
to `C`'s credentials. It is the existing journalled exchange of login files,
with no refresh and no request. Without it, a token that a session on this
host refreshed would be lost and `C`'s stored copy would go stale.

The move into `<root>/C/` is made only when one of these holds:

- the match came from rule 1 (`ledger`), no hand change is suspected, and
  `~/.claude.json` agrees with `C`'s record;
- `~/.claude/.credentials.json` is already byte for byte the same as
  `<root>/C/.credentials.json` (rule 3, or rule 1 with identical files).

In every other case, for example a match from rule 2 alone or any match
while a hand change is suspected, nothing is moved and `C`'s folder is left
as it is. The cycle writes, once per key:

```
ALERT: live profile C is excluded; not moved, as only ~/.claude.json names it
```

Such a match is not written to the ledger as the live profile, so a later
cycle cannot take it for a rule 1 match. While `~/.claude` is marked,
[hand-change recovery](#rotate-hand-change-recovery) and the seeding of a
[recorded account](#recorded-account) do not run either: both would act on
the local account alone. This stops a stale `~/.claude.json` after a manual
`/login` from moving another account's login into `C`'s folder, the slot
mix-up that #1845 and #1886 fixed. The ALERT clears once the ledger names
the live login again, for example after
[`profile switch`](#rotate-switch) or a fresh `profile login`.

#### Limits of the rule

- An expired access token does not stop the move. No identity read is sent
  for `C`, so the login is moved back to `C`'s folder unrefreshed. The
  rotator never refreshes the token to get the login moved.
- The local proof trusts the ledger, `~/.claude.json` and the stored
  record. A login file copied by hand into `~/.claude`, with a ledger that
  names `C`, is taken to be `C`'s. The write-back rule above stops it being
  copied into `C`'s folder unless `~/.claude.json` also agrees. Run
  [`profile verify`](#profile-verify) after any hand copy.
- When `~/.claude.json` gives no account and the login matches no excluded
  folder byte for byte, and the ledger does not name `C`, the rotator cannot
  tell that the login is excluded. It is treated as any other live login.
  Knowing which host owns a login without these files is the job of the
  lease design in [#1019](https://github.com/rysweet/amplihack-rs/issues/1019).
- When the ledger names a live profile that is not excluded, no hand change
  is suspected, and only `~/.claude.json` points at the excluded profile `C`,
  no rule matches. Rule 1 does not name `C`, and rules 2 and 3 apply only
  when the ledger has no `live` field or a hand change is suspected. The
  login is treated as the profile the ledger names, and may be probed or
  refreshed as that profile. Once a hand change is suspected, rule 2 or 3
  applies on the next cycle.
- When `<root>/.auto-switch` cannot be read or is unsafe, the cycle pauses
  and sends nothing; see [Unreadable exclude list](#unreadable-exclude-list).
- A pin that holds rotation on `C`, set before `C` was excluded, still holds.
  The held cycles do not probe its spend: see [Pin and overflow](#pin-and-overflow).

`--dry-run` prints the same decision as `would swap:` or `would alert:`.
Like a real cycle, it sends no request with `C`'s token and writes nothing.
When `<root>/.auto-switch` cannot be read or is unsafe it stops where a
real cycle pauses, before measuring, and prints
`would alert: auto-switch config unreadable or unsafe; rotation paused, no profile probed`.

#### Unreadable exclude list

An exclude list is only as trustworthy as whoever can write it, and a
writer can remove an entry as easily as add one. Rewriting `exclude=shared`
to `exclude=` would get `shared` probed and refreshed, and that refresh
signs out the other hosts that share the login. So the list is read from
`<root>/.auto-switch` only when it is a regular file owned by you and not
writable by group or others (group write by your own private group is
allowed, as elsewhere). A private file of yours that parses but is invalid
for its other settings, such as an unknown kind, still has its list
applied, since only you could have written it; those other settings, such
as overflow profiles, are ignored, and the cycle logs once:

```
warning: auto-switch config invalid; exclude list applied, other settings ignored
```

When the file exists but cannot be read, does not parse, or is unsafe
(owned by another user, or writable by group or others), the rotator cannot
tell which logins are excluded, so the cycle fails closed: every profile is
treated as excluded, so recovery, the live refresh and the identity checks
send nothing, and the cycle stops before measuring. It logs:

```
warning: auto-switch config unreadable or unsafe; every profile treated as excluded, none probed or refreshed
ALERT: auto-switch config unreadable or unsafe; rotation paused, no profile probed
```

Rotation resumes from the first cycle that reads the file. To fix the file:

- When it is a private file of yours that parses but is invalid for its
  other settings, run `amplihack profile auto-switch on` or `off`. They
  rewrite it with mode `0600` and the defaults for every other setting, and
  keep its exclude list, so the logins it excluded stay excluded.
- When it is owned by another user (for example one left by
  `sudo amplihack profile auto-switch …`), is a symlink, or is not a
  regular file, no command replaces it, and every command that writes it
  refuses with `AutoSwitchExcludeAtRisk`, `--exclude` or not. While it is
  there every profile counts as excluded. Do not delete it, which leaves no
  exclusions. Make it a regular file of yours holding the `exclude=` line
  you intend instead (check the line: another user could have edited it): `sudo chown "$USER" <root>/.auto-switch` for a
  foreign owner, or, for a symlink, copy its target over it
  (`cp --remove-destination "$(readlink -f <root>/.auto-switch)" <root>/.auto-switch`).
  Then run `on --exclude …` or `off --exclude …` as below.
- When it does not parse, or it is your file but writable by group or
  others, its exclude list is unknown: whoever else can write it could have
  removed an entry. `on`, `off`, `threshold`, `exclude`, `overflow add` and
  `overflow remove` refuse it with `AutoSwitchExcludeAtRisk`:

  ```
  error: auto-switch settings: auto-switch config does not parse or others can write it, so its exclude list is unknown; not changed. Run `amplihack profile auto-switch on --exclude NAME,...` or `off --exclude NAME,...` naming the logins that must stay excluded (`--exclude ''` for none)
  hint: do not remove <root>/.auto-switch: that drops its exclude list, and the rotator would probe and refresh the logins it excluded; see "Unreadable exclude list" in docs/reference/profile-command.md
  ```

  Run `amplihack profile auto-switch on --exclude NAME,...` to turn
  rotation on, or `amplihack profile auto-switch off --exclude NAME,...` to
  leave it off, with the logins that must stay excluded (`--exclude ''`
  names an empty list on purpose). Do not remove the file and run a bare
  `on` or `off`: that writes an empty list just the same. Writing an empty list by mistake would let the
  next cycle probe and refresh those logins, which signs out the other hosts
  that share them.

[`profile usage`](#excluded-profiles) and [`profile verify`](#profile-verify)
print the same warning on stderr and read no profile, as described in their
sections.

Every other reader of the exclude list follows the same rule: in-session
auto-switch swaps, its re-run and takeover checks, the capacity figures and
`profile auto-switch status`, whose `exclude:` line shows the list in force.
For a corrupt or unsafe file their other settings are still the defaults,
but the exclude list is the file's when it is a private file of yours that
parses, and every profile otherwise. With `AMPLIHACK_CLAUDE_AUTO_SWITCH=1`
and a file that does not parse or is unsafe, auto-switch is on but every
profile is excluded, so it swaps nothing. If the list is unknown and the
profile root cannot be listed either,
"every profile" is unknown, and it is never treated as an empty list: `profile usage`, `profile verify` and the rotate
cycle fail with the error, the live refresh is skipped, and in-session
auto-switch counts as off.

Treating an unsafe file as unreadable is a trade-off. Whoever can write it
cannot un-exclude a login, because nothing they write is applied, but they
can stop rotation by making the file unsafe or unparseable: every profile
is then excluded, a denial of service. That is the only power they get.
Treat the `unreadable or unsafe` warning as a sign that the file needs
fixing, and possibly that someone else can write it.

### Rotate candidates

A profile is a candidate unless any of these holds, checked in this order.
Every reason but `live` is logged as a `skip:` line: by `watch` only when the
set of skipped profiles changes, and by the one-shot command on every run.

| Skip reason | Condition |
|-------------|-----------|
| `live` | It is the live profile. A live profile in the `exclude` list is handled as in [Rotate excluded live profile](#rotate-excluded-live-profile). |
| `directory refused` | The profile directory fails the ownership and mode check: it is a symlink, not a directory, not owned by you, or open to group or other users. A profile in the auto-switch `exclude` list is skipped as `excluded` instead. While a plan profile has this reason, the rotator does not move onto an overflow profile (see [Overflow decision](#overflow-decision)). |
| `no login` | `<root>/<name>/.credentials.json` is missing, a symlink, not a regular file, not owned by you, readable by other users, or has more than one link. |
| `excluded` | It is in the auto-switch `exclude` list. It is not probed and no request is sent with its token. Its `.credentials.json` is read only for the byte comparison of [rule 3](#local-proof-of-an-excluded-live-login), and written only by the [write-back](#write-back-into-an-excluded-folder). It never reaches the `no account` check or the unbound set. |
| `session running` | A running process has `CLAUDE_CONFIG_DIR=<root>/<name>` (the same `/proc` scan auto-switch uses). If `/proc` cannot be scanned, every profile is skipped and `skip: cannot see running sessions; no candidates` is logged once. |
| `leased` | Its login is on loan to another profile's session in the auto-switch ledger. |
| `hosting` | Its directory hosts another account's login in the auto-switch ledger. |
| `parked` | `<root>/.auto-switch.d/parked/<name>` exists. |
| `no account` | Its login has no usable trusted account evidence for the required ownership check. Session display data cannot supply it. The cycle also writes `ALERT: trusted login identity unavailable for profile <name>: <cause>`, with the [cause](#identity-read-causes). |

### Rotate swap

`L` is `~/.claude/.credentials.json`, `T` is `<root>/<to>/.credentials.json`
and `S` is `<root>/<from>/.credentials.json`, where `<from>` is the live
profile (or the adopted one).

Before anything is written, every check below must pass; otherwise the swap
fails with `RotateRefused` and nothing changes:

- the profile root is private (as for auto-switch state writes); `$HOME` is
  absolute, a directory rather than a symlink, and owned by you; `~/.claude` is owned by you and
  not writable by others (group write is accepted only for your own private
  group, as in the [login directory rule](#login-directory-rule)); both profile directories pass the usual
  directory check;
- `L` and `T` are regular files owned by you, with no group or other mode
  bits and exactly one link (metadata checked with `lstat`; trusted
  identity is read separately by the bounded reader);
- `S` does not exist;
- trusted outgoing and incoming identities are bound to the logins being
  exchanged; their full accounts satisfy applicable expected records,
  duplicate claims and placement checks;
- required ownership scans are complete. An unreadable expected record or
  duplicate login is uncertainty and blocks movement; missing records can
  follow the existing trusted adoption/backfill rules;
- these bindings and material dependencies are revalidated under the swap
  locks immediately before exchange. Refresh requires fresh matching
  evidence; an earlier inode or account key is insufficient;
- display repairs use the trusted account, preserving unrelated fields
  and file modes. A display file that cannot be safely updated refuses
  before login movement. Its existing account never authorizes a move;
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
| `j0` | Write `journal.json` with `from`, `to`, `adopt`, the inode numbers of `L` (`live_ino`) and `T` (`target_ino`), and `$HOME`. Then take Claude's refresh locks (`.oauth_refresh.lock`, directories made with `mkdir`, as auto-switch does) in `~/.claude` and `<root>/<to>`, recording each acquired inode (`home_lock_ino`, `to_lock_ino`). A lock held by someone else is never broken. Before `s1`, validate fresh bound identities and material ownership dependencies under these locks; uncertainty prevents the exchange. |
| `s1` | Atomically exchange `T` and `L`. `~/.claude` now holds the incoming login; `<root>/<to>` holds the outgoing one. |
| `s2` | Rename the outgoing login from `<root>/<to>` to `S`, refusing to replace an existing file. Release the refresh locks: each is removed with `rmdir` only if its inode is the one recorded. |
| `s3` | Replace the `oauthAccount` display block in `$HOME/.claude.json` from the validated incoming identity, keeping every other key and the file's mode. |
| `s4` | Write the ledger: `live=<to>`, `previous=<from>`, `account` from the validated incoming account key, `since` and `swapped_at`. Clear any ALERT key. |
| | Delete `journal.json`. |
| | Write the [recorded account](#recorded-account), if one is due: for an adopted login, the outgoing account for the new `default-original[-N]` profile, only when no other profile holds that account and organization or has it recorded (otherwise a record left under that name is deleted); for `<from>` with no record, the trusted outgoing account, only when the existing ledger/provenance and compatibility checks establish ownership and a complete trusted scan finds no other claim. Session JSON cannot supply missing ownership proof. For `<from>`, the record is read again first, and a usable one is never overwritten. Nothing is recorded for `<to>`. A failure prints `warning: the recorded account for profile '<name>' was not saved: <reason>` and the swap still succeeds. |

A swap that [recovery](#rotate-recovery) finishes writes no record.

The identity reader uses the logins for bounded identity requests; movement
uses only the exchange and rename, never copy-and-delete. Each parent
directory is synced after each rename. At every point each
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
it. Recovery keeps the existing journal schema and inode-placement rules,
and reconstructs fresh login-bound identity and applicable expected-record
evidence before any further movement or ledger settlement. It never
deletes or overwrites a login.

Recovery selects the transaction action by where the recorded inodes are,
found with `lstat`. Every action that moves a login or settles ledger
identity also requires fresh trusted authorization. The journal's `phase`
is a hint for the log and for `status`; it never decides.
This matters most for the commonest crash point, just after the exchange and
before the journal is rewritten as `s1`, where the journal still says `j0`
but the logins have already moved.

`L` and `T` are not tested in `~/.claude`. Claude may refresh the token in
`~/.claude` at any time, which replaces the file there with a new inode, so
neither recorded inode can be expected in `~/.claude` after the exchange.
A regular file alone is insufficient: fresh bound identity must establish
the account currently there, including after a token refresh.

| Where the logins are (any phase) | Action | Log line |
|----------------------------------|--------|----------|
| No journal | Nothing. | |
| `target_ino` is at `<root>/<to>/.credentials.json`, `S` does not exist, and `~/.claude/.credentials.json` is a regular file | The exchange did not happen. Roll back: release the recorded refresh locks, then delete the journal. | `recovery: rolled back an interrupted swap work->spare` |
| `live_ino` is at `<root>/<to>/.credentials.json`, `S` does not exist, and `~/.claude/.credentials.json` is a regular file | The exchange happened; `s2` did not. Roll forward: run `s2`, `s3` and `s4`, then delete the journal. | `recovery: finished an interrupted swap work->spare` |
| `live_ino` is at `S`, nothing is at `<root>/<to>/.credentials.json`, and `~/.claude/.credentials.json` is a regular file | `s2` happened. Roll forward: release the recorded refresh locks, run `s3` and `s4`, then delete the journal. `s3` and `s4` are safe to repeat. | `recovery: finished an interrupted swap work->spare` |
| Required trusted evidence is unavailable, insufficient, changed or conflicting | Retain the unresolved journal, leave login placement and ledger identity unchanged, and safely release operation-owned refresh locks. Retry requires fresh evidence; report the identity refusal on each affected attempt. | Actionable identity ALERT; no swap. |
| Inconsistent login placement or a failed directory check, with a valid journal for the current `$HOME` | Safely release the recorded operation-owned refresh locks, then mark the journal `inconsistent`. | `ALERT: rotate recovery is inconsistent: <reason>; no swap until it is resolved` |
| A `home` that differs from `$HOME` | Leave the journal, logins and refresh locks untouched for a run with the correct `$HOME`. | `ALERT: rotate recovery is inconsistent: <reason>; no swap until it is resolved` |
| An unreadable or invalid journal, including an invalid name or unknown version | Refuse recovery without acting on the untrusted journal; leave it and the refresh locks for resolution. | Journal error; no swap. |

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
names a profile `L` whose login is no longer the one in `~/.claude`.

Hand-change recovery runs under the lock in every non-dry-run `rotate`,
`start` and cycle, after [rotate recovery](#rotate-recovery) and before the
live profile is identified. `profile switch` does not run it: a switch
refuses while a suspicion is open (see [Rotate switch](#rotate-switch)) and
leaves recovery to the next cycle.

Recovery identifies a login by what its credential returns, never by
`.claude.json`. Running Claude sessions rewrite `~/.claude.json` from the
account they hold in memory, so for some minutes after a swap that file can
still name the previous account
([#1886](https://github.com/rysweet/amplihack-rs/issues/1886)). No
`.claude.json` file is read for any recovery decision; the ledger's
`account` field and the `oauthAccount` blocks are used for display only.

Recovery moves at most one login, by rename, and only after two matching
readings on separate cycles. It never moves a login that is a profile's only
login, and it never opens, copies or deletes a credentials file itself.

#### Credential identity

A login's *identity reading* is one `GET /api/oauth/profile` to Anthropic's
OAuth profile endpoint, the endpoint Claude Code's `/status` reads, made
with that login's own OAuth token. The server answers with the account and
organization the token belongs to; the rotator keeps `account.uuid`,
`account.email` (lower-cased) and `organization.uuid`, each at most 320
bytes and free of control characters. An answer with neither a uuid nor an
e-mail cannot be used and gives the cause `unparseable`.

Two readings are one account when they report the same account uuid in the
same organization (by e-mail only when neither reports a uuid): one user in
two organizations is two accounts. `~/.claude.json` is not compared with the ledger; its account block is display state. The identity of a login is therefore
what the server says about its token, not what any local file says.

Recovery does not use any of these to identify a login, as the owner
decided on [#1886](https://github.com/rysweet/amplihack-rs/issues/1886) on
3 November:

- `.claude.json`, which running sessions rewrite from memory;
- `claude auth status`, which reads the config folder's `.claude.json` and
  answers even with the network blocked;
- usage reset times. Earlier builds of this fix compared the reset times of
  the usage windows, but accounts created together share them: on
  3 November `nnorth` and `mmorrison` both reset at 14:40Z (session) and
  2026-11-06T05:00Z (week). Two such accounts read as one, and every swap
  onto one of them ended in the [only-login ALERT](#only-login-alert).

Identity reads differ from the reads `profile usage` makes:

- token refresh is off, so an identity read never writes a credentials file
  and never takes `.oauth_refresh.lock`; a token at or past its `expiresAt`
  is not sent and gives the cause `token expired`;
- each read has a 5-second timeout, and all reads in one cycle share a
  30-second identity-work limit, excluding usage measurement; reads not
  started when the limit runs out are not made.
  A read started just before the limit may still take its full 5 seconds,
  so that assessment ends within 35 seconds. Rotation ownership checks
  share the existing bounded assessment budget rather than starting
  unbounded duplicate scans. Final rechecks retain their existing limits
  (see [Checks before the move](#checks-before-the-move));
- the endpoint base comes from the same setting as the direct read
  (`AMPLIHACK_PROFILE_USAGE_API_BASE`); an endpoint that fails the direct
  read's checks means no request is sent;
- [passive readings](#passive-readings), the usage endpoint and the
  `claude -p /usage` [fallback](#fallback-probe) are never used, and no
  child process is started.

Raw identity responses and file bindings remain in memory. Only sanitized
account fields are retained in the existing ledger/journal; no token or
raw response is stored. Swap completion
uses the validated incoming account for the existing ledger key and
sanitized display fields; authorized adoption/backfill uses the trusted
outgoing identity for the outgoing profile's ownership record. Routine
logs print no account identifiers or e-mails. Only
[`profile verify`](#profile-verify) prints an e-mail, on request. The token
goes only in the request's `Authorization` header.

##### Identity read causes

A failed identity read has one fixed cause. The rotate log and the dry run print
it, and so does `profile switch` when the read of the login in `~/.claude` fails.
A cause never holds a token, an account or server text.

| Cause | When |
|-------|------|
| `token expired` | The token is at or past its `expiresAt`, and no refresh replaced it. |
| `http 401`, `http 403`, `http 429` | The endpoint answered with that status. |
| `http 3xx`, `http 4xx`, `http 5xx` | Any other status of that class. |
| `timeout` | No answer within the 5-second timeout. |
| `transport` | The connection or TLS failed, the server closed it, or the endpoint setting is unusable, so nothing was sent. |
| `unparseable` | The body is too large, is not a JSON object, or names no account. |
| `no oauth token` | The login file has no usable token. |
| `directory not private` | The directory fails the [login directory rule](#login-directory-rule). |
| `login file not trusted` | The login file is missing where a login is required, is a symlink or not a regular file, belongs to another owner, can be written by group or others, has more than one link, or is on a different device from its directory. A [seeding scan](#seeding-a-record) treats a definitely missing login in another profile as no login, not as this cause. |
| `login changed during read` | The file or directory changed between the checks before and after the read. |
| `identity read budget used up` | The cycle's 30-second identity limit ran out before this read started. |

The ALERT for the live login is:

```
ALERT: trusted login identity unavailable: <cause>
```

This ALERT is written on every cycle it applies to, with the cause that cycle
found, whether or not the cause changed. So is the candidate form,
`ALERT: trusted login identity unavailable for profile <name>: <cause>`, the
[unreadable-ledger ALERT](#unreadable-ledger-stop) and the
[same-account ALERT](#seeding-a-record).

`profile switch` refuses with the same text when the read of the login in
`~/.claude` fails. Swap authorization does too, in `profile switch` and after
`ALERT: swap refused: ` in a cycle, when a read of either login or a read in its
scan of other profiles fails. Other identity refusals have no cause and keep their
fixed text. These are a profile root that cannot be listed, a login or record that
changed before the exchange, and a refresh lock that recovery does not recognize:

```
trusted login identity unavailable or changed; retry after checking profile verify
```

Swap authorization's specific refusals also stay as they are, except that
`trusted login identity: outgoing ownership is not established; run: amplihack profile seed-record <name>`
now names the command.

##### An expired live login

Because an identity read never refreshes, a login in `~/.claude` whose token
is at or past its `expiresAt` could not otherwise be identified, and with no
running Claude session to refresh it every cycle would raise the
trusted-identity ALERT and move nothing. So each rotate cycle and
`profile switch NAME`, after swap recovery and before hand-change recovery or
any identity read, refreshes that login once when its token has expired, as
the [direct read](#token-refresh) refreshes a profile's login, under
`~/.claude/.oauth_refresh.lock`.

The refresh does not depend on the rotate ledger or the lease view, so an
unreadable ledger or a lease view that bars the live profile does not stop it.
That exception covers `~/.claude` only: a profile folder the lease view bars is
never refreshed. These still refuse the refresh:

| Refusal | Reason in the log |
|---------|-------------------|
| A rule under [Local proof of an excluded live login](#local-proof-of-an-excluded-live-login) names an excluded profile as the live login | none: the line is `refresh: live profile <name> is excluded; token not refreshed`. See [Rotate excluded live profile](#rotate-excluded-live-profile). |
| A swap journal exists | `refresh busy` |
| Another process holds the refresh lock | `refresh busy` |
| The directory fails the [login directory rule](#login-directory-rule) | `refresh refused` |

A token that has not expired is left alone, and nothing is logged for it;
Claude refreshes it. The log says
`refresh: expired login in ~/.claude refreshed; identity read again`, or
`refresh: expired login in ~/.claude not refreshed (<reason>)` with one of the
direct read's reasons.

The refresh is not identity evidence. The cycle reads the identity of the
refreshed credential afresh, and only that reading can let a login move. A
refresh that fails, or a refreshed login whose identity cannot be read or
conflicts with its record, still gives the ALERT and moves nothing.

`--dry-run` never refreshes. For an expired live login it prints
`would refresh: expired login in ~/.claude` and then reports the cause
`token expired`, which a real cycle's refresh might have cleared.

A refresh replaces the login's token on disk. A running session may still hold
the old refresh token in memory.

##### Unreadable-ledger stop

When the rotate ledger exists but cannot be read, the cycle still refreshes an
expired live login and reads its identity, then stops:

```
ALERT: rotate ledger unreadable[ (<field>)]; live identity: <readable|cause>; nothing moved, no record written
```

`(<field>)` appears when the ledger was read and parsed but a field was
refused. It is one of these fixed texts, and never a value from the file:
`unknown version`, `invalid profile name`, `invalid account or alert`,
`invalid overflow field`, `invalid no_room`, `invalid hand-change field` or
`invalid login_identity`. A ledger that cannot be read, is not a private
file or is not valid JSON gives the line without it.

The cycle ends as `alerted`, seeds no record, moves no login and leaves the
ledger file unchanged. With no ledger to hold an ALERT key, the line is written on
every cycle while the ledger stays unreadable. `--dry-run` prints the same text after `would alert: `,
and `profile switch` refuses with it.

A ledger whose `login_identity` fails the load-time rule under
[Account identity](#account-identity) is one case of this stop, written as
`rotate ledger unreadable (invalid login_identity); live identity: ...`.

#### When recovery reads

Let `L` be the profile the ledger names as live. Recovery returns at once,
makes no request and writes no line, unless all of these hold:

1. the ledger names a live profile `L`;
2. no swap journal is pending;
3. the last swap is at least `SWAP_GRACE` (600 seconds) old, measured from
   the ledger's `swapped_at`, or the ledger has no `swapped_at`;
4. `~/.claude/.credentials.json` is a regular file holding an OAuth login.

`SWAP_GRACE` covers the minutes after a swap during which running sessions
can still write the previous account into `~/.claude.json` and refresh the
token they hold. With the recommended `--interval 120`, it spans five cycles.
A `swapped_at` more than `SWAP_GRACE` later than the cycle's time (the clock
went back) also turns recovery off; the cycle writes the
`the last swap time is later than now` note described under
[Notes](#hand-change-notes).

When it reads, recovery reads `~/.claude` first. When that account is the
one [recorded](#recorded-account) for `L`, the finding is Clear and no
profile is read. Otherwise it reads the profiles: each profile that passes
the usual directory check and has its own OAuth `.credentials.json`, in name
order. `L` is read too when its directory still holds a copy. A cycle
therefore makes one request when `~/.claude` cannot be read or holds `L`'s
recorded account, and at most one more for each profile with its own
login.

#### Findings

The `~/.claude` account is compared with `L`'s [recorded
account](#recorded-account) and with each profile's reading. The first row
that holds decides:

| Readings | Finding | What happens |
|----------|---------|--------------|
| The `~/.claude` read failed | Cannot tell | A [note](#hand-change-notes) with the [cause](#identity-read-causes). Nothing moves; the ledger is not relabelled. |
| The account recorded for `L` | Clear | Nothing. No line. No profile is read. |
| Same account as more than one profile | Cannot tell | A note naming those profiles. |
| Same account as `L`'s own copy | Clear | Nothing. No line. |
| Same account as exactly one other profile `M`, and `M` has a record naming another account | Cannot tell | A note: `M`'s own folder holds a login that is not `M`'s; run [`profile verify`](#profile-verify). |
| Same account as exactly one other profile `M` | Suspect `M` | See [Two readings](#two-readings). |
| Same as none, and one or more profile reads failed, or the limit ran out | Cannot tell | A note naming those profiles and each one's cause. |
| Same as none, and `L` has a record naming another account | Cannot tell | A note pointing at [`profile verify`](#profile-verify). |
| Same as none, and `L` has no record | Clear | No recovery action. This proves no duplicate was found, not that `L` owns the live login. A later swap still requires trusted authorization, and [seeding](#seeding-a-record) decides whether `L` gets a record. |

A profile with no login of its own is never compared, so recovery never
records a login-less profile as live. Such a profile's login can only be in
`~/.claude`, and nothing the credential returns says whose it is. The cycle
keeps the ledger's `L`. Earlier releases recorded that profile from
`~/.claude.json`, or kept `L` because its [recorded account](#recorded-account)
matched; neither happens now.

A Clear finding that no profile holds a duplicate does not establish the
live login's owner. Any later swap still requires trusted identity and
applicable ownership checks. A recorded-account conflict refuses movement
regardless of session JSON; missing records use the existing trusted
adoption/backfill rules or [seeding](#seeding-a-record). An uncertain scan cannot authorize movement or
ledger relabeling. See [Rotation login identity](#rotation-login-identity).

#### Two readings

The first "Suspect `M`" writes the ledger's `hand_suspect` (`M` and the
cycle's time as `first_seen`) and the suspicion note. It never moves a
login or relabels the ledger. A later cycle confirms the suspicion only when
all of these hold:

- its finding is "Suspect `M`" for the same `M`;
- at least one rotation interval (`--interval`, at least 1 second) has
  passed since `first_seen`;
- `first_seen` is not earlier than `swapped_at` and not later than the
  cycle's time.

A finding of "Suspect" for a different profile replaces the record and
starts again. Any other finding clears it. A `first_seen` later than the
cycle's time resets the record to the cycle's time.

While a suspicion is open, the live login counts as hand-changed: the cycle
writes the note, then the `hand-change` ALERT from
[Rotate live profile](#rotate-live-profile), measures nothing, does not swap,
and ends as `alerted`. `profile switch` refuses in the same way. A rotator
that has stopped leaves the suspicion in the ledger, and switching stays
refused until a cycle runs; `amplihack profile rotate` with no subcommand
runs one.

#### Only-login rule

A confirmed suspicion means the server reported the same account, twice,
for `~/.claude` and for `<root>/<M>/.credentials.json`: they are two logins
of one account. Recovery may move `M`'s copy aside and
record `M` as live only if every profile that had a login before the cycle
still has one after it. `L` had the login in `~/.claude`; once `M` is live,
`L` keeps a login only if `<root>/<L>/.credentials.json` holds one: a
regular file owned by you, opened without following a symlink, not writable
by group or others, and holding a JSON object (an OAuth login or an API-key
profile's file). A symlink, another file type, another owner, or an empty,
unparsable or unreadable file counts as no login.

| `L` after the move | Action |
|--------------------|--------|
| Has its own login | The checks below run. If they pass, `M`'s copy is moved and `M` is recorded as live. |
| Would have no login anywhere | Refused. The [only-login ALERT](#only-login-alert) is written; both logins stay where they are, the ledger still names `L`, and the suspicion stays open, so the cycle does not swap. |

A manual `/login` in `~/.claude` over a login the rotator swapped in
therefore ends in the only-login ALERT, not in an automatic recovery: the
`/login` already replaced `L`'s only login, and recording `M` would hide
that.

Because identity is the server's account, two different accounts never
reach this rule, however their usage windows line up. A swap onto one of a
pair of accounts that reset at the same times goes ahead with no note and
no ALERT. The rule is reached only when `~/.claude` and `M` hold one
account: a `/login` over `L`'s login, or `L` and `M` set up as two profiles
of one account with no record for `L`.

#### Checks before the move

Checks before recording, with the `<reason>` each gives when it fails: the
profile root is private, no swap journal is pending or inconsistent, and
`HOME` and `~/.claude` pass the [swap's](#rotate-swap) home checks. Then:

| Check | `<reason>` when it fails |
|-------|--------------------------|
| No running process has `CLAUDE_CONFIG_DIR=<root>/<M>`. A host where running sessions cannot be seen fails this check. | `a claude session is running on <M>` / `cannot see running sessions` |
| Neither `~/.claude/.oauth_refresh.lock` nor `<root>/<M>/.oauth_refresh.lock` exists. | `Claude holds its refresh lock` |
| `M`'s login is not leased and `M` does not host another login in the auto-switch ledger; an unreadable auto-switch ledger fails this check. | `<M>'s login is lent out or it hosts another login` |
| `<root>/<M>/.credentials.json` is a private regular file with one link, owned by you. | `a login file is not a private regular file with one link` |
| `~/.claude`, `<root>/<M>` and the root are on one device. | `~/.claude and the profile root are on different devices` |
| `~/.claude` and `M` are read again and the server reports one account for both, and `<root>/<M>` passes the directory check again. The two re-reads have their own limit of 10 seconds. | `~/.claude changed during recovery` |
| `.rejected-logins/` and the backup directory can be created. | `cannot create the backup directory: <error>` |
| `.rejected-logins/` and the backup directory are real directories owned by you with no group or other bits. | `backup directory is not private: <path>` |
| A free backup name exists. | `no free backup name` |
| After the backup directory is created, just before the rename: the device, inode and change time of both login files are those seen at the start of the cycle. The new, empty backup directory is removed again when this check or the rename fails. | `~/.claude changed during recovery` |

The backup is `<root>/.rejected-logins/rotate-<M>-<YYYYMMDDTHHMMSSZ>/`, where
the time is the cycle's start in UTC. If that name exists, `-2` to `-99` is
added. `.rejected-logins/` is created `0700` if it is missing; one that
exists must be a real directory owned by you with no group or other bits, as
`profile login` requires. The backup directory is created fresh, `0700`,
never through a symlink, and must then be a directory owned by you with no
group or other bits; one that is not is removed again (it is new and empty)
and the move is refused. Its parent is synced. The login is moved into it
with `renameat2(RENAME_NOREPLACE)`. Nothing else is moved, and
`<root>/<M>/.claude.json` is not changed.

On success the log line is:

```
recovered: ~/.claude and <M> held the same account; kept ~/.claude, moved <M>'s copy to <backup>/.credentials.json; recorded <M> as live
```

Recording `M` sets the ledger's `live` to `M` and `since` to the cycle's
time, records the trusted credential account, and clears `alert`, `guidance` and `hand_suspect`, in one
atomic write. The cycle then carries on with the new ledger and may swap in
the same cycle. When `M` is an [overflow](#rotate-overflow) profile, the
same cycle measures and applies the overflow policy as for any cycle that
starts on credits.

The steps run in this order: checks, re-read and re-check the directory,
create the backup directory, check the inodes, move the login, write the
ledger. A refusal therefore leaves no new directory under
`.rejected-logins/`. There is no journal. Every
interrupted state is safe:

| Stopped after | State | Next cycle |
|---------------|-------|------------|
| Creating the backup directory | An empty `rotate-<M>-<time>/`. | Uses the next free name. The empty directory is left. |
| Moving the login | `M` has no login; the ledger still names `L` and holds the suspicion. | `M` is no longer compared, so the finding is Clear and the suspicion is cleared. The ledger keeps `L`. Because `<root>/<L>/.credentials.json` exists, a [swap](#rotate-swap) away from `L` is refused, so no login is saved under the wrong profile. Remove `<ledger>` to set it right; the next cycle then finds the live profile afresh. |

If a session on `M` writes a new login there after the move, later cycles
compare it again from the start.

#### Hand-change notes

A note says that recovery could not tell, or has one reading and waits for a
second. It never moves anything. At most one note is written per cycle, and
a note identical to the previous cycle's is not written again, so an expired
token in `~/.claude` writes one line, not one per cycle. The note is kept in
the ledger's `guidance` until the finding changes, and
[`rotate status`](#rotate-status) prints it as a `note:` line. The one-shot
command prints it on every run.

Suspicion, first reading or before the interval has passed:

```
note: ~/.claude holds the same account as profile <M> (ledger names <L>); nothing moved, the next cycle reads again
```

Cannot tell:

```
note: cannot tell whose login is in ~/.claude (live=<L>): <why>; nothing moved
```

| Cause | `<why>` |
|-------|---------|
| The `~/.claude` read failed | `~/.claude identity read failed: <cause>`, with its [cause](#identity-read-causes), such as `http 429` or `token expired` |
| Same account as more than one profile | `it holds the same account as more than one profile (<M1>, <M2>)` |
| One or more profile reads failed | `profile <P1> not read (<cause>), <P2> not read (<cause>)` |
| The profile root could not be listed | `the profile root could not be listed` |
| The 30-second limit ran out | `the reads ran out of time` |
| `swapped_at` is more than `SWAP_GRACE` later than the cycle's time | `the last swap time is later than now` |
| Same account as one profile `M` whose record names another account | `it holds the same account as profile <M>, which is not the account recorded for <M>; run amplihack profile verify` |
| Same as none, and `L`'s record names another account | `it holds an account other than the one recorded for <L>; run amplihack profile verify` |

Profile lists are sorted by name and joined with `, `. Notes hold profile
names and fixed text only. A note longer than the ledger's 1024-byte `guidance`
limit is cut at a character boundary before it is saved, so a long list of
profiles cannot make the ledger unreadable.

#### Only-login ALERT

```
ALERT: ~/.claude holds the same account as profile <M> on two readings (ledger names <L>), but <L> has no login of its own, so recording <M> would leave <L> with no login anywhere; nothing moved, no swap
fix 1: if a /login in ~/.claude replaced <L>'s login: close every claude session that uses ~/.claude, then run: mkdir -m 700 <dir> && mv -n <root>/<M>/.credentials.json <dir>/ && rm <ledger>; then run: amplihack profile login <L>
fix 2: if <L> and <M> are profiles of one account, run amplihack profile verify to see the account each folder holds, then remove one of the two profiles
```

The ALERT and its `fix` lines are written on every cycle while the suspicion
stays confirmed, and kept in the ledger's `guidance`. `fix 1` sets `M`'s copy
aside, so the login in `~/.claude` is `M`'s only login, and removes the
ledger; with no ledger, the next cycle finds the live profile as
[Rotate live profile](#rotate-live-profile) describes.

`profile login <L>` can run at once; there is no need to run a rotate
cycle first. Standalone `profile login` keeps its existing
[account match](#logins-in-claude): with no ledger, it treats `L` as live
only when `L` has no login and its `.claude.json` names the account in
`~/.claude.json`. If stale session data still names `L`, that standalone
check can refuse with `profile '<L>' is the rotator's live profile`.
After closing the sessions, run plain `claude` with no `CLAUDE_CONFIG_DIR`
and without `/login`, then quit so it updates its display account before
retrying `profile login <L>`.

Rotation and `profile switch` use trusted login-bound evidence instead.
With no ledger, a cycle can identify `M` as live only through the
[trusted ownership checks](#rotate-live-profile); it cannot identify `L`
or write a new ledger from stale session JSON. Missing, uncertain or
conflicting required evidence prevents movement and ledger relabeling.

#### profile verify

`amplihack profile verify [--fix]` audits which account each folder holds,
by the same [identity reading](#credential-identity) recovery uses: one
`GET /api/oauth/profile` per login with its own token, no refresh, no
`.claude.json`, no reset times. It was added for
[#1886](https://github.com/rysweet/amplihack-rs/issues/1886), to be run
once after the fix to find and undo profile names crossed by the
3 November misfires, and can be run at any time.

It prints one line for `~/.claude`, then one for each profile in name
order:

```
~/.claude (live ffarmer) -> ffarmer@mistt.com: ok
ffarmer -> no login
mmorrison -> no login
nnorth -> mmorrison@mistt.com: MISMATCH, recorded for mmorrison
```

The account is the e-mail the server reports, or its uuid when it reports
no e-mail. `~/.claude` is compared with the record of the profile the rotate
ledger names as live. The statuses are:

| Status | Meaning |
|--------|---------|
| `ok` | The folder holds the account recorded for it. |
| `MISMATCH, recorded for <names>` | The folder holds another account, recorded for those profiles (`recorded for no profile` when none). |
| `no record, recorded for <names>` | The profile has no record. [`profile seed-record`](#profile-seed-record) can add one. |
| `no live profile, recorded for <names>` | `~/.claude` when the ledger names no live profile. |
| `no login` | No `.credentials.json`; for the live profile this is normal after a swap. |
| `not an OAuth login` | An API-key file, or a file that is not private or cannot be parsed. |
| `not read (<reason>)` | The server did not answer for the token: one of the fixed [direct-read](#direct-read) reasons, such as `token expired` or `http 401`, or `folder refused` for a folder that fails the directory check. |
| `excluded (not probed)` | The profile is in the auto-switch `exclude` list. Its folder and login are not read and no request is made. Having no e-mail, the line is `<P>: excluded (not probed)`. |

Excluded profiles are not read at all. `verify` has no swap to protect, so
it makes no request for them, writes nothing to their credentials and never
refreshes their logins. The `~/.claude` line is not read either when local
files show that the live login belongs to an excluded profile, by the rules
under [Local proof of an excluded live login](#local-proof-of-an-excluded-live-login).
That line is then `~/.claude: excluded (not probed), live profile <P>`.

```
~/.claude: excluded (not probed), live profile shared
review -> review@example.com: ok
shared: excluded (not probed)
```

Excluded lines do not change the exit status: the user chose to exclude
those profiles. `--fix` leaves them out of its plan and never touches their
folders.

The exclude list is read from `<root>/.auto-switch` once per run. When that
file cannot be read, does not parse or is unsafe, `verify` prints the
`auto-switch config unreadable or unsafe` warning on stderr and treats every profile
as excluded; see [Unreadable exclude list](#unreadable-exclude-list).

Without `--fix` nothing is written. The command exits non-zero when any
folder holds an account other than its recorded one and is still there at
the end.

Do not run `--fix` until
[#1960](https://github.com/rysweet/amplihack-rs/issues/1960) is closed. It
writes no records, so it never fixes `no record`; use
[`profile seed-record`](#profile-seed-record) for that. The rest of this section
describes what the flag does.

With `--fix`, under the rotate lock (refused when another process holds it
for 2 seconds) and never while a swap journal exists, it moves a profile
folder's own login `P` into the folder of profile `T` only when all of
these hold:

- `P`'s record names another account, and exactly one other profile `T`
  has a record naming the account `P` holds;
- `T` is not the profile the ledger names as live (its login is in
  `~/.claude`);
- `T`'s folder holds no `.credentials.json` at all. A target that holds a
  different account, the same account, or a login that could not be read is
  refused: two copies of one login share a refresh token, so one of them
  must be chosen by hand;
- no other move in the same run goes to `T`.

Each other mismatch prints `<P>: not moved: <why>`. An excluded profile is
never a source or a target of a move; a move that would target one prints
`<P>: not moved: <T> is excluded; --fix does not touch it`. `~/.claude` is never
moved; a mismatch there prints `~/.claude: not moved by --fix; use amplihack
profile switch`. A move goes from one folder to an empty one, so a pair of
folders that hold each other's accounts is reported, not swapped; fix such a
pair by hand.

Before the first move, `--fix` writes
`<root>/.rejected-logins/verify-<YYYYMMDDTHHMMSSZ>/moves` (directory `0700`,
file `0600`, created fresh, never through a symlink). It lists each move as a
`# <P> -> <T>` comment followed by the `mv -n` command that undoes it. This
list is the backup. No login is copied, because a copy of a login is a
second holder of its refresh token. Just before each move, `P` is read
again and must still report the account surveyed, and `T` must still hold
no login file. The move itself is `renameat2(RENAME_NOREPLACE)`, so it never
replaces a file. The ledger and the records are not changed. The command
prints `moved <P>'s login to <T>` for each move.

Nothing printed holds a token. The e-mail addresses are printed because the
command is asked to show them; the rotator itself never prints or logs an
account.

#### Seeding a record

Swapping away from a profile needs a [record](#recorded-account) or other
ownership evidence. A live profile with no record gets one in one of two ways.
Both use one fresh [identity reading](#credential-identity) per profile. Neither
moves a login or writes a credentials file, and neither goes through
`profile verify --fix`.

| Path | Requires | Outcome |
|------|----------|---------|
| A rotate cycle, live profile only | The ledger's `login_identity` matches the reading in full, plus the shared rules below | Records the account and logs `seeded: recorded account for live profile <L>`. Rotation goes on. |
| A rotate cycle whose ledger has no `login_identity` | Cannot seed: an older key alone does not prove ownership | No record and no swap. Measuring goes on, and the note below names the command. |
| [`profile seed-record <name>`](#profile-seed-record) | The operator names the profile, plus the shared rules below | Shows the account. Writes the record only with `--write`. |

[`profile switch`](#rotate-switch) never seeds. Run `profile seed-record` first if
a switch is refused because the live profile has no record.

The shared rules are:

- The reading is trusted and has both an account uuid and an organization. An
  account with no organization is never seeded.
- The account and organization are printable values. A value with control
  characters or terminal escape sequences stops the seed.
- No swap journal exists, whether the rotate or the auto-switch one, and no
  hand-change suspicion is open. A journal that cannot be read counts as present.
- The profile has no record. A usable record is never overwritten, and an
  unusable one stops the seed until you repair it.
- A complete scan finds no other profile claiming that uuid and organization.
- `~/.claude` is compared too. For the live profile it is the reading itself. For
  any other profile it is read once and must hold a different account; the seed
  stops if it holds the same uuid and organization or if that read fails. A
  definitely missing login or an API-key login in `~/.claude` holds no account to
  compare.
- Just before the write, under the lock, the reading still matches the login
  and the scan's claims still hold. A preview made before the lock cannot justify
  a write.

**How the scan counts each other profile.** Seeding does not refresh tokens, so
one expired spare login must not block every seed. Swap authorization keeps its
stricter scan, which refuses on any failed read.

| Other profile | Seeding counts it as |
|---------------|----------------------|
| Has a usable record | A claim on its recorded account, whatever the rows below find. |
| Has an unusable record | Blocks the seed, naming the profile. |
| Its login file is definitely missing | No read and no claim from the login. Any other failure to check the file, such as permission denied, is not proof that it is missing and blocks the seed as `login file not trusted`. |
| Holds an API-key login | No read and no claim from the login. |
| Read succeeds | A claim on the account it holds. |
| Has a usable record, and the read failed with `token expired`, `http 401`, `http 429`, `timeout`, `transport` or `identity read budget used up` | Its record stands in for the read. |
| Has a usable record, and the read failed with any other cause, including `no oauth token`, any other `http` status and `unparseable` | Blocks the seed, naming the profile and cause. |
| Has no record, and the read failed with any cause | Blocks the seed, naming the profile and cause. |

**Two profiles with one e-mail.** Accounts are compared by uuid and
organization, never e-mail alone. `ryan-mistt-personal` and `ryan-mistt-org` can
share `you@example.com` and still be told apart when their organizations differ.
When they cannot be told apart, the cycle alerts and names them all, sorted:

```
ALERT: cannot tell profiles <L>, <M> apart: both claim the account in ~/.claude; no record written, nothing moved
```

This line is written on every cycle it applies to. It describes the live profile,
so only a cycle writes it; [`profile seed-record`](#profile-seed-record) has its
own refusal text.

**When a cycle cannot seed.** It logs this line once per process, and a real
cycle goes on to measure. A swap away from the profile is then refused with
`ALERT: swap refused: trusted login identity: outgoing ownership is not established; run: amplihack profile seed-record <L>`.
The note says why the seed stopped:

```
note: no record for live profile <L>: <reason>; nothing moved; run: amplihack profile seed-record <L>
```

| `<reason>` | Meaning |
|------------|---------|
| `the ledger has no login identity` | The ledger is from before login identities were kept, so it cannot prove ownership. |
| `identity read failed: <cause>` | The live login's [identity read](#identity-read-causes) failed. |
| `the reading has no account uuid or organization` | The server's answer cannot identify one account in one organization. |
| `the account cannot be printed safely` | A value holds control characters or terminal escape sequences. |
| `a swap journal is pending or unreadable` | Finish [recovery](#rotate-recovery) first. |
| `a hand-change suspicion is open` | Only `profile seed-record` reports this; a cycle writes the hand-change ALERT instead. See [Two readings](#two-readings). |
| `profile <P> not read (<cause>)` | The scan is blocked by that profile's failed read. |
| `profile <P> has an unusable record; repair it first` | That profile's record, or the live profile's own, stops the seed. |
| `the profile root could not be listed` | The scan cannot be complete. |
| `the ledger does not match the reading` | The ledger's identity names another account. |

`--dry-run` prints `would seed: record for live profile <L> from a fresh reading`
when a real cycle would seed, and otherwise the same note after `would note: `.
A dry-run `live=<L>` token comes from the ledger. It names the login for
reporting and authorizes nothing, so read the `would` line beside it to see
whether the rotator can swap away from `<L>`.

A record is a trust anchor: with one, swap authorization skips the strict
outgoing scan. That is why every doubt above stops the seed.

#### profile seed-record

```
amplihack profile seed-record <name> [--write]
```

`<name>` must be a valid profile name; the command takes no folder argument. It
reads `~/.claude` for the profile the ledger names as live, and
`<root>/<name>/.credentials.json` for any other profile. It never refreshes a
token, starts `claude`, moves a login or writes a credentials file.

Without `--write` it writes nothing:

```
$ amplihack profile seed-record ryan-mistt-personal
ryan-mistt-personal -> you@example.com, organization 00000000-0000-0000-0000-000000000000: no record; nothing written; run with --write to record it

$ amplihack profile seed-record ryan-mistt-personal --write
ryan-mistt-personal -> you@example.com, organization 00000000-0000-0000-0000-000000000000: recorded
```

With `--write` it takes the rotate lock first and makes every check under it.
A busy lock fails with `rotate: lock busy`, and the log gets
`seed-record: recorded account for profile <name>`, with no account in it.

The [shared rules](#seeding-a-record) apply, together with these:

| Situation | Result |
|-----------|--------|
| A usable record exists | `<name>: a usable record exists; nothing written`, exit 0. Also the result when another process writes one first. |
| `<name>`'s record is unusable | Refused: `profile <name> has an unusable record; repair it first` |
| The ledger exists but cannot be read | Refused, with or without `--write`: `the rotate ledger is unreadable` |
| `<name>` is not the ledger's live profile, or there is no ledger, and `~/.claude` holds the account the reading shows | Refused: `~/.claude holds this account, so the login may be live under another name` |
| The same, and the read of `~/.claude` failed | Refused: `~/.claude identity read failed: <cause>` |
| Another profile claims the account | Refused, with names sorted: `another profile claims this account: <P1>, <P2>` |
| `<name>` is the ledger's live profile, the ledger has a full `login_identity`, and the reading differs | Refused: `the ledger does not match the reading` |
| `<name>` is the ledger's live profile, the ledger has only an older key, and it is not the reading's uuid | Refused: `the ledger's older key does not match the reading` |
| `<name>` is the ledger's live profile, and the older key is an e-mail | Refused: `the ledger's older key is an e-mail, which cannot prove ownership` |
| `<name>` is not the ledger's live profile, and no rule refuses | Allowed. The folder name is the only evidence tying the profile to its login, so read the account shown before using `--write`. |
| An account value is unsafe to print: empty, over 320 bytes, or holding a control or bidirectional-override character (U+202A–U+202E, U+2066–U+2069) | It prints `<unprintable>`, and `--write` is refused: `the account cannot be printed safely` |
| The token has expired | Refused: `identity read failed: token expired`. Run [`profile usage <name>`](#usage-command) to refresh a non-live profile. The next rotator cycle refreshes the live login. |

A refusal prints `seed-record <name>: refused: <reason>; nothing written` and exits
1. It writes no ALERT. Other `<reason>` values, such as a pending journal or a
blocked scan, are those in the [cycle's table](#seeding-a-record), except
`the ledger has no login identity`, which only a cycle reports.

The e-mail and organization go to stdout only, never to the rotate log, the
ledger or an alert.

The record is created without replacing anything: a private temporary file
(`0600`, no symlink followed) is linked to the final name, so a record that
already exists stays as it was. A filesystem that refuses hard links gives a
named error and no record; the command does not fall back to a rename.

Once the record is linked, it counts as written. If removing the temporary name
or syncing the directory then fails, that is logged as a warning. If the audit
line cannot be written, stderr gets
`seed-record <name>: recorded, but the audit log could not be written: <error>`,
and the command still reports the record and exits 0.

**A wrong record.** Seeding a non-live profile whose folder holds another
account's login, when nothing else claims that account, records the wrong
account. To remove it, run `rm <root>/.auto-switch.d/expected/<name>.json`.
An accepted [`profile login <name>`](#login-check) also replaces it.

#### Guidance

When a confirmed move fails a check, nothing moves, the cycle writes an
ALERT line and one `fix` line, ends as `alerted` and does not swap:

```
ALERT: the account in ~/.claude is not the one the ledger names (live=<L>); profile <M> holds the same account and has its own login; recovery refused: <reason>; no swap
```

The `fix` line for each `<reason>`:

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

`oauthAccount` is display state. A missing, incomplete or stale account
block does not authorize movement, invalidate trusted identity or start
hand-change recovery. The swap builds display details from trusted
identity and preserves unrelated JSON fields and file modes.

Safe file handling still applies. A symlink, unsafe permissions,
unreadable or malformed JSON, or a non-object can prevent a safe display
update and refuse the swap before movement. Resolve that reported file
problem; do not move a login or run `/login` merely because the display
account is stale. The serialized ledger remains compatible with existing
`home_unreadable_since` fields; such a timestamp is not ownership evidence.


Some recovery diagnostics retain their earlier display-file wording. These
messages describe file safety or older retained guidance; they never make
session account values proof of login ownership. A stale or missing
`oauthAccount` alone does not trigger recovery. Resolve the actual reported
file problem and trusted-identity refusal before following recovery guidance.
The retained message grammar is:

```text
ALERT: the account in ~/.claude.json cannot be read (live=<L>); no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace <L>'s login
ALERT: ~/.claude.json has had no account that can be read for 5 minutes or more (live=<L>): <problem>; no swap until it is fixed; do not /login in ~/.claude before the login there is set aside, it would replace <L>'s login
fix 1: run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if <home>/.claude.json then holds an account, the next cycle goes on
fix 2: or give <L> its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e <root>/<L>/.credentials.json && mv -n <home>/.claude/.credentials.json <root>/<L>/ && rm <ledger>; then run claude with no CLAUDE_CONFIG_DIR and /login
```

Older guidance can include `it has no oauthAccount block`. That display
condition cannot establish which account the moving credential belongs to.

The retained file-problem labels are `it does not exist`,
`it cannot be read (a symlink, not a regular file, over 32 MiB or not readable)`,
`it is not valid JSON (empty or cut short)`, `it is not a JSON object`, and
`its oauthAccount block has no usable accountUuid or emailAddress`.
These labels describe display metadata and never replace trusted identity.

Separately, every cycle writes, for each profile in the ledger's
`lost_login`:

```
ALERT: profile <L> has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login <L>
```

Recovery no longer adds profiles to `lost_login`: the
[only-login rule](#only-login-rule) refuses any move that would leave a
profile with no login. Entries written by an earlier release are still
reported. A profile leaves `lost_login` when
`<root>/<L>/.credentials.json` exists again or `<root>/<L>` is no longer a
real directory (it was removed, or replaced by a symlink). `lost_login` holds at most 64 profiles; the line

```
ALERT: profile <L> has no login anywhere, and the list of such profiles is full, so this is not repeated. Run: amplihack profile login <L>
```

is written once for a profile an earlier release could not add.

ALERT, `fix` and lost-login lines are written on every cycle while their
condition holds, at most once each per cycle, without the
[ALERT key](#rotate-log). Notes are written once until they change. The
one-shot command prints all of them on every run. The ledger's `guidance`
holds the current note or ALERT and `fix` lines and is cleared when the
finding is Clear; [`status`](#rotate-status) prints them.

`--dry-run` makes the same identity reads, without the lock and without
[refreshing](#an-expired-live-login), and applies the
same rules to the ledger's `hand_suspect` as it stands. It writes nothing,
so it never starts or confirms a suspicion:

| Ledger and readings | Dry-run line |
|---------------------|--------------|
| `hand_suspect` names `M`, its interval has passed, the reading is "Suspect `M`", and the checks pass | `would recover: <recovered line>`; then it stops before measuring |
| The same, but the only-login rule or a check refuses | `would alert: <ALERT line>`, followed by its `fix` lines; then it stops with outcome `alerted` |
| A suspicion or cannot-tell finding | `would note: <note>`, without the `note: ` prefix; a suspicion then stops with outcome `alerted`, a cannot-tell finding goes on to measure |
| Within `SWAP_GRACE`, or any other gate in [When recovery reads](#when-recovery-reads) | Nothing; no request is made |
| The live login has expired | `would refresh: expired login in ~/.claude`, before any other line of this table |
| The live profile has no record | `would seed: ...` or `would note: no record for live profile ...`; see [Seeding a record](#seeding-a-record) |
| The ledger exists but cannot be read | `would alert: rotate ledger unreadable; ...`; see [Unreadable-ledger stop](#unreadable-ledger-stop) |

`<recovered line>` and `<ALERT line>` are the log text without its
`recovered: ` or `ALERT: ` prefix. A dry run does not go on to the
[live profile](#rotate-live-profile) table after a `would alert:` line, so it
never prints a second `would alert:` line for the same finding. These
recovery identity reads send each login's token to the OAuth profile
endpoint (`GET /api/oauth/profile`), as the real cycle does. Usage
measurement is a separate read.

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
   | `directory refused` | `its directory is a symlink, not owned by you, or open to group or other users` |
   | `no login`, file missing | The whole message is ``profile <name> has no stored login; run `amplihack profile login <name>` first``. |
   | `no login`, file present | `a login file is not a private regular file with one link` |
   | `excluded` | ``it is in the auto-switch exclude list; set the list without it with `amplihack profile auto-switch exclude` first`` |
   | `session running` | `a running process has CLAUDE_CONFIG_DIR set to it; end that session first` |
   | `session running`, `/proc` cannot be scanned | `running sessions cannot be seen here, so its login may be in use`. Every profile but the live one is refused, as every profile is skipped by the rotator. This is every system but Linux, macOS included: there `switch` only sets the default for new launches. |
   | `leased` | `auto-switch has lent its login to another profile's session` |
   | `hosting` | `its directory holds another account's login for auto-switch` |
   | `parked` | `auto-switch has parked its own login` |
   | `no account` | Its login has no usable trusted identity for the required ownership check. The message includes the [cause](#identity-read-causes). |

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
   [usage-limit wait](#usage-limit-wait) rerun steps that stopped on a
   usage limit before the switch; see
   [Sessions stopped by a limit](#sessions-stopped-by-a-limit). No
   [swap record](#swap-record) is written.
10. Release the lock, then check the
    [shared session links](#shared-session-directories) of `<name>` and of
    the profile swapped out. Problems there are warnings on stderr only.

`switch` reads none of the `AMPLIHACK_ROTATE_*` [settings](#rotate-settings)
and never runs `claude`. It makes no usage measurement, but it does make
the bounded credential identity reads needed by the shared swap guards.
It does not look for a `claude` binary either, so it works with no `claude`
on `PATH`. A login with no capacity left is still moved: `switch`
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
[usage-limit wait](#usage-limit-wait) does not treat it as capacity,
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

A `switch` counts as capacity for the [usage-limit wait](#usage-limit-wait):
you chose that login, so the switch stands in for a measurement taken at the
moment of the switch, and a recipe step that stopped on a usage limit before
the switch reruns. This holds for every held cycle that follows, because a
held cycle keeps the `switched` record (see [Rotate pin](#rotate-pin)).

A `switch` does not resume interactive sessions. It writes no
[swap record](#swap-record), so the
[limit-resume hook](#rotate-session-resume) keeps waiting, and the held
cycles that follow swap nothing, so they write none either. Continue such a
session yourself (Esc, Ctrl+U, `continue`, Enter), or run
`switch --none` and let the rotator's next swap to a login with room resume
it.

#### Warning about an older rotator

After the success lines, `switch` checks whether a running rotator was
started from another amplihack binary, the same check `amplihack install`
and one-shot `rotate` make (see
[Rotator capacity rule](#rotator-capacity-rule)). Such a rotator does not
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

A held cycle clears the stored ALERT key (`alert` and `advisory_alert`),
as a `stayed` cycle does, so the same ALERT is written again if it is still
true after the pin is released. A `stayed-unread` cycle does not clear it.

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
| `ledger.json` | `live`, `account`, `since` (epoch seconds when `live` last changed), `swapped_at`, `previous`, `alert` (the de-duplication key, `<kind>:<live profile or ->`, of a hard ALERT), `advisory_alert` (the same key for an [advisory ALERT](#rotate-advisory-alert); never set together with `alert`), `last_cycle` (`at`, when the cycle was recorded; `measured_at`, when the usage it relied on was read, which for `stayed` is when the live reading was taken (see [Stays and capacity](#rotate-stays-and-capacity)) and for every other rotate outcome is when the cycle started measuring, absent for a cycle that measured nothing; `outcome`; and, for an [overflow](#rotate-overflow) cycle, `overflow`), and `overflow_blocked` (the [spend blocks](#overflow-spend-limit-block)). `account` is the validated incoming account key after a swap. It is display only for recovery: no recovery decision treats that key alone as identity proof, and a hand-change recovery records its independently verified account. Optionally `hand_suspect`, an open [hand-change suspicion](#two-readings): `profile` (a valid profile name) and `first_seen` (epoch seconds, greater than 0). It holds no account data, is omitted when there is none, and a ledger without it loads as before; one that breaks these rules is `RotateInconsistent`. Optionally `guidance`, the current [hand-change](#rotate-hand-change-recovery) note, or ALERT and `fix` lines (at most 16, each at most 1024 bytes, with no control or bidirectional-override characters), and `lost_login`, the profiles that lost their login to a manual `/login` (at most 64 distinct valid names). Both are omitted when empty, and a ledger that breaks these limits is `RotateInconsistent`. Optionally the legacy `home_unreadable_since` timestamp; retained for format compatibility, never as ownership evidence (see [Stuck `~/.claude.json`](#stuck-claudejson)). Optionally `swapped_away`, the logins the rotator left while they were full, each mapped to the epoch seconds of that swap; see [Swap-back damping](#swap-back-damping). While a rotator from an older amplihack binary runs, `measured_at`, `overflow`, `overflow_blocked`, `guidance`, `lost_login`, `home_unreadable_since`, `hand_suspect`, `swapped_away` and `advisory_alert` are left out so that rotator can read the file. A ledger whose `alert` or `advisory_alert` is not a valid key, or that sets both, is `RotateInconsistent`. |
| `journal.json` | The swap in progress: `from`, `to`, `adopt`, `phase` (`checking`, `j0`, `s1`, `s2`, `s3`, `s4` or `inconsistent`), `home`, `live_ino`, `target_ino`, `home_lock_ino` and `to_lock_ino` (each present once that refresh lock is taken), `started_at`, `pid`, and optional `identity` containing sanitized full incoming/outgoing accounts authorized before exchange. |
| `rotate.pid` | The background process's pid, as ASCII decimal. |
| `gate.json` | Written by `rotate watch` when it starts: `version`, `pid`, and the [overflow](#rotate-overflow) gate it resolved from its flags (`overflow_wait`, `overflow_unknown_reset`, `session_at`, `week_at`). [Auto-switch](#auto-switch-and-overflow-profiles) uses it only while the rotator with that pid runs. If it cannot be written, the log says `overflow: gate not recorded, auto-switch will not use overflow (<error>)` and auto-switch uses no overflow profile. |
| `rotate.log` | The log. |
| `rotate.log.1` | The previous log, after rotation. |
| `rotate.err` | The background process's own stdout and stderr, and one line from each launch that [started](#rotate-autostart) a rotator or failed to. Not rotated. |
| `autostart` | The [autostart](#rotate-autostart) setting: `on` or `off` and a newline. Missing means on. |
| `swap.json` | The [swap record](#swap-record): `seq`, `live_login`, `from_login`, `swapped_at` and `reading` (`session`, `week`, `measured_at`). Written atomically, mode `0600`, under the rotate lock, only after a successful swap in a rotate cycle. Read by the [limit-resume hook](#rotate-session-resume). Not part of the ledger; deleting it is safe, and the next swap writes `seq` 1. |
| `pin` | The [rotation pin](#rotate-pin): one profile name and a newline. Written by `profile switch <name>`, removed by `profile switch --none`. Not part of the ledger. |
| `sessions-archive.last` | When the [daily session archive](#rotate-daily-session-archive) last started: epoch seconds and a newline. Written atomically before each run; removed when a run stops early after archiving something, so the next pass carries on. |
| `owner` | The [owner marker](#rotate-owner): `unit=`, `pid=` and `since=` lines. Written by a supervised `rotate watch`, removed only by `rotate owner clear`. Not part of the ledger. |

The auto-switch `ledger.json` and `journal.json` one level up are separate
files and are not changed by rotation.

[Hand-change recovery](#rotate-hand-change-recovery) also writes outside this
directory:

| Path | Contents |
|------|----------|
| `<root>/.rejected-logins/rotate-<name>-<YYYYMMDDTHHMMSSZ>[-<n>]/` | `0700`. The `.credentials.json` moved out of profile `<name>` because `~/.claude` and `<name>` answered as the same account on two readings. Never deleted by amplihack. An empty one is left by a recovery that stopped before the move. |

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
stdout only, without the time. `--dry-run` makes the same re-reads as a real
run and prints their `reread:` lines; see
[Re-reads](#re-reads) for the `damp:` lines it prints.

| Message | Meaning |
|---------|---------|
| `cycle live=<name\|-> <name>:s=<n>%,w=<n>% ...` | One per cycle: every measured profile, live first, then by name. Percentages are rounded down and come from the windows read. An exhausted profile (`session_exhausted` or `weekly_exhausted`) shows the figures read, with the window that ran out never below `100%`, for example `team:s=100%,w=55%` or `team:s=30%,w=100%`; see [Exhausted figures](#rotate-exhausted-figures). `w=?` means no weekly window was read. A profile whose probe timed out or could not run shows `<name>:failed`, never a percentage: its usage is unknown, and it is probed again on the next cycle. A profile whose read had not finished when the cycle stopped waiting shows `<name>:unknown, still running`; its result is used by the next cycle. `spend_limit`, `not_logged_in` and `unparsed` also show `<name>:failed`. A profile with [no session limit](#no-session-limit) shows `s=0%`. A skipped profile is not listed. A live profile in the auto-switch `exclude` list shows `<name>:excluded (not probed)`; see [Rotate excluded live profile](#rotate-excluded-live-profile). A login no profile owns is shown as `-`. An API-key profile is never probed. A marked one shows as `<name>:per-token`, live or not. An unmarked one is listed, as `<name>:per-token`, only while it is live; otherwise it is not listed. |
| `swap <from>-><to> live s=<n>% w=<n>% target s=<n>% w=<n>% reason=threshold\|projected\|upgrade\|failed-probe\|plan-room\|overflow-in\|overflow-out` | A completed swap. The readings are formatted as in the `cycle` line, with a space in place of the comma, so an exhausted target can show `target s=100% w=55%`. For an adopted login, `<from>` is the new profile's name. A failed live probe shows `live failed`, one still running shows `live unknown, still running`, and an excluded live profile shows `live excluded (not probed)`. `reason=failed-probe` is a swap made after three consecutive failed live probes; see [Failed live probes](#rotate-failed-live-probes). `reason=projected` is a swap made because the live login's [projected session use](#projected-swap) reached `--session-at`, or because its last known reading was near `--session-at` when the live read failed or was stale and the stale reading would not itself have needed a swap (see [Failed or stale reading near the threshold](#failed-or-stale-reading-near-the-threshold)); the `live` figures are those measured. `reason=plan-room` is a [plan-room swap](#rotate-plan-room-swap). `overflow-in` and `overflow-out` are swaps onto and off an [overflow](#rotate-overflow) profile. |
| `per-token <name>: no measured plan login below 100%, 0 not measured; <gate>; marked overflow tier <N>; <check>` | Written before the `swap` line of every move onto a per-token profile, credit or API key, by the rotator or by auto-switch. Auto-switch writes it before it swaps a session's login or starts a launch or a rerun on the profile, also prints it on stderr, and ends the line with `; auto-switch from <profile>`; the rotate log is created if no rotator has run. `<gate>` is `earliest reset <time> beyond overflow_wait`, with `--overflow-unknown-reset 1` `reset unknown`, or `leaving the API key, credits cost less` for a move off the API key, which passes no gate. `<N>` is the profile's [tier](#overflow-tiers), `10` for an API-key profile. `<check>` is `login check passed (<detail>)` for an API-key profile, with `<detail>` from [API-key login check](#api-key-login-check), or `probe ok, no spend limit` for a credit profile the rotator probed. Auto-switch runs no probe, so for a credit profile `<check>` is `no spend block`. The line never holds a key value. |
| `key_check_refused <name>: <reason>` | The [API-key login check](#api-key-login-check) failed for the target `<name>`; no swap was made. |
| `ALERT: home_key (<detail>): <effect>` | A key is in `~/.claude.json` or `~/.claude/settings.json` while a plan login is live, so sessions bill per token. Written once, and again if the setting changes; see [API-key login check](#api-key-login-check). The cycle carries on. |
| `overflow: on <name>; no measured plan login below 100%, next plan reset <time\|unknown>` | Written after an `overflow-in` swap. |
| `overflow: staying on <name>; no measured plan login below 100%, next plan reset <time\|unknown>` | A cycle stayed on overflow profile `<name>` and the previous recorded cycle did not: its `last_cycle.overflow` was absent or `out`, or the ledger's `live` named another profile. So it is written once when the rotator finds itself on credits without having moved there, for example after a login was put in `~/.claude` by hand, and not again while it stays. After `overflow-in` the `overflow: on` line is written instead. |
| `overflow: off <name>; back on plan profile <name>` | Written after an `overflow-out` swap. |
| `ALERT: overflow profile <name> hit its spend limit; blocked until <time>` | A [spend block](#overflow-spend-limit-block) was recorded. Written once per block. |
| `ALERT: overflow profile <name> hit its spend limit; not blocked (too many blocks\|no end time)` | A spend-limit reading that could not be stored as a block: the ledger already holds 256 active blocks, or the cycle's time is outside the range for which the next month can be computed. The profile is still not used in that cycle; the ledger is left as it was, so it still loads. |
| `stay: live probe failed (<k>/3)` | The live probe failed for the `k`th cycle in a row; no swap yet. |
| `stay: live probe failed; held after last swap (<N>s left)` | The live probe has failed three or more times in a row, but the last swap was less than `--hold` seconds ago. |
| `reread: <name> reading stale s=<n>% w=<n>%; fresh s=<n>% w=<n>%` | The chosen swap target's reading failed the [freshness gate](#rotate-reading-freshness-and-swap-back-damping) and was read again by the [probe method](#probe-method), with passive readings off. The first figures are those the decision used, the second those of the new read, formatted as in the `swap` line (`w=?` when no weekly window was read). The decision was then made again on the new figures. |
| `stale live reading; re-read` | The live login's reading was [frozen, old or lagging](#stale-live-reading) and was read again by the configured probe method, with passive readings off. A direct read uses the supplied home's live credentials and never refreshes them. Written at most once per cycle, before the decision. The line holds no name or figures. |
| `reread: <name> failed; treated as unknown` | The re-read timed out or failed, or the re-read returned a reading that is itself still stale. `<name>` is unknown for this cycle: not chosen, not recorded as exhausted, and counted in `F` of the no-capacity ALERT's suffix. After a [`stale live reading; re-read`](#stale-live-reading) line, `<name>` is the live login: its re-read failed or was still stale, and the cycle is decided by the [near-threshold rule](#failed-or-stale-reading-near-the-threshold). |
| `reread: <name> limit reached; treated as unknown` | The chosen target was stale, but this cycle had already made 3 target re-reads. The separate live re-read does not count toward this limit. `<name>` is unknown for this cycle. |
| `reread: <name> time limit reached; treated as unknown` | The chosen target was stale, but this cycle's target re-reads had already used their shared time limit of one [probe deadline](#probe-deadline). The live re-read has a separate budget. `<name>` is not read and is unknown for this cycle. |
| `damp: <name> swapped away exhausted; no later reading shows room` | `<name>` is in the ledger's `swapped_away`, and its re-read shows it still at or above `--session-at` or `--week-at`, or exhausted. It stays damped. See [Swap-back damping](#swap-back-damping). |
| `damp: <name> later reading shows room; swap-back allowed` | A reading of `<name>` taken after the rotator swapped away from it shows room, so its `swapped_away` entry was removed. Written in the [record step](#rotate-cycle), after the cycle's `swap`, stay or ALERT line, and never by `--dry-run`. |
| `ALERT: no profile has capacity live=<name> s=<n>% w=<n>%` / `ALERT: no profile has capacity live=<name> s=<n>% w=<n>% (candidates: <F> failed, <E> exhausted)` | A threshold is reached and no candidate meets the margin. The suffix is added only when `F + E > 0`: `F` candidates had a failed probe and `E` were exhausted, whatever figures each exhausted candidate read. When every candidate failed, the probes are at fault rather than the accounts. The suffix holds no profile names, and the ALERT key stays `no-capacity`, so a change in the counts does not repeat the ALERT. `--dry-run` prints it as `would alert: no profile has capacity live=<name> s=<n>% w=<n>% (candidates: <F> failed, <E> exhausted)`, without the `ALERT: ` prefix, and with the same rule for the suffix. |
| `ALERT: live profile <name> is excluded; no eligible candidate` | ALERT `excluded-live`: the live profile is in the auto-switch `exclude` list and no move off it is possible. The login stays in `~/.claude`, unprobed and unrefreshed. Written once per key. See [Rotate excluded live profile](#rotate-excluded-live-profile). |
| `note: live profile <name> is excluded; moving off it` | Written before the `swap` line of a move off an excluded live profile. |
| `refresh: live profile <name> is excluded; token not refreshed` | The [expired-login refresh](#an-expired-live-login) was skipped because a rule under [Local proof of an excluded live login](#local-proof-of-an-excluded-live-login) named `<name>` as the live profile. |
| `skip: pinned profile <name> is excluded; spend not probed` | A held cycle did not probe the spend of a pinned excluded profile; see [Pin and overflow](#pin-and-overflow). |
| `warning: auto-switch config invalid; exclude list applied, other settings ignored` | `<root>/.auto-switch` is a private file of yours that parses but is invalid for its other settings (an unknown kind, say); its exclude list still applies. A file others can write never gets this warning. Written once per cycle. See [Unreadable exclude list](#unreadable-exclude-list). |
| `warning: auto-switch config unreadable or unsafe; every profile treated as excluded, none probed or refreshed` | `<root>/.auto-switch` exists but cannot be read, does not parse, or is owned by another user or writable by group or others. Followed by the `rotation paused` ALERT. See [Unreadable exclude list](#unreadable-exclude-list). |
| `ALERT: auto-switch config unreadable or unsafe; rotation paused, no profile probed` | The cycle stopped before measuring because the exclude list could not be read or trusted. Written every cycle until the file is fixed. |
| `ALERT: no login in ~/.claude; no swap` | `~/.claude/.credentials.json` is missing or not a regular file. |
| `ALERT: the account in ~/.claude is not the one the ledger names (live=<name>); no swap` | ALERT `hand-change`: a [hand-change suspicion](#two-readings) is open, or recovery refused a confirmed one. Written on each affected cycle while the identity refusal persists. |
| `ALERT: the account in ~/.claude is not the one the ledger names (live=<name>); profile <M> holds the same account and has its own login; recovery refused: <reason>; no swap` | A confirmed hand change whose move failed a [check](#checks-before-the-move). Followed by its `fix 1:` line. Written every cycle. |
| `ALERT: ~/.claude holds the same account as profile <M> on two readings (ledger names <L>), but <L> has no login of its own, so recording <M> would leave <L> with no login anywhere; nothing moved, no swap` | The [only-login ALERT](#only-login-alert). Followed by its two `fix` lines. Written every cycle. |
| `fix <n>: <command>` | A step of the guidance above, in order. |
| `note: ~/.claude holds the same account as profile <M> (ledger names <L>); nothing moved, the next cycle reads again` | The first of the [two readings](#two-readings). Written once until it changes. |
| `note: cannot tell whose login is in ~/.claude (live=<L>): <why>; nothing moved` | Recovery could not tell; see [Hand-change notes](#hand-change-notes) for each `<why>`. Written once until it changes. |
| `recovered: ~/.claude and <M> held the same account; kept ~/.claude, moved <M>'s copy to <path>; recorded <M> as live` | Hand-change recovery confirmed on two readings that `~/.claude` and `M` are the same account, and moved `M`'s copy to a backup. |
| `ALERT: profile <L> has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login <L>` | `L` is in the ledger's `lost_login`. Written every cycle. |
| `ALERT: profile <L> has no login anywhere, and the list of such profiles is full, so this is not repeated. Run: amplihack profile login <L>` | `L` lost its login under an earlier release but `lost_login` already held 64 profiles. Written once. |
| `ALERT: more than one profile matches the account in ~/.claude; no swap` | Trusted ownership evidence cannot select a unique login-less profile. |
| `ALERT: trusted login identity unavailable: <cause>` | The live login has no usable identity reading, and the [cause](#identity-read-causes) says why. Nothing moves. |
| `ALERT: trusted login identity unavailable for profile <name>: <cause>` | A candidate is skipped as `no account`, for that [cause](#identity-read-causes). Written on every cycle it applies to. |
| `ALERT: rotate ledger unreadable[ (<field>)]; live identity: <readable\|cause>; nothing moved, no record written` | Written on every cycle it applies to. See [Unreadable-ledger stop](#unreadable-ledger-stop). |
| `ALERT: cannot tell profiles <L>, <M> apart: both claim the account in ~/.claude; no record written, nothing moved` | Written on every cycle it applies to. See [Seeding a record](#seeding-a-record). |
| `seeded: recorded account for live profile <L>` | A cycle [seeded](#seeding-a-record) the live profile's record. |
| `note: no record for live profile <L>: <reason>; nothing moved; run: amplihack profile seed-record <L>` | A cycle could not seed. Written once per process. |
| `seed-record: recorded account for profile <name>` | [`profile seed-record --write`](#profile-seed-record) wrote a record. |
| `ALERT: swap refused: <reason>` | A [pre-swap check](#rotate-swap) failed. Missing, uncertain or conflicting required trusted identity produces an ALERT on every affected cycle; other refusals are written once per reason. The one-shot command also prints it and exits with `RotateRefused`. |
| `ALERT: swap refused: the login in ~/.claude is not the account recorded for profile <from>` | The trusted live login conflicts with `<from>`'s recorded account. Nothing moves or relabels. Reported on every affected cycle and manual attempt until resolved. The persisted alert key is `account_mismatch:<from>`. Session JSON cannot cause or clear this refusal. See [Recorded account](#recorded-account) and [Rotation login identity](#rotation-login-identity). |
| `ALERT: rotate recovery is inconsistent: <reason>; no swap until it is resolved` | See [Rotate recovery](#rotate-recovery). |
| `ALERT: auto-switch recovery is inconsistent; no swap` | Auto-switch state needs `auto-switch repair`. |
| `recovery: rolled back an interrupted swap <from>-><to>` / `recovery: finished an interrupted swap <from>-><to>` | Recovery acted. |
| `skip: state lock busy` / `skip: <name> (<reason>)` / `skip: cannot see running sessions; no candidates` | The cycle or a candidate was skipped. |
| `skip: state changed during measurement (<labels>)` | Step 6 found a change that affects the decision; see [Rotate revalidation](#rotate-revalidation). `<labels>` lists one or more of `journal`, `live`, `pin`, `overflow`, `target eligibility: <reason>` and `target identity`, separated by `, `. The labels are fixed text and [skip reasons](#rotate-candidates) only. Older readers that match the prefix `skip: state changed during measurement` still match. The bare form, with no labels, is also written when nothing in the snapshot changed but the target's reading went stale under the lock and no fresh target remains. |
| `revalidate: target <name> no longer eligible (<reason>); reconsidered` | The swap target became ineligible, or its account changed, while the cycle measured, and the cycle decided again under the lock. `<reason>` is the target's bare [skip reason](#rotate-candidates), such as `session running`, `gone`, or `account changed`, or `stale reading` when the snapshot is unchanged and the target's reading went stale under the lock; it never begins with `target eligibility: `. The next line is the new `swap` line, or `ALERT: swap refused: <reason>` when a [pre-swap check](#rotate-swap) refuses the new target. |
| `revalidate: ~/.claude.json held no account; read again` | Retained diagnostic wording for the bounded retry of trusted credential identity. Session JSON supplies no identity evidence. |
| `skip: the external rotator is running` / `skip: <error>` | `watch` only: the cycle failed with that error and the rotator carries on. |
| `skip: prototype ledger ignored (<reason>)` | See [Rotate live profile](#rotate-live-profile). Written only when the reason changes. |
| `stop: signal received; cycle abandoned` | `watch` stopped during measurement or before the swap; nothing was moved. |
| `idle: auto-switch is off` | `watch` with auto-switch turned off; written once per change. |
| `ALERT: auto-switch is off while the live login is exhausted (<profile>: <reason>)` | `watch` with auto-switch turned off, while the live login has no capacity left; see [Exhausted while off](#exhausted-while-off). Written on every cycle while the condition holds, and does not change what the cycle does. |
| `skip: alert check failed: <error>` | `watch` with auto-switch turned off, when a ledger or `usage.json` cannot be read, so the [exhausted-while-off](#exhausted-while-off) check cannot run. Written on every paused cycle until the file loads again. |
| `change: <setting> <old> -> <new> by pid=<p> ppid=<pp> parent="<command>"[ recipe_run=<id>][ invocation=<id>]` | The autostart setting or the auto-switch toggle was changed. See [Rotate change log](#rotate-change-log). |
| `skip: owner marker not written (<error>)` | A supervised `watch` could not write the [owner marker](#rotate-owner). It carries on without one. |
| ``skip: held by switch to <name>; run `amplihack profile switch --none` to release`` | The [pin](#rotate-pin) holds rotation; the cycle neither measured nor swapped. |
| ``skip: pin unreadable; run `amplihack profile switch --none` `` | The pin file cannot be trusted or parsed; the cycle holds. |
| `skip: pin names missing profile <name>; ignored` | The pin names a removed profile; the cycle runs normally. |
| `sessions archive: <summary>` | The [daily session archive](#rotate-daily-session-archive) ran; `<summary>` is the [`sessions archive` summary line](./sessions-command.md#output), followed by the reason if the run stopped early (the 60-second limit, `rotate stop` or a signal). |
| `sessions archive: cannot read sessions.daily_archive (<why>); not run` | The user config file cannot be used to decide whether the daily run is on, so it does not run; see [turning the daily run off](#turning-the-daily-run-off). |
| `sessions archive: cannot read <key> (<why>); not run` | `<key>` is `sessions.archive_days` or `sessions.archive_dest`, and the user config file cannot give it a value, so the run does not fall back to the default; see [`sessions` configuration](./sessions-command.md#configuration). |
| `sessions archive: <error>` | The daily session archive did not run, or stopped; `<error>` is one of the [`sessions` messages](./sessions-command.md#messages) or the lock line. |
| `switch <from>-><to>` | A swap made by [`profile switch`](#rotate-switch). It has no usage readings, because `switch` measures no usage, and it does not start with `swap `, so a search for rotator swaps does not match it. |
| `skip: <name> is already the live login; nothing moved` | `profile switch` found `<name>` already live. |
| `would refresh: expired login in ~/.claude` / `would seed: record for live profile <L> from a fresh reading` / `would note: no record for live profile <L>: <reason>; nothing moved; run: amplihack profile seed-record <L>` / `would swap ...` / `would block <name> (spend limit) until <time>` / `would adopt ~/.claude as <name>` / `would stay` / `would stay (live probe failed (1/3))` / `would stay (held by switch to <name>)` / `would stay (pin unreadable)` / `would alert: ...` / `would recover an interrupted swap (phase <phase>)` / `would alert: rotate recovery is inconsistent` / `would alert: the rotate journal is damaged` / `would recover: <recovered line>` / `would_refuse <name>: <reason>` | `--dry-run` only. `would_refuse` is printed in place of `would swap` when the target is an API-key profile that fails its [login check](#api-key-login-check). |

Identity refusals write an ALERT on every affected rotation cycle;
manual switches report the refusal on every attempt. Other ALERTs are
written only when their key (`no-capacity`, `no-login`,
`hand-change`, `home-unreadable`, `ambiguous`, `inconsistent`, `auto-switch` or `refused <reason>`
for a swap refused under [Swap](#rotate-swap), plus the live
profile) differs from the key
stored in the ledger. The key is cleared when a cycle stays without alerting
or a swap succeeds, so the same ALERT can be written again later. The
one-shot `rotate` still prints a repeated ALERT to stdout on every run; only
the unrelated log line is written once. Hand-change notes have no key
either; each is written once
until its text changes. The hand-change guidance and lost-login ALERTs
have no key: they are written on every cycle while their condition holds,
and so has the [exhausted-while-off](#exhausted-while-off) ALERT.

Before each write, a log over 1 MiB is renamed to `rotate.log.1`, replacing
the old one. The log is opened with `O_APPEND`, `O_NOFOLLOW` and `O_CLOEXEC`,
mode `0600`. A log that is not a regular file, is owned by another user, or is
readable or writable by group or others is not written to; a warning goes to
stderr and its mode is not changed.

Log lines and stdout contain only profile names (with control characters
removed), percentages, times, phases, paths under `HOME` and the root, and
fixed text. They never contain
account IDs, e-mail addresses, tokens, environment values or Claude's output.
The one exception is the redacted parent command line and the run ids in a
[`change:` line](#rotate-change-log).

### Rotate change log

Every change to the [autostart setting](#autostart-setting) or to the
auto-switch toggle appends one line to `rotate.log`, so that a log shows who
turned rotation off and when:

```
2026-11-01T09:14:02Z change: auto-switch on -> off by pid=51877 ppid=51702 parent="bash /home/you/bin/pause-rotation.sh" recipe_run=3f9c2a1e-6b7d-4c1e-9a52-0d8e7b6f4a21
2026-11-01T09:20:45Z change: rotate-autostart on -> off by pid=52011 ppid=4120 parent="-bash"
```

| Field | Value |
|-------|-------|
| `<setting>` | `rotate-autostart` for `rotate autostart on\|off`; `auto-switch` for any command that changes `enabled` in `<root>/.auto-switch` (`auto-switch on`, `auto-switch off`). |
| `<old>` | `on` or `off`; `unset` when there was no setting; `corrupt` when the setting could not be trusted. |
| `<new>` | `on` or `off`. |
| `pid`, `ppid` | The process that wrote the setting, and its parent. |
| `parent` | The parent's command line, read from `/proc/<ppid>/cmdline`, with the arguments joined by spaces. `-` when it cannot be read. |
| `recipe_run` | The value of `AMPLIHACK_RECIPE_RUN_ID`, when set and non-empty. |
| `invocation` | The value of `INVOCATION_ID`, when set and non-empty. |

Only a write that changes the value is logged. `autostart off` when the
setting is already `off` writes no line, and `off` then `on` writes two. A
change from `unset` or `corrupt` is always logged. Changes to the auto-switch
threshold, exclusions or overflow list are not logged, nor are the
`AMPLIHACK_ROTATE_AUTOSTART` and `AMPLIHACK_CLAUDE_AUTO_SWITCH` variables,
which override the settings without changing them.

The parent command line is redacted before it is written:

- `NAME=value` becomes `NAME=***`.
- A name counts as secret if it contains `key`, `token`, `secret`, `passw`,
  `auth` or `signature`, or if one of its parts is exactly `pass`, `pwd` or
  `sig`. So `--pass` and `--db-pwd` are secret, but `--bypass` is not.
- The argument after a flag with a secret name becomes `***`, and so does
  the value in `--flag=value` for such a flag.
- A `Name: value` or `Name:value` argument with a secret name, such as
  the `Authorization: Bearer ...` or `X-Api-Key: ...` header passed to
  `curl -H`, becomes `Name: ***`. Other headers, such as
  `Content-Type: json`, are left as they are.
- `user:password` given to `-u` or `--user` (`curl -u user:pw`,
  `--user=user:pw`, `-uuser:pw`) becomes `***`. A value without `:`, such
  as `git push -u origin`, is kept.
- The value attached to `-p`, as in `mysql -psecret`, becomes `-p***`. A
  separate argument after `-p` is kept.
- `user:password@` is removed from URLs, and a query or fragment parameter
  with a secret name, such as `?access_token=...`, `#access_token=...`,
  `&sig=...` or `&X-Amz-Signature=...`, becomes `name=***`.
- An argument holding whitespace, such as the script after `bash -c`, is
  split into words, `'` and `"` are dropped from each word's ends, and the
  words are redacted by the rules above. A secret header name whose value
  is in the following words, as in `-H 'Authorization: Bearer ...'`, hides
  every word after it. So `bash -c "export API_TOKEN=t; curl ..."` is
  written as `bash -c export API_TOKEN=*** curl ...`.
- Not redacted: a separate password after `-p`, as in `sshpass -p secret`;
  credentials after combined short flags, as in `curl -su user:pw`; and
  `--proxy-user user:pw`.

For example, `claude --api-key sk-abc TOKEN=xyz https://u:p@h/x` is written
as `claude --api-key *** TOKEN=*** https://h/x`. Control, newline and
bidirectional-override characters are then removed, `"` becomes `'`, and the
result is cut to 200 characters. The run ids have control characters removed,
spaces replaced by `_`, and are cut to 64 characters. A value can therefore
never start a second log line or forge a field.

The line is written through the same path as other log lines, so the mode,
symlink and size rules above apply. Writing it takes no lock. If it cannot be
written, the command prints one warning to stderr and still succeeds.

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
variable, `CLAUDE_CONFIG_DIR`, `AMPLIHACK_CLAUDE_PROFILE` and `INVOCATION_ID`
removed and
`AMPLIHACK_CLAUDE_PROFILE_ROOT` set to the absolute root `start` resolved.
Removing the profile variables means a rotator started from a
`--profile` launch still probes and swaps `~/.claude`. Removing
`INVOCATION_ID` means a rotator started from a shell inside a systemd unit
does not claim to be that unit's [owner](#rotate-owner); a `--spawned`
`watch` never writes the owner marker in any case. The child validates that value again when it starts, and
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
owner: none
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
capacity: headroom 2.07 login-weeks of 4 (52%), usable 2 of 4 logins, runway ~9 h (as of 2026-10-18T12:50:05Z, 3m ago, rotate cycle: probed profiles only)
overflow: -
on overflow: no
hooks repaired: none in the last 24h
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

With two profiles whose last reading had no session window:

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default)
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
overflow: -
api key: -
on overflow: no
no session window: review, weekly
hooks repaired: none in the last 24h
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

On credits, with a second overflow profile blocked:

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default)
owner: none
pid: 48213
live: ryan-mistt-org
last cycle: 2026-10-24T09:10:05Z stayed overflow=on
overflow: ryan-mistt-org, team-credits (blocked until 2026-11-01T00:00:00Z)
on overflow: yes (ryan-mistt-org)
hooks repaired: none in the last 24h
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

| Key | Value |
|-----|-------|
| `root` | Always the first line. `<path> (source: env\|config\|default)` for the resolved root, or `<path> (pinned by running rotator)` when the rotator was found through the [rotate root record](#rotate-root-record). The lines after it describe that root. |
| `rotate` | `running`, `stopped`, `stale` or `unverifiable`. |
| `autostart` | `on` or `off`, then the source in brackets: `default`, `setting`, `env` or `corrupt`. See [Rotate autostart](#rotate-autostart). While an [owner marker](#rotate-owner) exists, ends with ` (deferring to <unit>)`, or ` (deferring to an unreadable owner marker)`. When the root fails the privacy check, ends with ` (skipped: root is not private)` instead, and the marker is not read. Unix only. |
| `owner` | The [rotation owner](#rotate-owner): `none`; `<unit> pid <n> (running); autostart defers to it`; `<unit> pid <n> (not running); autostart defers to it; run 'amplihack profile rotate owner clear' to release`; `unreadable; autostart defers to it; run 'amplihack profile rotate owner clear' to release`; or `not read (root is not private)`. Unix only. |
| `pid` | The pid, or `-`. |
| `live` | The ledger's `live` profile, or `-`. |
| `last cycle` | The time and outcome (`swapped`, `stayed`, `stayed-unread`, `switched`, `alerted`, `skipped`, `idle` or `held`) of the last cycle, or `-`. `stayed-unread` is a stay that is not capacity: the live probe did not give a [qualifying reading](#rotate-stays-and-capacity), and it was not an overflow stay read in that cycle. `skipped` is recorded when the state changed during measurement; `switched` by [`profile switch`](#rotate-switch), and kept by the held cycles after it; `held` when the [pin](#rotate-pin) held rotation with no switch record; a cycle that could not take the lock records nothing. An [overflow](#rotate-overflow) cycle adds ` overflow=in`, ` overflow=out` or ` overflow=on`. |
| `capacity` | The [capacity summary](#capacity-summary) of the newest [usage history](#usage-history) entry. See [Capacity line](#capacity-line). |
| `hold` | Only while something is at the [pin](#rotate-pin) path: `hold: <name>`, `hold: <name> (profile missing; ignored)` or `hold: unreadable`. |
| `overflow` | The marked overflow profiles in list order, separated by `, `, each followed by ` (blocked until <time>)` when it has an active [spend block](#overflow-spend-limit-block); `-` when none is marked or `.auto-switch` is corrupt. When the ledger cannot be read, the list ends with ` (blocks unknown)`. |
| `api key` | The first API-key profile, followed by ` (blocked until <time>, <reason>)` while the ledger records an active API-key block; `-` when there is no API-key profile. Printed between `overflow` and `on overflow`. |
| `on overflow` | `yes (<name>, held by switch)` when a [pin](#rotate-pin) names the live overflow profile: a `profile switch` onto it is a choice to spend, and the rotator does not move off it until `profile switch --none` (see [Pin and overflow](#pin-and-overflow)). Otherwise `yes (<name>)` when the profile identified as live in `~/.claude` by account is an overflow profile and the ledger's `last_cycle.overflow` is `in` or `on`, which means the rotator found no plan capacity in its last cycle. `no (<name> is overflow; plan capacity remains)` when the live profile is an overflow profile but the last cycle carries no such tag, for example after the login was put in `~/.claude` by hand. `no` when it is a plan profile, `-` when the live profile cannot be identified. `status` runs no probe; the ledger is its only evidence. |
| `no session window` | Printed only when the list is not empty: the profiles whose current auto-switch [reading](#readings) has `seven_day` and no `five_hour`, sorted by name and separated by `, `. These profiles are ranked on their week. The line does not say `no session limit`: a stored reading also loses its `five_hour` when a session window was not recorded (see the known limitations under [No session limit](#no-session-limit)). Only profiles that exist now are listed. The line comes after `on overflow` and after the `~/.claude is on API key profile ...` line when that is printed, and before `default` and `hold`. It is read from `usage.json` without a lock; if the file is missing, unreadable or unsafe, the line is left out and nothing is printed to stderr. |
| `default` | Printed only when the [default pointer](#clearing-the-default) names an overflow profile: `<name> (overflow; launches use it directly)`. |
| `log` | The log path. |
| `note` | Zero or one. The current [hand-change note](#hand-change-notes), without its `note: ` prefix. |
| `alert` | Zero or more. A current [hand-change](#rotate-hand-change-recovery) ALERT or `fix` line. |
| `warning` | Zero or more. A profile that lost its login to a manual `/login` in `~/.claude`. |
| `alert` (exhausted) | `alert: auto-switch is off while the live login is exhausted (<profile>: <reason>)`, printed after the ledger's `alert:` and `warning:` lines when the [exhausted-while-off](#exhausted-while-off) condition holds; or `alert: unavailable (<error>)` when auto-switch is off and a ledger or `usage.json` cannot be read, so the condition cannot be checked. |

Run from a shell where the root now resolves elsewhere, for example cron with
no saved root, `status` finds the running rotator through the record:

```
root: /home/you/.claude-profiles (pinned by running rotator)
rotate: running
autostart: on (default)
owner: none
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
overflow: -
on overflow: no
hooks repaired: none in the last 24h
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

#### Capacity line

`status` runs no probe, starts no process and does not look for `claude`. The
`capacity:` line comes from the [usage history](#usage-history) alone, and
follows `last cycle:`:

```
capacity: headroom X login-weeks of N (P%), usable U of N logins[, M unknown], runway ~H h (as of <time>, <age> ago[, <writer>])
```

When the newest entry is a rotator entry that left logins out, the runway
reads `runway ~H h from the <time> entry (this cycle left out M logins)` or
`runway unknown (this cycle left out M logins)` instead.

The newest valid entry stands in for this run's readings, and its `at` for
now: `N` is the number of profiles in that entry, the headroom and usable
count are worked out from it as in the [summary](#capacity-summary), and the
[rate](#rate-and-runway) uses the entries up to six hours before it. The
runway is `runway unknown` when the rate is.

A rotator entry that lists logins as unknown holds only the logins it
probed, which are the idle ones; the busy logins being used up are the ones
it left out. Its own runway would divide the idle logins' headroom by their
small use and come out far too long, or `unknown`, just while capacity is
being used. So the runway of such an entry is not printed. Instead the line
takes the runway of the newest entry no more than six hours older that read
every login (a `profile usage` entry, or a rotator entry with no unknown
logins) and names that entry's time: `runway ~H h from the <time> entry
(this cycle left out M logins)`. With no such entry, or one with too little
history for a rate, it prints `runway unknown (this cycle left out M
logins)`. Running `amplihack profile usage` every few hours while logins
are busy gives the line a runway. Weights come from
[`capacity-weights.json`](#capacity-weights); the usable threshold comes from
`AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT`, or `95`. `<age>` is how long
before the clock now that entry was written, in the largest whole unit:
`45s`, `3m`, `2h` or `1d`. An old entry is shown as it is, with its age.
When the newest entry came from a [rotator cycle](#usage-history), it holds
only the profiles that cycle measured, so `N` may be fewer than all
profiles; `profile usage` with no names records every profile it could read.
So that a change in `N` between cycles is not read as a change in usage, the
line names the writer of the entry after its age: `rotate cycle: probed
profiles only` or `profile usage: all logins`. An entry written before the
writer was recorded names none. When the entry lists logins its writer could
not read, `, M unknown` follows the usable count; they are in no figure. A
rotator entry lists there the logins it skipped as busy, those with no
usable trusted identity and those whose probe failed, so `M` says how many
logins the line leaves out.
When a profile in that entry has a weight other than `1.0`, the line adds
`   [weighted: X' of T]` after `(P%)`, as in the summary.

| History | Line |
|---------|------|
| No file, or no readable entry | `capacity: no recorded cycle` |
| A file that is refused (see [Usage history](#usage-history)) | `capacity: no recorded cycle`, and `warning: usage history ignored (unsafe file)` on stderr |
| Newest entry has no profiles | `capacity: headroom unknown (no valid readings), usable 0 of 0 logins, runway unknown (as of <time>, <age> ago)` |

A pending or inconsistent journal adds a line `journal: pending (phase <phase>)`
or `journal: inconsistent`, and one that cannot be read `journal: damaged`. A
ledger that cannot be read shows `live: - (ledger unreadable)`, or
`live: - (ledger unreadable: <field>)` when a field was refused, with the
same fixed text as the [unreadable-ledger stop](#unreadable-ledger-stop).
When the ledger names no live profile and the excluded profiles cannot be
worked out (the auto-switch config is unreadable, so every profile counts as
excluded, and the profile store cannot be listed), `status` shows
`live: unknown (excluded profiles unreadable: <cause>)` rather than `live: -`,
which would say that nothing is live.

The line before `log:` counts the rotator's repairs of amplihack's hooks
block in the last 24 hours (issue #2279): `hooks repaired: none in the last
24h`, or `hooks repaired: N in the last 24h (last <time>); something is still
removing amplihack's hooks`. Each `settings.json` file the cycle rewrote counts
once. See [the hooks block reference](hook-block.md#rotator-cycle).

Under a systemd unit that owns rotation, `status` shows the owner and the
deferral:

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default) (deferring to amplihack-rotator.service)
owner: amplihack-rotator.service pid 48213 (running); autostart defers to it
pid: 48213
live: spare
last cycle: 2026-11-01T09:30:05Z stayed
capacity: no recorded cycle
overflow: -
on overflow: no
hooks repaired: none in the last 24h
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

`status` reads the owner marker only after the root has passed the same
privacy check autostart uses at step 2 (a real directory owned by you and not
writable by others). When the root fails it, the marker is not opened, and
`status` prints these two lines, because a launch stops at step 2 and starts
no rotator whatever the marker holds:

```
autostart: on (default) (skipped: root is not private)
owner: not read (root is not private)
```

`owner: unreadable` always has the full form in the table above; it means
the root is private and the marker failed one of the
[checks](#reading-the-marker). `status` reads the marker and the usage
readings without a lock and changes nothing.

With auto-switch off and the live login exhausted:

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default)
owner: none
pid: 48213
live: spare
last cycle: 2026-11-01T10:10:05Z idle
capacity: no recorded cycle
overflow: -
on overflow: no
alert: auto-switch is off while the live login is exhausted (spare: 100% used, threshold 90%)
hooks repaired: none in the last 24h
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

After `last cycle:` and `capacity:`, `status` prints each line of the
ledger's `guidance`: a hand-change note as `note: <text>`, any other line as
`alert: <line>`. Then it prints one `warning: <line>` for each profile in its
`lost_login`. It does not compare accounts itself; it shows what the last
cycle decided. Lines are cleaned of control characters again before
printing:

```
live: ryan-ryansweet
last cycle: 2026-10-27T03:17:05Z alerted
note: ~/.claude holds the same account as profile ryan-mistt-org (ledger names ryan-ryansweet); nothing moved, the next cycle reads again
```

A confirmed suspicion that the [only-login rule](#only-login-rule) refuses
shows the ALERT and its `fix` lines instead:

```
live: ryan-ryansweet
last cycle: 2026-10-27T03:19:05Z alerted
alert: ~/.claude holds the same account as profile ryan-mistt-org on two readings (ledger names ryan-ryansweet), but ryan-ryansweet has no login of its own, so recording ryan-mistt-org would leave ryan-ryansweet with no login anywhere; nothing moved, no swap
alert: fix 1: if a /login in ~/.claude replaced ryan-ryansweet's login: close every claude session that uses ~/.claude, then run: mkdir -m 700 <dir> && mv -n <root>/ryan-mistt-org/.credentials.json <dir>/ && rm <ledger>; then run: amplihack profile login ryan-ryansweet
alert: fix 2: if ryan-ryansweet and ryan-mistt-org are profiles of one account, run amplihack profile verify to see the account each folder holds, then remove one of the two profiles
```

Recovery no longer adds profiles to `lost_login`; a `warning:` line appears
only for an entry an earlier release wrote:

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

It checks, in order, and stops at the first that fails, silently except at
step 4. Nothing is created on disk before step 9.

| Step | Check | When it fails |
|------|-------|---------------|
| 0 | `AMPLIHACK_RECIPE_RUN_ID` is unset or empty, so the launch is not a step of a recipe run. | Skipped (`recipe-run`). Nothing is read, created or printed. |
| 1 | Auto-switch is on: `<root>/.auto-switch` says `enabled=on`, or `AMPLIHACK_CLAUDE_AUTO_SWITCH=1`. A missing, damaged or `off` setting counts as off. | No effect. |
| 2 | The root exists, is a real directory (not a symlink) owned by you, and is not writable by other users. | Skipped. Nothing else under the root is read; in particular neither the owner marker nor the autostart setting is. |
| 3 | The effective uid is not `0`. | Skipped. `rotate start` still works as root. Root never acts on the owner marker. |
| 4 | There is no [owner marker](#rotate-owner). | Deferred. Nothing is started or created. Silent when the owner is running; otherwise one line to stderr, shown below. |
| 5 | Autostart is on (see the precedence table below). | No effect. |
| 6 | Not Docker: no `--docker` flag, and Docker not activated through the environment. The environment activates it only when `AMPLIHACK_USE_DOCKER` is truthy, the process is not already inside a container, and Docker is running. `--auto` launches are not skipped. | Skipped. |
| 7 | The [daemon state](#rotate-daemon) is not `running` or `unverifiable`. | Already running, or skipped. |
| 8 | The [prototype lock](#rotate-prototype-coexistence) is free and the root is private, as `start` requires. | Skipped. |
| 9 | Creates the state directory if needed and tries `<root>/.auto-switch.d/lock` once, without waiting. | Busy: another process holds it. |
| 10 | Under the lock, the daemon state is still not `running`. | Already running. |
| 11 | The `AMPLIHACK_ROTATE_*` [settings](#rotate-settings) in the launch's environment are valid. | Failed; no rotator started. |
| 12 | Starts the [daemon](#rotate-daemon), writes `rotate.pid` and the [root record](#rotate-root-record), then releases the lock. | Failed. If the pid file cannot be written, the child is sent `SIGTERM`. |

Steps 9 to 12 run under the state lock, and `start` and other launches check
under the same lock, so at most one rotator runs per root. Unlike `start`,
autostart runs no recovery; the daemon's first cycle does.

After step 12 the launch prints one line to stderr:

```
amplihack: started Claude login rotator (pid <pid>); 'amplihack profile rotate autostart off' to disable
```

and appends `autostart: started pid <pid> exe <path>` to `rotate.err`. A
failure at step 11 or 12, or an I/O error at step 9, is appended to
`rotate.err` as one `autostart: failed: <error>` line, when that file can be
written, and is otherwise ignored. Every `rotate.err` line has the
[log](#rotate-log) time prefix and its control characters removed.

At step 4, a marker whose owner is not running gives one stderr line per
launch:

```
amplihack: rotation is owned by amplihack-rotator.service, which is not running; run 'amplihack profile rotate owner clear' to let launches start the rotator
```

A marker that cannot be read or trusted gives:

```
amplihack: the rotation owner marker is unreadable; run 'amplihack profile rotate owner clear' to let launches start the rotator
```

When autostart is [off](#autostart-setting), launches still defer to the
marker, but the line stops after `which is not running` or `is unreadable`
and gives no advice: clearing the marker would start nothing.

Neither line fails the launch, and neither is written to `rotate.err`.

Launches inside a recipe run never start the rotator: `amplihack recipe run`
sets `AMPLIHACK_RECIPE_RUN_ID` for its steps, and step 0 stops there. A
recipe run therefore cannot start a second rotator beside one run by a
service manager or by hand. A top-level launch, outside any recipe run,
starts one as before. The Claude Code `SessionStart` hook does not start the
rotator.

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

`rotate autostart on|off` is the only command that writes the file. When the
write changes the value, it appends a [`change:` line](#rotate-change-log)
to the log, for example
`change: rotate-autostart on -> off by pid=52011 ppid=4120 parent="-bash"`.

### Rotate owner

A rotator run by a service manager, such as a systemd user unit, records
itself as the rotation owner. While it is recorded, launches do not start a
second rotator: step 4 of [autostart](#rotate-autostart) defers to it, even
when autostart is on.

The marker defers autostart only. `rotate start` does not read it: while the
owner is running, `start` refuses because a verified rotator is already
running, and while it is not, `start` starts a rotator as usual. That
rotator then makes the unit's next `watch` refuse.

A supervised `watch` refuses like any other `watch` when a verified rotator
is already running, whether started by autostart, by `rotate start` or by
hand. It exits with status 1 before writing the marker, so the service
manager restarts it and it keeps failing until that rotator stops. Run
`amplihack profile rotate stop` before you enable the unit.

#### Owner marker

The marker is `<root>/.auto-switch.d/rotate/owner`:

```
unit=amplihack-rotator.service
pid=48213
since=1793524205
```

`unit` is the unit name, `pid` the rotator's pid and `since` the epoch
seconds, in decimal, when it was written. The keys are written in that order.
`since` is informational: no decision reads it, and a missing, duplicated or
malformed `since` does not make the marker unreadable.

`rotate watch` writes the marker when all of these hold:

- it was not started with `--spawned`, so it is not a child of `rotate start`
  or of autostart;
- it was given `--supervised` or `--supervised=<UNIT>`, or `INVOCATION_ID` is
  set and non-empty and step 2 of [Owner unit name](#owner-unit-name) finds
  a unit;
- it has taken the state lock and written `rotate.pid`.

The marker is written atomically with mode `0600`, never through a symlink
and never over a file owned by another user. If it cannot be written, `watch`
logs `skip: owner marker not written (<error>)` and runs without one.

Nothing removes the marker except `rotate owner clear`. A `watch` that exits,
`rotate stop`, `rotate start` and a `watch` without `--supervised` all leave
it in place. When the service manager restarts the unit, the new `watch`
replaces it with its own pid. An unsupervised `watch` that finds a marker
prints one note to stderr and then runs as usual:

```
rotate: rotation is owned by amplihack-rotator.service; this watch runs anyway and leaves the owner marker in place
```

#### Owner unit name

The unit name is the first of:

1. the value of `--supervised=<UNIT>`, which must be 1 to 256 bytes with no
   `=`, control character or bidirectional-override or isolate character
   (U+202A to U+202E, U+2066 to U+2069); any other value is refused with
   `RotateRefused` (`--supervised=UNIT needs a unit name of 1 to 256 bytes
   without '=', control or bidi characters`, status 1);
2. the last component of the path in `/proc/self/cgroup` (read up to 4 KiB,
   preferring the cgroup v2 and `name=systemd` lines), when it ends in
   `.service` and is not a user manager (`user@<uid>.service`); this is where
   systemd places a service's processes, while a shell started from a tmux
   or terminal server ends in `.scope` and has no unit here;
3. `amplihack-rotator.service`.

#### Reading the marker

A marker is read only from a regular file, opened without following
symlinks and without blocking, that is owned by you, is not writable by group
or others, and is at most 4 KiB. It holds `key=value` lines. It is
*unreadable* when any of these checks fails, when `unit` or `pid` is missing
(a marker with only one of them, or an empty file, is unreadable) or appears
twice, when `pid` is not a positive decimal number without a sign,
or when `unit` is empty, longer than 256 bytes, or contains `=` or a control
character. Unknown keys are ignored.

An unreadable marker still counts as an owner, so autostart defers. A damaged
or foreign file at that path therefore cannot let a stray rotator start; run
`rotate owner clear` to remove it. Neither stderr nor `status` ever prints
the marker's bytes.

#### Owner liveness

The owner is *running* when `/proc/<pid>` exists and is owned by your
effective uid, and its command line, read up to 64 KiB, is a
[verified rotator](#rotate-root-record): it contains `profile`, `rotate` and
`watch`, and its `argv[0]` has the same file name as the running `amplihack`.
Otherwise it is *not running*. A recycled pid that now belongs to some other
program is therefore not running.
If the name of the running `amplihack` cannot be found, `status` and
`rotate owner` print `owner: <unit> pid <n> (liveness unknown); autostart
defers to it` instead of claiming the owner has stopped.

| Marker | Owner | Autostart (step 4) | `status` |
|--------|-------|--------------------|----------|
| none | - | Carries on to step 5. | `owner: none` |
| valid | running | Defers silently. | `owner: <unit> pid <n> (running); autostart defers to it` |
| valid | not running | Defers, and prints the `which is not running` line to stderr. | `owner: <unit> pid <n> (not running); autostart defers to it; run 'amplihack profile rotate owner clear' to release` |
| unreadable | - | Defers, and prints the `unreadable` line to stderr. | `owner: unreadable; autostart defers to it; run 'amplihack profile rotate owner clear' to release` |

#### Clearing the owner

```bash
amplihack profile rotate owner clear
```

```
owner: cleared (was amplihack-rotator.service)
```

`clear` removes the path itself; a symlink there is removed, and its target
is left alone. It prints `owner: cleared (was unreadable)` for an unreadable
marker and `owner: none` when there is nothing to remove. It does not stop a
rotator. If the path cannot be removed, it fails with status 1.

Run it when you retire the service: stop and disable the unit, then clear the
owner, so that launches start the rotator again. Leaving a stale marker in
place is safe; launches defer and warn until it is cleared.

#### Owner limitations

- `INVOCATION_ID` is inherited by every process in a systemd unit. A shell
  started from a tmux or terminal server that the user manager runs sits in
  a `.scope`, so a `rotate watch` started from it does not claim ownership.
  A `rotate watch` started by hand from a shell whose cgroup is itself a
  `.service` unit does record itself as the owner. Run `rotate owner clear`
  after stopping it. `rotate start` and autostart
  are not affected, because they remove `INVOCATION_ID` from the child.
- An `amplihack` binary older than this feature ignores the marker and may
  still start a rotator from a launch. Upgrade every binary that launches
  Claude against the root.
- `rotate status` prints the `owner:` line straight after `autostart:`, so a
  script that reads `status` by line number finds `pid:` one line lower.

### Exhausted while off

When auto-switch has been turned off and the live login has no capacity
left, nothing moves work onto another login. The background rotator reports
this as an ALERT on every cycle, and `rotate status` as an `alert:` line:

```
2026-11-01T10:10:05Z ALERT: auto-switch is off while the live login is exhausted (spare: 100% used, threshold 90%)
```

The condition holds when all of these are true:

1. Auto-switch has been turned off: `auto-switch off`,
   `AMPLIHACK_CLAUDE_AUTO_SWITCH=0`, or a corrupt `<root>/.auto-switch`. With
   no auto-switch setting at all, it is not off.
2. The rotate ledger names a `live` profile, and the auto-switch ledger maps
   it to the profile whose login it holds.
3. That profile has a [reading](#readings) in `usage.json`, and either:
   - the reading's source is a usage-limit failure whose reset time is still
     in the future, giving `usage limit reached, resets <time>` (the latest
     such reset), or
   - the higher of its session and weekly percentages is at or above the
     auto-switch threshold, giving `<pct>% used, threshold <t>%`. A window
     whose reset time has passed counts as 0%, as it does for
     [candidate selection](#readings): `usage.json` is not refreshed while
     sessions are stopped, so its stored percentage is out of date once the
     window resets. A usage-limit failure whose reset has passed is
     therefore no ALERT, whatever percentage it recorded.

The reason has exactly one of two forms:

| Form | Values |
|------|--------|
| `<pct>% used, threshold <t>%` | `<pct>` is the higher percentage rounded down to a whole number, so `99.7` is written `99`; the comparison with the threshold uses the unrounded value. `<t>` is the threshold as a whole number. |
| `usage limit reached, resets <time>` | `<time>` is the reset time in the RFC 3339 UTC form the log uses for its own timestamps, to the second with a `Z` suffix, for example `2026-11-01T14:00:00Z`. |

When both apply, the usage-limit form is used. The same text appears in the
log line and in the `alert:` line of `rotate status`.

No rotate ledger, no `live` profile, a live profile with no reading, and a
non-finite percentage or threshold all give no ALERT. A rotate ledger,
auto-switch ledger or `usage.json` that exists but cannot be read is not
taken as "not exhausted": `watch` writes
`skip: alert check failed: <error>` on every paused cycle instead, and
`rotate status` prints `alert: unavailable (<error>)`. The check takes no lock, runs no
probe and writes nothing. The ALERT does not change what the cycle does: the
rotator stays idle, and `idle: auto-switch is off` is still written only once
per change. Turn auto-switch back on, or switch logins by hand with
[`profile switch`](#rotate-switch), to act on it.

### Rotate daily session archive

`rotate watch`, including the background rotator, runs
[`amplihack sessions archive --apply`](./sessions-command.md) at most once a
day, so old session logs do not build up in `~/.claude/projects`. It is on by
default, and the config key `sessions.daily_archive` turns it off.

After each pass of the watch loop, before it sleeps, and whether or not the
rotator is idle or held by a pin, the rotator reads
`<root>/.auto-switch.d/rotate/sessions-archive.last`. The archive is due when:

- the file is missing, or does not hold a whole number of epoch seconds;
- 86,400 seconds or more have passed since the time it holds; or
- the time it holds is in the future, because the clock went back.

When the archive is due, the rotator reads `sessions.daily_archive` from
`~/.amplihack/config.toml` (see [switching it off](#turning-the-daily-run-off)).
If the key is `false`, nothing runs and nothing is written. Otherwise the
rotator writes the current time to the file before the archive starts. A
crash, a held [lock](./sessions-command.md#lock) or an error therefore waits a
full day before the next attempt, rather than retrying every cycle. If the
file cannot be written, the archive does not run and the log gets
`sessions archive: cannot record the run time (<kind>); not run`.

The archive runs inside the rotator process, with the days and destination
from the config keys `sessions.archive_days` and `sessions.archive_dest`,
read from the same file at the same time as the switch, or their defaults
(3 days, `~/.amplihack/session-archive`). `amplihack sessions archive` and
`sessions restore` in a shell read the same keys, so `restore` looks in the
directory the daily run wrote to. If the file cannot give either key a
value, the archive does not run, the time file is already written, and the
log gets `sessions archive: cannot read <key> (<why>); not run`. See
[`sessions` configuration](./sessions-command.md#configuration).

One pass archives for at most 60 seconds, so a large backlog never holds up
rotation for longer than that. The rotator checks the limit, and its stop
flag, between sessions, so a pass can run past 60 seconds by the time one
session takes, and `rotate stop` or a signal ends the run after the session
in progress. A run that stops early ends its summary line with the reason:

- `; stopped at the 60-second limit; the rest continues at the next pass`
- `; stopped by rotate stop or a signal; the rest continues at the next pass`

and the rotator removes the time file, so the next pass, after the next
cycle, carries on where the run stopped. The first run over a backlog of
thousands of sessions is therefore spread over several passes. If a run
stops early without archiving any session, the summary ends with
`; nothing archived, so the next try is a day later` instead, and the time
file is kept, so sessions that keep failing are not retried on every pass.
If the file cannot be removed, the log gets
`sessions archive: cannot remove the run record (<kind>); the rest waits a day`.

Its summary line, or one error line, goes to the [log](#rotate-log) with the
prefix `sessions archive: `, for example:

```
2026-10-29T03:15:00Z sessions archive: archived 412 sessions, 0.11 GiB (older than 3 days; 7 live sessions kept; 0 failed)
```

Each session the archive skips or fails also gets its own line, such as
`sessions archive: skipped <project>/<id>: symlink`; see the
[`sessions` messages](./sessions-command.md#messages). No error from the
archive stops or changes the rotation cycle.

The run moves old logs into the archive directory; it does not delete them.
Nothing removes archives from that directory, so it keeps growing, at
roughly a quarter of the rate the session logs grew. See
[`sessions` disk use](./sessions-command.md#disk-use) for its size, how to
move it to another disk and how to delete old archives.

#### Turning the daily run off

The switch is the user config key `sessions.daily_archive`:

```
amplihack config set sessions.daily_archive false
```

`true`, or no value, means on. The rotator reads the key from
`~/.amplihack/config.toml` each time a run is due, so the change reaches a
rotator that is already running, without a restart. Turn the run back on
with `amplihack config set sessions.daily_archive true`, or remove the key
with `amplihack config unset sessions.daily_archive`. The key is user-only:
a project's `.amplihack/config.toml` cannot set it, since it decides whether
files under `~/.claude/projects` are removed. See
[user config](./user-config.md).

The switch is a config key, not an environment variable, because the
background rotator does not inherit the launching shell's
`AMPLIHACK_ROTATE*` variables (see [Rotate daemon](#rotate-daemon)): a
variable set where `rotate start` or [autostart](#rotate-autostart) runs
would never reach it.

Because the run removes files, a config file the rotator cannot use stops
it rather than counting as on. When the file cannot be read or parsed, the
key holds a value other than `true` or `false`, or the file is writable by
group or others, the archive does not run, the time file is written so the
next try is a day later, and the log gets
`sessions archive: cannot read sessions.daily_archive (<why>); not run`.

Days and destination are config keys for a related reason. A rotator keeps
the environment it was started with, and one started by
[autostart](#rotate-autostart) takes it from whichever launch started it. A
variable exported later in a shell would therefore reach `sessions restore`
in that shell but not the rotator, and `restore` would look for the
session in a different directory from the one the daily run wrote to.

### Rotate prototype coexistence

Before a non-dry-run `rotate`, `start` or cycle, the rotator opens
`<root>/.rotator/lock` read-only (`O_NOFOLLOW`, `O_NONBLOCK`, never
`O_CREAT`), checks that it is a regular file, tries a non-blocking exclusive
lock, and releases it at once. A missing file passes the check and is not
created. If the lock is held, the command fails with
`RotatePrototypeRunning`. `--dry-run` skips this check. The rotator never
creates, writes or deletes anything under `<root>/.rotator/`. A supervised
step that hits a far reset makes the same check to decide whether a
[capacity wait](#capacity-wait) may start. While it holds the lock, an
external rotator that takes it with `LOCK_NB` at that instant is refused
once; the check holds the lock only for the length of two system calls.

### Rotate exit status

| Status | When |
|--------|------|
| `0` | No swap, a swap, a held cycle, a dry run, an ALERT, `status`, a `stop` of a stopped or stale rotator, a successful `profile switch`. |
| `1` | `RotateConfig`, `RotatePrototypeRunning`, `RotateInconsistent`, `RotateRefused`, `RotateUnsupported`, `RotateDaemon`, or any other [error](#errors). |
| `2` | A usage error from argument parsing, such as a tuning flag before `start`. |

### Rotate session resume

`amplihack-hooks limit-resume` is a Claude Code `StopFailure` hook that
restarts a session which stopped on a usage limit, once the rotator has
swapped `~/.claude` to a login with room. `amplihack install` registers it;
see [StopFailure in the hook reference](hook-specifications.md#8-stopfailure--limit-resume).
The [how-to](../howto/claude-profile-rotation.md#resume-sessions-stopped-by-a-usage-limit)
shows how to use it. The hook reads the rotate ledger, the
[swap record](#swap-record) and its own [resume state](#resume-state-directory),
and may run `tmux` against the session's own pane. From the session's
transcript it reads only the `timestamp` of the last entry (see
[Stop time](#stop-time)). It never moves a login, never reads any other part
of the transcript, and never opens `.credentials.json`, a token file or
anything under a profile directory.

The hook resumes a session in one of two ways, never both:

- **Keys** (outcome `resumed-keys`, exit `0`): when the session runs in a
  [verified tmux pane](#resume-keys), it types Esc, Ctrl+U, `continue` and
  Enter into that pane.
- **Wake** (outcome `resumed`, exit `2`): otherwise it exits `2` with the
  resume message on stderr.

Every other outcome exits `0` with nothing on stdout or stderr. The hook
never writes to stdout and never exits with any other status. A panic exits
`0` with nothing printed. The resume message is exactly:

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

Claude Code's timed wait (`Usage limit reached · continuing automatically
{when} · esc to cancel`) is not covered by either check. The strings of the
Claude Code 2.1.319 binary show the wait behind a feature flag, with
`Continuing automatically {when} · Esc to cancel`, `Usage limit reached
again · …`, `/usage-credits to continue now` and `/rate-limit-options to
re-arm`. They do not show whether `StopFailure` runs when the wait starts or
whether an exit `2` ends it. Esc is the exit the wait names, so the hook
sends keys whenever it has a verified pane and uses the exit `2` wake only
when it does not.

#### Resume decision

The checks run in this order. The first one that fails decides the outcome.

| Step | Check | When it fails |
|------|-------|---------------|
| 1 | The [profile root](#profile-root) resolves (`AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`, then `~/.amplihack/claude-profiles`) to an absolute path with no `..` component. The pointer file must not be writable by group or others, and auto-switch is on by the same rule as `auto-switch status`. The pointer file is read with no-follow and at most 4 KiB. | Exit `0`. Nothing under the root is created or written, and nothing but the toggle and the pointer file is read. Nothing is logged. |
| 2 | The stop was a usage limit (see [Limit detection](#limit-detection)), and `session_id` is present and at most 256 bytes. | Exit `0`. Nothing is logged. |
| 3 | `stop_hook_active` is absent, `null` or `false` (any other non-boolean value counts as `true`). Before this check the hook reads the ledger once, for its `live` profile. When the check passes, it reads the [stop time](#stop-time), notes the current swap record and the session's last resume, and works out the login the session stopped on for the [event key](#resume-state-directory). When it fails, none of the transcript, the swap record or the resume state is read. | Exit `0`, outcome `stop-hook-active`. |
| 4 | No marker exists for this limit event. | Exit `0`, outcome `duplicate`, or `guard-failed` if `resume/` or one of its subdirectories is unsafe (see [Resume state directory](#resume-state-directory)). |
| 5 | This session has resumed fewer than 3 times in the last 24 hours. | Exit `0`, outcome `cap`, or `guard-failed` if `resume/` or one of its subdirectories is unsafe. |
| 6 | The rotator is running: `<root>/.auto-switch.d/rotate/rotate.pid` names a running `amplihack profile rotate watch` process of yours (checked in `/proc`; skipped where there is none), and the ledger, if present, can be read by the trust rules in [Rotator capacity rule](#rotator-capacity-rule). | Exit `0` without waiting, outcome `rotator-down`. |
| 7 | `resume/` and `resume/prompted/` exist or can be created safely, so a prompt during the wait can be recorded; prompt markers older than 7 days and 1 hour are removed. | Exit `0`, outcome `guard-failed`. |
| 8 | The hook keeps the swap record it noted in step 3 (or that there was none), sets up the [event wait](#event-wait), and reads the swap record once. Then it waits for a record that passes [Capacity for a resume](#capacity-for-a-resume), for at most 7 days from its start. A record that fails is logged (`swap-stale` or `swap-no-room`) and the wait goes on. | Exit `0`, outcome `watch-lost` if the wait cannot be set up or a watch is lost; `moved-on` if you sent a prompt in that session; `session-gone` if the session exited; `no-capacity-timeout` at the 7-day deadline; `guard-failed` if a prompt marker cannot be read safely. |
| 9 | Auto-switch is still on, no prompt was sent in the session since the hook started, and, if the hook noted a pane, the pane still [passes its check](#resume-keys); a pane that fails now is dropped and the wake is used instead. Then, under the resume lock, steps 4 and 5 still pass, the session has no resume after this hook's start, the event marker is created and the session record is updated and synced to disk. | Exit `0`, outcome `disabled` if auto-switch was turned off; `moved-on` if a prompt was sent; `duplicate` if another hook resumed this event or this session first; `cap` if the session reached its cap; `guard-failed` if the lock is not taken within 5 seconds or any write fails. |
| 10 | With a pane: send the [resume keys](#resume-keys). | Exit `0`, outcome `resume-keys-failed`, with the number of keys sent. The resume stays recorded and the hook does not exit `2`. |
| 11 | — | Exit `0`, outcome `resumed-keys`, when keys were sent; exit `2` with the resume message, outcome `resumed`, when there was no pane. |

Steps 4 and 5 run once without the lock, so a duplicate or capped session
exits at once instead of waiting up to 7 days, and again under the lock in
step 9. The lock is not held while waiting in step 8. The choice between
keys and the wake is made before the commit, and the marker and session
record are on disk before the first key or the exit `2`, so a resume that is
cut short never repeats and a session never gets both.

A session is resumed at most once per waiting period. When two hooks wait
for the same session with different events, the first to commit resumes
it; the other finds a resume after its own start time and ends as
`duplicate`, or, if keys typed `continue` first, as `moved-on`. A new limit
that starts in the same second as the previous resume may still resume.

##### Moved on

The session moved on when you sent a prompt in that session during the wait.
amplihack's `UserPromptSubmit` hook (`amplihack-hooks user-prompt-submit`)
records the time of each prompt in `resume/prompted/<sha256 hex>`, moving the
file into place. The waiting hook is woken by that move, reads its own
session's marker, checks it again right before the commit in step 9, and
exits `0` with outcome `moved-on` when the time is after its own start. A
marker for another session wakes the hook, which reads its own marker and
goes back to waiting. The hook's own exit `2` wake is not a prompt, so the
woken turn does not count. The `continue` the [resume keys](#resume-keys)
type is a prompt: it ends any other hook waiting for the same session as
`moved-on`. Background task reports and local commands such as `/usage` are
not prompts, so they never end the wait.

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

Only a [swap record](#swap-record) resumes a session. When the hook starts
waiting it notes the record in `swap.json`, or that there is none. Each
record it reads afterwards is judged in this order:

| Order | Check | Result when it holds |
|-------|-------|----------------------|
| 1 | The file is missing or unreadable, or the record equals the noted one in every field and is stale by the rule in order 2. | Nothing logged; keep waiting. |
| 2 | `swapped_at` is before the threshold (`swapped_at < min(stop, start - 1)`, or `swapped_at < start - 1` when there is no [stop time](#stop-time)), or at or before the session's last resume. `reading.measured_at` is not checked: the rotator measures before it swaps, so a session that stops between the two stopped on the login being swapped away from. | Log `swap-stale`; keep waiting. |
| 3 | `reading.session` or `reading.week` is 100 or more. | Log `swap-no-room`; keep waiting. |
| 4 | — | Resume (step 9 of [Resume decision](#resume-decision)). |

The threshold is the earlier of two times, both whole seconds:

- the [stop time](#stop-time), the `timestamp` of the last entry in the
  session's transcript, rounded down;
- one second before the hook's start (`STOP_SLACK_SECS`, 1 second).

The rotator takes `swapped_at` when the swap has finished. A swap in the
hook's start second, or in the second before it, may have finished after
the stop and is not stale, whether or not a stop time was read. When the
hook starts some seconds after the stop, the stop time lowers the
threshold, so a swap between the stop and the hook's start still counts.
With stop `T` and start `T+10`, a swap at `T+3` resumes the session and a
swap at `T-5` does not. The stop time never raises the threshold above
`start - 1`, so a swap that counts without a stop time also counts with
one. The same rule applies to the noted record: a swap that finishes
between the stop and the hook's start is the record the hook notes, and it
resumes the session at once when the new login has room. The cost of the
allowance is that a swap made just before the stop can send the session one
`continue` it did not need; without it, a session stopped on the outgoing
login would wait for the next swap, up to 7 days.

A swap at or before the session's last recorded resume is never treated as
after the stop, whatever the times: the session ran again after that resume,
so it stopped later. A resumed session that hits a limit again within a
second is therefore not woken twice on one swap; it waits for the next one.

`seq` is never compared for order. A record that differs from the noted one
in any field and passes the time checks counts, so a rotator whose `seq`
started again at 1, for example after `swap.json` was deleted, still
resumes sessions, and a record rewritten unchanged never does.

So a record written before the stop never resumes a session. A session that
stops on the login the rotator has just swapped to waits for the next swap.
A stay, an ALERT, a [`profile switch`](#rotate-switch) and the passing of
time never resume a session, because none of them writes a swap record. The
hook does not read the target login or apply the rotator's thresholds; it
trusts the reading in the record.

#### Stop time

The stop time is the `timestamp` of the last complete entry in the
transcript named by the payload's `transcript_path`, as whole epoch seconds,
rounded down. It is read once, after the `stop_hook_active` check in step 3 of
[Resume decision](#resume-decision) passes, by
`swap_record::read_stop_time` in `amplihack_utils::rotator_status`:

| Rule | Detail |
|------|--------|
| Open | `O_NOFOLLOW`, `O_NONBLOCK` and `O_CLOEXEC`. A symlink as the last path component is refused; symlinks in parent directories are allowed, because the checks below apply to the opened file. A FIFO does not block the open. |
| File | A regular file owned by you and not writable by group or others (`mode & 0o022 == 0`), checked on the open file. A transcript need not be `0600` or have a single link. |
| Size | At most the last 65 536 bytes (`STOP_TAIL_BYTES`) are read. A partial last line is ignored, and so is a partial first line when the read did not start at the beginning of the file. A last entry longer than 65 536 bytes gives no stop time. |
| Entry | Only the `timestamp` field of the last non-empty complete line is read. It must be an RFC 3339 string; a number, a missing field or a line that is not JSON gives no stop time. |
| Range | Accepted only when it is after 1970 and not after the hook's start (`0 < stop <= start`). |

When `transcript_path` is absent, or any rule fails, there is no stop time
and the hook decides exactly as it would without one. The stop time is
never logged and never reaches `resume.log`, stderr, stdout, the resume
message or the event key, and is never hashed. It only decides whether the
noted swap counts as after the stop, and so which login goes into the event
key. The reader returns only a number of seconds or nothing, with no error
to report. It does not retry or wait.

Claude Code may write the error entry after the hook starts, or not at all.
The last entry is then older than the stop, which can only lower the
threshold, so the session may get one `continue` it did not need; it never
waits for a swap it should have taken. The resume cap, the last-resume rule
and the room check still apply.

#### Swap record

The rotator writes `<root>/.auto-switch.d/rotate/swap.json` in the
`swapped` branch of the [rotate cycle](#rotate-cycle), after the ledger is
saved and reads back with `live` equal to the new login, and before it
releases the rotate lock. It is written through the same atomic writer as
the ledger: a temporary file in the same directory, mode `0600`, then a
rename, so a reader never sees a partial file.

```json
{
  "seq": 42,
  "live_login": "spare",
  "from_login": "work",
  "swapped_at": 1762654215,
  "reading": { "session": 12, "week": 31, "measured_at": 1762654190 }
}
```

| Field | Meaning |
|-------|---------|
| `seq` | The previous record's `seq` plus 1, or 1 when there is no previous record or it cannot be read. |
| `live_login` | The profile now live in `~/.claude`. |
| `from_login` | The profile swapped out. |
| `swapped_at` | Epoch seconds when the swap finished, taken after the login was moved, not when the cycle started. |
| `reading.session`, `reading.week` | The new login's session and weekly use from this cycle's measured reading, rounded up to a whole percent and clamped to 0–255. |
| `reading.measured_at` | Epoch seconds when that reading was taken. |

The record holds only these fields: profile names, numbers and times, never
an account, a token or a path. Nothing is written for a stay, an ALERT, a
refused or failed swap, a [`profile switch`](#rotate-switch) or an
[in-session swap](#in-session-swap). A swap whose target reading is not a
measured reading, or whose session or weekly figure is not a finite number,
writes nothing, so a missing figure is never recorded as 0%. When the record
is not written the rotator logs one line,
`swap record not written: <reason>`, and the swap and the cycle's outcome
stand.
When a previous record exists but cannot be read or parsed, the new record
is written with `seq` 1 and the rotator logs one line,
`swap record seq restarts at 1: previous unreadable (<reason>)` or
`swap record seq restarts at 1: previous unparsable`. The first record
logs nothing.

The hook reads the record loosely but safely: at most 4096 bytes, with
no-follow, from a regular file owned by you and not writable by group or
others. It ignores unknown fields. A record is not trusted, and is read as
missing, when `live_login` or `from_login` fails the
[name rules](#name-rules), or when `swapped_at` or `reading.measured_at` is
0 or less or more than 300 seconds in the future.

#### Event wait

The hook waits in one blocking `poll(2)` call, with nothing read on an
interval. It watches:

| Source | What it watches | What it means |
|--------|-----------------|---------------|
| inotify on `<root>/.auto-switch.d/rotate/` | `swap.json` closed after writing or renamed into place | A swap record may have changed; read and judge it. |
| inotify on `<root>/.auto-switch.d/resume/prompted/` | any file renamed into place | A prompt may have been sent; check this session's marker. |
| a pidfd on the hook's parent process | the process exits | The session is gone: `session-gone`. |
| the `poll` timeout | the time left until 7 days after the hook's start | `no-capacity-timeout`. The timeout never resumes a session. |

Both directories must be real directories, not symlinks, owned by you and
not writable by group or others. The hook's parent pid must not be 1, and
must still be the hook's parent once the pidfd is open, so a reused pid
cannot stand in for the session. If the pidfd is missing when the hook
checks the session before sending a key, the session counts as not running
and no key is sent; this check does not replace the `watch-lost` outcome
below. If any of this fails, or inotify or
`pidfd_open` is unavailable (a kernel older than 5.3, or a system other
than Linux), the hook ends as `watch-lost`. It does not fall back to
polling.

A watched directory that is deleted or moved, or an event the hook cannot
parse, ends the wait as `watch-lost`. An inotify queue overflow counts as a
possible swap: the hook reads and judges the record. After the watches are
in place the hook reads `swap.json` once before it waits, so a swap written
between noting the record and setting up the watch is not missed. Turning
auto-switch off does not wake the hook; it is checked in step 9, so a
waiting hook ends as `disabled` at the next swap that would resume it.

#### Resume keys

The hook notes a pane once, when it starts, from its own environment: only
when `$TMUX` is set and not empty and `$TMUX_PANE` is exactly `%` followed
by 1 to 9 digits. The pane id is kept in memory only, never written to disk,
and logged only as the first 12 hex digits of its SHA-256.

The pane is used only if it passes this check, made once, right before the
commit:

1. `tmux display-message -p -t %N '#{pane_tty}'` exits 0 and prints an
   absolute path under `/dev/` of at most 256 bytes;
2. that path is the target of `/proc/<parent>/fd/0`, Claude Code's standard
   input; and
3. `/proc/<parent>/stat` shows Claude Code's process group as the
   terminal's foreground process group.

A pane inherited from a shell that started Claude Code outside that pane,
or a pane where something else is in the foreground, fails the check, and
the hook uses the exit `2` wake instead.

The keys are four separate `tmux` runs, each with a fixed argument list, no
shell, null standard input and output, and each waited on for its exit
status before the next:

| Order | Command | Purpose |
|-------|---------|---------|
| 1 | `tmux send-keys -t %N Escape` | Leave the timed wait, or close the limit menu. |
| 2 | `tmux send-keys -t %N C-u` | Clear the input box, including a dimmed predicted prompt. |
| 3 | `tmux send-keys -t %N -l continue` | Type `continue` as literal text. |
| 4 | `tmux send-keys -t %N Enter` | Send it. |

Before each key the hook checks, without blocking, that the session's
process is still running and that `/proc/<parent>/stat` still shows it as
the terminal's foreground process group, so a session suspended with
Ctrl-Z gets no more keys. A key whose `tmux` run exits non-zero, or a
session found gone or suspended, stops the sequence: the remaining keys are not sent,
and the outcome is `resume-keys-failed` with `keys_sent` set to the number
sent and `detail` naming the cause (`session-gone` for a session that
exited or was suspended, or the error kind). The hook never falls back to
exit `2` after the commit. If the session exits or is suspended in
the instant between the check and a key, that one key may reach whatever
then reads the pane.

No other key or text can be sent: the key type has only these four values.
The hook never sends a digit, so it never picks a numbered limit option,
and never types `/usage-credits` or `/rate-limit-options`. `tmux` is found
through `PATH` and reaches the server named by `$TMUX`; both belong to the
same user as the session, which is the trust boundary.

#### Rotator capacity rule

This is the ledger rule the [usage-limit wait](#usage-limit-wait) uses for
its `rotator capacity` check (`CycleStatus::fresh_capacity` in
`amplihack_utils::rotator_status`). The limit-resume hook does not resume on
it; it resumes only on a [swap record](#capacity-for-a-resume). The hook
still reads the ledger, by the trust rules at the end of this section, for
the `live` profile and for the `rotator-down` check. The `live` profile is
the login in the [event key](#resume-state-directory) unless a noted swap
record shows that the session stopped on the outgoing login.

The rule reads `<root>/.auto-switch.d/rotate/ledger.json` (see
[Rotate state directory](#rotate-state-directory)) and uses only `live`,
`alert` and `last_cycle`. There is capacity when all of these hold:

- `last_cycle.measured_at` is after the time the wait started, so the
  measurement, or the reading behind it, was taken after the limit hit;
- `last_cycle.outcome` is `swapped`, `stayed` or `switched` (written by
  [`profile switch`](#sessions-stopped-by-a-limit), with `measured_at` the
  time of the switch);
- `alert` is not set.

The rotator writes these outcomes so that capacity means one of:

- a fresh, successful reading of the live login below 100% on both windows
  (`stayed`); a stay on an [overflow](#rotate-overflow) profile read in this
  cycle with no active spend block, at any figures, and a stay on a platform-tier login also record
  `stayed` (see [Stays and capacity](#rotate-stays-and-capacity));
- a swap to a login with room (`swapped`);
- a [`profile switch`](#rotate-switch) to a login (`switched`).

A stay on a failed, carried or unknown live probe records `stayed-unread`,
which is not in the list above, so it is not capacity. A `no-capacity` ALERT
written while the live login reads below 100% on both windows is
[advisory](#rotate-advisory-alert): it is stored in `advisory_alert`, not
`alert`, and the cycle records `stayed`, so it does not block it. The
rule does not read `advisory_alert`.

`last_cycle.at` is set when the cycle ends, so a cycle that was already
measuring when the limit hit can be recorded after the wait starts with
readings taken before the limit; `measured_at` rules it out whatever the
probe timeout. A ledger written by a rotator older than
[#1659](https://github.com/rysweet/amplihack-rs/issues/1659), which has no
`measured_at`, needs `last_cycle.at` more than 60 seconds after the wait
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
a limit-resume hook still finds a running rotator and waits its full 7 days
for a swap record that never comes.
Before the older binary runs, including a launch that would
[start the rotator automatically](#rotate-autostart), stop the rotator with
this version (`amplihack profile rotate stop`), then delete
`<root>/.auto-switch.d/rotate/ledger.json`, then start the older rotator.
The ledger holds only bookkeeping; the next cycle works out the live profile
again from the accounts. Deleting it also clears the swap hold and the alert
de-duplication key, so the first cycle may swap or alert once more. If an
older rotator is already running and logging failures, the same three steps,
with `rotate stop` run by either binary, recover it.

A cycle that moved onto an [overflow](#rotate-overflow) profile records
`swapped`, and one that stayed on an overflow profile whose probe read the
login in this cycle, with no active spend block on it, records `stayed` at
any figures, even with its plan windows at 100% (see [Stays and capacity](#rotate-stays-and-capacity)). Both
have no `alert`, so they count as capacity for the usage-limit wait, and the
step it reruns runs on credits. The limit-resume hook resumes only when the
swap record shows the plan windows below 100%. A stay on an overflow profile whose live probe
failed, was carried, was unknown or hit a spend limit, or whose login has
an unexpired entry in `overflow_blocked`, records `stayed-unread` and is
not capacity.
`last_cycle.overflow` does not change the verdict; the rule does not read
`overflow_blocked` itself, because the rotator has already applied it to the
outcome it records. A spend block with nowhere to move ends the cycle on the
no-capacity ALERT, which is not capacity.

Any outcome other than `swapped`, `stayed` or `switched` (`alerted`,
`stayed-unread`, `skipped`, `idle`, `held`, or one the rule does not know), a missing ledger, or a ledger that is not
trusted means no capacity yet. A hook from before
[#1907](https://github.com/rysweet/amplihack-rs/issues/1907) reads
`stayed-unread` as an outcome it does not know, so it also waits. The rule does not apply the
rotator's thresholds itself.

The hook and the wait read the ledger more loosely than the rotator does: they ignore
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
and epoch seconds. No pane id is stored.

| Path | Contents |
|------|----------|
| `lock` | Lock file for steps 4, 5 and 9. Waited on for at most 5 seconds. |
| `events/<sha256 hex>` | One empty marker per limit event. The name is the SHA-256 of a fixed prefix followed by `session_id`, `error`, `error_details`, `last_assistant_message` and the login the session stopped on, each written with its length, so no two different payloads share a name. The login the session stopped on is the noted swap record's `from_login` when the swap falls between one second before the [stop time](#stop-time) and the hook's start (`stop - 1 <= swapped_at <= start`). The bound depends on the stop alone, not on when the hook started or on the session's past resumes, so every run of the hook for one stop gets the same name, whatever second it starts in and before and after that stop is resumed; a repeat run is `duplicate` and never resumes the session a second time. The second before the stop is the allowance [Capacity for a resume](#capacity-for-a-resume) gives a hook that starts in the stop's second, so a session resumed through it has its event named for that record's `from_login`. A hook that starts two or more seconds after the stop does not resume on a swap in the second before the stop, but its event is still named for that swap's `from_login`. The cost: a session resumed on that swap in the second before its next stop, which stops again with the same text and whose hook starts two or more seconds late, gets the earlier event's name and is `duplicate`, so it is left stopped rather than sent keys twice. Without a stop time it is `from_login` when the record is after the stop by the rule in [Capacity for a resume](#capacity-for-a-resume) and not later than the hook's start. Otherwise it is the ledger's `live` profile when the hook started. A session that stops on the outgoing login while the rotator swaps, is resumed, and stops again on the new login therefore gets a new event, not `duplicate`. The same login hitting its limit again is the same event; a limit after the rotator swapped to another login is a new one. Markers older than 7 days are removed. |
| `sessions/<sha256 hex>` | One file per session, named by the SHA-256 of a different fixed prefix and its `session_id`, with one epoch-seconds line per resume in the last 24 hours. A file that cannot be parsed counts as the cap being reached and is left unchanged. |
| `prompted/<sha256 hex>` | One file per session, named like `sessions/`, holding the epoch seconds of your last prompt in it. Written by the `UserPromptSubmit` hook, replaced atomically. Removed after 7 days and 1 hour (the longest wait plus 1 hour) when a hook starts waiting or commits. A file that cannot be parsed or is not private makes a waiting hook end as `guard-failed`. |
| `resume.log` | One line per decision from step 3 on, and one per swap record judged stale or without room. |
| `resume.log.1` | The previous log. `resume.log` is renamed to it when it passes 64 KiB. |

#### Resume log

```
2026-11-09T00:41:07Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=swap-no-room live=work cycle=stayed swap_seq=41 swap_live=third
2026-11-09T00:43:12Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=resumed-keys live=work cycle=stayed swap_seq=42 swap_live=spare pane=5b0e19c4a7d2 keys_sent=4
2026-11-09T00:43:12Z resume session=91be04d2aa70 event=a0c55e1f9d38 outcome=resumed live=work cycle=stayed swap_seq=42 swap_live=spare
2026-11-09T02:10:55Z resume session=c41d7a9e0b13 event=e9f2b6a01c7d outcome=resume-keys-failed live=spare cycle=stayed swap_seq=43 swap_live=spare pane=0d3a8f61e2b9 keys_sent=2 detail=session-gone
2026-11-09T02:15:01Z resume session=6a0b3c9d1e42 event=17c5e8d2f0a6 outcome=watch-lost live=spare cycle=stayed detail=NotFound
```

`session` and `event` are the first 12 hex digits of the hashes above. `live`
and `cycle` are the ledger's `live` profile and `last_cycle.outcome` when the
hook started (the login that hit the limit), or `-` when the hook did not
get as far as reading it. The fields
after `cycle` appear only when they apply:

| Field | Contents |
|-------|----------|
| `swap_seq` | The `seq` of the swap record the decision was about. |
| `swap_live` | That record's `live_login`. |
| `pane` | The first 12 hex digits of the SHA-256 of the pane id, when keys were sent or tried. |
| `keys_sent` | How many of the four keys were sent (0–4). |
| `detail` | Why a watch or key failed: `session-gone` (the session exited, or was suspended while keys were sent), or the name of the I/O error kind, such as `NotFound` or `PermissionDenied`. |

Every field holds only `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` and `-`; any other
character is written as `-`. The log never holds message text, error text,
pane ids, account IDs or tokens. A log written by an earlier version, without
the new fields, is still valid. Failing to write it does not change the
outcome.

| Outcome | Exit | Meaning |
|---------|------|---------|
| `resumed-keys` | `0` | A swap record with room arrived, the resume was recorded, and all four keys were sent to the session's pane. |
| `resumed` | `2` | The same, for a session with no verified pane: the hook exited `2` and the session was told to continue. |
| `resume-keys-failed` | `0` | The resume was recorded, but a key failed or the session exited part way; `keys_sent` says how many went. No exit `2` follows. |
| `swap-stale` | — | A new swap record was swapped at or before the stop. The hook keeps waiting. Never the final outcome. |
| `swap-no-room` | — | A new swap record shows the new login at 100% on a window. The hook keeps waiting. Never the final outcome. |
| `watch-lost` | `0` | The [event wait](#event-wait) could not be set up, or a watch was lost. Never resumes. |
| `no-capacity-timeout` | `0` | No swap record with room within 7 days. |
| `moved-on` | `0` | You sent a prompt in that session during the wait, so the hook did not resume it. |
| `disabled` | `0` | Auto-switch was off when a swap record would have resumed the session. |
| `duplicate` | `0` | This limit event, or this session since the hook started, was already resumed. |
| `cap` | `0` | The session already resumed 3 times in 24 hours. |
| `stop-hook-active` | `0` | Claude Code reported a stop hook already running. |
| `session-gone` | `0` | The Claude Code session that started the hook exited during the wait. |
| `rotator-down` | `0` | When the hook started, no rotator was running, or its ledger could not be read (for example a newer version). The hook did not wait. |
| `guard-failed` | `0` | `resume/`, the lock, a marker or the session record could not be used safely. |

#### Resume limits

| Constant | Value |
|----------|-------|
| Wait for a swap record | 604800 s (7 days) |
| Hook timeout margin | 300 s |
| Registered hook timeout | 608400 s |
| Swap record size | 4096 bytes |
| Clock skew allowed | 300 s |
| Keys per resume | 4 |
| Resumes per session | 3 per 24 hours |
| Resumes per limit event | 1 |
| Resume lock wait | 5 s |
| Event marker retention | 7 days |

The wait plus the margin is less than the hook timeout, so the hook always
decides before Claude Code stops it. The wait is a lifetime cap, not an
interval: it ends the wait and never starts a resume.

## Usage-limit wait

When a recipe agent step fails on a usage limit and no auto-switch rerun is
left, the step's supervisor waits and then runs the step once more. It is on
by default and does not need auto-switch or profiles. Task-oriented guidance
is in the [how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

There are two kinds of wait:

- a **reset wait**, for a limit that resets within five hours: it waits for
  that reset, or for a login with capacity to become live, whichever comes
  first (see [Reset time](#reset-time));
- a **capacity wait**, for a limit that does not reset within five hours (a
  weekly limit) or states no readable reset, when another login could take
  over: it waits only for a login with capacity, for at most
  `max_wait_secs` (see [Capacity wait](#capacity-wait)).

When neither applies, the step is not waited for and the run ends as before.

The wait reads only files: the harness's limit message, the
[capacity file](#capacity-file), the stored [readings](#readings) and the
ledgers. It makes no usage, token or identity request for any profile and
writes no credential file, so a profile in the auto-switch `exclude` list is
never probed or refreshed by it, and never counts as a login that could take
over.

The [capacity governor](./capacity-governor.md) does not hold an agent step
of a running recipe at an exhausted weekly or session limit: this wait owns
that case, so the governor's shorter wait never replaces it (see
[Exhausted limits inside a recipe](./capacity-governor.md#exhausted-limits-inside-a-recipe)).

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
The retry after a reset wait always runs under the inherited profile, not
under the last profile auto-switch tried, so a reset wait is for the
**inherited profile's** reset (see [Order of attempts](#order-of-attempts)).
The retry after a capacity wait runs under the profile that showed capacity
(see [Retry login](#retry-login)).
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
6. the limit is not one a reset wait can outlast (see
   [Reset time](#reset-time)) and no [capacity wait](#capacity-wait)
   applies: `skipped reason=<why>`, and the result is passed up;
7. otherwise `start`, the wait, and `end`. After `end reason=signal` the
   step exits with `128` plus the signal; after `session ended`, or after
   `step timeout` on a reset wait, the failed attempt's result is passed up;
   after `capacity expired`, or `step timeout` on a capacity wait, the
   [closing line](#closing-line) is written and the failed attempt's result
   is passed up; after any other reason `resume` is written and the step
   runs once more.

Steps 6 and 7 read the inherited profile's limit, not the last attempt's.
The retry runs under the inherited profile, so only that profile's reset
says when it can work again. When auto-switch ran the step under other
profiles before running out, their limits only steer auto-switch's choice
of the next profile: a candidate that resets later does not stretch the
wait, and a candidate's weekly limit or spend limit does not cancel it.
When auto-switch moved the step off the inherited profile before the first
attempt, the inherited profile's limit comes from the usage readings: the
latest reset among its windows at 100%. A reset from a reading more than
five hours away is treated like a weekly limit in a message: it starts a
capacity wait when one applies, else `skipped reason=reset_too_far`. A window over the auto-switch
threshold but under 100% is a reason to prefer another profile, not a
limit, so it neither times nor cancels the wait. If no window is at 100%,
the inherited profile still has capacity and no wait is built: when the
candidates are limited, the step goes back to the inherited profile through
step 3, with no `start` or `end` event, and the wait is kept for a limit on
that profile itself. A candidate's limit never times it. If that move is
refused (a piped prompt), step 4 or 6 still writes the `skipped` event.

There is at most one wait per step, of either kind. A limit on the retry
writes `skipped reason=already_waited` and then the
[closing line](#closing-line), and the step ends as a usage limit. The retry
runs the step **from the start** with the same prompt, so anything the failed attempt already did (a
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
| Session limit | `You've hit your session limit · resets 1pm (America/Los_Angeles)` | reset wait, when the reset is at most five hours away |
| 5-hour limit | `You've hit your 5-hour limit · resets 8:20pm (America/Los_Angeles)` | reset wait, when the reset is at most five hours away |
| Per-model limit | `You've hit your Opus limit · resets 8:20pm (America/Los_Angeles)` | reset wait, when the reset is at most five hours away |
| Weekly limit | `You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)` | [capacity wait](#capacity-wait) when another login could take over, else `skipped reason=reset_too_far` |
| Spend limit with a session reset | `You've hit your org's monthly spend limit · run /usage-credits … · your session limit resets 1pm (America/Los_Angeles)` | wait for that reset: the session limit is what stops work |
| Spend limit with a weekly reset | `… monthly spend limit · … · your weekly limit resets Oct 8, 12am (America/Los_Angeles)` | `skipped reason=reset_too_far`, even when another login could take over |
| Spend limit, no reset | `You've hit your org's monthly spend limit · run /usage-credits …` | `skipped reason=spend_limit`: no reset lifts it |
| Epoch | `Claude AI usage limit reached\|1792335000` | reset wait, when at most five hours away; further away, as for a weekly limit |
| Shape, no readable reset | `You've hit your session limit · try again later` | capacity wait when another login could take over, else `skipped reason=no_reset` |
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
hours away, or none that parses, is not waited for by a reset wait: waiting
would hold the run for hours with nothing known to change. Such a limit
starts a [capacity wait](#capacity-wait) instead when another login could
take over.

### Deadline

This section covers reset waits; a capacity wait has its own
[deadline](#capacity-wait-deadline). A reset wait ends at the earliest
candidate that applies (a tie is named by the earlier row):

| Candidate | Applies when | `bound=` |
|-----------|--------------|----------|
| reset + 120 s, or now when the reset has passed | always | `reset+margin` |
| now + `max_wait_secs` | a maximum is set | `max_wait` |
| step start + `AMPLIHACK_STEP_TIMEOUT` − 60 s, never before now | the step timeout is set and above 0 | `step_timeout` |

No input can make a reset wait longer than five hours and 120 seconds.
`max_wait_secs = 0` does not wait but still retries once. A reset wait that
reaches `max_wait` retries once, whatever the reason the maximum was set.

### Ending the wait early

Both kinds of wait sleep in slices of at most one second and stop at the
first of the reasons below that applies to them. Signals and the session
check come first, then the deadline, then the 30 s probes in the order
listed.

| `end reason=` | Wait | Cause | Retry |
|---------------|------|-------|-------|
| `signal` | both | `SIGINT`, `SIGTERM` or `SIGHUP` was received | no; exits `128` + signal |
| `session ended` | both | the process that started the step exited (the supervisor's parent changed) | no |
| `reset reached` | both | the `reset+margin` deadline passed; on a capacity wait this is the failed login's own reset | yes |
| `max reached` | reset | the `max_wait` deadline passed | yes |
| `capacity expired` | capacity | the `max_wait` deadline passed with no login showing capacity | no; [closing line](#closing-line) |
| `step timeout` | both | the `step_timeout` deadline passed; a fresh attempt would be killed part way | no; [closing line](#closing-line) |
| `login changed` | both | two probes in a row, 30 s apart, agree on a fingerprint that differs from the baseline | yes |
| `rotator capacity` | both | the step uses `$HOME/.claude` and the [rotate ledger](#rotate-state-directory) records a cycle with capacity (`swapped`, `stayed` or `switched`, no alert; `stayed-unread` is not capacity) whose `measured_at` is after the wait began, or, from a rotator that writes no `measured_at`, recorded more than 60 s after it | yes |
| `login capacity` | capacity | the step runs under an inherited profile with auto-switch on, and a [reading](#readings) taken after the wait began shows that another stored profile has capacity (see [Capacity for a profile step](#capacity-for-a-profile-step)) | yes, under that profile |

`rotator capacity` uses the [rotator capacity rule](#rotator-capacity-rule)
(`CycleStatus::fresh_capacity` in `amplihack_utils::rotator_status`). The
limit-resume hook no longer uses that rule; it acts only on a
[swap record](#swap-record). It is checked with the fingerprint every 30 s. A
cycle that moved onto a [credit-backed overflow profile](#rotate-overflow)
(`swapped`, `overflow=in`) or stayed on one whose probe read it in this
cycle with no active spend block, at any figures (`stayed`, `overflow=on`),
counts as capacity, so the step runs again on
credits.

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

### Capacity wait

A capacity wait covers a limit that a reset wait would skip as
`reset_too_far` or `no_reset`. It does not wait for the failed login's own
reset. It waits for any login with capacity, then runs the step once more
on that login.

#### When a capacity wait starts

The checks run after steps 1 to 5 of [Order of attempts](#order-of-attempts),
so a disabled wait, CI, a used wait and a piped prompt are refused first,
with their usual `skipped` reasons.

| Limit | Step's login | Another login could take over | Result |
|-------|--------------|-------------------------------|--------|
| Reset within five hours | any | any | reset wait, as before |
| Weekly limit, or a reset more than five hours away | `$HOME/.claude`, with a rotator running | yes | capacity wait |
| Weekly limit, or a reset more than five hours away | inherited profile, auto-switch on, with a rotator running | yes | capacity wait |
| No readable reset (`no_reset`) | `$HOME/.claude` or inherited profile with auto-switch on, with a rotator running | yes | capacity wait |
| Weekly limit or no readable reset | `$HOME/.claude` or inherited profile, no rotator running | any | `skipped reason=reset_too_far` or `no_reset` |
| Weekly limit or no readable reset | `$HOME/.claude`, a rotator running with auto-switch off | any | `skipped reason=reset_too_far` or `no_reset` |
| Weekly limit or no readable reset | any of the above | no | `skipped reason=reset_too_far` or `no_reset` |
| Weekly limit or no readable reset | inherited profile, auto-switch off | any | `skipped reason=reset_too_far` or `no_reset` |
| Weekly limit or no readable reset | a profile from the `.default` pointer that is not the rotator's live profile | any | `skipped reason=reset_too_far` or `no_reset` |
| Weekly limit or no readable reset | `$CLAUDE_CONFIG_DIR` | any | `skipped reason=reset_too_far` or `no_reset` |
| Spend limit, with a weekly reset or none | any | any | `skipped reason=reset_too_far` or `spend_limit` |

A step whose `.default` pointer names the rotator's live profile uses
`$HOME/.claude` and is treated as a `$HOME/.claude` step. Such a step's wait
can end only on a rotator cycle with capacity or a login change, so it
needs a rotator running: the built-in one (`amplihack profile rotate start`,
checked through its pid file) or the external rotator, which holds
`<root>/.rotator/lock`. With neither, the step fails at once as before, and
a reading that shows another login has capacity does not change that,
because nothing would move that login into `$HOME/.claude`. A rotator that
is running while auto-switch is off does not count: it stays idle, records
no cycle and makes no swap, so the step fails at once in that case too. The
check uses the rotator's own rule for being paused (auto-switch turned off
in the environment or in `<root>/.auto-switch`), read in the step's process.
To detect the external rotator, the check takes a non-blocking exclusive
lock on `<root>/.rotator/lock` and releases it at once, as `rotate start`
does (see [Prototype coexistence](#rotate-prototype-coexistence)). With
auto-switch off it does not touch the lock.

A step under an inherited profile needs a running rotator for the same
reason. No `/usage` probe is made, so its wait ends only on a fresh
[reading](#readings), a rotator cycle, a login change in its own directory
or its own reset (see [Capacity for a profile step](#capacity-for-a-profile-step)).
In an unattended recipe run `AMPLIHACK_RECIPE_RUN_ID` stops launches from
starting the rotator and a `claude -p` step has no status line, so with no
rotator nothing may record a reading and the wait would run to its six-hour
maximum. With no rotator running the step therefore fails at once as before.

An explicit
`$CLAUDE_CONFIG_DIR` is skipped because the rotator never changes it and it
is the login the user chose. A spend limit is skipped because it can apply
to a whole organisation, so another login of the same organisation would
not end the wait.

**Another login could take over** is one read-only check made when the
limit is seen. It never probes the network and never writes, so it makes no
request for any profile, excluded or not. It is true
when the profile store holds at least one profile that:

- is not the step's own login: the inherited profile for a profile step,
  or, for a `$HOME/.claude` step, the [rotate ledger](#rotate-state-directory)'s
  `live` profile (any stored profile counts when the ledger names none);
- is not in the auto-switch `exclude` list;
- holds its own login, as auto-switch checks it (on macOS logins are in
  the Keychain, so every profile counts);
- is a plan login, or an overflow profile that `overflow=` marks while the
  overflow gate is open; an API-key profile is never a plan login, so an
  unmarked one never counts (issue #1804);
- for the marked API-key profile, has a key that passes its login check and
  no active API-key platform block;
- has no recorded spend block.

These are the auto-switch candidate rules that can never change during a
wait, taken from the same code the selection uses
(`OverflowPool::could_select` and `rerun::login_ready`), so the check cannot
start a wait for a login auto-switch would never choose. The rules that can change are ignored here: a profile over the
threshold, or with a weekly-limit record, still counts, because it may
regain capacity before the deadline. Capacity is not required at this
point; it is what the wait looks for. A rotate ledger that records cycles
does not count by itself: on a host with one stored login the rotator has
nothing to swap in, so no wait starts.

A missing profile root, a store that cannot be opened, or any read error
(a symlink, wrong owner, a file others can write, a malformed file)
counts as false, so the step is skipped as it would have been before
capacity waits existed. For a step with no inherited profile, the profile
store is opened only when the step's login directory is `$HOME/.claude` of
the `HOME` the step runs with. A nested run with a redirected `HOME`
therefore reads the store under that `HOME`, never the real user's.

#### Capacity wait deadline

A capacity wait ends at the earliest of:

| Candidate | Applies when | `bound=` |
|-----------|--------------|----------|
| the failed login's reset + 120 s | the limit message states a reset that is still in the future | `reset+margin` |
| now + `max_wait_secs` (default `21600`, 6 hours) | always | `max_wait` |
| step start + `AMPLIHACK_STEP_TIMEOUT` − 60 s, never before now | the step timeout is set and above 0 | `step_timeout` |

A tie between the reset and the maximum is named `reset+margin`; a tie
between the maximum and the step timeout is named `max_wait`. A reset
5 h 30 m away with a six-hour maximum therefore ends the wait at 5 h 32 m
with `end reason=reset reached` and a retry, because the failed login has
recovered by then. There is no five-hour ceiling: a user file or
`AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` may set a longer maximum, and a
capacity wait honours it. A project file can only lower it (see
[Usage-limit wait settings](#usage-limit-wait-settings)). The arithmetic
saturates, so a maximum near `u64::MAX` waits until the step timeout, or
indefinitely without one, rather than overflowing.

`max_wait_secs = 0` on a capacity wait expires at once: `start`, then
`end reason=capacity expired` and the closing line, with no sleep and no
retry. The step makes one attempt and ends as a usage limit. A reset wait
with the same setting still retries once.

Reaching the `max_wait` or `step_timeout` deadline ends the wait with no
retry. The failed attempt's result is passed up after the
[closing line](#closing-line).

#### Capacity for a profile step

A `$HOME/.claude` step uses the `login changed` and `rotator capacity`
checks that a reset wait uses. A step under an inherited profile with
auto-switch on uses `login changed` and, instead of `rotator capacity`,
checks every 30 s, after the fingerprint, for a profile that auto-switch
would choose now. The check reads the stored
[readings](#readings) only and uses auto-switch's own candidate rules, so a
profile with a spend block, over the threshold, on another platform, with a
weekly-limit record or in the `exclude` list never counts. This is stricter than the
start check in [When a capacity wait starts](#when-a-capacity-wait-starts),
which also counts profiles that are over the threshold or have a
weekly-limit record. A reading counts only when
it was taken after the wait began and no more than 60 s in the future
(`start < observed_at <= now + 60`). No `/usage` probe is made: a profile
whose window has reset is only seen once a status line or the rotator
records a new reading for it, so a wait on a profile step may run until its
deadline after a login has in fact recovered.

#### Retry login

| Step's login | After `login changed` | After `rotator capacity` | After `login capacity` |
|--------------|----------------------|--------------------------|------------------------|
| `$HOME/.claude` | the live `$HOME/.claude` | the live `$HOME/.claude` | not used |
| inherited profile, auto-switch on | the profile auto-switch chooses from fresh readings, else the inherited profile | not used | the profile that showed capacity |

The retry profile is never the exhausted inherited profile when another
profile showed capacity. It is passed only as a validated profile name.
After a reset wait the retry runs under the inherited profile, as before.

When the retry after a capacity wait moves to another profile, the step's
stderr names it before the retry starts, in the same form as every other
auto-switch move:

```
amplihack: a login has capacity again; rerunning this step under profile "personal" (auto-switch)
```

#### Closing line

When a step ends on a usage limit after a wait could not lift it, the
supervisor writes one more line after the last progress line:

```
amplihack.recipe.usage_limit_wait failed reason=capacity_expired: usage limit not lifted
```

| `reason=` | Written when |
|-----------|--------------|
| `capacity_expired` | a capacity wait reached its `max_wait` deadline |
| `step_timeout` | a wait of either kind reached its `step_timeout` deadline |
| `already_waited` | the step hit a limit again after its one wait, of either kind |

The recipe runner reads the last 40 lines of a failed step as evidence. A
long wait writes a progress line every 60 s, which can push Claude's own
limit message out of that window. The closing line contains `usage limit`,
so the runner's classifier still records the failure as `usage_limit` in its
`amplihack.recipe.failure_class` marker. The usual ranking (a work marker,
then an environmental marker, then a usage limit) is unchanged. The line is
not in the shape of a limit message, so it never starts another wait. The
child's exit code is passed up unchanged, so a policy refusal still exits
`79`.

[Auto-drive](./auto-drive-to-merge.md#usage-limits-are-not-stuck) reads
that marker and stops a loop as `USAGE_LIMIT`, not `STUCK`.

### Log lines

Each line goes to stderr and starts with `amplihack.recipe.usage_limit_wait`,
then the event and `key=value` fields. Times are local,
`%Y-%m-%d %H:%M:%S %:z`. Reset-wait lines keep their fields and order; new
fields are only ever added at the end. A capacity wait's `start` line has
`wait=capacity` after `reason=`.

| Event | Fields |
|-------|--------|
| `start` (reset wait) | `reason=usage_limit harness=claude reset=<time> until=<time> bound=<bound>` |
| `start` (capacity wait) | `reason=usage_limit wait=capacity harness=claude reset=<time>\|none until=<time> bound=max_wait\|step_timeout` |
| `progress` | `remaining=<secs>s`, every 60 s |
| `end` | `reason=<end reason>` from the table above |
| `resume` | `attempt=<n>`, counting every attempt in the step |
| `skipped` | `reason=disabled\|ci\|already_waited\|not_supervised\|spend_limit\|reset_too_far\|no_reset\|no_limit_message\|piped_prompt` |
| `failed` | `reason=capacity_expired\|step_timeout\|already_waited`, followed by `: usage limit not lifted` |

A reset wait:

```
amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin
amplihack.recipe.usage_limit_wait progress remaining=1740s
amplihack.recipe.usage_limit_wait end reason=login changed
amplihack.recipe.usage_limit_wait resume attempt=2
```

A capacity wait on a weekly limit, ended by the rotator:

```
amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude reset=2026-11-05 00:00:00 -07:00 until=2026-10-31 15:43:00 -07:00 bound=max_wait
amplihack.recipe.usage_limit_wait progress remaining=21540s
amplihack.recipe.usage_limit_wait end reason=rotator capacity
amplihack.recipe.usage_limit_wait resume attempt=2
```

A capacity wait with no readable reset that no login lifted:

```
amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude reset=none until=2026-10-31 15:43:00 -07:00 bound=max_wait
amplihack.recipe.usage_limit_wait progress remaining=60s
amplihack.recipe.usage_limit_wait end reason=capacity expired
amplihack.recipe.usage_limit_wait failed reason=capacity_expired: usage limit not lifted
```

No line contains credentials, account names, profile names, e-mail
addresses, organisation IDs, config directory paths, config content or text
copied from Claude's output. Only fixed words, event and reason tokens,
times and integers are written.

When `AMPLIHACK_USAGE_WAIT_LEDGER` names an absolute path, each wait that
ends also appends its real length in whole seconds, as one line, to that
file. The file is created with mode `0600` and is never opened through a
symlink. A failed write prints a `WARNING` and does not change the step's
result. Auto-drive sets the variable so that its wall budget leaves waits
out ([Budget keys and defaults](auto-drive-to-merge.md#budget-keys-and-defaults)).

### Usage-limit wait settings

| Setting | `config.toml` key | Environment variable | Default |
|---------|-------------------|----------------------|---------|
| Enabled | `recipe.usage_limit_wait` | `AMPLIHACK_USAGE_LIMIT_WAIT` | `true`; `false` in CI |
| Maximum wait, seconds | `recipe.usage_limit_max_wait_secs` | `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | `21600` (6 hours) |

Both are ordinary [user-config](./user-config.md#recipeusage_limit_wait)
keys: set them with `amplihack config set recipe.usage_limit_wait false`, in
the user or project `config.toml`, or with the variable, which outranks the
files. The variable takes `true`, `false`, `1` or `0`; an invalid value is
reported by the config loader, by the variable's name only, and ignored.

The maximum shortens a reset wait, which never lasts beyond its reset plus
120 s, and sets the length of a capacity wait. `21600` is a default, not a
ceiling: a user `config.toml` or the variable may set a higher value, which
a capacity wait honours. A project `config.toml` may only lower the
maximum, so a repository cannot make your runs wait longer.
A capacity wait does not use the reset time, so the maximum and
`AMPLIHACK_STEP_TIMEOUT` are its only bounds. With a very large maximum and
no step timeout, the step waits until a login has capacity again, a signal
arrives or the session ends.
An invalid maximum is not an error that stops a run: `config set` rejects
it with exit code `2`, and a file or variable value is reported by the
config loader and ignored, so the next layer down applies. See
[user-config](./user-config.md#recipeusage_limit_wait).
`amplihack config get recipe.usage_limit_max_wait_secs` prints `21600` when
nothing sets it. With no setting in the environment or a file, the wait is off
in CI, meaning any `CI` that is not, after trimming, empty, `0` or `false`
in any case: a CI job has no
second login to wait for, and its own timeout bounds it better. A limit
then writes `skipped reason=ci` where a supervisor runs. Setting
`recipe.usage_limit_wait = true` in the user or project `config.toml`, or
the variable, turns it back on in CI. Settings are read only when
`AMPLIHACK_NONINTERACTIVE` is exactly `1`.

### Limit-resume hook

The [limit-resume hook](#rotate-session-resume) resumes an interactive
Claude session stopped by a limit (`StopFailure`, with `asyncRewake`) after a
rotator swap, by typing into its tmux pane or by exiting `2`. A recipe
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
| `HostsForeignAccount`: the profile's directory holds another account's login during `profile login`, `profile remove`, an explicit launch or `profile run`, or a non-joining inherited launch | `` wait for that session to end, or run `amplihack profile auto-switch status` `` |
| Default directory hosts another login and no fallback is usable, or fallback validation fails | `amplihack profile run <other>`, or `amplihack profile switch --none` with a usable ambient login; inspect `amplihack profile auto-switch status` and address any underlying cause. See [Default-profile lease fallback](#default-profile-lease-fallback). |
| `AutoSwitchConfig`: `.auto-switch` cannot be written, the change would exceed 4096 bytes, or the [settings lock](#settings-file) stayed busy for 10 seconds (the message ends in `retry`; run the command again) | `` fix what the message names and retry; do not remove <root>/.auto-switch, which holds the exclude list (see "Settings file" in docs/reference/profile-command.md) `` |
| `AutoSwitchExcludeAtRisk`: `.auto-switch` is a symlink, not a regular file, or owned by another user (no command replaces it; the message says to fix the owner with `chown` or replace the symlink with a regular file holding the same `exclude=` line), or it is a corrupt regular file of yours; in each case removing it would drop its exclude list. Either `on` or `off` is run without `--exclude` while it does not parse or others can write it (its exclude list is unknown), or `threshold`, `exclude`, `overflow add` or `overflow remove` needs to keep its other keys. The message names the recovery: `on --exclude NAME,...` or `off --exclude NAME,...` when the list is unknown, `on` or `off` when it is a private file of yours that parses. See [Unreadable exclude list](#unreadable-exclude-list) | `` do not remove <root>/.auto-switch: that drops its exclude list, and the rotator would probe and refresh the logins it excluded; see "Unreadable exclude list" in docs/reference/profile-command.md `` |
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

Auto-switch moves `.credentials.json` files and does nothing else with
them: it never opens, reads, parses, copies or refreshes them. The
[direct usage read](#direct-read) and [credential identity
reader](#credential-identity) read credentials for their respective bounded
requests. Rotation/switch ownership checks reuse the identity reader.
Auto-switch's file operations are `renameat2(RENAME_NOREPLACE)` between two directories under the
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
makes no network calls. Profile usage uses the direct usage reader;
rotation, switching, recovery and verification use the bounded credential
identity reader when they need account evidence. The status-line wrapper
runs the original command with `sh -c`, which is what Claude itself does with it; the supervisor for
reruns starts amplihack directly with no shell.

Autostart reads only the auto-switch setting, its own setting file and
`/proc`; it never opens a login file. It prints only the pid, and its status
line only `on`, `off` and the source. It refuses a root writable by others
and runs nothing as root, so another user cannot use the setting or the pid
file to start a process for you. The rotator it starts keeps the
`AMPLIHACK_ROTATE_*` values and the binary of the launch that started it;
`rotate.err` records which binary that was.

The [direct read](#direct-read) reads a profile's `.credentials.json` in
memory and uses its token only for that profile's row. It takes these
measures:

- The file is opened without following symlinks, must be a regular file
  owned by you and not writable by group or others, and is read up to
  64 KiB. A token must be 1 to 4096 bytes of printable ASCII with no space.
  Any doubt about a token leads to the `claude` fallback (or to `error`
  under `direct`), never to a weaker check.
- The usage endpoint and the token URL are HTTPS unless both are overridden
  with loopback addresses. Overrides are checked as described under
  [Direct read](#direct-read), and their warnings never repeat the value.
  Redirects are not followed, proxies are not used, a non-`200` body is
  never read, and a `200` body is read up to 64 KiB and parsed only with
  `serde_json`.
- Every error on this path becomes a fixed reason. Only the HTTP status and
  the kind of transport error are inspected, in one function, and a source
  test fails the build if `usage_direct.rs` formats an error or a token
  binding in any other way.
- A refresh is never made for `~/.claude` or for a directory a login is
  lent to or from, because a running session may own that token. It runs
  under Claude's own `.oauth_refresh.lock`, is refused while a swap journal
  exists, re-reads the file under the lock, and checks the directory and
  file before the request is sent, so a refresh that cannot be saved is
  not attempted. The new file is written with mode `0600` through a
  private temporary file in the same directory, keeping every other field.
- Tokens are not zeroed in memory; they are dropped when the read returns.

The [fallback probe](#fallback-probe) never opens, reads, logs or prints a
login file or token, and amplihack writes nothing to the profile directory
for it; `claude` may update its own files there, as any `claude` run does.
Both attempts start `claude` through these command builders:
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
rotate ledger stops the cycle, a failed live probe never moves onto credits or the API key,
a plan candidate whose probe timed out or could not run blocks the move onto
credits or the API key, and so does a plan profile skipped as `directory refused`, on every
cycle until the directory is fixed or the profile removed, a reset more than 8 days away counts as unknown, an answer to
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
rerun by the [usage-limit wait](#usage-limit-wait) while on credits
spends them. The limit-resume hook does not resume a session onto a login
whose swap record shows a plan window at 100%.
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

For session/display callers, an account is read from the `oauthAccount`
object of a `.claude.json`:
`accountUuid` (the user), `organizationUuid` (the organization) and
`emailAddress`. A value that is not a string, is over 320 bytes or contains
control characters counts as missing. The e-mail address is lower-cased, and
it also counts as missing unless the lower-cased form is still at most 320
bytes and has exactly one `@`, text on both sides of it and no whitespace.
The profile endpoint's answer (`account.uuid`, `organization.uuid` and
`account.email`), which rotation and `profile verify` read, follows the same
rules. Only these fields are read.

Every caller that reads an account through these rules sees the e-mail rule,
not only rotation: auto-switch's swap, the login checks, the duplicate-login
check, the `.default` resolution, the limit-wait fingerprint, the direct
usage read, and the check that stops a swap while Claude rewrites
`~/.claude.json` (#1702). For each of them, a login with no account UUID
and a malformed e-mail is no account, and a record keyed by a malformed
e-mail matches no login. The swap stop treats such a `~/.claude.json` as
unreadable, so the cycle stops as `home-unreadable` and nothing moves.

| Left | Right | Same account |
|------|-------|--------------|
| uuid `u`, org `o1` | uuid `u`, org `o1` | yes |
| uuid `u`, org `o1` | uuid `u`, org `o2` | no |
| uuid `u`, org `o1` | uuid `u`, no org | no |
| uuid `u`, no org | uuid `u`, no org | yes |
| different uuids | (any) | no |
| uuid on one side only | (any) | no |
| no uuid on either side | — | the e-mail addresses match, ignoring case |

Rotation and manual switching apply the same full-account comparison to
[trusted login evidence](#rotation-login-identity), rather than to session
JSON. Organization changes remain identity changes. Expected records
retain the legacy compatibility rules below. Standalone `login`, display
warnings and other auto-switch callers retain their separate behavior.

The rotate ledger retains its existing fields and adds optional
`login_identity`, populated from authorized trusted evidence. Its
`account` remains a display key; the key alone is insufficient for a
full-account ownership or recovery decision. Older ledger readers ignore
the new field after #1659; the existing compatibility projection omits it
when a strict older rotator is detected. Older binaries reject new journal fields/phases, so finish
a pending transaction with the binary that understands its journal.

When the rotator loads the ledger, a present `login_identity` must meet
the rule below. This is stricter than the session reading above: there, an
unsafe value counts as missing, whereas in the ledger it stops the rotator.
The values are the same, though: amplihack writes `login_identity` only from
an account read under the rules above, so a block it wrote always loads
again. Only a hand edit or a foreign writer can produce a block that fails.

- it is a JSON object;
- each of `accountUuid`, `organizationUuid` and `emailAddress`, where
  present, is a string that is not empty, is at most 320 bytes and contains
  no control characters (C0, DEL or C1) and no bidirectional override or
  isolate characters (U+202A to U+202E and U+2066 to U+2069); a key set to
  `null`, a number or any other non-string fails;
- `emailAddress`, where present, contains exactly one `@`, has text on both
  sides of it and contains no whitespace, and is still at most 320 bytes
  once lower-cased;
- at least one of `accountUuid` and `emailAddress` is present, so `{}` and a
  block holding only `organizationUuid` fail.

Other keys inside the object are ignored, and letter case is not checked.
The rotator rejects a ledger that breaks the rule as `RotateInconsistent`,
with the message `rotate ledger: invalid login_identity`, without echoing
the value or naming the key inside the block that failed. An absent
`login_identity`, or a top-level `"login_identity": null`, still loads. The
error is reported and the file is never overwritten: the cycle ends with the
[unreadable-ledger stop](#unreadable-ledger-stop), which reads
`rotate ledger unreadable (invalid login_identity); ...`, `profile switch`
refuses with the same line, `--dry-run` prints it after `would alert: `, and
no login moves. `profile rotate status` shows
`live: - (ledger unreadable: invalid login_identity)`. The block is never repaired, dropped
or replaced silently.

Other readers of the rotate ledger treat a refused ledger as absent, as they
do for any other unreadable ledger: `profile verify` and the profile-kind check name
no live profile, `profile usage` and `profile overflow` warn and show no
API-key or spend block, and auto-switch's takeover and overflow checks fall
back to the choices they make with no ledger. The Claude Code status line
reads the auto-switch ledger (`<root>/.auto-switch.d/ledger.json`), not this
one, and is unaffected.

To recover, stop the rotator with `amplihack profile rotate stop`. In
`<root>/.auto-switch.d/rotate/ledger.json`, remove the `login_identity` key
or set it to `null`, and keep the file at mode `0600`. Then start the
rotator again with `amplihack profile rotate start`. The block is written
again only by authorised swap completion or confirmed credential-based
recovery. Until then, ownership decisions refuse conservatively, as they do
for a legacy ledger that holds only a UUID.

### Recorded account

Each profile can have a recorded account: the account amplihack knows the
profile is meant to hold. Claude can rewrite a profile's `.claude.json`, so
the account in that file is not trusted on its own. The record is what it is
compared against.

The record is `<root>/.auto-switch.d/expected/<name>.json`:

```json
{
  "account": "<key>",
  "org": "<organization uuid>"
}
```

`<key>` is the account's uuid, or its e-mail address when it has no uuid.
`org` is the login's `organizationUuid`, or `null` when it has none. The
file holds nothing else: no token or display name. It is written
atomically, mode `0600`, owned by you and never through a symlink. It has
no `version` field.

A record is written in five cases only:

| When | What is recorded |
|------|------------------|
| [`profile login <name>`](#login-check) is accepted | The account in the profile's new `.claude.json`. A later accepted login overwrites it. Nothing is recorded when that file has no account (for example a macOS Keychain login with no `oauthAccount` block). |
| A [swap](#rotate-swap) adopts the login in `~/.claude` as `default-original[-N]` | The trusted outgoing account, only after a complete ownership check establishes the new profile may own it. A session block cannot supply identity; unreadable claims prevent adoption. |
| A [swap](#rotate-swap) completes from a profile that has no record | The trusted outgoing account, only when existing ledger/provenance and compatibility checks establish ownership and a complete trusted scan finds no other profile claiming it. Session JSON cannot backfill missing evidence. Existing restrictions after hand-change recovery still apply. |
| A rotate cycle [seeds](#seeding-a-record) a live profile with no record | The account from one fresh trusted reading, only with full ledger ownership and no other claim. |
| [`profile seed-record <name> --write`](#profile-seed-record) | The account from one fresh trusted reading of the profile you named, only when the [seeding rules](#seeding-a-record) hold. |

Nothing else writes a record, and `profile verify --fix` never does. Both seeding
paths require an account uuid and an organization, and never replace an existing
record. A swap never records the incoming (`to`)
profile and never overwrites a usable record of the outgoing one. An
adopted login replaces any record left under the new `default-original[-N]`
name by a removed profile, or deletes it when the adopted login is not
recorded. The adoption and backfill cases share one rule: an account that
another profile holds or has recorded is never recorded for a second
profile, because that other profile may be its owner. No record is ever derived
from a profile name or from `accounts/<name>.json`. A failure to write one
prints a warning and does not fail the login or the swap:

```
warning: the recorded account for profile 'work' was not saved: <reason>
```

For rotation/switch authorization, missing and unusable records are
separate states. A missing record may follow the existing adoption or
backfill rules with trusted evidence. An existing unreadable record,
unsafe mode/owner, symlink, file over 64 KiB, invalid JSON, unusable
`account`, or missing/invalid `org` makes ownership uncertain and refuses
the affected operation. A warning cannot substitute for the check. No
login moves and no ledger identity changes while that required evidence
is unusable. Standalone login and display callers retain their existing
record-reading behavior.

A record whose `account` is the login's uuid matches only when `org`
equals the login's `organizationUuid`, both being absent counting as equal:
the same user in another organization is another account, and is refused,
as under the [account identity](#account-identity) rule. A record whose
`account` is an e-mail address, written from a login without a uuid,
matches any login with that e-mail, ignoring case, and the organization is
not compared. This holds even when the current login has a uuid, so such a
record does not tell one organization from another; the account identity
rule would. A normal OAuth login has a uuid, so e-mail records are rare.

The backfill in the last row of the table is skipped when another profile
holds the outgoing account or has it recorded. A profile folder can already
hold another account's login, written by a Claude session started with
`CLAUDE_CONFIG_DIR` on that folder; recording it would make the guard
protect the wrong account. Such a profile stays unrecorded until its next
accepted `profile login` or a [seed](#seeding-a-record) that passes.

Three things read the record:

- the [duplicate-login warning](#duplicate-login-warning), to name the
  profile whose login is wrong;
- `profile usage` and `profile auto-switch status`, which print one line per
  profile whose login is not its recorded account, whether or not that
  login is also a duplicate:

  ```
  warning: profile '<name>' holds a login that is not the account recorded for it; fix: run 'amplihack profile login <name>' with the browser signed out of claude.ai or in a private window.
  ```

  The line names the profile, never an account. `auto-switch status` prints
  it among its notes, without the `warning: ` prefix;
- the [rotate swap](#rotate-swap), which refuses to move the login in
  `~/.claude` into a profile whose record names another account.

A profile with no record has no expected account to compare. Its display
warnings may use the cannot-tell form; rotation/switch still require
trusted ownership authorization. Records are created by an accepted
`profile login`, an authorized adoption/backfill or
[seeding](#seeding-a-record), as described above.

`profile remove` leaves the record in place. If the name is reused, the old
record can refuse one swap; an accepted `profile login <name>` replaces it.

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
| Claude exited 0, no `.credentials.json` before or after | The macOS Keychain case. `oauthAccount` is left as Claude wrote it, even for a duplicate: restoring the old block would make `.claude.json` disagree with the login in the Keychain. If `.claude.json` names a duplicate account, the refusal is printed but there is no file to move. If it is not a duplicate and `.claude.json` names an account, that account becomes the [recorded account](#recorded-account). | 0, or 1 for a duplicate |
| New login is not a private regular file | Nothing is moved and its mode is not changed. The error reports the mode and says the login may be exposed; the previous login stays in the `.pending-…` directory, and the hint prints that path. | 1 |
| New login duplicates another login | Refused; see below. | 1 |
| The profiles cannot be listed, or the auto-switch state cannot be opened | The check fails closed: the new login is set aside and the previous one restored as for a refusal, with `could not check profile '<name>' for duplicate logins: <reason>`. | 1 |
| New login is accepted | The previous login, if any, moves to `<name>-<time>-replaced/` and its path is printed. The empty `.pending-…` directory is removed. The new account becomes the profile's [recorded account](#recorded-account). | 0 |

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

The warning has four forms. When amplihack can tell which profile holds the
wrong login, the fix names that profile:

```
profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login <wrong>' with the browser signed out of claude.ai or in a private window.
```

When it can tell that the login in `~/.claude` is the wrong one, the fix is
a `/login` in `~/.claude`, since `profile login` is refused for the live
profile:

```
profiles '~/.claude' and '<b>' are signed in to the same account and organization (duplicate login); the login in ~/.claude is not the account recorded for profile '<live>'; fix: run plain 'claude' and '/login' to the account '<live>' is meant for, with the browser signed out of claude.ai or in a private window.
```

Otherwise it says so. For two profiles the fix names no profile:

```
profiles '<a>' and '<b>' are signed in to the same account and organization (duplicate login); amplihack cannot tell which of them holds the wrong login; fix: run 'amplihack profile login <name>' for the one that should hold a different account, with the browser signed out of claude.ai or in a private window.
```

`<name>` is printed as written; it is not replaced by a profile name. For
`~/.claude` and a profile, the fix gives both commands:

```
profiles '~/.claude' and '<b>' are signed in to the same account and organization (duplicate login); amplihack cannot tell which of them holds the wrong login; fix: run 'amplihack profile login <b>' if '<b>' should hold a different account, or plain 'claude' and '/login' if ~/.claude should, with the browser signed out of claude.ai or in a private window.
```

A profile is *mismatched* when it has a [recorded account](#recorded-account)
and its current account does not match it. The current account is the one
in the profile's own `.claude.json`, or its saved block while its directory
hosts another account. `~/.claude` has no record of its own: it is judged
by the record of the rotator's [live profile](#rotate-live-profile)
`<live>`, which is the account `~/.claude` should hold, so it is mismatched
when `<live>` has a record and the account in `$HOME/.claude.json` does not
match it. With no live profile known, `~/.claude` is not judged.

| Pair | Records | Form | `<wrong>` |
|------|---------|------|-----------|
| two profiles | exactly one is mismatched (the other matches its record or has none) | profile | the mismatched profile, whether it is `<a>` or `<b>` |
| two profiles | one matches its record and the other has none | profile | the profile without a record, whether it is `<a>` or `<b>` |
| two profiles | both match their records, both are mismatched, or neither has a record | cannot tell | none |
| `~/.claude` and a profile | `~/.claude` is mismatched; the profile matches its record or has none | `~/.claude` | none; the `/login` fix |
| `~/.claude` and a profile | the profile is mismatched; `~/.claude` matches `<live>`'s record or is not judged | profile | the profile |
| `~/.claude` and a profile | the profile has no record; `~/.claude` matches `<live>`'s record, or no live profile is known | profile | the profile |
| `~/.claude` and a profile | the profile matches its record, both are mismatched, or a live profile is known and neither has a record | cannot tell | none |

A holder whose account matches its record is taken as the rightful one,
and a holder of the same account without a record is named as the
duplicate. This rests on how records are written (see
[Recorded account](#recorded-account)): `profile login` refuses a login
another profile already holds, and a swap records an adopted login or a
backfilled one only when no other profile holds that account or has it
recorded. So a record for an account another holder also has was written
before that holder had it. The rule is the same for two profiles and for
`~/.claude`, whose record is `<live>`'s. `~/.claude` is not named when only the profile has a record,
because the `~/.claude` form states that it is not `<live>`'s recorded
account, which is unknown then.

A profile that matches its record is never named. This keeps the warning in
step with the rotator: when the login in `~/.claude` is not `<live>`'s
recorded account, the swap is refused with
`the login in ~/.claude is not the account recorded for profile <live>`,
and the warning names `~/.claude` too, not the profile that happens to hold
the same account.

For example, `rssourcelabs` and `ryan-ryansweet` both hold the
`ryan-ryansweet` account, and only `rssourcelabs` has a record that names
another account:

```
profiles 'rssourcelabs' and 'ryan-ryansweet' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login rssourcelabs' with the browser signed out of claude.ai or in a private window.
```

`auto-switch status` prints it as a [status note](#status-notes) on stdout.
`profile usage` prints it on stderr prefixed with `warning: `, after its
other warnings, and it does not change the table, the JSON or the exit
status. Both cover every profile under the root, whatever names `usage` was
given.

One line is printed per pair. Pairs of profiles follow `profile list` order;
a pair with the login in `~/.claude` comes last, with `'~/.claude'` as
`<a>` and the profile as `<b>`. The rotator's live profile is never paired
with `~/.claude`. Names are cleaned
of control characters. Neither form prints a uuid, organization, e-mail or
the recorded key.

When the auto-switch state cannot be opened, no record can be read: every
profile pair gets the cannot-tell form, and so does every pair with
`~/.claude` while a live profile is known. Without a live profile, a pair
with `~/.claude` names the profile.

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
note: api-key is billed per token on the Claude Platform API key, which is stored outside the profile directory; the rotator and auto-switch use it only if you mark it with 'amplihack profile overflow add api-key', and then only after every plan and credit profile.
```

The override alone does not put the profile to use. An `overflow=` mark that is
already on the profile does: it may have been given for a credit account with a
spend limit, and on an API-key profile the same mark is the consent to per-token
billing at tier 10. So a `kind set` that makes a marked profile an API-key
profile also prints, on stderr:

```
warning: team-credits is already marked in overflow=; on an API-key profile that mark allows per-token billing on the API key, at tier 10, with no spend limit. If you did not mean that, run: amplihack profile overflow remove team-credits
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

For a [default-profile lease fallback](#default-profile-lease-fallback), an
authenticated Keychain login also counts as the profile's own OAuth login,
subject to the same identity and billing safeguards. The exception requires
fresh authenticated identity and protected storage continuity; a missing
file or displayed account alone never grants it.

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
| `overflow add` | Marks the profile for use as tier 10, the last tier; see [API-key rotation](#api-key-rotation). Prints `overflow: added <name> (api-key tier, tier 10)` and a cost note. `--tier` is refused with `PlatformTierFixed`; see [API-key errors](#api-key-errors). |
| `overflow remove` | Removes the mark. From the next cycle the profile is never moved onto; if it is live, the rotator moves off it by the [exit rules](#moving-off-the-api-key). |
| `overflow list` | After the marked credit profiles, one line per API-key profile: `<name>  api-key tier  marked`, or `<name>  api-key tier  not marked (not used)` when it is not named in `overflow=`, then `blocked until <time> (<reason>)` while the [API-key block](#api-key-block) is active, or `not used (<first> comes first)` for each marked API-key profile after the first by name. |
| `overflow clear --platform` | Clears the [API-key block](#api-key-block). |
| Duplicate-login check | Skips every API-key profile: they all share one credential by design. |

### API-key usage

`profile usage` gives an API-key profile a row with no reading. Its `STATE`
is `api_key`. In the [bar view](#bar-output) the cells from `WEEK` to
`S-RESET` are replaced by `api-key (billed per token)`; in `--table` its
`SESSION` cell is `api-key (billed per token)` and the other usage cells are
`-`. The row is marked live (`(live)` after the name in the bar view,
`live in ~/.claude` in `--table`) when its login file is live, and has the
mark `blocked until <time> (<reason>)` while the
[API-key block](#api-key-block) is active. The row never shows `unparsed`,
`timeout` or `failed`. API-key rows sort after every usable and exhausted
plan row and before the rows with no reading.

Below the rows, `usage` prints
`live: work is running on the API key (billed per token)` when an API-key
profile is live in `~/.claude`, and `blocked: the API key until <time>
(<reason>)` while a block is active:

```
$ amplihack profile usage
PROFILE         WEEK             W-RESET      SESSION           S-RESET     STATE
ryan-mistt      ██████░░░░  64%  11-01 02:00  ██████████ 100%   10-28 16:50 session_exhausted
ryan-ryansweet  ███████░░░  71%  11-01 02:00  ██████████ 100%   10-28 17:20 session_exhausted
api-key (live)  api-key (billed per token)                                  api_key
live: work is running on the API key (billed per token)
times: Europe/London
order: rotator pick (most room in the worse of session and week, then name)
```

The same run with `--table`:

```
$ amplihack profile usage --table
PROFILE         MARKS              STATE              SESSION                     SESSION RESETS                  WEEK  WEEK RESETS                 SPEND
ryan-mistt      -                  session_exhausted  100%                        Oct 28, 4:50pm (Europe/London)  64%   Nov 1, 2am (Europe/London)  n/a
ryan-ryansweet  -                  session_exhausted  100%                        Oct 28, 5:20pm (Europe/London)  71%   Nov 1, 2am (Europe/London)  n/a
api-key         live in ~/.claude  api_key            api-key (billed per token)  -                               -     -                           n/a
live: work is running on the API key (billed per token)
order: rotator pick (most room in the worse of session and week, then name)
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

An API-key profile is used by the rotator and auto-switch only after you
mark it with [`profile overflow add <name>`](#overflow-commands). The mark
is your consent to per-token billing. An API-key profile that is not marked
is never moved into `~/.claude` and never chosen for a launch or a rerun,
whatever the readings; `overflow list` shows it as
`api-key tier  not marked (not used)`. An API-key profile also never counts
as a plan login, marked or not: it has no usage reading, so it adds no plan
capacity or room, and the capacity notice can report `low` or `exhausted`
with one present. A [kind override](#kind-override) of
`api-key` does not mark a profile, and neither does its name, though it does
change what a mark [already on the profile](#kind-override) allows. There is no
other switch: "overflow on" means that at least one profile is named in
`overflow=`, and the API-key profile is used only when it is one of them and
the [gate](#earliest-plan-reset) allows the move.

If `.auto-switch` cannot be read while `AMPLIHACK_CLAUDE_AUTO_SWITCH=1` keeps
auto-switch on, kinds are unknown. Auto-switch then selects no profile and
writes no capacity file until the file is fixed; a capacity file already
there is left as it is. The same holds when the profile root cannot be
listed.

A session you start on an unmarked API-key profile does not count as
overflow, so auto-switch never moves it onto another API-key profile. A key
reports no rate limits, so the status line never moves it off the key
either. Only a nested launch or a usage-limit rerun from it can move to a
plan profile or a marked credit profile. Otherwise the session stays until
you move it.

```bash
amplihack profile overflow add api-key
```

A marked API-key profile is always tier 10, below every credit
[overflow tier](#overflow-tiers), whatever tier `.auto-switch` holds for it.
The full order is in [Rotate decision order](#rotate-decision-order):

1. plan (subscription) logins with capacity;
2. plan logins with [room](#rotate-plan-room-swap) below 100% on both
   windows, the live one included;
3. only when no plan login has room: marked credit overflow profiles, tiers
   1 to 9;
4. then the marked API-key profile, tier 10.

`overflow add` stores the mark as the bare name, for example
`overflow=ryan-mistt-org,api-key`, and reads it as tier 10. A hand-written
`api-key:3` is read as the same mark, at tier 10. When more than one API-key profile is marked, the first by name
is used and the others are listed as `not used (<first> comes first)`. When
`.auto-switch` is corrupt, no API-key profile counts as marked.

An unmarked API-key profile is left out of the [candidates](#rotate-candidates):
it is never probed, never counts as plan capacity or plan room and never
takes part in the headroom comparison. The rotator logs one line per process
when one exists:

```
skip: API-key profile api-key is not marked; it is never used. To allow per-token billing, run: amplihack profile overflow add api-key
```

A mark given for a credit account stays in `overflow=` if the profile's kind
later reads as API-key: from `kind set`, which warns, or by detection, when its
login file has no OAuth login. A detected kind has no command to warn from, so
the rotator logs one line per process:

```
warning: api-key is marked in overflow= and detected as an API-key profile; that mark allows per-token billing on the API key, at tier 10, with no spend limit. If you did not mean that, run: amplihack profile overflow remove api-key
```

If an unmarked API-key profile is already live in `~/.claude`, for example
after a hand change or an upgrade from a release that used the API key
without a mark, the rotator reports it as `platform_not_marked` and treats it
as live on the API key for the rules that move work **off** it, below. It
moves off as soon as a plan login has room, never moves work back **onto**
it, and never writes a mark for you. It does not probe the profile or give it
a reading.

A marked API-key profile is never probed with `/usage`. Its reading is usable
unless the API key is blocked. It never counts as plan capacity and never
takes part in the headroom comparison. The `exclude` list and the
[candidate](#rotate-candidates) skip reasons apply to it, except
`no account`.

#### Moving onto the API key

The rotator swaps `~/.claude` onto the API-key profile, with
`reason=overflow-in`, only when all of these hold:

- the profile is marked in `overflow=` and `.auto-switch` can be read;
- no plan login has *room*: every plan login with a measured reading, the
  live one included, is at 100% session or 100% week. A plan login whose
  probe failed, that was not measured, or that reads exhausted has no room.
  This is stricter than [plan capacity](#plan-pool): a plan login below
  100% on both windows is used by the
  [plan-room swap](#rotate-plan-room-swap) instead;
- no credit overflow profile can be used: each is blocked, excluded, not a
  candidate or failed its probe;
- the [earliest plan reset](#earliest-plan-reset) is further away than
  `--overflow-wait`, or unknown with `--overflow-unknown-reset 1`;
- the API key is not [blocked](#api-key-block);
- the live probe did not fail;
- the [API-key login check](#api-key-login-check) passes.

A failed live probe never moves work onto the API key: the cycle follows
the [failed-probe rules](#rotate-failed-live-probes) as without overflow.
Failed or missing probes on other plan logins count as no room, so an outage
in which every probe fails could otherwise satisfy the room condition. The
[earliest plan reset](#earliest-plan-reset) gate prevents this: with every
probe failed, no reset is known, and an unknown reset never opens per-token
billing unless `--overflow-unknown-reset 1` is set.

If the reset falls inside the wait, the rotator waits, as for credits. If
any other condition fails, the cycle ends as it would with no API-key
profile: `ALERT no-capacity`, or a stay.

#### Moving off the API key

While the API-key profile is live, these rules apply in order on every
cycle. `--hold`, `--margin` and `--upgrade-margin` do not apply to them:

| Condition | Outcome |
|-----------|---------|
| A plan candidate has capacity | Swap to the plan candidate with capacity that has the most headroom, `reason=overflow-out`. A failed plan probe is not capacity. |
| No plan candidate has capacity, and a plan candidate has room | Swap to the plan login the [plan-room ranking](#rotate-plan-room-swap) puts first, `reason=overflow-out`. A plan login below 100% costs nothing. |
| No plan candidate has room, and a credit overflow candidate can be used | Swap to the first by tier, `reason=overflow-out`. Credits cost less than the API key. |
| The API key is blocked, and a plan or credit candidate can be used | Swap to the best one: a plan candidate that is not exhausted and whose probe did not fail, then a credit candidate. |
| The API key is blocked, and nothing can be used | One ALERT and stay. No swap is attempted. |
| Otherwise | Stay on the API key. |

The same rules apply to an unmarked API-key profile found live in
`~/.claude`.

From a credit profile the [overflow rules](#rotate-overflow) apply
unchanged; the rotator never moves from credits to the API key while a
credit profile can be used, or from a plan login to credits or the API key
while a plan login has room.

#### API-key login check

Before every swap onto the API-key profile the rotator checks that a key is
where Claude will read it after the swap. The check runs in step 6 of the
[cycle](#rotate-cycle), under the rotate state lock, after
[revalidation](#rotate-revalidation) and immediately before the journal is
written. A swap moves only `.credentials.json`, and plain `claude` reads
`~/.claude` and `~/.claude.json`, so the check reads those paths, not the
profile's directory. It passes when one of these is a non-empty string after
trimming:

| Setting | File | Detail logged on success |
|---------|------|--------------------------|
| `primaryApiKey` | `~/.claude.json` | `primaryApiKey set` |
| `ANTHROPIC_API_KEY` under `env` | `~/.claude/settings.json` | `ANTHROPIC_API_KEY set` |
| `apiKeyHelper` | `~/.claude/settings.json` | `apiKeyHelper set` |

The check reads those files with the same rules as
[kind detection](#kind-detection): no symlinks, a size limit, and the file
must belong to you and not be writable by group or others. An empty,
whitespace, number, `null` or object value does not pass, and neither does
a file that cannot be parsed or is not a JSON object.

A login file alone is not enough. A `.credentials.json` with no OAuth login,
`{}` included, does not pass without one of the three settings, and nor does
a [kind override](#kind-override) of `api-key`. Claude Code keeps an API key
in `primaryApiKey` or in `settings.json`, not in `.credentials.json`, so a
`~/.claude` that has only that login file would start Claude as "not logged
in".

A key in the profile's own `.claude.json` or `settings.json` does not pass
either. The swap never copies those files or their keys, by design, so the
key would be left behind in the profile's directory and the session would be
"not logged in". That key is used only by a run with `CLAUDE_CONFIG_DIR` set
to the profile, which is how [auto-switch](#api-key-profiles-and-auto-switch) runs
it. Its own check reads the profile's files for that reason.

For the rotator to use an API-key profile, the key must therefore already be
in `~/.claude.json` or `~/.claude/settings.json`. That key is the one billed,
even if the profile names a different one.

A key in those files is host-wide, and every accepted setting is used ahead
of a plan login. Claude Code 2.1.316 was run in a throwaway `$HOME` holding a
fake OAuth login and each setting in turn. The last column comes from sending
a prompt to a local fake endpoint through `ANTHROPIC_BASE_URL`; the other
columns come from runs without that variable.

| Setting in `$HOME` | `claude auth status` | `get_usage` `rate_limits_available` | `claude -p /usage` | Credential sent with a prompt |
|--------------------|----------------------|-------------------------------------|--------------------|-------------------------------|
| `ANTHROPIC_API_KEY` under `env` | `authMethod: api_key` | `false` | cost text | The key, in `x-api-key` |
| `apiKeyHelper` | `authMethod: api_key_helper` | `false` | cost text | The key, in `x-api-key` and `Authorization` |
| `primaryApiKey` | `authMethod: claude.ai`, `apiKeySource: /login managed key` | `true` | cost text | The key, in `x-api-key` |

With no setting, `auth status` reports `authMethod: claude.ai`,
`rate_limits_available` is `true` and `/usage` prints no cost text.
`primaryApiKey` is the case to watch: `auth status` and `get_usage` both treat
the login as a subscription while the request carries the key.

The `get_usage` column has a limit. With a fake login no figures came back:
`rate_limits` was `null` in every row, so the probe went on to `/usage`.
Claude's schema describes `rate_limits_available: false` as "plan rate limits
do not apply", and with `true` it reads the plan's usage endpoint with the
OAuth login. So with a working login and `primaryApiKey`, figures are expected
as for a plain plan login. No run with a real login has confirmed this, so
both outcomes below are covered by tests.

So, while a plan login is live and one of these is present:

- Every session bills per token. A mark does not change this, and neither
  does plan-first order, which is decided on plan readings.
- What the live probe reads depends on the setting:
  - With `ANTHROPIC_API_KEY` under `env` or `apiKeyHelper`, `get_usage` gives
    no figures and `/usage` prints cost text, which is no reading. The live
    login reads as a failed probe on every cycle.
  - With `primaryApiKey`, an answer with figures is a normal reading, so the
    live login is treated as a plan login. The rotator moves off it at its
    thresholds as usual, including onto a marked API key, and the two rules
    below do not apply. The moves change nothing that is billed. If
    `get_usage` gives no figures, or the account has usage credits on, the
    probe falls back to `/usage`, and the login reads as a failed probe on
    that cycle.
  - With `AMPLIHACK_PROFILE_USAGE_PROBE=text`, `get_usage` does not run, so every
    setting reads as a failed probe.

On a cycle with a failed live reading:

- The rotator does not try the per-token tier
  ([decision order](#rotate-decision-order)). That loses nothing: Claude is
  already using the key, and the move would not change what is billed.
- The rotator makes no [failed-probe swap](#rotate-failed-live-probes),
  including one the
  [near-threshold rule](#failed-or-stale-reading-near-the-threshold) would
  make early. The other logins still read normally, because each is probed through its own
  directory and not these files. But the key applies to whichever
  login is live, so the login swapped in would fail its live probe the same
  way, and the billing would not change. The rotator stays on the live login
  and logs `stay: live probe failed; home key (<detail>) applies to every
  login once live, a swap changes nothing`. Swaps resume on the first cycle
  after the key is removed, because the live probe then reads normally.

No setting avoids this, so the rotator accepts these effects and says so. It
logs `ALERT: home_key (<detail>): ...` once when it finds a key while a plan
login is live, and again if the setting changes. The cycle carries on, because
no move fixes it. `profile overflow add` prints the same text as a warning. A
key in the rotator's own process environment gets the `profile usage` warning
instead.

The failed-probe stay uses the check above: it needs a key in a file that
check accepts. With a key only in a file the check rejects, the swap goes
ahead. It may be useless, but a stay on a login that is really dead would be
worse.

The ALERT and the warning do not use the check above. That check decides
whether a move is allowed, so it refuses a symlink, a file others can write
and a file it cannot parse. Claude may still read such a
file, so a warning that money is being spent cannot depend on those rules. A
separate read of the same two files follows a symlink, ignores owner and mode,
and lets neither file hide the other. It decides nothing and returns only the
setting's name. A file it cannot open or parse, or one over 32 MiB, counts as
holding no key. It has not been shown that Claude reads a symlinked settings
file: the Claude 2.1.316 results cover regular files only, so with a symlinked
file the warning may be a false alarm.

A host that bills only plan logins should leave those files without a key.
The rotator then refuses every move onto the API-key profile, which is the
outcome such a host wants.

The check makes no network request and does not run `apiKeyHelper`, so it
costs nothing. It shows that a key is configured, not that the key works;
an invalid or unfunded key is caught later by the [API-key block](#api-key-block).
The key value is never logged, printed, stored in the ledger or shown by
`status`: the detail is one of the fixed strings above, and a failure reason
names only the setting or the file.

When the check fails:

- no swap is made, no journal is written and `~/.claude` is unchanged;
- no `per-token`, `swap` or `moved` line is written, and the ledger records
  no `overflow=in` for the cycle;
- the cycle logs `key_check_refused` and an ALERT, and ends as `alerted`:

```
key_check_refused api-key: no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting in ~/.claude.json or ~/.claude/settings.json
ALERT: API key profile api-key failed its login check (no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting in ~/.claude.json or ~/.claude/settings.json); not moving onto it
```

The cycle does not try another profile in its place: the API-key profile is
the last tier, so nothing is left to try. The ALERT is keyed by the profile,
so a rotator running every 120 seconds writes it once, not every cycle. It is
written again only when the stored ALERT key changes or is cleared: after a
different ALERT has been written in between, or after a cycle that swaps or
records `stayed`, since such a cycle clears the key. A `stayed-unread` cycle
leaves the key in place. Other failure reasons are `settings.json is a symlink`,
`settings.json is writable by group or others`,
`settings.json is not owned by you`, `settings.json is too large`,
`settings.json cannot be parsed`, and the same for `.claude.json`. Each
reason is fixed text and never holds file content.

`--dry-run` runs the same check. The check only reads files, so the dry run
needs no lock for it. A failed check prints

```
would_refuse api-key: no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting
```

in place of the `would swap` line; a passed check prints the `would swap`
line as usual.

The check runs moments before the swap, not at the same instant. A file
changed in between is not seen until the next cycle. Only you can write
those files, since a file others can write fails the check, so no second
check is made.

#### How the swap works

The API-key profile's `{}` login file is moved into `~/.claude` by the same
[journalled rename swap](#rotate-swap) as any login, and the outgoing login
goes back to its own profile. Nothing is copied, an interrupted swap is
recovered from the journal, and `~/.claude` always holds a login file. A
profile with no login file cannot be swapped in; no empty file is created
for it. Auto-switch passes over such a profile too, so the capacity file does
not count it as usable overflow, and `profile overflow add` warns about it. A
profile the rotator has already made live has its login file in `~/.claude`
and still counts.

Before the journal is written, the swap is refused when it would leave
`~/.claude` without an OAuth login and the incoming profile does not resolve
as `api-key`. A refusal leaves no journal, renames nothing and logs:

```
ALERT: swap refused: ryan-ryansweet has no oauth login and is not an API-key profile
```

#### Log lines for a move onto the API key

Every move onto a per-token profile writes one `per-token` line first. It
names, in order, the rule that allowed the move, the gate result, the mark
and the login check. The ordinary [swap line](#rotate-log) and the `moved`
line follow it:

```
per-token api-key: no measured plan login below 100%, 0 not measured; earliest reset 2026-11-01T02:14:00Z beyond overflow_wait; marked overflow tier 10; login check passed (primaryApiKey set)
swap iivanovich->api-key live s=100% w=61% target s=0% w=0% reason=overflow-in
moved ~/.claude onto API key profile api-key (billed per token)
```

With `--overflow-unknown-reset 1` and no known reset, the gate part reads
`reset unknown`. A move onto a credit profile writes the same line, with the
credit profile's tier and `probe ok, no spend limit` in place of the login
check:

```
per-token team-credits: no measured plan login below 100%, 0 not measured; earliest reset 2026-11-01T02:14:00Z beyond overflow_wait; marked overflow tier 2; probe ok, no spend limit
```

A move off the API key onto a credit profile is tagged `overflow-out`, but it
still bills per token, so it writes the line too. That move passes no wait
gate, and the line says why it was made:

```
per-token team-credits: no plan login took the move; leaving the API key, credits cost less; marked overflow tier 2; probe ok, no spend limit
```

The line holds profile names, a time, a tier and fixed text only, never a
key value or a helper command.

A swap off the API key logs:

```
moved ~/.claude off API key profile api-key to ryan-mistt
```

### Several API-key profiles

All API-key profiles share one credential, so they form one pool: the
rotator and auto-switch use the first marked one by name and never move from one
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

`rotate status` also prints an `api key:` line: the API-key profile the tier
uses, followed by ` (marked)` or ` (not marked)`, and the active
[block](#api-key-block) when there is one; `-` when there is no API-key
profile. When the live login is an API-key profile that is not marked, the
line ends with ` platform_not_marked`:

```
api key: api-key (not marked)
api key: api-key (marked)
api key: api-key (marked) (blocked until 2026-12-29T10:14:03Z, key_exhausted)
api key: api-key (not marked) platform_not_marked
```

The `overflow:` line lists every marked profile, credit and API key, and
reads `overflow: -` when nothing is marked.

The ledger is the only evidence; nothing is probed. The `on overflow` line
reads `yes (<name>)` while an API-key profile is live. A block is shown by
`profile usage` and `overflow list`.

### API-key log lines

Every line the rotator writes to the [rotate log](#rotate-log) about the
API-key tier. Each ALERT is written once per change of state.

| Line | When |
|------|------|
| `per-token <name>: no measured plan login below 100%, 0 not measured; <gate>; marked overflow tier 10; login check passed (<detail>)` | Before the swap line of a move onto the API-key profile. See [Log lines for a move onto the API key](#log-lines-for-a-move-onto-the-api-key). |
| `moved ~/.claude onto API key profile <name> (billed per token)` | After a swap onto the API-key profile. |
| `moved ~/.claude off API key profile <name> to <target>` | After a swap off it. |
| `ALERT: the API key is blocked (<reason>) until <time>; API-key profiles are not used` | A new [API-key block](#api-key-block). |
| `ALERT: the API key is blocked (<reason>) and no plan or credit profile can be used; staying on <name>` | The live API-key profile is blocked and there is nowhere to go. |
| `ALERT: more than one API-key profile could be live in ~/.claude; no swap` | The live API-key profile is [ambiguous](#several-api-key-profiles). |
| `ALERT: swap refused: <name> has no oauth login and is not an API-key profile` | The swap check refused a target. |
| `skip: API-key profile <name> is not marked; it is never used. To allow per-token billing, run: amplihack profile overflow add <name>` | An API-key profile exists and is not named in `overflow=`; once per rotator process. |
| `skip: API-key profile <name> is marked but not used; another comes first by name` | More than one API-key profile is marked; written for each one after the first by name, once per rotator process. |
| `key_check_refused <name>: <reason>` | The [login check](#api-key-login-check) failed; no swap. |
| `ALERT: home_key (<detail>): <effect>` | A key in the `$HOME` files is billing plan sessions per token; see the [login check](#api-key-login-check). |
| `ALERT: API key profile <name> failed its login check (<reason>); not moving onto it` | Written with the refusal, once per profile until another ALERT or a passing check. |
| `platform block not recorded: <error kind>` | A session saw a key error but could not take the state lock. |

### API-key ledger fields

The [rotate ledger](#rotate-state-directory) keeps `version: 1` and gains
these optional fields. Older ledgers load unchanged.

| Field | Contents |
|-------|----------|
| `live_kind` | `"platform"` while `live` is an API-key profile; absent means subscription. Only present with `live`. The rotator checks and repairs it on every cycle; it is kept for readers, such as the limit-resume hook, that cannot detect kinds. |
| `platform_block` | `{"reason", "at", "until"}`, with `reason` one of `key_exhausted`, `key_revoked` or `key_rate_limited`, and `0 < at < until <= at + 62 days`; absent when there is no block. Expired blocks are removed the next time the rotator records a cycle. |
| `no_room` | `{"at", "until", "profiles"}`: the profiles the last measuring cycle found with no room (a reached limit, or a window at 100%), sorted, never the API-key profile and never one whose probe failed. `at` is when the cycle measured; `until` is two `--interval`s plus the cycle's own length after the record was written, so a slow cycle's record is still in force when the next one arrives, and at most one hour after `at`. It is earlier when a profile the record names has a known reset before then: the whole record ends at the earliest reset of a full window, or the later reset of a profile with both windows full, because that profile has room again. A profile no longer named is unread, so auto-switch holds the API key back until the next record. Written every measuring cycle whatever the capacity state; auto-switch reads it while `at <= now <= until`, for profiles it has no reading of. |

The limit-resume hook reads `live_kind` and `platform_block` without failing on a missing or
wrongly typed field, and treats a live API-key profile as on the API key,
not as plan capacity.

### API-key profiles and auto-switch

Auto-switch uses the same tier order, wait gate and move-back rules as the
rotator, taken from the same function, so the two agree on which profile is
the API-key candidate. An API-key profile that is not marked with
`overflow add` is left out of that order and never chosen. Auto-switch never
chooses a credit or API-key profile while any plan profile, the current one
included, has room below 100% on both windows.

Before it records a move onto the API-key profile, auto-switch runs the
[login check](#api-key-login-check) against the profile's own `.claude.json`
and `settings.json`, not the `~/` files, because the launch or rerun has
`CLAUDE_CONFIG_DIR` set to that profile. When the check fails, nothing is moved,
no journal entry is written, and the launch or rerun goes ahead on its
current profile, or on the plan profile it would choose if the current one
cannot be used. It never falls through to another credit or API-key profile
in place of the refused one. A warning naming the profile and the reason is
written to the auto-switch log.

The platform login is never lent to another profile's session, and an
API-key profile's directory never hosts another account's login, because
the credential is shared across the home directory. Auto-switch therefore
uses an API-key profile only for a new nested launch or a usage-limit rerun,
which runs in that profile's own directory, and never swaps it into a
running session. A session started in the API-key profile's directory is
not moved mid-session; once plan capacity returns, its next nested launch or
rerun goes back to a plan profile. Sessions in `~/.claude` leave the API key
when the rotator moves the login.

### Upgrading from the implicit API-key tier

This release changes two things that a running host notices.

**The API key needs a mark.** Earlier releases used every API-key profile as
the last tier without a mark. After the upgrade an API-key profile is used
only once it is marked. A host that relied on the implicit tier keeps its
API-key profile unused until you run:

```bash
amplihack profile overflow add api-key
```

Until then `overflow list` shows `api-key  api-key tier  not marked (not used)`,
`rotate status` shows `api key: api-key (not marked)`, and the rotator writes
the `skip:` line in [API-key log lines](#api-key-log-lines) once per process.
Nothing is marked automatically, no migration runs, and `.auto-switch` is
not changed by the upgrade.

An unmarked API-key profile that is live in `~/.claude` at the upgrade stays
live under the [exit rules](#moving-off-the-api-key), and keeps billing per
token, until a plan login has capacity or room, or a credit overflow profile
can be used. Once the rotator moves off it, it never moves back onto it.
`rotate status` reports it as `platform_not_marked` meanwhile. If you do not
want per-token billing in the meantime, either mark it with `overflow add`
to accept it, or move off it by hand with [`profile switch`](#rotate-switch)
to a plan login.

**Credits start later.** Earlier releases moved onto a credit overflow
profile once every plan login had reached `--session-at` or `--week-at`.
Credit profiles are billed too, so they now wait for the same condition as
the API key: no plan login, the live one included, below 100% on both
windows. Until then the [plan-room swap](#rotate-plan-room-swap) uses the
plan login with the most room.

### API-key errors

| Condition | Message | Hint |
|-----------|---------|------|
| `SubscriptionNoLogin`: `run`, `claude`, `launch` or `switch` named a subscription profile with no OAuth login of its own that is not held | `profile <name> is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. ...` (in full under [No silent billing](#no-silent-billing)) | `amplihack profile login <name>, or: amplihack profile kind set <name> api-key` |
| `PlatformTierFixed`: `overflow add --tier N` named an API-key profile | `profile <name> is an API-key profile; the API-key tier is fixed at 10, so --tier cannot be set` | `` run `amplihack profile overflow add <name>` without --tier `` |
| `PlatformBlocked`: `switch` named an API-key profile while the API-key block is active | `profile <name>: the API key is blocked (<reason>) until <time>` | `amplihack profile overflow clear --platform` |
| `AutoSwitchConfig` or `AutoSwitchExcludeAtRisk` from `kind set`: `.auto-switch` is busy, or is corrupt, a symlink or another user's | as for the other [settings commands](#settings-file) | as for the other settings commands |

Messages hold profile names, kinds, reasons and times only. No message, log
line, ALERT or ledger field holds a byte read from a login, settings or
`.claude.json` file.
