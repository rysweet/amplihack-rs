//! Shared rig for the real-binary `limit-resume` tests (#3172): a temporary
//! profile root, a stand-in rotator, a stand-in `tmux` found only through
//! `PATH`, and a launcher that gives the hook the process layout of a Claude
//! Code session on a pty.
//!
//! Nothing here reaches a real tmux server, pane, terminal or login. The
//! stand-in `tmux` is a shell script made of built-ins only (its `PATH` is
//! its own directory), it records each call, answers `display-message` with
//! the launcher's own pty path and `capture-pane` with canned text, and never
//! connects to anything. No test using this rig sleeps, polls or sets a
//! timeout: the barrier below is a pair of FIFOs read with blocking reads,
//! and every child is collected with `wait`.
#![cfg(target_os = "linux")]
#![allow(dead_code)]

use super::isolated_hook::HookSandbox;
use serde_json::{Value, json};
use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

/// What a held usage-limit screen looks like to the hook's screen check.
pub const LIMIT_MENU: &str =
    "Usage limit reached · Continuing automatically at 1:50am\nWhat do you want to do?\n";
/// A screen that is none of the hook's markers.
pub const PLAIN_SCREEN: &str = "$ ls\nCargo.toml  src\n";
/// The pane id the stand-in answers for.
pub const PANE: &str = "%7";

pub fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// RFC 3339 UTC for epoch seconds (Howard Hinnant's days-to-civil).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A rotate ledger whose last cycle was recorded `offset` seconds from now.
pub fn write_ledger(root: &Path, outcome: &str, offset: i64) {
    let dir = root.join(".auto-switch.d/rotate");
    private_dir(&root.join(".auto-switch.d"));
    private_dir(&dir);
    let l = json!({
        "version": 1,
        "live": "spare",
        "last_cycle": {"at": now() + offset, "outcome": outcome}
    });
    private_file(&dir.join("ledger.json"), l.to_string().as_bytes());
}

/// A stand-in for `amplihack profile rotate watch`: a process whose command
/// line names it, recorded in `rotate.pid`. Killed on drop. Same shape as in
/// `limit_resume_binary.rs`.
pub struct StandInRotator(Child);

impl StandInRotator {
    pub fn start(root: &Path) -> Self {
        let mut child = Command::new("sh")
            .args([
                "-c",
                "echo ready; while :; do sleep 1; done",
                "amplihack",
                "profile",
                "rotate",
                "watch",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().expect("piped stdout")),
            &mut line,
        )
        .expect("read the stand-in rotator's ready line");
        assert_eq!(line, "ready\n", "the stand-in rotator did not start");
        let dir = root.join(".auto-switch.d/rotate");
        private_dir(&root.join(".auto-switch.d"));
        private_dir(&dir);
        private_file(
            &dir.join("rotate.pid"),
            format!("{}\n", child.id()).as_bytes(),
        );
        Self(child)
    }
}

impl Drop for StandInRotator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A pty pair. The master stays open here, so the slave is a live terminal
/// for as long as the `Pty` lives.
pub struct Pty {
    pub master: File,
    pub slave: File,
    pub slave_path: PathBuf,
}

impl Pty {
    pub fn open() -> Self {
        // SAFETY: plain libc calls on descriptors this function owns; each
        // result is checked.
        unsafe {
            let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
            assert!(m >= 0, "posix_openpt: {}", std::io::Error::last_os_error());
            assert_eq!(libc::grantpt(m), 0, "grantpt");
            assert_eq!(libc::unlockpt(m), 0, "unlockpt");
            let mut buf = [0 as libc::c_char; 128];
            assert_eq!(
                libc::ptsname_r(m, buf.as_mut_ptr(), buf.len()),
                0,
                "ptsname_r"
            );
            let path = CStr::from_ptr(buf.as_ptr()).to_owned();
            let s = libc::open(
                path.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            );
            assert!(s >= 0, "open slave: {}", std::io::Error::last_os_error());
            Self {
                master: File::from_raw_fd(m as RawFd),
                slave: File::from_raw_fd(s as RawFd),
                slave_path: PathBuf::from(OsStr::from_bytes(path.to_bytes())),
            }
        }
    }
}

/// What the stand-in `tmux` is told to do.
pub struct Standin {
    /// The directory that is the hook's whole `PATH`.
    pub bin: PathBuf,
    /// One line per call: `[arg][arg]...`.
    pub record: PathBuf,
    /// One word per `send-keys` call: `yes` when a guard claim existed in
    /// `resume/events` when the keys arrived, else `no`.
    pub claim_seen: PathBuf,
    screen: PathBuf,
}

/// A temporary profile root, the stand-in `tmux` and what the hook needs to
/// find a swap that resumes at once.
pub struct Rig {
    pub sandbox: HookSandbox,
    pub home: PathBuf,
    pub root: PathBuf,
    pub standin: Standin,
    /// Where the barrier's two FIFOs live, when the rig has one.
    barrier: Option<(File, File)>,
    /// Variables set on every hook command after the fixture defaults.
    env: Vec<(String, String)>,
}

impl Rig {
    /// A rig whose stand-in `tmux` shows `screen`. With `barrier`, the first
    /// `display-message` of each hook process tells the test it has arrived
    /// and then blocks until the test releases it (see [`Rig::barrier`]).
    pub fn new(screen: &str, barrier: bool) -> Self {
        let sandbox = HookSandbox::new();
        let home = sandbox.home();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let root = sandbox.profiles();
        private_dir(&root);
        let base = sandbox.root().to_path_buf();
        let bin = base.join("standin-bin");
        private_dir(&bin);
        let work = base.join("standin-work");
        private_dir(&work);
        private_dir(&work.join("done"));
        let record = work.join("record");
        let claim_seen = work.join("claim-seen");
        let screen_file = work.join("screen");
        private_file(&screen_file, screen.as_bytes());
        let events = root.join(".auto-switch.d/resume/events");

        let (arrived, release) = (work.join("arrived"), work.join("release"));
        let barrier_code = if barrier {
            for f in [&arrived, &release] {
                let c = CString::new(f.as_os_str().as_bytes()).unwrap();
                // SAFETY: valid NUL-terminated path.
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            }
            format!(
                "    if [ ! -e '{done}'/$PPID ]; then\n      : > '{done}'/$PPID\n      \
                 printf x >> '{arrived}'\n      read -r _go < '{release}'\n    fi\n",
                done = work.join("done").display(),
                arrived = arrived.display(),
                release = release.display(),
            )
        } else {
            String::new()
        };
        // Only built-ins: the script's `PATH` is its own directory.
        let script = format!(
            "#!/bin/sh\n\
             line=''; for a in \"$@\"; do line=\"$line[$a]\"; done; printf '%s\\n' \"$line\" >> '{record}'\n\
             case \"$1\" in\n\
             display-message)\n{barrier_code}    printf '%s\\n' \"$AMPLIHACK_TEST_PANE_TTY\" ;;\n\
             capture-pane)\n    while IFS= read -r l; do printf '%s\\n' \"$l\"; done < '{screen}' ;;\n\
             send-keys)\n    set -- '{events}'/*\n    if [ -e \"$1\" ]; then echo yes; else echo no; fi >> '{claim}' ;;\n\
             esac\n\
             exit 0\n",
            record = record.display(),
            screen = screen_file.display(),
            events = events.display(),
            claim = claim_seen.display(),
        );
        let path = bin.join("tmux");
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .unwrap();
        f.write_all(script.as_bytes()).unwrap();
        drop(f);

        let barrier = barrier.then(|| {
            let open = |p: &Path| OpenOptions::new().read(true).write(true).open(p).unwrap();
            (open(&arrived), open(&release))
        });
        Self {
            sandbox,
            home,
            root,
            standin: Standin {
                bin,
                record,
                claim_seen,
                screen: screen_file,
            },
            barrier,
            env: Vec::new(),
        }
    }

    pub fn resume_dir(&self) -> PathBuf {
        self.root.join(".auto-switch.d/resume")
    }

    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// The resume folder as the rotator leaves it, private and present.
    pub fn private_resume_dir(&self) -> PathBuf {
        private_dir(&self.root.join(".auto-switch.d"));
        let d = self.resume_dir();
        private_dir(&d);
        d
    }

    pub fn resume_log(&self) -> String {
        fs::read_to_string(self.resume_dir().join("resume.log")).unwrap_or_default()
    }

    /// Lines the stand-in `tmux` recorded, or none when it was never run.
    pub fn tmux_calls(&self) -> Vec<String> {
        fs::read_to_string(&self.standin.record)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn send_keys_calls(&self) -> Vec<String> {
        self.tmux_calls()
            .into_iter()
            .filter(|l| l.starts_with("[send-keys]"))
            .collect()
    }

    pub fn claim_seen(&self) -> Vec<String> {
        fs::read_to_string(&self.standin.claim_seen)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Writes the swap record the rotator would have written between the
    /// stop and the hook's start, a transcript whose last entry is the stop,
    /// and a ledger, so a hook started now resumes at once: the swap is
    /// noted at the start, after the stop, onto a login with room. Returns
    /// the transcript's path.
    pub fn swap_already_happened(&self) -> PathBuf {
        let rotate = self.root.join(".auto-switch.d/rotate");
        private_dir(&self.root.join(".auto-switch.d"));
        private_dir(&rotate);
        let swapped = now() - 60;
        let record = json!({
            "seq": 1,
            "live_login": "spare",
            "from_login": "work",
            "swapped_at": swapped,
            "reading": {"session": 10, "week": 20, "measured_at": swapped}
        });
        private_file(&rotate.join("swap.json"), record.to_string().as_bytes());
        write_ledger(&self.root, "swapped", -30);
        let transcript = self.sandbox.project().join("transcript.jsonl");
        let line = json!({"type": "assistant", "timestamp": rfc3339(now() - 120)});
        private_file(&transcript, format!("{line}\n").as_bytes());
        transcript
    }

    /// A hook command on the rig's environment: cleared, `PATH` the
    /// stand-in directory alone, `TMUX` a socket path nothing listens on,
    /// `TMUX_PANE` the stand-in's pane.
    fn base(&self, program: &str, toggle: Option<&str>) -> Command {
        let mut c = self.sandbox.command(program);
        c.env("PATH", &self.standin.bin)
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root)
            .env("TMUX", self.sandbox.root().join("no-such-tmux-socket,1,0"))
            .env("TMUX_PANE", PANE);
        if let Some(t) = toggle {
            c.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", t);
        }
        c.envs(self.env.iter().map(|(key, value)| (key, value)));
        c
    }

    /// The hook, run directly, with `stdin` as its input. For gates that
    /// end before any pane is looked at.
    pub fn run_plain(&self, toggle: Option<&str>, stdin: &[u8]) -> Output {
        let mut cmd = self.base(env!("CARGO_BIN_EXE_amplihack-hooks"), toggle);
        cmd.arg("limit-resume")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        {
            let mut sin = child.stdin.take().unwrap();
            // The hook may stop reading at its 1 MiB cap; a broken pipe is fine.
            let _ = sin.write_all(stdin);
        }
        child.wait_with_output().unwrap()
    }

    /// Starts the hook as Claude Code's own command would run it on a pty: a
    /// `/bin/sh` that is a session leader holding the pty as its terminal and
    /// its stdin, in the terminal's foreground group, runs the hook as a
    /// child with the payload as the hook's stdin. The hook's parent is
    /// that shell (it does not `exec` the hook), so the holder proof sees the
    /// layout of a live session: a process with the pane's terminal on stdin
    /// and in the foreground group.
    ///
    /// The stand-in `tmux` names this child's own pty as the pane's
    /// terminal, through `AMPLIHACK_TEST_PANE_TTY`. Each child has a pty of
    /// its own, so several children share one pane id and one session, as
    /// concurrent hook calls for one event do.
    pub fn spawn_on_pty(&self, toggle: &str, payload: &[u8], pty: &Pty) -> Child {
        let payload_file = self.sandbox.root().join(format!(
            "payload-{}",
            pty.slave_path.file_name().unwrap().to_string_lossy()
        ));
        private_file(&payload_file, payload);
        let mut cmd = self.base("/bin/sh", Some(toggle));
        cmd.arg("-c")
            // The redirection is made inside a subshell, which then execs the
            // hook: dash applies a simple command's redirection in the shell
            // itself before it forks, which would leave this shell, the
            // terminal's holder, with the payload file on stdin while the
            // hook runs. The trailing `exit` keeps a shell that would `exec`
            // its last command (bash) from replacing itself with the hook.
            .arg("( exec \"$0\" limit-resume < \"$1\" ); exit $?")
            .arg(env!("CARGO_BIN_EXE_amplihack-hooks"))
            .arg(&payload_file)
            .env("AMPLIHACK_TEST_PANE_TTY", &pty.slave_path)
            .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: only async-signal-safe calls (setsid, ioctl, tcsetpgrp,
        // getpid); no allocation.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::tcsetpgrp(0, libc::getpid()) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd.spawn().unwrap()
    }

    /// One hook run on its own pty, to the end.
    pub fn run_on_pty(&self, toggle: &str, payload: &[u8]) -> Output {
        let pty = Pty::open();
        let child = self.spawn_on_pty(toggle, payload, &pty);
        let out = child.wait_with_output().unwrap();
        drop(pty);
        out
    }

    /// With a barrier: blocks until `n` hook processes have reached their
    /// first `display-message`, then lets them all go on. A blocking read of
    /// `n` bytes and a write of `n` lines: no sleep, no poll.
    pub fn barrier(&self, n: usize) {
        let (arrived, release) = self
            .barrier
            .as_ref()
            .expect("this rig was made without a barrier");
        let mut seen = vec![0u8; n];
        let mut arrived = arrived.try_clone().unwrap();
        arrived.read_exact(&mut seen).unwrap();
        let mut release = release.try_clone().unwrap();
        for _ in 0..n {
            release.write_all(b"go\n").unwrap();
        }
    }
}

/// `payload` with `transcript_path` set, as bytes.
pub fn with_transcript(payload: &Value, transcript: &Path) -> Vec<u8> {
    let mut v = payload.clone();
    v["transcript_path"] = json!(transcript);
    v.to_string().into_bytes()
}

/// The value of ` key=` on a log line.
pub fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(&format!("{key}=")))
}

/// The `error=` value of every log line matches `^[a-z_]*$`.
pub fn assert_error_fields_reduced(log: &str) {
    for line in log.lines() {
        let e = field(line, "error").unwrap_or_else(|| panic!("no error= on {line:?}"));
        assert!(
            e.len() <= 32 && e.bytes().all(|b| b == b'_' || b.is_ascii_lowercase()),
            "error field {e:?} on {line:?}"
        );
    }
}
