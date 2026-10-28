# Workstream Status

`amplihack workstreams status` prints a plain-text table of the workstreams
the calling session started and of Claude login capacity. The
`workstream-status` skill
(`amplifier-bundle/skills/workstream-status/SKILL.md`) tells an agent to run it and paste the output into its reply.

## Contents

- [Synopsis](#synopsis)
- [Example output](#example-output)
- [Which runs are listed](#which-runs-are-listed)
- [Stages and progress](#stages-and-progress)
- [Stage text and markers](#stage-text-and-markers)
- [Logins section](#logins-section)
- [Manifest file](#manifest-file)
- [The run meta file](#the-run-meta-file)
- [Layout](#layout)
- [Run time](#run-time)
- [Privacy and safety](#privacy-and-safety)
- [Exit codes](#exit-codes)

## Synopsis

```
amplihack workstreams status [--all] [--manifest <path>] [--no-logins] [--logins-deadline <secs>]
```

| Flag | Default | Effect |
| --- | --- | --- |
| `--all` | off | List every run on this host, including runs started before parent records existed. |
| `--manifest <path>` | `~/.amplihack/work/workstreams.tsv` | Read labels and queued items from this file. |
| `--no-logins` | off | Skip the logins section. No usage probe runs. |
| `--logins-deadline <secs>` | `20` | Stop waiting for login readings after this many seconds. Range 1–600. |

The environment variable `AMPLIHACK_WORKSTREAMS_LOGINS_DEADLINE_SECS` sets the
deadline when the flag is not given. A value that is not a whole number in
1–600 is ignored with one warning on stderr, and the default applies. A bad
flag value is a usage error.

## Example output

```text
WORKSTREAMS  2026-10-27 14:05
  WORKSTREAM                  ISSUE PR    PROGRESS                    STAGE
  ──────────────────────────────────────────────────────────────────────────────────────────────────
  Workstream status skill     #1710 #1715 █████████░░░░░░░░░░░  48% ▶ review round 2, 3 concerns, 1…
  Login rotator follow-ups    #1700 #1704 ███████████████░░░░░  75% ⏸ merge-ready round 1, stopped …
  Usage probe deadline for t… #1698 #1716 ██████░░░░░░░░░░░░░░  30% ⏸ PR open, conflicts with main,…
  auto-drive-to-merge #1711   #1711 -     ███░░░░░░░░░░░░░░░░░  15% ▶ building, 12m
  Docs cleanup                #1712 -     ░░░░░░░░░░░░░░░░░░░░   0% ⏸ queued
  Remaining step outputs      #1684 #1688 ████████████████████ 100%   merged, 1d3h

LOGINS
  PROFILE           SESSION           WEEK              STATE, NEXT RESET
  ──────────────────────────────────────────────────────────────────────────────────────────────────
  main (live)       S ███████░░░  72% W ████░░░░░░  41% ok, session resets 4pm, week resets Nov 2
  work              S ██████████ 100% W ████████░░  88% session_exhausted, session resets 3:30pm, w…
  spare             no reading (timeout)
  console           API key, billed per token (no plan limits)
```

When neither `AMPLIHACK_SESSION_ID` nor `AMPLIHACK_DETACHED_SESSION` is set
and `--all` is not given, the workstreams section is one line, followed by the
logins section:

```text
no amplihack session id in this environment; run with --all to list every run on the host
```

## Which runs are listed

Runs are found in the recipe log directory
([`recipe.log_dir`](../reference/user-config.md#recipelog_dir), default
`~/.amplihack/recipe-logs`). Each detached run writes a
[meta file](#the-run-meta-file) there that names the session that started it.

The calling session (the root) and the first runs listed depend on which
variable is set:

| Caller | Root | First runs listed |
| --- | --- | --- |
| a detached run (`AMPLIHACK_DETACHED_SESSION` set) | that run's session | runs whose `parent_run` is the root run |
| an interactive session (only `AMPLIHACK_SESSION_ID` set) | the session id | runs whose `parent_session_id` is the root session id |

When `AMPLIHACK_DETACHED_SESSION` is set, `parent_session_id` is ignored. A
detached run inherits its starter's `AMPLIHACK_SESSION_ID`, so matching on it
would also list the run's siblings.

Then any run whose `parent_run` is a run already listed is added. This repeats
until no new run is added, so runs started by runs started by the caller are
listed too. The root run itself is not listed.

Runs are never matched by start time or working directory.

Auto-drive rounds are not separate runs. A run's auto-drive progress is read
from its auto-drive state directory and shown on the run's own row.

If the log directory does not exist, no runs are found and only manifest rows
are shown. If it is a symlink, is not owned by you, or can be written by group
or others, the command prints one warning on stderr and lists no runs from it.
A meta file that is a symlink, is not owned by you, or can be written by group
or others is skipped, and its run counts as having no meta file.

`--all` lists every run in the log directory. Runs with no meta file (started
before parent records existed) are listed only with `--all`. Their start time
comes from the log's first event, otherwise from the tmux session's creation
time; if neither is known, elapsed shows `?`.

## Stages and progress

The first rule that matches sets the stage. `N` is the number of finished
rounds of that loop.

| Rule | Stage | Progress |
| --- | --- | --- |
| PR merged, or auto-drive recorded the `merged` phase | merged | 100% |
| auto-drive recorded the `merge-ready-loop` phase | merge-ready loop finished | 95% |
| `crusty-loop` phase recorded, or any merge-ready round exists | merge-ready round N | min(70 + 5N, 95)% |
| `build` phase recorded, or any review round exists | review round N | min(40 + 4N, 60)% |
| the run has a PR | PR open | 30% |
| the run was found | building | 15% |
| the row is only in the manifest | queued | 0% |

A loop with no finished round shows `review loop` (40%) or `merge-ready loop`
(70%). A PR that was closed without merging shows `PR closed` and keeps the
computed progress.

The PR for a run is, in order: the PR number in the run's meta file; the PR
number the run logged; an open or closed PR whose head branch is the run's
branch; a PR whose title contains `#<issue>` not followed by a digit.

Auto-drive state is read from
`${AMPLIHACK_STATE_DIR:-~/.amplihack/state}/auto-drive/<key>/`, where the key
is built the same way `autodrive_state.sh` builds it. The directory names, file
names and phase names come from `amplifier-bundle/tools/autodrive_layout.sh`,
which the auto-drive scripts also read.

## Stage text and markers

The stage text joins these parts with `, `, in this order:

1. the stage (`building`, `PR open`, `review round 2`, `merged`, ...);
2. `K concerns`, the open concerns from the latest review round, when a review
   round exists;
3. `conflicts with <base>`, when the PR does not merge cleanly into its base
   branch (`main` when the base is not known);
4. the reason, for a run that is not live;
5. the time since the run started: `<1m`, `12m`, `1h42m`, `1d3h`.

When the PR list could not be fetched, `PR state unknown` takes the place of
the parts that depend on it.

| Marker | Meaning |
| --- | --- |
| `▶` | the run's tmux session is live |
| `⏸` | the run is waiting or stopped; the reason follows |
| (blank) | the PR is merged |

| Reason | When |
| --- | --- |
| `stopped on a usage limit` | the end of the run's log shows a usage-limit message |
| `failed (exit N)` | the run ended with a non-zero exit code |
| `finished` | the run ended with exit code 0 and its PR is not merged |
| `queued` | the row is only in the manifest |
| `(waiting or stopped)` | none of the above is known |

The reason is never guessed. A queued row shows `queued` once.

## Logins section

One row per login profile, in the order `amplihack profile list` shows them.
The readings come from the same probe as `amplihack profile usage`. This
command only reads: it records no readings and changes no profile.

| Row | Shows |
| --- | --- |
| reading | `S` session bar and `W` week bar, 10 cells each, with percentages, then the state and the next reset |
| no reading | `no reading (<state>)`, for example `no reading (timeout)` |
| API-key profile | `API key, billed per token (no plan limits)` |

` (live)` marks the profile whose login is in `~/.claude`.

A profile is an API-key profile when it is named `api-key`, or when its stored
configuration holds an API-key setting: a `primaryApiKey` in its
`.claude.json`, or an `apiKeyHelper` or `env.ANTHROPIC_API_KEY` entry in its
`settings.json`. The name is checked first, and then no file is read. For the
settings, only the entry's presence counts, so an empty `primaryApiKey` counts
too. The command never reads or prints a key's value. No probe runs for an
API-key profile.

## Manifest file

The manifest adds labels and workstreams that have not started. It is
tab-separated, one line per workstream:

```text
# label	issue	pr
Workstream status skill	#1710	-
Login rotator follow-ups	1700	#1704
Docs cleanup	#1712	-
```

- Fields: `label`, `issue`, `pr`. Labels may contain spaces.
- Issue and PR are `123`, `#123` or `-` for none.
- Blank lines and lines starting with `#` are skipped.
- A malformed line is skipped with one warning on stderr, such as
  `manifest:4: issue is not a number`.

Rows merge with discovered runs by PR number, then by issue number. Each run
merges with one manifest row at most. A merged row takes the manifest's label
and the run's stage. Manifest rows come first, in file order, then runs not in
the manifest, oldest first. A run not in the manifest is labelled
`<recipe> #<issue>`, or by its session name when no issue is known.

The manifest applies with `--all` too. The two paths are checked differently:

| Path | Skipped with a warning on stderr | Error, exit 2 |
| --- | --- | --- |
| default `~/.amplihack/work/workstreams.tsv` | a symlink, not a regular file, not owned by you, or writable by group or others | never |
| explicit `--manifest <path>` | never | a symlink, a FIFO, a device or other non-regular file, missing, or unreadable |

A missing default file is ignored without a warning. An explicit path is not
checked for owner or mode, because you named it. A symlink is refused in both
cases: the file is opened without following a final symlink, so pass the
target's path instead.

## The run meta file

`amplihack recipe run --detach` writes `<session>.meta.json` (mode 0600) in
the log directory when it launches a run. It stays after the run ends and is
removed only when the launch fails.

```json
{
  "schema": 1,
  "session": "amp-auto-drive-to-merge-amplihack-rs-20261027-120300",
  "parent_session_id": "4f1c2b9e-1d0a-4c55-9a51-0c9f1e7d2a10",
  "parent_run": null,
  "recipe": "auto-drive-to-merge",
  "started_at": "2026-10-27T12:03:00Z",
  "cwd": "/home/alice/src/amplihack-rs/worktrees/feat/issue-1710-add-a-reusable",
  "branch": "feat/issue-1710-add-a-reusable",
  "issue_number": 1710,
  "pr_number": null
}
```

| Field | Source |
| --- | --- |
| `parent_session_id` | `$AMPLIHACK_SESSION_ID` at launch, or `null` |
| `parent_run` | `$AMPLIHACK_DETACHED_SESSION` at launch, or `null` |
| `branch`, `issue_number`, `pr_number` | the recipe's context values, or `null` |

An id that is not 1–128 characters of `A-Z a-z 0-9 . _ : -` is stored as
`null`. The run itself gets `AMPLIHACK_DETACHED_SESSION=<session>`, so runs it
starts name it as their parent.

## Layout

- No line is longer than 100 characters, and the output has no ANSI escape
  codes.
- Workstream columns: indent 2, label 28, issue 6, PR 6, bar 20, percentage 5,
  marker 3, stage text 30. Each width includes the trailing space.
- Text longer than its column is cut and ends with `…`.
- Bars use `█` for filled cells and `░` for empty ones. A 20-cell bar fills one
  cell per 5%, rounded down.
- Control characters and bidirectional-text characters from any outside
  source show as `?`.

## Run time

The command never hangs. The logins probes start first and run in parallel with
discovery. When the deadline passes, the probes still running are stopped and
their profiles show `no reading (timeout)`. Each other call has its own limit:
`tmux` and `git` 3 s, `gh pr list` 10 s per repository. The PR lists of up to 8
repositories are fetched in parallel; more repositories give a warning. A call
that fails or times out leaves that part unknown and the rest of the table is
still printed.

The command looks at most 4,096 entries in the recipe log directory and in
each auto-drive state directory. Past that it prints one warning, such as
`warning: more than 4096 entries in the recipe log directory; some runs may be
missing`, and lists what it read. When the recipe log directory cannot be
found at all, it warns `warning: recipe log directory cannot be resolved; no
runs listed`. Neither warning names a path.

## Privacy and safety

- The meta file holds no task text, URLs, tokens or other environment values.
- Parent records are for display only. Nothing uses them to decide whether a
  run may be stopped, merged or read.
- The log directory, the meta files and the default manifest are used only
  when they are owned by you and not writable by group or others; otherwise
  they are skipped with a warning (see [Which runs are listed](#which-runs-are-listed)
  and [Manifest file](#manifest-file)). An explicit `--manifest` is read
  whatever its owner or mode, but it must be a regular file.
- No symlink is followed: a symlinked log directory, meta file or manifest is
  never opened, including an explicit `--manifest`.
- The command creates and changes no files.
- PR titles, log lines, paths and remote URLs are never printed.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The table was printed, including when some parts are unknown or timed out. |
| `2` | Usage error, an explicit `--manifest` that is a symlink, is not a regular file, is missing or cannot be read, or a table that cannot be written to stdout. A closed pipe, as in `amplihack workstreams status \| head`, still exits 0. |

## Related

- [`amplihack recipe run --detach`](../reference/recipe-command.md) starts the
  runs this command lists.
- [`amplihack profile usage`](../reference/profile-command.md) measures the
  same logins with more detail.
- [Claude login rotation](../howto/claude-profile-rotation.md) keeps
  `~/.claude` on the login with the most capacity.
