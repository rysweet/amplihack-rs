//! The resume order (R5.5): agents whose checkouts have the most modified
//! tracked files go first, so the work most at risk is back soonest. git
//! runs without fsmonitor, optional locks or an inherited repository (S17),
//! with the bounds of S15; any failure counts as a clean tree.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: usize = 1024 * 1024;

/// `git -c core.fsmonitor=false -C DIR status --porcelain --untracked-files=no`,
/// with the inherited repository selection (`GIT_DIR`, `GIT_WORK_TREE`,
/// `GIT_INDEX_FILE`, `GIT_COMMON_DIR` and the rest) removed by
/// `amplihack_git::command` (issue #1278).
pub fn command(dir: &Path) -> Command {
    let mut cmd = amplihack_git::command();
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// The number of lines of porcelain output; a last line without a newline
/// counts.
pub fn count(stdout: &[u8]) -> usize {
    stdout
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .count()
}

/// The modified tracked files in `dir`; 0 for any error, timeout, non-zero
/// exit or a directory that is not a repository. Output at the cap means
/// "at least this many", which is enough for ordering.
pub fn dirty_count(dir: &Path) -> usize {
    let mut cmd = command(dir);
    cmd.stdin(Stdio::null());
    match crate::util::run_output_with_timeout_limited(cmd, TIMEOUT, MAX_OUTPUT) {
        Ok(out) if out.status.success() => count(&out.stdout),
        _ => 0,
    }
}

/// Names by count, highest first, then by name in byte order.
pub fn order(mut cands: Vec<(String, usize)>) -> Vec<String> {
    cands.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    cands.into_iter().map(|(name, _)| name).collect()
}
