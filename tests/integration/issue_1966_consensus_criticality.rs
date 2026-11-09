//! Issue #1966: `is_critical_code` is a required caller input to the consensus
//! workflow. It must be exactly `true` or `false`; nothing in the workflow
//! sets it. Three checkpoints enforce the contract, all without a
//! `condition`:
//!
//! - `consensus-workflow` step 0, `validate-criticality-input`, before
//!   `consensus-preflight`;
//! - `consensus-tdd-impl` `step5-validate-criticality`, before either route;
//! - `consensus-tdd-impl` `step5-require-complete-route`, after
//!   `step5-nversion-vote`, which fails unless exactly one route completed and
//!   outputs the route word as `implementation_route`.
//!
//! The checks live in one sourced helper,
//! `amplifier-bundle/tools/consensus_criticality.sh` (`cc_validate_input`,
//! `cc_require_route`). Contract: `docs/reference/consensus-criticality-contract.md`.
//! cc factory lesson 29 (required-consensus-implementation-route).
//!
//! Layers:
//!
//! - static checks of the recipes, the helper and the manifest;
//! - helper unit tests that source the helper in `bash` and drive it through
//!   `RECIPE_VAR_*` variables (no runner);
//! - `orch` decomposition copy-through;
//! - end-to-end runs of the real recipes through the real `recipe-runner-rs`.
//!   Every `agent:` step is replaced by a Bash marker with the same `id`,
//!   `condition`, `output` and `parse_json*` settings. In composed runs the
//!   Bash steps of the other phases (git, gh, CI) are replaced as well; the
//!   three checkpoints always run unchanged. No agent starts and nothing
//!   touches the network.
//!
//! Every child process goes through `isolated(..)` with a temporary `HOME`,
//! `XDG_*`, `CLAUDE_CONFIG_DIR` and `TMPDIR`. No sleeps, wall-clock windows or
//! retries. Without a local runner the end-to-end tests fail (they never
//! skip): set `RECIPE_RUNNER_RS_PATH` or put `recipe-runner-rs` on `PATH`.

use regex::Regex;
use serde_json::Value as Json;
use serde_yaml::{Mapping, Value as Yaml};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// --- contract constants -----------------------------------------------------

const VALIDATE_TOP: &str = "validate-criticality-input";
const VALIDATE_TDD: &str = "step5-validate-criticality";
const ROUTE_GATE: &str = "step5-require-complete-route";
const STANDARD_STEP: &str = "step5-standard-implementation";
const NVERSION_STEPS: &[&str] = &[
    "step5-nversion-identify-critical",
    "step5-nversion-builder1-conservative",
    "step5-nversion-builder2-pragmatic",
    "step5-nversion-builder3-minimalist",
    "step5-nversion-compare",
    "step5-nversion-synthesize",
    "step5-nversion-vote",
];
const HELPER: &str = "amplifier-bundle/tools/consensus_criticality.sh";

/// The consensus recipe set (composer plus the eleven phases).
const CONSENSUS_RECIPES: &[&str] = &[
    "consensus-workflow",
    "consensus-preflight",
    "consensus-requirements-debate",
    "consensus-issue-worktree",
    "consensus-design",
    "consensus-tdd-impl",
    "consensus-panel-cleanup",
    "consensus-test-fix",
    "consensus-publish",
    "consensus-pr-review",
    "consensus-pr-feedback",
    "consensus-merge-finalize",
];
const PHASES: &[&str] = &[
    "consensus-preflight",
    "consensus-requirements-debate",
    "consensus-issue-worktree",
    "consensus-design",
    "consensus-tdd-impl",
    "consensus-panel-cleanup",
    "consensus-test-fix",
    "consensus-publish",
    "consensus-pr-review",
    "consensus-pr-feedback",
    "consensus-merge-finalize",
];
/// Recipes edited by this change; their manifest entries must match.
const EDITED_RECIPES: &[&str] = &[
    "consensus-workflow",
    "consensus-tdd-impl",
    "consensus-requirements-debate",
    "consensus-publish",
    "consensus-merge-finalize",
];

/// D7: conditional remediation steps a valid fixture run does not trigger.
/// Checked in both directions: each must be skipped, and every other
/// inventory step outside the unselected route must complete.
const ALLOWED_SKIPS: &[(&str, &str)] = &[
    (
        "step7-fix-issues",
        "runs only when test_results.tests_run is 'true'; the stubbed test run reports none",
    ),
    (
        "step12-implement-feedback",
        "runs only on a needs_work PR verdict; the stubbed review gives none",
    ),
    (
        "step12-push-updates",
        "runs only on a needs_work PR verdict; the stubbed review gives none",
    ),
    (
        "step14-diagnose-ci",
        "runs only when ci_status.ci_checked is 'true'; the stubbed CI check reports none",
    ),
];

// D4: the five error lines. Tests assert the documented text; echoed values
// are cleaned (allowlist A-Za-z0-9._ - with '?') and cut to 64 bytes.
const E_MISSING: &str = "ERROR: missing required input is_critical_code (pass -c is_critical_code=true or -c is_critical_code=false)";
const P_MISSING: &str = "ERROR: missing required input is_critical_code";
const P_INVALID: &str = "ERROR: invalid is_critical_code";
const P_NOROUTE: &str = "ERROR: no implementation route ran";
const P_INCOMPLETE: &str = "ERROR: implementation route incomplete:";
const E_BOTH: &str = "ERROR: both implementation routes produced outputs";

fn e_invalid(shown: &str) -> String {
    format!("ERROR: invalid is_critical_code '{shown}'; expected 'true' or 'false'")
}
fn e_noroute(v: &str) -> String {
    format!("ERROR: no implementation route ran (is_critical_code='{v}')")
}
fn e_incomplete(route: &str, missing: &str) -> String {
    format!("ERROR: implementation route incomplete: {route} missing {missing}")
}

// --- paths and recipe loading -----------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn recipe_path(name: &str) -> PathBuf {
    repo_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn load_yaml(name: &str) -> Yaml {
    let path = recipe_path(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn helper_text() -> String {
    let p = repo_root().join(HELPER);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn steps(recipe: &Yaml) -> &Vec<Yaml> {
    recipe["steps"].as_sequence().expect("recipe has steps")
}

fn step_ids(recipe: &Yaml) -> Vec<String> {
    steps(recipe)
        .iter()
        .map(|s| s["id"].as_str().expect("step id").to_string())
        .collect()
}

fn index_of(recipe: &Yaml, id: &str) -> usize {
    step_ids(recipe)
        .iter()
        .position(|s| s == id)
        .unwrap_or_else(|| panic!("step '{id}' not found"))
}

fn step<'a>(recipe: &'a Yaml, id: &str) -> &'a Yaml {
    &steps(recipe)[index_of(recipe, id)]
}

fn command(step: &Yaml) -> &str {
    step["command"].as_str().unwrap_or_default()
}

fn is_bash(step: &Yaml) -> bool {
    step["type"].as_str() == Some("bash") && step.get("command").is_some()
}

// --- fixture: stub agent (and optionally bash) steps --------------------------

/// How the stubbed fixture differs from the real recipe.
#[derive(Default, Clone)]
struct Stub {
    /// Exact stdout for a stubbed step, by step id.
    output: HashMap<&'static str, String>,
    /// Replacement `condition` for a step, by step id.
    condition: HashMap<&'static str, String>,
    /// Replacement or added context entries.
    context: Vec<(&'static str, Yaml)>,
    /// Replace Bash steps too, except the three checkpoints.
    stub_bash: bool,
}

const CHECKPOINTS: &[&str] = &[VALIDATE_TOP, VALIDATE_TDD, ROUTE_GATE];

/// The real recipe with each `agent:` step (and, with `stub_bash`, each Bash
/// step other than a checkpoint) replaced by a Bash step that keeps `id`,
/// `condition`, `output`, `parse_json` and `parse_json_required` and prints a
/// marker. Kept Bash steps are copied unchanged.
fn stub_recipe(name: &str, stub: &Stub) -> Yaml {
    let mut recipe = load_yaml(name);
    let original = load_yaml(name);
    let seq = recipe["steps"].as_sequence_mut().expect("steps");
    for (i, s) in seq.iter_mut().enumerate() {
        let id = s["id"].as_str().expect("id").to_string();
        let replace = s.get("agent").is_some()
            || (stub.stub_bash
                && s["type"].as_str() == Some("bash")
                && !CHECKPOINTS.contains(&id.as_str()));
        if replace {
            let parse_json = s["parse_json"].as_bool().unwrap_or(false);
            let text = stub
                .output
                .iter()
                .find(|(k, _)| **k == id)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| {
                    if parse_json {
                        format!("{{\"stub\":\"{id}\"}}")
                    } else {
                        format!("stub:{id}")
                    }
                });
            let mut m = Mapping::new();
            m.insert("id".into(), id.clone().into());
            for key in ["condition", "output", "parse_json", "parse_json_required"] {
                if let Some(v) = s.get(key) {
                    m.insert(key.into(), v.clone());
                }
            }
            m.insert("type".into(), "bash".into());
            m.insert(
                "command".into(),
                format!("printf '%s' {}\n", shell_quote(&text)).into(),
            );
            *s = Yaml::Mapping(m);
        } else {
            assert_eq!(
                s,
                &steps(&original)[i],
                "kept step '{id}' must be copied unchanged"
            );
        }
        if let Some((_, c)) = stub.condition.iter().find(|(k, _)| **k == id) {
            s.as_mapping_mut()
                .unwrap()
                .insert("condition".into(), c.clone().into());
        }
    }
    assert!(
        steps(&recipe).iter().all(|s| s.get("agent").is_none()),
        "{name}: an agent step survived stubbing"
    );
    let root = recipe.as_mapping_mut().expect("recipe mapping");
    if !root.get("context").is_some_and(Yaml::is_mapping) {
        root.insert("context".into(), Yaml::Mapping(Mapping::new()));
    }
    let ctx = root
        .get_mut("context")
        .and_then(Yaml::as_mapping_mut)
        .expect("context mapping");
    for (k, v) in &stub.context {
        ctx.insert((*k).into(), v.clone());
    }
    recipe
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// --- running the real runner --------------------------------------------------

fn runner() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(v) = std::env::var_os("RECIPE_RUNNER_RS_PATH") {
        let p = PathBuf::from(&v);
        if p.components().count() > 1 && p.is_file() {
            return p;
        }
        if let Some(found) = std::env::split_paths(&path)
            .map(|d| d.join(&p))
            .find(|c| c.is_file())
        {
            return found;
        }
    }
    std::env::split_paths(&path)
        .map(|d| d.join("recipe-runner-rs"))
        .find(|c| c.is_file())
        .expect(
            "issue #1966 end-to-end tests need the real recipe-runner-rs: set \
             RECIPE_RUNNER_RS_PATH or put recipe-runner-rs on PATH",
        )
}

struct Run {
    code: i32,
    stderr: String,
    json: Json,
}

impl Run {
    fn result(&self, id: &str) -> Option<&Json> {
        self.json["step_results"]
            .as_array()?
            .iter()
            .find(|r| r["step_id"] == id)
    }
    fn status(&self, id: &str) -> Option<String> {
        self.result(id)
            .and_then(|r| r["status"].as_str().map(str::to_string))
    }
    fn error(&self, id: &str) -> String {
        self.result(id)
            .and_then(|r| r["error"].as_str())
            .unwrap_or_default()
            .to_string()
    }
    fn output(&self, id: &str) -> String {
        self.result(id)
            .and_then(|r| r["output"].as_str())
            .unwrap_or_default()
            .to_string()
    }
    fn ctx(&self, key: &str) -> &Json {
        &self.json["context"][key]
    }
    /// Runner stderr plus every step error and output.
    fn all_text(&self) -> String {
        let mut t = self.stderr.clone();
        if let Some(a) = self.json["step_results"].as_array() {
            for r in a {
                for k in ["error", "output"] {
                    t.push('\n');
                    t.push_str(r[k].as_str().unwrap_or_default());
                }
            }
        }
        t
    }
    fn debug(&self) -> String {
        format!(
            "exit={} stderr=\n{}\njson=\n{}",
            self.code,
            self.stderr,
            serde_json::to_string_pretty(&self.json).unwrap_or_default()
        )
    }
    /// Status of every step: top-level results plus the child lines each
    /// sub-recipe step prints (`  [completed] step-id (0.0s)`).
    fn statuses(&self) -> BTreeMap<String, String> {
        let line = Regex::new(r"^\s*\[\s*([a-z]+)\]\s+(\S+)").unwrap();
        let mut map = BTreeMap::new();
        for r in self.json["step_results"].as_array().into_iter().flatten() {
            let id = r["step_id"].as_str().unwrap_or_default().to_string();
            let st = r["status"].as_str().unwrap_or_default().to_string();
            map.insert(id, st);
            for text in [r["output"].as_str(), r["error"].as_str()]
                .into_iter()
                .flatten()
            {
                for l in text.lines() {
                    if let Some(c) = line.captures(l) {
                        map.insert(c[2].to_string(), c[1].to_string());
                    }
                }
            }
        }
        map
    }
}

/// Assert `step` failed with `exit` and that its error ends with `line`.
fn assert_step_error(run: &Run, step: &str, exit: i32, line: &str) {
    let err = run.error(step);
    assert_eq!(
        run.status(step).as_deref(),
        Some("failed"),
        "{step} must fail; {}",
        run.debug()
    );
    assert!(
        err.contains(&format!("(exit {exit})")),
        "{step} must exit {exit}; error: {err}"
    );
    assert!(
        err.trim_end().ends_with(line),
        "{step} error must end with exactly\n  {line}\ngot\n  {err}"
    );
}

struct Sandbox {
    temp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        for d in [
            "home",
            "home/.claude",
            "xdg/config",
            "xdg/cache",
            "xdg/data",
            "xdg/state",
            "tmp",
            "work",
            "recipes",
        ] {
            fs::create_dir_all(temp.path().join(d)).unwrap();
        }
        Sandbox { temp }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.temp.path().join(rel)
    }
    fn write_recipe(&self, file_name: &str, recipe: &Yaml) -> PathBuf {
        let path = self.p("recipes").join(format!("{file_name}.yaml"));
        fs::write(&path, serde_yaml::to_string(recipe).unwrap()).unwrap();
        path
    }
    fn isolate(&self, cmd: &mut Command) {
        cmd.env("HOME", self.p("home"))
            .env("XDG_CONFIG_HOME", self.p("xdg/config"))
            .env("XDG_CACHE_HOME", self.p("xdg/cache"))
            .env("XDG_DATA_HOME", self.p("xdg/data"))
            .env("XDG_STATE_HOME", self.p("xdg/state"))
            .env("CLAUDE_CONFIG_DIR", self.p("home/.claude"))
            .env("TMPDIR", self.p("tmp"))
            .env_remove("REPO_PATH")
            .env_remove("AMPLIHACK_CONTEXT_FILE")
            .current_dir(self.p("work"));
    }

    /// Run `recipe` with the bundle at `amplihack_home` (the worktree by
    /// default) and the given `-c` overrides.
    fn run(&self, recipe: &Path, sets: &[&str], amplihack_home: &Path) -> Run {
        let mut cmd: Command = crate::isolated_amplihack::isolated(runner());
        self.isolate(&mut cmd);
        // Only flags the runner pinned in claude-plugin/recipe-runner.rev
        // (what CI installs) accepts: it rejects `--no-update-check` and
        // `--no-default-search`. The env var turns the update check off, and
        // the extra search-dir variables are removed so only `-R` and the
        // bundle at `amplihack_home` are searched.
        cmd.env("AMPLIHACK_HOME", amplihack_home)
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            .env_remove("AMPLIHACK_PACKAGE_RECIPE_DIR")
            .env_remove("RECIPE_RUNNER_RECIPE_DIRS")
            .arg(recipe)
            .args(["--output-format", "json", "-C"])
            .arg(self.p("work"))
            .arg("-R")
            .arg(self.p("recipes"));
        for s in sets {
            cmd.args(["-c", s]);
        }
        let out = cmd.output().expect("spawn recipe-runner-rs");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "runner stdout is not JSON ({e}); stdout:\n{}\nstderr:\n{stderr}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
        Run {
            code: out.status.code().unwrap_or(-1),
            stderr,
            json,
        }
    }
}

// --- direct consensus-tdd-impl runs -----------------------------------------

fn run_tdd(stub: &Stub, sets: &[&str]) -> Run {
    let sb = Sandbox::new();
    let path = sb.write_recipe(
        "consensus-tdd-impl",
        &stub_recipe("consensus-tdd-impl", stub),
    );
    sb.run(&path, sets, &repo_root())
}

fn force(ids: &[&'static str], condition: &str) -> Stub {
    let mut s = Stub::default();
    for id in ids {
        s.condition.insert(*id, condition.to_string());
    }
    s
}

fn every_route_step() -> Vec<&'static str> {
    std::iter::once(STANDARD_STEP)
        .chain(NVERSION_STEPS.iter().copied())
        .collect()
}

fn assert_no_route_completed(run: &Run) {
    let st = run.statuses();
    for id in every_route_step() {
        assert_ne!(
            st.get(id).map(String::as_str),
            Some("completed"),
            "route step {id} must not complete; {}",
            run.debug()
        );
    }
    assert_ne!(
        st.get(ROUTE_GATE).map(String::as_str),
        Some("completed"),
        "the route gate must not pass; {}",
        run.debug()
    );
}

fn assert_route(run: &Run, route: &str) {
    assert_eq!(run.code, 0, "{route} route must succeed; {}", run.debug());
    let st = run.statuses();
    let status = |id: &str| st.get(id).cloned().unwrap_or_default();
    assert_eq!(status(VALIDATE_TDD), "completed", "{}", run.debug());
    let (ran, skipped): (Vec<&str>, Vec<&str>) = if route == "standard" {
        (vec![STANDARD_STEP], NVERSION_STEPS.to_vec())
    } else {
        (NVERSION_STEPS.to_vec(), vec![STANDARD_STEP])
    };
    for id in ran {
        assert_eq!(status(id), "completed", "{id}; {}", run.debug());
    }
    for id in skipped {
        assert_eq!(status(id), "skipped", "{id}; {}", run.debug());
    }
    assert_eq!(status(ROUTE_GATE), "completed", "{}", run.debug());
    assert_eq!(
        run.ctx("implementation_route"),
        route,
        "D5: the gate outputs the route word only; {}",
        run.debug()
    );
}

// --- composed consensus-workflow runs ---------------------------------------

/// All eleven phases stubbed (agent and Bash steps, except the checkpoints)
/// and the real composer, step 0 included. `requires_debate` is 'true' so the
/// requirements debate runs (D7).
fn run_composed(sets: &[&str], composer_context: &[(&'static str, Yaml)]) -> Run {
    let sb = Sandbox::new();
    for phase in PHASES {
        let mut stub = Stub {
            stub_bash: true,
            ..Stub::default()
        };
        stub.output.insert(
            "step1-requirements-analysis",
            r#"{"requires_debate":"true"}"#.to_string(),
        );
        sb.write_recipe(phase, &stub_recipe(phase, &stub));
    }
    let mut wf = load_yaml("consensus-workflow");
    let ctx = wf["context"].as_mapping_mut().expect("composer context");
    for (k, v) in composer_context {
        ctx.insert((*k).into(), v.clone());
    }
    let composer = sb.write_recipe("consensus-workflow", &wf);
    sb.run(&composer, sets, &repo_root())
}

/// The composed inventory: composer steps with each recipe step expanded.
fn composed_inventory() -> Vec<String> {
    let mut ids = Vec::new();
    for s in steps(&load_yaml("consensus-workflow")) {
        match s["recipe"].as_str() {
            Some(name) if s["type"].as_str() == Some("recipe") => {
                ids.extend(step_ids(&load_yaml(name)));
            }
            _ => ids.push(s["id"].as_str().unwrap().to_string()),
        }
    }
    ids
}

/// A top-level run that fails at step 0: no phase starts.
fn assert_stopped_at_step0(run: &Run, line: &str) {
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(run, VALIDATE_TOP, 1, line);
    for phase in PHASES {
        let st = run.status(phase);
        assert!(
            st.is_none() || st.as_deref() == Some("skipped"),
            "phase {phase} must not start after step 0 fails (status {st:?}); {}",
            run.debug()
        );
    }
    assert!(
        run.ctx("implementation").is_null() && run.ctx("implementation_route").is_null(),
        "no implementation may run; {}",
        run.debug()
    );
}

/// D7: every inventory step completed, except the unselected route (skipped)
/// and the allowlisted remediation steps (skipped); checked both ways.
fn assert_full_review(run: &Run, route: &str) {
    assert_eq!(run.code, 0, "{}", run.debug());
    let st = run.statuses();
    let unselected: Vec<&str> = if route == "standard" {
        NVERSION_STEPS.to_vec()
    } else {
        vec![STANDARD_STEP]
    };
    let inventory = composed_inventory();
    for (id, reason) in ALLOWED_SKIPS {
        assert!(
            inventory.iter().any(|i| i == id),
            "stale ALLOWED_SKIPS entry {id} ({reason}): not in the inventory"
        );
    }
    let mut problems = Vec::new();
    for id in &inventory {
        let got = st.get(id).map(String::as_str).unwrap_or("<not run>");
        let allowed_skip = ALLOWED_SKIPS.iter().any(|(a, _)| a == id);
        let want = if unselected.contains(&id.as_str()) || allowed_skip {
            "skipped"
        } else {
            "completed"
        };
        if got != want {
            problems.push(format!("{id}: expected {want}, got {got}"));
        }
    }
    assert!(
        problems.is_empty(),
        "D7 inventory mismatch:\n{}\n{}",
        problems.join("\n"),
        run.debug()
    );
    assert_eq!(run.ctx("implementation_route"), route, "{}", run.debug());
}

// --- helper unit harness (bash, no runner) -----------------------------------

struct Shell {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Source `recipe_context_value.sh` and the helper in a fresh `bash`, run
/// `body`, and print `rc=<n>` and `route=<CC_ROUTE>` on stdout.
fn shell(vars: &[(&str, &str)], prelude: &str, body: &str) -> (Sandbox, Shell) {
    let sb = Sandbox::new();
    let tools = repo_root().join("amplifier-bundle/tools");
    let script = format!(
        "set -u\n. {rcv}\n{prelude}\n. {helper}\n{body}\nrc=$?\nprintf 'rc=%s\\nroute=%s\\n' \"$rc\" \"${{CC_ROUTE-}}\"\n",
        rcv = shell_quote(&tools.join("recipe_context_value.sh").display().to_string()),
        helper = shell_quote(&tools.join("consensus_criticality.sh").display().to_string()),
    );
    let mut cmd = crate::isolated_amplihack::isolated("bash");
    sb.isolate(&mut cmd);
    cmd.arg("-c").arg(script);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn bash");
    let res = Shell {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    (sb, res)
}

impl Shell {
    fn rc(&self) -> i32 {
        assert_eq!(self.code, 0, "the shell must survive; {}", self.dump());
        self.field("rc").parse().expect("rc")
    }
    fn route(&self) -> String {
        self.field("route")
    }
    fn field(&self, name: &str) -> String {
        self.stdout
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name}= line; {}", self.dump()))
            .to_string()
    }
    fn dump(&self) -> String {
        format!(
            "exit={} stdout=\n{}\nstderr=\n{}",
            self.code, self.stdout, self.stderr
        )
    }
    fn assert_err(&self, rc: i32, line: &str) {
        assert_eq!(self.rc(), rc, "{}", self.dump());
        assert!(
            self.stderr.lines().any(|l| l == line),
            "stderr must contain the line\n  {line}\n{}",
            self.dump()
        );
        assert!(
            self.route().is_empty(),
            "CC_ROUTE is set only on success; {}",
            self.dump()
        );
    }
}

fn validate(value: Option<&str>) -> Shell {
    let vars: Vec<(&str, &str)> = value
        .map(|v| vec![("RECIPE_VAR_is_critical_code", v)])
        .unwrap_or_default();
    shell(&vars, "", "cc_validate_input").1
}

const NV_OUTPUTS: &[&str] = &[
    "critical_sections",
    "nversion_v1",
    "nversion_v2",
    "nversion_v3",
    "nversion_comparison",
    "implementation",
    "n_version_implementations",
];

fn require_route(value: &str, present: &[&str]) -> Shell {
    let mut vars: Vec<(String, String)> =
        vec![("RECIPE_VAR_is_critical_code".to_string(), value.to_string())];
    for name in present {
        vars.push((format!("RECIPE_VAR_{name}"), format!("out-{name}")));
    }
    let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    shell(&refs, "", "cc_require_route").1
}

// =============================================================================
// Static contract
// =============================================================================

#[test]
fn checkpoints_have_no_condition() {
    for (recipe, id) in [
        ("consensus-workflow", VALIDATE_TOP),
        ("consensus-tdd-impl", VALIDATE_TDD),
        ("consensus-tdd-impl", ROUTE_GATE),
    ] {
        let r = load_yaml(recipe);
        let s = step(&r, id);
        assert!(
            s.get("condition").is_none(),
            "{recipe}/{id} must run unconditionally"
        );
        assert!(is_bash(s), "{recipe}/{id} must be a bash step");
    }
}

#[test]
fn top_level_validation_is_step_zero_before_preflight() {
    let wf = load_yaml("consensus-workflow");
    let ids = step_ids(&wf);
    assert_eq!(ids[0], VALIDATE_TOP, "{ids:?}");
    assert_eq!(ids[1], "consensus-preflight", "{ids:?}");
    assert_eq!(&ids[1..], PHASES, "phases keep their order");
}

#[test]
fn tdd_impl_checkpoint_positions() {
    let r = load_yaml("consensus-tdd-impl");
    let ids = step_ids(&r);
    let v = index_of(&r, VALIDATE_TDD);
    assert_eq!(v, index_of(&r, "step4-write-tests") + 1, "{ids:?}");
    for id in every_route_step() {
        assert!(v < index_of(&r, id), "{VALIDATE_TDD} must precede {id}");
    }
    let g = index_of(&r, ROUTE_GATE);
    assert_eq!(g, index_of(&r, "step5-nversion-vote") + 1, "{ids:?}");
    assert_eq!(g, ids.len() - 1, "the gate is the last step before step 6");
}

#[test]
fn route_and_review_step_conditions_are_preserved() {
    let expected: &[(&str, &str, Option<&str>)] = &[
        (
            "consensus-tdd-impl",
            STANDARD_STEP,
            Some("is_critical_code == 'false'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-identify-critical",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-builder1-conservative",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-builder2-pragmatic",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-builder3-minimalist",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-compare",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-synthesize",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-tdd-impl",
            "step5-nversion-vote",
            Some("is_critical_code == 'true'"),
        ),
        (
            "consensus-requirements-debate",
            "step1-debate-round1-architect",
            Some("initial_requirements.requires_debate == 'true'"),
        ),
        (
            "consensus-requirements-debate",
            "step1-debate-round3-consensus",
            Some("initial_requirements.requires_debate == 'true'"),
        ),
        (
            "consensus-requirements-debate",
            "step1-finalize-requirements",
            None,
        ),
        ("consensus-design", "step4-design-round3-consensus", None),
        ("consensus-panel-cleanup", "step6-panel-consensus", None),
        ("consensus-pr-review", "step11-pr-review-consensus", None),
        (
            "consensus-pr-review",
            "step12-implement-feedback",
            Some("pr_review_consensus.final_verdict == 'needs_work'"),
        ),
        ("consensus-pr-feedback", "step13-philosophy-guardian", None),
        ("consensus-merge-finalize", "step14-verify-mergeable", None),
        ("consensus-merge-finalize", "step15-final-consensus", None),
    ];
    for (recipe, id, cond) in expected {
        let r = load_yaml(recipe);
        assert_eq!(
            step(&r, id)["condition"].as_str(),
            *cond,
            "{recipe}/{id} condition changed"
        );
    }
}

#[test]
fn every_review_and_route_step_id_is_kept() {
    let inventory = composed_inventory();
    for id in every_route_step().into_iter().chain([
        "step1-debate-round1-architect",
        "step1-debate-round1-security",
        "step1-debate-round1-api",
        "step1-debate-round1-database",
        "step1-debate-round1-tester",
        "step1-debate-round2-cross-examination",
        "step1-debate-round3-consensus",
        "step1-finalize-requirements",
        "step4-design-round1-architect",
        "step4-design-round3-consensus",
        "step6-panel-cleanup",
        "step6-panel-optimizer",
        "step6-panel-reviewer",
        "step6-panel-patterns",
        "step6-panel-consensus",
        "step11-pr-review-reviewer",
        "step11-pr-review-security",
        "step11-pr-review-optimizer",
        "step11-pr-review-patterns",
        "step11-pr-review-tester",
        "step11-pr-review-consensus",
        "step13-philosophy-reviewer",
        "step13-philosophy-patterns",
        "step13-philosophy-cleanup",
        "step13-philosophy-guardian",
        "step14-check-ci",
        "step14-verify-mergeable",
        "step15-final-cleanup",
        "step15-final-reviewer",
        "step15-final-patterns",
        "step15-final-consensus",
    ]) {
        assert!(inventory.iter().any(|i| i == id), "{id} was removed");
    }
}

#[test]
fn classifier_steps_are_removed() {
    let r = load_yaml("consensus-requirements-debate");
    let ids = step_ids(&r);
    for gone in ["step1-classify-criticality", "step1-validate-criticality"] {
        assert!(!ids.iter().any(|i| i == gone), "D1: {gone} must be deleted");
    }
    let text = fs::read_to_string(recipe_path("consensus-requirements-debate")).unwrap();
    assert!(
        !text.contains("criticality_classification"),
        "D1: no classifier output may remain"
    );
    assert!(
        r["context"].get("is_critical_code").is_none(),
        "the debate phase does not declare is_critical_code"
    );
}

#[test]
fn no_consensus_recipe_declares_a_non_empty_default() {
    for name in CONSENSUS_RECIPES {
        let r = load_yaml(name);
        if let Some(v) = r["context"].get("is_critical_code") {
            assert_eq!(
                v.as_str(),
                Some(""),
                "{name}: is_critical_code default must be \"\" (missing)"
            );
        }
    }
    for name in [
        "consensus-workflow",
        "consensus-tdd-impl",
        "consensus-publish",
        "consensus-merge-finalize",
    ] {
        assert_eq!(
            load_yaml(name)["context"]["is_critical_code"].as_str(),
            Some(""),
            "{name} declares is_critical_code: \"\""
        );
    }
}

#[test]
fn no_step_outputs_is_critical_code() {
    let dir = repo_root().join("amplifier-bundle/recipes");
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "yaml") {
            continue;
        }
        let r: Yaml = serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        for s in r["steps"].as_sequence().into_iter().flatten() {
            assert_ne!(
                s["output"].as_str(),
                Some("is_critical_code"),
                "{}: step {:?} outputs is_critical_code",
                path.display(),
                s["id"]
            );
        }
    }
}

#[test]
fn only_the_route_gate_outputs_implementation_route() {
    let mut found = Vec::new();
    for name in CONSENSUS_RECIPES {
        for s in steps(&load_yaml(name)) {
            if s["output"].as_str() == Some("implementation_route") {
                found.push(format!("{name}/{}", s["id"].as_str().unwrap()));
            }
        }
    }
    assert_eq!(found, vec![format!("consensus-tdd-impl/{ROUTE_GATE}")]);
}

#[test]
fn no_criticality_template_inside_a_bash_command() {
    let template = Regex::new(r"\{\{\s*(is_critical_code|criticality)\b").unwrap();
    for name in CONSENSUS_RECIPES {
        for s in steps(&load_yaml(name)) {
            if s["type"].as_str() == Some("bash") {
                assert!(
                    !template.is_match(command(s)),
                    "{name}/{:?}: bash must read RECIPE_VAR_*, never a template",
                    s["id"]
                );
            }
        }
    }
}

#[test]
fn checkpoints_check_and_source_the_helper() {
    for (recipe, id, call) in [
        ("consensus-workflow", VALIDATE_TOP, "cc_validate_input"),
        ("consensus-tdd-impl", VALIDATE_TDD, "cc_validate_input"),
        ("consensus-tdd-impl", ROUTE_GATE, "cc_require_route"),
    ] {
        let r = load_yaml(recipe);
        let cmd = command(step(&r, id));
        let check = cmd
            .find(r#"[ -r "$H/consensus_criticality.sh" ]"#)
            .unwrap_or_else(|| panic!("{id}: helper existence check missing:\n{cmd}"));
        let source = cmd
            .find(r#". "$H/consensus_criticality.sh""#)
            .unwrap_or_else(|| panic!("{id}: helper is not sourced:\n{cmd}"));
        assert!(check < source, "{id}: check the helper before sourcing it");
        let call_at = cmd.find(call).unwrap_or_else(|| panic!("{id}: {call}"));
        assert!(source < call_at, "{id}: call after sourcing");
        assert!(!cmd.contains("eval"), "{id}: no eval");
    }
    let r = load_yaml("consensus-tdd-impl");
    let gate = step(&r, ROUTE_GATE);
    let cmd = command(gate);
    assert!(cmd.contains("cc_require_route || exit $?"), "{cmd}");
    assert!(cmd.contains(r#"printf '%s' "$CC_ROUTE""#), "{cmd}");
    assert!(
        cmd.contains("implementation route complete: "),
        "D5: the gate reports the route on stderr:\n{cmd}"
    );
    assert_eq!(gate["output"].as_str(), Some("implementation_route"));
}

#[test]
fn helper_shape() {
    let text = helper_text();
    assert!(
        text.lines().count() <= 80,
        "{HELPER} must be at most 80 lines, has {}",
        text.lines().count()
    );
    for f in ["cc_validate_input()", "cc_require_route()"] {
        assert!(text.contains(f), "{HELPER} must define {f}");
    }
    for prefix in [P_MISSING, P_INVALID, P_NOROUTE, P_INCOMPLETE, E_BOTH] {
        assert_eq!(
            text.matches(prefix).count(),
            1,
            "{HELPER}: '{prefix}' must appear exactly once"
        );
    }
    for banned in ["eval", "$(cc_", "| while"] {
        assert!(
            !text.contains(banned),
            "{HELPER} must not contain {banned:?}"
        );
    }
    assert!(text.contains("LC_ALL=C"), "cleaning runs under LC_ALL=C");
}

#[test]
fn helper_parses_with_bash_n() {
    let mut cmd = crate::isolated_amplihack::isolated("bash");
    cmd.arg("-n").arg(repo_root().join(HELPER));
    let out = cmd.output().expect("bash -n");
    assert!(
        out.status.success(),
        "bash -n: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn manifest_hashes_match_edited_recipes() {
    let manifest: HashMap<String, String> = serde_json::from_str(
        &fs::read_to_string(repo_root().join("amplifier-bundle/recipes/_recipe_manifest.json"))
            .unwrap(),
    )
    .unwrap();
    for name in EDITED_RECIPES {
        let mut cmd = crate::isolated_amplihack::isolated("sha256sum");
        cmd.arg(recipe_path(name));
        let out = cmd.output().expect("sha256sum");
        assert!(out.status.success());
        let hex = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            manifest.get(*name).map(String::as_str),
            Some(&hex[..16]),
            "_recipe_manifest.json entry for {name} is stale"
        );
    }
}

// =============================================================================
// Helper unit tests: cc_validate_input
// =============================================================================

#[test]
fn helper_valid_values_pass_silently() {
    for v in ["true", "false"] {
        let sh = validate(Some(v));
        assert_eq!(sh.rc(), 0, "{v}; {}", sh.dump());
        assert!(
            sh.stderr.is_empty(),
            "{v}: no output on success; {}",
            sh.dump()
        );
    }
}

#[test]
fn helper_missing_and_empty_are_missing() {
    validate(None).assert_err(1, E_MISSING);
    validate(Some("")).assert_err(1, E_MISSING);
}

#[test]
fn helper_inexact_values_are_invalid() {
    for v in [
        "maybe", "True", "FALSE", "TRUE", " true", "true ", "false\n", "1", "0", "yes", "no",
    ] {
        let shown: String = v
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"._ -".contains(&b) {
                    b as char
                } else {
                    '?'
                }
            })
            .collect();
        validate(Some(v)).assert_err(1, &e_invalid(&shown));
    }
}

#[test]
fn helper_cleans_and_cuts_echoed_values() {
    let long = "x".repeat(300);
    let cases: Vec<(String, String)> = vec![
        ("tr\u{1b}[31mue".into(), "tr??31mue".into()),
        ("a\u{9b}31m".into(), "a??31m".into()),
        ("a\nb".into(), "a?b".into()),
        ("%s%n".into(), "?s?n".into()),
        ("$(touch marker)".into(), "??touch marker?".into()),
        ("`id`".into(), "?id?".into()),
        (long, "x".repeat(64)),
    ];
    for (raw, shown) in cases {
        let (sb, sh) = shell(
            &[("RECIPE_VAR_is_critical_code", raw.as_str())],
            "",
            "cc_validate_input",
        );
        sh.assert_err(1, &e_invalid(&shown));
        assert!(!sb.p("work/marker").exists(), "value was executed");
        assert!(
            !sh.stderr.contains('\u{1b}') && !sh.stderr.contains('\u{9b}'),
            "no control bytes may reach stderr; {}",
            sh.dump()
        );
    }
}

#[test]
fn helper_writes_nothing_to_stdout_and_keeps_shell_options() {
    let (_sb, sh) = shell(
        &[("RECIPE_VAR_is_critical_code", "maybe")],
        "before=$-",
        "cc_validate_input; r=$?; [ \"$-\" = \"$before\" ] || echo OPTIONS-CHANGED; (exit $r)",
    );
    assert_eq!(sh.rc(), 1, "{}", sh.dump());
    assert!(!sh.stdout.contains("OPTIONS-CHANGED"), "{}", sh.dump());
    assert!(
        !sh.stdout.contains("ERROR"),
        "messages go to stderr only; {}",
        sh.dump()
    );
}

// =============================================================================
// Helper unit tests: cc_require_route
// =============================================================================

#[test]
fn helper_route_standard_complete() {
    let sh = require_route("false", &["implementation"]);
    assert_eq!(sh.rc(), 0, "{}", sh.dump());
    assert_eq!(sh.route(), "standard");
}

#[test]
fn helper_route_nversion_complete() {
    let sh = require_route("true", NV_OUTPUTS);
    assert_eq!(sh.rc(), 0, "{}", sh.dump());
    assert_eq!(sh.route(), "nversion");
}

#[test]
fn helper_route_rejects_missing_and_invalid_input() {
    require_route("", &["implementation"]).assert_err(1, E_MISSING);
    require_route("maybe", &["implementation"]).assert_err(1, &e_invalid("maybe"));
    require_route("True", NV_OUTPUTS).assert_err(1, &e_invalid("True"));
}

#[test]
fn helper_route_no_route_ran() {
    require_route("false", &[]).assert_err(1, &e_noroute("false"));
    require_route("true", &[]).assert_err(1, &e_noroute("true"));
}

#[test]
fn helper_route_empty_output_counts_as_absent() {
    let (_sb, sh) = shell(
        &[
            ("RECIPE_VAR_is_critical_code", "false"),
            ("RECIPE_VAR_implementation", ""),
        ],
        "",
        "cc_require_route",
    );
    sh.assert_err(1, &e_noroute("false"));
}

#[test]
fn helper_route_incomplete_nversion() {
    let present: Vec<&str> = NV_OUTPUTS
        .iter()
        .copied()
        .filter(|n| *n != "nversion_v2")
        .collect();
    require_route("true", &present).assert_err(1, &e_incomplete("nversion", "nversion_v2"));
}

#[test]
fn helper_route_both_routes() {
    require_route("false", &["implementation", "nversion_v1"]).assert_err(1, E_BOTH);
    require_route("false", NV_OUTPUTS).assert_err(1, E_BOTH);
}

#[test]
fn helper_route_clears_stale_cc_route() {
    let (_sb, sh) = shell(
        &[("RECIPE_VAR_is_critical_code", "false")],
        "CC_ROUTE=nversion",
        "cc_require_route",
    );
    sh.assert_err(1, &e_noroute("false"));
}

/// E-READ: a helper read failure (not "absent") exits 2 and never reports a
/// route; the probe runs in the function's own shell so the 2 propagates.
#[test]
fn helper_route_read_failure_exits_2() {
    let (_sb, sh) = shell(
        &[
            ("RECIPE_VAR_is_critical_code", "false"),
            ("RECIPE_VAR_implementation", "x"),
        ],
        "",
        "write_context_value() { return 1; }\ncc_require_route",
    );
    assert_eq!(sh.rc(), 2, "{}", sh.dump());
    assert!(
        sh.stderr.lines().any(|l| l.starts_with("ERROR: ")),
        "{}",
        sh.dump()
    );
    assert!(!sh.stderr.contains("complete"), "{}", sh.dump());
    assert!(sh.route().is_empty(), "{}", sh.dump());
}

#[test]
fn helper_route_invalid_input_precedes_read_failure() {
    let (_sb, sh) = shell(
        &[("RECIPE_VAR_is_critical_code", "maybe")],
        "",
        "write_context_value() { return 1; }\ncc_require_route",
    );
    sh.assert_err(1, &e_invalid("maybe"));
}

// =============================================================================
// orch decomposition copy-through (D6)
// =============================================================================

fn orch_entry(field: Option<&str>) -> Json {
    let extra = field
        .map(|f| format!(r#","is_critical_code":{f}"#))
        .unwrap_or_default();
    let decomp = format!(
        r#"{{"task_type":"Development","workstreams":[
            {{"name":"a","classification":"Consensus","recipe":"consensus-workflow","description":"x"{extra}}}]}}"#
    );
    let path = amplihack_cli::commands::orch::build_workstreams_config_to_tempfile(&decomp)
        .expect("build workstreams config");
    let body = fs::read_to_string(&path).unwrap();
    let _ = fs::remove_file(&path);
    let parsed: Json = serde_json::from_str(&body).unwrap();
    parsed[0].clone()
}

#[test]
fn orch_without_field_writes_no_key() {
    let e = orch_entry(None);
    assert!(e.get("is_critical_code").is_none(), "{e}");
}

#[test]
fn orch_copies_string_unchanged() {
    for v in ["true", "false", " True", "maybe"] {
        let e = orch_entry(Some(&serde_json::to_string(v).unwrap()));
        assert_eq!(e["is_critical_code"], Json::String(v.into()), "{e}");
    }
}

#[test]
fn orch_turns_boolean_into_string() {
    assert_eq!(orch_entry(Some("true"))["is_critical_code"], "true");
    assert_eq!(orch_entry(Some("false"))["is_critical_code"], "false");
}

#[test]
fn orch_leaves_out_other_types() {
    for v in ["1", "{}", "[]", "null"] {
        let e = orch_entry(Some(v));
        assert!(e.get("is_critical_code").is_none(), "{v}: {e}");
    }
}

// =============================================================================
// (e) direct consensus-tdd-impl: absent or malformed fails at step 5 validation
// =============================================================================

#[test]
fn tdd_direct_absent_value_fails_before_any_route() {
    let run = run_tdd(&Stub::default(), &[]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, VALIDATE_TDD, 1, E_MISSING);
    assert_no_route_completed(&run);
}

#[test]
fn tdd_direct_empty_value_is_missing() {
    let run = run_tdd(&Stub::default(), &["is_critical_code="]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, VALIDATE_TDD, 1, E_MISSING);
    assert_no_route_completed(&run);
}

#[test]
fn tdd_direct_malformed_values_fail_before_any_route() {
    // `-c x=1` reaches the step as JSON 1, read back as the text "1".
    for bad in ["maybe", "FALSE", "1"] {
        let set = format!("is_critical_code={bad}");
        let run = run_tdd(&Stub::default(), &[&set]);
        assert_ne!(run.code, 0, "'{bad}'; {}", run.debug());
        assert_step_error(&run, VALIDATE_TDD, 1, &e_invalid(bad));
        assert_no_route_completed(&run);
    }
}

/// The runner turns `-c x=True` into a JSON Boolean, so a capitalised string
/// can only arrive through a context block; it must still fail.
#[test]
fn tdd_direct_capitalised_string_fails() {
    let stub = Stub {
        context: vec![("is_critical_code", Yaml::String("True".into()))],
        ..Stub::default()
    };
    let run = run_tdd(&stub, &[]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, VALIDATE_TDD, 1, &e_invalid("True"));
    assert_no_route_completed(&run);
}

#[test]
fn tdd_direct_false_runs_standard_route() {
    let run = run_tdd(&Stub::default(), &["is_critical_code=false"]);
    assert_route(&run, "standard");
    assert_eq!(
        run.output(ROUTE_GATE),
        "standard",
        "stdout is the route word"
    );
    assert_eq!(
        run.ctx("implementation"),
        "stub:step5-standard-implementation"
    );
}

#[test]
fn tdd_direct_true_runs_nversion_route() {
    let run = run_tdd(&Stub::default(), &["is_critical_code=true"]);
    assert_route(&run, "nversion");
    assert_eq!(
        run.output(ROUTE_GATE),
        "nversion",
        "stdout is the route word"
    );
    assert!(
        run.ctx("n_version_implementations").is_object(),
        "{}",
        run.debug()
    );
}

#[test]
fn tdd_direct_string_context_values_select_routes() {
    let stub = |v: &str| Stub {
        context: vec![("is_critical_code", Yaml::String(v.into()))],
        ..Stub::default()
    };
    assert_route(&run_tdd(&stub("false"), &[]), "standard");
    assert_route(&run_tdd(&stub("true"), &[]), "nversion");
}

// =============================================================================
// (f) the route gate
// =============================================================================

#[test]
fn f1_no_route_ran_for_false() {
    let stub = force(&every_route_step(), "is_critical_code == 'never'");
    let run = run_tdd(&stub, &["is_critical_code=false"]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, ROUTE_GATE, 1, &e_noroute("false"));
}

#[test]
fn f2_no_route_ran_for_true() {
    let stub = force(&every_route_step(), "is_critical_code == 'never'");
    let run = run_tdd(&stub, &["is_critical_code=true"]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, ROUTE_GATE, 1, &e_noroute("true"));
}

#[test]
fn f2b_empty_standard_output_is_no_route() {
    let mut stub = Stub::default();
    stub.output.insert(STANDARD_STEP, String::new());
    let run = run_tdd(&stub, &["is_critical_code=false"]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, ROUTE_GATE, 1, &e_noroute("false"));
}

#[test]
fn f3_partial_nversion_route_is_incomplete() {
    let stub = force(
        &["step5-nversion-builder2-pragmatic"],
        "is_critical_code == 'never'",
    );
    let run = run_tdd(&stub, &["is_critical_code=true"]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(
        &run,
        ROUTE_GATE,
        1,
        &e_incomplete("nversion", "nversion_v2"),
    );
}

#[test]
fn f4_both_routes_fail_for_false() {
    let stub = force(NVERSION_STEPS, "is_critical_code == 'false'");
    let run = run_tdd(&stub, &["is_critical_code=false"]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, ROUTE_GATE, 1, E_BOTH);
}

/// The gate validates again whatever the earlier steps did: with step 5
/// validation skipped, a malformed value still stops at the gate.
#[test]
fn route_gate_rejects_invalid_value_when_validation_was_skipped() {
    let stub = force(&[VALIDATE_TDD], "is_critical_code == 'never'");
    let run = run_tdd(&stub, &["is_critical_code=maybe"]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, ROUTE_GATE, 1, &e_invalid("maybe"));

    let run = run_tdd(&stub, &[]);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_step_error(&run, ROUTE_GATE, 1, E_MISSING);
}

#[test]
fn checkpoint_with_missing_helper_exits_2() {
    let sb = Sandbox::new();
    let path = sb.write_recipe(
        "consensus-tdd-impl",
        &stub_recipe("consensus-tdd-impl", &Stub::default()),
    );
    // A bundle that has recipe_context_value.sh but not the new helper.
    let home = sb.p("stale-home");
    let tools = home.join("amplifier-bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    fs::copy(
        repo_root().join("amplifier-bundle/tools/recipe_context_value.sh"),
        tools.join("recipe_context_value.sh"),
    )
    .unwrap();
    let run = sb.run(&path, &["is_critical_code=false"], &home);
    assert_ne!(run.code, 0, "{}", run.debug());
    assert_eq!(
        run.status(VALIDATE_TDD).as_deref(),
        Some("failed"),
        "{}",
        run.debug()
    );
    assert!(
        run.error(VALIDATE_TDD).contains("(exit 2)"),
        "{}",
        run.debug()
    );
    assert!(
        run.error(VALIDATE_TDD).contains("consensus_criticality.sh"),
        "the message names the missing helper; {}",
        run.debug()
    );
    assert_no_route_completed(&run);
}

// =============================================================================
// (a), (a2), (b), (b2) top-level runs that stop at step 0
// =============================================================================

#[test]
fn a_top_level_without_inputs_fails_missing() {
    let run = run_composed(&[], &[]);
    assert_stopped_at_step0(&run, E_MISSING);
}

#[test]
fn a_top_level_empty_value_fails_missing() {
    let run = run_composed(&["is_critical_code="], &[]);
    assert_stopped_at_step0(&run, E_MISSING);
}

#[test]
fn a2_criticality_alone_does_not_satisfy_the_requirement() {
    let run = run_composed(&["criticality=high: payment code"], &[]);
    assert_stopped_at_step0(&run, E_MISSING);
}

#[test]
fn b_top_level_malformed_values_fail_invalid() {
    for bad in ["maybe", "1"] {
        let set = format!("is_critical_code={bad}");
        let run = run_composed(&[&set], &[]);
        assert_stopped_at_step0(&run, &e_invalid(bad));
    }
    let run = run_composed(&[], &[("is_critical_code", Yaml::String("True".into()))]);
    assert_stopped_at_step0(&run, &e_invalid("True"));
}

#[test]
fn b2_top_level_hostile_values_are_cleaned_and_never_run() {
    let long = "y".repeat(300);
    let cases: Vec<(String, String)> = vec![
        ("tr\u{1b}[31mue".into(), "tr??31mue".into()),
        ("a\u{9b}31m".into(), "a??31m".into()),
        ("a\nb".into(), "a?b".into()),
        ("%s%n".into(), "?s?n".into()),
        ("$(touch marker)".into(), "??touch marker?".into()),
        ("`touch marker`".into(), "?touch marker?".into()),
        (long, "y".repeat(64)),
    ];
    for (raw, shown) in cases {
        let set = format!("is_critical_code={raw}");
        let run = run_composed(&[&set], &[]);
        assert_stopped_at_step0(&run, &e_invalid(&shown));
        assert!(
            !run.all_text().contains('\u{1b}') && !run.all_text().contains('\u{9b}'),
            "no control bytes in diagnostics; {}",
            run.debug()
        );
    }
}

// =============================================================================
// (c), (d) top-level valid runs: one complete route plus every review phase
// =============================================================================

#[test]
fn c_top_level_false_runs_standard_route_and_every_review() {
    let run = run_composed(&["is_critical_code=false"], &[]);
    assert_full_review(&run, "standard");
    assert_eq!(
        run.ctx("implementation"),
        "stub:step5-standard-implementation"
    );
}

#[test]
fn c2_top_level_string_false_from_context_runs_standard_route() {
    let run = run_composed(&[], &[("is_critical_code", Yaml::String("false".into()))]);
    assert_full_review(&run, "standard");
}

/// Also check (g): the validated value reaches consensus-tdd-impl, which
/// validates it again and selects the N-version route.
#[test]
fn d_top_level_true_runs_nversion_route_and_every_review() {
    let run = run_composed(&["is_critical_code=true"], &[]);
    assert_full_review(&run, "nversion");
    let st = run.statuses();
    assert_eq!(st.get(VALIDATE_TOP).map(String::as_str), Some("completed"));
    assert_eq!(st.get(VALIDATE_TDD).map(String::as_str), Some("completed"));
    assert_eq!(run.ctx("implementation"), "stub:step5-nversion-synthesize");
}
