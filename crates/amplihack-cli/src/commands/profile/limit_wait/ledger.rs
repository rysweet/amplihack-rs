//! The wait ledger (issue #1743). When `AMPLIHACK_USAGE_WAIT_LEDGER` names an
//! absolute path, each finished wait appends its real length in whole seconds
//! as one line. Auto-drive sets it for every child it times, so its wall
//! budget can leave the time spent waiting for a usage limit out; see
//! docs/reference/auto-drive-to-merge.md#budget-keys-and-defaults.
//!
//! The file is created `0600` and never followed through a symlink. A write
//! that fails is a WARNING on stderr and never changes the step's result.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

pub(super) const LEDGER_ENV: &str = "AMPLIHACK_USAGE_WAIT_LEDGER";

/// Appends `waited` to the ledger named in the environment, if any.
pub(super) fn record(waited: Duration) {
    let Some(path) = std::env::var_os(LEDGER_ENV) else {
        return;
    };
    let path = Path::new(&path);
    if path.as_os_str().is_empty() {
        return;
    }
    if !path.is_absolute() {
        eprintln!("WARNING: {LEDGER_ENV} is not an absolute path; the wait is not recorded.");
        return;
    }
    if append(path, waited.as_secs()).is_err() {
        eprintln!("WARNING: the usage-limit wait could not be recorded in {LEDGER_ENV}.");
    }
}

/// One line, `<seconds>\n`, in a single write on an append descriptor.
pub(super) fn append(path: &Path, secs: u64) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut f = options.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "the wait ledger is not a regular file",
        ));
    }
    f.write_all(format!("{secs}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::append;

    #[test]
    fn appends_one_line_per_wait_in_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ledger");
        append(&p, 5).unwrap();
        append(&p, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "5\n0\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "").unwrap();
        let link = dir.path().join("ledger");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(append(&link, 7).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
    }
}
