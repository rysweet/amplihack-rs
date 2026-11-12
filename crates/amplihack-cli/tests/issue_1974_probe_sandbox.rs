//! Issue #1974: a usage probe runs `claude` under a real `bwrap` with an
//! empty `tmpfs` over the shared `projects` folder.
//!
//! Runs the real `amplihack` binary and the host's real `bwrap` with a fake
//! `claude`. The profiles get their `projects` link from `profile add`, as on
//! a real host. While the probe is running, the fake lists the folder, then
//! waits on a FIFO while a writer outside the sandbox, standing in for a
//! worker session under another login, writes a transcript through its own
//! profile's link. The fake then lists the folder again and writes a file of
//! its own. The test checks that:
//!
//! - the probe saw an empty folder both times;
//! - the host folder kept its file, gained the worker's and not the probe's;
//! - no fallback note was printed, so the wrapped path really ran.
//!
//! Skipped, with a line on stderr, where `bwrap --dev-bind / / true` fails
//! (no `bwrap`, or user namespaces refused), since there is then no sandbox
//! to test. The unit tests in `tests_probe_sandbox.rs` cover the arguments
//! and the fallback on every host.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `--version` answers; `-p /usage` runs the handshake described above and
/// prints a canned reading. Paths come from the environment only.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
printf '%s\n' "$@" > "$RECORD_DIR/args"
p="$CLAUDE_CONFIG_DIR/projects"
{ echo "start:"; ls -A "$p"; } > "$RECORD_DIR/ready"
read _ < "$RECORD_DIR/go"
{ echo "after worker:"; ls -A "$p"; } > "$RECORD_DIR/listing"
echo probe > "$p/probe-wrote.jsonl"
echo 'You are currently using your subscription to power your Claude Code usage'
echo
echo 'Current session: 7% used'
echo 'Current week (all models): 9% used'
exit 0
"#;

const NOTE_PREFIX: &str = "note: usage probes run claude with --no-session-persistence only: ";

fn bwrap_usable() -> bool {
    Command::new("bwrap")
        .args(["--dev-bind", "/", "/", "--", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo {}", path.display());
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    records: PathBuf,
    tools: PathBuf,
    work: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = home.join("profiles");
        let records = tmp.path().join("records");
        let tools = tmp.path().join("tools");
        let work = tmp.path().join("work");
        for d in [&home.join(".claude/projects"), &records, &tools, &work] {
            fs::create_dir_all(d).unwrap();
        }
        crate::support::exec_stub::write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
        Self {
            _tmp: tmp,
            home,
            root,
            records,
            tools,
            work,
        }
    }

    fn shared(&self) -> PathBuf {
        self.home.join(".claude/projects")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env(
                crate::isolated_amplihack::PROC_ROOT_ENV,
                crate::isolated_amplihack::shared_empty_proc_root(),
            )
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env(
                "CODEX_PROFILES_ROOT",
                self.home.with_file_name("no-codex-profiles"),
            )
            .env("CODEX_BIN", self.home.join("no-codex"))
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "text")
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    fn add(&self, name: &str) {
        let out = self.cmd(&["profile", "add", name]).output().unwrap();
        assert!(
            out.status.success(),
            "profile add {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let link = self.root.join(name).join("projects");
        assert!(
            fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()),
            "profile add links {name}/projects to the shared folder"
        );
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(self.shared()).unwrap()
        );
    }
}

/// Lists the names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn a_probe_under_real_bwrap_sees_an_empty_projects_folder_and_a_worker_still_writes() {
    if !bwrap_usable() {
        eprintln!("skipped: `bwrap --dev-bind / / true` fails on this host");
        return;
    }
    let f = Fixture::new();
    f.add("a");
    f.add("b");
    fs::write(f.shared().join("old.jsonl"), "kept\n").unwrap();
    fs::set_permissions(f.shared(), fs::Permissions::from_mode(0o700)).unwrap();
    let ready = f.records.join("ready");
    let go = f.records.join("go");
    mkfifo(&ready);
    mkfifo(&go);

    let child = f
        .cmd(&["profile", "usage", "--json", "a"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The handshake runs on its own thread so that a probe which never
    // reaches the fake cannot leave this test blocked on a FIFO: once the
    // child has exited, the main thread opens `ready` itself to release it.
    let worker_link = f.root.join("b").join("projects").join("worker.jsonl");
    let handshake = {
        let (ready, go) = (ready.clone(), go.clone());
        std::thread::spawn(move || -> Option<String> {
            let mut seen = String::new();
            fs::File::open(&ready)
                .unwrap()
                .read_to_string(&mut seen)
                .unwrap();
            if !seen.starts_with("start:") {
                return None;
            }
            // A worker under login `b`, outside the sandbox, writes its
            // transcript through `b/projects` while the probe is running.
            fs::write(&worker_link, "worker\n").unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&go)
                .unwrap()
                .write_all(b"go\n")
                .unwrap();
            Some(seen)
        })
    };
    let out = child.wait_with_output().unwrap();
    if !handshake.is_finished() {
        // A read-write open of a FIFO never blocks on Linux and releases a
        // thread blocked opening the other end; dropping it ends the read.
        for fifo in [&ready, &go] {
            drop(
                fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(fifo)
                    .unwrap(),
            );
        }
    }
    let seen = handshake.join().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        seen.as_deref(),
        Some("start:\n"),
        "the probe reached the fake and saw an empty projects folder; stderr: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(f.records.join("listing")).unwrap(),
        "after worker:\n",
        "the worker's write is not visible inside the probe"
    );
    assert!(
        !stderr.contains(NOTE_PREFIX),
        "the wrapped path ran, so no fallback note: {stderr}"
    );
    let args = fs::read_to_string(f.records.join("args")).unwrap();
    assert_eq!(args, "-p\n/usage\n--no-session-persistence\n");

    assert_eq!(
        names(&f.shared()),
        ["old.jsonl", "worker.jsonl"],
        "the host folder kept its file, gained the worker's, and not the probe's"
    );
    assert_eq!(
        fs::read_to_string(f.shared().join("old.jsonl")).unwrap(),
        "kept\n"
    );
    assert_eq!(
        fs::read_to_string(f.shared().join("worker.jsonl")).unwrap(),
        "worker\n"
    );
    let mode = fs::metadata(f.shared()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "the host folder's mode is unchanged");

    let rows: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(rows[0]["profile"], "a");
    assert_eq!(rows[0]["session"]["used_percentage"], 7.0, "{stdout}");
}
