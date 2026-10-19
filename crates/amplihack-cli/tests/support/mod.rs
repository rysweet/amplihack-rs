//! Helpers shared by the modules of the `cli_integration` test binary.

pub mod amplihack_bin;
pub mod exec_stub;
pub mod profile_rotate;
pub mod quiet_process;
/// The crate-private bounded capability probe, compiled into this binary once
/// for `runner_probe_cleanup` and `runner_probe_lifecycle` (a file may be a
/// module only once per crate). Both run their tests in a quiet process, so
/// its thread-local test hooks are never shared.
#[cfg(unix)]
#[path = "../../src/runner_probe.rs"]
pub mod runner_probe;
pub mod test_path;
