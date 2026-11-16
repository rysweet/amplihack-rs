//! The wait's two lines in the run's recipe log (issue #2731).
//!
//! The wait's `progress` lines go to the step's agent-stderr file, which the
//! recipe runner keeps but does not stream. A reader watching the recipe log
//! saw only the runner's `working (Ns elapsed)` heartbeat, so a step that
//! slept for hours looked hung and runs were stopped on that reading. A
//! detached run now exports its log's absolute path in
//! `AMPLIHACK_RECIPE_LOG`, and each wait appends its `start` and `end` lines
//! there as well.
//!
//! The write is best effort and silent. The stderr lines already carry the
//! information, so a log that cannot be appended to never changes what the
//! step does. Nothing is ever written to `AMPLIHACK_RECIPE_LOG_JSONL`:
//! appending plain text to the runner's own JSONL schema would corrupt it.
//!
//! Tested in `tests_recipe_log.rs`.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The marker a detached run sets for its children. Internal: never read
/// from a `config.toml`, a recipe spec or a step's declared environment.
pub(crate) const RECIPE_LOG_ENV: &str = "AMPLIHACK_RECIPE_LOG";

/// The log to append to, from the marker's raw value. A relative path is
/// refused rather than resolved, because the step's working directory is
/// repository content.
pub(super) fn path_from(raw: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(raw?);
    (!path.as_os_str().is_empty() && path.is_absolute()).then_some(path)
}

/// Appends `line` to the recipe log named in the environment, if any. Every
/// failure is silent.
pub(super) fn append(line: &str) {
    if let Some(path) = path_from(std::env::var_os(RECIPE_LOG_ENV)) {
        let _ = append_to(&path, line);
    }
}

/// One line, `<line>\n`, in a single write on an append descriptor. The file
/// is created `0600` and must be a regular file this user owns, with one
/// link and no group or other mode bits; a symlink is never followed and no
/// parent directory is ever created.
pub(super) fn append_to(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = amplihack_utils::secure_files::open_private_nofollow(path)?;
    file.write_all(format!("{line}\n").as_bytes())
}
