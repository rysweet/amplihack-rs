//! #2553 execution contracts. These deliberately go beyond registration:
//! observable child completion, service actions and persisted results are the
//! oracle. No clock windows, polling, shared profiles or production services.

#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

fn fixture_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let output = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "-p",
                "amplihack-cli",
                "--example",
                "issue_2553_fixture_agent",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("build Rust fixture agent");
        assert!(
            output.status.success(),
            "fixture build: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/issue_2553_fixture_agent")
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for directory in [
            "home",
            "claude",
            "codex",
            "profiles",
            "runtime",
            "bin",
            "repository/.git",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(
            root.join("repository/keep.txt"),
            b"caller-owned repository\n",
        )
        .unwrap();
        for name in ["git", "gh", "amplihack"] {
            std::os::unix::fs::symlink(fixture_binary(), root.join("bin").join(name)).unwrap();
        }
        Self { _temp: temp, root }
    }

    fn command(&self, action: &str) -> Command {
        let mut command = self.isolated_command(fixture_binary());
        command.arg(action);
        command
    }

    fn external(&self, name: &str) -> Command {
        self.isolated_command(&self.root.join("bin").join(name))
    }

    fn isolated_command(&self, executable: &Path) -> Command {
        let mut command = crate::isolated_amplihack::isolated(executable);
        command
            .current_dir(self.root.join("repository"))
            .env("ISSUE_2553_ROOT", &self.root)
            .env("HOME", self.root.join("home"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CODEX_HOME", self.root.join("codex"))
            .env("AMPLIHACK_HOME", self.root.join("home/.amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.root.join("profiles"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("XDG_STATE_HOME", self.root.join("home/.state"))
            .env("TMPDIR", self.root.join("runtime"))
            .env(
                "PATH",
                std::env::join_paths([
                    self.root.join("bin"),
                    PathBuf::from("/usr/bin"),
                    PathBuf::from("/bin"),
                ])
                .unwrap(),
            )
            .env("AMPLIHACK_MAX_PARALLEL_WORKSTREAMS", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1");
        command
    }

    fn config(&self, leaves: Value) -> PathBuf {
        let path = self.root.join("workstreams.json");
        fs::write(&path, serde_json::to_vec(&leaves).unwrap()).unwrap();
        path
    }

    fn run(&self, leaves: Value) -> Output {
        self.command("driver")
            .arg(self.config(leaves))
            .output()
            .unwrap()
    }

    fn actions(&self, kind: &str) -> Vec<Value> {
        fs::read_to_string(self.root.join("actions.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|action| action["action"] == kind)
            .collect()
    }

    fn state(&self, key: &str) -> Value {
        let path = self.state_dir().join(format!("ws-{key}.json"));
        serde_json::from_slice(
            &fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
        )
        .unwrap()
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join("runtime/amplihack-workstreams/state")
    }

    fn assert_preserved(&self) {
        assert_eq!(
            fs::read(self.root.join("repository/keep.txt")).unwrap(),
            b"caller-owned repository\n"
        );
        assert!(self.root.join("repository/.git").is_dir());
    }
}

fn operations(key: &str) -> Value {
    json!({"key": key, "branch": format!("feat/fixture-{key}"), "description": key,
        "task": format!("Report {key} status in chat."), "classification": "Operations",
        "recipe": "operations-direct", "create_issue": false, "dependencies": []})
}

fn succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "production execution failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fixture_agent_and_tracking_adapter_are_executable_in_the_private_environment() {
    let fixture = Fixture::new();
    let context = fixture.root.join("context.json");
    fs::write(&context, br#"{"task_description":"Fixture self-check"}"#).unwrap();
    let output = fixture
        .external("amplihack")
        .args(["recipe", "run", "operations-direct", "--context-file"])
        .arg(context)
        .output()
        .unwrap();
    succeeded(&output);
    assert_eq!(fixture.actions("completed").len(), 1);
    succeeded(
        &fixture
            .external("gh")
            .args(["issue", "create", "--title", "Fixture only"])
            .output()
            .unwrap(),
    );
    assert_eq!(fixture.actions("gh").len(), 1);
    assert!(fixture.root.join("created-issue.json").is_file());
}

#[test]
fn an_operations_led_mixed_plan_executes_every_leaf_in_dependency_order_at_capacity_one() {
    let fixture = Fixture::new();
    let mut edit = operations("edit");
    edit["task"] = json!("Edit docs/routing.md using the research answers.");
    edit["classification"] = json!("Development");
    edit["recipe"] = json!("default-workflow");
    edit["dependencies"] = json!(["research"]);
    let mut coordination = operations("cross-check");
    coordination["dependencies"] = json!(["edit"]);
    let config = fixture.config(json!([edit, coordination, operations("research")]));
    let output = fixture
        .command("driver")
        .arg(config)
        .env("ISSUE_2553_DEVELOPMENT_KEY", "edit")
        .output()
        .unwrap();
    succeeded(&output);
    let executions = fixture.actions("execute");
    assert_eq!(
        executions.len(),
        3,
        "capacity limits must preserve every leaf"
    );
    assert_eq!(
        executions
            .iter()
            .map(|entry| entry["recipe"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["operations-direct", "default-workflow", "operations-direct"]
    );
    assert_eq!(
        executions[1]["context"]["prerequisite_results"]["research"]["operations_result"],
        "OPERATIONS_RESULT: completed — fixture answer"
    );
    assert!(
        executions[1]["context"]["task_description"]
            .as_str()
            .unwrap()
            .contains("fixture answer"),
        "the Development agent must receive prerequisite answers in its task prompt"
    );
    assert_eq!(
        fixture
            .actions("git")
            .iter()
            .filter(|entry| entry["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == "clone"))
            .count(),
        1
    );
    for key in ["research", "edit", "cross-check"] {
        assert_eq!(fixture.state(key)["lifecycle_state"], "completed");
    }
    fixture.assert_preserved();
}

#[test]
fn pure_operations_overrides_development_recipe_before_any_development_setup() {
    let fixture = Fixture::new();
    let mut leaf = operations("triage");
    leaf["recipe"] = json!("default-workflow");
    succeeded(&fixture.run(json!([leaf])));
    let executions = fixture.actions("execute");
    assert_eq!(executions.len(), 1, "must execute, not merely register");
    assert_eq!(executions[0]["recipe"], "operations-direct");
    assert_eq!(executions[0]["cwd"], json!(fixture.root.join("repository")));
    assert!(!fixture.actions("git").iter().any(|action| {
        action["args"].as_array().unwrap().iter().any(|arg| {
            matches!(
                arg.as_str(),
                Some("clone" | "worktree" | "checkout" | "switch" | "ls-remote")
            )
        })
    }));
    assert!(fixture.actions("gh").is_empty());
    assert_eq!(fixture.actions("completed").len(), 1);
    assert_eq!(fixture.state("triage")["lifecycle_state"], "completed");
    let canonical = fixture.state_dir().join("canonical-plan.json");
    let plan: Value =
        serde_json::from_slice(&fs::read(&canonical).expect("canonical plan survives execution"))
            .unwrap();
    assert_eq!(plan[0]["key"], "triage");
    assert_eq!(plan[0]["classification"], "Operations");
    assert_eq!(plan[0]["recipe"], "operations-direct");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(canonical).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fixture.assert_preserved();
}

#[test]
fn every_supported_absence_form_reaches_observable_completion_without_creating_tracking() {
    let mut cases = vec![
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!(" \t ")),
    ];
    cases.extend(
        [
            "TBD",
            " tBc ",
            "TODO",
            "N/A",
            "none",
            "null",
            "Unknown",
            "unassigned",
            "PENDING",
        ]
        .map(|value| Some(json!(value))),
    );
    for issue in cases {
        let fixture = Fixture::new();
        let mut leaf = operations("untracked");
        if let Some(value) = issue {
            leaf["issue"] = value;
        }
        succeeded(&fixture.run(json!([leaf])));
        assert_eq!(fixture.actions("completed").len(), 1);
        assert!(fixture.actions("gh").is_empty());
        let state = fixture.state("untracked");
        assert!(state.get("issue").is_none_or(Value::is_null));
        assert_eq!(state["key"], "untracked");
    }
}

#[test]
fn valid_tracking_keeps_legacy_numeric_identity_and_never_acquires_a_second_issue() {
    let fixture = Fixture::new();
    let mut leaf = operations("2553");
    leaf["issue"] = json!("#2553");
    leaf["create_issue"] = json!(true);
    succeeded(&fixture.run(json!([leaf])));
    assert_eq!(fixture.state("2553")["issue"], 2553);
    assert_eq!(
        fixture.actions("execute")[0]["context"]["issue_number"],
        2553
    );
    assert!(fixture.actions("gh").is_empty());
}

#[test]
fn explicit_tracking_is_acquired_and_persisted_before_execution_without_changing_identity() {
    let fixture = Fixture::new();
    let mut leaf = operations("tracked-later");
    leaf["create_issue"] = json!(true);
    leaf["issue"] = json!("TBD");
    succeeded(&fixture.run(json!([leaf])));
    assert_eq!(fixture.actions("gh").len(), 1);
    assert_eq!(
        fixture.actions("execute")[0]["context"]["issue_number"],
        711
    );
    assert_eq!(
        fixture.actions("execute")[0]["persisted_state"]["issue"],
        711,
        "tracking must be durable before the affected agent starts"
    );
    let state = fixture.state("tracked-later");
    assert_eq!(state["key"], "tracked-later");
    assert_eq!(state["issue"], 711);
    assert_eq!(state["lifecycle_state"], "completed");
}

#[test]
fn failed_tracking_acquisition_prevents_the_affected_leaf_from_executing() {
    let fixture = Fixture::new();
    let mut leaf = operations("tracking-failure");
    leaf["create_issue"] = json!(true);
    let output = fixture
        .command("driver")
        .arg(fixture.config(json!([leaf])))
        .env("ISSUE_2553_TRACKING_MODE", "failure")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fixture.actions("gh").len(),
        1,
        "failure must come from the acquisition boundary"
    );
    assert!(fixture.actions("execute").is_empty());
    assert!(fixture.state_dir().is_dir(), "retain recovery information");
}

#[test]
fn uncertain_acquisition_is_reconciled_on_resume_without_duplicate_creation() {
    let fixture = Fixture::new();
    let mut leaf = operations("uncertain");
    leaf["create_issue"] = json!(true);
    let config = fixture.config(json!([leaf]));
    let first = fixture
        .command("driver")
        .arg(&config)
        .env("ISSUE_2553_TRACKING_MODE", "uncertain")
        .output()
        .unwrap();
    assert!(!first.status.success());
    assert!(
        fixture.root.join("created-issue.json").is_file(),
        "exercise lost acknowledgement, not a pre-service failure"
    );
    succeeded(&fixture.command("driver").arg(config).output().unwrap());
    let creates = fixture
        .actions("gh")
        .into_iter()
        .filter(|action| action["args"][1] == "create")
        .count();
    assert_eq!(creates, 1);
    assert_eq!(fixture.state("uncertain")["issue"], 711);
    assert_eq!(fixture.actions("completed").len(), 1);
}

#[test]
fn completed_issue_less_work_is_not_reexecuted_after_resumption_or_tracking_attachment() {
    let fixture = Fixture::new();
    let leaf = operations("stable");
    succeeded(&fixture.run(json!([leaf.clone()])));
    let original = fixture.state("stable");
    let mut attached = leaf;
    attached["issue"] = json!(2553);
    succeeded(&fixture.run(json!([attached])));
    let resumed = fixture.state("stable");
    assert_eq!(resumed["key"], original["key"]);
    assert_eq!(resumed["work_dir"], original["work_dir"]);
    assert_eq!(fixture.actions("completed").len(), 1);
}

#[test]
fn process_failure_cannot_be_reported_as_success_because_the_agent_printed_completed() {
    let fixture = Fixture::new();
    let output = fixture
        .command("driver")
        .arg(fixture.config(json!([operations("bad-exit")])))
        .env("ISSUE_2553_AGENT_EXIT", "17")
        .output()
        .unwrap();
    assert_eq!(
        fixture.actions("execute").len(),
        1,
        "must exercise child exit handling"
    );
    assert!(!output.status.success());
    assert_ne!(fixture.state("bad-exit")["lifecycle_state"], "completed");
}

#[test]
fn partial_operations_outcome_is_retained_and_blocks_dependants() {
    let fixture = Fixture::new();
    let mut dependent = operations("dependent");
    dependent["dependencies"] = json!(["partial"]);
    let output = fixture
        .command("driver")
        .arg(fixture.config(json!([operations("partial"), dependent])))
        .env("ISSUE_2553_AGENT_STATUS", "partial")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fixture.actions("execute").len(),
        1,
        "dependent must not run after partial work"
    );
    assert_eq!(fixture.state("partial")["lifecycle_state"], "partial");
    assert_eq!(fixture.state("dependent")["lifecycle_state"], "blocked");
}

#[test]
fn invalid_plans_produce_no_external_actions_even_when_the_first_leaf_is_valid() {
    let mut cycle_a = operations("cycle-a");
    let mut cycle_b = operations("cycle-b");
    cycle_a["dependencies"] = json!(["cycle-b"]);
    cycle_b["dependencies"] = json!(["cycle-a"]);
    let mut cases = vec![
        json!([operations("duplicate"), operations("duplicate")]),
        json!([cycle_a, cycle_b]),
    ];
    for (field, value) in [
        ("task", json!(" \n ")),
        ("issue", json!(0)),
        ("key", json!("../escape")),
        ("dependencies", json!(["missing"])),
        ("classification", json!("unsupported-route")),
        ("create_issue", json!("perhaps")),
    ] {
        let mut invalid = operations("invalid");
        invalid[field] = value;
        cases.push(json!([operations("valid"), invalid]));
    }
    for plan in cases {
        let fixture = Fixture::new();
        let output = fixture.run(plan.clone());
        assert!(!output.status.success(), "accepted invalid plan {plan}");
        for action in ["git", "gh", "execute"] {
            assert!(
                fixture.actions(action).is_empty(),
                "{action} before complete validation of {plan}"
            );
        }
    }
}

#[test]
fn cleanup_preserves_an_unowned_directory_even_under_the_runtime_root() {
    let fixture = Fixture::new();
    // Legacy numeric path, so identity derivation cannot hide a deletion bug.
    let foreign = fixture.root.join("runtime/amplihack-workstreams/ws-2553");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("keep.txt"), b"another run owns this").unwrap();
    let mut leaf = operations("2553");
    leaf["issue"] = json!(2553);
    let output = fixture
        .command("cleanup")
        .arg(fixture.config(json!([leaf])))
        .output()
        .unwrap();
    assert!(
        foreign.join("keep.txt").is_file(),
        "cleanup deleted unowned data: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    fixture.assert_preserved();
}

#[test]
fn a_symlinked_runtime_directory_cannot_escape_to_the_callers_repository() {
    let fixture = Fixture::new();
    let base = fixture.root.join("runtime/amplihack-workstreams");
    fs::create_dir_all(&base).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("repository"), base.join("ws-2553")).unwrap();
    let mut leaf = operations("2553");
    leaf["issue"] = json!(2553);
    let output = fixture.run(json!([leaf]));
    assert!(!output.status.success());
    assert!(fixture.actions("execute").is_empty());
    fixture.assert_preserved();
}

#[test]
fn cancellation_after_child_readiness_preserves_state_without_a_clock_window() {
    use std::io::Read;
    let fixture = Fixture::new();
    let socket = fixture.root.join("r");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut child = fixture
        .command("driver")
        .arg(fixture.config(json!([operations("cancelled")])))
        .env("ISSUE_2553_READY_SOCKET", &socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (mut connection, _) = listener.accept().unwrap();
    let mut ready = [0; 5];
    connection.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready");
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    assert!(!child.wait().unwrap().success());
    let state = fixture.state("cancelled");
    assert_eq!(state["lifecycle_state"], "interrupted_resumable");
    assert_eq!(fixture.actions("completed").len(), 0);
    assert!(
        fixture
            .root
            .join("runtime/amplihack-workstreams/ws-cancelled")
            .is_dir()
    );
    fixture.assert_preserved();
}

#[test]
fn an_unproven_saved_live_process_is_preserved_and_never_adopted_or_relaunched() {
    let fixture = Fixture::new();
    let leaf = operations("live");
    succeeded(&fixture.run(json!([leaf.clone()])));
    let mut saved = fixture.state("live");
    saved["lifecycle_state"] = json!("running");
    saved["last_pid"] = json!(std::process::id());
    saved["last_exit_code"] = Value::Null;
    fs::write(
        fixture.state_dir().join("ws-live.json"),
        serde_json::to_vec(&saved).unwrap(),
    )
    .unwrap();
    let output = fixture.run(json!([leaf]));
    assert!(!output.status.success());
    assert_eq!(fixture.actions("execute").len(), 1);
    assert_eq!(fixture.state("live"), saved);
    fixture.assert_preserved();
}

#[test]
fn runner_json_provides_results_without_a_sidecar_and_failure_overrides_completed_prose() {
    for failure in [false, true] {
        let fixture = Fixture::new();
        let mut command = fixture.command("driver");
        command
            .arg(fixture.config(json!([operations("runner")])))
            .env("ISSUE_2553_NO_SIDECAR", "1");
        if failure {
            command.env("ISSUE_2553_RUNNER_FAILURE", "1");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.success(), !failure);
        assert_eq!(fixture.actions("execute").len(), 1);
        assert_eq!(
            fixture.state("runner")["lifecycle_state"],
            if failure {
                "failed_terminal"
            } else {
                "completed"
            }
        );
    }
}

#[test]
fn owned_operations_cleanup_keeps_completion_and_never_repeats_external_actions() {
    let fixture = Fixture::new();
    let leaf = operations("clean");
    succeeded(&fixture.run(json!([leaf.clone()])));
    succeeded(
        &fixture
            .command("cleanup")
            .arg(fixture.config(json!([leaf.clone()])))
            .output()
            .unwrap(),
    );
    assert!(
        !fixture
            .root
            .join("runtime/amplihack-workstreams/ws-clean")
            .exists()
    );
    assert_eq!(fixture.state("clean")["lifecycle_state"], "completed");
    succeeded(&fixture.run(json!([leaf])));
    assert_eq!(fixture.actions("completed").len(), 1);
    assert!(fixture.actions("gh").is_empty());
    fixture.assert_preserved();
}
