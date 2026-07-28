# Bug Fix #1093 — Deterministic liveness decision in blarify poll test

> **Issue:** [#1093](https://github.com/rysweet/amplihack-rs/issues/1093)

---

## Summary

The blarify session-start regression test
`slow_growing_file_is_not_truncated_before_markers_arrive` in
`crates/amplihack-hooks/src/session_start/blarify.rs` flaked under a saturated
`cargo nextest run`. It spawned a writer thread that appended five chunks with
`120ms` sleeps between them while the poll loop's `idle_bound` was only `200ms`.
In isolation the writer always re-grew the file well inside the `200ms` idle
window, so the liveness timer reset and polling continued. Under CI load,
scheduler jitter could delay a writer wakeup past `200ms`, the idle timer
expired between two real writes, and the poll gave up **before** the
`index-code` marker arrived — a false "truncation" that the test then reported
as a hard failure.

| Test | Old shape | Symptom under load |
| --- | --- | --- |
| `slow_growing_file_is_not_truncated_before_markers_arrive` | Real writer thread with `120ms` sleeps vs. a `200ms` `idle_bound` | Scheduler jitter delayed a real write past the idle window, so the poll gave up early and the assertion failed non-deterministically |

The fix extracts the poll loop's clock-free decision rule into a pure helper,
`liveness_step`, and rewrites the flaky test to drive that helper with
**scripted observations** instead of threads, sleeps, or the filesystem. The
race disappears because no wall clock is involved. The real-IO tests that
exercise the actual `poll_file_for_content` loop are kept unchanged as the
regression guard for real-world timing behavior — the four direct-loop tests
plus the `setup_blarify_indexing_*` integration tests that also drive the loop.

This is a **test-only** change. No production truncation-detection, session
hook, or blarify indexing behavior was modified. The PRD and the liveness
("growth resets the idle timer") semantics are preserved exactly.

## The liveness rule, made testable

`poll_file_for_content` decides, on every iteration, whether to return
`Found`, give up, or keep polling. That decision depends on three observations
and one bound:

1. whether all `markers` are present in the file content,
2. whether the file **grew** since the previous observation,
3. how long the file has been **idle** (no growth), and
4. the configured `idle_bound`.

Those observations were previously entangled with real `fs::metadata`,
`fs::read_to_string`, `Instant::now()`, and `thread::sleep` calls, so the only
way to test the rule was to reproduce real timing — which is exactly what
flaked. The fix pulls the rule out as a pure function.

### `liveness_step` (pure, `#[cfg(test)]`)

```rust
/// Terminal-or-continue decision for one poll iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LivenessStep {
    /// All markers observed — stop and report success.
    Found,
    /// File has been idle (no growth) for at least `idle_bound` without all
    /// markers — stop and report a give-up (the real loop returns
    /// `(false, content)`).
    GiveUp,
    /// Alive: markers not yet complete but growth is still resetting the idle
    /// timer, or the idle bound has not elapsed — keep polling.
    Alive,
}

/// Pure liveness decision. Contains no clock, no I/O, no sleep, so it is fully
/// deterministic and cannot race under load.
///
/// * `grew` — did the file's byte length change since the previous poll?
///   Growth resets the idle timer: whenever `grew` is `true` this function
///   returns `Alive` (unless the markers are already complete), regardless of
///   `idle_elapsed`. The give-up branch is gated by the explicit `!grew`
///   guard, so a growing file is never truncated even if `idle_elapsed` is
///   large.
/// * `all_markers_present` — are all required markers in the content read?
/// * `idle_elapsed` — time since the last observed growth. The real loop
///   passes `last_progress.elapsed()` here; on a growth observation this value
///   is neutralized by the `!grew` guard, not by being zero.
/// * `idle_bound` — how long the file may stay idle before we give up.
fn liveness_step(
    grew: bool,
    all_markers_present: bool,
    idle_elapsed: Duration,
    idle_bound: Duration,
) -> LivenessStep {
    if all_markers_present {
        LivenessStep::Found
    } else if !grew && idle_elapsed >= idle_bound {
        LivenessStep::GiveUp
    } else {
        LivenessStep::Alive
    }
}
```

**Decision order matches the original loop exactly:** markers are checked
first (a fully-written file returns `Found` even if it just went idle), then
the idle-bound give-up, otherwise keep polling. Growth (`grew == true`) always
keeps the poll `Alive`, which is the "liveness resets the idle timer"
guarantee that prevents a slow-but-alive subprocess from being truncated.

### `poll_file_for_content` delegates to the rule

The real-IO loop keeps its exact public signature and behavior; it now owns
only the I/O and clock, and delegates the *decision* to `liveness_step`:

```rust
fn poll_file_for_content(
    path: &Path,
    markers: &[&str],
    poll_interval: Duration,
    idle_bound: Duration,
) -> (bool, String) {
    let mut last_len: u64 = 0;
    let mut last_progress = std::time::Instant::now();
    loop {
        let current_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let grew = current_len != last_len;
        if grew {
            last_len = current_len;
            last_progress = std::time::Instant::now();
        }
        let content = fs::read_to_string(path).unwrap_or_default();
        let all_markers_present = markers.iter().all(|m| content.contains(m));

        match liveness_step(grew, all_markers_present, last_progress.elapsed(), idle_bound) {
            LivenessStep::Found => return (true, content),
            LivenessStep::GiveUp => return (false, content),
            LivenessStep::Alive => std::thread::sleep(poll_interval),
        }
    }
}
```

Because the signature and return contract are unchanged, the real-IO tests
below continue to exercise the *whole* loop — clock, filesystem, and
sleep included — and remain the authoritative regression guard against a
real-world timing break.

## Behavior after the fix

### 1. The slow-growth guarantee is proven without a clock

`slow_growing_file_is_not_truncated_before_markers_arrive` no longer spawns a
thread or sleeps. It scripts the exact sequence of observations the real loop
would make for a slow-but-alive subprocess and asserts the decision at each
step:

```rust
#[test]
fn slow_growing_file_is_not_truncated_before_markers_arrive() {
    let idle_bound = Duration::from_millis(200);

    // Four slow-but-live growth observations before the marker arrives. Each
    // one is observed as growth only *after* the idle window would otherwise
    // have expired (idle_elapsed == idle_bound), so we prove the `!grew` guard
    // -- not merely a reset timer -- keeps the poll Alive. This is exactly the
    // #1093 race: growth must override an elapsed idle bound.
    for _ in 0..4 {
        assert_eq!(
            liveness_step(true, false, idle_bound, idle_bound),
            LivenessStep::Alive,
            "growth must override an elapsed idle bound and keep the poll alive"
        );
    }

    // Final observation: the last write brought in `index-code`.
    assert_eq!(
        liveness_step(true, true, idle_bound, idle_bound),
        LivenessStep::Found,
        "markers present on a live file must report Found, never truncate"
    );
}
```

The intent — *"growth resets the idle timer, so a slow subprocess is never
truncated before its markers arrive"* — is now asserted directly and cannot be
perturbed by CPU contention. Because each growth observation carries an
`idle_elapsed` already at the bound, the test would fail if the `!grew` guard
were dropped, so it genuinely regression-guards #1093 rather than merely
passing on a zeroed timer.

### 2. The give-up rule is still proven deterministically

A companion pure test pins the give-up boundary that the real loop relies on:

```rust
#[test]
fn liveness_step_gives_up_only_when_idle_past_bound() {
    let bound = Duration::from_millis(200);

    // Idle but not yet past the bound -> keep polling.
    assert_eq!(liveness_step(false, false, Duration::from_millis(199), bound), LivenessStep::Alive);
    // Idle at/over the bound with no markers -> give up.
    assert_eq!(liveness_step(false, false, bound, bound), LivenessStep::GiveUp);
    // Markers present wins even if idle past the bound.
    assert_eq!(liveness_step(false, true, bound, bound), LivenessStep::Found);
}
```

## What did NOT change

- **Production code is untouched.** `poll_file_for_content` keeps its
  signature and observable behavior; only the decision was factored into a
  helper it calls. No blarify indexing, session-start hook, or
  truncation-detection behavior changed.
- **The real-IO tests remain as-is** and are the regression guard for
  real timing. Four tests drive `poll_file_for_content` directly:
  - `content_present_immediately_returns_found_fast`
  - `content_appended_after_delay_is_detected_via_liveness`
  - `file_idle_without_markers_gives_up_after_idle_bound`
  - `missing_file_then_created_is_handled`

  In addition, the `setup_blarify_indexing_*` integration tests (e.g.
  `setup_blarify_indexing_background_imports_current_json_when_db_missing`)
  exercise the loop end-to-end via `setup_blarify_indexing`. All of these
  must stay green — the enumerated set is the full guard, not just three
  tests.
- **No new dependency.** No `serial_test`, no clock-injection crate. The fix is
  a plain extract-function refactor within `#[cfg(test)]`.
- **No `print!`/`println!`.** Diagnostics, if any, use `tracing`.

## Trade-offs

Splitting the loop's decision into `liveness_step` adds one small indirection,
but entirely within `#[cfg(test)]` test code — both the helper and the
`poll_file_for_content` harness it serves are gated on `#[cfg(test)]`, so they
compile out of release builds and add zero indirection to production code. This
is acceptable:

- The extracted rule is a pure, exhaustively-tested function — strictly easier
  to reason about than the previous inline decision.
- The real-IO tests still cover the loop end-to-end, so the refactor cannot
  silently change real behavior without tripping a test.
- The previous thread+sleep test produced load-dependent false failures, which
  eroded trust in the suite more than a small refactor costs.

## Verification

```bash
# The (previously flaky) test plus the new pure liveness tests, in isolation.
cargo nextest run -p amplihack-hooks \
  slow_growing_file_is_not_truncated_before_markers_arrive \
  liveness_step_gives_up_only_when_idle_past_bound

# The real-IO regression guards (direct-loop tests).
cargo nextest run -p amplihack-hooks \
  content_present_immediately_returns_found_fast \
  content_appended_after_delay_is_detected_via_liveness \
  file_idle_without_markers_gives_up_after_idle_bound \
  missing_file_then_created_is_handled

# The integration tests that drive the loop through setup_blarify_indexing.
cargo nextest run -p amplihack-hooks setup_blarify_indexing

# Under a saturated full-workspace run (repeat to stress the old race).
for i in $(seq 1 100); do \
  cargo nextest run -p amplihack-hooks slow_growing_file_is_not_truncated_before_markers_arrive \
  || { echo "FLAKED on run $i"; break; }; done

# Formatting and lints for the touched crate.
cargo fmt -p amplihack-hooks
cargo clippy -p amplihack-hooks --tests
```

Because the rewritten test contains no clock or I/O, the 100× loop is expected
to pass every iteration regardless of machine load.
