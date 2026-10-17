//! Issue #1570: `llm_client::completion` sends a Claude prompt on stdin, not
//! as an argv element, where a 200 KiB prompt would fail with `E2BIG`.
//!
//! A stub `claude` on `PATH` records its argv and stdin.
//!
//! This binary mutates the process-global `PATH` and `AMPLIHACK_AGENT_BINARY`.
//! That is safe only because it is its own test binary and holds a single
//! test; keep it that way (see `no_global_path_mutation.rs`).
//!
//! Run with:
//!     cargo test -p amplihack-utils --test llm_client_stdin

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use amplihack_utils::llm_client::completion;

/// Linux `MAX_ARG_STRLEN`.
const MAX_ARG_STRLEN: usize = 131_072;

fn large_prompt(min_bytes: usize) -> String {
    let pattern = "llm_client 'quoted' \"double\" $HOME `tick` é € 中 𝄞\n";
    let mut out = String::with_capacity(min_bytes + pattern.len());
    while out.len() < min_bytes {
        out.push_str(pattern);
    }
    out
}

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

#[test]
fn claude_completion_sends_large_prompt_on_stdin_not_argv() {
    let bin_dir = tempfile::tempdir().expect("bin tempdir");
    let record_dir = tempfile::tempdir().expect("record tempdir");
    write_stub(bin_dir.path(), record_dir.path());

    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(&original_path));
    let new_path = std::env::join_paths(paths).expect("join PATH");
    // SAFETY: this test binary contains a single test, so no other thread
    // reads or writes the environment concurrently.
    unsafe {
        std::env::set_var("PATH", &new_path);
        std::env::set_var("AMPLIHACK_AGENT_BINARY", "claude");
        std::env::remove_var("AMPLIHACK_PROMPT_DELIVERY");
    }

    let prompt = large_prompt(200 * 1024);
    assert!(prompt.len() > MAX_ARG_STRLEN);

    let result = completion(&prompt);
    let output = result.unwrap_or_else(|e| {
        panic!("completion through the stub claude must succeed for a 200 KiB prompt; got: {e}")
    });
    assert_eq!(output, "stub-claude-ok");

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
            !String::from_utf8_lossy(arg).contains("llm_client 'quoted'"),
            "the prompt must not appear in argv"
        );
    }
    assert!(
        argv.iter().any(|a| *a == b"-p"),
        "claude must still be run in print mode (-p)"
    );

    let stdin = std::fs::read(record_dir.path().join("stdin")).expect("stub recorded stdin");
    assert!(
        stdin == prompt.as_bytes(),
        "stdin must carry the prompt byte-for-byte ({} bytes received, {} expected)",
        stdin.len(),
        prompt.len()
    );
}
