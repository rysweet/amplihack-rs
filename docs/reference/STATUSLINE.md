# Statusline Reference

Real-time session information bar displayed at the bottom of Claude Code interface.

## Overview

The statusline shows progress, costs, context usage, and active features for your Claude Code session. When a login rotator manages `~/.claude`, a second line shows the live login and its capacity.

## Indicators Reference

| Indicator             | Shows                     | Format                                      | Notes                                                   |
| --------------------- | ------------------------- | ------------------------------------------- | ------------------------------------------------------- |
| **Directory**         | Current working directory | `~/path`                                    | `~` = home directory                                    |
| **Git Branch**        | Branch name and status    | `(branch → remote)` or `(branch* → remote)` | `*` = uncommitted changes, Cyan = clean, Yellow = dirty |
| **Repository URI**    | Git remote repository URL | `[github.com/user/repo]`                    | Shortened format (cyan), only if remote exists          |
| **Model**             | Active Claude model       | `Opus`, `Sonnet`, `Haiku`                   | Red=Opus, Green=Sonnet, Blue=Haiku                      |
| **Tokens** 🎫         | Total token usage         | `234K`, `1.2M`, or raw number               | M=millions, K=thousands                                 |
| **Cost** 💰           | Total session cost        | `$1.23`                                     | USD                                                     |
| **Duration** ⏱        | Session elapsed time      | `15m`, `1h`, `30s`                          | s/m/h format                                            |
| **Power-Steering** 🚦 | Redirect count            | `🚦×3`                                      | Only when active (purple)                               |
| **Lock Mode** 🔒      | Lock invocation count     | `🔒×5`                                      | Only when active (yellow)                               |

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
| Unset, empty, or `$HOME/.claude`                                                       | Home        | Runs `statusline.sh`, then prints the capacity line for the login live in `~/.claude`.   |
| `<root>/<p>`, where the canonical parent is the canonical root and `<p>` is a valid profile name | Profile     | Behaves exactly like `--owner <p>`. See [Inside profiles](#inside-profiles).             |
| Anything else                                                                          | Passthrough | Runs `statusline.sh` only. No capacity line and nothing recorded.                        |

A directory reached through a symlink is matched on its canonical path, so a
symlink to a profile from outside the root is treated as passthrough unless
it resolves into the root.

With `--owner`, the command always runs in profile mode for that profile.

### Output order

The order is the same in every mode:

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
| `· <n> spare`     | Profiles with room in both windows        | The profile list, auto-switch state or `usage.json` cannot be read. |

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
| Spare count | The profile list, the auto-switch settings and ledger, the overflow marks and the readings in `<root>/.auto-switch.d/usage.json`                               |

`<root>` is the profile root (see
[Profile root](./profile-command.md#profile-root)).

Credentials are never read. The capacity line does not open, test or name
any `.credentials.json` file, and does not read `~/.claude.json` or any
profile's `.claude.json`.

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
previous login until the rotator next writes its ledger. Recording is
protected separately; see [Recording readings](#recording-readings).

### Recording readings

In home mode the command records the input's reading in `usage.json` only
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
`statusline`, merged with any existing reading in the same way as the
[status-line wrapper](./profile-command.md#status-line-wrapper). The command
waits at most 50 ms for the state lock; when the lock is busy, it records
nothing on that run. After recording it updates the
[capacity file](./profile-command.md#capacity-file) with writer
`statusline`. Recording happens whether auto-switch is enabled or not.

When any condition fails, nothing is recorded. The capacity line is still
printed, with the bars from this run's input.

Home mode never runs auto-switch recovery or an in-session swap, because the
rotator owns `~/.claude`. It writes nothing other than `usage.json` and the
capacity file: the journal, the ledgers and `hosts` are left untouched.

`profile rotate` does not read `usage.json`. Readings taken in `~/.claude`
reach it only through the spare count and auto-switch.

### Spare rule

Candidates are the profiles that `amplihack profile list` shows, other than:

- the live login;
- profiles in the auto-switch `exclude` list;
- profiles marked for [overflow](./profile-command.md#overflow-commands).

For each candidate, the reading is looked up in `usage.json` under the
account the auto-switch ledger records for that profile, or under the
profile's own name when it has no entry. A candidate counts as spare when
all of these hold:

- it has a reading;
- it is not exhausted: its reading is not a usage-limit failure whose reset
  time is still in the future;
- its five-hour window is under 50% used, or the reading has no five-hour
  window;
- its seven-day window is present and under 90% used.

Both comparisons are strict, so a profile at exactly 50% or 90% is not
spare. A window whose reset time has passed counts as 0%. Run
`amplihack profile usage` to record readings for every profile.

When home mode has just recorded a reading, the count uses the `usage.json`
it saved under the state lock; if the save failed, the spare segment is left
out. Otherwise the count reads its files without taking the state lock. A
damaged or half-written `usage.json` leaves the spare segment out for that
run. A missing `usage.json` holds no readings, so every candidate counts as
not spare.

### Inside profiles

In profile mode, for profile `<p>`, the command:

1. applies the [status-line wrapper](./profile-command.md#status-line-wrapper)
   checks: the root is private and `CLAUDE_CONFIG_DIR` resolves to
   `<root>/<p>`;
2. waits at most 50 ms for the state lock, runs recovery, records the
   reading when auto-switch is on and checks for an in-session swap, as the
   wrapper always has;
3. runs the saved original from `<root>/.auto-switch.d/statusline/<p>.json`.
   An original whose command contains `profile statusline` is never run;
   `statusline.sh` is run in its place. When the command was started without
   `--owner` and there is no saved original, `statusline.sh` is run too;
4. prints the capacity line, when the step 1 checks passed: the account the
   auto-switch ledger records for `<p>` as the login, falling back to `<p>`,
   the bars from this run's input, and the spare count excluding `<p>`;
5. prints the capacity notice, when it is due.

New profiles receive the `~/.claude` entry through the shared settings keys,
so a profile without a wrapper still gets the capacity line for its own
login. Seeding is unchanged.

When `profile auto-switch install` or `repair` finds the `~/.claude` entry
in a profile, it saves `{"type": "command", "command": "'<home>/.amplihack/.claude/tools/statusline.sh'"}`
as the original and installs the wrapper. The script path is saved only when
it passes the [script checks](#script-checks); otherwise no original is
saved and the wrapper prints only the capacity line. A command containing
`profile statusline` is never saved as an original.

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
| `usage.json` or auto-switch state damaged or unreadable                   | No spare segment.                                                         |
| State lock held for more than 50 ms                                       | Capacity line printed. The reading is not recorded on that run.          |
| Internal panic                                                            | The original command's output only, and one generic line on stderr.      |

The exit code is the original command's when it ran (`1` if a signal ended
it), and `0` otherwise. Logs name operations and errors only, never input,
file contents or environment values. On non-Unix platforms the command
prints nothing and exits `0`.

The capacity work (resolving the login, counting spares, rendering and
recording) takes under 200 ms with 20 or more profiles. Process start-up and
the original command are not counted.

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

`<cmd>` is the installed amplihack binary followed by `profile statusline`,
with the path single-quoted for `sh`. When the binary cannot be located or
is refused (it or a directory above it can be changed by other users), or
its path cannot be quoted safely (for example, it contains a newline),
install prints a warning and writes no `statusLine` entry. It never falls
back to a bare `amplihack` found through `PATH`. The `statusLine` decision
never makes install fail.

Running install again gives the same file, with one `statusLine` key. Other
keys keep their values; their order may change. `settings.json` is written
in place after the usual timestamped backup, not through a temporary file.

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
- [Login capacity line](#login-capacity-line) - Live login and capacity in `~/.claude`
- [Profile command](./profile-command.md#status-line-wrapper) - Per-profile status-line wrapper and readings
- [Configuration Guide](../howto/configure-hooks.md) - Session hooks and settings
- [Development Workflow](../concepts/default-workflow.md) - Process customization
