//! Synchronized interruption tests: no network and no timing-based readiness.
use super::fixture::{PublishFixture, describe, executable, root};
use std::fs;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn wait_for_file(f: &PublishFixture, child: &mut Child, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.tmp.path().join(name).exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before {name}"
        );
        assert!(Instant::now() < deadline, "child never reached {name}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn interrupt(f: &PublishFixture, mut command: Command, signal: &str) -> Output {
    let stdout = fs::File::create(f.tmp.path().join("cancel.stdout")).unwrap();
    let stderr = fs::File::create(f.tmp.path().join("cancel.stderr")).unwrap();
    let mut child = command.stdout(stdout).stderr(stderr).spawn().unwrap();
    wait_for_file(f, &mut child, "ready");
    assert!(
        Command::new("kill")
            .args(["-s", signal, &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "cancellation did not terminate: {}",
                f.read("cancel.stderr")
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        fs::read_dir(f.tmp.path().join("private")).unwrap().count(),
        0,
        "owned captures must be removed (status {status}): {}",
        f.read("cancel.stderr")
    );
    let output = Output {
        status,
        stdout: fs::read(f.tmp.path().join("cancel.stdout")).unwrap(),
        stderr: fs::read(f.tmp.path().join("cancel.stderr")).unwrap(),
    };
    assert!(!describe(&output).contains("ghp_CANCEL_SECRET_ABCDEFGHIJKLMNOP"));
    let permissions = f.read("cancel.permissions");
    assert!(!permissions.is_empty());
    assert!(
        permissions.lines().all(|l| l.starts_with("-rw-------")),
        "{permissions}"
    );
    // The blocked executable and its blocking child must both have been reaped.
    for pid in f.read("owned.pids").lines() {
        assert!(
            !Command::new("kill")
                .args(["-0", pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success(),
            "owned process {pid} survived"
        );
    }
    output
}

const BLOCK: &str = r#"
printf '%s\n' "$$" >> "$CASE_ROOT/owned.pids"
if [ "${BLOCK_BODY:-0}" = 1 ]; then
  printf 'ghp_CANCEL_SECRET_ABCDEFGHIJKLMNOP\n'
else
  printf 'ghp_CANCEL_SECRET_ABCDEFGHIJKLMNOP\n' >&2
fi
for file in "$TMPDIR"/* "$TMPDIR"/.[!.]*; do
  [ -f "$file" ] || continue
  ls -l "$file" >> "$CASE_ROOT/cancel.permissions"
done
# exec retains the recorded pid and leaves no unowned grandchildren.
: > "$CASE_ROOT/ready"
exec /bin/sleep 300
"#;

#[test]
fn retry_cancellation_cleans_captures_and_preserves_returning_caller_traps() {
    for signal in ["TERM", "HUP", "INT"] {
        let f = PublishFixture::new();
        executable(&f.bin.join("gh"), &format!("#!/bin/bash\n{BLOCK}"));
        let mut command = f.command("bash");
        command
            .args([
                "-c",
                r#"
. "$1/amplifier-bundle/tools/workflow_gh_retry.sh"
trap 'printf caller-term > "$CASE_ROOT/caller-signal"' TERM
trap 'printf caller-hup > "$CASE_ROOT/caller-signal"' HUP
trap 'printf caller-int > "$CASE_ROOT/caller-signal"' INT
trap 'printf "%s" "$?" > "$CASE_ROOT/caller-exit"' EXIT
before=$(trap -p)
_gh_retry_core 'pr create' pr create
status=$?
after=$(trap -p)
[ "$before" = "$after" ] || exit 99
printf '%s' "$status" > "$CASE_ROOT/retry-status"
exit "$status"
"#,
                "cancel-test",
            ])
            .arg(root());
        let output = interrupt(&f, command, signal);
        let expected = match signal {
            "TERM" => 143,
            "HUP" => 129,
            _ => 130,
        };
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            describe(&output)
        );
        assert_eq!(f.read("retry-status"), expected.to_string());
        assert_eq!(f.read("caller-exit"), expected.to_string());
        assert_eq!(
            f.read("caller-signal"),
            format!("caller-{}", signal.to_lowercase())
        );
    }
}

#[test]
fn publishing_cancellation_cleans_creation_body_and_nested_retry_captures() {
    let f = PublishFixture::new();
    let gh = fs::read_to_string(f.bin.join("gh")).unwrap();
    executable(
        &f.bin.join("gh"),
        &gh.replace("  'pr create')", &format!("  'pr create')\n{BLOCK}")),
    );
    let output = interrupt(&f, f.publish_command(), "TERM");
    assert_eq!(output.status.code(), Some(143), "{}", describe(&output));
}

#[test]
fn body_writer_cancellation_cleans_scratch_and_preserves_sourcing_caller() {
    let f = PublishFixture::new();
    f.shim(
        "cat",
        &format!("ps -o ppid= -p \"$PPID\" > \"$CASE_ROOT/writer-owner.pid\"\n{BLOCK}"),
    );
    let mut command = f.command("bash");
    command.env("BLOCK_BODY", "1");
    command
        .args([
            "-c",
            r#"
. "$1/amplifier-bundle/tools/workflow_body_file.sh"
trap 'printf caller > "$CASE_ROOT/caller-signal"' TERM
trap 'printf "%s" "$?" > "$CASE_ROOT/caller-exit"' EXIT
before=$(trap -p)
write_body_file "$CASE_ROOT/result.md" < /dev/null &
writer=$!
printf '%s' "$writer" > "$CASE_ROOT/writer.pid"
wait "$writer"
status=$?
[ "$before" = "$(trap -p)" ] || exit 99
exit "$status"
"#,
            "body-cancel",
        ])
        .arg(root());
    // The parent is intentionally kept alive: interrupt the actual writer,
    // then wait for its caller to verify the returned status and caller traps.
    let stdout = fs::File::create(f.tmp.path().join("cancel.stdout")).unwrap();
    let stderr = fs::File::create(f.tmp.path().join("cancel.stderr")).unwrap();
    let mut child = command.stdout(stdout).stderr(stderr).spawn().unwrap();
    wait_for_file(&f, &mut child, "ready");
    let owner = f.read("writer-owner.pid");
    let pid = owner.trim();
    assert!(
        Command::new("kill")
            .args(["-TERM", pid])
            .status()
            .unwrap()
            .success(),
        "owner={owner:?}, stderr={}",
        f.read("cancel.stderr")
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "body cancellation hung");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(143));
    assert_eq!(f.read("caller-exit"), "143");
    assert!(!f.tmp.path().join("caller-signal").exists());
    assert_eq!(
        fs::read_dir(f.tmp.path().join("private")).unwrap().count(),
        0
    );
    assert!(!f.tmp.path().join("result.md").exists());
    assert!(
        !f.read("cancel.stderr")
            .contains("ghp_CANCEL_SECRET_ABCDEFGHIJKLMNOP")
    );
    assert!(
        f.read("cancel.permissions")
            .lines()
            .all(|l| l.starts_with("-rw-------"))
    );
}

#[test]
fn publishing_cancellation_during_body_writing_cleans_every_nested_owner() {
    let f = PublishFixture::new();
    f.shim(
        "cat",
        &format!(
            r#"
for file in "$TMPDIR"/workflow-body-raw.*; do
  if [ -f "$file" ]; then
    {BLOCK}
  fi
done
exec "$REAL_TOOL" "$@"
"#
        ),
    );
    let mut command = f.publish_command();
    command.env("BLOCK_BODY", "1");
    let output = interrupt(&f, command, "TERM");
    assert_eq!(output.status.code(), Some(143), "{}", describe(&output));
    assert!(!f.read("gh.log").contains("pr create "));
}

#[test]
fn retry_default_cancellation_preserves_exit_trap_and_signal_status() {
    let f = PublishFixture::new();
    executable(&f.bin.join("gh"), &format!("#!/bin/bash\n{BLOCK}"));
    let mut command = f.command("bash");
    command
        .args([
            "-c",
            r#"
. "$1/amplifier-bundle/tools/workflow_gh_retry.sh"
trap 'printf "%s" "$?" > "$CASE_ROOT/caller-exit"' EXIT
_gh_retry_core 'pr create' pr create
"#,
            "cancel-default",
        ])
        .arg(root());
    let output = interrupt(&f, command, "TERM");
    assert_eq!(output.status.code(), Some(143), "{}", describe(&output));
    assert_eq!(f.read("caller-exit"), "143");
}

#[test]
fn direct_body_writer_cancellation_cleans_private_scratch() {
    let f = PublishFixture::new();
    f.shim("cat", BLOCK);
    let mut command = f.command("bash");
    command
        .env("BLOCK_BODY", "1")
        .arg(root().join("amplifier-bundle/tools/workflow_body_file.sh"))
        .arg(f.tmp.path().join("result.md"));
    let output = interrupt(&f, command, "TERM");
    assert_eq!(output.status.code(), Some(143), "{}", describe(&output));
    assert!(!f.tmp.path().join("result.md").exists());
}
