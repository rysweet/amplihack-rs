use super::command_builder_tests::{keep, no_env};
use super::launcher::{
    VALID_DELEGATES, write_classic_launcher_with, write_executable_script,
    write_recipe_launcher_with,
};
use super::models::Workstream;
use crate::util::run_output_with_timeout;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Upper bound for the `bash -n` syntax check on a generated launcher script.
///
/// A healthy `bash -n` on a tiny launcher completes in well under a
/// millisecond. This ceiling exists only to catch a genuinely hung `bash`
/// (and let `run_output_with_timeout` kill + reap it) -- it must be generous
/// enough that process-spawn latency on a saturated CI runner can never trip
/// it, and bounded so a real hang cannot stall the suite.
const BASH_SYNTAX_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn test_valid_delegates() {
    assert!(VALID_DELEGATES.contains(&"amplihack claude"));
    assert!(VALID_DELEGATES.contains(&"amplihack copilot"));
    assert!(VALID_DELEGATES.contains(&"amplihack amplifier"));
    assert!(VALID_DELEGATES.contains(&"amplihack codex"));
    assert!(!VALID_DELEGATES.contains(&"rm -rf /"));
}

/// Issue #1983: every binary the resolver can name has a delegate, so a
/// workstream never falls back to another CLI because its own is missing.
#[test]
fn every_allowlisted_binary_has_a_delegate() {
    for binary in amplihack_utils::agent_binary::ALLOWED_BINARIES {
        let delegate = format!("amplihack {binary}");
        assert!(VALID_DELEGATES.contains(&delegate.as_str()), "{delegate}");
    }
    assert_eq!(
        VALID_DELEGATES.len(),
        amplihack_utils::agent_binary::ALLOWED_BINARIES.len()
    );
}

/// Issue #1983: with no `AMPLIHACK_DELEGATE`, the delegate is the active agent
/// binary, not a fixed `amplihack claude`.
#[test]
fn delegate_follows_the_active_agent_binary() {
    use super::command_builder::delegate_for;
    for binary in amplihack_utils::agent_binary::ALLOWED_BINARIES {
        assert_eq!(delegate_for(None, binary), format!("amplihack {binary}"));
    }
}

/// Issue #1983: `AMPLIHACK_DELEGATE='amplihack codex'` is an explicit choice
/// and is honoured; an invalid value falls back to the active binary, not to
/// claude.
#[test]
fn explicit_delegate_wins_and_an_invalid_one_falls_back_to_the_active_binary() {
    use super::command_builder::delegate_for;
    assert_eq!(
        delegate_for(Some("amplihack codex"), "claude"),
        "amplihack codex"
    );
    assert_eq!(
        delegate_for(Some("amplihack claude"), "codex"),
        "amplihack claude"
    );
    assert_eq!(delegate_for(Some("rm -rf /"), "codex"), "amplihack codex");
    assert_eq!(delegate_for(Some(""), "codex"), "amplihack codex");
}

/// Issue #1983: classic mode starts each delegate in its own non-interactive
/// form. Codex takes `exec` (its `-p` is `--profile`), and only Claude reads
/// the `@TASK.md` file mention.
#[test]
fn classic_script_uses_each_delegates_prompt_form() {
    use super::command_builder::classic_run_script;
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = test_workstream(base.path(), state.path());

    let claude = classic_run_script(&ws, "amplihack claude", no_env, keep).unwrap();
    assert!(
        claude.contains("amplihack claude --subprocess-safe -- -p \"@TASK.md"),
        "{claude}"
    );

    let codex = classic_run_script(&ws, "amplihack codex", no_env, keep).unwrap();
    assert!(
        codex.contains("amplihack codex --subprocess-safe -- exec \""),
        "{codex}"
    );
    assert!(codex.contains("TASK.md"), "{codex}");
    assert!(!codex.contains("@TASK.md"), "{codex}");

    let amplifier = classic_run_script(&ws, "amplihack amplifier", no_env, keep).unwrap();
    assert!(
        amplifier.contains("amplihack amplifier --subprocess-safe -- run \""),
        "{amplifier}"
    );
    assert!(!amplifier.contains("@TASK.md"), "{amplifier}");

    let copilot = classic_run_script(&ws, "amplihack copilot", no_env, keep).unwrap();
    assert!(
        copilot.contains("amplihack copilot --subprocess-safe -- -p \""),
        "{copilot}"
    );
    assert!(!copilot.contains("@TASK.md"), "{copilot}");
}

#[test]
fn executable_script_writer_normalizes_crlf_and_lone_cr_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let crlf_script = dir.path().join("crlf-run.sh");
    let lone_cr_script = dir.path().join("lone-cr-run.sh");

    write_executable_script(
        &crlf_script,
        "#!/usr/bin/env bash\r\nset -euo pipefail\r\necho crlf\r\n",
    )
    .unwrap();
    write_executable_script(
        &lone_cr_script,
        "#!/usr/bin/env bash\rset -euo pipefail\recho lone-cr\r",
    )
    .unwrap();

    for script in [&crlf_script, &lone_cr_script] {
        assert_script_is_lf_only_and_bash_valid(script);
    }
}

#[test]
fn recipe_launcher_scripts_are_lf_only_and_bash_valid() {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = test_workstream(base.path(), state.path());
    fs::create_dir_all(&ws.work_dir).unwrap();

    // Issue #2014: the profile environment is injected. Another test in the
    // same process may set AMPLIHACK_CLAUDE_PROFILE_ROOT to invalid UTF-8.
    write_recipe_launcher_with(&ws, "amplihack copilot", no_env, keep).unwrap();

    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("launcher.sh"));
    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
}

#[test]
fn classic_launcher_script_is_lf_only_and_bash_valid() {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = test_workstream(base.path(), state.path());
    fs::create_dir_all(&ws.work_dir).unwrap();

    // Issue #2014: see `recipe_launcher_scripts_are_lf_only_and_bash_valid`.
    write_classic_launcher_with(&ws, "amplihack copilot", no_env, keep).unwrap();

    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
}

pub(super) fn test_workstream(base_dir: &Path, state_dir: &Path) -> Workstream {
    Workstream::new(
        792,
        "feature/line-endings".to_string(),
        "Fix line endings".to_string(),
        "Normalize generated executable scripts".to_string(),
        "default-workflow".to_string(),
        base_dir,
        state_dir,
    )
}

pub(super) fn assert_script_is_lf_only_and_bash_valid(script: &Path) {
    let bytes = fs::read(script).unwrap();
    assert!(
        !bytes.contains(&b'\r'),
        "{} contains carriage returns and will fail under bash",
        script.display()
    );

    // Resolve `bash` to an absolute path via the filesystem rather than
    // relying on the ambient `PATH`. Spawning a bare program name makes the
    // test depend on process-global `PATH`, which is shared mutable state:
    // any concurrent test that overrides `PATH` could otherwise cause a
    // spurious `ENOENT` spawn failure here. An absolute program path skips
    // `PATH` resolution entirely.
    let bash = bash_program();
    let mut command = Command::new(&bash);
    command.arg("-n").arg(script);
    let output = run_output_with_timeout(command, BASH_SYNTAX_CHECK_TIMEOUT)
        .unwrap_or_else(|err| panic!("failed to run bash -n for {}: {err:#}", script.display()));

    assert!(
        output.status.success(),
        "bash -n rejected {}:\nstdout:\n{}\nstderr:\n{}",
        script.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Resolve an absolute path to `bash` without consulting `PATH`.
///
/// Checks the standard system locations directly on the filesystem so the
/// result is unaffected by concurrent tests that mutate the process-global
/// `PATH`. Falls back to the bare name only if no known location exists,
/// preserving the original behavior on unusual systems.
fn bash_program() -> std::path::PathBuf {
    for candidate in ["/usr/bin/bash", "/bin/bash"] {
        let path = Path::new(candidate);
        if path.exists() {
            return path.to_path_buf();
        }
    }
    std::path::PathBuf::from("bash")
}
