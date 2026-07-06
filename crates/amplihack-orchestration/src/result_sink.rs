//! The **clean result channel**: a dedicated sink an agent writes its answer to,
//! kept OUT of the noisy human/log stdout stream.
//!
//! Motivation: when one agent's output feeds another agent (or a verdict
//! decision), the handoff should be semantic and clean. Consumers must not have
//! to strip ANSI, brace-scan, or `serde`-parse a firehose of interleaved
//! tracing/banner/log lines to recover the agent's answer — the exact fragility
//! that keeps breaking `extract_json_payload`-style scraping downstream.
//!
//! This module is the small, dependency-free surface the runner and its
//! consumers share:
//!
//! * [`RESULT_SINK_ENV`] — the single env var the runner exports to the child.
//! * [`allocate_sink_path`] — allocate a fresh, unique, runner-owned sink path.
//! * [`inject_sink_env`] — export the sink path onto a child `Command`.
//! * [`read_sink_verbatim`] — read a sink's bytes back **verbatim** (or `None`).
//!
//! The transport is deliberately minimal: one env var naming one file, captured
//! byte-for-byte. See `docs/reference/clean-result-channel.md` for the full
//! contract and rationale.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The env var the runner exports to the child when the caller opts into the
/// clean channel. The child writes its final answer to the file this names.
/// External consumers (recipe-runner-rs, Simard) key off this exact name.
pub const RESULT_SINK_ENV: &str = "AMPLIHACK_RESULT_SINK";

/// Upper bound on a sink read (SEC-3). A sink larger than this yields `None`
/// so a runaway or hostile child can never force an unbounded allocation;
/// consumers simply fall back to captured stdout. Answers are semantic text,
/// not bulk data, so 16 MiB is comfortably generous.
const MAX_SINK_BYTES: u64 = 16 * 1024 * 1024;

/// Monotonic per-process counter guaranteeing distinct sink names even when two
/// allocations land within the same nanosecond.
static ALLOC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Allocate a fresh, unique sink path under `runtime_dir`.
///
/// Creates `runtime_dir` (recursively) if it does not exist. On Unix the
/// runner-created directory is owner-only (`0700`, SEC-6). The returned path
/// does **not** exist yet — it names the file the child is expected to write.
/// Each call returns a distinct path so parallel spawns (debate / n_version /
/// expert_panel fan-out) never race one another's writes.
pub fn allocate_sink_path(runtime_dir: &Path) -> std::io::Result<PathBuf> {
    ensure_owner_only_dir(runtime_dir)?;

    let counter = ALLOC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let file_name = format!("result-{pid}-{nanos}-{counter}.sink");
    Ok(runtime_dir.join(file_name))
}

#[cfg(unix)]
fn ensure_owner_only_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn ensure_owner_only_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Export `AMPLIHACK_RESULT_SINK=<path>` onto `command`'s environment so the
/// spawned child knows where to write its answer. Call this on the
/// `std::process::Command` before it is spawned.
pub fn inject_sink_env(command: &mut std::process::Command, sink: &Path) {
    command.env(RESULT_SINK_ENV, sink);
}

/// Read a sink's contents back **verbatim** — no ANSI stripping, no trimming,
/// no newline normalization, no parsing.
///
/// Returns:
/// * `Some(contents)` when the file exists, is a regular file within the size
///   cap, and holds valid UTF-8.
/// * `None` when the file is missing, empty, larger than [`MAX_SINK_BYTES`], not
///   a regular file, or not valid UTF-8.
///
/// Empty and unwritten collapse to the same `None` signal so a consumer never
/// observes `Some("")` and a child that "opts out" by not writing behaves
/// identically to one that never touched the file.
pub fn read_sink_verbatim(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let len = meta.len();
    if len == 0 || len > MAX_SINK_BYTES {
        return None;
    }

    let bytes = std::fs::read(path).ok()?;
    // Re-check on the bytes actually read: the file may have changed between the
    // metadata probe and the read (TOCTOU). Reject if it grew past the cap or
    // shrank to empty, so both the size bound and the empty-is-None invariant
    // hold regardless of a concurrent write.
    if bytes.is_empty() || bytes.len() as u64 > MAX_SINK_BYTES {
        return None;
    }

    String::from_utf8(bytes).ok()
}
