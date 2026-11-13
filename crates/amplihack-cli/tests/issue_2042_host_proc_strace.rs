//! Issue #2042, acceptance criterion 1, checked by the kernel's view rather
//! than by reading source: commands whose tests use a fake process table
//! must not touch the host's.
//!
//! Each test runs one real fixture command under `strace -f`, so every
//! file-path syscall of `amplihack` and of everything it starts is logged.
//! The log fails the test when any traced process
//!
//! - opens `/proc` itself (a whole-table scan), or
//! - names `/proc/<pid>/...` for a pid outside the traced tree (another
//!   process's `cmdline`, `environ`, `cwd`, `fd`, `stat`, ...).
//!
//! `/proc/self/*`, `/proc/<own pid>/*` and system files such as
//! `/proc/loadavg` are allowed. Each test also checks that the command read
//! its fake tree, so a trace that saw nothing cannot pass.
//!
//! This catches a host read wherever it is written, including in a file no
//! source guard lists.
//!
//! `recipe stop <run_id>` (issue #2175) is traced too. It finds the run
//! through the record `recipe run` wrote, so besides the traced tree it may
//! name only the pids of the run it stops, and never `/proc` itself.
//!
//! Without `strace` the tests print a skip line and pass, unless
//! `AMPLIHACK_TEST_REQUIRE_STRACE` is non-empty. CI installs `strace` and
//! sets it, so CI always runs the check.

#![cfg(target_os = "linux")]

use crate::isolated_amplihack::reapply_proc_root;
use crate::support::fake_proc::{FAKE_PID_BASE, FakeProc};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const REQUIRE_ENV: &str = "AMPLIHACK_TEST_REQUIRE_STRACE";

/// The `strace` binary, or `None` when the check is skipped. Panics when it
/// is missing and `AMPLIHACK_TEST_REQUIRE_STRACE` is set.
fn strace() -> Option<PathBuf> {
    let found = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
        .map(|d| d.join("strace"))
        .find(|p| p.is_file());
    if found.is_none() {
        let required = std::env::var_os(REQUIRE_ENV).is_some_and(|v| !v.is_empty());
        assert!(
            !required,
            "{REQUIRE_ENV} is set but strace is not installed"
        );
        eprintln!("skip: strace is not installed; the host /proc check did not run");
    }
    found
}

/// `cmd` re-run under `strace`, logging to `log`. The fixture's environment
/// is copied onto a cleared one, so the traced child sees what it would see
/// untraced.
fn traced(strace: &Path, cmd: &Command, log: &Path) -> Command {
    let mut c = Command::new(strace);
    c.args(["-f", "-qq", "-s", "4096", "-e", "trace=%file", "-o"])
        .arg(log)
        .arg("--")
        .arg(cmd.get_program())
        .args(cmd.get_args())
        .stdin(Stdio::null());
    if let Some(dir) = cmd.get_current_dir() {
        c.current_dir(dir);
    }
    c.env_clear();
    reapply_proc_root(&mut c);
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => c.env(key, value),
            None => c.env_remove(key),
        };
    }
    c
}

/// The traced lines that read the host process table: `/proc` itself, or
/// `/proc/<pid>/...` for a pid that is neither in the traced tree nor in
/// `allowed`. `trace` is `strace -f -o` output: each line starts with the pid
/// that made the call.
fn host_proc_reads(trace: &str, allowed: &BTreeSet<u32>) -> Vec<String> {
    let pid_of = |line: &str| -> Option<u32> { line.split_whitespace().next()?.parse().ok() };
    let traced: BTreeSet<u32> = trace.lines().filter_map(pid_of).collect();
    let mut bad = Vec::new();
    for line in trace.lines() {
        let Some(path) = line.split('"').nth(1) else {
            continue;
        };
        let path = path.trim_end_matches('/');
        let host = if path == "/proc" {
            true
        } else if let Some(rest) = path.strip_prefix("/proc/") {
            let first = rest.split('/').next().unwrap_or("");
            !first.is_empty()
                && first.bytes().all(|b| b.is_ascii_digit())
                && first
                    .parse::<u32>()
                    .is_ok_and(|pid| !traced.contains(&pid) && !allowed.contains(&pid))
        } else {
            false
        };
        if host {
            bad.push(line.to_string());
        }
    }
    bad
}

/// Runs `cmd` under strace and asserts that it read `fake` and never the
/// host table. Returns the command's output; `None` when the check is skipped.
fn check(name: &str, cmd: &Command, fake: &Path) -> Option<Output> {
    let strace = strace()?;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("trace");
    let out = traced(&strace, cmd, &log).output().expect("run strace");
    let trace = std::fs::read_to_string(&log).unwrap_or_default();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        trace.lines().count() > 0,
        "{name}: strace logged nothing (ptrace refused?): {stderr}"
    );
    assert!(
        !stderr.contains("AMPLIHACK_TEST_PROC_ROOT ignored"),
        "{name}: the child refused its fake tree: {stderr}"
    );
    let bad = host_proc_reads(&trace, &BTreeSet::new());
    assert!(
        bad.is_empty(),
        "{name} read the host process table (see \
         docs/howto/keep-tests-off-the-host-process-table.md):\n{}",
        bad.join("\n")
    );
    let fake = fake.to_str().unwrap();
    assert!(
        trace.contains(fake),
        "{name}: the command never read its fake tree {fake}, so the trace proves nothing"
    );
    Some(out)
}

// --- the parser ---------------------------------------------------------------

#[test]
fn the_trace_parser_flags_host_reads_and_nothing_else() {
    let trace = "\
100 execve(\"/bin/amplihack\", [\"amplihack\"], 0x0 /* 3 vars */) = 0
100 openat(AT_FDCWD, \"/proc/self/cgroup\", O_RDONLY|O_CLOEXEC) = 3
100 openat(AT_FDCWD, \"/proc/loadavg\", O_RDONLY|O_CLOEXEC) = 3
100 openat(AT_FDCWD, \"/proc/100/stat\", O_RDONLY|O_CLOEXEC) = 3
101 openat(AT_FDCWD, \"/proc/100/environ\", O_RDONLY|O_CLOEXEC) = 3
100 openat(AT_FDCWD, \"/tmp/x/amplihack-fake-proc-1/7/environ\", O_RDONLY) = 3
100 openat(AT_FDCWD, \"/proc\", O_RDONLY|O_NONBLOCK|O_CLOEXEC|O_DIRECTORY) = 4
100 openat(AT_FDCWD, \"/proc/\", O_RDONLY|O_DIRECTORY) = 4
100 openat(AT_FDCWD, \"/proc/4242/cmdline\", O_RDONLY|O_CLOEXEC) = 5
101 readlink(\"/proc/4243/cwd\", \"/\", 4096) = 1
100 openat(AT_FDCWD, \"/proc/sys/kernel/pid_max\", O_RDONLY) = 3
";
    let bad = host_proc_reads(trace, &BTreeSet::new());
    assert_eq!(bad.len(), 4, "{bad:#?}");
    assert!(bad[0].contains("\"/proc\","));
    assert!(bad[1].contains("\"/proc/\","));
    assert!(bad[2].contains("/proc/4242/cmdline"));
    assert!(bad[3].contains("/proc/4243/cwd"));

    // Allowed pids may be named; `/proc` itself never may.
    let bad = host_proc_reads(trace, &BTreeSet::from([4242]));
    assert_eq!(bad.len(), 3, "{bad:#?}");
    assert!(bad[0].contains("\"/proc\","));
    assert!(bad[1].contains("\"/proc/\","));
    assert!(bad[2].contains("/proc/4243/cwd"));
}

// --- the commands -------------------------------------------------------------

#[test]
fn profile_commands_never_read_the_host_process_table() {
    use crate::support::profile_rotate::Fixture;
    let f = Fixture::new();
    let fake = FakeProc::new();
    let dir = f.root.join("spare");
    fake.process(FAKE_PID_BASE)
        .cmdline(&["claude"])
        .status('S')
        .environ(&[("CLAUDE_CONFIG_DIR", dir.to_str().unwrap())]);
    f.with_proc(&fake);
    for args in [
        &["profile", "auto-switch", "status"][..],
        &["profile", "rotate", "status"],
        &["profile", "switch", "spare"],
        &["profile", "rotate", "--dry-run"],
    ] {
        check(&args.join(" "), &f.cmd(args), fake.path());
    }
}

#[test]
fn sessions_archive_never_reads_the_host_process_table() {
    use crate::isolated_amplihack::PROC_ROOT_ENV;
    use crate::support::sessions_archive::Fx;
    let fx = Fx::new();
    fx.session("old-1", 10, &[]);
    let fake = FakeProc::new();
    fake.process(FAKE_PID_BASE)
        .cmdline(&["sleep", "3600"])
        .status('S')
        .environ(&[(
            "CLAUDE_CONFIG_DIR",
            fx.home.join(".claude").to_str().unwrap(),
        )]);
    let dest = fx.dest.to_str().unwrap().to_string();
    let mut c = fx.cmd(&["sessions", "archive", "--dest", &dest]);
    c.env(PROC_ROOT_ENV, fake.path());
    check("sessions archive", &c, fake.path());
}

#[test]
fn cleanup_never_reads_the_host_process_table() {
    use crate::issue_1808_run_cleanup::fixture::{Fx, age_tree};
    let mut fx = Fx::new();
    let busy = fx.tmp.join("replay-busy");
    let idle = fx.tmp.join("replay-idle");
    for d in [&busy, &idle] {
        std::fs::create_dir_all(d).unwrap();
    }
    fx.live_process(&busy);
    for d in [&busy, &idle] {
        age_tree(d, 72);
    }
    let mut c = fx.amplihack();
    c.args(["cleanup", "--format", "json", "--only", "tmp-leftover"]);
    let Some(out) = check("cleanup", &c, fx.proc.path()) else {
        return;
    };
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let action = |name: &str| {
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"].as_str().is_some_and(|p| p.ends_with(name)))
            .map(|e| (e["action"].clone(), e["reason"].clone()))
            .unwrap_or_else(|| panic!("{name} not in the report: {report}"))
    };
    let (busy_action, busy_reason) = action("replay-busy");
    assert_eq!(busy_action, "skipped", "{report}");
    assert_eq!(busy_reason, "in use by a running process", "{report}");
    assert_ne!(action("replay-idle").0, "skipped", "{report}");
}

// --- recipe stop <run_id> (issue #2175) ---------------------------------------

/// `recipe stop <run_id>` finds the run through its record and reads `/proc`
/// only for that run's own pids: no table scan, no other process's
/// `environ`. The run starts outside strace; only the stop is traced.
#[test]
fn recipe_stop_by_run_id_reads_no_host_process_table() {
    use crate::issue_1606_detach::{Fx, assert_gone, text, wait_bounded};
    let Some(strace) = strace() else {
        return;
    };
    let fx = Fx::new();
    let (mut amp, run_id) = fx.spawn_foreground_run();
    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");

    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("trace");
    let stop = fx.cmd(&["recipe", "stop", &run_id], &[]);
    let tracer = traced(&strace, &stop, &log)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run strace");
    // `recipe stop` walks its own parent chain (strace, this test, ...) to
    // refuse an outer run it descends from; those `stat` reads are its own
    // ancestry, not the process table.
    let ancestry: BTreeSet<u32> = std::iter::once(tracer.id())
        .chain(test_ancestry())
        .collect();
    let out = tracer.wait_with_output().expect("wait for strace");
    let trace = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        trace.lines().count() > 0,
        "strace logged nothing (ptrace refused?): {}",
        text(&out.stderr)
    );
    assert!(out.status.success(), "{}", text(&out.stderr));

    let ancestor_reads: Vec<_> = trace
        .lines()
        .filter(|line| {
            let path = line.split('"').nth(1).unwrap_or("");
            ancestry.iter().any(|pid| {
                path.strip_prefix(&format!("/proc/{pid}/"))
                    .is_some_and(|rest| rest != "stat")
            })
        })
        .collect();
    assert!(
        ancestor_reads.is_empty(),
        "recipe stop may read only `stat` of its ancestors: {ancestor_reads:#?}"
    );
    let allowed: BTreeSet<u32> = [amp.id(), runner as u32, child as u32]
        .into_iter()
        .chain(ancestry.iter().copied())
        .collect();
    let bad = host_proc_reads(&trace, &allowed);
    assert!(
        bad.is_empty(),
        "recipe stop <run_id> read the host process table (issue #2175):\n{}",
        bad.join("\n")
    );
    let environ: Vec<_> = trace.lines().filter(|l| l.contains("/environ")).collect();
    assert!(
        environ.is_empty(),
        "recipe stop must not read any environ: {environ:#?}"
    );
    let record = format!("procs/{run_id}.json");
    assert!(
        trace.contains(&record),
        "recipe stop never opened {record}, so the trace proves nothing"
    );

    assert_gone(&[runner, child]);
    let status = wait_bounded(&mut amp);
    assert!(!status.success(), "a stopped run must not report success");
}

/// This test process and its ancestors, by `/proc/<pid>/stat` field 4.
fn test_ancestry() -> Vec<u32> {
    let mut chain = Vec::new();
    let mut pid = std::process::id();
    while pid > 1 && !chain.contains(&pid) {
        chain.push(pid);
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            break;
        };
        let Some(ppid) = stat
            .rfind(')')
            .and_then(|end| stat[end + 1..].split_whitespace().nth(1))
            .and_then(|ppid| ppid.parse().ok())
        else {
            break;
        };
        pid = ppid;
    }
    chain
}
