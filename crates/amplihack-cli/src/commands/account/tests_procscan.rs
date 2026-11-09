//! Issue #2006: counting running jobs per account from a fake `/proc`.
//!
//! Wired from `account/mod.rs` as `#[cfg(all(test, target_os = "linux"))] mod tests_procscan;`.
//!
//! Contract (docs/reference/account-pick.md, "Running jobs"):
//!
//! - `procscan::walk(proc_root, key, self_pid, uid) -> Vec<ProcEntry>` where
//!   `ProcEntry { pid: u32, ppid: u32, matched: Option<OsString> }` keeps
//!   only the value of `key` (`CODEX_HOME` or `CLAUDE_CONFIG_DIR`). The real
//!   uid comes from `status` and is checked before `environ` is opened. The
//!   parent comes from `stat`, after the last `)`.
//! - `procscan::count_jobs(proc_root, key, homes, self_pid, uid) -> (JobCounts, Vec<String>)`
//!   with `homes` canonical. `JobCounts::count(&Path) -> usize` and
//!   `JobCounts::jobs: Vec<Job { pid, home, ancestors: Vec<u32> }>`, where
//!   `ancestors` is the parent chain of the job, nearest first.
//! - A process matches a home when its value is non-empty, absolute and
//!   canonicalises to that home. It is a job when its parent does not match
//!   the same home. The scanner's own pid, zombies, other users' processes
//!   and unreadable entries are skipped.

use super::procscan::{self, JobCounts};
use super::test_fixtures::*;
use std::path::PathBuf;

const KEY: &str = "CODEX_HOME";

fn count(p: &FakeProc, homes: &Homes) -> JobCounts {
    procscan::count_jobs(&p.root, KEY, &homes.all(), SELF_PID, UID).0
}

#[test]
fn a_matching_process_counts_as_one_job() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'S', &[b"A=1", &kv(KEY, &h.home("a"))]);
    let c = count(&p, &h);
    assert_eq!(c.count(&h.home("a")), 1);
    assert_eq!(c.count(&h.home("b")), 0);
    assert_eq!(c.jobs.len(), 1);
    assert_eq!(c.jobs[0].pid, 100);
    assert_eq!(c.jobs[0].home, h.home("a"));
}

#[test]
fn a_parent_and_child_with_the_same_home_count_once() {
    let (p, h) = (FakeProc::new(), Homes::new());
    let env = kv(KEY, &h.home("a"));
    p.process(100, 1, UID, 'S', &[&env]);
    p.process(101, 100, UID, 'S', &[&env]);
    p.process(102, 101, UID, 'S', &[&env]);
    let c = count(&p, &h);
    assert_eq!(c.count(&h.home("a")), 1, "a job tree counts once");
    assert_eq!(c.jobs[0].pid, 100);
}

#[test]
fn a_child_on_a_different_home_is_its_own_job() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    p.process(101, 100, UID, 'S', &[&kv(KEY, &h.home("b"))]);
    let c = count(&p, &h);
    assert_eq!(c.count(&h.home("a")), 1);
    assert_eq!(c.count(&h.home("b")), 1);
}

#[test]
fn two_separate_launches_on_one_home_are_two_jobs() {
    let (p, h) = (FakeProc::new(), Homes::new());
    let env = kv(KEY, &h.home("c"));
    p.process(100, 1, UID, 'S', &[&env]);
    p.process(200, 1, UID, 'S', &[&env]);
    assert_eq!(count(&p, &h).count(&h.home("c")), 2);
}

#[test]
fn the_scanner_itself_is_not_counted() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(SELF_PID, 1, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    assert_eq!(count(&p, &h).count(&h.home("a")), 0);
}

#[test]
fn zombies_are_not_counted() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'Z', &[&kv(KEY, &h.home("a"))]);
    assert_eq!(count(&p, &h).count(&h.home("a")), 0);
}

#[test]
fn another_users_process_is_skipped_before_its_environ_is_opened() {
    let (p, h) = (FakeProc::new(), Homes::new());
    let trap = p.trapped_process(100, 1, OTHER_UID);
    p.process(101, 1, UID, 'S', &[&kv(KEY, &h.home("b"))]);
    let c = count(&p, &h);
    trap.assert_never_opened("count_jobs on another user's process");
    assert_eq!(c.count(&h.home("a")), 0);
    assert_eq!(c.count(&h.home("b")), 1, "the scan still finishes");
    let entries = procscan::walk(&p.root, KEY, SELF_PID, UID);
    trap.assert_never_opened("walk on another user's process");
    assert!(entries.iter().all(|e| e.pid != 100), "{entries:?}");
}

#[test]
fn an_unreadable_environ_is_skipped() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.unreadable_process(100, 1, UID, &[&kv(KEY, &h.home("a"))]);
    p.process(101, 1, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    // Running as root reads mode-000 files; then the entry counts, which
    // still proves nothing panics. Otherwise only 101 counts.
    let n = count(&p, &h).count(&h.home("a"));
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(n, 1);
    }
}

#[test]
fn a_symlinked_home_matches_its_target() {
    let (p, h) = (FakeProc::new(), Homes::new());
    let link = h.root.join("link-to-a");
    std::os::unix::fs::symlink(h.home("a"), &link).unwrap();
    p.process(100, 1, UID, 'S', &[&kv(KEY, &link)]);
    assert_eq!(count(&p, &h).count(&h.home("a")), 1);
}

#[test]
fn a_parent_through_a_symlink_and_a_child_through_the_target_count_once() {
    let (p, h) = (FakeProc::new(), Homes::new());
    let link = h.root.join("link-to-a");
    std::os::unix::fs::symlink(h.home("a"), &link).unwrap();
    p.process(100, 1, UID, 'S', &[&kv(KEY, &link)]);
    p.process(101, 100, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    assert_eq!(count(&p, &h).count(&h.home("a")), 1);
}

#[test]
fn empty_relative_and_overlong_values_are_ignored() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'S', &[b"CODEX_HOME="]);
    p.process(101, 1, UID, 'S', &[b"CODEX_HOME=homes/a"]);
    let long = format!("CODEX_HOME=/{}", "x".repeat(8192));
    p.process(102, 1, UID, 'S', &[long.as_bytes()]);
    let c = count(&p, &h);
    assert!(c.jobs.is_empty(), "{:?}", c.jobs);
}

#[test]
fn a_value_naming_no_candidate_is_not_counted() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'S', &[b"CODEX_HOME=/nonexistent/elsewhere"]);
    assert!(count(&p, &h).jobs.is_empty());
}

#[test]
fn only_the_requested_key_matches() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'S', &[&kv("CLAUDE_CONFIG_DIR", &h.home("a"))]);
    p.process(101, 1, UID, 'S', &[&kv("XCODEX_HOME", &h.home("a"))]);
    assert_eq!(count(&p, &h).count(&h.home("a")), 0);
    let claude = procscan::count_jobs(&p.root, "CLAUDE_CONFIG_DIR", &h.all(), SELF_PID, UID).0;
    assert_eq!(claude.count(&h.home("a")), 1);
}

#[test]
fn the_parent_is_read_after_the_last_parenthesis_of_stat() {
    // The fixture names every command "(co) dex)". A parser splitting on
    // the first ')' would read the state as the parent and miss the tree.
    let (p, h) = (FakeProc::new(), Homes::new());
    let env = kv(KEY, &h.home("a"));
    p.process(300, 1, UID, 'S', &[&env]);
    p.process(301, 300, UID, 'S', &[&env]);
    let entries = procscan::walk(&p.root, KEY, SELF_PID, UID);
    let child = entries
        .iter()
        .find(|e| e.pid == 301)
        .expect("pid 301 walked");
    assert_eq!(child.ppid, 300);
    assert_eq!(count(&p, &h).count(&h.home("a")), 1);
}

#[test]
fn ancestors_are_recorded_nearest_first() {
    let (p, h) = (FakeProc::new(), Homes::new());
    // amplihack (500) -> sh (501) -> codex (502, matches)
    p.process(500, 1, UID, 'S', &[b"A=1"]);
    p.process(501, 500, UID, 'S', &[b"A=1"]);
    p.process(502, 501, UID, 'S', &[&kv(KEY, &h.home("b"))]);
    let c = count(&p, &h);
    assert_eq!(c.jobs.len(), 1);
    assert_eq!(c.jobs[0].pid, 502);
    assert_eq!(&c.jobs[0].ancestors[..2], &[501, 500]);
}

#[test]
fn a_parent_cycle_does_not_loop() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(700, 701, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    p.process(701, 700, UID, 'S', &[b"A=1"]);
    let c = count(&p, &h);
    assert_eq!(c.count(&h.home("a")), 1);
    assert!(c.jobs[0].ancestors.len() <= 64);
}

#[test]
fn walk_keeps_only_the_requested_value() {
    let (p, h) = (FakeProc::new(), Homes::new());
    let secret = format!("OPENAI_API_KEY={SECRET}");
    p.process(
        100,
        1,
        UID,
        'S',
        &[secret.as_bytes(), &kv(KEY, &h.home("a"))],
    );
    p.process(101, 1, UID, 'S', &[secret.as_bytes()]);
    let entries = procscan::walk(&p.root, KEY, SELF_PID, UID);
    let e100 = entries.iter().find(|e| e.pid == 100).unwrap();
    assert_eq!(e100.matched.as_deref(), Some(h.home("a").as_os_str()));
    let e101 = entries.iter().find(|e| e.pid == 101).unwrap();
    assert_eq!(e101.matched, None);
    let dump = format!("{entries:?}");
    assert!(
        !dump.contains(SECRET),
        "the rest of environ was kept: {dump}"
    );
    let jobs = format!("{:?}", count(&p, &h).jobs);
    assert!(!jobs.contains(SECRET), "{jobs}");
}

#[test]
fn a_missing_proc_root_counts_nothing_and_warns() {
    let h = Homes::new();
    let (c, warnings) = procscan::count_jobs(
        &PathBuf::from("/nonexistent/proc"),
        KEY,
        &h.all(),
        SELF_PID,
        UID,
    );
    assert!(c.jobs.is_empty());
    assert_eq!(c.count(&h.home("a")), 0);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("job counting unavailable")
            && warnings[0].contains("/nonexistent/proc"),
        "{warnings:?}"
    );
}

#[test]
fn a_readable_proc_root_gives_no_warning() {
    let (p, h) = (FakeProc::new(), Homes::new());
    p.process(100, 1, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    let (_, warnings) = procscan::count_jobs(&p.root, KEY, &h.all(), SELF_PID, UID);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn adding_jobs_moves_the_choice_to_the_next_account() {
    use super::rank::{self, Scored};
    let (p, h) = (FakeProc::new(), Homes::new());
    let live = [80.0, 50.0, 20.0];
    let choose = |c: &JobCounts| {
        let rows: Vec<Scored> = h
            .all()
            .into_iter()
            .zip(live)
            .map(|(home, l)| {
                let n = c.count(&home);
                Scored::new(home, Some(l), true, n, 0, 15.0)
            })
            .collect();
        rows[rank::choose(&rows, &rank::order(&rows)).unwrap()]
            .home
            .clone()
    };
    assert_eq!(choose(&count(&p, &h)), h.home("a"));
    // Three jobs on a: 80 - 45 = 35 < 50.
    for pid in [100, 200, 300] {
        p.process(pid, 1, UID, 'S', &[&kv(KEY, &h.home("a"))]);
    }
    assert_eq!(choose(&count(&p, &h)), h.home("b"));
}
