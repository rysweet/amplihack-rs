//! The files every fake process table holds (issues #2042, #2196). Std only,
//! so `isolated_amplihack.rs`, `cli_integration`'s `fake_proc.rs` and the
//! `amplihack-cli` lib tests (through `fake_proc.rs`) all include this one
//! file with `#[path]` and seed their trees the same way.

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

/// The constant `self/environ` of every fake process table.
pub const SELF_ENVIRON: &[u8] = b"AMPLIHACK_FAKE=1\0";

/// Fill an existing directory `root` with the files every fake process
/// table holds: `self/` (mode 0700) and `self/environ` (mode 0600) with
/// [`SELF_ENVIRON`]. `self/environ` must exist, or the liveness scan reports
/// "unsupported" and counts every profile as live. The one builder for both
/// `shared_empty_proc_root` and `FakeProc` (issue #2042); panics on any
/// I/O failure.
pub fn seed_proc_tree(root: &Path) {
    let dir = root.join("self");
    DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    let environ = dir.join("environ");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&environ)
        .and_then(|mut f| f.write_all(SELF_ENVIRON))
        .unwrap_or_else(|e| panic!("write {}: {e}", environ.display()));
}
