//! Component contracts for #2053 using the existing exported interfaces.
//! The binary suite separately exercises dispatch, ordering and subprocesses.
use super::{StopHook, lock, power_steering, reflection};
use crate::protocol::Hook;
use crate::test_support::{CwdGuard, EnvVarGuard, env_lock};
use amplihack_types::{HookInput, ProjectDirs, session_state_dir_under};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Observe the mode selected by the actual Stop construction sites while
/// delegating storage to the real counter. All existing lock and power-steering
/// execution tests therefore reject accidental use of the timeout default.
/// This adapter and its two imports are compiled only into unit tests.
pub(super) struct StopCounter {
    counter: amplihack_state::AtomicCounter,
    blocking: bool,
}

impl StopCounter {
    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            counter: amplihack_state::AtomicCounter::new(path),
            blocking: false,
        }
    }

    pub(super) fn with_blocking_lock(self) -> Self {
        Self {
            counter: self.counter.with_blocking_lock(),
            blocking: true,
        }
    }

    pub(super) fn increment(&self) -> Result<u64, amplihack_state::counter::CounterError> {
        assert!(
            self.blocking,
            "Stop counter must explicitly select blocking acquisition"
        );
        self.counter.increment()
    }
}

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Callers hold env_lock for this guard's entire lifetime. Every environment
/// variable that can select a home, profile, framework or agent is isolated.
pub(super) struct Environment {
    _vars: Vec<EnvVarGuard>,
    root: tempfile::TempDir,
}

impl Environment {
    pub(super) fn new() -> Self {
        let root = crate::test_support::isolated_tempdir();
        let mut vars = Vec::new();
        for (key, name) in [
            ("HOME", "home"),
            ("AMPLIHACK_HOME", "state"),
            ("AMPLIHACK_ROOT", "framework"),
            ("AMPLIHACK_CLAUDE_PROFILE_ROOT", "profiles"),
            ("CLAUDE_CONFIG_DIR", "claude-profile"),
            ("CODEX_HOME", "codex-profile"),
            ("XDG_CONFIG_HOME", "config"),
        ] {
            let path = root.path().join(name);
            fs::create_dir(&path).unwrap();
            vars.push(EnvVarGuard::set(key, path));
        }
        for key in [
            "CLAUDE_SESSION_ID",
            "AMPLIHACK_SKIP_POWER_STEERING",
            "AMPLIHACK_SKIP_REFLECTION",
            "AMPLIHACK_ENABLE_REFLECTION",
            "AMPLIHACK_MAX_LOCK_ITERATIONS",
        ] {
            vars.push(EnvVarGuard::unset(key));
        }
        vars.push(EnvVarGuard::set(
            "AMPLIHACK_REFLECTION_BINARY",
            root.path().join("absent-agent"),
        ));
        vars.push(EnvVarGuard::set("GIT_CONFIG_NOSYSTEM", "1"));
        vars.push(EnvVarGuard::set("GIT_CONFIG_GLOBAL", "/dev/null"));
        Self { _vars: vars, root }
    }

    fn dirs(&self) -> ProjectDirs {
        let project = self.root.path().join("project");
        fs::create_dir_all(&project).unwrap();
        ProjectDirs::new(project)
    }

    fn state(&self, id: Option<&str>) -> PathBuf {
        session_state_dir_under(&self.root.path().join("state"), id)
    }
}

/// Call before changing HOME, while holding env_lock. Only the installed Rust
/// toolchain is read from outside the fixture; compiler and child homes,
/// profiles, sources and outputs all belong to temporary directories.
pub(super) fn reflection_fixture() -> &'static Path {
    use std::process::Command;
    use std::sync::OnceLock;
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE.get_or_init(|| {
        let root = crate::test_support::isolated_tempdir();
        let source = root.path().join("reflection.rs");
        write(&source, include_str!("../../../../bins/amplihack-hooks/tests/support/issue_2053_reflection_fixture.rs"));
        let binary = root.path().join("reflection-fixture");
        let mut compiler = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        compiler.env_clear().current_dir(root.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root.path());
        if let Some(rustup) = std::env::var_os("RUSTUP_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup"))) {
            compiler.env("RUSTUP_HOME", rustup);
        }
        let output = compiler.args(["--edition=2024", "--crate-name", "stop_reflection_fixture"])
            .arg(&source).arg("-o").arg(&binary).output().unwrap();
        assert!(output.status.success(), "compile Rust fixture: {}", String::from_utf8_lossy(&output.stderr));
        (root, binary)
    }).1
}

fn count(path: &Path) -> u64 {
    let value: Value = serde_json::from_slice(
        &fs::read(path).unwrap_or_else(|error| panic!("counter {}: {error}", path.display())),
    )
    .unwrap();
    value["value"].as_u64().unwrap()
}

#[test]
fn lock_wrapper_reads_project_prompt_and_reuses_shared_session_counter() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    write(dirs.lock_active_file(), "");
    write(
        dirs.continuation_prompt_file(),
        "  Finish the isolated task  \n",
    );
    for id in ["lane 3", "lane_3"] {
        assert_eq!(
            lock::handle_lock_mode(&dirs, id).unwrap(),
            json!({"decision":"block", "reason":"Finish the isolated task"})
        );
    }
    let counter = env.state(Some("lane 3")).join("locks/lock_invocations.txt");
    assert_eq!(count(&counter), 2);
    assert!(counter.with_extension("json.lock").is_file());
    assert!(!dirs.session_locks("lane_3").exists());
    assert_eq!(
        fs::read_to_string(dirs.continuation_prompt_file()).unwrap(),
        "  Finish the isolated task  \n"
    );
}

#[test]
fn lock_wrapper_propagates_storage_failure_without_creating_project_state() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    write(env.state(Some("blocked")).join("locks"), "obstruction");
    assert!(lock::handle_lock_mode(&dirs, "blocked").is_err());
    assert!(!dirs.runtime.exists());
}

#[test]
fn lock_wrapper_uses_default_prompt_for_missing_blank_or_overlong_project_input() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    for prompt in [None, Some("  \n".to_owned()), Some("x".repeat(1001))] {
        if let Some(prompt) = prompt {
            write(dirs.continuation_prompt_file(), &prompt);
        }
        let result = lock::handle_lock_mode(&dirs, "prompt-boundary").unwrap();
        assert_eq!(result["decision"], "block");
        assert_eq!(result["reason"], super::DEFAULT_CONTINUATION_PROMPT);
    }
    assert_eq!(
        count(
            &env.state(Some("prompt-boundary"))
                .join("locks/lock_invocations.txt")
        ),
        3
    );
    assert!(!dirs.session_locks("prompt-boundary").exists());
}

#[test]
fn power_wrapper_reuses_resolved_completion_marker_and_still_increments() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    let power = env.state(Some("lane 3")).join("power-steering");
    write(power.join("session_count"), r#"{"value":4}"#);
    write(power.join(".lane_3_completed"), "");
    write(power.join("summary.md"), "previous summary");
    assert!(
        power_steering::check(&dirs, "lane_3", None)
            .unwrap()
            .is_none()
    );
    assert_eq!(count(&power.join("session_count")), 5);
    assert_eq!(
        fs::read_to_string(power.join("summary.md")).unwrap(),
        "previous summary"
    );
    assert!(!dirs.runtime.exists());
}

#[test]
fn power_wrapper_propagates_storage_failure_without_creating_project_state() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    write(
        env.state(Some("blocked")).join("power-steering"),
        "obstruction",
    );
    assert!(power_steering::check(&dirs, "blocked", None).is_err());
    assert!(!dirs.runtime.exists());
}

#[test]
fn reflection_wrapper_consumes_only_the_resolved_sessions_semaphore() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    let semaphore = env
        .state(Some("lane 3"))
        .join("reflection/.reflection_presented_lane_3");
    let other = env
        .state(Some("other"))
        .join("reflection/.reflection_presented_other");
    write(&semaphore, "");
    write(&other, "");
    assert!(
        reflection::run_reflection(&dirs, "lane_3", None)
            .unwrap()
            .is_none()
    );
    assert!(!semaphore.exists());
    assert!(other.exists());
    assert!(!dirs.runtime.exists());
}

#[test]
fn reflection_wrapper_rejects_obstructed_state_before_subprocess_execution() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    write(env.state(Some("blocked")).join("logs"), "obstruction");
    assert!(reflection::run_reflection(&dirs, "blocked", None).is_err());
    assert!(!dirs.runtime.exists());
}

#[test]
fn coordinator_without_root_skips_enabled_features_and_keeps_project_controls() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    write(dirs.lock_active_file(), "");
    write(dirs.power_steering_config(), r#"{"enabled":true}"#);
    write(
        dirs.tools_amplihack.join(".reflection_config"),
        r#"{"enabled":true}"#,
    );
    let _home = EnvVarGuard::unset("HOME");
    let _state = EnvVarGuard::unset("AMPLIHACK_HOME");
    let _threshold = EnvVarGuard::set("AMPLIHACK_MAX_LOCK_ITERATIONS", "1");
    let _cwd = CwdGuard::set(&dirs.root).unwrap();
    let input: HookInput = serde_json::from_value(json!({"hook_event_name":"Stop"})).unwrap();
    assert_eq!(
        StopHook.process(input).unwrap(),
        json!({"decision":"approve"})
    );
    assert!(dirs.lock_active_file().exists());
    assert!(!dirs.session_locks("unknown-session").exists());
    assert!(!dirs.power_steering.exists());
    assert!(!dirs.runtime.join("reflection").exists());
}

#[test]
fn exported_feature_wrappers_fail_open_when_no_root_resolves() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let env = Environment::new();
    let dirs = env.dirs();
    let _home = EnvVarGuard::unset("HOME");
    let _state = EnvVarGuard::unset("AMPLIHACK_HOME");
    let lock_result = lock::handle_lock_mode(&dirs, "no-root");
    let power_result = power_steering::check(&dirs, "no-root", None);
    let reflection_result = reflection::run_reflection(&dirs, "no-root", None);
    assert_eq!(lock_result.unwrap(), json!({"decision":"approve"}));
    assert!(power_result.unwrap().is_none());
    assert!(reflection_result.unwrap().is_none());
    assert!(!dirs.runtime.exists());
}
