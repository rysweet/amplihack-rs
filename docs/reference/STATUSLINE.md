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

`amplihack install` (explicit, global scope) sets the `statusLine` entry in
`~/.claude/settings.json` to the amplihack command:

```json
{
  "statusLine": {
    "type": "command",
    "command": "'/usr/local/bin/amplihack' profile statusline"
  }
}
```

That one command prints both lines: the session line from
`~/.amplihack/.claude/tools/statusline.sh` described above, then the
[login capacity line](#login-capacity-line). See
[Install wiring](#install-wiring) for when install writes, updates or keeps
the entry.

## Login capacity line

When the login rotator has moved a profile's login into `~/.claude`, the
status line shows which login is live and how much of its capacity is used:

```
~/src/amplihack-rs (main → origin) [github.com/rysweet/amplihack-rs] Opus 🎫 1.2M 💰$15.67 ⏱1h
🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%  · 2 spare
```

### Line format

```
🔑 <login>  5h <bar> <pct>%  7d <bar> <pct>%  · <n> spare
```

Segments are separated by two spaces.

| Segment           | Shows                                          | Notes                                                                                                                 |
| ----------------- | ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `🔑 <login>`      | The profile whose login is live in `~/.claude` | Always present when the line is printed.                                                                              |
| `5h <bar> <pct>%` | The five-hour session window                   | Left out when the input has no `five_hour` window.                                                                    |
| `7d <bar> <pct>%` | The seven-day window                           | Left out when the input has no `seven_day` window.                                                                    |
| `· <n> spare`     | Profiles with room in both windows             | `· 0 spare` is printed. Left out when the readings file cannot be read (see [Failure behaviour](#failure-behaviour)). |

When both windows are missing, the line is not printed.

The bar has 10 cells. The number of filled cells (`█`) is
`floor(clamp(pct, 0, 100) / 10)` and the rest are empty (`░`), so a full bar
appears only at 100%. The percentage is rounded half away from zero.

| Used | Printed           |
| ---- | ----------------- |
| 34   | `███░░░░░░░ 34%`  |
| 43   | `████░░░░░░ 43%`  |
| 42.5 | `████░░░░░░ 43%`  |
| 100  | `██████████ 100%` |
| 0    | `░░░░░░░░░░ 0%`   |

#### The 100% cap

Percentages are read from the status-line input and clamped to `0`–`100`
by the same parser auto-switch uses to record readings. An account past its
limit therefore shows `██████████ 100%`, never a higher figure.

### Data sources

| Value       | Source                                                                                                                                                         |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Login name  | The rotator ledger: `<root>/.auto-switch.d/rotate/ledger.json` (written by `profile rotate`) or `<root>/.rotator/ledger.json` (written by an external rotator) |
| Percentages | The `rate_limits` object in the JSON Claude Code sends on standard input                                                                                       |
| Spare count | Recorded readings in `<root>/.auto-switch.d/usage.json`                                                                                                        |

`<root>` is the profile root (see
[Profile root](./profile-command.md#profile-root)). The ledger is read with
the trust rules in
[Logins in `~/.claude`](./profile-command.md#logins-in-claude): it must be
owned by you, not writable by other users, not a symlink, and must name
a valid profile. When both ledgers are trusted, the one with the later
`since` decides. A ledger that names a profile whose own
`<root>/<name>/.credentials.json` still exists is not trusted, as in
`profile usage`; that check tests only whether the file exists.

As in `profile usage`, a rotate ledger that records an `account` counts only
while that account is the one in `~/.claude.json`. If someone runs
`claude /login` by hand in `~/.claude` as another account, or
`~/.claude.json` is missing, too large or unreadable, the ledger is not
trusted: no capacity line is printed and nothing is recorded. That file is
read once, bounded and without following a symlink. Unlike `profile usage`,
the status line never falls back to matching accounts across profiles,
reads no profile's `.claude.json` and does not list the profile
directories.

Credentials are never opened. The only check made on
`~/.claude/.credentials.json` is that it exists as a regular file.

The percentages shown are the current session's; nothing is fetched from the
network.

### Recording readings

Each run that resolves a login also records the input's reading for that
login in `usage.json`, in the same format and under the same 50 ms state
lock as the [status-line wrapper](./profile-command.md#status-line-wrapper)
(`source` is `statusline`). It does this whether auto-switch is on or off,
because the readings feed the spare count and `profile auto-switch`.

The reading is credited to the login the ledger names. It does not use the
auto-switch `hosts` attribution, and it never runs auto-switch recovery or
an in-session swap: the rotator owns `~/.claude`. It writes nothing else; the
journal, the ledgers and `hosts` are left untouched.

In the first seconds after the rotator swaps logins, Claude Code may still
send the previous login's figures, and that reading is credited to the new
login. The next status-line refresh corrects it.

`profile rotate` does not read `usage.json`. Readings taken in `~/.claude`
reach the rotator only indirectly, through the spare count and auto-switch.

### Spare rule

A profile counts as spare when both of these hold:

- its five-hour window is under 50% used, and
- its seven-day window is under 90% used.

Both comparisons are strict, so a profile at exactly 50% or 90% is not
spare. A window whose `resets_at` has passed counts as 0%. A profile with
either window missing from its reading is not spare.

Candidates are the entries in `usage.json` other than the live login, whose
names are valid profile names and whose `<root>/<name>` directory exists.
Profiles with no recorded reading are not counted. Run
`amplihack profile usage` to record readings for every profile.

### No-owner mode

`amplihack profile statusline` with no `--owner` is the command written to
`~/.claude/settings.json`. Each run:

1. reads standard input once, up to 16 MiB;
2. if `~/.amplihack/.claude/tools/statusline.sh` passes the checks below,
   runs it as `sh -c '<path>'` (the path single-quoted, as the
   [status-line wrapper](./profile-command.md#status-line-wrapper) runs an
   original), feeding it the same input and copying its output through. The
   script's `#!` line chooses its interpreter, so it must be executable;
3. resolves the profile root and the live login, records the reading and
   prints the capacity line.

Only the first 1 MiB is parsed for `rate_limits`. Input larger than 1 MiB is
still passed to `statusline.sh` in full, but gives no capacity line and
records nothing.

Both arguments are optional:

| Argument         | Effect                                                                                                                                                       |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `--root <path>`  | Uses this profile root. It must be absolute; a relative path means no root, and step 3 is skipped.                                                           |
| (none)           | Resolves the root as other profile commands do: `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then the saved `profile root` setting, then `~/.amplihack/claude-profiles`. |
| `--owner <name>` | Selects wrapper mode, used inside profiles (see [Inside profiles](#inside-profiles)). |

In wrapper mode `--root` is required. `--owner` without `--root` prints
nothing on standard output and one `status line disabled` line on stderr.

The root must already exist and pass the profile-root checks (a real
directory owned by you and not writable by other users). The command never
creates it.

`statusline.sh` is run only if it is a regular file, not a symlink, owned by
you or root, and not writable by other users, and each directory above
it up to `$HOME` passes the same ownership and mode checks. Otherwise the
first line is left out and the capacity line is still printed.

#### The inner-run rule

If `CLAUDE_CONFIG_DIR` is set and resolves to a directory other than
`$HOME/.claude` (symlinks resolved, a trailing slash ignored), the command
is running inside a profile. It then prints only the first line, and
records and prints no capacity line. This stops the line from appearing
twice when a profile's wrapper runs the command as its original.

### Inside profiles

New profiles receive the `~/.claude` entry through the shared settings
keys. `install` and `repair` treat an entry that runs `profile statusline`
without `--owner` as an ordinary original status line: they save it and
install the profile's
[wrapper](./profile-command.md#status-line-wrapper) around it. It is not
reported as stale.

When the wrapper's saved original is this command, the wrapper prints, in
order:

1. the original output (the `statusline.sh` line; the inner run prints no
   capacity line);
2. the capacity line for the account live in that profile, as the
   auto-switch ledger records it, falling back to the profile's own name;
3. the [capacity notice](./profile-command.md#capacity-notice), when it is
   due.

The spare count excludes that account. The wrapper reads `usage.json`
without taking the state lock, and leaves the spare segment out when the file
is missing or damaged. A wrapper whose saved original is any other command
prints exactly what it printed before.

A profile that runs `amplihack profile statusline` with no wrapper around it
shows only the first line, because of the inner-run rule. Profiles created
before this change keep their existing status line; nothing is copied into
them.

### Failure behaviour

Every failure gives less output, never an error on standard output.

| Condition                                                                                           | Result                                                                                    |
| --------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `statusline.sh` missing or failing its checks                                                       | No first line; capacity line printed.                                                     |
| No root resolves, or the root is relative, a symlink, or writable by other users                    | First line only. Nothing is recorded.                                                     |
| No ledger, a damaged or untrusted ledger, or a ledger naming a profile that still has its own login | First line only. Nothing is recorded.                                                     |
| A rotate ledger whose `account` is not the one in `~/.claude.json`                                  | First line only. Nothing is recorded.                                                     |
| Input has no `rate_limits`, or neither window                                                       | First line only.                                                                          |
| Input larger than 1 MiB                                                                             | First line only. Nothing is recorded.                                                     |
| `usage.json` damaged, or the state lock held for more than 50 ms                                    | Capacity line without the `· <n> spare` segment. The reading is not recorded on that run. |
| Internal panic                                                                                      | First line only, and one generic line on stderr.                                          |

The exit code is that of `statusline.sh` when it ran (`1` if a signal ended
it), and `0` otherwise. In no-owner mode the command prints no paths and adds
no stderr lines of its own. On non-Unix platforms (Windows) it prints nothing
and exits `0`.

The capacity work (resolving, reading the ledger, recording and rendering)
takes well under 200 ms; `statusline.sh` runs separately and is not counted
in that figure.

### Install wiring

Only `amplihack install` in explicit mode with global scope writes the
entry. Launch-time auto-repair, implicit installs and repo-local
`.claude/settings.json` files never add, change or remove `statusLine`.

| Existing `statusLine` in `~/.claude/settings.json` | What install does                                        | Printed                                                           |
| -------------------------------------------------- | -------------------------------------------------------- | ----------------------------------------------------------------- |
| None                                               | Writes `{"type": "command", "command": "<cmd>"}`         | `statusLine: installed`                                           |
| An amplihack entry with an older command           | Replaces the entry, keeping only `padding`               | `statusLine: updated`                                             |
| The current command                                | Nothing                                                  | `statusLine: unchanged`                                           |
| Anything else                                      | Nothing; the entry is kept byte for byte                 | `statusLine: kept your existing entry in ~/.claude/settings.json` |

An entry is amplihack's when it is an object whose `command` either
contains `profile statusline` without `--owner`, or contains
`.amplihack/.claude/tools/statusline.sh`. The second test matches any
command that mentions that path, including your own pipeline built around
the script, and such an entry is replaced; the timestamped backup of
`settings.json` that install writes keeps the old entry. Fields other than
`type`, `command` and `padding` are dropped. Everything else is kept as
yours, including:

- the older example `$CLAUDE_PROJECT_DIR/.claude/tools/statusline.sh`;
- a profile wrapper (`profile statusline ... --owner ...`) copied in by hand;
- a value that is not an object or has no `command`.

To switch a kept entry to amplihack's, delete the `statusLine` key and run
`amplihack install` again.

`<cmd>` is the installed amplihack binary followed by `profile statusline`,
with the path single-quoted for `sh`. If the binary cannot be located or
fails its ownership checks, install prints
`⚠️  statusLine not installed: <reason>`, leaves the key alone and still
succeeds. Running install again gives the same file, with one `statusLine`
key. Other keys keep their values; their order may change.

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
