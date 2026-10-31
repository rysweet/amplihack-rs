//! The synthetic `/proc` and Claude's `sessions/<pid>.json` files for the
//! issue #1760 tests. `/proc/self` is a link to `SELF_PID`, an `amplihack`
//! process; the `amplihack` under test reads this tree through
//! `AMPLIHACK_TEST_PROC_ROOT`.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::issue_1760_fixture::{Fx, SELF_PID, chmod};
use std::fs;
use std::path::{Path, PathBuf};

/// One synthetic `/proc/<pid>` entry.
#[derive(Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub start: u64,
    pub state: char,
    /// Real and effective uid; `None` means this test's uid.
    pub uids: Option<(u32, u32)>,
}

impl Proc {
    /// `comm` is the base name of `argv[0]`, cut to 15 bytes as the kernel does.
    pub fn new(pid: u32, ppid: u32, argv: &[&str], cwd: &Path) -> Self {
        let base = Path::new(argv[0]).file_name().unwrap().to_string_lossy();
        Self {
            pid,
            ppid,
            comm: base.chars().take(15).collect(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            cwd: cwd.to_path_buf(),
            start: 777,
            state: 'S',
            uids: None,
        }
    }
    pub fn env(mut self, k: &str, v: &str) -> Self {
        self.env.retain(|(n, _)| n != k);
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn start(mut self, start: u64) -> Self {
        self.start = start;
        self
    }
    pub fn state(mut self, state: char) -> Self {
        self.state = state;
        self
    }
    pub fn uids(mut self, ruid: u32, euid: u32) -> Self {
        self.uids = Some((ruid, euid));
        self
    }
    pub fn comm(mut self, comm: &str) -> Self {
        self.comm = comm.into();
        self
    }
}

/// A `/proc/<pid>/stat` line with `starttime` (field 22) set to `start`.
pub fn stat_line(pid: u32, comm: &str, state: char, ppid: u32, start: u64) -> String {
    format!(
        "{pid} ({comm}) {state} {ppid} {pid} {pid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {start} 1000000 100\n"
    )
}

impl Fx {
    // ----- synthetic /proc -----

    /// `/proc/self` is a link to `SELF_PID`, an `amplihack` process whose
    /// parent is `ppid`.
    pub fn set_self_parent(&self, ppid: u32) {
        self.add_proc(&Proc::new(
            SELF_PID,
            ppid,
            &["amplihack", "agent"],
            &self.work,
        ));
        let link = self.proc_root.join("self");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(SELF_PID.to_string(), &link).unwrap();
    }
    /// Makes `/proc` unreadable for a snapshot: `self` is gone.
    pub fn break_proc(&self) {
        fs::remove_file(self.proc_root.join("self")).unwrap();
    }
    pub fn add_proc(&self, p: &Proc) {
        let d = self.proc_root.join(p.pid.to_string());
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("stat"),
            stat_line(p.pid, &p.comm, p.state, p.ppid, p.start),
        )
        .unwrap();
        let (r, e) = p.uids.unwrap_or((self.uid, self.uid));
        let status = format!(
            "Name:\t{}\nState:\t{} (x)\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nUid:\t{r}\t{e}\t{e}\t{e}\nGid:\t0\t0\t0\t0\n",
            p.comm, p.state, p.pid, p.pid, p.ppid
        );
        fs::write(d.join("status"), status).unwrap();
        fs::write(d.join("comm"), format!("{}\n", p.comm)).unwrap();
        let nul = |v: Vec<String>| v.into_iter().map(|s| s + "\0").collect::<String>();
        fs::write(d.join("cmdline"), nul(p.argv.clone())).unwrap();
        let env = p.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        fs::write(d.join("environ"), nul(env)).unwrap();
        std::os::unix::fs::symlink(&p.cwd, d.join("cwd")).unwrap();
    }
    pub fn remove_proc(&self, pid: u32) {
        fs::remove_dir_all(self.proc_root.join(pid.to_string())).unwrap();
    }
    /// The process's `cmdline` cannot be read (it is a directory).
    pub fn unreadable_cmdline(&self, pid: u32) {
        let f = self.proc_root.join(pid.to_string()).join("cmdline");
        fs::remove_file(&f).unwrap();
        fs::create_dir(&f).unwrap();
    }

    /// `<config>/sessions/<pid>.json` as Claude Code writes it (`procStart`
    /// as a string).
    pub fn sessions_file(&self, config: &Path, pid: u32, conv: &str, start: u64, cwd: &Path) {
        let d = config.join("sessions");
        fs::create_dir_all(&d).unwrap();
        chmod(&d, 0o700);
        let body = serde_json::json!({
            "pid": pid, "sessionId": conv, "cwd": cwd, "startedAt": 1_793_300_000_000_i64,
            "procStart": start.to_string(), "kind": "interactive", "entrypoint": "cli"
        });
        fs::write(d.join(format!("{pid}.json")), body.to_string()).unwrap();
    }
}
