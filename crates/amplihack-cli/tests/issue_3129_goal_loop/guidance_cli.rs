//! Issue #3129 (D4, D5, D-1): `amplihack recipe guidance show|wait`.
//!
//! Runs the real binary through `support::recipe_guide::GuideFixture`
//! (private HOME, state and work root; no real login is touched).
//!
//! Contract:
//! - `show [<TARGET>] [--pr N --pr-repo O/R] [--since ENTRY] [--json]`
//!   prints every entry oldest first, marks entries after `--since` as `NEW`;
//!   `--json` is schema 1 and never carries guidance text (D-1).
//! - `wait [--pr N --pr-repo O/R] --since ENTRY [-- COMMAND...]` arms a FIFO,
//!   reads again, runs the command (its stdout goes to stderr) and prints one
//!   JSON line: `{"event":"guidance"}` or `{"event":"command","exit":N}`.
//!   Exit 0 for either, 1 error, 2 usage, 3 unarmed. It never passes 75 or 79
//!   through as its own exit status.
//!
//! No sleep, timer or timeout. A test that has to hold a command live opens a
//! FIFO the command opens, which returns only once the command is running.

#![cfg(target_os = "linux")]

use crate::support::recipe_guide::{GuideFixture, PR_URL, parse_appended, show, stdout};
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::Stdio;

const PR: [&str; 4] = ["--pr", "7", "--pr-repo", "o/r"];

fn guide(fx: &GuideFixture, text: &str) -> String {
    let out = fx.guide_text(&[PR_URL], text);
    assert!(out.status.success(), "{}", show(&out));
    let (path, _, _) = parse_appended(&out);
    path.file_name().unwrap().to_str().unwrap().to_owned()
}

fn run(fx: &GuideFixture, args: &[&str]) -> std::process::Output {
    let mut full = vec!["recipe", "guidance"];
    full.extend_from_slice(args);
    fx.run_with_stdin(fx.amplihack(&full), None)
}

fn mkfifo(path: &Path) {
    let c = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

fn json_line(out: &std::process::Output) -> serde_json::Value {
    let text = stdout(out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line: {}", show(out));
    serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("{e}: {}", show(out)))
}

#[test]
fn show_marks_only_entries_after_since_as_new() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "first note ZQ3129A");
    let second = guide(&fx, "second note ZQ3129B");
    let mut args = vec!["show"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first]);
    let out = run(&fx, &args);
    assert!(out.status.success(), "{}", show(&out));
    let text = stdout(&out);
    assert!(
        text.contains("ZQ3129A") && text.contains("ZQ3129B"),
        "{text}"
    );
    let new_lines: Vec<&str> = text.lines().filter(|l| l.contains("NEW")).collect();
    assert!(
        new_lines.iter().any(|l| l.contains(&second)),
        "the later entry is marked NEW: {text}"
    );
    assert!(
        !new_lines.iter().any(|l| l.contains(&first)),
        "the entry at --since is not new: {text}"
    );
}

#[test]
fn show_json_has_metadata_and_never_the_text() {
    let fx = GuideFixture::new();
    let name = guide(&fx, "secret-ish ZQ3129TEXT");
    let mut args = vec!["show", "--json"];
    args.extend_from_slice(&PR);
    let out = run(&fx, &args);
    assert!(out.status.success(), "{}", show(&out));
    assert!(
        !stdout(&out).contains("ZQ3129TEXT"),
        "D-1: no guidance text in JSON"
    );
    let v: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(v["schema_version"], 1, "{v}");
    let dump = v.to_string();
    assert!(dump.contains(&name), "{dump}");
    assert!(!dump.contains("\"text\""), "{dump}");
}

#[test]
fn show_with_a_bad_since_or_pr_is_a_usage_error_with_empty_stdout() {
    let fx = GuideFixture::new();
    guide(&fx, "x");
    for args in [
        vec![
            "show",
            "--pr",
            "7",
            "--pr-repo",
            "o/r",
            "--since",
            "../etc/passwd",
        ],
        vec!["show", "--pr", "0", "--pr-repo", "o/r"],
        vec!["show", "--pr", "x7", "--pr-repo", "o/r"],
    ] {
        let out = run(&fx, &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", show(&out));
        assert!(out.stdout.is_empty(), "{args:?}: {}", show(&out));
    }
}

#[test]
fn wait_without_since_is_a_usage_error() {
    let fx = GuideFixture::new();
    guide(&fx, "x");
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    let out = run(&fx, &args);
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert!(out.stdout.is_empty(), "{}", show(&out));
}

#[test]
fn wait_returns_at_once_when_an_entry_already_exists_and_never_starts_the_command() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    guide(&fx, "two");
    let marker = fx.root.join("started");
    let script = format!("touch {}", marker.display());
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", &script]);
    let out = run(&fx, &args);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(json_line(&out)["event"], "guidance");
    assert!(
        !marker.exists(),
        "an entry after --since ends the wait before the command"
    );
}

#[test]
fn wait_reports_the_command_finishing_first_with_its_exit() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", "exit 3"]);
    let out = run(&fx, &args);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = json_line(&out);
    assert_eq!(v["event"], "command", "{v}");
    assert_eq!(v["exit"], 3, "{v}");
}

#[test]
fn wait_never_passes_exit_79_through() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", "exit 79"]);
    let out = run(&fx, &args);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(json_line(&out)["exit"], 79);
}

#[test]
fn wait_never_passes_exit_75_through() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", "exit 75"]);
    let out = run(&fx, &args);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(json_line(&out)["exit"], 75);
}

#[test]
fn wait_command_stdout_goes_to_stderr_so_the_json_line_is_alone() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", "echo CHATTER3129"]);
    let out = run(&fx, &args);
    assert!(!stdout(&out).contains("CHATTER3129"), "{}", show(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("CHATTER3129"));
    json_line(&out);
}

#[test]
fn wait_for_an_unresolvable_target_is_unarmed_exit_3_and_the_command_is_not_started() {
    let fx = GuideFixture::new();
    let marker = fx.root.join("started-unarmed");
    let script = format!("touch {}", marker.display());
    let out = run(
        &fx,
        &[
            "wait",
            "--pr",
            "99999",
            "--pr-repo",
            "o/never",
            "--since",
            "20200101T000000.000000000Z-1-0.md",
            "--",
            "sh",
            "-c",
            &script,
        ],
    );
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    assert!(
        !marker.exists(),
        "unarmed means the caller falls back; nothing ran"
    );
}

#[test]
fn wait_stops_and_reaps_the_command_when_guidance_arrives_during_it() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    let ready = fx.root.join("ready.fifo");
    let block = fx.root.join("block.fifo");
    let pidfile = fx.root.join("cmd.pid");
    mkfifo(&ready);
    mkfifo(&block);
    // The command records its PID, tells the test it is live by opening the
    // ready FIFO, then blocks on a FIFO nobody writes.
    let script = format!(
        "echo $$ > {pid}; : > {ready}; exec 3<> {block}; read x <&3",
        pid = pidfile.display(),
        ready = ready.display(),
        block = block.display()
    );
    let mut args = vec!["recipe", "guidance", "wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", &script]);
    let mut cmd = fx.amplihack(&args);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let child = cmd.spawn().expect("start wait");
    // Rendezvous: opening for read returns once the command opens for write.
    let _live = fs::OpenOptions::new().read(true).open(&ready).unwrap();
    guide(&fx, "stop now");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(json_line(&out)["event"], "guidance");
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only checks that the PID exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "the command ({pid}) was stopped and reaped");
}

#[test]
fn the_spawned_command_holds_no_wake_fifo_descriptor() {
    let fx = GuideFixture::new();
    let first = guide(&fx, "one");
    let out_file = fx.root.join("fds.txt");
    let script = format!("ls -l /proc/self/fd > {}", out_file.display());
    let mut args = vec!["wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", &first, "--", "sh", "-c", &script]);
    let out = run(&fx, &args);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let fds = fs::read_to_string(&out_file).unwrap();
    assert!(
        !fds.contains("guidance-wake"),
        "O_CLOEXEC on every wake descriptor:\n{fds}"
    );
    let _ = fs::metadata(&out_file).map(|m| m.file_type().is_fifo());
}

#[test]
fn recipe_guide_still_succeeds_when_a_waiter_folder_holds_a_stale_fifo() {
    let fx = GuideFixture::new();
    guide(&fx, "make the folder");
    let wake = fx.pr_parent().join("guidance-wake");
    fs::create_dir_all(&wake).unwrap();
    mkfifo(&wake.join("424242-1.fifo")); // no reader
    let out = fx.guide_text(&[PR_URL], "second");
    assert!(
        out.status.success(),
        "a failed wake never fails the append: {}",
        show(&out)
    );
    assert!(
        !wake.join("424242-1.fifo").exists(),
        "the stale FIFO was cleaned up"
    );
}
