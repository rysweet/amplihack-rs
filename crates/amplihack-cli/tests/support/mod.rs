//! Helpers shared by the modules of the `cli_integration` test binary.

pub mod amplihack_bin;
pub mod auto_drive_light;
pub mod exec_stub;
/// Issue #2018: process liveness read from `/proc`.
pub mod proc_watch;
pub mod profile_1962;
pub mod profile_default;
pub mod profile_rotate;
pub mod quick_change_repo;
pub mod quiet_process;
/// The crate-private bounded capability probe, compiled into this binary once
/// for `runner_probe_cleanup` and `runner_probe_lifecycle` (a file may be a
/// module only once per crate). Both run their tests in a quiet process, so
/// its thread-local test hooks are never shared.
#[cfg(unix)]
#[path = "../../src/runner_probe.rs"]
pub mod runner_probe;
pub mod scratch_base;
pub mod sessions_archive;
pub mod socket_dir;
/// Issue #2018: the headless-step launch fixture.
pub mod step_2018;
/// Issue #2018: records planted by hand for the `recipe steps` tests.
pub mod step_2018_plant;
pub mod test_path;
pub mod wait;
