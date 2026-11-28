//! Unit tests for `run_guidance::wait` (issue #3129, D4): arming, stopping
//! the command, and a whole wait.
//!
//! Nothing here sleeps or sets a timer. A test that needs a command to be
//! live has the command open a FIFO the test opens too: the test's open
//! returns only once the command is running, so the waiter is armed by then.

use super::env::DirCheck;
use super::folders::{Folder, Folders, PrFolder};
use super::wait::{WaitOutcome, arm, stop, wait_armed};
use super::wake::WAKE_DIR;
use super::watch::{ChildWatch, wait_readable};
use super::*;

use std::ffi::{CString, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn private_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, root)
}

fn mkfifo(path: &Path) {
    let c = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

fn folder(path: PathBuf, check: DirCheck) -> Folder {
    Folder { path, check }
}

fn pr_folders(state_root: &Path) -> Folders {
    let key = "https___github.com_o_r_7";
    let path = state_root.join(AUTO_DRIVE_DIR).join(key).join(GUIDANCE_DIR);
    Folders {
        list: vec![folder(path.clone(), DirCheck::Missing)],
        pr: Some(PrFolder {
            state_root: state_root.to_path_buf(),
            key: key.to_string(),
            path,
        }),
    }
}

/// A run folder `<root>/run` whose `guidance/` will hold entries.
fn run_folders(root: &Path) -> Folders {
    let run = root.join("run");
    fs::create_dir(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
    Folders {
        list: vec![folder(run.join(GUIDANCE_DIR), DirCheck::Missing)],
        pr: None,
    }
}

fn sh(script: String) -> Vec<OsString> {
    ["sh".into(), "-c".into(), script.into()].to_vec()
}

fn name_of(appended: &Appended) -> String {
    appended
        .path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn arming_for_a_pr_never_creates_the_state_root() {
    let (_t, root) = private_root();
    let state = root.join("state");
    let (waiters, problems) = arm(&pr_folders(&state));
    assert!(waiters.is_empty());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("does not exist"), "{problems:?}");
    assert!(!state.exists(), "the state root was not created");
}

#[test]
fn arming_for_a_pr_creates_its_state_folder_under_an_existing_root() {
    let (_t, root) = private_root();
    let folders = pr_folders(&root);
    let (waiters, problems) = arm(&folders);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(waiters.len(), 1);
    let guidance = &folders.list[0].path;
    let parent = guidance.parent().unwrap();
    assert_eq!(waiters[0].path().parent().unwrap(), parent.join(WAKE_DIR));
    let mode = fs::metadata(parent).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert!(!guidance.exists(), "arming creates no guidance folder");
}

#[test]
fn one_waiter_per_parent_and_none_beside_a_path_that_is_not_a_guidance_folder() {
    let (_t, root) = private_root();
    let folders = Folders {
        list: vec![
            folder(root.join(GUIDANCE_DIR), DirCheck::Missing),
            folder(root.join(GUIDANCE_DIR), DirCheck::Missing),
            folder(
                PathBuf::from("relative/guidance"),
                DirCheck::Untrusted("it is not an absolute path".to_string()),
            ),
            folder(
                root.join("other"),
                DirCheck::Untrusted("its last component is not `guidance`".to_string()),
            ),
        ],
        pr: None,
    };
    let (waiters, problems) = arm(&folders);
    assert_eq!(waiters.len(), 1, "{problems:?}");
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn stop_reaps_a_command_that_already_exited_without_signalling_it() {
    let mut child = Command::new("sh")
        .args(["-c", "exit 0"])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let watch = ChildWatch::start(child.id()).unwrap();
    // The watch reports the exit without reaping: the PID is still ours.
    assert_eq!(wait_readable(&[watch.as_raw_fd()]).unwrap(), [true]);
    stop(&mut child).unwrap();
    assert_eq!(child.try_wait().unwrap().unwrap().code(), Some(0));
}

#[test]
fn stop_ends_a_running_command_with_sigterm_and_reaps_it() {
    let (_t, root) = private_root();
    let block = root.join("block.fifo");
    mkfifo(&block);
    // Blocks reading a FIFO nobody writes, until SIGTERM ends it.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("exec 3<> {}; read x <&3", block.display()))
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    stop(&mut child).unwrap();
    let status = child.try_wait().unwrap().expect("reaped");
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

#[test]
fn an_entry_already_after_since_ends_the_wait_before_the_command_starts() {
    let (_t, root) = private_root();
    let mut folders = run_folders(&root);
    let dir = folders.list[0].path.clone();
    let first = name_of(&append_entry(&dir, "one", "alice").unwrap());
    let second = name_of(&append_entry(&dir, "two", "alice").unwrap());
    let (mut waiters, _) = arm(&folders);
    let marker = root.join("started");
    let command = sh(format!("touch {}", marker.display()));
    let outcome = wait_armed(&mut folders, &mut waiters, &first, &command).unwrap();
    assert_eq!(outcome, WaitOutcome::Guidance(second));
    assert!(!marker.exists(), "the command never started");
}

#[test]
fn the_command_finishing_first_is_reported_with_its_status() {
    let (_t, root) = private_root();
    let mut folders = run_folders(&root);
    let (mut waiters, _) = arm(&folders);
    let outcome = wait_armed(&mut folders, &mut waiters, "", &sh("exit 75".into())).unwrap();
    let WaitOutcome::Command(status) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(status.code(), Some(75));
}

#[test]
fn guidance_during_the_command_stops_it() {
    let (_t, root) = private_root();
    let mut folders = run_folders(&root);
    let dir = folders.list[0].path.clone();
    let first = name_of(&append_entry(&dir, "one", "alice").unwrap());
    let (mut waiters, _) = arm(&folders);
    let ready = root.join("ready.fifo");
    mkfifo(&ready);
    // The command says it is live by opening `ready`, then blocks for good:
    // the wait can only return by stopping it.
    let command = sh(format!(
        ": > {r}; exec 3<> {r}; read x <&3",
        r = ready.display()
    ));
    let sender = std::thread::spawn(move || {
        // Returns once the command has opened `ready`: it is running.
        drop(fs::File::open(&ready).unwrap());
        name_of(&append_entry(&dir, "stop now", "alice").unwrap())
    });
    let outcome = wait_armed(&mut folders, &mut waiters, &first, &command).unwrap();
    assert_eq!(outcome, WaitOutcome::Guidance(sender.join().unwrap()));
}

#[test]
fn a_spurious_wake_byte_does_not_end_the_wait() {
    let (_t, root) = private_root();
    let mut folders = run_folders(&root);
    let (mut waiters, _) = arm(&folders);
    let fifo = waiters[0].path().to_path_buf();
    let (ready, release) = (root.join("ready.fifo"), root.join("release.fifo"));
    mkfifo(&ready);
    mkfifo(&release);
    // Live once `ready` is open; exits 5 once the test writes `release`.
    let command = sh(format!(
        ": > {}; read x < {}; exit 5",
        ready.display(),
        release.display()
    ));
    let sender = std::thread::spawn(move || {
        drop(fs::File::open(&ready).unwrap());
        // A wake byte with no new entry behind it.
        let mut wake = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        std::io::Write::write_all(&mut wake, b"x").unwrap();
        fs::write(&release, "go\n").unwrap();
    });
    let outcome = wait_armed(&mut folders, &mut waiters, "", &command).unwrap();
    sender.join().unwrap();
    let WaitOutcome::Command(status) = outcome else {
        panic!("a spurious byte ended the wait: {outcome:?}");
    };
    assert_eq!(status.code(), Some(5));
}

#[test]
fn a_wait_with_no_command_ends_when_an_entry_is_appended() {
    let (_t, root) = private_root();
    let mut folders = run_folders(&root);
    let dir = folders.list[0].path.clone();
    let (mut waiters, _) = arm(&folders);
    // Appended after arming, from another thread: the wake byte, or the
    // read after arming, ends the wait; either way the entry is the answer.
    let sender = std::thread::spawn(move || name_of(&append_entry(&dir, "go", "bob").unwrap()));
    let outcome = wait_armed(&mut folders, &mut waiters, "", &[]).unwrap();
    assert_eq!(outcome, WaitOutcome::Guidance(sender.join().unwrap()));
}
