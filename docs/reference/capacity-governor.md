# Capacity Governor: Reference

The capacity governor decides whether a new `amplihack recipe run`,
`amplihack claude` or `amplihack launch` may start now, must wait, or must not
start. It reads the Claude usage readings that
[`amplihack profile usage`](./profile-command.md#usage-command) and the rotator
record in each profile, and admits work at a rate the remaining session and weekly budgets can
sustain. It never moves a login between accounts; that remains the
[rotator's](../howto/claude-profile-rotation.md) job.

The governor is on by default. `AMPLIHACK_GOVERNOR=off` turns it off
completely.

## Contents

- [Quick start](#quick-start)
- [Where admission happens](#where-admission-happens)
- [Model](#model)
- [Budgets and the weekly-only rule](#budgets-and-the-weekly-only-rule)
- [Reserves and classes](#reserves-and-classes)
- [Rate feedback](#rate-feedback)
- [Fair queue](#fair-queue)
- [Nesting](#nesting)
- [Waiting, denial and exit code 80](#waiting-denial-and-exit-code-80)
- [Kill switch and governor keys](#kill-switch-and-governor-keys)
- [State files](#state-files)
- [governor status](#governor-status)
- [Fail-open compared with overflow](#fail-open-compared-with-overflow)
- [Security model](#security-model)
- [Related](#related)

## Quick start

See what the governor would do for the next start:

```sh
amplihack governor status
```

```text
Capacity governor: on; concurrency limit 3 of 4

Profiles:
  alpha [active]
    session: 62.0% (resets 2026-11-01 15:00 UTC; burn 2.1/h, sustainable 0.4/h)
    weekly:  71.0% (resets 2026-11-05 09:00 UTC; burn 0.9/h, sustainable 0.3/h)
  beta
    session: no session window
    weekly:  38.0% (resets 2026-11-07 09:00 UTC; burn 0.6/h, sustainable 0.5/h)

Reserve: 10 points; best weekly remainder 62.0; ordinary work admitted

Slots:
  pid 41022 ws-1811 ordinary since 2026-11-01 14:02 UTC; 3 nested
  pid 41377 ws-1790 review since 2026-11-01 14:09 UTC; 0 nested
  pid 39811 default ordinary since 2026-11-01 12:40 UTC; 0 nested [stale]

Queue:
  #57 ws-1811 ordinary pid 41590 since 2026-11-01 14:11 UTC

Workstreams:
  default: 1 admissions in 24 h, 1 slots, 0 waiting, token share -
  ws-1790: 2 admissions in 24 h, 1 slots, 0 waiting, token share -
  ws-1811: 4 admissions in 24 h, 1 slots, 1 waiting, token share -
```

Start a recipe that may wait up to 30 minutes for capacity:

```sh
amplihack recipe run amplifier-bundle/recipes/default-workflow.yaml \
  --governor-wait 30m -c task_description="Fix issue #1811"
```

Run without the governor for one command:

```sh
AMPLIHACK_GOVERNOR=off amplihack claude
```

## Where admission happens

| Command | When the governor is asked | Slot held until |
|---------|----------------------------|-----------------|
| `amplihack recipe run` | After the pre-flight checks and the capacity warning, before `recipe-runner-rs` starts | The recipe returns |
| `amplihack claude` | After the capacity warning, before `claude` starts | `claude` exits, including interactive sessions |
| `amplihack launch` | After the capacity warning, before the launcher starts | The launcher returns |

`recipe run --dry-run`, `recipe list`, `recipe show`, `recipe validate`,
`profile …` and `governor status` are never governed.

Agent steps inside a recipe run start through `amplihack claude`; they are
admitted under the recipe's slot. See [Nesting](#nesting).

### Flags

All three commands accept the same flags. They come before any arguments that
are forwarded to `claude`, so `amplihack claude -- --workstream x` passes
`--workstream x` to `claude` unchanged.

| Flag | Default | Description |
|------|---------|-------------|
| `--governor-wait <DURATION>` | `AMPLIHACK_GOVERNOR_WAIT`, then `governor.wait_secs`, else `10m` | Longest time to wait for admission. `0` means do not wait. |
| `--governor-class <CLASS>` | `AMPLIHACK_GOVERNOR_CLASS`, then inferred, else `ordinary` | `coordination`, `recovery`, `review` or `ordinary`. See [Reserves and classes](#reserves-and-classes). |
| `--workstream <ID>` | See [Workstream identity](#workstream-identity) | The workstream the start is counted against for fairness. |

`<DURATION>` is one to six digits with an optional unit: `s` (the default),
`m` or `h`. `90`, `90s`, `15m` and `2h` are valid. Values above 24 hours are
reduced to 24 hours. Any other form is a usage error (exit 2).

## Model

Each admission attempt gathers its inputs, takes a decision, and records the
result. The decision is a pure function of its inputs and the current time:
the same inputs always give the same decision.

The inputs are:

- the latest usage reading for each profile, read from the profile root
  without changing it;
- the governor's own history of readings, used for burn rates;
- the active profile, read from the default-login pointer;
- the slots held and the requests waiting, from `.governor.d`;
- the request itself: class, workstream, whether it is a `recipe run` or an
  agent launch, the kind of slot it is nested under if any, and how long it
  may wait.

The decision is one of **admit**, **wait** or **deny**, with a reason. The
checks run in this order, and the first that applies decides:

| Step | Check | Result |
|------|-------|--------|
| 1 | No profile has a usable reading | Admit, fail open (`no-readings`) |
| 2 | Every usable profile's weekly window is exhausted | Wait for the earliest weekly reset (`weekly-exhausted`); deny if that reset is after the wait deadline or the wait is `0` |
| 3 | Every usable profile's session window is exhausted, and at least one has weekly room | Wait for the earliest session reset, for every class (`session-exhausted`) |
| 4 | The request is `ordinary` and the best usable weekly remainder is at or below the reserve | Wait (`reserve`) |
| 5 | Rate feedback (at most once per interval) sets the concurrency limit; the slots in use are at that limit | Wait (`concurrency`) |
| 6 | Another waiting request comes first in the [fair queue](#fair-queue) | Wait (`queue`) |
| 7 | Otherwise | Admit (`room`, or `nested` for a nested request) |

A [nested](#nesting) request skips steps 5 and 6. Only a request nested
under a running `recipe run`'s slot also skips step 4; one nested under an
interactive session's slot is held at the reserve like a top-level start.
Steps 1 to 3 apply to every request.

The decision function denies only at step 2, and only when at least one
profile has a reading. Separately, the admission loop ends any wait that runs
out, whatever its reason, and the command exits with 80 and the reason
`wait-expired`; see [Waiting, denial and exit code 80](#waiting-denial-and-exit-code-80).
No fault inside the governor can deny a start; see
[Failure modes](#failure-modes).

### Which profile a start is charged to

The governor does not choose a profile. It charges the start to the active
profile, the one the default-login pointer names. The pool it checks is the
active profile plus every other listed profile that is eligible: profiles
marked as overflow (credit-backed) or API-key profiles are left out, so their
capacity never counts as room. Overflow stays behind the rotator's own gate
([#1781](https://github.com/rysweet/amplihack-rs/pull/1781)). A start is
admitted when **any** profile in the pool has room after the reserve is
applied.

Admission prints nothing in the usual case. When the room is on a profile
other than the active one, it prints one line to stderr before the child
starts:

```text
amplihack: governor: admitted (room): profile beta has room; the rotator moves work there from the active profile
```

The concurrency limit is one per host, shared by every profile.

## Budgets and the weekly-only rule

All budget arithmetic uses **percentage points** of each window, taken from
the readings. A profile has a weekly window and, for most plans, a five-hour
session window.

| Quantity | Definition |
|----------|------------|
| Weekly remainder | `100 - weekly used %` |
| Session remainder | `100 - session used %` |
| Usable points | The smaller of the session remainder and the weekly remainder |
| Weekly-exhausted | Weekly used is 100% |
| Session-exhausted | Session used is 100% and the weekly window still has room |

A reading that is not a finite number between 0 and 100 is treated as
unknown.

### Weekly-only profiles

A profile whose reading has a weekly window and no session window is
**weekly-only**. For it:

- usable points are the weekly remainder;
- it is never session-exhausted, and no session reset is expected for it;
- status shows `session: null` and `session_window: false` in JSON and
  `no session window` in the table. It does not say `no session limit`:
  the stored reading cannot tell a plan with no session limit from a
  session window that was not recorded, the same reason `profile rotate
  status` prints `no session window`.

A profile whose reading has neither window is unknown and is left out of the
pool. So is a profile whose session window was read but holds a percentage
that is not a finite number between 0 and 100: a damaged session reading is
never taken as "no session window", which would drop the session limit.
Status marks such a profile with `invalid_reading: true` in JSON and
`invalid reading` in the table. If every profile is unknown, step 1 admits
the start.

### A session reset never adds weekly capacity

An imminent session reset never raises usable points above the weekly
remainder. A profile at 99% weekly with a session window that resets in two
minutes has 1 usable point before and after that reset.

## Reserves and classes

Classes, from highest priority to lowest:

| Class | Typical use | Can use the reserve |
|-------|-------------|---------------------|
| `coordination` | Coordination traffic between agents | Yes |
| `recovery` | Recovering a failed or stuck run | Yes |
| `review` | Reviewing a pull request | Yes |
| `ordinary` | Everything else | No |

The **reserve** is `governor.reserve_points` (default 10) points of weekly
remainder. A new top-level ordinary start waits when the best usable
profile's weekly remainder is at or below the reserve. The other three
classes may spend the reserve down to 0, and so may an agent step or
sub-recipe of a `recipe run` that was already admitted (see
[Nesting](#nesting)). A `recipe run` or launch started from inside an
interactive session is new work and is held at the reserve.

The reserve is a scheduling preference, not a guarantee: a class can be set
by any parent process. A session-exhausted pool (step 3) makes every class
wait, because a session window cannot be borrowed from.

### How the class is chosen

The first of these that is set wins:

1. `--governor-class`
2. `AMPLIHACK_GOVERNOR_CLASS`
3. Inference from the work:
   - for `amplihack launch` and `amplihack claude` only, a `-p` task
     text whose first token is `[coord]` gives `coordination` (see
     [Coordination Messages](./coordination-messages.md)). The
     `task_description` of a recipe run is never read, because it can come
     from a GitHub issue; a recipe run that is coordination work sets
     `--governor-class coordination` or
     `AMPLIHACK_GOVERNOR_CLASS=coordination`;
   - a recipe whose name starts with `recovery` gives `recovery` (no such
     recipe ships yet; the prefix rule covers future ones);
   - the recipes `consensus-pr-review`, `workflow-pr-review` and
     `workflow-refactor-review` give `review`.
4. `ordinary`

Recipe names are matched against fixed lists in the code, not against text
the task supplies. `[coord]` anywhere other than the first token has no
effect.

## Rate feedback

The governor keeps a concurrency limit between 1 and
`governor.max_concurrency` (default 4) and adjusts it from the burn rate.

| Term | Definition |
|------|------------|
| Burn rate | Change in a window's used percentage over the observation period, in points per hour, from two or more readings in `history.json`. |
| Sustainable rate | Remaining points divided by the hours until that window resets. |
| Forecast exhaustion | Now plus remaining points divided by the burn rate. |

The weekly window always has these values when its readings allow. The
session window has them only on a profile with a session window. The rates
used for feedback are those of the active profile.

The burn rate is measured from the oldest to the newest reading taken since
the window last reset; readings from before the reset are ignored. A burn
rate is **unknown** when there are fewer than two readings since the reset,
or when the used percentage went down.
An unknown rate never changes the limit.

At most one evaluation runs per `governor.eval_interval_secs` (default 60),
on an admission attempt. With `band = governor.band_pct / 100` (default
0.10):

| Signal | Condition | Change |
|--------|-----------|--------|
| Over | Burn rate above `(1 + band) ×` sustainable rate in any window | Limit down by `governor.step` (default 1) |
| Under | Burn rate below `(1 - band) ×` sustainable rate in every window | Limit up by `governor.step` |
| Steady | Neither | No change |
| Unknown | No burn rate | No change |

The limit is never below 1 or above `governor.max_concurrency`. When the
limit is already 1 and the signal is still over, the limit stays at 1; one
slot remains available, and further starts wait with the reason
`concurrency` while it is held. There is no separate `rate` hold. Burn
readings are noisy, and holding every ordinary start on the rate signal alone
could stop all work on a single high reading; keeping one slot free bounds
the burn without that risk.

The band stops the limit stepping up and down on every evaluation, in both
directions. The forecast exhaustion shown in status is not a separate test:
it and the sustainable rate are computed from the same remaining points at
the same moment, so a forecast before the reset means the same thing as a
burn rate above sustainable. Testing it as well would step the limit down at
any burn above sustainable, bypassing the band. For example, with 50 points
left and 10 hours to the reset the sustainable rate is 5 points per hour; a
burn rate of 5.25 points per hour forecasts exhaustion after 9.5 hours, but
is inside the band, so the signal is steady and the limit holds.

## Fair queue

A start that cannot be admitted at once joins the queue in
`.governor.d/queue.json`. When a slot is free, the request admitted is the one
with the lowest value of:

```text
(class rank, admissions to its workstream in the fairness window, seq)
```

- Class rank: `coordination` 0, `recovery` 1, `review` 2, `ordinary` 3.
- The fairness window is the admissions recorded in the last 24 hours, at
  most 1024 entries.
- `seq` is the order in which requests joined the queue.

A process is admitted only when its own request is at the head of that order.
With two workstreams waiting and a limit of 1, admissions alternate between
them. Within one class and one workstream, requests are admitted in the
order they arrived.

A request leaves the queue when it is admitted, when its process gives up,
or when its process is found to be dead. The queue holds at most 4096
entries; beyond that, starts are admitted without queueing (fail open,
`queue-full`).

### Workstream identity

The first of these that is set wins:

1. `--workstream <ID>`
2. `AMPLIHACK_WORKSTREAM`
3. `AMPLIHACK_WORKSTREAM_ISSUE`, as `ws-<n>`
4. `AMPLIHACK_RECIPE_RUN_ID`, cleaned (see below)
5. `default`

An identity must match `[A-Za-z0-9._-]{1,64}` and must not be `.` or `..`.
An invalid `--workstream` is a usage error (exit 2). An invalid value from
`AMPLIHACK_WORKSTREAM` becomes `default`, and a warning gives its length but
not its content.

`AMPLIHACK_WORKSTREAM_ISSUE` is used only when it is a decimal number, giving
`ws-<n>`. `AMPLIHACK_RECIPE_RUN_ID` is cleaned before use: each character
outside `[A-Za-z0-9._-]` becomes `-`, and the result is cut to 64 characters.
A value that is still invalid after cleaning, such as `.`, is skipped.

## Nesting

A recipe run holds a slot and starts its agent steps through
`amplihack claude`. If each step needed a slot of its own, a limit of 1 would
deadlock.

A launch is **nested** when either of these holds:

- `AMPLIHACK_SESSION_DEPTH` is greater than 0 and `AMPLIHACK_TREE_ID` equals
  the tree id of a live slot whose process is this process's parent or
  another ancestor;
- `AMPLIHACK_GOVERNOR_PARENT` equals the tree id of a live slot whose
  process is this process's parent or another ancestor.

The ancestry is checked through `/proc` on Linux. Every child of an admitted
launch inherits the slot's tree id and, under a recipe, a session depth, so a
tmux server or a background process that a step starts holds both too. A
start from such an environment is not in the slot's process tree, so neither
variable nests it and it counts against the limit. Without `/proc` the
ancestry cannot be checked and a matching tree id is trusted.

Each slot records what holds it in its `start` field: `recipe` for a
`recipe run`, `agent` for `amplihack claude` or `amplihack launch`.

A nested launch:

- takes no new slot, and runs under its parent's slot;
- is not limited by the concurrency limit or the queue (steps 5 and 6);
- is not limited by the reserve (step 4) only when its parent slot is a
  `recipe run`'s; under an `agent` slot it waits at the reserve unless its
  class is protected;
- is still held by weekly exhaustion (step 2) and session exhaustion
  (step 3);
- is counted in the parent slot's `nested_children` in status.

A top-level `recipe run` usually starts without `AMPLIHACK_TREE_ID`. Before
it records its slot, admission settles the tree id: it uses the inherited
value if there is one, and otherwise generates one. The slot records that
id, and the environment built for the recipe's children reuses it rather
than generating another, so every agent step carries the slot's tree id.

A process that holds a slot of its own also sets `AMPLIHACK_GOVERNOR_PARENT`
to the slot's tree id in the environment it builds for its children. A
top-level `amplihack claude` or `amplihack launch` sets no session depth, so
this marker is what lets a `recipe run` started inside the session run under
the session's slot instead of waiting for a second one. A `recipe run --detach`
removes the marker from the run it starts in tmux, so a detached run started
from an interactive session asks for a slot of its own. One started from an
agent step also asks for a slot of its own: it still carries the step's
session depth and tree id, but tmux starts it outside the step's process
tree, so the ancestry check does not match.

A nested start holds no slot of its own. If it outlives its parent (for
example a background `recipe run` left running after an interactive session
ends), it runs on with no slot until it finishes; the parent's slot is
released when the parent exits.

A nested launch whose tree id has no live slot is treated as top-level and
needs a slot of its own.

Only starts inside a running recipe are exempt from the reserve. A recipe
that was admitted above the reserve and crosses it part-way through
finishes its remaining agent steps and any sub-recipe it starts with
`recipe run`, spending reserve points, rather than holding each step and
abandoning the run with the points already spent and a half-built worktree
left behind. A recipe is bounded work that was admitted once, so this
spends at most what that recipe still needs.

An interactive session is not bounded in the same way: it can stay open
for days and start a new `recipe run` (as dev-orchestrator does) or a new
launch at any time. Each of those is new work. It runs under the session's
slot, so it does not wait for a slot of its own, but an ordinary one waits
at the reserve. Without this rule a session admitted at 60% weekly use could
spend the whole reserve, and the reserve would gate only the start of
sessions and detached runs.

Because a nested launch can still wait on weekly or session exhaustion, an
agent step inside a running recipe can wait for up to the wait time, and
exits 80 in the middle of the run if that wait expires. An agent step
started while either limit is exhausted would fail against that limit
anyway, so the governor does not exempt nested launches from steps 2 and 3.

## Waiting, denial and exit code 80

A start told to wait polls every 15 seconds until it is admitted or the wait
runs out. The wait comes from `--governor-wait`, then
`AMPLIHACK_GOVERNOR_WAIT`, then `governor.wait_secs`, else 10 minutes.

While it waits, it prints a reason line to stderr, and prints another only
when the reason changes:

```text
amplihack: governor: waiting (concurrency): 4 of 4 slots are in use; giving up at 14:41:07 UTC
amplihack: governor: waiting (queue): an earlier request is first in the fair queue; giving up at 14:41:07 UTC
```

When the wait runs out, or the decision is deny, the command prints one line
and exits with **80** without starting the child:

```text
amplihack: governor: not started (weekly-exhausted): every eligible profile has used its weekly limit; the next weekly reset is at 2026-11-05 09:00 UTC
```

Exit code 80 means "not started for lack of capacity". Nothing was run and
nothing needs to be cleaned up; the same command can be run again later.
Admission writes nothing to stdout, so `amplihack claude -p` output is
unaffected.

### Reason codes

Reason codes are stable and appear in stderr lines and in status JSON.

| Code | Decision | Meaning |
|------|----------|---------|
| `room` | Admit | A profile in the pool has room; printed only when that profile is not the active one |
| `nested` | Admit | Admitted under the parent's slot: inside a running recipe, or with room above the reserve (or a protected class) under a session |
| `weekly-exhausted` | Wait or deny | Every usable profile's weekly window is used |
| `session-exhausted` | Wait | Every usable profile is at its session limit; the message names the reset time |
| `reserve` | Wait | Ordinary work that is top-level or nested under a session, and the best weekly remainder is at or below the reserve |
| `concurrency` | Wait | The slots in use are at the limit |
| `queue` | Wait | Another request comes first |
| `wait-expired` | Deny | A wait, for any of the reasons above, ran out; set by the admission loop, not the decision function |

### Failure modes

These faults admit the start without taking a slot and write the reason to
the log at warning level. Every fault except `no-root` and `no-readings` also
prints one line to stderr, sanitised and with no path:

```text
amplihack: governor: admitted without a decision (state-dir): <reason>
```

Neither of those two prints anything: `no-root` is the usual state of a host
with no login profiles, and `no-readings` admits with a slot and is shown by
`governor status`. Where the state can still be written
(`corrupt-state`, `no-readings` and `queue-full`), the reason is also recorded
in `state.json` and shown as `fail_open_reason` by status; `no-readings`
admits with a slot, so the concurrency limit still counts it. Status also
reports a corrupt or newer state file it finds, without changing it.

| Code | Cause |
|------|-------|
| `no-root` | No profile root is configured |
| `state-dir` | `.governor.d` cannot be created or is unsafe (a symlink, or owned by another user) |
| `lock-timeout` | The governor lock cannot be taken within 5 seconds |
| `corrupt-state` | A state file cannot be parsed or fails validation; it is renamed to `<name>.corrupt` and rebuilt |
| `state-version` | A state file has a newer `version` than this binary understands; it is left untouched |
| `no-readings` | No profile has a usable reading |
| `queue-full` | The queue already holds 4096 entries |

The decision function denies only at step 2: readings exist and every usable
profile is weekly-exhausted. Any wait, whatever its reason, can also end in
exit 80 with `wait-expired` when it runs out.

## Kill switch and governor keys

`AMPLIHACK_GOVERNOR` set to `off`, `0` or `false` turns the governor off. In
that state it reads and writes no state, prints nothing, and the commands
behave as they did before the governor existed. `on`, `1` or `true` turns it
on. The variable outranks `governor.enabled`.

The keys live in the user config file only. A `governor.*` key in a project
`.amplihack/config.toml` is reported and ignored, so a repository cannot turn
the governor off or loosen it. See [User Configuration](./user-config.md).

| Key | Type | Default | Overridden by |
|-----|------|---------|---------------|
| `governor.enabled` | bool | `true` | `AMPLIHACK_GOVERNOR` |
| `governor.max_concurrency` | integer, 1 to 64 | `4` | |
| `governor.reserve_points` | integer, 0 to 100 | `10` | |
| `governor.step` | integer, 1 to 8 | `1` | |
| `governor.band_pct` | integer, 0 to 50 | `10` | |
| `governor.eval_interval_secs` | integer, 1 to 3600 | `60` | |
| `governor.wait_secs` | integer, 0 to 86400 | `600` | `--governor-wait`, then `AMPLIHACK_GOVERNOR_WAIT` |

The governor reads `AMPLIHACK_GOVERNOR`, `AMPLIHACK_GOVERNOR_WAIT` and
`--governor-wait` itself, because they accept values the config file does
not (`off`, `15m`). They are not the keys' config-resolver variables or
flags, so `config get --show-origin` never reports `env` or `flag` for a
`governor.*` key, even when one of them is in effect.

```sh
amplihack config set governor.max_concurrency 2
amplihack config set governor.reserve_points 15
amplihack config set governor.enabled false
```

All defaults are starting values chosen for a host with a few logins; tune
them to your plans and workload.

### Environment variables

| Variable | Effect |
|----------|--------|
| `AMPLIHACK_GOVERNOR` | `off`, `0`, `false` turn the governor off; `on`, `1`, `true` turn it on |
| `AMPLIHACK_GOVERNOR_WAIT` | Default wait, same grammar as `--governor-wait` |
| `AMPLIHACK_GOVERNOR_CLASS` | Default class |
| `AMPLIHACK_GOVERNOR_MANIFEST` | Absolute path to a [workstream usage manifest](./workstream-usage.md); used only by `governor status` for `token_share` |
| `AMPLIHACK_WORKSTREAM` | Workstream identity |

## State files

State lives in `<profile root>/.governor.d/`, a directory with mode 0700. Each
file has mode 0600 and a `version` field. The governor writes every file
atomically while it holds `.governor.d/lock`, which is separate from the
rotator's lock.

| File | Contents |
|------|----------|
| `lock` | Empty; the exclusive lock for admission and release |
| `state.json` | `limit`, `last_evaluated_at`, `next_seq`, `slots[]`, `admissions[]` (the fairness window) and `fail_open` (the last fail-open reason) |
| `queue.json` | Waiting requests: `seq`, workstream, class, PID, process start time, `enqueued_at` |
| `history.json` | Up to 64 readings per profile and window (`weekly`, `session`), by profile name, for burn rates. Rings of profiles that are no longer listed are dropped. |

A `limit` of `0` in `state.json` means "not yet set"; the first admission
sets it to `governor.max_concurrency`.

The files hold profile names, workstream ids, classes, PIDs and timestamps
only. They hold no account keys, emails, task text, `claude` arguments or
paths.

Example `state.json`:

```json
{
  "version": 1,
  "limit": 3,
  "last_evaluated_at": 1793541720,
  "next_seq": 58,
  "slots": [
    {
      "pid": 41022,
      "start_time": 88213344,
      "workstream": "ws-1811",
      "class": "ordinary",
      "tree_id": "t-5c1e2a",
      "admitted_at": 1793541320,
      "nested_children": 3,
      "start": "recipe"
    }
  ],
  "admissions": [
    { "workstream": "ws-1811", "at": 1793541320 }
  ],
  "fail_open": null
}
```

### Dead processes

A slot or queue entry belongs to a process identified by its PID and, on
Linux, its start time from `/proc/<pid>/stat`. On each admission the governor
removes entries whose process has exited or whose PID now belongs to a
different process. It never signals another process: it only checks
existence with signal 0.

On hosts without `/proc`, such as macOS, only the PID is checked, so a reused
PID keeps a dead slot until that process also exits. On Linux, a slot whose
recorded start time can no longer be read (for example under `hidepid=`)
counts as dead.

A slot is held for the whole life of the admitted process, so a forgotten
interactive session holds a slot. `governor status` shows when each slot was
admitted; end the session, or start other work with `AMPLIHACK_GOVERNOR=off`.

## governor status

```text
amplihack governor status [--json]
```

Status is read-only. It takes no lock, writes nothing, removes no dead
entries and renames no corrupt files. A dead slot or queue entry is shown
with `stale`. With no profile root, or with only `HOME` set, it reports the
governor state and creates nothing.

The table shows `no session window` for weekly-only profiles and
`invalid reading` for a profile whose reading cannot be trusted. Profile names
and other strings are printed with control characters replaced by `?` and
are cut to 64 characters.

### JSON output

`--json` prints one object with a stable schema, `version` 1:

```json
{
  "version": 1,
  "generated_at": 1793541780,
  "enabled": true,
  "limit": 3,
  "max": 4,
  "profiles": [
    {
      "name": "alpha",
      "active": true,
      "eligible": true,
      "session_window": true,
      "invalid_reading": false,
      "session": {
        "used_pct": 62.0,
        "resets_at": 1793548800,
        "burn_rate": 9.5,
        "sustainable_rate": 19.5,
        "forecast_exhaustion": 1793556171
      },
      "weekly": {
        "used_pct": 71.0,
        "resets_at": 1793782800,
        "burn_rate": 2.1,
        "sustainable_rate": 0.4,
        "forecast_exhaustion": 1793591494
      },
      "burn_rate": 2.1,
      "sustainable_rate": 0.4,
      "forecast_exhaustion": 1793591494
    },
    {
      "name": "beta",
      "active": false,
      "eligible": true,
      "session_window": false,
      "invalid_reading": false,
      "session": null,
      "weekly": {
        "used_pct": 38.0,
        "resets_at": 1793955600,
        "burn_rate": null,
        "sustainable_rate": 0.5,
        "forecast_exhaustion": null
      },
      "burn_rate": null,
      "sustainable_rate": 0.5,
      "forecast_exhaustion": null
    }
  ],
  "reserve": {
    "points": 10,
    "best_weekly_remaining": 62.0,
    "ordinary_held": false
  },
  "slots": [
    {
      "pid": 41022,
      "workstream": "ws-1811",
      "class": "ordinary",
      "admitted_at": 1793541320,
      "nested_children": 3,
      "stale": false
    }
  ],
  "queue": [
    {
      "seq": 57,
      "workstream": "ws-1811",
      "class": "ordinary",
      "pid": 41590,
      "enqueued_at": 1793541660,
      "stale": false
    }
  ],
  "workstreams": [
    {
      "name": "ws-1811",
      "admissions": 4,
      "slots": 1,
      "waiting": 1,
      "token_share": null
    }
  ],
  "fail_open_reason": null
}
```

| Field | Meaning |
|-------|---------|
| `enabled` | `false` when the kill switch or `governor.enabled` turns the governor off. `limit` is then `null` and every list is empty. |
| `limit`, `max` | Current concurrency limit and `governor.max_concurrency` |
| `profiles[]` | Every listed profile, including those left out of the pool; `eligible` is `false` for overflow and API-key profiles |
| `profiles[].session_window` | `false` for a weekly-only profile (a week window and no session window); `session` is then `null`. Not "no session limit": the reading cannot tell the two apart |
| `profiles[].invalid_reading` | `true` when a window holds a percentage that is not a finite number between 0 and 100; the profile is then out of the pool and `session` and `weekly` are `null` |
| `profiles[].weekly` | `null` for a profile with no usable weekly reading; such a profile is not part of the pool |
| `profiles[].burn_rate`, `sustainable_rate`, `forecast_exhaustion` | The weekly window's values, repeated for scripts; `null` when unknown |
| Window objects | `used_pct`, `resets_at` (Unix seconds or `null`), and the window's rate fields |
| `reserve` | Reserve points, the best usable weekly remainder, and whether ordinary work is currently held |
| `slots[]`, `queue[]` | As in the state files, without start times or tree ids, plus `stale` |
| `workstreams[].admissions` | Admissions in the 24-hour fairness window |
| `workstreams[].token_share` | For the workstream the manifest named by `AMPLIHACK_GOVERNOR_MANIFEST` describes: its attributed tokens divided by every observed token in the manifest's sessions, so unattributed tokens lower the share. `null` for other workstreams, without a manifest, or when the manifest is not an absolute path to a regular file of the current user of at most 4096 bytes. The manifest path is never printed. |
| `fail_open_reason` | `null`, or `{ "code": "...", "message": "..." }` for the last fail-open |

Status never prints account keys, emails, task text or `claude` arguments.

## Fail-open compared with overflow

The governor and the rotator's overflow gate
([#1781](https://github.com/rysweet/amplihack-rs/pull/1781)) treat an unknown
reading in opposite ways, on purpose.

| | Rotator overflow gate | Capacity governor |
|---|---|---|
| Question | May the login move to a credit-backed or API-key profile? | May this work start now? |
| Unknown reading | Holds: overflow is not used | Admits: the start goes ahead |
| Cost of a wrong yes | Money spent per token on a guess | Work starts that may hit a usage limit, which the [usage-limit wait](./profile-command.md#usage-limit-wait) already handles |
| Cost of a wrong no | A short delay before overflow is used | No work can start on a host whose probes do not work |

Overflow decides whether to spend money, so it fails closed. The governor
decides only when work starts, and an internal fault must not stop all work,
so it fails open. It holds only on known weekly exhaustion.

## Security model

The governor schedules work for one user. It is not a security boundary
against other processes of the same user: any parent can set
`AMPLIHACK_GOVERNOR=off` or a higher class.

- **Writes.** Only inside `.governor.d`, with fixed file names. The
  directory must be a real directory owned by the current user; the governor
  refuses a symlink and never changes permissions. The lock is opened without
  following symlinks and must be a regular file owned by the current user.
  A corrupt file is renamed only to its fixed `.corrupt` name.
- **Reads.** State files are read without following symlinks and are capped
  at 1 MiB; a larger file is treated as corrupt. Every field is validated on
  load: finite percentages from 0 to 100, plausible timestamps, a limit of at
  most 64, unique ids and `seq` values.
- **Profiles.** The governor reads usage readings, the default-login pointer
  and the ledger through a read-only view. It never writes `usage.json`, the
  pointer, the ledger, `.credentials.json`, `.claude.json` or any profile
  directory, never takes the rotator's lock and starts no processes. The only
  change it makes to the child's environment is the `AMPLIHACK_TREE_ID` it
  settles for [nesting](#nesting), which the environment builder would
  otherwise have generated.
- **Processes.** Signal 0 is the only signal used, and only for PIDs from 2
  to `i32::MAX`. A process the governor may not signal counts as dead, and
  so does a slot whose recorded start time cannot be checked. Without
  `/proc`, PID reuse goes undetected.
- **State files.** Files are created 0600 in a 0700 directory. They hold
  workstream ids, classes, PIDs, start times, tree ids and readings, and no
  secrets.
- **Reserve.** The reserve is advisory. `[coord]` as the first word of a
  `claude -p` task, `--governor-class` or `AMPLIHACK_GOVERNOR_CLASS` raises
  a start's class and so its access to the reserve. Recipe task text is
  not read, so anyone who can open an issue that a recipe run picks up
  cannot claim the reserve.
- **Nesting.** A process nests under a live slot, and is not counted against
  the concurrency limit (nor held by the reserve, when the slot is a
  `recipe run`'s), only when the slot's process is its parent or
  another ancestor (on Linux). A tree id or parent marker that leaked into
  an unrelated environment, such as a tmux server started from an agent
  step, therefore nests nothing. A process of the same user can still nest
  on purpose by running under the slot's process; that is accepted under
  the same-user model above.
- **Input.** Workstream ids, wait durations, classes and tree ids are
  validated before use. Classes inferred from recipes come from fixed lists.
  The manifest must be an absolute path to a regular file owned by the
  current user.
- **Output.** No account keys, emails, task text, `claude` arguments,
  invalid environment values or manifest paths appear in state, status or
  logs. Strings printed to the terminal or written to the log are
  sanitised, and fail-open messages are cut to 512 bytes and never quote
  the content of a corrupt file.
- **Availability.** Every internal fault admits the start. Waits are
  bounded; a wait of `0` never sleeps. Releasing a slot never panics.

## Related

- [`amplihack profile` Reference](./profile-command.md): Usage readings, the rotator, overflow and the usage-limit wait
- [Rotate Claude Logins](../howto/claude-profile-rotation.md): The rotator that moves the login to the profile with room
- [`amplihack recipe` Reference](./recipe-command.md): Where `recipe run` asks for admission
- [Workstream token usage](./workstream-usage.md): The manifest that supplies `token_share`
- [User Configuration](./user-config.md): The `governor.*` keys
- [Environment Variables](./environment-variables.md): `AMPLIHACK_GOVERNOR` and related variables
