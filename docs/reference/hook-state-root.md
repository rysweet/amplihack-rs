---
title: Hook State Root Reference
description: Hook state paths, Stop controls, counter synchronisation and storage failure behaviour.
last_updated: 2026-11-12
doc_type: reference
related_issues: ["#1963", "#2053", "#2361", "#2368"]
---

# Hook State Root Reference

The `amplihack-hooks` binary keeps its own state (tool metrics, workflow
enforcement state, captured original requests, pre-compact transcripts,
compaction metadata, classification state, Stop lock counters, power-steering
state, reflection output and the blarify staleness marker) under one resolved
state root. Generated hook state stays outside the project, subject to the
[state-root resolver](#resolving-the-state-root) and the resolver and symlink
limitations in [Security notes](#security-notes). This does not guarantee an
unchanged repository git status: the Stop safety valve deliberately removes
the four project lock controls listed under
[Continuation, completion and presentation](#continuation-completion-and-presentation).
Other project changes are listed under
[What still touches the working directory](#what-still-touches-the-working-directory).

## Contents

- [Quick check](#quick-check)
- [Resolving the state root](#resolving-the-state-root)
- [Directory layout](#directory-layout)
- [Session keys](#session-keys)
- [Project keys](#project-keys)
- [Files written by each hook](#files-written-by-each-hook)
- [Stop state and controls](#stop-state-and-controls)
- [Stop counter synchronisation](#stop-counter-synchronisation)
- [Directory permissions](#directory-permissions)
- [When no state root resolves](#when-no-state-root-resolves)
- [Write failures](#write-failures)
- [What still touches the working directory](#what-still-touches-the-working-directory)
- [Rust API](#rust-api)
- [Security notes](#security-notes)
- [Related](#related)

## Quick check

Run the PostToolUse hook from a fresh git repository with a throwaway home
directory, then confirm that the repository is clean and the metric landed in
the state root:

```sh
home=$(mktemp -d)
repo=$(mktemp -d) && git -C "$repo" init -q
cd "$repo"
echo '{"hook_event_name":"PostToolUse","session_id":"demo-1","tool_name":"Bash","tool_input":{"command":"ls"}}' \
  | env -u AMPLIHACK_HOME HOME="$home" amplihack-hooks post-tool-use

git status --porcelain
# (no output)

cat "$home/.amplihack/runtime/sessions/demo-1/metrics/post_tool_use_metrics.jsonl"
# {"timestamp":"1794046502","tool_name":"Bash","category":"bash_commands","session_id":"demo-1","hook":"post_tool_use"}
```

## Resolving the state root

The state root is resolved once per hook invocation from the environment. The
first rule that matches wins.

| Priority | Source | Accepted when | Result |
| --- | --- | --- | --- |
| 1 | `AMPLIHACK_HOME` | The value is an absolute path with no `..` component, and `<value>/.git` does not exist | `$AMPLIHACK_HOME` |
| 2 | `HOME` | The value is an absolute path with no `..` component | `$HOME/.amplihack` |
| 3 | Neither accepted | | No state root; see [When no state root resolves](#when-no-state-root-resolves) |

Values are compared exactly as the operating system returns them:

- Values are not trimmed. A value with a leading space is relative and is
  skipped. A value with a trailing space is used as written, space included.
  An empty value is not absolute and is skipped.
- Values are not canonicalised and symlinks are not resolved.
- Values do not need to be valid UTF-8.
- A relative `AMPLIHACK_HOME` such as `.amplihack` or `../amplihack` is
  ignored, and the root falls back to `$HOME/.amplihack`.
- The working directory and `.` are never used as a state root.

The same `..` rule applies to `AMPLIHACK_HOME` when the launcher sets it; see
[`AMPLIHACK_HOME`](./environment-variables.md#amplihack_home).

Check which root a hook will use:

```sh
root="$HOME/.amplihack"
case "${AMPLIHACK_HOME:-}" in
  /*) if [ ! -e "$AMPLIHACK_HOME/.git" ] && [ ! -L "$AMPLIHACK_HOME/.git" ]; then
        root="$AMPLIHACK_HOME"
      fi ;;
esac
echo "$root"
# /home/alice/.amplihack
```

This shell check does not reject `..` components, and treats an unreadable `.git` as absent; the hooks reject both.

### `AMPLIHACK_HOME` pointing at a Git checkout

The launcher and `recipe run -w` can set `AMPLIHACK_HOME` to an amplihack-rs
source checkout (a directory containing `amplifier-bundle/`). Bundle and asset
lookup still use that checkout. Runtime state does not: when
`<AMPLIHACK_HOME>/.git` exists as a directory, file or symlink, or cannot be
checked, hooks write under `$HOME/.amplihack/runtime/` instead, and skip the
write when `HOME` is unusable. The launch contract file follows the same
checkout rule, except that with no usable `HOME` the launch fails instead of
skipping the file; see
[Agent Prompt Channels Reference](./agent-prompt-channels.md#errors). This
keeps runtime files out of checkouts that recipe steps stage (#2361).

The probe is one `lstat` of `<AMPLIHACK_HOME>/.git`. It does not run git and
does not look at parent directories, so a linked worktree (where `.git` is a
file) and a submodule are skipped, but a subdirectory of a checkout is not.

```sh
home=$(mktemp -d)
checkout=$(mktemp -d) && git -C "$checkout" init -q
cd "$checkout"
echo '{"hook_event_name":"PostToolUse","session_id":"demo-2","tool_name":"Bash","tool_input":{"command":"ls"}}' \
  | AMPLIHACK_HOME="$checkout" HOME="$home" amplihack-hooks post-tool-use

git status --porcelain
# (no output)

ls "$home/.amplihack/runtime/sessions/demo-2/metrics/"
# post_tool_use_metrics.jsonl
```

Releases before #2361 wrote hook state to `<checkout>/runtime/` and the
contract file to `<checkout>/private/`. Those directories are not moved or
deleted. The amplihack-rs `.gitignore` keeps ignoring both with root-anchored
`/runtime/` and `/private/` entries; remove them by hand if you no longer
need them.

## Directory layout

```text
<state root>/
└── runtime/
    ├── sessions/
    │   └── <session key>/
    │       ├── classification_state.json
    │       ├── workflow_state.json
    │       ├── metrics/
    │       │   └── post_tool_use_metrics.jsonl
    │       ├── locks/
    │       │   ├── lock_invocations.txt
    │       │   └── lock_invocations.json.lock
    │       ├── power-steering/
    │       │   ├── session_count
    │       │   ├── session_count.json.lock
    │       │   ├── .<session key>_completed
    │       │   └── summary.md
    │       ├── logs/
    │       │   ├── ORIGINAL_REQUEST.md
    │       │   ├── original_request.json
    │       │   ├── transcript_pre_compact.jsonl
    │       │   ├── compaction_metadata.jsonl
    │       │   └── FEEDBACK_SUMMARY.md
    │       └── reflection/
    │           ├── reflection_<session key>_<epoch seconds>.md
    │           ├── current_findings.md
    │           └── .reflection_presented_<session key>
    └── projects/
        └── <project key>/
            └── blarify_stale
```

The record formats inside each file are the same as they were when the files
lived under `<project>/.claude/runtime/`. Only the location changed.

## Session keys

`<session key>` is always exactly one path component made of
`[A-Za-z0-9_-]` characters, directly under `runtime/sessions/`.

| Session id received | Session key |
| --- | --- |
| `demo-1` | `demo-1` |
| `lane 3` | `lane_3` |
| ` a ` | `_a_` |
| `..` | `__` |
| `.` | `_` |
| `../../escape` | `______escape` |
| `/abs` | `_abs` |
| `a/b` | `a_b` |
| `x` NUL `y` | `x_y` |
| absent, empty or only whitespace | `unknown-session` |

Every character outside `[A-Za-z0-9_-]` becomes `_`. The fallback key is chosen
before the id is sanitised, so an empty or whitespace-only id never reaches the
sanitiser.

Two fallback keys are in use when the payload carries no session id:

| Fallback key | Used by |
| --- | --- |
| `unknown-session` | Tool metrics, original request, classification state, Stop lock counters, power steering and reflection |
| `current` | Workflow enforcement state, so that a `/dev` command and the PostToolUse calls that follow it in the same id-less session share one `workflow_state.json` |

In one PostToolUse call without a session id, the workflow state is therefore
written under `sessions/current/` and the metric under
`sessions/unknown-session/`. This is intended.

Without a session id, SessionStart saves the original request under
`sessions/unknown-session/logs/`, but the `session_id` field inside
`original_request.json` is still `session-start`, as before. The key chooses
the directory only and does not change the record.

PreCompact generates a session id of the form `session-<epoch seconds>` when
the payload has none, and uses it as the key.

Stop uses the payload's session id through `session_state_dir`. Missing,
empty and whitespace-only identifiers consistently reuse `unknown-session`;
`CLAUDE_SESSION_ID` and timestamps do not supply a storage identity. Repeated
invocations with the same resolved key reuse counters, completion markers and
reflection presentation state. Distinct keys have independent state, while
identifiers such as `lane 3` and `lane_3` intentionally share it. All session
components in Stop filenames use the resolved key, never the raw identifier.

The key has no length limit. A session id longer than the file system's name
limit (`NAME_MAX`, usually 255 bytes) makes the write fail. That failure is
handled like any other write failure; see
[Write failures](#write-failures).

## Project keys

The blarify staleness marker describes a project's code graph, not a session,
so it is keyed by project. `<project key>` is the first 16 lowercase hex
characters of the SHA-256 of the canonicalised working directory path. If the
path cannot be canonicalised, the path as given is hashed instead. The project
path is used only as hash input and never appears in a path component.

Find the marker for the current project:

```sh
key=$(printf '%s' "$(pwd -P)" | sha256sum | cut -c1-16)
cat "${HOME}/.amplihack/runtime/projects/$key/blarify_stale"
# {"path":"src/main.rs","reason":"code_file_modified","stale":true,"timestamp":1794046502,"tool":"Edit"}
```

If the working directory cannot be read, the marker is skipped and the hook
logs at `debug` level.

## Files written by each hook

In this table, `S` is `<state root>/runtime/sessions/<session key>` and `P` is
`<state root>/runtime/projects/<project key>`.

| Hook | File | Format | Previous location |
| --- | --- | --- | --- |
| `post-tool-use` | `S/metrics/post_tool_use_metrics.jsonl` | One JSON line per tool call | `<cwd>/.claude/runtime/metrics/` |
| `post-tool-use`, `user-prompt-submit` (`/dev`) | `S/workflow_state.json` | Workflow enforcement state | `<cwd>/.claude/runtime/workflow_state/<sid>.json` |
| `post-tool-use` | `P/blarify_stale` | JSON marker | `<cwd>/.amplihack/blarify_stale` |
| `session-start`, `pre-compact` | `S/logs/ORIGINAL_REQUEST.md`, `S/logs/original_request.json` | Markdown and JSON | `<cwd>/.claude/runtime/logs/<sid>/` |
| `pre-compact` | `S/logs/transcript_pre_compact.jsonl` | Copy of the transcript | `<cwd>/.claude/runtime/logs/<sid>/` |
| `pre-compact` | `S/logs/compaction_metadata.jsonl` | One JSON line per compaction | `<cwd>/.claude/runtime/logs/<sid>/` |
| `workflow-classification-reminder` | `S/classification_state.json` | Classification state; see [Coordination messages: state file](./coordination-messages.md#state-file) | `<cwd>/.claude/runtime/classification_state/<sid>.json` |
| `stop` (lock) | `S/locks/lock_invocations.txt` | JSON counter, `{"value": N}` | `<cwd>/.claude/runtime/locks/<sid>/` |
| `stop` (power steering) | `S/power-steering/session_count` | JSON counter, `{"value": N}` | `<cwd>/.claude/runtime/power-steering/<sid>/` |
| `stop` (power steering) | `S/power-steering/.<session key>_completed`, `S/power-steering/summary.md` | Empty completion marker and Markdown summary | `<cwd>/.claude/runtime/power-steering/` and its `<sid>/` subdirectory |
| `stop` (reflection) | `S/logs/FEEDBACK_SUMMARY.md` | Markdown feedback | `<cwd>/.claude/runtime/logs/<sid>/` |
| `stop` (reflection) | `S/reflection/reflection_<session key>_<epoch seconds>.md`, `S/reflection/current_findings.md` | Markdown history and current findings | `<cwd>/.claude/runtime/reflection/` |
| `stop` (reflection) | `S/reflection/.reflection_presented_<session key>` | Empty presentation semaphore | `<cwd>/.claude/runtime/reflection/` |

The original-request directory always comes from the session id the hook
received, never from a `session_id` field inside the captured record.

The `transcript_path` field of the PreCompact output reports the new location:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreCompact",
    "status": "success",
    "message": "Conversation exported successfully - transcript preserved at /home/alice/.amplihack/runtime/sessions/demo-1/logs/transcript_pre_compact.jsonl",
    "transcript_path": "/home/alice/.amplihack/runtime/sessions/demo-1/logs/transcript_pre_compact.jsonl",
    "metadata": {
      "compaction_trigger": "auto",
      "event": "pre_compact",
      "original_request_preserved": true,
      "session_id": "demo-1",
      "timestamp": 1794046502,
      "transcript_path": "/home/alice/.amplihack/runtime/sessions/demo-1/logs/transcript_pre_compact.jsonl"
    }
  }
}
```

Existing files at the old locations are not moved, copied or deleted. Remove
them by hand if you no longer need them:

```sh
rm -f .claude/runtime/metrics/post_tool_use_metrics.jsonl .amplihack/blarify_stale
rm -rf .claude/runtime/classification_state
```

## Stop state and controls

The normal `amplihack-hooks stop` protocol processes lock mode, power steering,
then reflection. It resolves `S` once using
`amplihack_types::session_state_dir` and keeps project inputs separate from
generated state. Features remain opt-in.

### Generated state

All Stop counters, completion markers, summaries, feedback, reflection history,
current findings and presentation semaphores are beneath `S`. Atomic companion
locks and temporary files stay beside their counters in `S/locks/` or
`S/power-steering/` throughout their lifetime, including before the counter
file exists. Successful atomic operations leave no temporary files. Companion
lock files can remain after the operating-system lock is released:

| Counter | Companion lock |
| --- | --- |
| `S/locks/lock_invocations.txt` | `S/locks/lock_invocations.json.lock` |
| `S/power-steering/session_count` | `S/power-steering/session_count.json.lock` |

Directories are created only when needed, using the
[private-directory permissions](#directory-permissions).

For example, with `HOME=/home/alice`, no `AMPLIHACK_HOME` override and payload
session id `lane 3`, the lock counter is
`/home/alice/.amplihack/runtime/sessions/lane_3/locks/lock_invocations.txt`.
The power-steering completion marker is
`/home/alice/.amplihack/runtime/sessions/lane_3/power-steering/.lane_3_completed`.
Reflection feedback is
`/home/alice/.amplihack/runtime/sessions/lane_3/logs/FEEDBACK_SUMMARY.md`.
No additional session subdirectory is added inside these feature directories.

### Project configuration

Paths in this table are relative to the session's project directory. They
remain project-owned inputs and are not moved into `S`.

| Feature | Control | Behaviour |
| --- | --- | --- |
| Lock | `.claude/runtime/locks/.lock_active` | Enables lock mode |
| Lock | `.claude/runtime/locks/.continuation_prompt` | Supplies the continuation prompt; missing, blank or overlong content uses the default |
| Lock | `AMPLIHACK_MAX_LOCK_ITERATIONS` | Safety-valve threshold; default `50` |
| Power steering | `.claude/tools/amplihack/.power_steering_config` | Enables processing when present and enabled; `{"enabled": true}` enables it and `{"enabled": false}` disables it |
| Power steering | `.claude/runtime/power-steering/.disabled` or presence of `AMPLIHACK_SKIP_POWER_STEERING` | Disables enforcement |
| Reflection | `AMPLIHACK_ENABLE_REFLECTION` | Enables reflection for `1`, `true` or `yes`, case-insensitively |
| Reflection | `.claude/tools/amplihack/.reflection_config` | `{"enabled": true}` explicitly enables reflection when the environment has not enabled it |
| Reflection | Non-empty `AMPLIHACK_SKIP_REFLECTION` or `.claude/runtime/reflection/.reflection_lock` | Suppresses reflection even when enabled |

### Continuation, completion and presentation

An active lock successfully increments its session counter before evaluating
the safety valve. Below the threshold it blocks with the continuation prompt.
When the counter reaches or exceeds the safety-valve threshold, it
removes the project controls
`.lock_active`, `.lock_goal`, `.lock_message` and `.continuation_prompt` from
`.claude/runtime/locks/`, then approves immediately. Neither lock result runs
power steering or reflection on that invocation.

Power steering increments its counter before the first-invocation exemption,
disable checks and completion check. Its first invocation passes through to
reflection. Later invocations can block for unfinished work; a block prevents
reflection. Successful completion writes the session's completion marker and
summary. Subsequent invocations recognise completion and still increment the
counter.

Reflection runs only when enabled and earlier features permit it. Findings
are saved and presented in a blocking response, with a presentation semaphore
under `S/reflection/`. The next invocation that reaches reflection consumes
that semaphore and passes through without invoking the subprocess again.

Reflection keeps the project as its subprocess working directory. Transcript,
preferences, framework templates and repository context retain their existing
input locations. Redirect history is still read from
`.claude/runtime/logs/<normalised session key>/redirects.jsonl` in the project;
relocating generated feedback does not move or create this input.

### Legacy state

Legacy project-local generated Stop state is not read, imported, moved,
modified or deleted. An old lock counter cannot trigger the new session's
safety valve. Old power-steering counters and completion markers cannot skip
the first-invocation exemption or mark the new session complete. Old reflection
findings and presentation semaphores cannot supply or suppress its findings.
Existing state beneath `S` is reused independently of those legacy files.
Project controls and redirect history remain inputs as described above.

Stop adds no Git ignore rules and makes no changes to `.git/info/exclude`.
Generated state needs no checkout exclusions.

## Stop counter synchronisation

Both Stop counters explicitly select blocking acquisition through
`AtomicCounter`, `AtomicJsonFile` and `FileLock`. On Unix, acquisition uses
`fcntl(F_SETLKW)` for the existing whole-file lock range. An interrupted call
(`EINTR`) is retried immediately, with no timer, deadline, polling or delayed
retry. A competing invocation waits until the holder releases the lock; a
live holder can delay it indefinitely. Contention alone does not cause a
timeout and fail-open exit. Other storage errors follow Stop's
[fail-open policy](#write-failures).
See the [lock implementation](../../crates/amplihack-state/src/file_lock.rs),
[atomic storage](../../crates/amplihack-state/src/atomic_json.rs) and Stop's
[lock](../../crates/amplihack-hooks/src/stop/lock.rs) and
[power-steering](../../crates/amplihack-hooks/src/stop/power_steering/mod.rs)
counter sites.

An exclusive companion lock covers the whole read–modify–replace operation
on Unix. The counter remains JSON
(`{"value": N}`), including the file named `lock_invocations.txt`; replacement
uses a temporary file in the same directory and an atomic rename. The lock is
released when its guard is dropped, including on an error.

The companion file's continued existence does not mean the lock is held.

Blocking acquisition is opt-in for these Stop operations. Other callers
retain their existing timeout defaults. Non-Unix platforms retain
best-effort locking without a cross-process exclusion guarantee.

### Platform coverage and validation

The blocking path runs on both macOS and Linux. macOS evidence cannot establish
Linux-only behaviour involving `/proc`, pidfd or FIFO pools. In particular,
these tests in
[`limit_resume_binary.rs`](../../bins/amplihack-hooks/tests/limit_resume_binary.rs)
are gated with `#[cfg(target_os = "linux")]` and are omitted on macOS:

- `resumes_when_a_fresh_cycle_has_capacity`
- `shell_wrapped_hook_delivers_the_warning_from_the_swap_record_alone`
- `second_run_on_the_same_event_is_a_silent_duplicate`
- `toggle_file_turns_it_on_too`

The [CI Test job](../../.github/workflows/ci.yml) runs the workspace tests on
`ubuntu-latest`, where these cases are included. Corresponding Linux CI
validation remains pending; no passing run is available for this branch.
These limit-resume cases do not validate Stop counter locking.

Live lock and temporary-file placement requires
observation while the files exist; a clean checkout after execution alone
cannot establish that property.

## Directory permissions

On Unix, every directory the hooks create under the state root, including
`<state root>` and `runtime/` when they do not already exist, is created with
mode `0700`. Pre-compact transcripts and original prompts are therefore not
readable by other local accounts, even when the home directory is `0755`.
Directories that already exist keep their modes. On other platforms the
directories are created with the default mode.

This includes Stop's `locks/`, `power-steering/`, `logs/` and `reflection/`
directories and their missing ancestors. Existing file-permission behaviour
is unchanged.

## When no state root resolves

If no rule in [Resolving the state root](#resolving-the-state-root) accepts a
value, the hook logs it, skips every state write and still succeeds. This
includes an `AMPLIHACK_HOME` that is a Git checkout when `HOME` is unusable.
Nothing falls back to the checkout or the working directory.

| Hook | Behaviour with no state root |
| --- | --- |
| `post-tool-use` | Logs one `warn!` line and skips the metric, the workflow state and the blarify marker. The hook output is unchanged. |
| `user-prompt-submit` (`/dev`) | Workflow tracking is not started and the failure is logged. The prompt is processed as usual. |
| `session-start` | The original request is not saved, but its context is still injected into the session. |
| `pre-compact` | Returns success with the message below, and `transcript_path` and `metadata` set to `null`. |
| `workflow-classification-reminder` | No turn counter or outcome is read or written (logged at `debug`). A new-topic prompt gets the held-back output (`{}` for an unmarked prompt, the coordination note for a marked one), the same as when the state file cannot be written. |
| `stop` | Immediately returns `{"decision":"approve"}`. Lock, power steering and reflection are skipped, with no generated writes or safety-valve control removals. |

The PreCompact output with no state root:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreCompact",
    "status": "success",
    "message": "Pre-compact state not saved: no amplihack state root (set HOME or AMPLIHACK_HOME)",
    "transcript_path": null,
    "metadata": null
  }
}
```

## Write failures

When a state root resolves but a write fails, the outcome depends on the file:

| Write | On failure |
| --- | --- |
| Tool metric, workflow state, blarify marker (`post-tool-use`) | Logged and skipped; the hook succeeds with unchanged output |
| Workflow state (`user-prompt-submit`, `/dev`) | Logged; the prompt is processed as usual |
| Original request (`session-start`, `pre-compact`) | Logged and skipped; the hook succeeds |
| Classification state (`workflow-classification-reminder`) | Logged at `debug`; a new-topic prompt gets the held-back output |
| Transcript export or compaction metadata (`pre-compact`) | The hook returns its error response, as it did before this change |
| Stop directory preparation, counter update, completion marker or summary | Propagated errors follow the hook's existing fail-open policy: exit is allowed and later feature processing is skipped |
| Reflection feedback, history, current findings or presentation semaphore | Later write failures are logged; available findings are still presented. If the semaphore cannot be saved, a later Stop may present findings again |

Stop never redirects failed writes to the project or another storage root.
Missing storage or a propagated storage error therefore prevents enforcement
for that invocation. A write failure can leave partial state beneath `S`,
such as a completion marker saved before a summary failure.

A resolved but unwritable root is a storage failure, rather than a reason to
try another root. If lock directory preparation or the counter update fails,
Stop approves without removing any of the four project lock controls. A
power-steering completion marker written before a summary failure remains in
place and is recognised on subsequent invocations. Reflection output writes
are independent: failure to save one does not prevent attempts to save the
others or present available findings. Failure to remove a presentation
semaphore is logged and still passes through on that invocation.

Only the no-root case makes PreCompact report success with `transcript_path`
and `metadata` set to `null`. A failed export or metadata write under a
resolved root is an error.

Log lines name paths and `io::Error` kinds only. They never include prompt,
transcript or requirement text.

## What still touches the working directory

The hooks still read these project files from the working directory and make
the listed deliberate removals:

| Hook | Reads or removes |
| --- | --- |
| `session-start` | Project context files (`PROJECT.md`, discoveries, preferences, workflow); removes a stale `.workflow_active` file |
| `post-tool-use` | Launcher detection |
| `workflow-classification-reminder` | The `.workflow_active` semaphore written by the orchestrator |
| `stop` | Lock controls, power-steering configuration and disable sentinel, reflection configuration and suppression lock, redirect history, preferences and project context; removes the four lock controls only when the safety valve triggers |

These writes still go under the project and are tracked by follow-up issues:

- `record_background_index_pid` and the blarify index artefacts written by
  `amplihack-memory` during session start.

## Rust API

The resolver lives in `amplihack-types` and is re-exported from the crate root
next to `sanitize_session_id`. It uses the standard library only.

```rust
pub const UNKNOWN_SESSION_KEY: &str = "unknown-session";

/// True when `<dir>/.git` exists as a directory, file or symlink (dangling
/// included). One `symlink_metadata` call; never runs git, never climbs to
/// parents. Only `NotFound` means false; any other error means true.
pub fn is_git_checkout(dir: &Path) -> bool;

/// Pure: chooses the state root from the candidate values.
/// `amplihack_home` is accepted only if `Path::is_absolute()` holds, it has
/// no `..` component and `amplihack_home_is_checkout` is false. `home` is
/// accepted under the same path rule. No trimming, no environment access,
/// no I/O, never the cwd.
pub fn resolve_state_root(
    amplihack_home: Option<&OsStr>,
    home: Option<&OsStr>,
    amplihack_home_is_checkout: bool,
) -> Option<PathBuf>;

/// Reads `AMPLIHACK_HOME` and `HOME` with `std::env::var_os`, probes an
/// accepted `AMPLIHACK_HOME` with `is_git_checkout` and calls
/// `resolve_state_root`.
pub fn amplihack_state_root() -> Option<PathBuf>;

/// Pure: `<state_root>/runtime/sessions/<key>`. The key is
/// `UNKNOWN_SESSION_KEY` when the id is absent, empty or whitespace-only,
/// and `sanitize_session_id(id)` otherwise. Never panics.
pub fn session_state_dir_under(state_root: &Path, session_id: Option<&str>) -> PathBuf;

/// `amplihack_state_root()` joined with `session_state_dir_under`.
pub fn session_state_dir(session_id: Option<&str>) -> Option<PathBuf>;
```

Examples:

```rust
use std::ffi::OsStr;
use std::path::Path;
use amplihack_types::{resolve_state_root, session_state_dir_under, UNKNOWN_SESSION_KEY};

let home = Some(OsStr::new("/home/alice"));

let root = resolve_state_root(Some(OsStr::new("relative")), home, false);
assert_eq!(root.as_deref(), Some(Path::new("/home/alice/.amplihack")));

let root = resolve_state_root(Some(OsStr::new("/opt/amp/../etc")), None, false);
assert_eq!(root, None);

// An AMPLIHACK_HOME that is a Git checkout is skipped in favour of HOME.
let root = resolve_state_root(Some(OsStr::new("/src/amplihack-rs")), home, true);
assert_eq!(root.as_deref(), Some(Path::new("/home/alice/.amplihack")));

let dir = session_state_dir_under(Path::new("/home/alice/.amplihack"), Some("../../escape"));
assert_eq!(dir, Path::new("/home/alice/.amplihack/runtime/sessions/______escape"));

let dir = session_state_dir_under(Path::new("/home/alice/.amplihack"), Some("  "));
assert!(dir.ends_with(UNKNOWN_SESSION_KEY));
```

`ProjectDirs::metrics` (`<project>/.claude/runtime/metrics/`) remains in the
type for compatibility, but the hooks no longer write there.

Stop uses the existing `session_state_dir` API for generated storage and
`ProjectDirs` for project inputs. Exported feature signatures remain compatible;
the separation of resolved storage paths is internal to the hooks crate.

## Security notes

The change closes one threat: hook output (tool metrics, full transcripts and
the user's original prompts) being written into whatever directory the session
runs in. In a target repository those files appear as untracked changes, fail
commit checks and can be committed by accident. In the installed bundle they
mix user data into framework files.

Residual cases that remain by design:

- An `AMPLIHACK_HOME` that is the root of a Git checkout is skipped (#2361).
  If `HOME`, or an `AMPLIHACK_HOME` below the root of a work tree, is inside a
  git work tree, state still lands in that work tree, because the probe does
  not look at parent directories.
- The state root is not checked for ownership or existence before use.
- Existing symbolic links in the state path are followed by filesystem
  operations. The resolver does not provide protection against symlink
  redirection; the configured state path must be trusted.
- PreCompact copies whatever file the payload's `transcript_path` names.
- The working-directory writes listed under
  [What still touches the working directory](#what-still-touches-the-working-directory).

## Related

- [Hook Specifications Reference](./hook-specifications.md): the eight hooks
  and their events
- [Coordination messages](./coordination-messages.md#state-file): the
  classification state file format
- [`AMPLIHACK_HOME`](./environment-variables.md#amplihack_home): launcher
  resolution of the same variable
- [Agent Prompt Channels Reference](./agent-prompt-channels.md#the-contract-file):
  the launch contract file, which follows the same checkout rule
- [Artifact Guard](../artifact-guard.md): how runtime output in a repository is
  treated at publish time
