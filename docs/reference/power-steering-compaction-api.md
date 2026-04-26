# Power-Steering API

> **Status: Shipped (prompt API only).** The shipped surface is the
> re-enable prompt in `crates/amplihack-utils/src/power_steering.rs`.
> The "compaction" half of this reference page is **planned** and not
> implemented; treat the planned-section signatures as a design sketch,
> not as bindings you can call.

## Crate location

```
amplihack_utils::power_steering
```

Workspace dependency:

```toml
amplihack-utils = { path = "crates/amplihack-utils" }
```

## Shipped: prompt API

### Constants

| Name              | Type   | Value | Notes                          |
|-------------------|--------|-------|--------------------------------|
| `TIMEOUT_SECONDS` | `u64`  | `30`  | Default Y/n response timeout.  |

### Types

```rust
pub enum ReEnableResult {
    Enabled,
    Disabled,
}

pub enum PowerSteeringError {
    RuntimeDir(worktree::WorktreeError),
    Io(std::io::Error),
}
```

### Functions

```rust
/// Prompt the user to re-enable power-steering if it is currently disabled.
///
/// Fail-open: any unexpected error returns `ReEnableResult::Enabled`.
pub fn prompt_re_enable_if_disabled(
    project_root: Option<&std::path::Path>,
) -> ReEnableResult;
```

Behavior summary:

- Resolves `<runtime-dir>/power-steering/.disabled` via
  `worktree::get_shared_runtime_dir`.
- If absent → returns `Enabled` immediately.
- If present and terminal is non-interactive → removes the marker and
  returns `Enabled`.
- If present and terminal is interactive → prints prompt, waits up to
  `TIMEOUT_SECONDS`, defaults to YES.

## Planned: compaction API

> Everything in this section is **proposed only**. None of these symbols
> exist in the workspace; do not depend on them.

### Proposed CLI subcommand

```text
# Planned
amplihack compact          # trigger a compaction pass
amplihack compact status   # show last-pass summary
amplihack compact history  # show recent passes
```

### Proposed event topics

| Event              | Direction             | Purpose                                      |
|--------------------|-----------------------|----------------------------------------------|
| `compact.started`  | runtime → subscribers | A compaction pass has begun.                 |
| `compact.skipped`  | runtime → subscribers | Pass declined (e.g. below threshold).        |
| `compact.complete` | runtime → subscribers | Pass finished; payload includes byte deltas. |

The exact payload schema, event-bus channel, and config knobs are
intentionally left unspecified here until the design lands. Do not invent
defaults.

### Proposed environment variables

None are pinned. Any `AMPLIHACK_COMPACT_*` environment knobs you may have
seen in earlier drafts have been removed because they were speculative.

## See also

- [Power-Steering Re-enable Prompt](../concepts/power-steering-compaction.md)
- [Git Worktree Support](../concepts/worktree-support.md)
- `crates/amplihack-utils/src/power_steering.rs`
- `crates/amplihack-utils/src/worktree.rs`
