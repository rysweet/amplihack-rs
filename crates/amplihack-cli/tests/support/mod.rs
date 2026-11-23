//! Helpers shared by the modules of the `cli_integration` test binary.

pub mod amplihack_bin;
pub mod auto_drive_light;
/// The five profile-redirecting variables and the guards that keep a
/// test's settings writes inside its own temporary home (issue #2922).
/// One definition, shared with every crate's `test_support`.
#[path = "../../../../tests/support/claude_profile_isolation.rs"]
pub mod claude_profile_isolation;
/// Fixture for #2569: the criterion-1 QA judgement.
pub mod criterion1_world;
pub mod events;
pub mod exec_stub;
/// Issue #2931 — the end of file on a pipe that a process group holds, as
/// the event "the group is gone".
pub mod exit_witness;
pub mod fake_proc;
/// Issue #2582 — hold a process tree live on a FIFO while a test walks it.
pub mod fifo_release;
pub mod git_fixture;
pub mod ls_mode;
pub mod non_utf8_name;
pub mod proc_root_scan;
pub mod proc_tree;
pub mod profile_1962;
pub mod profile_default;
pub mod profile_rotate;
pub mod quick_change_repo;
pub mod quiet_process;
pub mod recipe_guide;
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
pub mod step_recovery;
pub mod step_recovery_check;
pub mod test_path;
/// Issue #1619 — the fake harness and temp-dir layout the model-tier
/// propagation and argv-safety tests share.
pub mod tier_harness;
pub mod unix_process;
/// The fake clocks shared with the lib tests and the utils and hooks tests
/// (issue #2019).
pub mod virtual_clock;
pub mod wait;
