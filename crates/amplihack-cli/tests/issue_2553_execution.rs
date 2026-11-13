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
                "--locked",
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
    use std::os::unix::ffi::OsStringExt;
    let output = fixture
        .command("driver")
        .arg(fixture.config(json!([leaf])))
        .env(
            "ISSUE_2553_NON_UTF8",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .output()
        .unwrap();
    succeeded(&output);
    let executions = fixture.actions("execute");
    assert_eq!(executions.len(), 1, "must execute, not merely register");
    assert_eq!(executions[0]["recipe"], "operations-direct");
    assert_eq!(executions[0]["delegate"], "amplihack claude");
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
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-secret-token"));
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
fn oversized_tracking_acknowledgement_is_drained_and_reconciled_without_duplicate_creation() {
    let fixture = Fixture::new();
    let mut leaf = operations("tracked");
    leaf["create_issue"] = json!(true);
    let config = fixture.config(json!([leaf]));
    let output = fixture
        .command("driver")
        .arg(&config)
        .env("ISSUE_2553_TRACKING_MODE", "oversized")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("capture limit"));
    assert!(fixture.actions("execute").is_empty());
    assert!(
        !fixture.state("tracked")["tracking_reference"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    succeeded(&fixture.command("driver").arg(config).output().unwrap());
    assert_eq!(fixture.state("tracked")["issue"], 711);
    assert_eq!(fixture.actions("execute").len(), 1);
    assert_eq!(
        fixture
            .actions("gh")
            .iter()
            .filter(|entry| entry["args"][1] == "create")
            .count(),
        1
    );
}

#[test]
fn malformed_saved_tracking_reference_is_rejected_before_any_service_call() {
    let fixture = Fixture::new();
    let mut leaf = operations("unsafe-reference");
    leaf["create_issue"] = json!(true);
    let config = fixture.config(json!([leaf]));
    let first = fixture
        .command("driver")
        .arg(&config)
        .env("ISSUE_2553_TRACKING_MODE", "uncertain")
        .output()
        .unwrap();
    assert!(!first.status.success());
    let saved = fixture.state("unsafe-reference");
    let mut tampered = saved.clone();
    tampered["tracking_reference"] = json!("fixture-secret-token in:body repo:other/project");
    let path = fixture.state_dir().join("ws-unsafe-reference.json");
    fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    let rejected = fixture.command("driver").arg(&config).output().unwrap();
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("fixture-secret-token"));
    assert!(fixture.actions("execute").is_empty());
    assert_eq!(fixture.actions("gh").len(), 1);
    assert_eq!(
        fixture.state("unsafe-reference")["tracking_reference"],
        tampered["tracking_reference"]
    );
    fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
    succeeded(&fixture.command("driver").arg(config).output().unwrap());
    assert_eq!(fixture.actions("gh").len(), 2);
    assert_eq!(fixture.state("unsafe-reference")["issue"], 711);
}

#[test]
fn malformed_creation_acknowledgements_preserve_intent_and_reconcile_without_recreating() {
    for mode in ["zero", "absent", "untrusted"] {
        let fixture = Fixture::new();
        let mut leaf = operations("invalid-ack");
        leaf["create_issue"] = json!(true);
        let config = fixture.config(json!([leaf]));
        let first = fixture
            .command("driver")
            .arg(&config)
            .env("ISSUE_2553_TRACKING_MODE", mode)
            .output()
            .unwrap();
        assert!(!first.status.success(), "mode: {mode}");
        assert!(!String::from_utf8_lossy(&first.stderr).contains("fixture-secret-token"));
        assert!(fixture.actions("execute").is_empty());
        let saved = fixture.state("invalid-ack");
        assert!(saved["issue"].is_null());
        assert!(!saved["tracking_reference"].as_str().unwrap().is_empty());
        succeeded(&fixture.command("driver").arg(config).output().unwrap());
        assert_eq!(fixture.state("invalid-ack")["issue"], 711);
        assert_eq!(fixture.actions("gh").len(), 2, "one create and one lookup");
    }
}

#[test]
fn reconciliation_filters_search_results_by_exact_reference() {
    let fixture = Fixture::new();
    let mut leaf = operations("fuzzy-search");
    leaf["create_issue"] = json!(true);
    let config = fixture.config(json!([leaf]));
    let first = fixture
        .command("driver")
        .arg(&config)
        .env("ISSUE_2553_TRACKING_MODE", "uncertain")
        .output()
        .unwrap();
    assert!(!first.status.success());
    succeeded(
        &fixture
            .command("driver")
            .arg(config)
            .env("ISSUE_2553_TRACKING_MODE", "fuzzy")
            .output()
            .unwrap(),
    );
    assert_eq!(fixture.state("fuzzy-search")["issue"], 711);
    assert_eq!(fixture.actions("gh").len(), 2);
    assert_eq!(fixture.actions("completed").len(), 1);
}

#[test]
fn unresolved_reconciliation_blocks_execution_and_preserves_recovery_information() {
    for mode in [
        "duplicate",
        "empty",
        "saturated",
        "wrong-reference",
        "invalid-number",
        "malformed",
        "invalid-json-type",
        "failure",
    ] {
        let fixture = Fixture::new();
        let mut leaf = operations("unresolved");
        leaf["create_issue"] = json!(true);
        let config = fixture.config(json!([leaf]));
        let first = fixture
            .command("driver")
            .arg(&config)
            .env("ISSUE_2553_TRACKING_MODE", "uncertain")
            .output()
            .unwrap();
        assert!(!first.status.success());
        let saved = fixture.state("unresolved");
        let retry = fixture
            .command("driver")
            .arg(&config)
            .env("ISSUE_2553_TRACKING_MODE", mode)
            .output()
            .unwrap();
        assert!(!retry.status.success(), "mode: {mode}");
        assert!(!String::from_utf8_lossy(&retry.stderr).contains("fixture-secret-token"));
        assert!(fixture.actions("execute").is_empty());
        assert_eq!(fixture.actions("gh").len(), 2);
        assert_eq!(
            fixture.state("unresolved")["tracking_reference"],
            saved["tracking_reference"]
        );
        succeeded(&fixture.command("driver").arg(config).output().unwrap());
        assert_eq!(fixture.state("unresolved")["issue"], 711);
        assert_eq!(fixture.actions("gh").len(), 3, "one create and two lookups");
    }
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
    let mut unsupported = operations("unsupported");
    unsupported["classification"] = json!("Investigation");
    unsupported["recipe"] = json!("unknown-workflow");
    let mut cases = vec![
        json!([operations("valid"), unsupported]),
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
        ("timeout_policy", json!("ignore")),
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
    let mut dependent = operations("child");
    dependent["dependencies"] = json!(["cancelled"]);
    let mut grandchild = operations("grandchild");
    grandchild["dependencies"] = json!(["child"]);
    let mut child = fixture
        .command("driver")
        .arg(fixture.config(json!([grandchild, dependent, operations("cancelled")])))
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
    assert_eq!(fixture.state("child")["lifecycle_state"], "blocked");
    assert_eq!(fixture.state("grandchild")["lifecycle_state"], "blocked");
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

#[test]
fn development_recovery_is_bound_to_the_source_repository_and_worktree() {
    let fixture = Fixture::new();
    let mut leaf = operations("edit");
    leaf["task"] = json!("Update src/lib.rs.");
    leaf["classification"] = json!("Development");
    leaf["recipe"] = json!("default-workflow");
    let config = fixture.config(json!([leaf]));
    let run = || {
        fixture
            .command("driver")
            .arg(&config)
            .env("ISSUE_2553_DEVELOPMENT_KEY", "edit")
            .output()
            .unwrap()
    };
    succeeded(&run());
    succeeded(&run());
    assert_eq!(fixture.actions("execute").len(), 1);
    let saved = fixture.state("edit");
    let other = fixture.root.join("other-repository");
    fs::create_dir_all(other.join(".git")).unwrap();
    let output = fixture
        .command("driver")
        .arg(&config)
        .current_dir(&other)
        .env("ISSUE_2553_DEVELOPMENT_KEY", "edit")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "foreign repository adopted completed success"
    );
    assert_eq!(fixture.state("edit"), saved);
    assert_eq!(fixture.actions("execute").len(), 1);
    let linked_worktree = fixture
        .command("driver")
        .arg(&config)
        .current_dir(&other)
        .env(
            "ISSUE_2553_COMMON_DIR",
            fixture.root.join("repository/.git"),
        )
        .env("ISSUE_2553_DEVELOPMENT_KEY", "edit")
        .output()
        .unwrap();
    assert!(
        !linked_worktree.status.success(),
        "shared Git common directory must not hide a different source worktree"
    );
    assert_eq!(fixture.actions("execute").len(), 1);
}

#[test]
fn retained_atomic_temporary_files_do_not_prevent_recovery() {
    let fixture = Fixture::new();
    let plan = json!([operations("resume")]);
    succeeded(&fixture.run(plan.clone()));
    fs::write(
        fixture.state_dir().join("canonical-plan.tmp"),
        b"interrupted replacement",
    )
    .unwrap();
    succeeded(&fixture.run(plan));
    assert_eq!(fixture.actions("execute").len(), 1);
}

#[test]
fn transitive_blocking_is_independent_of_plan_order_and_failed_outcome() {
    for order in [[0, 1, 2, 3], [2, 1, 3, 0], [3, 2, 0, 1]] {
        for status in ["partial", "failed"] {
            let fixture = Fixture::new();
            let mut child = operations("child");
            child["dependencies"] = json!(["root"]);
            let mut grandchild = operations("grandchild");
            grandchild["dependencies"] = json!(["child"]);
            let mut branch = operations("branch");
            branch["dependencies"] = json!(["root", "child"]);
            let nodes = [operations("root"), child, grandchild, branch];
            let output = fixture
                .command("driver")
                .arg(fixture.config(Value::Array(
                    order.iter().map(|&i| nodes[i].clone()).collect(),
                )))
                .env("ISSUE_2553_AGENT_STATUS", status)
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert_eq!(fixture.actions("execute").len(), 1);
            for key in ["child", "grandchild", "branch"] {
                assert_eq!(
                    fixture.state(key)["lifecycle_state"],
                    "blocked",
                    "{order:?}: {key}"
                );
            }
        }
    }
}

#[test]
fn zero_runtime_budgets_preserve_event_driven_execution_and_dependency_results() {
    for policy in ["continue-preserve", "interrupt-preserve"] {
        let fixture = Fixture::new();
        let mut root = operations("root");
        root["max_runtime"] = json!(0);
        root["timeout_policy"] = json!(policy);
        let mut dependent = operations("dependent");
        dependent["dependencies"] = json!(["root"]);
        succeeded(&fixture.run(json!([dependent, root])));
        assert_eq!(fixture.state("root")["lifecycle_state"], "completed");
        assert_eq!(fixture.state("dependent")["lifecycle_state"], "completed");
        let executions = fixture.actions("execute");
        assert_eq!(executions.len(), 2);
        assert_eq!(
            executions[1]["context"]["prerequisite_results"]["root"]["operations_result"],
            "OPERATIONS_RESULT: completed — fixture answer"
        );
    }
}

#[test]
fn failed_checkpoint_write_retains_committed_state_and_prevents_next_launch() {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let mut dependent = operations("dependent");
    dependent["dependencies"] = json!(["root"]);
    let socket = fixture.root.join("r");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let child = fixture
        .command("driver")
        .arg(fixture.config(json!([
            operations("root"),
            dependent,
            operations("independent")
        ])))
        .env("ISSUE_2553_READY_SOCKET", &socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (mut connection, _) = listener.accept().unwrap();
    let mut ready = [0; 5];
    connection.read_exact(&mut ready).unwrap();
    let path = fixture.state_dir().join("ws-root.json");
    let committed = fs::read(&path).unwrap();
    fs::set_permissions(fixture.state_dir(), fs::Permissions::from_mode(0o500)).unwrap();
    connection.write_all(b"x").unwrap();
    let output = child.wait_with_output().unwrap();
    fs::set_permissions(fixture.state_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(path).unwrap(), committed);
    assert_eq!(fixture.actions("execute").len(), 1);
    assert_ne!(fixture.state("root")["lifecycle_state"], "completed");
    assert_ne!(fixture.state("dependent")["lifecycle_state"], "completed");
    assert_eq!(fixture.state("independent")["lifecycle_state"], "pending");
}

#[test]
fn corrupt_saved_routes_and_legacy_identity_fail_before_any_further_execution() {
    for field in [
        "recipe",
        "source_repository",
        "dependencies",
        "classification",
    ] {
        let fixture = Fixture::new();
        let leaf = operations("saved");
        succeeded(&fixture.run(json!([leaf.clone()])));
        let mut saved = fixture.state("saved");
        if field == "source_repository" {
            saved.as_object_mut().unwrap().remove(field);
        } else {
            saved[field] = match field {
                "dependencies" => json!(["saved"]),
                _ => json!("untrusted-route"),
            };
        }
        fs::write(
            fixture.state_dir().join("ws-saved.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        assert!(
            !fixture.run(json!([leaf])).status.success(),
            "accepted corrupt {field}"
        );
        assert_eq!(fixture.actions("execute").len(), 1);
        assert_eq!(fixture.state("saved"), saved);
    }
}

#[test]
fn cleanup_rejects_changed_ownership_and_symlink_substitution() {
    for substitution in ["marker", "directory", "foreign-repository"] {
        let fixture = Fixture::new();
        let leaf = operations("owned");
        succeeded(&fixture.run(json!([leaf.clone()])));
        let marker = fixture.state_dir().join("ws-owned.owner.json");
        let directory = fixture.root.join("runtime/amplihack-workstreams/ws-owned");
        match substitution {
            "marker" => {
                fs::remove_file(&marker).unwrap();
                std::os::unix::fs::symlink(fixture.root.join("repository/keep.txt"), &marker)
                    .unwrap();
            }
            "directory" => {
                fs::remove_dir_all(&directory).unwrap();
                std::os::unix::fs::symlink(fixture.root.join("repository"), &directory).unwrap();
            }
            _ => {
                let mut owner: Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
                owner["source_repository"]["worktree"] = json!("/foreign");
                fs::write(marker, serde_json::to_vec(&owner).unwrap()).unwrap();
            }
        }
        let output = fixture
            .command("cleanup")
            .arg(fixture.config(json!([leaf])))
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {substitution}");
        fixture.assert_preserved();
        assert!(fixture.actions("gh").is_empty());
    }
}

#[test]
fn cancellation_with_a_zero_runtime_budget_reaps_the_child_and_blocks_dependencies() {
    use std::io::Read;
    let fixture = Fixture::new();
    let mut root = operations("root");
    root["max_runtime"] = json!(0);
    root["timeout_policy"] = json!("continue-preserve");
    let mut dependent = operations("dependent");
    dependent["dependencies"] = json!(["root"]);
    let socket = fixture.root.join("r");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut child = fixture
        .command("driver")
        .arg(fixture.config(json!([dependent, root])))
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
    assert_eq!(
        fixture.state("root")["lifecycle_state"],
        "interrupted_resumable"
    );
    assert_eq!(fixture.state("dependent")["lifecycle_state"], "blocked");
    assert!(fixture.state("root")["last_pid"].is_null());
    assert_eq!(fixture.actions("execute").len(), 1);
}

#[test]
fn data_and_parent_environment_cannot_override_validated_routes_or_run_shell_commands() {
    let fixture = Fixture::new();
    let sentinel = fixture.root.join("injected");
    let mut first = operations("first");
    first["task"] = json!(format!(
        "Report status in chat; $(touch '{}')",
        sentinel.display()
    ));
    let mut second = operations("second");
    second["dependencies"] = json!(["first"]);
    let output = fixture
        .command("driver")
        .arg(fixture.config(json!([first, second])))
        .env("RECIPE_VAR_recipe", "default-workflow")
        .env("RECIPE_VAR_create_issue", "true")
        .env("AMPLIHACK_CREATE_ISSUE", "true")
        .env("AMPLIHACK_WORKSTREAM_ISSUE", "999")
        .env("GITHUB_TOKEN", "private-fixture-credential-2553")
        .output()
        .unwrap();
    succeeded(&output);
    assert!(!sentinel.exists());
    assert!(fixture.actions("gh").is_empty());
    for action in fixture.actions("execute") {
        assert_eq!(action["recipe"], "operations-direct");
        assert!(action["inherited_recipe"].is_null());
        assert!(action["inherited_issue"].is_null());
        assert!(action["inherited_create_issue"].is_null());
        assert_eq!(action["context"]["create_issue"], "false");
    }
    assert_eq!(fixture.state("second")["recipe"], "operations-direct");
    for entry in fs::read_dir(fixture.state_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert!(
                !fs::read_to_string(path)
                    .unwrap()
                    .contains("private-fixture-credential-2553")
            );
        }
    }
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-fixture-credential-2553"));
}

#[test]
fn runtime_metadata_survives_successful_completion_under_both_policies() {
    for policy in ["continue-preserve", "interrupt-preserve"] {
        let fixture = Fixture::new();
        let mut leaf = operations("quick");
        leaf["max_runtime"] = json!(10);
        leaf["timeout_policy"] = json!(policy);
        succeeded(&fixture.run(json!([leaf])));
        assert_eq!(fixture.state("quick")["lifecycle_state"], "completed");
        assert_eq!(fixture.state("quick")["last_exit_code"], 0);
        assert_eq!(fixture.state("quick")["max_runtime"], 10);
        assert_eq!(fixture.state("quick")["timeout_policy"], policy);
    }
}

#[test]
fn mixed_actions_keep_the_complete_task_and_use_a_route_that_permits_the_edit() {
    for task in [
        "Write the research findings into docs/report.md and post an issue comment.",
        "Post an issue comment and write the research findings into docs/report.md.",
        "Create an issue to add tests for src/router.rs and assign it to Alex.",
    ] {
        let fixture = Fixture::new();
        let mut leaf = operations("edit");
        leaf["task"] = json!(task);
        let output = fixture
            .command("driver")
            .arg(fixture.config(json!([leaf])))
            .env("ISSUE_2553_DEVELOPMENT_KEY", "edit")
            .output()
            .unwrap();
        succeeded(&output);
        let executions = fixture.actions("execute");
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0]["context"]["task_description"], task);
        assert_eq!(
            executions[0]["recipe"],
            if task.starts_with("Create") {
                "operations-direct"
            } else {
                "default-workflow"
            }
        );
    }
}

#[test]
fn credential_bearing_origins_are_rejected_before_git_arguments_or_diagnostics_can_expose_them() {
    let fixture = Fixture::new();
    let mut leaf = operations("edit");
    leaf["task"] = json!("Update src/lib.rs.");
    let output = fixture
        .command("driver")
        .arg(fixture.config(json!([leaf])))
        .env(
            "ISSUE_2553_ORIGIN",
            "https://user:private-origin-token@github.com/test/repo.git",
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(fixture.actions("execute").is_empty());
    assert!(!fixture.actions("git").iter().any(|action| {
        action["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "clone" || arg == "ls-remote")
    }));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-origin-token"));
}

#[test]
fn cleanup_service_errors_do_not_expose_credentials_or_remove_artefacts() {
    let fixture = Fixture::new();
    let mut leaf = operations("edit");
    leaf["task"] = json!("Update src/lib.rs.");
    let config = fixture.config(json!([leaf]));
    succeeded(
        &fixture
            .command("driver")
            .arg(&config)
            .env("ISSUE_2553_DEVELOPMENT_KEY", "edit")
            .output()
            .unwrap(),
    );
    let output = fixture.command("cleanup").arg(config).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fixture.actions("gh").len(), 1);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-secret-token"));
    assert!(
        fixture
            .root
            .join("runtime/amplihack-workstreams/ws-edit")
            .is_dir()
    );
    fixture.assert_preserved();
}
