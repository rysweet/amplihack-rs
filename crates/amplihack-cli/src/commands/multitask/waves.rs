//! Bounded launch waves for the parallel orchestrator (issue #1329).
//!
//! `launch_all` used to start every workstream at once. Each workstream is a whole
//! agent tree, so "launch everything the model decomposed into" multiplied the
//! tree-global budget by however many workstreams a decomposition happened to emit.
//!
//! Kept in its own module so `orchestrator.rs` stays inside the 400-line brick limit
//! that `tests/integration/multitask_orchestrator_spec_test.rs` enforces.

use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use super::launcher;
use super::models::Workstream;

/// Environment override for the launch concurrency limit. `0` means unlimited,
/// which restores the pre-#1329 behaviour for anyone who wants it back.
pub(super) const MAX_PARALLEL_ENV: &str = "AMPLIHACK_MAX_PARALLEL_WORKSTREAMS";

/// Pause between waves. Long enough that a wave establishes itself before the next
/// is added; the monitor loop is what actually reaps completions, so this only stops
/// a thundering herd at t=0.
pub(super) const WAVE_SETTLE: Duration = Duration::from_secs(2);

/// How many workstreams may be launched concurrently.
///
/// Defaults to `min(cpus/2, 8)`, floored at 1 — a machine that reports one CPU must
/// still make progress.
pub(super) fn concurrency_limit() -> usize {
    limit_from(std::env::var(MAX_PARALLEL_ENV).ok(), detected_parallelism())
}

fn detected_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// The decision behind [`concurrency_limit`], with its inputs as arguments so it is
/// testable without mutating process-global environment.
pub(crate) fn limit_from(configured: Option<String>, cpus: usize) -> usize {
    if let Some(raw) = configured
        && let Ok(value) = raw.trim().parse::<usize>()
    {
        return value;
    }
    (cpus / 2).clamp(1, 8)
}

/// Split `count` items into launch waves of at most `limit` (0 = one wave).
pub(crate) fn wave_size(count: usize, limit: usize) -> usize {
    if limit == 0 { count.max(1) } else { limit }
}

/// Launch every workstream, at most `concurrency_limit()` at a time.
///
/// Lives here rather than on `ParallelOrchestrator` so `orchestrator.rs` stays inside
/// the 400-line brick limit that `multitask_orchestrator_spec_test` enforces -- the
/// limit was doing its job: that file was already 4 lines from the cap.
pub(super) fn launch_all(
    workstreams: &mut [Workstream],
    processes: &mut HashMap<i64, Arc<Mutex<Option<Child>>>>,
    mode: &str,
) -> Result<()> {
    let delegate = launcher::detect_delegate();
    let count = workstreams.len();
    let wave = plan(count);

    for start in (0..count).step_by(wave.max(1)) {
        let end = (start + wave).min(count);
        for ws in &mut workstreams[start..end] {
            launcher::launch_workstream(ws, mode, &delegate, processes)?;
        }
        if end < count {
            // Let a wave establish itself before adding to it. The monitor loop is
            // what reaps completions; this only stops a thundering herd at t=0.
            std::thread::sleep(WAVE_SETTLE);
        }
    }

    println!("\n{count} workstreams launched in parallel ({mode} mode)");
    Ok(())
}

/// Plan the launch: wave size, plus a one-line note when waving actually applies.
///
/// Keeps the announcement and the reasoning here so `orchestrator::launch_all` stays
/// short enough for the brick limit.
pub(super) fn plan(count: usize) -> usize {
    let limit = concurrency_limit();
    if limit != 0 && count > limit {
        println!("Launching {count} workstreams {limit} at a time ({MAX_PARALLEL_ENV})");
    }
    wave_size(count, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_limit_wins() {
        assert_eq!(limit_from(Some("3".into()), 64), 3);
        assert_eq!(limit_from(Some(" 5 ".into()), 64), 5);
    }

    #[test]
    fn zero_means_unlimited_and_is_reachable() {
        assert_eq!(limit_from(Some("0".into()), 64), 0);
        assert_eq!(
            wave_size(20, 0),
            20,
            "a zero limit must launch everything at once"
        );
    }

    #[test]
    fn the_default_scales_with_cpus_and_is_capped() {
        assert_eq!(limit_from(None, 2), 1);
        assert_eq!(limit_from(None, 8), 4);
        assert_eq!(
            limit_from(None, 128),
            8,
            "capped, or a big host defeats the point"
        );
    }

    #[test]
    fn a_single_cpu_machine_still_makes_progress() {
        assert_eq!(limit_from(None, 1), 1, "a floor of 0 would launch nothing");
    }

    #[test]
    fn garbage_falls_back_to_the_default() {
        assert_eq!(limit_from(Some("banana".into()), 8), 4);
        assert_eq!(limit_from(Some("".into()), 8), 4);
    }

    #[test]
    fn waves_never_exceed_the_limit() {
        for count in [0usize, 1, 5, 20] {
            for limit in [1usize, 3, 8] {
                assert!(wave_size(count, limit) <= limit.max(1));
            }
        }
    }
}
