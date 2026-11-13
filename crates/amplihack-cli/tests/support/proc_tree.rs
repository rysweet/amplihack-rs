//! Issue #2582: the live process tree under one PID this test started, with
//! each process's argv.
//!
//! Read-only: it sends no signal and never looks at a process outside the
//! tree. It walks `/proc/<pid>/task/<tid>/children` breadth-first from the
//! root instead of reading every `/proc/<n>/stat`, so the test stays off the
//! host process table (docs/howto/keep-tests-off-the-host-process-table.md,
//! #2042). A process that exits during the walk is skipped.
//!
//! The `children` files need `CONFIG_PROC_CHILDREN`. Without it the walk
//! returns the root alone, and callers assert on a minimum tree size so that
//! case fails rather than passes.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::io::ErrorKind;

/// One process: its PID and its argv elements (the NUL-separated fields of
/// `/proc/<pid>/cmdline`).
pub type Process = (u32, Vec<Vec<u8>>);

/// `root` and every descendant still alive, root first.
pub fn descendants(root: u32) -> Vec<Process> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
    let mut out = Vec::new();
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        let Some(argv) = cmdline(pid) else {
            continue;
        };
        out.push((pid, argv));
        queue.extend(children(pid));
    }
    out
}

/// The argv of `pid`, or `None` once it has exited.
fn cmdline(pid: u32) -> Option<Vec<Vec<u8>>> {
    match fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(raw) => Some(
            raw.split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(<[u8]>::to_vec)
                .collect(),
        ),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound) || e.raw_os_error() == Some(3) => None,
        Err(e) => panic!("cannot read /proc/{pid}/cmdline: {e}"),
    }
}

/// The direct children of every thread of `pid`.
fn children(pid: u32) -> Vec<u32> {
    let tasks = match fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => tasks,
        Err(e) if e.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(e) => panic!("cannot list /proc/{pid}/task: {e}"),
    };
    let mut out = Vec::new();
    for task in tasks.flatten() {
        let text = match fs::read_to_string(task.path().join("children")) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound || e.raw_os_error() == Some(3) => continue,
            Err(e) => panic!("cannot read {}/children: {e}", task.path().display()),
        };
        out.extend(
            text.split_whitespace()
                .filter_map(|p| p.parse::<u32>().ok()),
        );
    }
    out
}

/// Every `(pid, argv element)` in `tree` that contains any of `needles`.
pub fn matches(tree: &[Process], needles: &[&str]) -> Vec<(u32, String)> {
    tree.iter()
        .flat_map(|(pid, argv)| argv.iter().map(move |arg| (*pid, arg)))
        .filter(|(_, arg)| {
            needles
                .iter()
                .any(|n| arg.windows(n.len()).any(|w| w == n.as_bytes()))
        })
        .map(|(pid, arg)| (pid, String::from_utf8_lossy(arg).into_owned()))
        .collect()
}
