//! Step failure recovery (issue #2539).
//!
//! A recipe step that opts in runs through `step-recovery run`. When it
//! fails, an agent reads the failure and chooses one of four ways forward;
//! the driver validates that choice and records it. Only a recorded handoff
//! can end a run, at a `report` step, with [`STEP_RECOVERY_HANDOFF_EXIT`].
//! See `docs/reference/step-failure-recovery.md`.

mod decision;
mod driver;
mod evidence;
mod exec;
mod invoke;
mod record;
mod recover;
mod report;
mod signature;
mod text;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_review;

use super::autodrive_completion::secure_fs;
use clap::Subcommand;

/// Exit status of a run that ended in a recorded handoff. Not used by any
/// other amplihack exit path (70-77, 79, 80, 86, 91 and 97-99 are taken).
pub const STEP_RECOVERY_HANDOFF_EXIT: i32 = 81;
/// Version of the record and evidence formats.
pub const SCHEMA_VERSION: u32 = 1;
/// The default handoff owner when the caller names none.
pub const RUN_REQUESTER: &str = "run-requester";

#[derive(Subcommand, Debug)]
pub enum StepRecoveryCommands {
    /// Create (or check, with --dir) a private recovery directory and print it.
    Init(report::InitArgs),
    /// Run a step; on failure, let the recovery agent choose and record it.
    Run(driver::RunArgs),
    /// Record a handoff when required evidence is missing and no step failed.
    Record(report::RecordArgs),
    /// Exit 81 when any record in the directory is a handoff, else 0.
    Report(report::ReportArgs),
}

/// Runs a subcommand and exits with its code. Any error (bad arguments, an
/// unsafe directory or record file, a record that cannot be appended) exits 2.
pub fn run(command: StepRecoveryCommands) -> ! {
    let result = match command {
        StepRecoveryCommands::Init(args) => report::init(args),
        StepRecoveryCommands::Run(args) => driver::run(args),
        StepRecoveryCommands::Record(args) => report::record(args),
        StepRecoveryCommands::Report(args) => report::report(args),
    };
    use std::io::Write;
    let code = match result.and_then(|code| Ok(std::io::stdout().flush().map(|()| code)?)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("step-recovery: error: {e:#}");
            2
        }
    };
    std::process::exit(code)
}
