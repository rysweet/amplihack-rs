# `active_agent_binary()` — Parent Agent Runtime Detection

## Overview

`active_agent_binary()` returns the identifier of the agent binary that the current `amplihack-cli` process should treat as its "active" runtime. As of #441, it auto-detects the parent agent runtime from well-known environment variables instead of unconditionally returning `"claude"`.

This means that when `amplihack-cli` is invoked from inside a Copilot CLI, Claude Code, or Codex session, nested workflow steps stay on the caller's runtime without requiring the user to set `AMPLIHACK_AGENT_BINARY` manually.

**Module:** `amplihack_cli::env_builder::helpers`
**Signature:** `pub fn active_agent_binary() -> String`

## Detection Priority

The function evaluates the following sources in order and returns on the first match. A value is considered "set" only if it is present **and** non-empty after trimming whitespace.

| # | Source                                                       | Returned identifier                  |
| - | ------------------------------------------------------------ | ------------------------------------ |
| 1 | `AMPLIHACK_AGENT_BINARY` (explicit override)                 | the override value, verbatim (after trim check) |
| 2 | `COPILOT_AGENT_SESSION_ID`                                   | `"copilot"`                          |
| 3 | `CLAUDECODE` **or** any env var starting with `CLAUDE_CODE_` | `"claude"`                           |
| 4 | `CODEX_HOME` **or** any env var starting with `CODEX_`       | `"codex"`                            |
| 5 | (none of the above)                                          | `"claude"` + a `tracing::warn!` log  |

Empty or whitespace-only values are treated as "not set" at every level, including the override. For example, `AMPLIHACK_AGENT_BINARY=""` and `AMPLIHACK_AGENT_BINARY="   "` are both treated as unset and detection proceeds to step 2.

### Prefix-match scope

The `CLAUDE_CODE_` and `CODEX_` prefix scans iterate `std::env::vars_os()` and match **any** variable whose name starts with the given prefix:

- `CLAUDE_CODE_*` matches `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SSE_PORT`, etc. It does **not** match the bare `CLAUDE_*` namespace — `CLAUDE_API_KEY` will not trigger Claude detection.
- `CODEX_*` matches every variable starting with `CODEX_`. `CODEX_HOME` is one such match; any other `CODEX_*` variable also triggers detection. The explicit `CODEX_HOME` check is a fast path, not an exclusion.

## Usage

```rust
use amplihack_cli::env_builder::helpers::active_agent_binary;

let binary = active_agent_binary();
tracing::info!(%binary, "dispatching nested workflow on detected runtime");
```

The returned string is an **identifier**, not a path. Callers that need to launch the binary should resolve it through their existing platform allowlist (e.g. the launcher's binary resolver), never pass it directly to `Command::new`. The two existing in-tree consumers (`execute.rs` and `rust_trial.rs`) use the return value purely as an identifier passed to the platform binary resolver.

## Examples

### Inside Copilot CLI

```bash
# Copilot CLI sets COPILOT_AGENT_SESSION_ID for every nested process.
$ env | grep COPILOT_AGENT_SESSION_ID
COPILOT_AGENT_SESSION_ID=abc123

$ amplihack recipe run smart-orchestrator -c task_description="..."
# active_agent_binary() -> "copilot"
```

### Inside Claude Code

```bash
$ env | grep -E '^(CLAUDECODE|CLAUDE_CODE_)'
CLAUDECODE=1
CLAUDE_CODE_ENTRYPOINT=cli

$ amplihack recipe run default-workflow ...
# active_agent_binary() -> "claude"
```

Note that only the `CLAUDE_CODE_` prefix is scanned. A value such as `CLAUDE_API_KEY` would not trigger Claude detection on its own.

### Inside Codex

```bash
$ env | grep ^CODEX_
CODEX_HOME=/home/me/.codex

$ amplihack recipe run investigation-workflow ...
# active_agent_binary() -> "codex"
```

### Explicit override (highest priority)

```bash
$ AMPLIHACK_AGENT_BINARY=copilot amplihack ...
# active_agent_binary() -> "copilot"  (regardless of any other detection signals)
```

### Empty override falls through to detection

```bash
$ AMPLIHACK_AGENT_BINARY="   " COPILOT_AGENT_SESSION_ID=abc amplihack ...
# active_agent_binary() -> "copilot"  (whitespace-only override is treated as unset)
```

### No runtime detected (fallback)

```bash
$ env -i amplihack ...
# active_agent_binary() -> "claude"
# WARN: AMPLIHACK_AGENT_BINARY not set; defaulting to 'claude'. ...
```

## Configuration Reference

| Variable                    | Effect                                                       | Notes                                  |
| --------------------------- | ------------------------------------------------------------ | -------------------------------------- |
| `AMPLIHACK_AGENT_BINARY`    | Forces the returned identifier to this exact value           | Highest priority. Untrusted; callers must allowlist before exec. Empty/whitespace-only values fall through to detection. |
| `COPILOT_AGENT_SESSION_ID`  | Presence selects `"copilot"`                                 | Set automatically by Copilot CLI       |
| `CLAUDECODE`                | Presence selects `"claude"`                                  | Set automatically by Claude Code       |
| `CLAUDE_CODE_*` (any)       | Presence of any matching var selects `"claude"`              | Strict prefix `CLAUDE_CODE_`. Bare `CLAUDE_*` does **not** match. |
| `CODEX_HOME`                | Presence selects `"codex"`                                   | Set automatically by Codex             |
| `CODEX_*` (any)             | Presence of any matching var selects `"codex"`               | Prefix `CODEX_`. `CODEX_HOME` is one such match; any other `CODEX_*` var also triggers detection. |

All checks ignore values that are empty or whitespace-only.

## Security Notes

- `AMPLIHACK_AGENT_BINARY` is **untrusted user input**. `active_agent_binary()` returns it verbatim; do not pass the value directly to `Command::new` or shell. Resolve through your platform's binary allowlist.
- The function never logs env var **values**, only detection outcomes (and only on the fallback branch).
- Prefix scans iterate `std::env::vars_os()` once per call; cost is O(n) in env size — negligible for typical usage. Cache the result if invoked in a hot path.
- No new dependencies are introduced.

## Behavior Change Summary (#441)

| Scenario                                               | Before #441           | After #441                                                 |
| ------------------------------------------------------ | --------------------- | ---------------------------------------------------------- |
| Run from Copilot CLI, no override                      | `"claude"` + warn     | `"copilot"`                                                |
| Run from Claude Code, no override                      | `"claude"` + warn     | `"claude"` (no warn)                                       |
| Run from Codex, no override                            | `"claude"` + warn     | `"codex"`                                                  |
| `AMPLIHACK_AGENT_BINARY=foo` set                       | `"foo"`               | `"foo"` (unchanged)                                        |
| `AMPLIHACK_AGENT_BINARY=""` (or whitespace) set        | `"claude"` + warn     | Falls through to runtime detection (or fallback + warn)    |
| No detection vars and no override                      | `"claude"` + warn     | `"claude"` + warn (unchanged; warn message preserved verbatim) |

The fallback warn message is preserved verbatim from the pre-#441 implementation so that existing log scrapers and alerts continue to match.

## Testing

Unit tests live in `crates/amplihack-cli/src/env_builder/tests_builder.rs` and cover all branches:

- `override_wins_over_runtime_signals`
- `detects_copilot_via_session_id`
- `detects_claude_via_claudecode_or_prefix`
- `detects_codex_via_home_or_prefix`
- `fallback_when_nothing_set`

Tests are standard `#[test]` functions. They serialize on a module-local `AGENT_BINARY_ENV_LOCK: Mutex<()>` and use a RAII `EnvSnapshot` guard that scrubs all detection-relevant variables (`AMPLIHACK_AGENT_BINARY`, `COPILOT_AGENT_SESSION_ID`, `CLAUDECODE`, `CODEX_HOME`, plus any `CLAUDE_CODE_*` / `CODEX_*` discovered at runtime) and restores the originals on drop. The mutex makes parallel `cargo test` execution safe.

Run locally:

```bash
cargo clippy -p amplihack-cli --all-targets -- -D warnings
TMPDIR=/tmp cargo test -p amplihack-cli --lib
```
