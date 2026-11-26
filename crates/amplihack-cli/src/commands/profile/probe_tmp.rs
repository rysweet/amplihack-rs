//! A private temporary folder for each usage probe (issue #3292).
//!
//! `claude` makes a session folder under its per-user temporary root even
//! with `--no-session-persistence`, and never removes it. Each probe attempt
//! therefore gets a fresh, owner-only folder under the system temporary
//! directory, passed to `claude` as [`VAR`], and the folder and everything
//! in it are removed once the attempt's process group has been ended. This
//! works the same with and without `bwrap`, because the sandbox binds `/`
//! and passes the environment through.
//!
//! Only the folder this process made is removed; nothing else in the
//! temporary root is read or changed. A failure to make or remove it is
//! reported on stderr, without a path, and does not change the probe.

use std::process::Command;

/// The variable that moves Claude Code's temporary root.
pub(super) const VAR: &str = "CLAUDE_CODE_TMPDIR";

/// The private folder of one probe attempt, if it could be made.
pub(super) struct ProbeTmp(Option<tempfile::TempDir>);

impl ProbeTmp {
    /// Makes the folder and sets [`VAR`] to it on `cmd`. When the folder
    /// cannot be made, `cmd` is left as it was and a warning is printed.
    pub(super) fn give(cmd: &mut Command) -> Self {
        let mut builder = tempfile::Builder::new();
        builder.prefix("amplihack-probe-");
        // Owner-only whatever the umask: the default mode follows it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        match builder.tempdir() {
            Ok(dir) => {
                cmd.env(VAR, dir.path());
                Self(Some(dir))
            }
            Err(_) => {
                eprintln!("warning: usage probe: could not create its private temporary folder");
                Self(None)
            }
        }
    }

    /// Removes the folder and everything in it. Call only after the probe's
    /// process group has been ended and reaped.
    pub(super) fn close(self) {
        if let Some(dir) = self.0
            && dir.close().is_err()
        {
            eprintln!("warning: usage probe: could not remove its private temporary folder");
        }
    }
}
