//! Issues #1887 and #1900: the caller's `issue_number`, `create_issue` and
//! `remote_host_type` reach workflow-prep's step-03 when the run starts at
//! `smart-orchestrator.yaml`, on the single-workstream path and on the
//! parallel path.
//!
//! The shipped `smart-orchestrator.yaml` and `smart-execute-routing.yaml` run
//! unchanged through `recipe-runner-rs`. Only the parts that need a model are
//! stubbed, in a recipe directory searched with `--no-default-search`:
//!
//! - `smart-classify-route`: bash steps that print a fixed classification,
//!   decomposition and session status;
//! - `smart-reflect-loop`, `smart-validate-summarize`: one no-op bash step;
//! - `default-workflow`: the shipped workflow-prep `step-02d`, `step-03` and
//!   `step-03b` bodies, copied verbatim, plus a step that records what step-03
//!   decided.
//!
//! On the parallel path the real `amplihack orch helper
//! build-workstreams-config` and `amplihack orch run` build and launch the
//! workstreams; each clones a local bare origin and runs its `launcher.sh`.
//! A shim named `amplihack` first on PATH passes everything to the real
//! binary except `recipe run`, which it hands to `recipe-runner-rs` with the
//! workstream's own `context.json`, so each workstream runs the stubbed
//! `default-workflow` exactly as `amplihack recipe run` would hand it over.
//!
//! A fake `gh` logs every call. A control case without `create_issue=false`
//! shows that the harness does see `gh issue create` when step-03 makes one,
//! so the "no issue create" assertions are not vacuous.
//!
//! Needs `recipe-runner-rs` (CI installs the pinned one). Without it the test
//! is skipped, unless `AMPLIHACK_1900_REQUIRE_RECIPE_RUNNER` is set.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_yaml::{Mapping, Value};

use crate::isolated_amplihack::isolated;

const FAKE_REPO_URL: &str = "https://github.com/example-org/example-repo";
const CALLER_ISSUE: &str = "368";

/// Fake `gh`: logs each call; issue 368 exists and is open (`issue view`,
/// `api repos/.../issues/368`);
/// `issue list` finds nothing; `issue create` succeeds with issue 901.
const FAKE_GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$1:$2" in
  api:repos/*)
    n="${2##*/}"
    if [ "$n" = "368" ]; then printf 'open\tfalse\t%s/issues/%s\n' "$FAKE_REPO_URL" "$n"; exit 0; fi
    echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
  issue:view)
    if [ "$3" = "368" ]; then printf '%s/issues/368\n' "$FAKE_REPO_URL"; exit 0; fi
    echo "GraphQL: Could not resolve to an issue" >&2; exit 1 ;;
  issue:list) exit 0 ;;
  label:create) exit 0 ;;
  issue:create) printf '%s/issues/901\n' "$FAKE_REPO_URL"; exit 0 ;;
  pr:list) printf '[]\n'; exit 0 ;;
esac
echo "fake gh: unexpected invocation: $*" >&2
exit 7
"#;

/// `amplihack` shim: `recipe run NAME --context-file F` runs the stubbed
/// recipe through recipe-runner-rs with F's keys as `--set` values and logs
/// F; everything else goes to the real binary.
const AMPLIHACK_SHIM: &str = r#"#!/bin/bash
set -euo pipefail
if [ "${1:-}" = recipe ] && [ "${2:-}" = run ]; then
  name="$3"; shift 3; ctx=""
  while [ $# -gt 0 ]; do
    case "$1" in --context-file) ctx="$2"; shift 2 ;; *) shift ;; esac
  done
  [ -n "$ctx" ] || { echo "shim: no --context-file" >&2; exit 9; }
  cp "$ctx" "$WS_CONTEXT_DIR/$(basename "$PWD").json"
  args=()
  while IFS= read -r -d '' kv; do args+=(--set "$kv"); done < <(
    jq -j 'to_entries[] | "\(.key)=\(.value | if type == "string" then . else tojson end)\u0000"' "$ctx")
  exec recipe-runner-rs "$STUB_RECIPES/$name.yaml" --no-default-search -R "$STUB_RECIPES" \
    -C "$PWD" --no-auto-stage --no-update-check --set "repo_path=$PWD" "${args[@]}"
fi
exec "$REAL_AMPLIHACK" "$@"
"#;

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn recipes_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle").join("recipes")
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("write executable");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

/// The absolute path of `recipe-runner-rs`, or `None` (test skipped).
fn find_runner() -> Option<PathBuf> {
    let candidate = std::env::var_os("RECIPE_RUNNER_QA_BINARY")
        .map(PathBuf::from)
        .or_else(|| {
            let out = Command::new("sh")
                .args(["-c", "command -v recipe-runner-rs"])
                .output()
                .ok()?;
            let p = String::from_utf8(out.stdout).ok()?.trim().to_string();
            (!p.is_empty()).then(|| PathBuf::from(p))
        });
    let usable = candidate.as_ref().is_some_and(|p| {
        isolated(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    if usable {
        return candidate;
    }
    if std::env::var_os("AMPLIHACK_1900_REQUIRE_RECIPE_RUNNER").is_some() {
        panic!("recipe-runner-rs is required for the #1900 runner scenario");
    }
    eprintln!("recipe-runner-rs unavailable; skipping the #1900 runner scenario");
    None
}

fn workflow_prep_step(id: &str) -> Value {
    let text =
        fs::read_to_string(recipes_dir().join("workflow-prep.yaml")).expect("read workflow-prep");
    let recipe: Value = serde_yaml::from_str(&text).expect("parse workflow-prep");
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-prep step {id} missing"))
        .clone()
}

fn bash_step(id: &str, command: &str, output: Option<&str>) -> Value {
    let mut m = Mapping::new();
    m.insert("id".into(), id.into());
    m.insert("type".into(), "bash".into());
    m.insert("command".into(), command.into());
    if let Some(o) = output {
        m.insert("output".into(), o.into());
    }
    Value::Mapping(m)
}

fn write_recipe(dir: &Path, name: &str, context: &[(&str, &str)], steps: Vec<Value>) {
    let mut ctx = Mapping::new();
    for (k, v) in context {
        ctx.insert((*k).into(), (*v).into());
    }
    let mut m = Mapping::new();
    m.insert("name".into(), name.into());
    m.insert("description".into(), "issue #1900 test stub".into());
    m.insert("version".into(), "1.0.0".into());
    m.insert("context".into(), Value::Mapping(ctx));
    m.insert("steps".into(), Value::Sequence(steps));
    let text = serde_yaml::to_string(&Value::Mapping(m)).expect("yaml");
    fs::write(dir.join(format!("{name}.yaml")), text).expect("write stub recipe");
}

fn run(cmd: &mut Command, what: &str) -> Output {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(
        out.status.success(),
        "{what} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

struct Harness {
    temp: tempfile::TempDir,
}

impl Harness {
    fn new(runner: &Path, workstreams: usize) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let h = Harness { temp };
        for d in ["fakebin", "stubs", "home", "scratch", "ws-contexts", "repo"] {
            fs::create_dir_all(h.path(d)).expect("mkdir");
        }
        let bin = h.path("fakebin");
        write_executable(&bin.join("gh"), FAKE_GH);
        write_executable(&bin.join("amplihack"), AMPLIHACK_SHIM);
        std::os::unix::fs::symlink(runner, bin.join("recipe-runner-rs")).expect("link runner");
        h.write_stubs(workstreams);
        h.init_repo();
        h
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn write_stubs(&self, workstreams: usize) {
        let stubs = self.path("stubs");
        let decomposition = serde_json::json!({
            "task_type": "Development",
            "workstreams": (1..=workstreams).map(|i| serde_json::json!({
                "name": format!("part {i}"),
                "description": format!("Implement part {i} of the widget"),
                "classification": "Development",
            })).collect::<Vec<_>>(),
        })
        .to_string();
        write_recipe(
            &stubs,
            "smart-classify-route",
            &[],
            vec![
                bash_step("stub-task-type", "printf 'Development'", Some("task_type")),
                bash_step(
                    "stub-workstream-count",
                    &format!("printf '{workstreams}'"),
                    Some("workstream_count"),
                ),
                bash_step(
                    "stub-decomposition",
                    &format!("cat <<'EOF'\n{decomposition}\nEOF"),
                    Some("decomposition_json"),
                ),
                bash_step(
                    "stub-session",
                    r#"printf '{"status":"ok","tree_id":"t1900","depth":0}'"#,
                    Some("session_info"),
                ),
                bash_step(
                    "stub-selected-workflow",
                    "printf 'default-workflow'",
                    Some("selected_workflow"),
                ),
            ],
        );
        for name in ["smart-reflect-loop", "smart-validate-summarize"] {
            write_recipe(&stubs, name, &[], vec![bash_step("noop", "true", None)]);
        }
        // default-workflow: the shipped step-02d, step-03 and step-03b, then a
        // record of what they decided.
        let record = r#"printf 'host=%s\tissue_number=%s\tcreation=%s\n' "${RECIPE_VAR_remote_host_type:-}" "${RECIPE_VAR_issue_number:-}" "$(printf '%s' "${RECIPE_VAR_issue_creation:-}" | tr '\n' ' ')" >> "$WS_RESULT_LOG""#;
        write_recipe(
            &stubs,
            "default-workflow",
            &[
                ("task_description", ""),
                ("repo_path", "."),
                ("issue_number", ""),
                ("create_issue", ""),
                ("remote_host_type", ""),
                ("final_requirements", "issue #1900 test requirements"),
            ],
            vec![
                workflow_prep_step("step-02d-detect-host-type"),
                workflow_prep_step("step-03-create-issue"),
                workflow_prep_step("step-03b-extract-issue-number"),
                bash_step("record", record, None),
            ],
        );
    }

    /// A repo whose `origin` is a local bare repo with one commit on `main`.
    fn init_repo(&self) {
        let origin = self.path("origin.git");
        let repo = self.path("repo");
        let git = |dir: &Path, args: &[&str]| {
            let mut c = isolated("git");
            c.args(args)
                .current_dir(dir)
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .env("HOME", self.path("home"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com");
            run(&mut c, &format!("git {args:?}"));
        };
        git(
            self.temp.path(),
            &["init", "--bare", "-b", "main", "origin.git"],
        );
        git(&repo, &["init", "-b", "main"]);
        fs::write(repo.join("README.md"), "issue 1900\n").expect("write README");
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "init"]);
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().expect("utf8")],
        );
        git(&repo, &["push", "origin", "main"]);
    }

    /// Run the shipped smart-orchestrator.yaml with `sets` as `--set` values.
    fn run_smart_orchestrator(&self, sets: &[(&str, &str)]) -> Output {
        let mut cmd = isolated(self.path("fakebin").join("recipe-runner-rs"));
        cmd.arg(recipes_dir().join("smart-orchestrator.yaml"))
            .arg("--no-default-search")
            .arg("-R")
            .arg(self.path("stubs"))
            .arg("-R")
            .arg(recipes_dir())
            .arg("-C")
            .arg(self.path("repo"))
            .args(["--no-auto-stage", "--no-update-check"])
            .args([
                "--set",
                "task_description=Add widget support to the dashboard component",
            ])
            .arg("--set")
            .arg(format!("repo_path={}", self.path("repo").display()));
        for (k, v) in sets {
            cmd.arg("--set").arg(format!("{k}={v}"));
        }
        for var in ["GH_TOKEN", "GITHUB_TOKEN", "GH_HOST", "GH_REPO"] {
            cmd.env_remove(var);
        }
        cmd.env(
            "PATH",
            format!(
                "{}:/usr/local/bin:/usr/bin:/bin",
                self.path("fakebin").display()
            ),
        )
        .env("HOME", self.path("home"))
        .env("TMPDIR", self.path("scratch"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AMPLIHACK_HOME", workspace_root())
        .env("REAL_AMPLIHACK", env!("CARGO_BIN_EXE_amplihack"))
        .env("STUB_RECIPES", self.path("stubs"))
        .env("WS_CONTEXT_DIR", self.path("ws-contexts"))
        .env("WS_RESULT_LOG", self.path("results.log"))
        .env("GH_LOG", self.path("gh.log"))
        .env("FAKE_REPO_URL", FAKE_REPO_URL)
        .current_dir(self.path("repo"));
        cmd.output().expect("run recipe-runner-rs")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    fn results(&self) -> Vec<String> {
        self.read("results.log")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn contexts(&self) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.path("ws-contexts"))
            .expect("read ws-contexts")
            .flatten()
        {
            let text = fs::read_to_string(e.path()).expect("read context");
            out.push(serde_json::from_str(&text).expect("context json"));
        }
        out
    }

    fn issue_creates(&self) -> usize {
        self.read("gh.log")
            .lines()
            .filter(|l| l.starts_with("issue create"))
            .count()
    }

    fn diag(&self, out: &Output) -> String {
        format!(
            "exit: {:?}\nstdout:\n{}\nstderr:\n{}\nresults:\n{}\ngh log:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            self.read("results.log"),
            self.read("gh.log")
        )
    }
}

// ---------------------------------------------------------------------------
// Single-workstream path: smart-orchestrator -> smart-execute-routing ->
// default-workflow (sub-recipe, context merged by the runner).
// ---------------------------------------------------------------------------

#[test]
fn single_path_create_issue_false_reaches_step_03() {
    let Some(runner) = find_runner() else { return };
    let h = Harness::new(&runner, 1);
    let out =
        h.run_smart_orchestrator(&[("remote_host_type", "github"), ("create_issue", "false")]);
    let results = h.results();
    assert_eq!(
        results.len(),
        1,
        "default-workflow must run once\n{}",
        h.diag(&out)
    );
    assert!(results[0].starts_with("host=github\t"), "{}", h.diag(&out));
    assert!(
        results[0].contains("tracking_system=local"),
        "{}",
        h.diag(&out)
    );
    assert_eq!(h.issue_creates(), 0, "no gh issue create\n{}", h.diag(&out));
}

/// Control: the same run without `create_issue=false` does create an issue,
/// so the harness sees creation when it happens.
#[test]
fn single_path_without_create_issue_creates_one_issue() {
    let Some(runner) = find_runner() else { return };
    let h = Harness::new(&runner, 1);
    let out = h.run_smart_orchestrator(&[("remote_host_type", "github")]);
    assert_eq!(
        h.issue_creates(),
        1,
        "control must create\n{}",
        h.diag(&out)
    );
}

// ---------------------------------------------------------------------------
// Parallel path: smart-orchestrator -> smart-execute-routing ->
// build-workstreams-config -> orch run -> launcher.sh -> default-workflow.
// ---------------------------------------------------------------------------

#[test]
fn parallel_path_create_issue_false_reaches_every_workstream() {
    let Some(runner) = find_runner() else { return };
    let h = Harness::new(&runner, 2);
    let out =
        h.run_smart_orchestrator(&[("remote_host_type", "github"), ("create_issue", "false")]);
    let contexts = h.contexts();
    assert_eq!(
        contexts.len(),
        2,
        "both workstreams must launch\n{}",
        h.diag(&out)
    );
    for ctx in &contexts {
        assert_eq!(ctx["create_issue"], "false", "{ctx}");
        assert_eq!(ctx["remote_host_type"], "github", "{ctx}");
        assert!(
            ctx.get("issue_number").is_none(),
            "no placeholder issue: {ctx}"
        );
    }
    let results = h.results();
    assert_eq!(
        results.len(),
        2,
        "step-03 must run in both\n{}",
        h.diag(&out)
    );
    for line in &results {
        assert!(
            line.starts_with("host=github\t"),
            "{line}\n{}",
            h.diag(&out)
        );
        assert!(
            line.contains("tracking_system=local"),
            "{line}\n{}",
            h.diag(&out)
        );
    }
    assert_eq!(h.issue_creates(), 0, "no gh issue create\n{}", h.diag(&out));
}

/// #1900: with the caller's issue number every workstream gets it, launches
/// (no `Invalid issue number: "TBD"`) and reuses that issue.
#[test]
fn parallel_path_issue_number_reaches_every_workstream() {
    let Some(runner) = find_runner() else { return };
    let h = Harness::new(&runner, 2);
    let out = h.run_smart_orchestrator(&[
        ("remote_host_type", "github"),
        ("issue_number", CALLER_ISSUE),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("Invalid issue number"), "{}", h.diag(&out));
    let contexts = h.contexts();
    assert_eq!(
        contexts.len(),
        2,
        "both workstreams must launch\n{}",
        h.diag(&out)
    );
    for ctx in &contexts {
        assert_eq!(ctx["issue_number"], 368, "{ctx}");
    }
    let results = h.results();
    assert_eq!(
        results.len(),
        2,
        "step-03 must run in both\n{}",
        h.diag(&out)
    );
    for line in &results {
        assert!(
            line.contains("issue_number=368\t"),
            "{line}\n{}",
            h.diag(&out)
        );
        assert!(
            line.contains("creation=https://github.com/example-org/example-repo/issues/368"),
            "{line}\n{}",
            h.diag(&out)
        );
    }
    assert_eq!(h.issue_creates(), 0, "no gh issue create\n{}", h.diag(&out));
}

/// #1900: no issue number and no `create_issue` means local tracking in every
/// workstream, never a placeholder and never one new issue per workstream.
#[test]
fn parallel_path_without_issue_number_uses_local_tracking() {
    let Some(runner) = find_runner() else { return };
    let h = Harness::new(&runner, 2);
    let out = h.run_smart_orchestrator(&[("remote_host_type", "github")]);
    let contexts = h.contexts();
    assert_eq!(
        contexts.len(),
        2,
        "both workstreams must launch\n{}",
        h.diag(&out)
    );
    for ctx in &contexts {
        assert_eq!(ctx["create_issue"], "false", "{ctx}");
        assert!(ctx.get("issue_number").is_none(), "{ctx}");
    }
    let results = h.results();
    assert_eq!(
        results.len(),
        2,
        "step-03 must run in both\n{}",
        h.diag(&out)
    );
    for line in &results {
        assert!(
            line.contains("tracking_system=local"),
            "{line}\n{}",
            h.diag(&out)
        );
    }
    assert_eq!(h.issue_creates(), 0, "no gh issue create\n{}", h.diag(&out));
}
