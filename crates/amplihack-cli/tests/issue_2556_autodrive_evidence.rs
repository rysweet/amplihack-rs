//! #2556: auto-drive evidence with a `null` string or a stale head.
//!
//! On v1.2.149 `orch helper autodrive assess` and `round` exited non-zero on
//! `invalid type: null, expected a string at line 1 column 26` (a `null`
//! `attempts[0].cause`), and on `evidence head differs from current worktree`.
//! The Crusty loop was also skipped for an open PR whose Crusty round was not
//! complete at the current head, because the preflight set `should_run`
//! to `"false"` for a worktree that was merely behind the PR head.
//!
//! These tests run the real binary with temporary HOME and AMPLIHACK_HOME,
//! evaluate the recipes' real conditions with the recipe runner's evaluator,
//! and run the recipe's own `report-unaccepted-readiness` command. No agent,
//! hosting service, timer or sleep is used.

use crate::support::amplihack_bin::amplihack_bin;
use recipe_runner_rs::context::RecipeContext;
use serde_json::{Value, json};
use serde_yaml::Value as Yaml;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn recipe_text(name: &str) -> String {
    std::fs::read_to_string(
        root()
            .join("amplifier-bundle/recipes")
            .join(format!("{name}.yaml")),
    )
    .unwrap()
}

fn recipe(name: &str) -> Yaml {
    let text = recipe_text(name);
    recipe_runner_rs::parser::RecipeParser::new()
        .parse(&text)
        .unwrap_or_else(|e| panic!("{name} must parse with the recipe runner: {e:?}"));
    serde_yaml::from_str(&text).unwrap()
}

fn steps(doc: &Yaml) -> Vec<Yaml> {
    doc["steps"].as_sequence().unwrap().clone()
}

fn step<'a>(steps: &'a [Yaml], id: &str) -> &'a Yaml {
    steps
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing step {id}"))
}

fn ids(steps: &[Yaml]) -> Vec<String> {
    steps
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

fn text_of(s: &Yaml, key: &str) -> String {
    s[key].as_str().unwrap_or("").to_string()
}

// ---- the binary: exact failing input, exit 0 ---------------------------------

const POLICY: &str =
    "trusted-launch-bundle:sha256:0000000000000000000000000000000000000000000000000000000000000000";
const RUN_ID: &str = "8f3a2b1c-4d5e-4f60-8a7b-9c0d1e2f3a4b";

struct Drive {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    state: PathBuf,
    invocation: PathBuf,
    head: String,
    base: String,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let o = amplihack_git::command_in(repo)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

#[cfg(unix)]
fn private_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
impl Drive {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "feature"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["update-ref", "refs/remotes/origin/main", &base]);
        std::fs::write(repo.join("code.rs"), "fn implemented() {}\n").unwrap();
        git(&repo, &["add", "code.rs"]);
        git(&repo, &["commit", "-q", "-m", "implementation"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let state = tmp.path().join("state");
        private_dir(&state);
        let invocation_dir = tmp.path().join("invocation");
        private_dir(&invocation_dir);
        let invocation = invocation_dir.join("invocation.json");
        std::fs::write(
            &invocation,
            json!({
                "schema_version": 1, "selected_pr": null, "selected_url": null,
                "selected_branch": null, "policy_provenance": POLICY, "run_id": RUN_ID,
                "no_merge": true, "dry_run": false, "requested_action": "verify_readiness",
                "state_dir": state
            })
            .to_string(),
        )
        .unwrap();
        std::fs::set_permissions(&invocation, std::fs::Permissions::from_mode(0o600)).unwrap();
        for d in ["home", "amplihack-home", "tmp"] {
            private_dir(&tmp.path().join(d));
        }
        Drive {
            tmp,
            repo,
            state,
            invocation,
            head,
            base,
        }
    }

    /// The exact #2556 input: `attempts[0].cause` is `null` at column 26.
    fn null_cause_candidate(&self) -> String {
        let binding = json!({
            "base_branch": "main", "base_sha": self.base, "head_branch": "feature",
            "head_sha": self.head, "host": "github.example.test",
            "merge_base_sha": self.base, "platform": "github",
            "policy_provenance": POLICY, "pr_number": 2556,
            "pr_url": "https://github.example.test/example/project/pull/2556",
            "repository": "example/project", "round_id": "merge-1", "run_id": RUN_ID,
            "source_repository": "example/project"
        });
        let rest = json!({
            "binding": binding, "blockers": ["checks pending"], "criteria": [],
            "readiness": "incomplete", "review_range": null,
            "reviewed_head_sha": self.head, "schema_version": 1,
            "verified_head_sha": self.head
        })
        .to_string();
        let text = format!(
            r#"{{"attempts":[{{"cause":null,"operation":"gh pr view","outcome":"failed","recovery":null}}],{}"#,
            &rest[1..]
        );
        assert_eq!(&text[22..26], "null", "the incident column");
        text
    }

    fn run(&self, op: &str, extra: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["orch", "helper", "autodrive", op, "--invocation"])
            .arg(&self.invocation)
            .arg("--directory")
            .arg(&self.state)
            .arg("--repo")
            .arg(&self.repo)
            .args(extra)
            .current_dir(self.tmp.path())
            .env("HOME", self.tmp.path().join("home"))
            .env("AMPLIHACK_HOME", self.tmp.path().join("amplihack-home"))
            .env("TMPDIR", self.tmp.path().join("tmp"))
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

fn show(o: &Output) -> String {
    format!(
        "status {:?}\nstdout {}\nstderr {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", show(o)))
}

/// The #2556 serde message must never reach either stream.
fn assert_no_serde_null_error(o: &Output) {
    for stream in [&o.stdout, &o.stderr] {
        assert!(
            !String::from_utf8_lossy(stream).contains("invalid type: null, expected a string"),
            "{}",
            show(o)
        );
    }
}

fn has_blocker(out: &Value, blocker: &str) -> bool {
    out["blockers"]
        .as_array()
        .unwrap_or_else(|| panic!("blockers array: {out}"))
        .iter()
        .any(|b| b == blocker)
}

#[cfg(unix)]
#[test]
fn assess_exits_zero_on_the_exact_null_cause_input() {
    let d = Drive::new();
    let o = d.run("assess", &[], &d.null_cause_candidate());
    assert!(o.status.success(), "{}", show(&o));
    assert_no_serde_null_error(&o);
    let out = stdout_json(&o);
    assert_eq!(out["status"], "refresh_required", "{out}");
    assert_eq!(out["readiness"], "incomplete", "{out}");
    assert_eq!(out["accepted"], Value::Null, "{out}");
    assert!(
        out["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "unknown-field:attempts[0].cause"),
        "{out}"
    );
    assert!(
        !String::from_utf8_lossy(&o.stderr).contains("column 26"),
        "{}",
        show(&o)
    );
}

#[cfg(unix)]
#[test]
fn round_exits_zero_on_the_exact_null_cause_input() {
    let d = Drive::new();
    let record = d.state.join("merge-ready-round-1.json");
    let o = d.run(
        "round",
        &["--record", record.to_str().unwrap()],
        &d.null_cause_candidate(),
    );
    assert!(o.status.success(), "{}", show(&o));
    assert_no_serde_null_error(&o);
    let out = stdout_json(&o);
    assert_eq!(out["merge_ready_verdict"], "NOT_MERGE_READY", "{out}");
    assert_ne!(out["merge_ready_verdict"], "MISSING", "{out}");
    assert!(
        out["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "unknown-field:attempts[0].cause"),
        "{out}"
    );
    let findings = std::fs::read_to_string(format!("{}.findings", record.display())).unwrap();
    assert!(
        findings
            .lines()
            .any(|l| l == "unknown-field:attempts[0].cause"),
        "{findings}"
    );

    // The round recipe's next step reads this verdict and addresses the blocker.
    let round = steps(&recipe("autodrive-merge-round"));
    let condition = text_of(step(&round, "step-04-address-blockers"), "condition");
    let ctx = RecipeContext::new(HashMap::from([("merge_ready_verdict".into(), out)]));
    assert!(ctx.evaluate(&condition).unwrap(), "{condition}");
}

#[cfg(unix)]
#[test]
fn assess_exits_zero_with_refresh_required_for_an_older_evidence_head() {
    let d = Drive::new();
    let candidate = d
        .null_cause_candidate()
        .replace(r#""cause":null"#, r#""cause":"GraphQL refused""#);
    git(
        &d.repo,
        &["commit", "-q", "--allow-empty", "-m", "newer head"],
    );
    let newer = git(&d.repo, &["rev-parse", "HEAD"]);
    let o = d.run("assess", &[], &candidate);
    assert!(o.status.success(), "{}", show(&o));
    let out = stdout_json(&o);
    assert_eq!(out["status"], "refresh_required", "{out}");
    assert_eq!(out["refresh_required"], true, "{out}");
    assert_eq!(out["blockers"], json!(["stale-evidence-head"]), "{out}");
    assert_eq!(out["evidence_head"], d.head, "{out}");
    assert_eq!(out["current_head"], newer, "{out}");
    assert_eq!(out["current_base"], d.base, "{out}");
}

/// D6: the same candidate with `attempts[0].cause` removed, on an attempt
/// that did not succeed, is also a named blocker with exit 0.
#[cfg(unix)]
#[test]
fn assess_exits_zero_when_a_failed_attempt_has_no_cause() {
    let d = Drive::new();
    let candidate = d.null_cause_candidate().replace(r#""cause":null,"#, "");
    assert!(!candidate.contains(r#""cause""#), "{candidate}");
    assert!(candidate.contains(r#""outcome":"failed""#), "{candidate}");
    let o = d.run("assess", &[], &candidate);
    assert!(o.status.success(), "{}", show(&o));
    assert_no_serde_null_error(&o);
    let out = stdout_json(&o);
    assert_eq!(out["status"], "refresh_required", "{out}");
    assert_eq!(out["refresh_required"], true, "{out}");
    assert_eq!(out["accepted"], Value::Null, "{out}");
    assert!(
        has_blocker(&out, "unknown-field:attempts[0].cause"),
        "{out}"
    );
}

/// D3: a worktree whose HEAD cannot be read (no commits yet, on the branch
/// the binding names) is a recoverable fault, not a failed command.
#[cfg(unix)]
#[test]
fn assess_exits_zero_when_the_worktree_head_is_unreadable() {
    let mut d = Drive::new();
    let unborn = d.tmp.path().join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init", "-q", "-b", "feature"]);
    d.repo = unborn;
    let candidate = d
        .null_cause_candidate()
        .replace(r#""cause":null"#, r#""cause":"GraphQL refused""#);
    let o = d.run("assess", &[], &candidate);
    assert!(o.status.success(), "{}", show(&o));
    let out = stdout_json(&o);
    assert_eq!(out["status"], "refresh_required", "{out}");
    assert_eq!(out["blockers"], json!(["current-head-unreadable"]), "{out}");
    assert_eq!(out["current_head"], "", "{out}");
    assert_eq!(out["current_base"], "", "{out}");
    assert_eq!(out["evidence_head"], d.head, "{out}");
}

// ---- finalize: one remeasurement, then accept or report ----------------------

const AGENT_OUTPUT: &str = "{\"agent\":\"output\"}";

/// Walk finalize in step order, as the runner does: evaluate each condition
/// against the current context, then record the step's output. `assess`
/// results are taken in order from `assessments`.
fn walk_finalize(assessments: &[Value]) -> (Vec<String>, RecipeContext) {
    let doc = recipe("autodrive-finalize");
    let mut ctx = RecipeContext::new(HashMap::new());
    let mut remaining = assessments.iter();
    let mut ran = Vec::new();
    for s in steps(&doc) {
        let id = text_of(&s, "id");
        let condition = text_of(&s, "condition");
        if !condition.is_empty()
            && !ctx
                .evaluate(&condition)
                .unwrap_or_else(|e| panic!("{id}: {e:?}"))
        {
            continue;
        }
        let command = text_of(&s, "command");
        let value = if command.contains("autodrive assess") {
            remaining
                .next()
                .unwrap_or_else(|| panic!("{id}: an extra assessment"))
                .clone()
        } else {
            json!(AGENT_OUTPUT)
        };
        if let Some(output) = s["output"].as_str() {
            ctx.set(output, value);
        }
        ran.push(id);
    }
    (ran, ctx)
}

fn accepted() -> Value {
    json!({"status":"accepted","accepted":"/state/accepted-1.json","readiness":"merge_ready","refresh_required":false})
}

fn refresh(blocker: &str) -> Value {
    json!({"status":"refresh_required","accepted":null,"readiness":"incomplete","refresh_required":true,"evidence_head":"","current_head":"","current_base":"","blockers":[blocker]})
}

#[test]
fn finalize_has_one_remeasurement_between_assessment_and_action() {
    let doc = recipe("autodrive-finalize");
    let all = steps(&doc);
    let order = ids(&all);
    let pos = |id: &str| {
        order
            .iter()
            .position(|s| s == id)
            .unwrap_or_else(|| panic!("missing {id}: {order:?}"))
    };
    assert!(pos("accept-refreshed-readiness") < pos("remeasure-readiness"));
    assert!(pos("remeasure-readiness") < pos("accept-remeasured-readiness"));
    assert!(pos("accept-remeasured-readiness") < pos("finalize-observed-action"));
    assert!(pos("finalize-observed-action") < pos("reduce-observed-outcome"));
    assert!(pos("report-unaccepted-readiness") > pos("accept-remeasured-readiness"));
    assert_eq!(
        order.iter().filter(|s| s.contains("remeasure")).count(),
        2,
        "exactly one remeasurement and one second assessment: {order:?}"
    );
    let remeasure = step(&all, "remeasure-readiness");
    assert_eq!(text_of(remeasure, "type"), "agent");
    assert_eq!(remeasure["parse_json"].as_bool(), Some(false));
    let second = step(&all, "accept-remeasured-readiness");
    assert!(text_of(second, "command").contains("orch helper autodrive assess"));
    assert_eq!(second["parse_json"].as_bool(), Some(true));
    assert_eq!(text_of(second, "output"), "accepted_readiness");
    assert_eq!(
        text_of(step(&all, "report-unaccepted-readiness"), "output"),
        "autodrive_finalization"
    );
}

#[test]
fn finalize_acts_only_on_an_accepted_assessment() {
    let (ran, _) = walk_finalize(&[accepted()]);
    assert!(ran.contains(&"finalize-observed-action".into()), "{ran:?}");
    assert!(ran.contains(&"reduce-observed-outcome".into()), "{ran:?}");
    for skipped in [
        "remeasure-readiness",
        "accept-remeasured-readiness",
        "report-unaccepted-readiness",
    ] {
        assert!(!ran.contains(&skipped.into()), "{skipped}: {ran:?}");
    }

    let (ran, ctx) = walk_finalize(&[refresh("stale-evidence-head"), accepted()]);
    for expected in [
        "remeasure-readiness",
        "accept-remeasured-readiness",
        "finalize-observed-action",
        "reduce-observed-outcome",
    ] {
        assert!(ran.contains(&expected.into()), "{expected}: {ran:?}");
    }
    assert!(
        !ran.contains(&"report-unaccepted-readiness".into()),
        "{ran:?}"
    );
    assert_eq!(ctx.get("accepted_readiness.status").unwrap(), "accepted");

    let (ran, _) = walk_finalize(&[
        refresh("stale-evidence-head"),
        refresh("stale-evidence-head"),
    ]);
    assert!(
        ran.contains(&"report-unaccepted-readiness".into()),
        "{ran:?}"
    );
    assert!(!ran.contains(&"finalize-observed-action".into()), "{ran:?}");
    assert!(!ran.contains(&"reduce-observed-outcome".into()), "{ran:?}");
}

fn jq_available() -> bool {
    let ok = Command::new("jq")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skip: `jq` not available on PATH");
    }
    ok
}

#[test]
fn report_unaccepted_readiness_builds_json_from_blockers_as_data() {
    if !jq_available() {
        return;
    }
    let all = steps(&recipe("autodrive-finalize"));
    let command = text_of(step(&all, "report-unaccepted-readiness"), "command");
    assert!(command.contains("jq"), "{command}");
    assert!(!command.contains("eval"), "{command}");
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("pwned");
    let hostile = format!(
        "$(touch {}) `touch {}` \"; quote",
        marker.display(),
        marker.display()
    );
    let mut assessment = refresh("stale-evidence-head");
    assessment["blockers"] = json!(["stale-evidence-head", hostile]);
    let o = Command::new("bash")
        .arg("-c")
        .arg(&command)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("HOME", tmp.path())
        .env("TMPDIR", tmp.path())
        .env("RECIPE_VAR_accepted_readiness", assessment.to_string())
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    assert!(!marker.exists(), "blockers are never evaluated");
    let out = stdout_json(&o);
    assert_eq!(out["accepted_snapshot"], Value::Null, "{out}");
    assert_eq!(out["outcome"]["readiness"], "incomplete", "{out}");
    assert_eq!(out["outcome"]["blockers"], assessment["blockers"], "{out}");
}

// ---- the Crusty loop preflight ---------------------------------------------

#[test]
fn crusty_loop_runs_for_an_open_pr_with_an_incomplete_round() {
    let all = steps(&recipe("autodrive-crusty-loop"));
    for id in ["step-02-crusty-loop", "step-03-record-crusty-phase"] {
        assert_eq!(
            text_of(step(&all, id), "condition"),
            "crusty_loop_preflight.should_run == 'true'",
            "{id} keeps its condition"
        );
    }
    let condition = text_of(step(&all, "step-02-crusty-loop"), "condition");
    let decide = |preflight: Value| {
        RecipeContext::new(HashMap::from([("crusty_loop_preflight".into(), preflight)]))
            .evaluate(&condition)
            .unwrap()
    };
    // Open PR, worktree repaired to the PR head, no complete round there.
    assert!(decide(
        json!({"should_run":"true","skip_reason":"","pr_state":"OPEN"})
    ));
    for reason in ["merged", "complete-at-head", "worktree-diverged"] {
        assert!(
            !decide(json!({"should_run":"false","skip_reason":reason})),
            "{reason}"
        );
    }
}
