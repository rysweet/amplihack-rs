//! Issue #1973: the `profile rotate` fixture under host load.
//!
//! - Every command the fixture builds raises both identity knobs to the
//!   wait ceiling, so a slow host cannot use up the 30 s identity budget.
//! - The loopback identity endpoint answers each connection on its own
//!   thread: a client that is slow, silent or closes mid-request ends only
//!   its own connection, never the endpoint, and dropping the fixture never
//!   panics.
//! - The fake `claude` can be held inside a `/usage` probe, after it has
//!   recorded its environment, until the test releases it; a drop guard
//!   releases it if the test panics.

#![cfg(unix)]

use crate::support::profile_rotate::Fixture;
use crate::support::wait::{CEILING, wait_child, wait_until};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};

const BUDGET_ENV: &str = "AMPLIHACK_TEST_IDENTITY_BUDGET_SECS";
const READ_TIMEOUT_ENV: &str = "AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS";

fn env_of<'a>(cmd: &'a Command, name: &str) -> Option<&'a OsStr> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(name))
        .and_then(|(_, v)| v)
}

#[test]
fn every_fixture_command_raises_both_identity_knobs_to_the_ceiling() {
    let f = Fixture::new();
    let ceiling = CEILING.as_secs().to_string();
    for cmd in [
        f.cmd(&["profile", "rotate", "--dry-run"]),
        f.cmd_no_root_env(&["profile", "rotate", "--dry-run"]),
    ] {
        for name in [BUDGET_ENV, READ_TIMEOUT_ENV] {
            assert_eq!(
                env_of(&cmd, name),
                Some(OsStr::new(&ceiling)),
                "{name} must be set to {ceiling} on every fixture command"
            );
        }
    }
}

fn host_port(base: &str) -> &str {
    base.strip_prefix("http://")
        .expect("the endpoint is plain http")
}

/// One request; the endpoint closes the connection after its answer.
fn request(base: &str, extra_header: &str) -> String {
    let mut s = TcpStream::connect(host_port(base)).expect("connect to the identity endpoint");
    s.set_read_timeout(Some(CEILING)).unwrap();
    write!(
        s,
        "GET /api/oauth/profile HTTP/1.1\r\nHost: x\r\n{extra_header}\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    s.read_to_string(&mut answer)
        .expect("the endpoint answers within the ceiling");
    answer
}

#[test]
fn the_identity_endpoint_listens_on_loopback_only() {
    let f = Fixture::new();
    assert!(
        f.identity_base().starts_with("http://127.0.0.1:"),
        "{}",
        f.identity_base()
    );
}

#[test]
fn a_silent_client_does_not_stop_the_identity_endpoint() {
    let f = Fixture::new();
    let base = f.identity_base().to_string();
    // Connected, sends nothing, stays open for the rest of the test.
    let _silent = TcpStream::connect(host_port(&base)).unwrap();
    // Sends half a request line and closes.
    {
        let mut half = TcpStream::connect(host_port(&base)).unwrap();
        half.write_all(b"GET /api/oauth/pro").unwrap();
    }
    // Sends the request line and part of a header, then closes.
    {
        let mut partial = TcpStream::connect(host_port(&base)).unwrap();
        partial
            .write_all(b"GET /api/oauth/profile HTTP/1.1\r\nHost:")
            .unwrap();
    }
    // A complete request without a fake token is still answered.
    let answer = request(&base, "");
    assert!(
        answer.starts_with("HTTP/1.1 401 "),
        "a request without a fake token gets 401: {answer:?}"
    );
    // And again, so the endpoint did not end after one more connection.
    let answer = request(&base, "");
    assert!(answer.starts_with("HTTP/1.1 401 "), "{answer:?}");
    // Dropping the fixture with the silent client still open must not
    // panic or hang: the acceptor stops and is joined.
    drop(f);
}

#[test]
fn the_identity_endpoint_never_echoes_request_data() {
    let f = Fixture::new();
    let answer = request(f.identity_base(), "X-Probe: ECHO-MARK-1973\r\n");
    assert!(!answer.contains("ECHO-MARK-1973"), "{answer:?}");
}

/// The fake `claude` as a `/usage` probe, run directly with the fixture's
/// directories and nothing inherited.
fn probe(f: &Fixture) -> Command {
    let mut c = Command::new(f.tools.join("claude"));
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &f.home)
        .env("RECORD_DIR", &f.records)
        .env("CANNED", &f.canned)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c
}

fn env_records(f: &Fixture) -> usize {
    std::fs::read_dir(&f.records)
        .map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("env."))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_held_probe_records_its_environment_and_waits_for_release() {
    let f = Fixture::new();
    let before = env_records(&f);
    let hold = f.hold_probes();
    let mut child = probe(&f).spawn().unwrap();
    wait_until("the held probe records its environment", || {
        env_records(&f) > before
    });
    // A check that something does not happen: the hold has no end of its
    // own, so load can only make this miss a fault, never fail wrongly.
    assert!(
        child.try_wait().unwrap().is_none(),
        "a held probe must not finish before release"
    );
    f.release_probes();
    let status = wait_child("the released probe exits", &mut child);
    assert!(status.success(), "{status}");
    drop(hold);
}

#[test]
fn the_hold_guard_releases_probes_when_the_test_panics() {
    let f = Fixture::new();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _hold = f.hold_probes();
        panic!("a test failing while probes are held");
    }));
    assert!(panicked.is_err());
    let mut child = probe(&f).spawn().unwrap();
    let status = wait_child("a probe after the guard dropped exits", &mut child);
    assert!(status.success(), "{status}");
}

#[test]
fn dropping_the_hold_guard_releases_probes() {
    let f = Fixture::new();
    drop(f.hold_probes());
    let mut child = probe(&f).spawn().unwrap();
    let status = wait_child("a probe after the guard dropped exits", &mut child);
    assert!(status.success(), "{status}");
}
