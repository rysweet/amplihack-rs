//! #2053: blocking storage and live artefact contracts. All coordination is
//! through child messages and completion; no clock or retry loop is involved.
use crate::{AtomicCounter, AtomicJsonFile, FileLock};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const EVENT: &str = "STOP_STORAGE_EVENT ";

/// A blocking writer must never enter the existing timed F_SETLK backend.
/// Observing the executed branch avoids a timing assertion or a source scan.
pub(crate) fn reject_timed_acquisition() {
    assert!(
        !matches!(
            std::env::var("STOP_STORAGE_FIXTURE").as_deref(),
            Ok("observe" | "contender")
        ),
        "Stop counters must use blocking acquisition, never the timed retry backend"
    );
}

// Called only in this crate's unit-test executable, immediately after creating
// the temporary file. The production library has neither this function nor its
// call sites. Only an explicitly selected subprocess fixture can pause here.
pub(crate) fn observe_temp_file(target: &Path, temporary: &Path) {
    if std::env::var_os("STOP_STORAGE_FIXTURE").as_deref() != Some(std::ffi::OsStr::new("observe"))
        || std::env::var_os("STOP_STORAGE_TARGET").as_deref() != Some(target.as_os_str())
    {
        return;
    }
    event(json!({"target":target, "temporary":temporary}));
    let mut release = String::new();
    std::io::stdin().read_line(&mut release).unwrap();
    assert_eq!(
        release, "release\n",
        "parent must explicitly release writer"
    );
}

fn event(value: Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{EVENT}{value}").unwrap();
    stdout.flush().unwrap();
}

fn next_event(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "fixture exited before readiness"
        );
        // libtest writes its own prefix when --nocapture is used.
        if let Some((_, payload)) = line.split_once(EVENT) {
            return serde_json::from_str(payload.trim()).unwrap();
        }
    }
}

fn child(root: &Path, mode: &str, target: &Path) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear();
    command
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("AMPLIHACK_HOME", root.join("state"))
        .env("AMPLIHACK_ROOT", root.join("framework"))
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", root.join("profiles"))
        .env("CLAUDE_CONFIG_DIR", root.join("profiles/claude"))
        .env("CODEX_HOME", root.join("profiles/codex"))
        .env("STOP_STORAGE_FIXTURE", mode)
        .env("STOP_STORAGE_TARGET", target)
        .args([
            "--exact",
            "stop_counter_tests::subprocess_fixture",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().unwrap()
}

fn finish(child: Child) {
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "fixture failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Probe from a different process: POSIX record locks belong to the process,
/// so a thread in the writer would falsely report that the lock was available.
fn companion_is_locked(target: &Path) -> bool {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(target.with_extension("json.lock"))
        .unwrap();
    let mut lock = libc::flock {
        l_type: libc::F_WRLCK as libc::c_short,
        l_whence: libc::SEEK_SET as i16,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &mut lock) };
    if result == 0 {
        false
    } else {
        let error = std::io::Error::last_os_error();
        assert!(
            matches!(error.raw_os_error(), Some(code) if code == libc::EAGAIN || code == libc::EACCES),
            "{error}"
        );
        true
    }
}

#[test]
#[ignore = "Rust subprocess fixture; invoked explicitly by the parent tests"]
fn subprocess_fixture() {
    let target = std::path::PathBuf::from(std::env::var_os("STOP_STORAGE_TARGET").unwrap());
    match std::env::var("STOP_STORAGE_FIXTURE").unwrap().as_str() {
        "observe" => {
            let counter = AtomicCounter::new(&target).with_blocking_lock();
            assert_eq!(counter.increment().unwrap(), 1);
        }
        "probe" => event(json!({"locked":companion_is_locked(&target)})),
        "contender" => {
            // Establish that a real owner exists before attempting the update.
            assert!(companion_is_locked(&target));
            event(json!({"owner_observed":true}));
            assert_eq!(
                AtomicCounter::new(&target)
                    .with_blocking_lock()
                    .increment()
                    .unwrap(),
                2
            );
        }
        other => panic!("unknown fixture mode: {other}"),
    }
}

#[test]
fn blocking_file_lock_releases_on_drop() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let target = root.path().join("counter.txt");
    let lock = FileLock::exclusive_blocking(&target.with_extension("json.lock")).unwrap();
    let mut probe = child(root.path(), "probe", &target);
    assert_eq!(
        next_event(&mut BufReader::new(probe.stdout.as_mut().unwrap())),
        json!({"locked":true})
    );
    finish(probe);
    drop(lock);
    let mut probe = child(root.path(), "probe", &target);
    assert_eq!(
        next_event(&mut BufReader::new(probe.stdout.as_mut().unwrap())),
        json!({"locked":false})
    );
    finish(probe);
}

#[test]
fn blocking_file_lock_returns_open_errors_without_redirecting_storage() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let directory = root.path().join("directory.lock");
    fs::create_dir(&directory).unwrap();
    assert!(matches!(
        FileLock::exclusive_blocking(&directory),
        Err(crate::file_lock::FileLockError::Open { .. })
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn blocking_json_write_and_update_preserve_atomic_replacement() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let path = root.path().join("nested/state.json");
    let file = AtomicJsonFile::new(&path).with_blocking_lock();
    file.write(&json!({"count":2})).unwrap();
    let updated: Value = file
        .update(|value: &mut Value| value["count"] = json!(3))
        .unwrap();
    assert_eq!(updated, json!({"count":3}));
    assert_eq!(file.read::<Value>().unwrap(), Some(updated));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
}

#[test]
fn blocking_json_serialisation_failure_preserves_original_and_cleans_temporary() {
    struct CannotSerialise;
    impl serde::Serialize for CannotSerialise {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "deliberate serialisation failure",
            ))
        }
    }
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let path = root.path().join("state.json");
    fs::write(&path, r#"{"value":7}"#).unwrap();
    assert!(
        AtomicJsonFile::new(&path)
            .with_blocking_lock()
            .write(&CannotSerialise)
            .is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"value":7}"#);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    let mut probe = child(root.path(), "probe", &path);
    assert_eq!(
        next_event(&mut BufReader::new(probe.stdout.as_mut().unwrap())),
        json!({"locked":false})
    );
    finish(probe);
}

#[test]
fn blocking_json_persistence_failure_cleans_temporary_without_fallback() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let path = root.path().join("state.json");
    fs::create_dir(&path).unwrap();
    assert!(
        AtomicJsonFile::new(&path)
            .with_blocking_lock()
            .write(&json!({"value":1}))
            .is_err()
    );
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn blocking_counter_preserves_formats_reset_and_parse_failure() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let path = root.path().join("counter.txt");
    let counter = AtomicCounter::new(&path).with_blocking_lock();
    assert_eq!(counter.get().unwrap(), 0);
    fs::write(&path, "5").unwrap();
    assert_eq!(counter.increment().unwrap(), 6);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        json!({"value":6})
    );
    counter.reset().unwrap();
    assert_eq!(counter.get().unwrap(), 0);
    fs::write(&path, "malformed").unwrap();
    assert!(counter.increment().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "malformed");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn live_stop_counter_locks_and_temporary_files_stay_beneath_the_session() {
    for name in ["locks/lock_invocations.txt", "power-steering/session_count"] {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        let session =
            amplihack_types::session_state_dir_under(&root.path().join("state"), Some("lane 3"));
        let target = session.join(name);
        let mut writer = child(root.path(), "observe", &target);
        let ready = next_event(&mut BufReader::new(writer.stdout.as_mut().unwrap()));
        assert_eq!(ready["target"], json!(target));
        let temporary = std::path::PathBuf::from(ready["temporary"].as_str().unwrap());
        assert!(temporary.starts_with(&session));
        assert_eq!(temporary.parent(), target.parent());
        assert!(
            temporary.is_file(),
            "observe the temporary file before persistence"
        );
        assert!(
            !target.exists(),
            "the first counter has not yet been persisted"
        );
        assert!(target.with_extension("json.lock").is_file());
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 2);
        let mut probe = child(root.path(), "probe", &target);
        assert_eq!(
            next_event(&mut BufReader::new(probe.stdout.as_mut().unwrap())),
            json!({"locked":true})
        );
        finish(probe);
        writeln!(writer.stdin.take().unwrap(), "release").unwrap();
        finish(writer);
        assert!(!temporary.exists());
        assert_eq!(AtomicCounter::new(&target).get().unwrap(), 1);
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 2);
    }
}

#[test]
fn blocking_counter_serialises_a_writer_behind_a_live_companion_lock() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let target = root
        .path()
        .join("state/runtime/sessions/shared/locks/lock_invocations.txt");
    let mut owner = child(root.path(), "observe", &target);
    let ready = next_event(&mut BufReader::new(owner.stdout.as_mut().unwrap()));
    assert!(Path::new(ready["temporary"].as_str().unwrap()).exists());
    let mut contender = child(root.path(), "contender", &target);
    assert_eq!(
        next_event(&mut BufReader::new(contender.stdout.as_mut().unwrap())),
        json!({"owner_observed":true})
    );
    writeln!(owner.stdin.take().unwrap(), "release").unwrap();
    finish(owner);
    finish(contender);
    assert_eq!(AtomicCounter::new(&target).get().unwrap(), 2);
}
