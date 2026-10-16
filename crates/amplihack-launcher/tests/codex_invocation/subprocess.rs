//! Subprocess stdin and real terminal inheritance scenarios.
use super::*;

pub(super) fn remote_exec_subprocess_receives_complete_stdin() {
    use std::{fs, io::Write, os::unix::fs::PermissionsExt, process::Stdio};
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("codex");
    let captured = dir.path().join("input");
    fs::write(
        &binary,
        "#!/bin/sh\n/bin/cat > \"$CAPTURE\"\nprintf '%s\\n' \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let prompt = format!("SYSTEM\nPERSONA\n{}\nTASK-END", "日本語\n".repeat(30_000));
    let mut delivered = build(
        &[
            "--no-daemon",
            "--remote=resume",
            "--remote-auth-token-env",
            "exec",
            "e",
        ],
        &prompt,
    )
    .unwrap();
    delivered
        .command
        .env("PATH", dir.path())
        .env("CAPTURE", &captured)
        .stdout(Stdio::piped());
    let mut child = delivered.command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(delivered.stdin_payload.as_ref().unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(fs::read(captured).unwrap(), prompt.as_bytes());
    assert_eq!(
        output.stdout,
        b"--no-daemon\n--remote=resume\n--remote-auth-token-env\nexec\ne\n-\n"
    );
}

pub(super) fn interactive_resume_subprocess_inherits_real_terminal_stdin() {
    use std::{
        fs,
        os::{fd::FromRawFd, unix::fs::PermissionsExt},
        process::{Command, Stdio},
    };
    const FIXTURE: &str = "AMPLIHACK_CODEX_TERMINAL_TEST_FIXTURE";
    if let Some(dir) = std::env::var_os(FIXTURE) {
        for args in [
            vec!["resume", "--last", "--include-non-interactive"],
            vec!["--remote", "exec", "--no-daemon"],
        ] {
            let mut d = build(&args, "continue λ").unwrap();
            assert!(d.stdin_payload.is_none());
            let output = d
                .command
                .env("PATH", &dir)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
                .wait_with_output()
                .unwrap();
            assert!(
                output.status.success(),
                "terminal stdin was replaced: {output:?}"
            );
            let mut expected = args.join("\n");
            expected.push_str("\n--\ncontinue λ\n");
            assert_eq!(output.stdout, expected.as_bytes());
        }
        println!("CODEX_TERMINAL_FIXTURE_COMPLETE");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("codex");
    fs::write(
        &binary,
        "#!/bin/sh\n[ -t 0 ] || exit 73\nprintf '%s\\n' \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let _master = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "interactive_resume_subprocess_inherits_real_terminal_stdin",
            "--nocapture",
        ])
        .env(FIXTURE, dir.path())
        .stdin(Stdio::from(slave))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let transcript = String::from_utf8_lossy(&output.stdout);
    assert!(transcript.contains("running 1 test"), "{transcript}");
    assert!(
        transcript.contains("CODEX_TERMINAL_FIXTURE_COMPLETE"),
        "{transcript}"
    );
    assert!(transcript.contains("1 passed; 0 failed"), "{transcript}");
}
