---
title: Hook State Root Reference
description: Where amplihack-hooks writes per-session and per-project state, how the location is resolved, and what happens when it cannot be.
last_updated: 2026-11-07
related_issues: ["#1963"]
---

# Hook State Root Reference

The `amplihack-hooks` binary keeps its own state (tool metrics, workflow
enforcement state, captured original requests, pre-compact transcripts,
compaction metadata, classification state and the blarify staleness marker)
under one state root in the user's home directory. It never writes these files
into the session's working directory, so a target repository's git status and
the installed framework bundle stay unchanged while hooks run.

## Contents

- [Quick check](#quick-check)
- [Resolving the state root](#resolving-the-state-root)
- [Directory layout](#directory-layout)
- [Session keys](#session-keys)
- [Project keys](#project-keys)
- [Files written by each hook](#files-written-by-each-hook)
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
| 1 | `AMPLIHACK_HOME` | The value is an absolute path with no `..` component | `$AMPLIHACK_HOME` |
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
case "${AMPLIHACK_HOME:-}" in
  /*) echo "$AMPLIHACK_HOME" ;;
  *)  echo "$HOME/.amplihack" ;;
esac
# /home/alice/.amplihack
```

This shell check does not reject `..` components; the hooks do.

### `AMPLIHACK_HOME` pointing at a source checkout

The launcher can set `AMPLIHACK_HOME` to an amplihack-rs source checkout (a
directory containing `amplifier-bundle/`). Hook state then lands in
`<checkout>/runtime/`, which the repository's `.gitignore` ignores with a
root-anchored `/runtime/` entry. Other repositories cannot be a bundle root,
so this case does not apply to them.

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
    │       └── logs/
    │           ├── ORIGINAL_REQUEST.md
    │           ├── original_request.json
    │           ├── transcript_pre_compact.jsonl
    │           └── compaction_metadata.jsonl
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
| `unknown-session` | Tool metrics, original request, classification state |
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

## Directory permissions

On Unix, every directory the hooks create under the state root, including
`<state root>` and `runtime/` when they do not already exist, is created with
mode `0700`. Pre-compact transcripts and original prompts are therefore not
readable by other local accounts, even when the home directory is `0755`.
Directories that already exist keep their modes. On other platforms the
directories are created with the default mode.

## When no state root resolves

If neither `AMPLIHACK_HOME` nor `HOME` gives an absolute path, the hook logs
it, skips every state write and still succeeds. Nothing falls back to the
working directory.

| Hook | Behaviour with no state root |
| --- | --- |
| `post-tool-use` | Logs one `warn!` line and skips the metric, the workflow state and the blarify marker. The hook output is unchanged. |
| `user-prompt-submit` (`/dev`) | Workflow tracking is not started and the failure is logged. The prompt is processed as usual. |
| `session-start` | The original request is not saved, but its context is still injected into the session. |
| `pre-compact` | Returns success with the message below, and `transcript_path` and `metadata` set to `null`. |
| `workflow-classification-reminder` | No turn counter or outcome is read or written (logged at `debug`). A new-topic prompt gets the held-back output (`{}` for an unmarked prompt, the coordination note for a marked one), the same as when the state file cannot be written. |

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

Only the no-root case makes PreCompact report success with `transcript_path`
and `metadata` set to `null`. A failed export or metadata write under a
resolved root is an error.

Log lines name paths and `io::Error` kinds only. They never include prompt,
transcript or requirement text.

## What still touches the working directory

The hooks still read these project files from the working directory, and do
not create anything there:

| Hook | Reads or removes |
| --- | --- |
| `session-start` | Project context files (`PROJECT.md`, discoveries, preferences, workflow); removes a stale `.workflow_active` file |
| `post-tool-use` | Launcher detection |
| `workflow-classification-reminder` | The `.workflow_active` semaphore written by the orchestrator |

These writes still go under the project and are tracked by follow-up issues:

- The Stop hook's lock counter, power-steering state and reflection output.
  Each is opt-in: the first two need a sentinel or config file the project
  already owns, and reflection needs an explicit environment variable or
  config setting.
- `record_background_index_pid` and the blarify index artefacts written by
  `amplihack-memory` during session start.

## Rust API

The resolver lives in `amplihack-types` and is re-exported from the crate root
next to `sanitize_session_id`. It uses the standard library only.

```rust
pub const UNKNOWN_SESSION_KEY: &str = "unknown-session";

/// Pure: chooses the state root from the two candidate values.
/// A candidate is accepted only if `Path::is_absolute()` holds and it has no
/// `..` component. No trimming, no environment access, no I/O, never the cwd.
pub fn resolve_state_root(
    amplihack_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf>;

/// Reads `AMPLIHACK_HOME` and `HOME` with `std::env::var_os` and calls
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

let root = resolve_state_root(Some(OsStr::new("relative")), Some(OsStr::new("/home/alice")));
assert_eq!(root.as_deref(), Some(Path::new("/home/alice/.amplihack")));

let root = resolve_state_root(Some(OsStr::new("/opt/amp/../etc")), None);
assert_eq!(root, None);

let dir = session_state_dir_under(Path::new("/home/alice/.amplihack"), Some("../../escape"));
assert_eq!(dir, Path::new("/home/alice/.amplihack/runtime/sessions/______escape"));

let dir = session_state_dir_under(Path::new("/home/alice/.amplihack"), Some("  "));
assert!(dir.ends_with(UNKNOWN_SESSION_KEY));
```

`ProjectDirs::metrics` (`<project>/.claude/runtime/metrics/`) remains in the
type for compatibility, but the hooks no longer write there.

## Security notes

The change closes one threat: hook output (tool metrics, full transcripts and
the user's original prompts) being written into whatever directory the session
runs in. In a target repository those files appear as untracked changes, fail
commit checks and can be committed by accident. In the installed bundle they
mix user data into framework files.

Residual cases that remain by design:

- If `HOME` or `AMPLIHACK_HOME` is itself inside a git work tree, state lands
  in that work tree.
- The state root is not checked for ownership or existence before use.
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
- [Artifact Guard](../artifact-guard.md): how runtime output in a repository is
  treated at publish time
