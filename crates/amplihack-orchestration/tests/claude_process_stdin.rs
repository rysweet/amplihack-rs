//! Issue #1570 (design decision D13): `TokioProcessRunner::run` delivers a
//! large prompt to a `claude` delegate on stdin. On argv, a 200 KiB prompt
//! would be one argv element and the spawn would fail with `E2BIG`.
//!
//! A stub `claude` on `PATH` records its argv and stdin. This binary mutates
//! the process-global `PATH`, `AMPLIHACK_DELEGATE` and
//! `AMPLIHACK_PROMPT_DELIVERY`, so it must stay a separate test binary with
//! a single test.
//!
//! Run with:
//!     cargo test -p amplihack-orchestration --test claude_process_stdin

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use amplihack_orchestration::claude_process::{ProcessRunner, RunOptions, TokioProcessRunner};
use amplihack_utils::root_sandbox::{self, SkipPermissionsEnv};

/// Linux `MAX_ARG_STRLEN`.
const MAX_ARG_STRLEN: usize = 131_072;

fn write_stub(bin_dir: &Path, record_dir: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         : > '{rec}/argv'\n\
         for a in \"$@\"; do printf '%s\\000' \"$a\" >> '{rec}/argv'; done\n\
         cat > '{rec}/stdin'\n\
         printf 'stub-claude-ok'\n",
        rec = record_dir.display()
    );
    let path = bin_dir.join("claude");
    std::fs::write(&path, script).expect("write stub claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod stub claude");
}

#[tokio::test]
async fn claude_delegate_receives_large_prompt_on_stdin_byte_exact() {
    // Root outside a sandbox is refused before spawning (issue #1482); that
    // path is covered elsewhere and says nothing about prompt delivery.
    if matches!(
        root_sandbox::detect(),
        SkipPermissionsEnv::RootOutsideSandbox
            | SkipPermissionsEnv::UntrustedMarker(_)
            | SkipPermissionsEnv::ExplicitlyNotSandboxed { .. }
    ) {
        eprintln!("skipping: root outside a sandbox refuses a bare claude child");
        return;
    }

    let bin_dir = tempfile::tempdir().expect("bin tempdir");
    let record_dir = tempfile::tempdir().expect("record tempdir");
    write_stub(bin_dir.path(), record_dir.path());

    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(&original_path));
    let new_path = std::env::join_paths(paths).expect("join PATH");
    // SAFETY: single test in this binary; no concurrent environment access.
    unsafe {
        std::env::set_var("PATH", &new_path);
        std::env::set_var("AMPLIHACK_DELEGATE", "amplihack claude");
        std::env::remove_var("AMPLIHACK_PROMPT_DELIVERY");
    }

    let pattern = "claude_process stdin: 'quoted' \"double\" $HOME é € 中 𝄞\n";
    let mut prompt = String::new();
    while prompt.len() < 200 * 1024 {
        prompt.push_str(pattern);
    }
    assert!(prompt.len() > MAX_ARG_STRLEN);

    let runner = TokioProcessRunner::new();
    let result = runner
        .run(RunOptions {
            prompt: prompt.clone(),
            process_id: "issue-1570-claude-stdin".to_string(),
            timeout: Some(Duration::from_secs(20)),
            model: None,
            working_dir: Some(record_dir.path().to_path_buf()),
            result_sink: None,
        })
        .await;

    assert_eq!(
        result.exit_code, 0,
        "claude delegate run must succeed for a 200 KiB prompt; stderr: {}",
        result.stderr
    );
    assert_eq!(result.output, "stub-claude-ok");

    let argv_raw = std::fs::read(record_dir.path().join("argv")).expect("stub recorded argv");
    let argv: Vec<&[u8]> = argv_raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .collect();
    for arg in &argv {
        assert!(
            arg.len() <= MAX_ARG_STRLEN,
            "argv element of {} bytes exceeds MAX_ARG_STRLEN",
            arg.len()
        );
        assert!(
            !String::from_utf8_lossy(arg).contains("claude_process stdin:"),
            "the prompt must not appear in argv"
        );
    }
    assert!(argv.iter().any(|a| *a == b"--dangerously-skip-permissions"));
    assert!(argv.iter().any(|a| *a == b"-p"));

    let stdin = std::fs::read(record_dir.path().join("stdin")).expect("stub recorded stdin");
    assert!(
        stdin == prompt.as_bytes(),
        "stdin must carry the prompt byte-for-byte ({} bytes received, {} expected)",
        stdin.len(),
        prompt.len()
    );
}
