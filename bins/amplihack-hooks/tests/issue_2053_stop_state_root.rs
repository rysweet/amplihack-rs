//! Behavioural contract for #2053. No production implementation belongs here.
//!
//! All children use isolated homes, profiles, repositories and state roots.
//! Coordination uses subprocess completion only; there are no timing windows.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use isolated_hook::HookSandbox;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const FINDINGS: &str = "## Task Summary\nControlled Rust reflection findings";
const CONTROLS: [&str; 4] = [
    ".lock_active",
    ".lock_goal",
    ".lock_message",
    ".continuation_prompt",
];

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn enable_lock(s: &HookSandbox) {
    for control in CONTROLS {
        write(
            s.project().join(".claude/runtime/locks").join(control),
            "Continue the isolated task",
        );
    }
}

fn enable_power(s: &HookSandbox) {
    write(
        s.project()
            .join(".claude/tools/amplihack/.power_steering_config"),
        r#"{"enabled":true}"#,
    );
}

fn enable_reflection(s: &HookSandbox) {
    write(
        s.project()
            .join(".claude/tools/amplihack/.reflection_config"),
        r#"{"enabled":true}"#,
    );
    write(
        s.project().join("USER_PREFERENCES.md"),
        "fixture-preference",
    );
    write(
        s.project()
            .join(".claude/tools/amplihack/hooks/templates/reflection_prompt.txt"),
        "fixture-template\n{user_preferences_context}\n{repository_context}\n{conversation_summary}\n{redirects_context}\n{template}",
    );
    write(
        s.project().join(".claude/templates/FEEDBACK_SUMMARY.md"),
        "fixture-feedback-template",
    );
}

fn transcript(s: &HookSandbox, complete: bool) -> PathBuf {
    let path = s.root().join("transcript.jsonl");
    let final_text = if complete {
        "Implemented the fix and validated it."
    } else {
        "Next steps: finish the remaining work."
    };
    write(
        &path,
        &format!(
            "{}\n{}\n",
            json!({"type":"user","message":{"role":"user","content":"Implement fixture-task"}}),
            json!({"type":"assistant","message":{"role":"assistant","content":final_text}}),
        ),
    );
    path
}

fn session(root: &Path, id: Option<&str>) -> PathBuf {
    amplihack_types::session_state_dir_under(root, id)
}

fn state(s: &HookSandbox, id: Option<&str>) -> PathBuf {
    session(&s.amplihack_home(), id)
}

fn hook(s: &HookSandbox) -> Command {
    let mut command = s.command(env!("CARGO_BIN_EXE_amplihack-hooks"));
    command
        .arg("stop")
        // Explicitly select the normal protocol; native Codex skips Stop checks.
        .env("AMPLIHACK_HOOK_PROTOCOL", "claude")
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_ROOT", s.root().join("absent-framework"))
        .env("CLAUDE_CONFIG_DIR", s.profiles().join("claude"))
        .env("CODEX_HOME", s.profiles().join("codex"))
        .env("XDG_CONFIG_HOME", s.profiles().join("config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.excludesFile")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
        // Even a regression in opt-in must never launch an installed agent.
        .env("AMPLIHACK_REFLECTION_BINARY", s.root().join("absent-agent"));
    command
}

fn invoke(mut command: Command, id: Option<&str>, transcript: Option<&Path>) -> Value {
    let mut input = json!({"hook_event_name":"Stop", "stop_hook_active":true});
    if let Some(id) = id {
        input["session_id"] = json!(id);
    }
    if let Some(path) = transcript {
        input["transcript_path"] = json!(path);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start normal Stop hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Stop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.stderr.is_empty() {
        eprintln!(
            "Stop diagnostics: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid Stop JSON: {error}; stdout={:?}", output.stdout))
}

fn assert_counter(path: &Path, expected: u64) {
    let value: Value = serde_json::from_slice(
        &fs::read(path)
            .unwrap_or_else(|error| panic!("missing counter {}: {error}", path.display())),
    )
    .unwrap();
    assert_eq!(value["value"], expected, "counter {}", path.display());
    assert!(path.with_extension("json.lock").is_file());
}

// Include contents, directory modes and symlink targets. Git's index and
// status are checked separately. Excluding .git avoids Git's own cache files;
// its exclude/config files are explicitly included below.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Directory(u32),
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn walk(root: &Path, at: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
        for item in fs::read_dir(at).unwrap() {
            let path = item.unwrap().path();
            if path == root.join(".git") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let entry = if metadata.file_type().is_symlink() {
                Entry::Symlink(fs::read_link(&path).unwrap())
            } else if metadata.is_dir() {
                Entry::Directory(metadata.permissions().mode() & 0o777)
            } else {
                Entry::File(fs::read(&path).unwrap())
            };
            entries.insert(relative, entry);
            if metadata.is_dir() {
                walk(root, &path, entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

fn git(s: &HookSandbox, args: &[&str]) -> Vec<u8> {
    let mut command = amplihack_git::command_in(s.project());
    command.env_clear();
    command
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", s.home())
        .env("XDG_CONFIG_HOME", s.profiles().join("git"))
        .envs(isolated_hook::NO_GIT_PROMPT_ENV)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "core.excludesFile=/dev/null"])
        .args(args);
    let output = command.output().unwrap();
    assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
    output.stdout
}

fn init_repo(s: &HookSandbox) {
    git(s, &["init", "-q"]);
    write(s.project().join("tracked.txt"), "tracked fixture\n");
    git(s, &["add", "."]);
}

#[derive(Debug, PartialEq, Eq)]
struct RepoSnapshot {
    entries: BTreeMap<PathBuf, Entry>,
    stage: Vec<u8>,
    status: Vec<u8>,
    exclude: Option<Vec<u8>>,
    config: Vec<u8>,
}

fn snapshot(s: &HookSandbox) -> RepoSnapshot {
    RepoSnapshot {
        entries: tree(&s.project()),
        stage: git(s, &["ls-files", "--stage", "-z"]),
        status: git(
            s,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "-z",
            ],
        ),
        exclude: fs::read(s.project().join(".git/info/exclude")).ok(),
        config: fs::read(s.project().join(".git/config")).unwrap(),
    }
}

fn assert_private_tree(root: &Path) {
    for (path, entry) in tree(root) {
        if let Entry::Directory(mode) = entry {
            assert_eq!(mode, 0o700, "permissions of {}", root.join(path).display());
        }
    }
}

fn assert_owned_by_sessions(root: &Path, ids: &[Option<&str>]) {
    let allowed: Vec<_> = ids.iter().map(|id| session(root, *id)).collect();
    for relative in tree(root).keys() {
        let path = root.join(relative);
        assert!(
            allowed
                .iter()
                .any(|session| path.starts_with(session) || session.starts_with(&path)),
            "generated path has no owning session: {}",
            path.display()
        );
    }
}

/// libtest cannot accept Claude's `-p` arguments. Build a tiny Rust executable
/// once and keep its temporary directory alive until this test process exits.
/// It consumes stdin, verifies the subprocess contract and records evidence
/// outside the project. No shell wrapper or installed agent is involved.
fn reflection_fixture() -> &'static Path {
    static FIXTURE: OnceLock<(HookSandbox, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let sandbox = HookSandbox::new();
            let source = sandbox.root().join("fixture.rs");
            write(
                &source,
                include_str!("support/issue_2053_reflection_fixture.rs"),
            );
            let binary = sandbox.root().join("reflection-fixture");
            let mut compiler =
                sandbox.command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
            // rustup may read the installed toolchain; compiler HOME and hook
            // state remain isolated, just as for every other child here.
            if let Some(rustup) = std::env::var_os("RUSTUP_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup"))
                })
            {
                compiler.env("RUSTUP_HOME", rustup);
            }
            let output = compiler
                .args(["--edition=2024", "--crate-name", "stop_reflection_fixture"])
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "compile Rust fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            (sandbox, binary)
        })
        .1
}

fn reflective_hook(s: &HookSandbox, extra_prompt: &str) -> Command {
    let mut command = hook(s);
    command
        .env("AMPLIHACK_REFLECTION_BINARY", reflection_fixture())
        .env("STOP_FIXTURE_PROJECT", s.project())
        .env(
            "STOP_FIXTURE_EVIDENCE",
            s.root().join("reflection-invocations"),
        )
        .env("STOP_FIXTURE_EXTRA_PROMPT", extra_prompt)
        .env("STOP_FIXTURE_FINDINGS", FINDINGS);
    command
}

fn invocations(s: &HookSandbox) -> usize {
    fs::read_to_string(s.root().join("reflection-invocations"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn lock_reuses_normalised_sessions_and_unknown_session_without_cwd_writes() {
    let s = HookSandbox::new();
    enable_lock(&s);
    init_repo(&s);
    let before = snapshot(&s);
    let mut expected = BTreeMap::new();
    for id in [
        Some("lane 3"),
        Some("lane_3"),
        Some("distinct"),
        Some("../../unsafe/雪"),
        None,
        Some(""),
        Some(" \t\n"),
    ] {
        let mut command = hook(&s);
        command.env("CLAUDE_SESSION_ID", "must-not-select-storage");
        let result = invoke(command, id, None);
        assert_eq!(result["decision"], "block");
        let path = state(&s, id).join("locks/lock_invocations.txt");
        let count = expected.entry(path.clone()).or_insert(0);
        *count += 1;
        assert_counter(&path, *count);
    }
    assert_eq!(snapshot(&s), before);
    assert!(!state(&s, Some("must-not-select-storage")).exists());
    assert!(!s.home().join(".amplihack").exists());
    assert_private_tree(&s.amplihack_home());
    assert_owned_by_sessions(
        &s.amplihack_home(),
        &[
            Some("lane 3"),
            Some("distinct"),
            Some("../../unsafe/雪"),
            None,
        ],
    );
}

#[test]
fn lock_uses_shared_root_precedence_and_rejects_checkout_overrides() {
    for case in [
        "valid", "absent", "empty", "relative", "parent", "git-dir", "git-file", "git-link",
    ] {
        let s = HookSandbox::new();
        enable_lock(&s);
        init_repo(&s);
        let mut command = hook(&s);
        let expected_root = match case {
            "valid" => s.amplihack_home(),
            "absent" => {
                command.env_remove("AMPLIHACK_HOME");
                s.home().join(".amplihack")
            }
            "empty" | "relative" => {
                command.env(
                    "AMPLIHACK_HOME",
                    if case == "empty" { "" } else { "relative" },
                );
                s.home().join(".amplihack")
            }
            "parent" => {
                command.env("AMPLIHACK_HOME", s.root().join("unused/../escape"));
                s.home().join(".amplihack")
            }
            _ => {
                let checkout = s.root().join("checkout");
                fs::create_dir(&checkout).unwrap();
                match case {
                    "git-dir" => fs::create_dir(checkout.join(".git")).unwrap(),
                    "git-file" => write(checkout.join(".git"), "gitdir: absent\n"),
                    _ => symlink(s.root().join("absent-git"), checkout.join(".git")).unwrap(),
                }
                command.env("AMPLIHACK_HOME", checkout);
                s.home().join(".amplihack")
            }
        };
        let before = tree(s.root());
        let repo_before = snapshot(&s);
        assert_eq!(
            invoke(command, Some("root-case"), None)["decision"],
            "block",
            "{case}"
        );
        assert_counter(
            &session(&expected_root, Some("root-case")).join("locks/lock_invocations.txt"),
            1,
        );
        assert_private_tree(&expected_root);
        assert_owned_by_sessions(&expected_root, &[Some("root-case")]);
        if case != "valid" {
            assert_eq!(
                fs::metadata(&expected_root).unwrap().permissions().mode() & 0o777,
                0o700,
                "new state root must be private: {case}"
            );
        }
        assert_eq!(snapshot(&s), repo_before, "{case}");
        // Every change outside the selected root would be a fallback leak.
        let mut after = tree(s.root());
        after.retain(|path, _| !s.root().join(path).starts_with(&expected_root));
        let mut before = before;
        before.retain(|path, _| !s.root().join(path).starts_with(&expected_root));
        assert_eq!(after, before, "writes outside selected root: {case}");
    }
}

#[test]
fn no_root_approves_without_writes_or_safety_valve_removal() {
    for home in ["absent", "empty", "relative", "parent"] {
        let s = HookSandbox::new();
        enable_lock(&s);
        enable_power(&s);
        enable_reflection(&s);
        init_repo(&s);
        let mut command = hook(&s);
        command
            .env("AMPLIHACK_HOME", s.project())
            .env("AMPLIHACK_MAX_LOCK_ITERATIONS", "1");
        match home {
            "absent" => {
                command.env_remove("HOME");
            }
            "empty" => {
                command.env("HOME", "");
            }
            "relative" => {
                command.env("HOME", "relative-home");
            }
            _ => {
                command.env("HOME", s.root().join("unusable/../home"));
            }
        }
        let before = tree(s.root());
        let repo_before = snapshot(&s);
        assert_eq!(invoke(command, None, None), json!({"decision":"approve"}));
        assert_eq!(tree(s.root()), before);
        assert_eq!(snapshot(&s), repo_before);
    }
}

#[test]
fn invalid_or_absent_roots_never_fall_back_to_the_project() {
    for override_value in ["absent", "empty", "relative", "parent"] {
        let s = HookSandbox::new();
        enable_power(&s);
        enable_reflection(&s);
        init_repo(&s);
        let before = tree(s.root());
        let mut command = hook(&s);
        command.env_remove("HOME");
        match override_value {
            "absent" => {
                command.env_remove("AMPLIHACK_HOME");
            }
            "empty" => {
                command.env("AMPLIHACK_HOME", "");
            }
            "relative" => {
                command.env("AMPLIHACK_HOME", "relative-state");
            }
            _ => {
                command.env("AMPLIHACK_HOME", s.root().join("unusable/../state"));
            }
        }
        assert_eq!(
            invoke(command, Some("no-root"), None),
            json!({"decision":"approve"})
        );
        assert_eq!(tree(s.root()), before);
    }
}

#[test]
fn lock_short_circuits_later_features_and_preserves_project_prompt() {
    let s = HookSandbox::new();
    enable_lock(&s);
    enable_power(&s);
    enable_reflection(&s);
    init_repo(&s);
    let before = snapshot(&s);
    let output = invoke(hook(&s), Some("ordered"), None);
    assert_eq!(
        output,
        json!({"decision":"block", "reason":"Continue the isolated task"})
    );
    assert_counter(
        &state(&s, Some("ordered")).join("locks/lock_invocations.txt"),
        1,
    );
    assert!(!state(&s, Some("ordered")).join("power-steering").exists());
    assert!(!state(&s, Some("ordered")).join("reflection").exists());
    assert_eq!(snapshot(&s), before);
}

#[test]
fn safety_valve_removes_only_four_project_controls_and_approves_immediately() {
    let s = HookSandbox::new();
    enable_lock(&s);
    enable_power(&s);
    enable_reflection(&s);
    write(s.project().join(".claude/runtime/locks/keep.txt"), "keep");
    init_repo(&s);
    let repository_before = snapshot(&s);
    let before = tree(&s.project());
    let mut command = hook(&s);
    command.env("AMPLIHACK_MAX_LOCK_ITERATIONS", "2");
    assert_eq!(invoke(command, Some("valve"), None)["decision"], "block");
    let mut command = hook(&s);
    command.env("AMPLIHACK_MAX_LOCK_ITERATIONS", "2");
    assert_eq!(
        invoke(command, Some("valve"), None),
        json!({"decision":"approve"})
    );
    assert_counter(
        &state(&s, Some("valve")).join("locks/lock_invocations.txt"),
        2,
    );
    let mut expected = before;
    for control in CONTROLS {
        expected.remove(&PathBuf::from(".claude/runtime/locks").join(control));
    }
    assert_eq!(tree(&s.project()), expected);
    let repository_after = snapshot(&s);
    assert_eq!(repository_after.stage, repository_before.stage);
    assert_eq!(repository_after.config, repository_before.config);
    assert_eq!(repository_after.exclude, repository_before.exclude);
    assert!(!state(&s, Some("valve")).join("power-steering").exists());
    assert!(!state(&s, Some("valve")).join("reflection").exists());
}

#[test]
fn power_completion_and_count_are_reused_only_by_the_resolved_session() {
    let s = HookSandbox::new();
    enable_power(&s);
    let path = transcript(&s, true);
    init_repo(&s);
    let before = snapshot(&s);
    for count in 1..=2 {
        assert_eq!(
            invoke(hook(&s), Some("lane 3"), Some(&path))["decision"],
            "approve"
        );
        assert_counter(
            &state(&s, Some("lane 3")).join("power-steering/session_count"),
            count,
        );
    }
    let power = state(&s, Some("lane 3")).join("power-steering");
    assert!(power.join(".lane_3_completed").is_file());
    let summary = fs::read(power.join("summary.md")).unwrap();
    assert!(String::from_utf8_lossy(&summary).contains("fixture-task"));
    transcript(&s, false);
    assert_eq!(
        invoke(hook(&s), Some("lane_3"), Some(&path))["decision"],
        "approve"
    );
    assert_counter(&power.join("session_count"), 3);
    assert_eq!(fs::read(power.join("summary.md")).unwrap(), summary);
    assert_eq!(
        invoke(hook(&s), Some("other"), Some(&path))["decision"],
        "approve"
    );
    assert_eq!(
        invoke(hook(&s), Some("other"), Some(&path))["decision"],
        "block"
    );
    assert_counter(
        &state(&s, Some("other")).join("power-steering/session_count"),
        2,
    );
    assert!(
        !state(&s, Some("other"))
            .join("power-steering/.other_completed")
            .exists()
    );
    assert_eq!(snapshot(&s), before);
    assert_private_tree(&s.amplihack_home());
    assert_owned_by_sessions(&s.amplihack_home(), &[Some("lane 3"), Some("other")]);
    let names: Vec<_> = fs::read_dir(power)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names.len(),
        4,
        "counter, companion lock, completion and summary only: {names:?}"
    );
}

#[test]
fn power_missing_and_invalid_transcripts_fail_open_after_counting() {
    for input in [None, Some("missing"), Some("malformed")] {
        let s = HookSandbox::new();
        enable_power(&s);
        let path = s.root().join("invalid.jsonl");
        if input == Some("malformed") {
            write(&path, "invalid json\n");
        }
        init_repo(&s);
        let before = snapshot(&s);
        for count in 1..=2 {
            assert_eq!(
                invoke(hook(&s), None, input.map(|_| path.as_path()))["decision"],
                "approve"
            );
            assert_counter(&state(&s, None).join("power-steering/session_count"), count);
        }
        assert!(
            !state(&s, None)
                .join("power-steering/.unknown-session_completed")
                .exists()
        );
        assert_eq!(snapshot(&s), before);
    }
}

#[test]
fn power_project_disable_controls_keep_counting_without_blocking() {
    for sentinel in [true, false] {
        let s = HookSandbox::new();
        enable_power(&s);
        if sentinel {
            write(
                s.project().join(".claude/runtime/power-steering/.disabled"),
                "",
            );
        }
        let path = transcript(&s, false);
        init_repo(&s);
        let before = snapshot(&s);
        for count in 1..=2 {
            let mut command = hook(&s);
            if !sentinel {
                command.env("AMPLIHACK_SKIP_POWER_STEERING", "");
            }
            assert_eq!(
                invoke(command, Some("disabled"), Some(&path))["decision"],
                "approve"
            );
            assert_counter(
                &state(&s, Some("disabled")).join("power-steering/session_count"),
                count,
            );
        }
        assert_eq!(snapshot(&s), before);
    }
}

#[test]
fn reflection_keeps_project_inputs_but_saves_and_consumes_session_presentation() {
    let s = HookSandbox::new();
    enable_reflection(&s);
    let id = "lane 3";
    write(
        s.project()
            .join(".claude/runtime/logs/lane_3/redirects.jsonl"),
        r#"{"redirect_number":1,"failed_considerations":["tests"],"continuation_prompt":"fixture-project-redirect"}"#,
    );
    let path = transcript(&s, true);
    init_repo(&s);
    let before = snapshot(&s);
    let result = invoke(
        reflective_hook(&s, "fixture-project-redirect"),
        Some(id),
        Some(&path),
    );
    assert_eq!(
        invocations(&s),
        1,
        "controlled Rust subprocess must actually run"
    );
    assert_eq!(result["decision"], "block");
    assert!(result["reason"].as_str().unwrap().contains(FINDINGS));
    let session = state(&s, Some(id));
    assert_eq!(
        fs::read_to_string(session.join("logs/FEEDBACK_SUMMARY.md")).unwrap(),
        FINDINGS
    );
    assert_eq!(
        fs::read_to_string(session.join("reflection/current_findings.md")).unwrap(),
        FINDINGS
    );
    let reflection = session.join("reflection");
    let semaphore = reflection.join(".reflection_presented_lane_3");
    assert!(semaphore.is_file());
    let histories: Vec<_> = fs::read_dir(&reflection)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("reflection_lane_3_")
        })
        .collect();
    assert_eq!(histories.len(), 1);
    assert_eq!(fs::read_to_string(&histories[0]).unwrap(), FINDINGS);
    assert_eq!(
        invoke(
            reflective_hook(&s, "fixture-project-redirect"),
            Some("lane_3"),
            Some(&path)
        )["decision"],
        "approve"
    );
    assert_eq!(invocations(&s), 1);
    assert!(!semaphore.exists());
    assert_eq!(
        invoke(reflective_hook(&s, ""), Some("distinct"), Some(&path))["decision"],
        "block"
    );
    assert_eq!(invocations(&s), 2);
    assert!(
        state(&s, Some("distinct"))
            .join("reflection/.reflection_presented_distinct")
            .is_file()
    );
    assert_eq!(
        fs::read_to_string(session.join("reflection/current_findings.md")).unwrap(),
        FINDINGS
    );
    assert_eq!(snapshot(&s), before);
    assert_private_tree(&s.amplihack_home());
    assert_owned_by_sessions(&s.amplihack_home(), &[Some("lane 3"), Some("distinct")]);
}

#[test]
fn reflection_filenames_use_safe_and_unknown_session_keys() {
    for id in [None, Some(""), Some(" \t"), Some("../../unsafe/雪")] {
        let s = HookSandbox::new();
        enable_reflection(&s);
        let path = transcript(&s, true);
        init_repo(&s);
        let before = snapshot(&s);
        let mut command = reflective_hook(&s, "");
        command.env("CLAUDE_SESSION_ID", "must-not-select-storage");
        assert_eq!(invoke(command, id, Some(&path))["decision"], "block");
        assert_eq!(invocations(&s), 1);
        let resolved = state(&s, id);
        let key = resolved.file_name().unwrap().to_str().unwrap();
        assert!(
            resolved
                .join("reflection")
                .join(format!(".reflection_presented_{key}"))
                .is_file()
        );
        assert!(resolved.join("logs/FEEDBACK_SUMMARY.md").is_file());
        assert_eq!(
            invoke(reflective_hook(&s, ""), id, Some(&path))["decision"],
            "approve"
        );
        assert_eq!(invocations(&s), 1);
        assert_eq!(snapshot(&s), before);
    }
}

#[test]
fn reflection_still_presents_findings_when_later_output_writes_fail() {
    for output in ["logs/FEEDBACK_SUMMARY.md", "reflection/current_findings.md"] {
        let s = HookSandbox::new();
        enable_reflection(&s);
        let path = transcript(&s, true);
        let resolved = state(&s, Some("late-error"));
        fs::create_dir_all(resolved.join(output)).unwrap();
        init_repo(&s);
        let before = snapshot(&s);
        let result = invoke(reflective_hook(&s, ""), Some("late-error"), Some(&path));
        assert_eq!(invocations(&s), 1);
        assert_eq!(result["decision"], "block");
        assert!(result["reason"].as_str().unwrap().contains(FINDINGS));
        assert!(resolved.join(output).is_dir());
        assert!(
            resolved
                .join("reflection/.reflection_presented_late-error")
                .is_file()
        );
        assert_eq!(snapshot(&s), before);
    }
}

#[test]
fn power_summary_failure_is_fail_open_without_writing_another_root() {
    let s = HookSandbox::new();
    enable_power(&s);
    enable_reflection(&s);
    let path = transcript(&s, true);
    let resolved = state(&s, Some("summary-error"));
    write(
        resolved.join("power-steering/session_count"),
        r#"{"value":1}"#,
    );
    fs::create_dir(resolved.join("power-steering/summary.md")).unwrap();
    init_repo(&s);
    let before = snapshot(&s);
    assert_eq!(
        invoke(hook(&s), Some("summary-error"), Some(&path)),
        json!({})
    );
    assert_counter(&resolved.join("power-steering/session_count"), 2);
    assert!(
        resolved
            .join("power-steering/.summary-error_completed")
            .is_file()
    );
    assert!(!resolved.join("reflection").exists());
    assert_eq!(snapshot(&s), before);
}

#[test]
fn power_first_invocation_reaches_reflection_but_later_block_does_not() {
    let s = HookSandbox::new();
    enable_power(&s);
    enable_reflection(&s);
    let path = transcript(&s, false);
    init_repo(&s);
    let before = snapshot(&s);
    let first = invoke(reflective_hook(&s, ""), Some("ordered"), Some(&path));
    assert!(first["reason"].as_str().unwrap().contains(FINDINGS));
    assert_counter(
        &state(&s, Some("ordered")).join("power-steering/session_count"),
        1,
    );
    assert_eq!(invocations(&s), 1);
    let semaphore = state(&s, Some("ordered")).join("reflection/.reflection_presented_ordered");
    assert!(semaphore.exists());
    let second = invoke(reflective_hook(&s, ""), Some("ordered"), Some(&path));
    assert_eq!(second["decision"], "block");
    assert!(
        second["reason"]
            .as_str()
            .unwrap()
            .contains("remaining work")
    );
    assert_counter(
        &state(&s, Some("ordered")).join("power-steering/session_count"),
        2,
    );
    assert_eq!(invocations(&s), 1);
    assert!(
        semaphore.exists(),
        "power block must leave reflection presentation unconsumed"
    );
    assert_eq!(snapshot(&s), before);
}

#[test]
fn reflection_requires_opt_in_and_obeys_project_and_environment_suppression() {
    for case in ["default", "disabled-config", "project-lock", "skip-env"] {
        let s = HookSandbox::new();
        if case != "default" {
            enable_reflection(&s);
        }
        match case {
            "disabled-config" => write(
                s.project()
                    .join(".claude/tools/amplihack/.reflection_config"),
                r#"{"enabled":false}"#,
            ),
            "project-lock" => write(
                s.project()
                    .join(".claude/runtime/reflection/.reflection_lock"),
                "",
            ),
            _ => (),
        }
        let path = transcript(&s, true);
        init_repo(&s);
        let before = snapshot(&s);
        let mut command = hook(&s);
        if case == "skip-env" {
            command
                .env("AMPLIHACK_ENABLE_REFLECTION", "true")
                .env("AMPLIHACK_SKIP_REFLECTION", "1");
        }
        assert_eq!(
            invoke(command, Some("disabled"), Some(&path))["decision"],
            "approve",
            "{case}"
        );
        assert!(!state(&s, Some("disabled")).exists());
        assert_eq!(snapshot(&s), before);
    }
}

#[test]
fn deterministic_storage_obstructions_fail_open_without_cwd_fallback() {
    for feature in ["locks", "power-steering", "logs", "reflection"] {
        for obstruct_session in [false, true] {
            let s = HookSandbox::new();
            match feature {
                "locks" => enable_lock(&s),
                "power-steering" => enable_power(&s),
                _ => enable_reflection(&s),
            }
            // Enable reflection after earlier features to detect accidental fall-through.
            enable_reflection(&s);
            let resolved = state(&s, Some("obstructed"));
            let obstruction = if obstruct_session {
                resolved.clone()
            } else {
                resolved.join(feature)
            };
            write(&obstruction, "not a directory");
            init_repo(&s);
            let before = snapshot(&s);
            assert_eq!(invoke(hook(&s), Some("obstructed"), None), json!({}));
            assert_eq!(fs::read_to_string(obstruction).unwrap(), "not a directory");
            assert_eq!(snapshot(&s), before);
        }
    }
}

#[test]
fn existing_directory_modes_are_preserved_and_atomic_operations_leave_no_residue() {
    let s = HookSandbox::new();
    enable_lock(&s);
    let resolved = state(&s, Some("modes"));
    fs::create_dir_all(&resolved).unwrap();
    fs::set_permissions(&resolved, fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(s.amplihack_home(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(invoke(hook(&s), Some("modes"), None)["decision"], "block");
    assert_counter(&resolved.join("locks/lock_invocations.txt"), 1);
    assert_eq!(
        fs::metadata(&resolved).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(s.amplihack_home())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(resolved.join("locks"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let mut names: Vec<_> = fs::read_dir(resolved.join("locks"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            OsStr::new("lock_invocations.json.lock"),
            OsStr::new("lock_invocations.txt")
        ]
    );
}

#[test]
fn legacy_project_state_is_neither_imported_nor_modified() {
    let s = HookSandbox::new();
    enable_lock(&s);
    write(
        s.project()
            .join(".claude/runtime/locks/legacy/lock_invocations.txt"),
        r#"{"value":49}"#,
    );
    init_repo(&s);
    let before = snapshot(&s);
    assert_eq!(invoke(hook(&s), Some("legacy"), None)["decision"], "block");
    assert_counter(
        &state(&s, Some("legacy")).join("locks/lock_invocations.txt"),
        1,
    );
    assert_eq!(snapshot(&s), before);
}

#[test]
fn legacy_power_counter_and_completion_cannot_skip_new_session_processing() {
    let s = HookSandbox::new();
    enable_power(&s);
    let path = transcript(&s, false);
    let legacy = s.project().join(".claude/runtime/power-steering");
    write(legacy.join("legacy/session_count"), r#"{"value":49}"#);
    write(
        legacy.join("legacy/session_count.json.lock"),
        "legacy companion",
    );
    write(legacy.join(".legacy_completed"), "legacy completion");
    write(legacy.join("legacy/summary.md"), "legacy summary");
    init_repo(&s);
    let before = snapshot(&s);
    assert_eq!(
        invoke(hook(&s), Some("legacy"), Some(&path))["decision"],
        "approve"
    );
    let power = state(&s, Some("legacy")).join("power-steering");
    assert_counter(&power.join("session_count"), 1);
    assert_eq!(
        invoke(hook(&s), Some("legacy"), Some(&path))["decision"],
        "block"
    );
    assert_counter(&power.join("session_count"), 2);
    assert!(!power.join(".legacy_completed").exists());
    assert!(!power.join("summary.md").exists());
    assert_eq!(snapshot(&s), before);
}

#[test]
fn legacy_reflection_presentation_cannot_suppress_or_seed_new_findings() {
    let s = HookSandbox::new();
    enable_reflection(&s);
    let path = transcript(&s, true);
    let legacy = s.project().join(".claude/runtime/reflection");
    write(
        legacy.join(".reflection_presented_legacy"),
        "legacy semaphore",
    );
    write(legacy.join("reflection_legacy_0.md"), "legacy history");
    write(legacy.join("current_findings.md"), "legacy findings");
    write(
        s.project()
            .join(".claude/runtime/logs/legacy/FEEDBACK_SUMMARY.md"),
        "legacy feedback",
    );
    init_repo(&s);
    let before = snapshot(&s);
    assert_eq!(
        invoke(reflective_hook(&s, ""), Some("legacy"), Some(&path))["decision"],
        "block"
    );
    assert_eq!(invocations(&s), 1);
    let resolved = state(&s, Some("legacy"));
    assert_eq!(
        fs::read_to_string(resolved.join("logs/FEEDBACK_SUMMARY.md")).unwrap(),
        FINDINGS
    );
    assert_eq!(
        fs::read_to_string(resolved.join("reflection/current_findings.md")).unwrap(),
        FINDINGS
    );
    assert_eq!(
        invoke(reflective_hook(&s, ""), Some("legacy"), Some(&path))["decision"],
        "approve"
    );
    assert_eq!(invocations(&s), 1);
    assert!(
        !resolved
            .join("reflection/.reflection_presented_legacy")
            .exists()
    );
    assert_eq!(snapshot(&s), before);
}

#[test]
fn unknown_session_reuses_power_completion_and_reflection_presentation() {
    let s = HookSandbox::new();
    enable_power(&s);
    enable_reflection(&s);
    let path = transcript(&s, true);
    init_repo(&s);
    let before = snapshot(&s);
    // First power invocation reaches reflection. The second completes power
    // steering and consumes that same presentation despite the empty raw id.
    assert_eq!(
        invoke(reflective_hook(&s, ""), None, Some(&path))["decision"],
        "block"
    );
    assert_eq!(
        invoke(reflective_hook(&s, ""), Some(""), Some(&path))["decision"],
        "approve"
    );
    let resolved = state(&s, None);
    let power = resolved.join("power-steering");
    assert_counter(&power.join("session_count"), 2);
    assert!(power.join(".unknown-session_completed").is_file());
    let summary = fs::read(power.join("summary.md")).unwrap();
    // An unfinished transcript must still reuse completion and run reflection.
    transcript(&s, false);
    assert_eq!(
        invoke(reflective_hook(&s, ""), Some(" \t\n"), Some(&path))["decision"],
        "block"
    );
    assert_counter(&power.join("session_count"), 3);
    assert_eq!(fs::read(power.join("summary.md")).unwrap(), summary);
    assert_eq!(invocations(&s), 2);
    assert!(
        resolved
            .join("reflection/.reflection_presented_unknown-session")
            .is_file()
    );
    assert_eq!(snapshot(&s), before);
    assert_owned_by_sessions(&s.amplihack_home(), &[None]);
}

#[test]
fn counter_file_and_companion_failures_preserve_controls_before_the_safety_valve() {
    for feature in ["locks", "power-steering"] {
        for obstruction in [
            "counter-directory",
            "counter-malformed",
            "companion-directory",
        ] {
            let s = HookSandbox::new();
            if feature == "locks" {
                enable_lock(&s);
            } else {
                enable_power(&s);
            }
            enable_reflection(&s);
            let resolved = state(&s, Some("counter-error"));
            let counter = resolved.join(if feature == "locks" {
                "locks/lock_invocations.txt"
            } else {
                "power-steering/session_count"
            });
            match obstruction {
                "counter-directory" => fs::create_dir_all(&counter).unwrap(),
                "counter-malformed" => write(&counter, "broken counter"),
                _ => fs::create_dir_all(counter.with_extension("json.lock")).unwrap(),
            }
            init_repo(&s);
            let before = snapshot(&s);
            let mut command = hook(&s);
            command.env("AMPLIHACK_MAX_LOCK_ITERATIONS", "1");
            assert_eq!(
                invoke(command, Some("counter-error"), None),
                json!({}),
                "{feature}: {obstruction}"
            );
            assert!(!resolved.join("reflection").exists());
            assert_eq!(snapshot(&s), before, "{feature}: {obstruction}");
        }
    }
}

#[test]
fn completion_marker_failure_approves_without_summary_or_later_feature_processing() {
    let s = HookSandbox::new();
    enable_power(&s);
    enable_reflection(&s);
    let path = transcript(&s, true);
    let resolved = state(&s, Some("completion-error"));
    write(
        resolved.join("power-steering/session_count"),
        r#"{"value":1}"#,
    );
    // A dangling symlink has exists() == false, so it reaches mark_complete;
    // its missing parent makes that write fail on macOS even for root users.
    symlink(
        s.root().join("absent-parent/marker"),
        resolved.join("power-steering/.completion-error_completed"),
    )
    .unwrap();
    init_repo(&s);
    let before = snapshot(&s);
    assert_eq!(
        invoke(hook(&s), Some("completion-error"), Some(&path)),
        json!({})
    );
    assert_counter(&resolved.join("power-steering/session_count"), 2);
    assert!(!resolved.join("power-steering/summary.md").exists());
    assert!(!resolved.join("reflection").exists());
    assert!(!s.root().join("absent-parent").exists());
    assert_eq!(snapshot(&s), before);
}

#[test]
fn presentation_write_failure_keeps_findings_and_allows_subsequent_reflection() {
    let s = HookSandbox::new();
    enable_reflection(&s);
    let path = transcript(&s, true);
    let resolved = state(&s, Some("presentation-error"));
    let reflection = resolved.join("reflection");
    fs::create_dir_all(&reflection).unwrap();
    let marker = reflection.join(".reflection_presented_presentation-error");
    symlink(s.root().join("absent-parent/marker"), &marker).unwrap();
    init_repo(&s);
    let before = snapshot(&s);
    for count in 1..=2 {
        let result = invoke(
            reflective_hook(&s, ""),
            Some("presentation-error"),
            Some(&path),
        );
        assert_eq!(result["decision"], "block");
        assert!(result["reason"].as_str().unwrap().contains(FINDINGS));
        assert_eq!(invocations(&s), count);
        assert_eq!(
            fs::read_to_string(reflection.join("current_findings.md")).unwrap(),
            FINDINGS
        );
        assert!(!marker.exists());
        assert!(
            fs::symlink_metadata(&marker)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    assert_eq!(snapshot(&s), before);
}
