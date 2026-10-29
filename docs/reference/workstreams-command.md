# Reference: `amplihack workstreams`

Print a plain-text table of the workstreams the calling session started and
the capacity of every Claude login, for an agent to paste into its reply. The
[`workstream-status` skill](#skill) tells agents how to use it.

## Contents

- [Synopsis](#synopsis)
- [Flags](#flags)
- [Output](#output)
  - [Workstreams section](#workstreams-section)
  - [Logins section](#logins-section)
  - [Width and characters](#width-and-characters)
- [Which runs are shown](#which-runs-are-shown)
  - [Caller identity](#caller-identity)
  - [Descendants](#descendants)
  - [Runs without a record](#runs-without-a-record)
  - [Auto-drive rounds](#auto-drive-rounds)
  - [Log directory warnings](#log-directory-warnings)
- [Stages and progress](#stages-and-progress)
  - [Finding the PR](#finding-the-pr)
  - [Running or stopped](#running-or-stopped)
  - [Elapsed time](#elapsed-time)
- [Logins](#logins)
- [Manifest](#manifest)
- [Time limits](#time-limits)
- [Exit codes](#exit-codes)
- [Security](#security)
- [Skill](#skill)
- [Related](#related)

---

## Synopsis

```
amplihack workstreams status [--all] [--no-logins] [--manifest <PATH>] [--logins-deadline <SECS>]
```

The output is meant for people. It is not a machine interface: columns,
wording and order may change. Scripts should read
[`profile usage --json`](./profile-command.md#json-output) and the
[run records](./recipe-command.md#run-records) instead.

## Flags

| Flag | Default | Effect |
|------|---------|--------|
| `--all` | off | Show every run on this host, not only the caller's. See [Which runs are shown](#which-runs-are-shown). |
| `--no-logins` | off | Leave out the `LOGINS` section. No usage probe is started. |
| `--manifest <PATH>` | `~/.amplihack/work/workstreams.tsv` | Read labels and queued items from this file. See [Manifest](#manifest). |
| `--logins-deadline <SECS>` | `8` | Seconds the `LOGINS` section may take, from `1` to `60`. |

A deadline outside `1` to `60`, or `--manifest` without a value, is a usage
error.

## Output

```console
$ amplihack workstreams status
WORKSTREAMS
▶ workstream-status skill #1710 #1715 ███████░░░  70% 2h05m merge-ready round 1
▶ rotator manual login    #1693 #1702 █████░░░░░  45% 3h12m review round 2, 3 concerns
⏸ limit-resume wake       #1698 -     ██░░░░░░░░  15% 0h48m building, stopped on a usage limit
⏸ hook payload fixes      #1700 #1704 ███░░░░░░░  30% 5h40m PR open, conflicts, finished
⏸ docs sweep              #1720 -     ░░░░░░░░░░   0% -     queued

LOGINS
  PROFILE          SESSION           WEEK              STATE    RESETS
  work*            ███░░░░░░░  31%   ██████░░░░  58%   ok       Oct 28, 4:50pm
  personal         █████████░  92%   ███░░░░░░░  34%   ok       Oct 28, 6pm
  spare            no reading                          timeout
  platform         API key: billed per token, no plan limits
* live in ~/.claude
```

The workstream columns are marker, label, issue, PR, progress, elapsed time
and stage. Each column is as wide as its widest cell, with one space between
columns, so the stage keeps as much of the 100 columns as possible.

A section with no rows prints `none` under its title.

### Workstreams section

One row per workstream:

| Column | Content |
|--------|---------|
| marker | `▶` running, `⏸` waiting, stopped or finished. See [Running or stopped](#running-or-stopped). |
| label | The [manifest](#manifest) label; otherwise `<recipe> #<issue>`; otherwise the tmux session name. Cut to 24 characters. |
| issue | `#<issue>`, or `-` when the run has none. |
| PR | `#<pr>`, `-` when there is none, or `#?` when GitHub could not be asked. |
| progress | A 10-cell bar and the percentage from [Stages and progress](#stages-and-progress). |
| elapsed | See [Elapsed time](#elapsed-time). |
| stage | The stage, then `N concerns`, `conflicts` and the stop reason when they apply, separated by commas. Cut to fit 100 columns. |

Rows are ordered by start time, oldest first. Queued manifest rows come last,
in manifest order.

When the caller has no [identity](#caller-identity) and `--all` is not given,
the section shows only manifest rows and this line:

```
no session id found; run with --all to see every run on this host
```

### Logins section

One row per profile under the [profile root](./profile-command.md#profile-root):

| Column | Content |
|--------|---------|
| `PROFILE` | The profile name, with `*` when its login is live in `~/.claude`. |
| `SESSION` | A 10-cell bar and the current-session percentage. |
| `WEEK` | A 10-cell bar and the all-models weekly percentage. |
| `STATE` | The state `amplihack profile usage` reports: `ok`, `timeout`, `unparsed`, and so on. |
| `RESETS` | The session reset, or the weekly reset when the session has none, as `claude` printed it without the time zone. `-` when neither is known. |

- A profile whose probe did not finish before the deadline, or whose state is
  `timeout` or `unparsed`, shows `no reading` instead of bars.
- An [API-key profile](#logins) shows `API key: billed per token, no plan
  limits` and is not probed.
- A credit-backed overflow profile (see `amplihack profile overflow`) shows
  `overflow: paid credits, no plan limits` instead of bars.
- When `~/.claude` holds a login that no profile holds, a `~/.claude*` row is
  added last.
- The legend line `* live in ~/.claude` follows the rows when any row is
  marked.

### Width and characters

- No line is longer than 100 characters.
- The output has no ANSI escape sequences and no control characters.
- Apart from ASCII, only these five single-width characters appear:
  `█ ░ ▶ ⏸ …`. A cut field ends with `…`.
- Labels, stage text and profile names are cleaned before they are cut or
  padded: any character outside printable ASCII becomes `?`.

## Which runs are shown

The command reads the run records in
[`recipe.log_dir`](./user-config.md#recipelog_dir) (default
`~/.amplihack/recipe-logs`). Each detached run writes a
[`<session>.run.json`](./recipe-command.md#run-records) when it starts and
updates it when it ends. The record names the session that launched the run
in its `parent` field.

| View | Rows |
|------|------|
| default | Runs launched by the caller, their [descendants](#descendants), and every manifest row. |
| `--all` | Every run with a record or a log in the log directory, and every manifest row. |

Rows are matched to the caller by recorded identity only, never by timing or
working directory.

### Caller identity

The caller's identity is the first of these variables that is set, non-empty,
at most 128 bytes and printable ASCII:

| Order | Variable | Identity |
|-------|----------|----------|
| 1 | `AMPLIHACK_RUN_ID` | `run:<value>` |
| 2 | `AMPLIHACK_SESSION_ID` | `session:<value>` |
| 3 | `AMPLIHACK_TREE_ID` | `tree:<value>` |
| 4 | `CLAUDE_CODE_SESSION_ID` | `claude:<value>` |

The prefix keeps identities from different sources apart. `recipe run
--detach` uses the same order to fill in a new run's `parent`, and sets
`AMPLIHACK_RUN_ID` to the run's session name for everything the run starts.
So a run launched from inside a detached run is that run's child, even though
both share `AMPLIHACK_SESSION_ID`.

`amplihack launch` sets `AMPLIHACK_SESSION_ID` for Claude Code, Copilot CLI
and Codex alike, so a session started that way has an identity whichever
agent it runs. A session started some other way may have none; use `--all`
or a manifest.

An identity is a filter, not a credential. Anyone who can read the log
directory can see every record in it.

### Descendants

The default view starts with the runs whose `parent` is the caller's
identity, then adds every run whose `parent` is `run:<session>` of a run
already selected, and so on. Finished runs are included. A cycle in the
records is visited once. The walk stops after 2,000 runs or 32 levels.

### Runs without a record

A `<session>.log` with no `<session>.run.json` came from an older amplihack
or a run whose record could not be written. It has no parent, so it appears
only under `--all`. Its recipe and session come from the file name. Its exit
code is unknown, so its stop reason comes only from the last terminal
failure-class marker line in the final 64 KiB of the log; `retry` markers and
failure text are not used. With no such marker the reason is unknown.
A `<session>.log` that is not owned by you is skipped.

### Auto-drive rounds

[`auto-drive-to-merge`](./auto-drive-to-merge.md)
runs its review and merge-ready rounds in the foreground of its own run, not
detached. They do not get rows of their own. They set the stage of the run
that drives the PR.

### Log directory warnings

The log directory is read only when it is a directory owned by you and not
writable by group or others (mode `& 0o022` is `0`). Up to 20,000 entries
are listed by name and modification time. Runs are then taken newest first,
by the latest change to either of their files, until 2,000 run files are
taken, and only those records are read. While the directory holds at most
20,000 entries, the runs left out are the oldest, and a live run, which
writes to its log, is among the newest and is kept. Past 20,000 entries this
no longer holds: the directory lists its entries in no particular order, and
the limit applies before the sort by age, so any run, including a live one,
may be left out. That case has its own warning. In each case below the command prints
one line on stderr, with no path or system error, and still prints the
table:

| Condition | Warning | Runs shown |
|-----------|---------|------------|
| `recipe.log_dir` and HOME are both unset | `warning: recipe log directory unavailable (HOME not set)` | None. |
| The directory cannot be listed, or is not a directory | `warning: recipe log directory unreadable` | None. |
| The directory is not owned by you, or group or others may write to it | `warning: recipe log directory is not private; ignoring it` | None. |
| The directory has more than 2,000 run files and at most 20,000 entries | `warning: more than 2000 run files in the recipe log directory; older runs are not shown` | The newest runs, up to 2,000 files. |
| The directory has more than 20,000 entries | `warning: more than 20000 entries in the recipe log directory; some runs, possibly recent ones, are not shown` | Up to 2,000 files, newest first, from the first 20,000 entries listed. These are not necessarily the newest runs. |

A directory that does not exist yet gives no warning and no runs.
`recipe run --detach` creates it with mode `0700`.

## Stages and progress

The first rule that matches sets the stage:

| Order | Condition | Stage | Progress |
|-------|-----------|-------|----------|
| 1 | The PR is merged | `merged` | 100% |
| 2 | The PR is closed without merging | `closed` | 0%, marked `⏸` |
| 3 | M merge-ready rounds, M ≥ 1 | `merge-ready round M` | 70 + 5·(M−1), at most 95 |
| 4 | C review rounds, C ≥ 1 | `review round C`, then `N concerns` | 40 + 5·(C−1), at most 60 |
| 5 | The PR is open | `PR open` | 30% |
| 6 | A PR number is known, but GitHub gave no state for it | `PR state unknown` | 30% |
| 7 | A run exists, with no PR | `building` | 15% |
| 8 | Manifest row only | `queued` | 0% |

- Rounds are counted from the auto-drive state directory of the PR,
  `<state root>/auto-drive/<key>`, where the state root is
  `AMPLIHACK_STATE_DIR`, otherwise `~/.amplihack/state`. The key and file
  names come from the same rules as `amplifier-bundle/tools/autodrive_state.sh`,
  which writes them; a test checks that both agree.
- A review round is a `crusty-round-*` file; a merge-ready round is a
  `merge-ready-round-*` file. Files whose names contain `health` or `before`
  are not rounds. Only regular files count, and at most 500 are read.
- `N concerns` is `concern_count` from `crusty-latest.json`. It is left out
  when the file is missing or the value is not a whole number.
- `conflicts` is added when GitHub reports the PR as conflicting with its
  base branch.
- When GitHub could not be asked, rules 1, 2 and 5 cannot match. A run with a
  recorded PR number still shows its rounds; the `PR` column shows `#?`.

### Finding the PR

A run's PR is the `pr_number` in its record or manifest row. Otherwise it is
the first PR in its repository whose title contains `#<issue>` followed by a
non-digit, as the prototype did. This is a heuristic: a PR that mentions the
issue in its title without fixing it can be matched.

GitHub is asked once per repository:

```
gh pr list --repo=<owner/name> --state all --limit 100 --json number,title,state,mergeable
```

PR titles are used only for this match. They are never printed.

### Running or stopped

`▶` means the run's tmux session is live. `⏸` means it is not, and the reason
comes from the record's [outcome class](./recipe-command.md#run-records), or
from a marker line for a [run without a record](#runs-without-a-record):

| Class | Reason shown |
|-------|--------------|
| `usage_limit` | `stopped on a usage limit` |
| `ok`, PR not merged | `finished` |
| any other class | `stopped: <class>` |
| unknown | `stopped` |

Merged rows show no marker reason. Queued rows show `queued`.

### Elapsed time

| Run | Elapsed |
|-----|---------|
| live | From start to now. |
| finished | From start to the outcome's `finished_at`. |
| start unknown | `-` |

The start is the record's `started_at`; for a live run with no record, the
tmux session's creation time (`#{session_created}`), collected by the same
`tmux` call that lists live sessions. File modification times are never used.
Elapsed time under a day prints as `2h05m`, longer as `3d04h`.

## Logins

The section reads the same data as
[`amplihack profile usage`](./profile-command.md#usage-command), in process,
without parsing its table. Differences:

- Nothing is written to the profile store. A reading taken here is not
  recorded for auto-switch or the rotator.
- The section stops at the [deadline](#time-limits). Probes still running are
  cancelled and their process groups killed.
- A profile is an API-key (Claude Platform) profile when its `settings.json`
  has `apiKeyHelper`, its `settings.json` `env` sets `ANTHROPIC_API_KEY`, or
  its `.claude.json` has `primaryApiKey`. The profile name plays no part.
  Such a profile is billed per token, has no plan limits, and is not probed.
  No key, key path or parse error is printed.
- For this check, `settings.json` and `.claude.json` are read up to 4 MiB
  each. A file larger than that is not inspected, so a profile whose only
  API-key sign is in an oversized `.claude.json` is probed like a plan
  login.
- Like `profile usage`, it refuses to run inside Docker. When it refuses, or
  cannot read the profile store or run the probe, the section prints one
  fixed reason in place of rows, never a path:

  | Reason | When |
  |--------|------|
  | `profile store unavailable in Docker` | Docker mode is on. |
  | `no profile store` | The store's root cannot be resolved, for example with no HOME or an invalid root setting. |
  | `profile store unreadable` | The root, or a name in it, cannot be listed safely. |
  | `usage probe failed` | The probe could not run, for example because `claude` was not found. |

Every profile in the store gets a row, even when its probe did not finish.

## Manifest

The manifest adds labels to discovered runs and lists work that has not
started yet. Default path: `~/.amplihack/work/workstreams.tsv`.

```text
# label<TAB>issue<TAB>pr
workstream-status skill	1710	-
rotator manual login	#1693	#1702
docs sweep	1720	-
```

- Three fields separated by tabs: label, issue, PR.
- A number may be written with or without `#`. `-` means empty.
- Each row needs an issue or a PR. A label of `-` uses the
  [default label](#workstreams-section).
- Lines starting with `#` and blank lines are skipped.
- The file may be at most 64 KiB.

A manifest that is readable by others, such as one saved with mode `0644`,
is accepted; only write access for group or others is refused.

A manifest row and a discovered run are the same workstream when they share
an issue number, or a PR number when both have one. The row then uses the
manifest label. A manifest row that matches no run is `queued` at 0%. Manifest
rows appear in both views.

| Situation | Result |
|-----------|--------|
| Default path missing | Ignored. |
| Default path not a regular file, not owned by you, or writable by group or others | Ignored, with a warning on stderr. |
| `--manifest` path missing, unreadable, not a regular file, not owned by you, or writable by group or others | Error, exit 1. |
| Malformed line, in either file | Error naming the line, exit 1. |

```console
$ amplihack workstreams status --manifest ~/.amplihack/work/q4.tsv
error: manifest /home/alice/.amplihack/work/q4.tsv:3: expected 3 tab-separated fields, found 2
```

## Time limits

Every outside call has a limit, so the command does not hang:

| Call | Limit | When it runs out |
|------|-------|------------------|
| `tmux ls -F '#{session_name} #{session_created}'` | 3 s | No session counts as live. |
| `gh pr list`, per repository | 10 s | PR fields show `?`. |
| `LOGINS` section | `--logins-deadline`, default 8 s, plus 1 s to stop | Unfinished profiles show `no reading`. |

A missing `tmux`, `gh` or `claude` binary is treated like a call that ran out.
Subprocesses run without a shell, with stdin from `/dev/null`, in their own
process group, which is killed when the limit passes.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | The table was printed, even if some sources failed. |
| `1` | The manifest named by `--manifest` is missing, unreadable or fails the [ownership and permission check](#security), or a manifest line is malformed. |
| `2` | Usage error. |

## Security

- The command only reads. It never changes file modes, writes to the profile
  store or starts a run.
- The log directory is used only when it is owned by you and not writable by
  group or others; see [Log directory warnings](#log-directory-warnings).
- Run records, auto-drive state files and profile files are read only when
  they are regular files, owned by you, have no permission bits for group or
  others (mode `& 0o077` is `0`), and are within their size limit. Others are
  skipped.
- A manifest is read only when it is a regular file, owned by you, not
  writable by group or others (mode `& 0o022` is `0`), and at most 64 KiB. It
  may be readable by others, since it holds only labels and numbers. A
  default manifest that fails the check is skipped with a warning; one named
  by `--manifest` is an error.
- Log tails, PR titles and `gh` output are treated as untrusted. They decide
  stages and matches but are never printed.
- The origin URL of a repository is never printed, logged or stored; only its
  `owner/name` slug is used.

## Skill

The bundled `workstream-status` skill
([`amplifier-bundle/skills/workstream-status/SKILL.md`](https://github.com/rysweet/amplihack-rs/blob/main/amplifier-bundle/skills/workstream-status/SKILL.md))
tells an agent in Claude Code, Copilot CLI or Codex to run
`amplihack workstreams status` and paste the output verbatim in a fenced
`text` block at each status update, and to use its own scheduling, such as
`/loop 15m`, when the user asks for updates at an interval.

## Related

- [`amplihack recipe` reference: Detached runs](./recipe-command.md#detached-runs) - how runs are launched and what the run record holds
- [`amplihack profile` reference: Usage command](./profile-command.md#usage-command) - where login readings come from
- [Rotate Claude Logins](../howto/claude-profile-rotation.md) - keep `~/.claude` on the login with the most capacity
