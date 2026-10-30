//! Shared fixture for the issue #1746 (`sessions archive` / `restore`)
//! integration tests, used as `crate::support::sessions_archive` by the
//! `issue_1746_*` modules.
//!
//! Every run gets its own temporary `HOME`, profile root and `--dest`, all
//! under one canonical temp directory and asserted before each command, so
//! nothing touches the real `~/.claude`, `~/.amplihack` or profile root.
//! Session files hold [`BODY_MARKER`], which must never be printed.

#![cfg(unix)]
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

pub use super::amplihack_bin::amplihack_bin;

/// Written into every session file; no output may contain it.
pub const BODY_MARKER: &str = "SESSION-BODY-MARKER-1746";
/// Set in the environment of a process the `/proc` source reads; no output
/// may contain it.
pub const ENVIRON_MARKER: &str = "ENVIRON-MARKER-1746";
pub const PROJECT: &str = "-home-user-src-demo";
pub const DAY: u64 = 86_400;

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().mode() & 0o7777
}

pub fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// Mode bits mean nothing to root; tests that rely on them skip there.
pub fn running_as_root() -> bool {
    euid() == 0
}

/// Sets the modification time of `p` (file or directory, not followed if a
/// symlink is not involved) to `days_ago` days before now.
pub fn age(p: &Path, days_ago: u64) {
    let t = SystemTime::now() - Duration::from_secs(days_ago * DAY);
    let f = fs::File::open(p).unwrap();
    f.set_times(fs::FileTimes::new().set_modified(t).set_accessed(t))
        .unwrap();
}

/// A pid that belonged to a process that has exited and been reaped.
pub fn dead_pid() -> i64 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    i64::from(pid)
}

/// No session contents, environ marker, or error detail text is printed.
pub fn assert_clean(out: &Output) {
    for s in [text(&out.stdout), text(&out.stderr)] {
        for bad in [BODY_MARKER, ENVIRON_MARKER, "os error", "Os {", "SENTINEL"] {
            assert!(!s.contains(bad), "{bad:?} printed: {s}");
        }
    }
}

pub struct Fx {
    pub tmp: tempfile::TempDir,
    /// Canonical temp root; every other path is below it.
    pub base: PathBuf,
    pub home: PathBuf,
    pub profile_root: PathBuf,
    pub dest: PathBuf,
    pub cwd: PathBuf,
}

impl Fx {
    /// `HOME/.claude/projects` exists and is empty; the profile root exists
    /// with no profiles; DEST does not exist.
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        chmod(&base, 0o700);
        let home = base.join("home");
        let profile_root = base.join("profiles");
        let cwd = base.join("cwd");
        for d in [&home, &profile_root, &cwd] {
            fs::create_dir_all(d).unwrap();
            chmod(d, 0o700);
        }
        fs::create_dir_all(home.join(".claude/projects")).unwrap();
        chmod(&home.join(".claude"), 0o700);
        Self {
            dest: base.join("archive-dest"),
            tmp,
            base,
            home,
            profile_root,
            cwd,
        }
    }

    pub fn projects(&self) -> PathBuf {
        self.home.join(".claude/projects")
    }

    pub fn project_dir(&self) -> PathBuf {
        self.projects().join(PROJECT)
    }

    pub fn manifest(&self) -> PathBuf {
        self.dest.join("MANIFEST.jsonl")
    }

    /// Writes session `id` in `PROJECT`: `<id>.jsonl` plus, for each
    /// `(relative path, body)` in `parts`, a file under `<id>/`. Every part,
    /// directories included, is aged `days_ago` days.
    pub fn session(&self, id: &str, days_ago: u64, parts: &[&str]) -> PathBuf {
        self.session_in(PROJECT, id, days_ago, parts)
    }

    pub fn session_in(&self, project: &str, id: &str, days_ago: u64, parts: &[&str]) -> PathBuf {
        let pdir = self.projects().join(project);
        fs::create_dir_all(&pdir).unwrap();
        let jsonl = pdir.join(format!("{id}.jsonl"));
        fs::write(&jsonl, format!("{{\"m\":\"{BODY_MARKER} {id}\"}}\n")).unwrap();
        let mut dirs = Vec::new();
        for rel in parts {
            let f = pdir.join(id).join(rel);
            let mut d = f.parent().unwrap().to_path_buf();
            fs::create_dir_all(&d).unwrap();
            while d != pdir {
                dirs.push(d.clone());
                d = d.parent().unwrap().to_path_buf();
            }
            fs::write(&f, format!("{BODY_MARKER} {rel}\n")).unwrap();
            age(&f, days_ago);
        }
        age(&jsonl, days_ago);
        dirs.sort();
        dirs.dedup();
        // Deepest first, so ageing a child does not refresh a parent later.
        for d in dirs.iter().rev() {
            age(d, days_ago);
        }
        jsonl
    }

    /// Writes `<dir>/sessions/<stem>.json` holding `body`.
    pub fn pid_file(&self, config_dir: &Path, stem: &str, body: &str) -> PathBuf {
        let sdir = config_dir.join("sessions");
        fs::create_dir_all(&sdir).unwrap();
        let p = sdir.join(format!("{stem}.json"));
        fs::write(&p, body).unwrap();
        p
    }

    /// A profile directory (`<root>/<name>`), as `profile add` leaves it
    /// for listing purposes.
    pub fn profile(&self, name: &str) -> PathBuf {
        let p = self.profile_root.join(name);
        fs::create_dir_all(&p).unwrap();
        chmod(&p, 0o700);
        p
    }

    pub fn assert_isolated(&self) {
        for p in [&self.home, &self.profile_root, &self.dest, &self.cwd] {
            assert!(p.starts_with(&self.base), "{} outside temp", p.display());
        }
        if let Some(real) = std::env::var_os("HOME").map(PathBuf::from) {
            assert_ne!(self.home, real, "never the real HOME");
        }
    }

    /// `amplihack <args>` with this fixture's HOME and profile root, and no
    /// `AMPLIHACK_SESSIONS_*` variable unless the caller sets one.
    pub fn cmd(&self, args: &[&str]) -> Command {
        self.assert_isolated();
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.cwd)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profile_root)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0");
        c
    }

    pub fn run_cmd(&self, mut c: Command) -> Output {
        let out = c.output().unwrap();
        assert_clean(&out);
        out
    }

    /// `amplihack sessions archive <extra> --dest <DEST>`.
    pub fn archive(&self, extra: &[&str]) -> Output {
        let dest = self.dest.to_str().unwrap().to_string();
        let mut args = vec!["sessions", "archive"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["--dest", &dest]);
        self.run_cmd(self.cmd(&args))
    }

    pub fn apply(&self) -> Output {
        self.archive(&["--apply"])
    }

    /// `amplihack sessions restore <id> --dest <DEST>`.
    pub fn restore(&self, id: &str) -> Output {
        let dest = self.dest.to_str().unwrap().to_string();
        self.run_cmd(self.cmd(&["sessions", "restore", id, "--dest", &dest]))
    }

    /// Every manifest line, parsed.
    pub fn manifest_lines(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.manifest())
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// Member paths of a `.tar.gz`, in archive order.
    pub fn members(&self, archive: &Path) -> Vec<String> {
        let f = fs::File::open(archive).unwrap();
        let mut a = tar::Archive::new(flate2::read::GzDecoder::new(f));
        a.entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let p = e.path().unwrap().to_string_lossy().into_owned();
                p.trim_end_matches('/').to_string()
            })
            .collect()
    }
}

pub fn ok(out: &Output) {
    assert!(
        out.status.success(),
        "failed: {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    );
}

pub fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("exited normally")
}

/// stdout is exactly `line` plus a newline.
pub fn assert_stdout(out: &Output, line: &str) {
    assert_eq!(
        text(&out.stdout),
        format!("{line}\n"),
        "stderr: {}",
        text(&out.stderr)
    );
}

/// stderr has a line exactly equal to `line`.
pub fn assert_stderr_line(out: &Output, line: &str) {
    let err = text(&out.stderr);
    assert!(
        err.lines().any(|l| l == line),
        "no stderr line {line:?} in:\n{err}"
    );
}
