# Power-Steering Re-enable Prompt

> **Status: Shipped (the prompt feature only).** "Power-steering" in
> amplihack-rs today refers to the **re-enable prompt** implemented in
> `crates/amplihack-utils/src/power_steering.rs` — a startup check that
> asks the user whether to re-enable a feature they previously disabled
> via a `.disabled` marker file. It is **not** a context-window
> compaction system. The compaction-related sections below are clearly
> marked as planned and have no implementation in the workspace today.

This page documents the shipped prompt feature and adjacent planned work
under the same umbrella. The page name retains "compaction" only to keep
existing cross-links working; the user-visible feature today is the
re-enable prompt.

## What ships today

When amplihack-rs starts up it calls
`power_steering::prompt_re_enable_if_disabled(project_root)`. That
function:

1. Resolves the shared runtime directory via
   [`worktree::get_shared_runtime_dir`](./worktree-support.md).
2. Checks for a `.disabled` marker file under
   `<runtime>/power-steering/.disabled`.
3. If present **and** the terminal is interactive, prints:
   `Would you like to re-enable it? [Y/n] (30s timeout, defaults to YES):`
4. Defaults to YES on timeout, on empty input, or on non-interactive
   terminals (CI, pipes, tmux without a TTY).
5. Removes the marker file when the user accepts.
6. **Fails open** — any unexpected error returns `Enabled`.

Constants of interest (from `power_steering.rs`):

| Constant          | Value | Meaning                                       |
|-------------------|-------|-----------------------------------------------|
| `TIMEOUT_SECONDS` | `30`  | How long to wait for a Y/n response.          |

## Today vs. Planned

| Capability                                | Today        | Planned                                       |
|-------------------------------------------|--------------|-----------------------------------------------|
| Re-enable prompt with timeout / fail-open | ✅ shipped   | unchanged                                      |
| `.disabled` marker shared across worktrees | ✅ shipped   | unchanged                                      |
| Context-window **compaction** ("autoshrink") | ❌ not implemented | proposed; design TBD                     |
| `amplihack compact` CLI                    | ❌ not in clap enum | proposed                                  |
| `compact.*` event-bus events               | ❌ not emitted today | proposed                                 |
| `AMPLIHACK_COMPACT_*` environment knobs    | ❌ not present | proposed                                      |

The "compaction" portion of this page exists only to anchor the planned
design discussion. Until those items land, do not include them in
runbooks, do not assert any thresholds in tests, and do not configure
non-existent environment variables.

## Why fail-open

If anything in the prompt path errors (missing runtime dir, broken
terminal, I/O failure) the function deliberately returns `Enabled`.
Power-steering should default to **on** so that a flaky environment
cannot silently leave it off.

## See also

- [Git Worktree Support](./worktree-support.md) — runtime-dir resolution
  the prompt depends on.
- [Power-Steering Compaction API reference](../reference/power-steering-compaction-api.md)
  — the matching reference page (also marked accordingly).
- `crates/amplihack-utils/src/power_steering.rs`
