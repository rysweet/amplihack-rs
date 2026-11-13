//! Opt-in stage measurements for SessionStart investigation. These clocks
//! measure completed work; they never schedule hooks, probes or retries.
use std::time::Instant;

pub(super) fn measure<T>(stage: &'static str, work: impl FnOnce() -> T) -> T {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return work();
    }
    let start = Instant::now();
    let value = work();
    tracing::debug!(
        stage,
        elapsed_micros = start.elapsed().as_micros() as u64,
        "session-start stage completed"
    );
    value
}
