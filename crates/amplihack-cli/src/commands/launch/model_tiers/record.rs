//! Issue #1619: the measurement record — one line per launch, so the saving
//! can be counted.
//!
//! The schema is a closed set of eleven named scalars with no argv,
//! environment, prompt, working-directory or path field, pinned by
//! `tests_record.rs`: this file is a run artefact that people paste into
//! issues.
//!
//! Writing is best effort and never blocks a launch: with no run directory
//! nothing is written, and a planted symlink, a directory in the file's place
//! or a lock amplihack cannot take all skip the record rather than the step.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::table::{Effort, Tier};

/// The file every launch of one run appends to, inside `AMPLIHACK_RUN_DIR`.
pub(crate) const RECORD_FILE: &str = "model-tiers.jsonl";

/// Largest record file amplihack reads back for the run summary.
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

/// Why a tier did not become a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Fallback {
    /// The harness has no column in the mapping at all.
    NoColumn,
    /// The column exists and has no cell for this tier.
    NoMapping,
    /// The cell exists and names no model.
    NoModel,
    /// The launch goes through the LiteLLM gateway, which routes on the
    /// model name.
    Gateway,
    /// A local-model Codex launch, whose profile sets the model and the
    /// reasoning effort.
    LocalModel,
}

/// One launch's tier decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    /// Schema version of this line.
    pub(crate) v: u32,
    /// Launch sequence within the run, assigned by [`append`] under the lock.
    pub(crate) seq: u64,
    pub(crate) run_id: Option<String>,
    /// The binary amplihack launched, by name.
    pub(crate) harness: String,
    /// The mapping column that name reads.
    pub(crate) column: String,
    pub(crate) tier: Option<Tier>,
    /// The model the tier asked for.
    pub(crate) requested_model: Option<String>,
    /// The model that actually went on the command line.
    pub(crate) effective_model: Option<String>,
    pub(crate) requested_effort: Option<Effort>,
    /// The effort that actually reaches the harness: only Codex has a launch
    /// control for it.
    pub(crate) effective_effort: Option<Effort>,
    pub(crate) fallback: Option<Fallback>,
}

/// Append one record to `run_dir`'s file, assigning its sequence number.
///
/// Does nothing when `run_dir` is `None` or is not an existing directory;
/// amplihack never creates a run directory for this file.
pub(crate) fn append(run_dir: Option<&Path>, record: &Record) {
    let Some(dir) = run_dir else { return };
    if !fs::metadata(dir).is_ok_and(|meta| meta.is_dir()) {
        return;
    }
    let path = dir.join(RECORD_FILE);
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW refuses a planted symlink rather than following it into,
        // say, a settings file. A directory in the file's place fails too.
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(mut file) = options.open(&path) else {
        return;
    };
    // The open handle, not the path: re-stating by path would check a
    // different object from the one about to be written.
    if !file.metadata().is_ok_and(|meta| meta.is_file()) {
        return;
    }
    // Advisory, and skipping beats blocking: the record is a report, and it
    // must never hold up the launch it describes.
    use fs2::FileExt;
    if FileExt::try_lock_exclusive(&file).is_err() {
        return;
    }
    let mut numbered = record.clone();
    numbered.seq = count_lines(&mut file) + 1;
    if let Ok(line) = serde_json::to_string(&numbered) {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
    let _ = FileExt::unlock(&file);
}

/// How many records the open file already holds, counting a line that is not
/// a valid record: the sequence numbers launches, not valid records.
///
/// Zero when the file is not valid UTF-8, and short of the truth past the
/// `MAX_RECORD_BYTES` cut, so `seq` can repeat. The record is a report, not a
/// control.
fn count_lines(file: &mut fs::File) -> u64 {
    let mut text = String::new();
    if file
        .try_clone()
        .and_then(|handle| handle.take(MAX_RECORD_BYTES).read_to_string(&mut text))
        .is_err()
    {
        return 0;
    }
    text.lines().filter(|line| !line.trim().is_empty()).count() as u64
}

/// Every record of the run in `run_dir`, in launch order.
///
/// A half-written or unreadable line is skipped: the run summary is a report
/// and must never fail because of what it reports on.
pub(crate) fn read_all(run_dir: &Path) -> Vec<Record> {
    let path = run_dir.join(RECORD_FILE);
    let Ok(file) = fs::File::open(&path) else {
        return Vec::new();
    };
    let mut text = String::new();
    if file
        .take(MAX_RECORD_BYTES)
        .read_to_string(&mut text)
        .is_err()
    {
        return Vec::new();
    }
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Record>(line).ok())
        .collect()
}
