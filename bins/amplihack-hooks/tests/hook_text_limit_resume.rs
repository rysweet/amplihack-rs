//! AC5: `amplihack-hooks limit-resume` (the StopFailure hook) is outside
//! AMPLIHACK_HOOK_TEXT altogether. Under `none` it still gives its exit-2 wake
//! with the same stderr, so a local-model session stopped at a limit resumes.
//!
//! Each case runs the hook with the variable unset first (the control), then
//! with `none` and with an unknown value, each in a fresh [`HookSandbox`], and
//! compares exit code, stdout, stderr and the outcome the hook logged. The
//! only normalisation is the leading UTC timestamp of each `resume.log` line.
//!
//! The harness is a trimmed copy of the one in `limit_resume_binary.rs`
//! (stand-in rotator, ledger, swap record written after the hook has read its
//! snapshot), kept separate so that this change adds no lines to a file the
//! limit-resume work (#3172) is also changing.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

#[cfg(target_os = "linux")]
use amplihack_hooks::limit_resume::RESUME_MSG;
#[cfg(target_os = "linux")]
use amplihack_utils::file_event::{Waiter, Watch, Woke};
use isolated_hook::HookSandbox;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const VAR: &str = "AMPLIHACK_HOOK_TEXT";
const CHILD_LIMIT: Duration = Duration::from_secs(60);
const SETTINGS: [Option<&str>; 4] = [None, Some("none"), Some("NONE"), Some("bogus-value")];

struct Env {
    sandbox: HookSandbox,
    root: PathBuf,
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn env() -> Env {
    let sandbox = HookSandbox::new();
    fs::set_permissions(sandbox.home(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = sandbox.profiles();
    private_dir(&root);
    Env { sandbox, root }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(target_os = "linux")]
fn write_ledger(root: &Path) {
    let dir = root.join(".auto-switch.d/rotate");
    private_dir(&root.join(".auto-switch.d"));
    private_dir(&dir);
    let ledger = json!({
        "version": 1,
        "live": "spare",
        "last_cycle": {"at": now() + 120, "outcome": "swapped"}
    });
    private_file(&dir.join("ledger.json"), ledger.to_string().as_bytes());
}

/// A process whose command line names `profile rotate watch`, recorded in
/// `rotate.pid`. Killed on drop. See `StandInRotator` in
/// `limit_resume_binary.rs` for why it waits for its ready line.
#[cfg(target_os = "linux")]
struct StandInRotator(Child);

#[cfg(target_os = "linux")]
impl StandInRotator {
    fn start(root: &Path) -> Self {
        let mut child = std::process::Command::new("sh")
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
        .unwrap();
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

#[cfg(target_os = "linux")]
impl Drop for StandInRotator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn limit_payload(session: &str) -> String {
    json!({
        "hook_event_name": "StopFailure",
        "session_id": session,
        "transcript_path": "/tmp/t.jsonl",
        "error": "rate_limit",
        "error_details": "429",
        "last_assistant_message": "You've hit your weekly limit · resets 9pm"
    })
    .to_string()
}

/// Starts the hook with `AMPLIHACK_CLAUDE_AUTO_SWITCH=1` and, when `setting`
/// is `Some`, the hook-text variable. PATH is an empty private directory, so
/// no real tmux is reachable and a resume takes the exit-2 path.
fn spawn(e: &Env, setting: Option<&str>) -> (Child, Instant) {
    let empty_path = e.sandbox.root().join("empty-path");
    private_dir(&empty_path);
    let mut cmd = e.sandbox.command(env!("CARGO_BIN_EXE_amplihack-hooks"));
    cmd.arg("limit-resume")
        .env("PATH", &empty_path)
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &e.root)
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(value) = setting {
        cmd.env(VAR, value);
    }
    let start = Instant::now();
    let mut child = cmd.spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(limit_payload("sess-1").as_bytes());
    }
    (child, start)
}

fn finish(mut child: Child, start: Instant) -> Output {
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > CHILD_LIMIT {
            let _ = child.kill();
            panic!("limit-resume did not exit within {CHILD_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

/// The outcome fields of each `resume.log` line, without the timestamp.
fn logged(e: &Env) -> Vec<String> {
    fs::read_to_string(e.root.join(".auto-switch.d/resume/resume.log"))
        .unwrap_or_default()
        .lines()
        .map(|line| {
            line.split_once(' ')
                .map_or(line, |(_, rest)| rest)
                .to_string()
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct Seen {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    log: Vec<String>,
}

/// No rotator runs, so the hook decides at once (`rotator-down`).
#[test]
fn limit_resume_ignores_the_setting_when_no_rotator_runs() {
    let mut control = None;
    for setting in SETTINGS {
        let e = env();
        let (child, start) = spawn(&e, setting);
        let out = finish(child, start);
        let seen = Seen {
            code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
            log: logged(&e),
        };
        assert!(
            !String::from_utf8_lossy(&seen.stderr).contains(VAR),
            "limit-resume never reads {VAR}: {setting:?}"
        );
        match &control {
            None => {
                assert!(
                    seen.log.iter().any(|l| l.contains("outcome=rotator-down")),
                    "fixture check: {:?}",
                    seen.log
                );
                control = Some(seen);
            }
            Some(control) => assert_eq!(&seen, control, "{setting:?}"),
        }
    }
}

/// The wake itself: exit 2 with the resume line, under every value.
#[cfg(target_os = "linux")]
#[test]
fn limit_resume_wakes_the_session_under_none() {
    const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
    let mut control = None;
    for setting in SETTINGS {
        let e = env();
        let _rotator = StandInRotator::start(&e.root);
        write_ledger(&e.root);
        let rotate = e.root.join(".auto-switch.d/rotate");
        private_file(&rotate.join("swap.json"), b"{}");
        let mut watch = Waiter::new(
            &[Watch {
                dir: rotate.clone(),
                name: Some("swap.json"),
                mask: IN_CLOSE_NOWRITE,
            }],
            std::process::id(),
        )
        .unwrap();
        let (child, start) = spawn(&e, setting);
        assert_eq!(
            watch.wait(start + CHILD_LIMIT).unwrap(),
            Woke::File(0),
            "the hook never read swap.json ({setting:?})"
        );
        let at = now() + 120;
        let record = json!({
            "seq": 1,
            "live_login": "spare",
            "from_login": "work",
            "swapped_at": at,
            "reading": {"session": 10, "week": 20, "measured_at": at}
        });
        let tmp = rotate.join(".swap.json.tmp");
        private_file(&tmp, record.to_string().as_bytes());
        fs::rename(&tmp, rotate.join("swap.json")).unwrap();
        let out = finish(child, start);
        let seen = Seen {
            code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
            log: logged(&e),
        };
        match &control {
            None => {
                assert_eq!(seen.code, Some(2), "fixture check: the control wakes");
                assert_eq!(seen.stderr, format!("{RESUME_MSG}\n").into_bytes());
                assert!(seen.stdout.is_empty());
                control = Some(seen);
            }
            Some(control) => assert_eq!(&seen, control, "{setting:?}"),
        }
    }
}
