//! Issue #1760: registry safety (R2.1, R2.2, R5.1, S20, C1, C9).
//!
//! - A `~/.amplihack` other users can write to, or a symlink, is refused.
//! - A corrupt, too-new, too-large or unsafe registry is never overwritten,
//!   and every command that needs it exits 1 with the documented text.
//! - A second supervisor is refused while the first holds `supervisor.lock`.
//! - `supervisor.log` is opened without following symlinks and must have one
//!   link; a failed append is a warning and changes no other file.
//! - A `registry.lock` symlink makes `register` exit 1.

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::{CONV_A, Fx, chmod, expect_code, stderr, stdout};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

fn lost_without_id(fx: &Fx) {
    let e = fx.entry("coord", &fx.dir("d"), None);
    fx.write_agents(&[("coordinator", e)]);
}

#[test]
fn an_amplihack_dir_other_users_can_write_is_refused() {
    let fx = Fx::new();
    let top = fx.home.join(".amplihack");
    fs::create_dir_all(&top).unwrap();
    chmod(&top, 0o777);
    let o = fx.register("a", "s", &fx.dir("d"), CONV_A, &[]);
    expect_code(&o, 1);
    let err = stderr(&o);
    assert!(err.contains(".amplihack: writable by other users"), "{err}");
    assert!(!fx.agents_dir().exists(), "nothing created under it");
    let o = fx.run(&["agent", "status"]);
    expect_code(&o, 1);
    assert!(stdout(&o).is_empty(), "{}", stdout(&o));
}

#[test]
fn a_symlinked_amplihack_dir_is_refused() {
    let fx = Fx::new();
    let real = fx.root.join("elsewhere");
    fs::create_dir_all(&real).unwrap();
    chmod(&real, 0o700);
    std::os::unix::fs::symlink(&real, fx.home.join(".amplihack")).unwrap();
    let o = fx.register("a", "s", &fx.dir("d"), CONV_A, &[]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains(".amplihack: is a symlink"),
        "{}",
        stderr(&o)
    );
    assert!(!real.join("agents").exists());
}

#[test]
fn a_registry_that_fails_to_load_is_never_overwritten() {
    let cases: [(&str, &str, &[&str]); 4] = [
        (
            "{\"version\":1,\"agents\":{",
            "registry.json: not valid JSON (line ",
            &["; it was not changed"],
        ),
        (
            r#"{"version":2,"agents":{"x":{"state":"paused"}}}"#,
            "registry.json: written by a newer amplihack (registry version 2); upgrade amplihack. It was not changed",
            &[],
        ),
        // C9: an unknown state at version 1 is corrupt; line and column only.
        (
            r#"{"version":1,"agents":{"x":{"state":"paused"}}}"#,
            "registry.json: does not match the registry format (line 1, column ",
            &["; it was not changed"],
        ),
        (
            r#"{"agents":{}}"#,
            "registry.json: ",
            &["; it was not changed"],
        ),
    ];
    for (raw, message, more) in cases {
        let fx = Fx::new();
        let dir = fx.dir("d").display().to_string();
        fx.write_registry_raw(raw);
        for args in [
            vec!["agent", "register", "a", "--session", "s", "--window", "w"],
            vec!["agent", "unregister", "x"],
            vec!["agent", "supervise", "--once"],
            vec!["agent", "status", "--json"],
        ] {
            let mut args = args;
            if args[1] == "register" {
                args.extend(["--dir", &dir, "--conversation", CONV_A]);
            }
            let o = fx.run(&args);
            expect_code(&o, 1);
            let err = stderr(&o);
            assert!(err.contains(message), "{args:?} on {raw}:\n{err}");
            for m in more {
                assert!(err.contains(m), "{args:?} on {raw}:\n{err}");
            }
            assert!(!err.contains("paused"), "file contents quoted: {err}");
            assert!(stdout(&o).is_empty(), "{args:?}: {}", stdout(&o));
            assert_eq!(fs::read_to_string(fx.registry_path()).unwrap(), raw);
        }
    }
}

/// Review issue 7: a registry over 8 MiB is named as too large, not as a
/// permissions problem, and is left as it is.
#[test]
fn a_registry_over_8_mib_is_named_as_too_large() {
    let fx = Fx::new();
    fx.write_registry_raw(&" ".repeat(8 * 1024 * 1024 + 1));
    let o = fx.run(&["agent", "status"]);
    expect_code(&o, 1);
    let want = "registry.json: larger than 8 MiB; it was not changed";
    assert!(stderr(&o).contains(want), "{}", stderr(&o));
    let len = fs::metadata(fx.registry_path()).unwrap().len();
    assert_eq!(len, 8 * 1024 * 1024 + 1);
}

#[test]
fn an_invalid_field_is_named_without_its_value() {
    let fx = Fx::new();
    let mut e = fx.entry("coord", &fx.dir("d"), Some(CONV_A));
    e["conversation_id"] = serde_json::json!("--print");
    fx.write_agents(&[("coordinator", e)]);
    let raw = fx.registry_raw();
    let o = fx.supervise_once(&[]);
    expect_code(&o, 1);
    let err = stderr(&o);
    assert!(
        err.contains("agent coordinator: field conversation_id is not valid; it was not changed"),
        "{err}"
    );
    assert!(!err.contains("--print"), "value quoted: {err}");
    assert_eq!(fx.registry_raw(), raw);
    assert!(fx.new_sessions().is_empty());
}

#[test]
fn a_registry_readable_by_others_is_refused() {
    let fx = Fx::new();
    expect_code(&fx.register("a", "s", &fx.dir("d"), CONV_A, &[]), 0);
    chmod(&fx.registry_path(), 0o644);
    let raw = fx.registry_raw();
    let o = fx.run(&["agent", "status"]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains(
            "registry.json: must be a regular file owned by you with mode 0600; it was not changed"
        ),
        "{}",
        stderr(&o)
    );
    expect_code(&fx.register("b", "t", &fx.dir("e"), CONV_A, &[]), 1);
    assert_eq!(fx.registry_raw(), raw);
}

#[test]
fn a_second_supervisor_is_refused_while_the_lock_is_held() {
    let fx = Fx::new();
    lost_without_id(&fx);
    let path = fx.agents_dir().join("supervisor.lock");
    for (content, expected) in [
        (
            "424242\n",
            "another amplihack agent supervisor is running (pid 424242)",
        ),
        (
            "not a pid\n",
            "another amplihack agent supervisor is running (pid unknown)",
        ),
    ] {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        f.write_all(content.as_bytes()).unwrap();
        // SAFETY: flock on a descriptor this test owns; released when `f` drops.
        // Retried briefly: a child another test thread forks while the previous
        // iteration's descriptor is open shares that lock until it execs.
        let take = || unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut rc = take();
        while rc != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            rc = take();
        }
        assert_eq!(rc, 0, "the test takes the lock first");
        let o = fx.supervise_once(&["--gap", "0"]);
        expect_code(&o, 1);
        assert!(stderr(&o).contains(expected), "{}", stderr(&o));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            content,
            "lock file untouched"
        );
        assert!(fx.log_lines().is_empty(), "no pass ran");
        drop(f);
    }
    // Once released, the next supervisor takes it and writes its own pid.
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let pid = fs::read_to_string(&path).unwrap();
    assert!(pid.trim().parse::<u32>().is_ok(), "{pid:?}");
}

#[test]
fn a_symlinked_log_is_not_followed() {
    let fx = Fx::new();
    lost_without_id(&fx);
    let target = fx.root.join("precious");
    fs::write(&target, "precious\n").unwrap();
    chmod(&target, 0o600);
    std::os::unix::fs::symlink(&target, fx.log_path()).unwrap();
    let o = fx.supervise_once(&["--gap", "0"]);
    expect_code(&o, 0);
    assert!(
        stdout(&o).contains("agent=coordinator conversation=- "),
        "the line still goes to stdout: {}",
        stdout(&o)
    );
    let err = stderr(&o);
    assert!(err.contains("warning: "), "{err}");
    assert!(err.contains("supervisor.log: is a symlink"), "{err}");
    assert!(err.contains("the line was printed but not logged"), "{err}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "precious\n");
}

#[test]
fn a_hard_linked_log_is_refused() {
    let fx = Fx::new();
    lost_without_id(&fx);
    let other = fx.root.join("other-file");
    fs::write(&other, "keep\n").unwrap();
    chmod(&other, 0o600);
    fs::hard_link(&other, fx.log_path()).unwrap();
    let o = fx.supervise_once(&["--gap", "0"]);
    expect_code(&o, 0);
    let err = stderr(&o);
    assert!(
        err.contains("supervisor.log: has more than one hard link"),
        "{err}"
    );
    assert!(err.contains("the line was printed but not logged"), "{err}");
    assert_eq!(fs::read_to_string(&other).unwrap(), "keep\n");
}

#[test]
fn a_symlinked_registry_lock_makes_register_exit_1() {
    let fx = Fx::new();
    fx.private_agents_dir();
    let target = fx.root.join("lock-target");
    fs::write(&target, "").unwrap();
    chmod(&target, 0o600);
    std::os::unix::fs::symlink(&target, fx.agents_dir().join("registry.lock")).unwrap();
    let o = fx.register("a", "s", &fx.dir("d"), CONV_A, &[]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains("registry.lock: is a symlink"),
        "{}",
        stderr(&o)
    );
    assert!(!fx.registry_path().exists(), "nothing written");
    assert_eq!(fs::read_to_string(&target).unwrap(), "");
}
