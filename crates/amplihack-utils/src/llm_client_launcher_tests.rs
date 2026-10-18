//! Issue #1570: which delivery `run_launcher` gives each launcher binary.
//!
//! The stubs are named by absolute path, which exercises the same file-name
//! rule as a bare name on `PATH` without mutating the process environment.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::run_launcher;

/// A stub that prints each argument on its own line.
fn stub(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::write(
        &path,
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().to_string()
}

fn args_of(binary: &str, prompt: &str) -> Vec<String> {
    // A parallel test can fork while a stub's write fd is still open, so the
    // first exec may fail with ETXTBSY; retry briefly.
    let mut attempts = 0;
    let out = loop {
        match run_launcher(binary, prompt) {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => break result.expect("stub launcher runs"),
        }
    };
    assert!(out.status.success(), "stub failed: {:?}", out.status);
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn large_prompt() -> String {
    "launcher prompt line\n".repeat(1000)
}

#[test]
fn copilot_gets_a_file_reference_and_add_dir_for_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let args = args_of(&stub(dir.path(), "copilot"), &large_prompt());
    let reference = args
        .iter()
        .find(|a| a.contains('/') && !a.starts_with("--"))
        .unwrap_or_else(|| panic!("no file reference in {args:?}"));
    assert!(
        !args.iter().any(|a| a.contains("launcher prompt line")),
        "the prompt must not appear in argv"
    );
    let at = args
        .iter()
        .position(|a| a == "--add-dir")
        .unwrap_or_else(|| panic!("copilot must be given --add-dir; got {args:?}"));
    let granted = Path::new(&args[at + 1]);
    assert!(
        reference.contains(granted.to_str().unwrap()),
        "--add-dir {} must name the directory of the prompt file in {reference:?}",
        granted.display()
    );
    // Security review (#1570): the granted directory is a private per-call
    // directory, never the shared temp root.
    assert_ne!(
        granted,
        std::env::temp_dir().as_path(),
        "--add-dir must not grant the whole temp root"
    );
    assert!(
        granted
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("simard-prompt-dir-")),
        "--add-dir must name the private prompt directory, got {}",
        granted.display()
    );
}

#[test]
fn an_unknown_launcher_receives_the_prompt_in_argv() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = large_prompt();
    let args = args_of(&stub(dir.path(), "other-launcher"), &prompt);
    assert!(!args.iter().any(|a| a == "--add-dir"));
    let joined = args.join("\n");
    assert!(
        joined.contains(prompt.trim_end()),
        "an unknown launcher must receive the prompt in argv"
    );
}
