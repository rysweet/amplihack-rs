//! Private atomic usage evidence. No credential contents are read here.
//!
//! Capture and publication share `.auto-switch.d/lock` with credential
//! transactions. Callers already holding that lock use `under_state_lock`.
//! Accepted ordering is persisted before the record: interruption can hide
//! evidence, but can never resurrect an older accepted reading.
#[cfg(unix)]
mod refresh;
#[cfg(unix)]
mod storage;
#[cfg(unix)]
pub use refresh::{RefreshCapture, capture_refresh, publish_refresh};
mod types;
#[cfg(unix)]
pub use storage::{LoginLocation, ObservationGuard, ObservationStore, read_bounded_record};
pub use types::*;
