//! Issue #1570 (finding 8): `Executor` hands the remote script the API key on
//! the first stdin line and the prompt after it, up to end of file. Nothing
//! large reaches `azlin` argv, so a 200 KiB prompt no longer hits E2BIG.
//!
//! A stub `azlin` on `PATH` records the argv (NUL-separated) and stdin of each
//! call.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;

use amplihack_remote::{Executor, VM};
use tempfile::TempDir;

/// Every test here mutates the process-wide `PATH`.
static PATH_GUARD: Mutex<()> = Mutex::new(());

const ARG_MAX_STRLEN: usize = 131_072;
const KEY: &str = "sk-ant-issue-1570-executor-key";

fn install_recording_azlin(dir: &Path, out: &Path) {
    let script = format!(
        r#"#!/bin/sh
out='{out}'
n=$(cat "$out/count" 2>/dev/null || echo 0); n=$((n+1)); printf '%s' "$n" > "$out/count"
printf '%s\0' "$@" > "$out/call$n.argv"
cat > "$out/call$n.stdin"
exit 0
"#,
        out = out.display()
    );
    let path = dir.join("azlin");
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(script.as_bytes()).unwrap();
    drop(file);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn vm() -> VM {
    VM {
        name: "amplihack-tester-1570".into(),
        size: "Standard_D2s_v3".into(),
        region: "eastus".into(),
        created_at: None,
        tags: None,
    }
}

fn prompt_200k() -> String {
    let mut prompt = String::from("ISSUE-1570-EXECUTOR-MARKER '; touch /tmp/pwned; ' é🚀\n");
    while prompt.len() < 200 * 1024 {
        prompt.push_str("remote executor prompt line with $VAR and `ticks`\n");
    }
    prompt.push_str("no trailing newline");
    prompt
}

struct Recorded {
    _bin: TempDir,
    out: TempDir,
}

impl Recorded {
    fn count(&self) -> usize {
        fs::read_to_string(self.out.path().join("count"))
            .map(|n| n.trim().parse().unwrap())
            .unwrap_or(0)
    }
    fn argv(&self, n: usize) -> Vec<Vec<u8>> {
        let raw = fs::read(self.out.path().join(format!("call{n}.argv"))).unwrap();
        raw.split(|b| *b == 0).map(<[u8]>::to_vec).collect()
    }
    fn stdin(&self, n: usize) -> Vec<u8> {
        fs::read(self.out.path().join(format!("call{n}.stdin"))).unwrap()
    }
}

/// Run `f` with the recording stub first on `PATH`.
fn with_stub_azlin<T>(f: impl FnOnce(&Executor) -> T) -> (T, Recorded) {
    let _guard = PATH_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let rec = Recorded {
        _bin: TempDir::new().unwrap(),
        out: TempDir::new().unwrap(),
    };
    install_recording_azlin(rec._bin.path(), rec.out.path());
    let original = std::env::var("PATH").unwrap_or_default();
    // SAFETY: PATH access is serialised by PATH_GUARD and restored below.
    unsafe { std::env::set_var("PATH", format!("{}:{original}", rec._bin.path().display())) };
    let executor = Executor::new(vm(), 1, None);
    let result = f(&executor);
    unsafe { std::env::set_var("PATH", original) };
    (result, rec)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn assert_small_argv_and_exact_stdin(rec: &Recorded, prompt: &str) {
    assert_eq!(rec.count(), 1, "exactly one azlin call");
    let marker = b"ISSUE-1570-EXECUTOR-MARKER";
    for arg in rec.argv(1) {
        assert!(
            arg.len() <= ARG_MAX_STRLEN,
            "argv element of {} bytes",
            arg.len()
        );
        assert!(
            !arg.windows(marker.len()).any(|w| w == marker),
            "prompt in argv"
        );
    }
    let expected = format!("{KEY}\n{prompt}");
    let stdin = rec.stdin(1);
    assert_eq!(stdin.len(), expected.len(), "stdin length");
    assert!(
        stdin == expected.as_bytes(),
        "stdin must be key line then prompt, byte for byte"
    );
}

#[test]
fn execute_remote_sends_key_then_200k_prompt_on_stdin() {
    let prompt = prompt_200k();
    let (result, rec) =
        with_stub_azlin(|e| block_on(e.execute_remote_with_api_key("auto", &prompt, 10, KEY)));
    let result = result.expect("a 200 KiB prompt must not fail the launch (E2BIG)");
    assert_eq!(result.exit_code, 0);
    assert_small_argv_and_exact_stdin(&rec, &prompt);
}

#[test]
fn execute_remote_tmux_sends_key_then_200k_prompt_on_stdin() {
    let prompt = prompt_200k();
    let (result, rec) =
        with_stub_azlin(|e| block_on(e.execute_remote_tmux("sess-1", "auto", &prompt, 10, KEY)));
    result.expect("a 200 KiB prompt must not fail the tmux launch (E2BIG)");
    assert_small_argv_and_exact_stdin(&rec, &prompt);
}

#[test]
fn non_auto_commands_fail_locally_before_azlin_runs() {
    let expected =
        "remote command 'ultrathink' is not supported: only 'auto' accepts --prompt-file";
    let (direct, rec) =
        with_stub_azlin(|e| block_on(e.execute_remote_with_api_key("ultrathink", "p", 10, KEY)));
    let err = direct.expect_err("ultrathink must be refused");
    assert!(err.to_string().contains(expected), "got: {err}");
    assert_eq!(rec.count(), 0, "nothing may be sent to the VM");

    let (tmux, rec) =
        with_stub_azlin(|e| block_on(e.execute_remote_tmux("sess-1", "ultrathink", "p", 10, KEY)));
    let err = tmux.expect_err("ultrathink must be refused");
    assert!(err.to_string().contains(expected), "got: {err}");
    assert_eq!(rec.count(), 0, "nothing may be sent to the VM");
}
