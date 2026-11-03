//! Issue #1860: the auto-switch liveness scan takes its process table from
//! `procfs::proc_root()`, so the seam is still read only in `procfs.rs`. The
//! issue-1760 source scan covers `src/commands/agent/` only and is held to
//! 300 lines, so `liveness.rs` is checked here.

use std::path::PathBuf;

#[test]
fn issue_1860_the_liveness_scan_reads_the_process_table_through_procfs() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/commands/profile/auto_switch/liveness.rs");
    let raw = std::fs::read_to_string(&path).unwrap();
    // Without `//` comments, so the doc comment may describe the seam.
    let src: String = raw
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| l.split(" // ").next().unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !src.contains("AMPLIHACK_TEST_PROC_ROOT"),
        "liveness.rs must not read the seam itself; procfs.rs does"
    );
    assert!(
        src.contains("procfs::proc_root()"),
        "scan_system must ask procfs::proc_root() where the process table is"
    );
    assert!(
        !src.contains(r#"Path::new("/proc")"#),
        "scan_system must not hard-code the host's /proc"
    );
}
