//! Consume metadata reset before the updater can reconcile installed assets.
use amplihack_cli::{Commands, commands::install::reset_codex_recovery, update};
use std::ffi::OsString;

pub(super) fn prepare(
    command: &mut Commands,
    args: &mut Vec<OsString>,
) -> anyhow::Result<update::StartupUpdateOutcome> {
    prepare_with(command, args, update::maybe_print_update_notice_from_args)
}

fn prepare_with(
    command: &mut Commands,
    args: &mut Vec<OsString>,
    updater: impl FnOnce(&[OsString]) -> update::StartupUpdateOutcome,
) -> anyhow::Result<update::StartupUpdateOutcome> {
    if let Commands::Codex {
        reset_recovery,
        args: native,
        ..
    } = command
        && *reset_recovery
    {
        reset_codex_recovery()?;
        *reset_recovery = false;
        // trailing_var_arg may start native arguments without an explicit --.
        // Preserve that entire parsed suffix, including native reset spellings.
        let native_start = args.len() - native.len();
        let mut index = 0;
        args.retain(|arg| {
            let keep = index >= native_start || arg != "--reset-recovery";
            index += 1;
            keep
        });
    }
    Ok(updater(args))
}

#[cfg(all(test, unix))]
#[path = "startup_tests.rs"]
mod tests;
