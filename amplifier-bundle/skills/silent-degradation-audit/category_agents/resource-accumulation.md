# Category G: Resource Accumulation Agent

## Role

Specialized agent for detecting silent resource accumulation in long-running
processes (daemons, servers, workers). Asks "What grows without bound over
the lifetime of this process?"

## Core Question

**"What grows without bound over the lifetime of this process?"**

Where "resource accumulation" includes:

- In-memory collections (HashMap, Vec, BTreeMap) that add but never remove
- Caches without eviction policies or size caps
- File handles, temp files, or disk artifacts never cleaned
- Build artifacts, logs, or snapshots that accumulate
- Subprocess/thread handles not joined or reaped
- Database connections or sessions not returned to pool

## Origin

These patterns were identified through production incidents on a Rust daemon
(Simard) that grew from 1G to 32G RSS within 16-24h, requiring periodic
restarts. Five root causes were found — all variants of the same meta-bug:
**something grew each cycle and nothing shrank it**.

## Detection Focus

### 1. Unbounded Collections in Loop Bodies

Look for `HashMap`, `Vec`, `BTreeMap`, or custom containers that:
- Are fields on a struct that lives for the process lifetime
- Have `insert()`, `push()`, `extend()` calls inside recurring logic (event loops, request handlers, scheduled tasks)
- Lack corresponding `remove()`, `retain()`, `drain()`, `clear()`, or capacity-check eviction

**Severity**: HIGH — this is the #1 cause of daemon memory leaks.

```rust
// BAD: grows forever
struct State {
    failure_counts: HashMap<String, u32>,  // keys never pruned
}
fn on_cycle(&mut self) {
    self.failure_counts.entry(goal_id).and_modify(|c| *c += 1).or_insert(1);
    // ← never calls .retain() or .remove() for goals no longer active
}

// GOOD: prune after each cycle
fn on_cycle(&mut self) {
    // ... do work ...
    self.failure_counts.retain(|id, _| active_goal_ids.contains(id));
}
```

### 2. Large Allocations Held Across Iterations

Look for structs or fields that:
- Hold large temporary data (LLM prompts, serialized payloads, response buffers)
- Are set during one phase of a loop iteration
- Are NOT cleared/dropped before the next iteration

**Severity**: HIGH — single allocation may be megabytes (LLM context).

```rust
// BAD: prepared_context retained across cycles
struct DaemonState {
    prepared_context: Option<LargeContext>,  // set in orient(), read in act()
}
fn run_cycle(&mut self) {
    self.prepared_context = Some(build_context()); // 2MB+
    self.act();
    // ← prepared_context still alive for next cycle's orient()
}

// GOOD: explicit drop
fn run_cycle(&mut self) {
    self.prepared_context = Some(build_context());
    self.act();
    self.prepared_context = None;  // free immediately
}
```

### 3. One-Time Cleanup That Should Be Periodic

Look for cleanup/maintenance operations that:
- Run only at process startup or shutdown
- Should run periodically (temp file sweep, orphan detection, cache rotation)
- Have no timer, schedule, or cycle-count trigger

**Severity**: MEDIUM — silent disk/resource exhaustion over days.

```rust
// BAD: only at startup
fn main() {
    sweep_orphaned_worktrees();  // runs once
    loop { run_ooda_cycle(); }   // creates new worktrees each cycle
}

// GOOD: periodic sweep
fn main() {
    let mut last_sweep = Instant::now();
    loop {
        run_ooda_cycle();
        if last_sweep.elapsed() > Duration::from_secs(1800) {
            sweep_orphaned_worktrees();
            last_sweep = Instant::now();
        }
    }
}
```

### 4. Missing Runtime Health Monitoring

Look for long-running processes that:
- Have no RSS/memory monitoring
- Have no disk usage monitoring
- Log errors but don't track error *rates* or consecutive failure counts
- Have no circuit breaker or self-healing mechanism

**Severity**: MEDIUM — operator blindness until OOM/disk-full.

### 5. Build/Disk Artifact Accumulation

Look for processes that create disk artifacts (compilation outputs, logs,
snapshots, temp files) where:
- No cap or rotation policy exists
- Cleanup only targets a subset of artifact locations (e.g., caps `/tmp` but not `~/.cache`)
- The write path and cleanup path reference different directories (drift)

**Severity**: HIGH — disk exhaustion cascades to DB corruption.

```rust
// BAD: cleanup watches /tmp but builds go to ~/.cargo-targets
const CLEANUP_DIR: &str = "/tmp/simard-*-target";
const BUILD_DIR: &str = "~/.cargo-targets";  // ← not cleaned!

// GOOD: cleanup covers all artifact locations
fn cleanup() {
    cap_dir("/tmp/simard-*-target", MAX_BYTES);
    cap_dir("~/.cargo-targets", MAX_BYTES);
    cap_dir("worktrees/main/target", MAX_BYTES);
}
```

### 6. Subprocess Lifecycle Leaks

Look for subprocesses (engineers, workers, bridges) where:
- Child process handles are stored but never waited/reaped
- Zombie processes accumulate (`<defunct>`)
- No timeout or idle-detection kills hung subprocesses
- Shutdown signal doesn't propagate to children

**Severity**: MEDIUM — PID exhaustion, memory from zombie accumulation.

## Audit Checklist

For each long-running process found in the codebase:

| # | Check | Pattern |
|---|-------|---------|
| 1 | List all `HashMap`/`Vec`/`BTreeMap` fields on long-lived structs | Unbounded collection |
| 2 | For each: is there a `retain()`/`remove()`/`drain()` call? | Missing eviction |
| 3 | List all `Option<LargeStruct>` fields set in loop bodies | Held allocation |
| 4 | For each: is it set to `None` before next iteration? | Missing cleanup |
| 5 | List all cleanup functions — are they called periodically? | One-time cleanup |
| 6 | Does the process monitor its own RSS? | Missing health check |
| 7 | List all disk write paths — does cleanup cover ALL of them? | Write/cleanup drift |
| 8 | List all child process spawns — are they all waited/reaped? | Subprocess leak |

## Scoring

- **Critical**: Unbounded collection in a loop that runs every N seconds
- **High**: Large held allocation, disk artifact accumulation with no cap
- **Medium**: Missing periodic cleanup, no health monitoring
- **Low**: Subprocess lifecycle gaps (usually caught by OS)
