//! #2569: criterion 1 is a recorded judgement made by an agent step, not a
//! table of hard stops.
//!
//! Each case runs the real pieces in order, as one merge round does:
//!
//! 1. `autodrive_qa_evidence.sh` (step-02a-qa-inputs), which only measures
//!    facts and writes `qa-inputs.json`;
//! 2. a fake agent script that plays step-02: it reads `qa-inputs.json`,
//!    calls the fake tools, writes and commits scenarios in a temporary
//!    repository, and records its judgement through the real
//!    `amplihack orch helper autodrive qa-record`;
//! 3. `autodrive_round_evidence.sh` (step-00d's hash, step-03's
//!    measured-downgrade and merge-ready-verdict, step-05's round-record),
//!    which is the gate and is not changed by #2569.
//!
//! Only the agent is fake. The fakes on `PATH` (`npm`, `gh`, `gadugi-test`,
//! `qa-suite`) log every call to a file and behave as the case requires.
//! Every process runs with a private `HOME`, `TMPDIR`, `XDG_STATE_HOME` and
//! `AMPLIHACK_HOME` and a `PATH` of the fakes plus `/usr/bin:/bin`, so the
//! host's own `gadugi-test`, `~/.claude` and `~/.amplihack` are never
//! reached. No test sleeps, no test scans prompt text, and nothing under
//! `tests/gadugi/` is run.

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SCENARIO: &str = "tests/agentic/checkout.yaml";

fn bundle_tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools")
        .canonicalize()
        .expect("bundle tools directory")
}

fn private_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir");
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
}

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("chmod fake");
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn show(o: &Output) -> String {
    format!(
        "status={:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        text(&o.stdout),
        text(&o.stderr)
    )
}

/// Run `cmd` with `stdin` written from a thread, so a child that refuses
/// early (and closes its stdin) cannot block the test.
fn run_with_stdin(mut cmd: Command, stdin: Vec<u8>) -> Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn");
    let mut pipe = child.stdin.take().expect("stdin pipe");
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&stdin);
    });
    let out = child.wait_with_output().expect("wait");
    writer.join().expect("stdin writer");
    out
}

/// One isolated root: private home directories, the fakes, a repository and
/// a private state directory for the evidence.
struct World {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    tmpdir: PathBuf,
    xdg: PathBuf,
    amplihack_home: PathBuf,
    bin: PathBuf,
    repo: PathBuf,
    state: PathBuf,
    tool_log: PathBuf,
    npm_fails: PathBuf,
    agent: PathBuf,
    agent_saw: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().canonicalize().expect("canonical tempdir");
        private_dir(&base);
        let w = World {
            home: base.join("home"),
            tmpdir: base.join("tmp"),
            xdg: base.join("xdg-state"),
            amplihack_home: base.join("amplihack-home"),
            bin: base.join("bin"),
            repo: base.join("repo"),
            state: base.join("state"),
            tool_log: base.join("tool-calls.log"),
            npm_fails: base.join("tool-state/npm-fails"),
            agent: base.join("agent/criterion-1-agent.sh"),
            agent_saw: base.join("agent/saw-qa-inputs.json"),
            base,
            _tmp: tmp,
        };
        for d in [
            &w.home,
            &w.tmpdir,
            &w.xdg,
            &w.amplihack_home,
            &w.bin,
            &w.repo,
            &w.state,
            &w.base.join("tool-state"),
            &w.base.join("agent"),
        ] {
            private_dir(d);
        }
        fs::write(
            w.home.join(".gitconfig"),
            "[user]\n\tname = QA Fixture\n\temail = qa@example.test\n[init]\n\tdefaultBranch = main\n",
        )
        .expect("gitconfig");
        symlink(amplihack_bin(), w.bin.join("amplihack")).expect("amplihack on PATH");
        w.write_fakes();
        w.write_agent();
        w
    }

    fn write_fakes(&self) {
        let log = self.tool_log.display();
        let st = self.base.join("tool-state");
        let st = st.display();
        let fails = self.npm_fails.display();
        write_exec(
            &self.bin.join("gadugi-test"),
            &format!(
                r#"#!/bin/sh
printf 'gadugi-test %s\n' "$*" >> '{log}'
case "${{GADUGI_PLAN:-pass}}" in
  pass) echo "1 scenario passed"; exit 0 ;;
  fail-then-pass)
    n=$(cat '{st}/gadugi-runs' 2>/dev/null || echo 0); n=$((n + 1))
    echo "$n" > '{st}/gadugi-runs'
    if [ "$n" -eq 1 ]; then echo "step 2: expected 'Order placed', saw 'TypeError: total is undefined'" >&2; exit 1; fi
    echo "1 scenario passed"; exit 0 ;;
  refused) echo "connect ECONNREFUSED 127.0.0.1:8443" >&2; exit 1 ;;
  *) echo "fake gadugi-test: unknown plan" >&2; exit 97 ;;
esac
"#
            ),
        );
        write_exec(
            &self.bin.join("npm"),
            &format!(
                r#"#!/bin/sh
printf 'npm %s\n' "$*" >> '{log}'
if [ -f '{fails}' ]; then echo "Tests: 1 failed, 2 passed, 3 total"; exit 1; fi
echo "Tests: 3 passed, 3 total"
exit 0
"#
            ),
        );
        write_exec(
            &self.bin.join("qa-suite"),
            &format!("#!/bin/sh\nprintf 'qa-suite %s\\n' \"$*\" >> '{log}'\necho ok\nexit 0\n"),
        );
        write_exec(
            &self.bin.join("gh"),
            &format!(
                "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> '{log}'\necho 'gh: offline fixture' >&2\nexit 1\n"
            ),
        );
    }

    /// The fake agent: it plays step-02's judgement for one named case. It
    /// writes nothing to the evidence file itself; only qa-record does.
    fn write_agent(&self) {
        let saw = self.agent_saw.display();
        write_exec(
            &self.agent,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
inputs="${{AUTODRIVE_QA_EVIDENCE%/*}}/qa-inputs.json"
[ -f "$inputs" ] || {{ echo "fake agent: no qa-inputs.json beside the evidence path" >&2; exit 3; }}
cp -- "$inputs" '{saw}'
cd -- "$REPO_PATH"
record() {{
  amplihack orch helper autodrive qa-record --inputs "$inputs" --repo "$REPO_PATH" --out "$AUTODRIVE_QA_EVIDENCE"
}}
scenario='{SCENARIO}'
write_scenario() {{
  mkdir -p tests/agentic
  printf 'name: checkout\ndescription: a shopper places an order\nsteps:\n  - run: node src/app.js checkout\n    expect: Order placed\n' > "$scenario"
  git add -- "$scenario"
  git commit -q -m "test: outside-in scenario for checkout"
  git rev-parse HEAD
}}
case "$1" in
  docs)
    printf '%s' '{{"qa_status":"PASS","qa_judgement":"not_user_facing","qa_judgement_reason":"Only installation and usage prose changes; no code, configuration or build input.","qa_cited_files":["docs/install.md","docs/usage.md"],"qa_scenarios":[],"qa_iterations":[],"qa_handoff":null}}' | record
    ;;
  feature)
    c="$(write_scenario)"
    gadugi-test run --scenario "$scenario"
    printf '{{"qa_status":"PASS","qa_judgement":"user_facing","qa_judgement_reason":"Adds the checkout command that shoppers run.","qa_cited_files":["src/app.js"],"qa_scenarios":[{{"path":"%s","written_in_pr":true,"commit":"%s","result":"pass"}}],"qa_iterations":[],"qa_handoff":null}}' "$scenario" "$c" | record
    ;;
  fix-loop)
    c="$(write_scenario)"
    if gadugi-test run --scenario "$scenario"; then echo "fake agent: the first run was meant to fail" >&2; exit 5; fi
    printf 'const total = 3;\nconsole.log("Order placed: " + total);\n' > src/app.js
    git add -- src/app.js
    git commit -q -m "fix: checkout prints the order confirmation"
    f="$(git rev-parse HEAD)"
    gadugi-test run --scenario "$scenario"
    printf '{{"qa_status":"PASS","qa_judgement":"user_facing","qa_judgement_reason":"Adds the checkout command that shoppers run.","qa_cited_files":["src/app.js"],"qa_scenarios":[{{"path":"%s","written_in_pr":true,"commit":"%s","result":"pass"}}],"qa_iterations":[{{"attempt":1,"failing_scenario":"%s","cause":"product","fix_commit":"%s","result":"fail"}},{{"attempt":2,"failing_scenario":"%s","cause":"product","fix_commit":null,"result":"pass"}}],"qa_handoff":null}}' "$scenario" "$c" "$scenario" "$f" "$scenario" | record
    ;;
  handoff)
    c="$(write_scenario)"
    if out="$(gadugi-test run --scenario "$scenario" 2>&1)"; then echo "fake agent: the run was meant to fail" >&2; exit 5; fi
    case "$out" in *ECONNREFUSED*) ;; *) echo "fake agent: expected a refused connection" >&2; exit 6 ;; esac
    printf '{{"qa_status":"HANDOFF","qa_judgement":"user_facing","qa_judgement_reason":"Checkout needs the payment service, which refuses connections.","qa_cited_files":["src/app.js"],"qa_scenarios":[{{"path":"%s","written_in_pr":true,"commit":"%s","result":"fail"}}],"qa_iterations":[{{"attempt":1,"failing_scenario":"%s","cause":"environment","fix_commit":null,"result":"fail"}}],"qa_handoff":{{"step_id":"step-02-qa-team-scenarios","failure":"connection refused: payment service","choice":"handoff","reason":"payment service unreachable from CI and isolation","owner":"repository maintainer","needs":"restore the payment sandbox or name another endpoint"}}}}' "$scenario" "$c" "$scenario" | record
    ;;
  *) echo "fake agent: unknown case" >&2; exit 2 ;;
esac
"#
            ),
        );
    }

    /// A command with the isolated environment. `GIT_CONFIG_NOSYSTEM` keeps
    /// the host's git configuration out; the private HOME holds the rest.
    fn cmd(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = isolated(program);
        c.env("HOME", &self.home)
            .env("TMPDIR", &self.tmpdir)
            .env("XDG_STATE_HOME", &self.xdg)
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
            .stdin(Stdio::null());
        c
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let o = self
            .cmd("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(o.status.success(), "git {args:?}: {}", show(&o));
        text(&o.stdout).trim().to_string()
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.repo, args)
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.repo.join(rel);
        fs::create_dir_all(p.parent().unwrap()).expect("mkdir");
        fs::write(p, body).expect("write repo file");
    }

    fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A Node repository with a root package.json. `extra` files go into the
    /// base commit too. The base is published as `refs/remotes/origin/main`
    /// and the PR branch `feature` starts from it.
    fn repo_with(&self, extra: &[(&str, &str)]) {
        self.git(&["init", "-q", "-b", "main"]);
        self.write(
            "package.json",
            r#"{"name":"shop","version":"1.0.0","scripts":{"test":"node test.js"}}"#,
        );
        self.write("src/app.js", "console.log(\"shop\");\n");
        self.write("docs/install.md", "# Install\n");
        self.write("docs/usage.md", "# Usage\n");
        self.write("README.md", "# Shop\n");
        for (rel, body) in extra {
            self.write(rel, body);
        }
        let base = self.commit("base");
        self.git(&["update-ref", "refs/remotes/origin/main", &base]);
        self.git(&["checkout", "-q", "-b", "feature"]);
    }

    fn node_repo(&self) {
        self.repo_with(&[]);
    }

    fn docs_pr(&self) -> String {
        self.write("docs/install.md", "# Install\n\nRun `npm install`.\n");
        self.write("docs/usage.md", "# Usage\n\nRun `npm start`.\n");
        self.commit("docs: explain install and usage")
    }

    fn feature_pr(&self) -> String {
        self.write(
            "src/app.js",
            "console.log(\"shop\");\nif (process.argv[2] === \"checkout\") { console.log(\"Order placed: \" + total); }\n",
        );
        self.commit("feat: checkout command")
    }

    fn evidence_path(&self) -> PathBuf {
        self.state.join("qa-evidence.json")
    }

    fn inputs_path(&self) -> PathBuf {
        self.state.join("qa-inputs.json")
    }

    /// step-02a-qa-inputs: the real inputs script, as the recipe runs it.
    fn measure(&self, extra: &[(&str, &str)]) -> Output {
        let mut c = self.cmd("bash");
        c.arg(bundle_tools().join("autodrive_qa_evidence.sh"))
            .current_dir(&self.repo)
            .env("REPO_PATH", &self.repo)
            .env("AUTODRIVE_QA_EVIDENCE", self.evidence_path())
            .env("AUTODRIVE_QA_BASE", "main")
            .env("AUTODRIVE_ROUND_LABEL", "round-1");
        for (k, v) in extra {
            c.env(k, v);
        }
        c.output().expect("run autodrive_qa_evidence.sh")
    }

    fn measured_inputs(&self) -> Value {
        let o = self.measure(&[]);
        assert_eq!(o.status.code(), Some(0), "inputs script: {}", show(&o));
        let inputs = read_json(&self.inputs_path());
        assert!(
            !self.evidence_path().exists(),
            "the inputs script measures facts only; it must not write qa-evidence.json"
        );
        inputs
    }

    /// step-02, played by the fake agent.
    fn run_agent(&self, case: &str, gadugi_plan: &str) -> Output {
        self.cmd("bash")
            .arg(&self.agent)
            .arg(case)
            .current_dir(&self.repo)
            .env("REPO_PATH", &self.repo)
            .env("AUTODRIVE_QA_EVIDENCE", self.evidence_path())
            .env("GADUGI_PLAN", gadugi_plan)
            .output()
            .expect("run fake agent")
    }

    fn tool_calls(&self) -> Vec<String> {
        fs::read_to_string(&self.tool_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn runner_calls(&self) -> Vec<String> {
        self.tool_calls()
            .into_iter()
            .filter(|l| l.starts_with("gadugi-test"))
            .collect()
    }

    /// `autodrive_round_evidence.sh <args>` with every other measurement
    /// agreeing, so only criterion 1 can hold the round. step-00d's hash is
    /// taken here, after the agent has finished, as the round does.
    fn gate(&self, args: &[&str], verdict: Option<&str>, stdin: &str) -> Output {
        let tool = bundle_tools().join("autodrive_round_evidence.sh");
        let hash = self
            .cmd("bash")
            .arg(&tool)
            .arg("qa-evidence-hash")
            .arg(self.evidence_path())
            .current_dir(&self.repo)
            .output()
            .expect("qa-evidence-hash");
        assert!(hash.status.success(), "qa-evidence-hash: {}", show(&hash));
        let evidence = fs::read_to_string(self.evidence_path()).unwrap_or_default();
        let mut c = self.cmd("bash");
        c.arg(&tool)
            .args(args)
            .current_dir(&self.repo)
            .env("REPO_PATH", &self.repo)
            .env("AUTODRIVE_QA_EVIDENCE", self.evidence_path())
            .env("AUTODRIVE_ROUND_RECORD", self.state.join("round-1.json"))
            .env("AUTODRIVE_ROUND_LABEL", "round-1")
            .env("AUTODRIVE_BASE_BRANCH", "main")
            .env("RECIPE_VAR_qa_evidence", evidence.trim())
            .env("RECIPE_VAR_qa_evidence_hash", text(&hash.stdout).trim())
            .env(
                "RECIPE_VAR_ci_evidence",
                r#"{"ci_status":"GREEN","ci_signal":"GREEN"}"#,
            )
            .env("RECIPE_VAR_merge_sync", r#"{"conflict":"false"}"#)
            .env(
                "RECIPE_VAR_platform_facts",
                r#"{"unresolved_threads":"0","approval_status":"MET"}"#,
            )
            .env(
                "RECIPE_VAR_crusty_evidence",
                r#"{"crusty_status":"DONE_CLEAN"}"#,
            );
        if let Some(v) = verdict {
            c.env("RECIPE_VAR_merge_ready_verdict", v);
        }
        run_with_stdin(c, stdin.as_bytes().to_vec())
    }

    fn downgrade(&self) -> String {
        let o = self.gate(&["measured-downgrade"], None, "");
        assert!(o.status.success(), "measured-downgrade: {}", show(&o));
        text(&o.stdout).trim().to_string()
    }

    /// `amplihack orch helper autodrive qa-record`, run directly.
    fn qa_record(&self, inputs: &Path, out: &Path, stdin: Vec<u8>, extra: &[&str]) -> Output {
        let mut c = self.cmd(self.bin.join("amplihack"));
        c.args(["orch", "helper", "autodrive", "qa-record", "--inputs"])
            .arg(inputs)
            .arg("--repo")
            .arg(&self.repo)
            .arg("--out")
            .arg(out)
            .args(extra)
            .current_dir(&self.repo);
        run_with_stdin(c, stdin)
    }

    /// A hand-written inputs file in the documented shape, so qa-record is
    /// tested apart from the shell script.
    fn inputs_fixture(&self) -> PathBuf {
        let p = self.state.join("fixture-inputs.json");
        fs::write(
            &p,
            serde_json::to_vec(&json!({
                "schema": 1,
                "qa_round": "round-1",
                "head_sha": self.git(&["rev-parse", "HEAD"]),
                "base_sha": self.git(&["rev-parse", "refs/remotes/origin/main"]),
                "repo_type": "node",
                "docs_only": "true",
                "changed_files": ["docs/install.md", "docs/usage.md"],
                "scenario_dir": "tests/agentic",
                "existing_scenarios": [],
                "declared_qa": {
                    "source": "detected", "dir": ".", "command": "npm test",
                    "status": "ran", "exit_status": 0, "cause": "", "candidates": "",
                    "ci_skip": "no-match", "log_tail": "LOGTAIL-SECRET-5b1e Tests: 3 passed"
                },
                "git_env_cleared": "none",
                "run_env_cleared": "none"
            }))
            .unwrap(),
        )
        .expect("write inputs fixture");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        p
    }
}

fn read_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

fn strings(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {v}"))
        .iter()
        .map(|s| s.as_str().expect("string entry").to_string())
        .collect();
    out.sort();
    out
}

fn docs_judgement() -> Value {
    json!({
        "qa_status": "PASS",
        "qa_judgement": "not_user_facing",
        "qa_judgement_reason": "Only installation and usage prose changes.",
        "qa_cited_files": ["docs/install.md", "docs/usage.md"],
        "qa_scenarios": [],
        "qa_iterations": [],
        "qa_handoff": null
    })
}

fn handoff_judgement() -> Value {
    json!({
        "qa_status": "HANDOFF",
        "qa_judgement": "user_facing",
        "qa_judgement_reason": "Checkout needs the payment service, which refuses connections.",
        "qa_cited_files": ["src/app.js"],
        "qa_scenarios": [{"path": SCENARIO, "written_in_pr": true, "commit": null, "result": "fail"}],
        "qa_iterations": [{"attempt": 1, "failing_scenario": SCENARIO, "cause": "environment", "fix_commit": null, "result": "fail"}],
        "qa_handoff": {
            "step_id": "step-02-qa-team-scenarios", "failure": "connection refused",
            "choice": "handoff", "reason": "service unreachable",
            "owner": "repository maintainer", "needs": "restore the payment sandbox"
        }
    })
}

/// What every record written by qa-record carries, whatever the case.
fn assert_recorded_by_helper(w: &World, ev: &Value) {
    assert_eq!(
        ev["head_sha"].as_str(),
        Some(w.git(&["rev-parse", "HEAD"]).as_str()),
        "head_sha is the worktree HEAD after the agent's commits: {ev}"
    );
    assert!(
        ev.get("qa_judgement").is_some(),
        "qa_judgement recorded: {ev}"
    );
    let raw = fs::read_to_string(w.evidence_path()).unwrap();
    assert!(!raw.contains("gadugi_"), "no gadugi_* fields remain: {raw}");
    assert!(
        !raw.contains("log_tail"),
        "log_tail never reaches the evidence: {raw}"
    );
    assert_eq!(
        ev["qa_declared_command"]["command"].as_str(),
        Some("npm test"),
        "the declared command is carried as evidence: {ev}"
    );
    let mode = fs::metadata(w.evidence_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "qa-evidence.json is private");
}

// ---------------------------------------------------------------------------
// The four cases of #2569.
// ---------------------------------------------------------------------------

/// (a) A docs-only PR in a Node repository passes with a recorded reason,
/// citing the files, and no scenario runner is called.
#[test]
fn docs_only_pr_in_node_repository_passes_with_recorded_reason() {
    let w = World::new();
    w.node_repo();
    let head = w.docs_pr();

    let inputs = w.measured_inputs();
    assert_eq!(inputs["repo_type"], "node", "{inputs}");
    assert_eq!(inputs["docs_only"], "true", "{inputs}");
    assert_eq!(inputs["head_sha"].as_str(), Some(head.as_str()), "{inputs}");
    assert_eq!(
        strings(&inputs["changed_files"]),
        ["docs/install.md", "docs/usage.md"]
    );
    assert_eq!(inputs["declared_qa"]["command"], "npm test", "{inputs}");
    assert_eq!(inputs["declared_qa"]["status"], "ran", "{inputs}");
    assert_eq!(inputs["declared_qa"]["exit_status"], 0, "{inputs}");

    let o = w.run_agent("docs", "pass");
    assert_eq!(o.status.code(), Some(0), "agent: {}", show(&o));
    assert_eq!(
        read_json(&w.agent_saw),
        inputs,
        "the agent read the inputs file"
    );

    let ev = read_json(&w.evidence_path());
    assert_eq!(ev["qa_status"], "PASS", "{ev}");
    assert_eq!(ev["qa_judgement"], "not_user_facing", "{ev}");
    assert_eq!(ev["qa_reason"], "", "{ev}");
    assert!(
        !ev["qa_judgement_reason"].as_str().unwrap_or("").is_empty(),
        "the reason is recorded: {ev}"
    );
    assert_eq!(
        strings(&ev["qa_cited_files"]),
        ["docs/install.md", "docs/usage.md"]
    );
    assert_recorded_by_helper(&w, &ev);
    assert_eq!(
        stdout_json(&o),
        ev,
        "qa-record prints exactly the record it wrote"
    );

    assert!(
        w.runner_calls().is_empty(),
        "no scenario runner call for a docs-only judgement: {:?}",
        w.tool_calls()
    );
    assert_eq!(w.downgrade(), "none", "criterion 1 is met");
}

/// (b) A feature PR with no scenarios: the agent writes one, commits it in
/// the PR, runs it, and the round passes. A missing scenario never ends it.
#[test]
fn feature_pr_without_scenarios_gets_one_written_and_run() {
    let w = World::new();
    w.node_repo();
    let feature = w.feature_pr();

    let inputs = w.measured_inputs();
    assert_eq!(inputs["docs_only"], "false", "{inputs}");
    assert_eq!(inputs["scenario_dir"], "tests/agentic", "{inputs}");
    assert_eq!(inputs["existing_scenarios"], json!([]), "{inputs}");
    assert!(
        w.runner_calls().is_empty(),
        "the inputs script never runs scenarios: {:?}",
        w.tool_calls()
    );

    let o = w.run_agent("feature", "pass");
    assert_eq!(o.status.code(), Some(0), "agent: {}", show(&o));

    assert_eq!(
        w.git(&["ls-files", "--", SCENARIO]),
        SCENARIO,
        "scenario committed"
    );
    let commits = w.git(&["rev-list", &format!("{feature}..HEAD")]);
    assert_eq!(
        commits.lines().count(),
        1,
        "one scenario commit on the PR branch"
    );
    assert_eq!(
        w.runner_calls(),
        [format!("gadugi-test run --scenario {SCENARIO}")]
    );

    let ev = read_json(&w.evidence_path());
    assert_eq!(ev["qa_status"], "PASS", "{ev}");
    assert_eq!(ev["qa_judgement"], "user_facing", "{ev}");
    let s = &ev["qa_scenarios"][0];
    assert_eq!(s["path"], SCENARIO, "{ev}");
    assert_eq!(s["written_in_pr"], true, "{ev}");
    assert_eq!(s["result"], "pass", "{ev}");
    let c = s["commit"].as_str().expect("scenario commit");
    w.git(&["cat-file", "-e", &format!("{c}^{{commit}}")]);
    assert_recorded_by_helper(&w, &ev);
    assert_eq!(w.downgrade(), "none", "criterion 1 is met");
}

/// (c) The first run fails, the agent fixes the product and commits, the
/// second run passes: two iterations are recorded and the round passes.
#[test]
fn failing_scenario_loops_to_a_fix_and_passes() {
    let w = World::new();
    w.node_repo();
    w.feature_pr();
    w.measured_inputs();

    let o = w.run_agent("fix-loop", "fail-then-pass");
    assert_eq!(o.status.code(), Some(0), "agent: {}", show(&o));
    assert_eq!(w.runner_calls().len(), 2, "{:?}", w.tool_calls());

    let ev = read_json(&w.evidence_path());
    assert_eq!(ev["qa_status"], "PASS", "{ev}");
    let it = ev["qa_iterations"].as_array().expect("iterations");
    assert_eq!(it.len(), 2, "{ev}");
    assert_eq!(it[0]["attempt"], 1);
    assert_eq!(it[0]["result"], "fail");
    assert_eq!(it[0]["cause"], "product");
    assert_eq!(it[1]["attempt"], 2);
    assert_eq!(it[1]["result"], "pass");
    let fix = it[0]["fix_commit"].as_str().expect("fix commit recorded");
    w.git(&["cat-file", "-e", &format!("{fix}^{{commit}}")]);
    let touched = w.git(&["show", "--name-only", "--format=", fix]);
    assert_eq!(touched, "src/app.js", "the fix commit changes the product");
    assert_recorded_by_helper(&w, &ev);
    assert_eq!(w.downgrade(), "none", "criterion 1 is met after the fix");
}

/// (d) The scenario runner reports a refused connection: the agent records a
/// HANDOFF with reason, owner and needs; the round is not merge-ready and
/// the round record names the hand-off as what held it.
#[test]
fn unreachable_service_ends_in_recorded_handoff() {
    let w = World::new();
    w.node_repo();
    w.feature_pr();
    w.measured_inputs();

    let o = w.run_agent("handoff", "refused");
    assert_eq!(o.status.code(), Some(0), "agent: {}", show(&o));

    let ev = read_json(&w.evidence_path());
    assert_eq!(ev["qa_status"], "HANDOFF", "{ev}");
    assert_eq!(ev["qa_reason"], "qa-handoff", "{ev}");
    let h = &ev["qa_handoff"];
    assert_eq!(h["choice"], "handoff", "{ev}");
    for k in ["step_id", "failure", "reason", "owner", "needs"] {
        assert!(
            !h[k].as_str().unwrap_or("").is_empty(),
            "qa_handoff.{k} recorded: {ev}"
        );
    }
    assert_recorded_by_helper(&w, &ev);

    let d = w.downgrade();
    assert!(d.contains("qa_status=HANDOFF"), "downgrade: {d}");

    let v = w.gate(
        &["merge-ready-verdict", "MERGE_READY", "review"],
        None,
        "[]",
    );
    assert!(v.status.success(), "merge-ready-verdict: {}", show(&v));
    let verdict: Value = serde_json::from_slice(&v.stdout).expect("verdict JSON");
    assert_eq!(
        verdict["merge_ready_verdict"], "NOT_MERGE_READY",
        "{verdict}"
    );

    let r = w.gate(&["round-record"], Some(text(&v.stdout).trim()), "");
    assert!(r.status.success(), "round-record: {}", show(&r));
    let rec = read_json(&w.state.join("round-1.json"));
    assert_eq!(rec["merge_ready_verdict"], "NOT_MERGE_READY", "{rec}");
    assert_eq!(rec["qa_status"], "HANDOFF", "{rec}");
    assert!(
        rec["held_by"]
            .as_str()
            .unwrap_or("")
            .contains("criterion 1: qa_status=HANDOFF"),
        "the drive's end reason names the hand-off: {rec}"
    );
}

/// A declared QA command that fails is evidence, not a verdict: the inputs
/// script records it and writes no evidence, and a docs-only judgement can
/// still pass.
#[test]
fn failing_declared_command_is_evidence_not_a_verdict() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    fs::write(&w.npm_fails, "").unwrap();

    let inputs = w.measured_inputs();
    assert_eq!(inputs["declared_qa"]["status"], "ran", "{inputs}");
    assert_eq!(inputs["declared_qa"]["exit_status"], 1, "{inputs}");

    let o = w.run_agent("docs", "pass");
    assert_eq!(o.status.code(), Some(0), "agent: {}", show(&o));
    let ev = read_json(&w.evidence_path());
    assert_eq!(ev["qa_status"], "PASS", "{ev}");
    assert_eq!(ev["qa_declared_command"]["exit_status"], 1, "{ev}");
    assert_eq!(w.downgrade(), "none");
}

/// A record written straight to the evidence file in the old shape, without
/// qa-record, is still read only for qa_status by the gate. This pins the
/// gate harness itself: with qa_status PASS and every other measurement
/// agreeing, nothing holds the round.
#[test]
fn gate_harness_agrees_when_every_measurement_agrees() {
    let w = World::new();
    w.node_repo();
    fs::write(w.evidence_path(), "{\"qa_status\":\"PASS\"}\n").unwrap();
    assert_eq!(w.downgrade(), "none");
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}: {}", show(o)))
}

// ---------------------------------------------------------------------------
// The inputs script (step-02a-qa-inputs).
// ---------------------------------------------------------------------------

/// The declared command comes from the merge-base `.amplihack/qa.toml`
/// through the existing #1763 reader and is run.
#[test]
fn inputs_take_declared_command_from_merge_base_qa_toml() {
    let w = World::new();
    w.repo_with(&[(".amplihack/qa.toml", "command = \"qa-suite --fast\"\n")]);
    w.docs_pr();
    let inputs = w.measured_inputs();
    assert_eq!(inputs["declared_qa"]["source"], "repo-file", "{inputs}");
    assert_eq!(
        inputs["declared_qa"]["command"], "qa-suite --fast",
        "{inputs}"
    );
    assert_eq!(inputs["declared_qa"]["exit_status"], 0, "{inputs}");
    assert!(
        w.tool_calls().contains(&"qa-suite --fast".to_string()),
        "{:?}",
        w.tool_calls()
    );
}

/// A PR that edits qa.toml cannot choose its own suite: the reader refuses
/// it as changed-in-pr, the refusal is recorded, nothing runs, and the
/// script still exits 0.
#[test]
fn inputs_record_a_pr_edited_qa_toml_as_config_invalid() {
    let w = World::new();
    w.repo_with(&[(".amplihack/qa.toml", "command = \"qa-suite --fast\"\n")]);
    w.write(".amplihack/qa.toml", "command = \"qa-suite --none\"\n");
    w.commit("chore: easier suite");
    let inputs = w.measured_inputs();
    assert_eq!(
        inputs["declared_qa"]["status"], "config-invalid",
        "{inputs}"
    );
    assert_eq!(inputs["declared_qa"]["cause"], "changed-in-pr", "{inputs}");
    assert!(inputs["declared_qa"]["exit_status"].is_null(), "{inputs}");
    assert!(
        !w.tool_calls().iter().any(|l| l.starts_with("qa-suite")),
        "{:?}",
        w.tool_calls()
    );
}

/// A repository type outside the detection table is `unknown`, never an
/// exit: the inputs file is still written.
#[test]
fn inputs_record_unknown_repository_type_without_stopping() {
    let w = World::new();
    w.git(&["init", "-q", "-b", "main"]);
    w.write("Gemfile", "source 'https://rubygems.org'\n");
    w.write("lib/shop.rb", "puts 'shop'\n");
    let base = w.commit("base");
    w.git(&["update-ref", "refs/remotes/origin/main", &base]);
    w.git(&["checkout", "-q", "-b", "feature"]);
    w.write("lib/shop.rb", "puts 'shop'\nputs 'checkout'\n");
    w.commit("feat: checkout");
    let inputs = w.measured_inputs();
    assert_eq!(inputs["repo_type"], "unknown", "{inputs}");
    assert_eq!(inputs["declared_qa"]["status"], "missing", "{inputs}");
    assert!(w.runner_calls().is_empty(), "{:?}", w.tool_calls());
}

/// File names holding a newline and a quote are JSON-encoded, not split.
#[test]
fn inputs_encode_awkward_file_names_as_json() {
    let w = World::new();
    w.node_repo();
    let odd = "docs/odd\"name\nline.md";
    w.write(odd, "# odd\n");
    w.commit("docs: odd name");
    let inputs = w.measured_inputs();
    assert_eq!(strings(&inputs["changed_files"]), [odd.to_string()]);
}

/// AUTODRIVE_QA_EVIDENCE must be absolute; a relative path writes nothing.
#[test]
fn inputs_refuse_a_relative_evidence_path() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    let o = w.measure(&[("AUTODRIVE_QA_EVIDENCE", "qa-evidence.json")]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(!w.repo.join("qa-inputs.json").exists());
    assert!(!w.repo.join("qa-evidence.json").exists());
}

/// The inputs file is private, and log_tail stays in it (it never reaches
/// the evidence; see assert_recorded_by_helper).
#[test]
fn inputs_file_is_private_and_keeps_the_log_tail() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    let inputs = w.measured_inputs();
    let mode = fs::metadata(w.inputs_path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "qa-inputs.json is private");
    assert!(
        inputs["declared_qa"]["log_tail"]
            .as_str()
            .unwrap_or("")
            .contains("3 passed"),
        "{inputs}"
    );
}

// ---------------------------------------------------------------------------
// The qa-record helper, run as the binary.
// ---------------------------------------------------------------------------

fn refusal_line(o: &Output) -> String {
    let err = text(&o.stderr);
    let lines: Vec<&str> = err.lines().filter(|l| l.contains("qa-record:")).collect();
    assert_eq!(lines.len(), 1, "one refusal line: {}", show(o));
    lines[0].to_string()
}

#[test]
fn qa_record_writes_the_record_and_derives_fields() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    let inputs = w.inputs_fixture();
    let out = w.evidence_path();
    let o = w.qa_record(
        &inputs,
        &out,
        serde_json::to_vec(&docs_judgement()).unwrap(),
        &["--round", "round-7"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(
        o.stdout,
        fs::read(&out).unwrap(),
        "stdout is the bytes written"
    );
    let ev = read_json(&out);
    assert_eq!(ev["qa_round"], "round-7", "--round overrides: {ev}");
    assert_eq!(ev["qa_reason"], "", "{ev}");
    assert_eq!(ev["qa_repo_type"], "node", "{ev}");
    assert_eq!(ev["qa_command"], "npm test", "{ev}");
    assert_eq!(ev["qa_command_source"], "detected", "{ev}");
    assert_eq!(ev["docs_only"], "true", "{ev}");
    assert_eq!(
        ev["qa_declared_command"],
        json!({"source": "detected", "dir": ".", "command": "npm test", "exit_status": 0})
    );
    assert_eq!(ev["declared_qa"]["status"], "ran", "{ev}");
    assert!(ev["declared_qa"].get("log_tail").is_none(), "{ev}");
    let raw = fs::read_to_string(&out).unwrap();
    assert!(
        !raw.contains("LOGTAIL-SECRET"),
        "log tail never copied: {raw}"
    );
    assert_recorded_by_helper(&w, &ev);
}

#[test]
fn qa_record_derives_reason_from_status() {
    let w = World::new();
    w.node_repo();
    w.feature_pr();
    let inputs = w.inputs_fixture();
    let mut fail = handoff_judgement();
    fail["qa_status"] = json!("FAIL");
    fail["qa_handoff"] = Value::Null;
    for (judgement, status, reason) in [
        (fail, "FAIL", "qa-scenario-failed"),
        (handoff_judgement(), "HANDOFF", "qa-handoff"),
    ] {
        let out = w.state.join(format!("ev-{status}.json"));
        let o = w.qa_record(&inputs, &out, serde_json::to_vec(&judgement).unwrap(), &[]);
        assert_eq!(o.status.code(), Some(0), "{status}: {}", show(&o));
        let ev = read_json(&out);
        assert_eq!(ev["qa_status"], status);
        assert_eq!(ev["qa_reason"], reason, "{ev}");
    }
}

/// HANDOFF without an owner is refused: exit 2, one short line naming the
/// rule and field, the value never repeated, and nothing written.
#[test]
fn qa_record_refuses_handoff_without_owner() {
    let w = World::new();
    w.node_repo();
    let inputs = w.inputs_fixture();
    let out = w.evidence_path();
    let mut j = handoff_judgement();
    j["qa_handoff"]["owner"] = json!("");
    j["qa_handoff"]["needs"] = json!("MARKER-NEEDS-91c2");
    let o = w.qa_record(&inputs, &out, serde_json::to_vec(&j).unwrap(), &[]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    let line = refusal_line(&o);
    assert!(
        line.contains("refused: handoff-incomplete at qa_handoff.owner"),
        "{line}"
    );
    assert!(line.len() <= 200, "{line}");
    assert!(!text(&o.stderr).contains("MARKER"), "value not repeated");
    assert!(o.stdout.is_empty(), "{}", show(&o));
    assert!(!out.exists(), "nothing written after a refusal");
}

/// The record's shape checks, through the binary. The full rule table is
/// in the unit tests beside qa_record.rs.
#[test]
fn qa_record_refusals_write_nothing() {
    let w = World::new();
    w.node_repo();
    let inputs = w.inputs_fixture();
    let mut cases: Vec<(&str, Vec<u8>, &str)> = Vec::new();
    let mut j = docs_judgement();
    j["head_sha"] = json!(w.git(&["rev-parse", "HEAD"]));
    cases.push((
        "head_sha on stdin",
        serde_json::to_vec(&j).unwrap(),
        "schema",
    ));
    let mut j = docs_judgement();
    j["qa_handoff"] = handoff_judgement()["qa_handoff"].clone();
    cases.push((
        "hand-off without HANDOFF",
        serde_json::to_vec(&j).unwrap(),
        "handoff-without-status",
    ));
    let mut j = handoff_judgement();
    j["qa_handoff"]["owner"] = json!("ops\u{1b}[2J MARKER-ANSI-77");
    cases.push(("ANSI in owner", serde_json::to_vec(&j).unwrap(), "schema"));
    let mut j = docs_judgement();
    j["qa_cited_files"] = json!(["../etc/passwd"]);
    cases.push(("parent path", serde_json::to_vec(&j).unwrap(), "schema"));
    let mut j = docs_judgement();
    j["qa_cited_files"] = json!([]);
    cases.push((
        "no cited file",
        serde_json::to_vec(&j).unwrap(),
        "not-user-facing-needs-cited-files",
    ));
    let mut big = vec![b' '; (1 << 20) + 1];
    big.extend_from_slice(&serde_json::to_vec(&docs_judgement()).unwrap());
    cases.push(("over 1 MiB", big, "input-too-large"));
    cases.push(("not JSON", b"qa_status=PASS".to_vec(), "schema"));

    for (name, stdin, rule) in cases {
        let out = w.state.join("refused.json");
        let o = w.qa_record(&inputs, &out, stdin, &[]);
        assert_eq!(o.status.code(), Some(2), "{name}: {}", show(&o));
        let line = refusal_line(&o);
        assert!(line.contains(&format!("refused: {rule}")), "{name}: {line}");
        assert!(line.len() <= 200, "{name}: {line}");
        assert!(
            !text(&o.stderr).contains("MARKER"),
            "{name}: value repeated"
        );
        assert!(!text(&o.stderr).contains('\u{1b}'), "{name}: escape echoed");
        assert!(!out.exists(), "{name}: nothing written");
    }
}

/// head_sha comes from the repository with GIT_* removed: a GIT_DIR naming
/// another repository does not change it.
#[test]
fn qa_record_reads_head_from_repo_ignoring_git_env() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    let decoy = w.base.join("decoy");
    private_dir(&decoy);
    w.git_in(&decoy, &["init", "-q", "-b", "main"]);
    fs::write(decoy.join("x"), "x").unwrap();
    w.git_in(&decoy, &["add", "x"]);
    w.git_in(&decoy, &["commit", "-q", "-m", "decoy"]);
    let inputs = w.inputs_fixture();
    let out = w.evidence_path();
    let mut c = w.cmd(w.bin.join("amplihack"));
    c.args(["orch", "helper", "autodrive", "qa-record", "--inputs"])
        .arg(&inputs)
        .arg("--repo")
        .arg(&w.repo)
        .arg("--out")
        .arg(&out)
        .env("GIT_DIR", decoy.join(".git"))
        .current_dir(&w.repo);
    let o = run_with_stdin(c, serde_json::to_vec(&docs_judgement()).unwrap());
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(
        read_json(&out)["head_sha"].as_str(),
        Some(w.git(&["rev-parse", "HEAD"]).as_str())
    );
}

#[test]
fn qa_record_environment_errors_exit_1_and_write_nothing() {
    let w = World::new();
    w.node_repo();
    let inputs = w.inputs_fixture();
    let body = serde_json::to_vec(&docs_judgement()).unwrap();

    // The inputs file as a symlink.
    let link = w.state.join("inputs-link.json");
    symlink(&inputs, &link).unwrap();
    let out = w.state.join("a.json");
    let o = w.qa_record(&link, &out, body.clone(), &[]);
    assert_eq!(o.status.code(), Some(1), "symlinked inputs: {}", show(&o));
    assert!(!out.exists());

    // A missing inputs file.
    let o = w.qa_record(&w.state.join("absent.json"), &out, body.clone(), &[]);
    assert_eq!(o.status.code(), Some(1), "missing inputs: {}", show(&o));
    assert!(!out.exists());

    // An output directory writable by group.
    let shared = w.base.join("shared-state");
    fs::create_dir_all(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).unwrap();
    let o = w.qa_record(&inputs, &shared.join("qa-evidence.json"), body.clone(), &[]);
    assert_eq!(
        o.status.code(),
        Some(1),
        "group-writable parent: {}",
        show(&o)
    );
    assert!(!shared.join("qa-evidence.json").exists());

    // An output directory readable by group: the evidence store's own rule.
    let readable = w.base.join("readable-state");
    fs::create_dir_all(&readable).unwrap();
    fs::set_permissions(&readable, fs::Permissions::from_mode(0o750)).unwrap();
    let o = w.qa_record(
        &inputs,
        &readable.join("qa-evidence.json"),
        body.clone(),
        &[],
    );
    assert_eq!(
        o.status.code(),
        Some(1),
        "group-readable parent: {}",
        show(&o)
    );
    assert!(!readable.join("qa-evidence.json").exists());

    // A repository with no HEAD.
    let empty = w.base.join("empty-repo");
    private_dir(&empty);
    w.git_in(&empty, &["init", "-q"]);
    let mut c = w.cmd(w.bin.join("amplihack"));
    c.args(["orch", "helper", "autodrive", "qa-record", "--inputs"])
        .arg(&inputs)
        .arg("--repo")
        .arg(&empty)
        .arg("--out")
        .arg(&out);
    let o = run_with_stdin(c, body);
    assert_eq!(o.status.code(), Some(1), "no HEAD: {}", show(&o));
    assert!(!out.exists());
}

/// A symlink at --out is replaced by a private regular file; its target is
/// never written. The mode is 0600 even under umask 000.
#[test]
fn qa_record_replaces_a_symlink_at_out_and_stays_private_under_umask_000() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    let inputs = w.inputs_fixture();
    let victim = w.base.join("victim.txt");
    fs::write(&victim, "untouched").unwrap();
    let out = w.evidence_path();
    symlink(&victim, &out).unwrap();

    let mut c = w.cmd("sh");
    c.arg("-c")
        .arg("umask 000; exec \"$0\" \"$@\"")
        .arg(w.bin.join("amplihack"))
        .args(["orch", "helper", "autodrive", "qa-record", "--inputs"])
        .arg(&inputs)
        .arg("--repo")
        .arg(&w.repo)
        .arg("--out")
        .arg(&out)
        .current_dir(&w.repo);
    let o = run_with_stdin(c, serde_json::to_vec(&docs_judgement()).unwrap());
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched");
    let meta = fs::symlink_metadata(&out).unwrap();
    assert!(meta.file_type().is_file(), "--out is now a regular file");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(read_json(&out)["qa_status"], "PASS");
}

/// R2.3: an inputs field copied into the evidence is held to the judgement's
/// text rule. A control character exits 1, names the field and not the
/// value, and writes nothing. Unknown inputs fields are ignored (R2.2).
#[test]
fn qa_record_refuses_control_characters_in_copied_inputs_fields() {
    let w = World::new();
    w.node_repo();
    w.docs_pr();
    let inputs = w.inputs_fixture();
    let body = serde_json::to_vec(&docs_judgement()).unwrap();
    let out = w.evidence_path();

    let mut v = read_json(&inputs);
    v["declared_qa"]["command"] = json!("npm test\u{1b}[31mFORGED-5c2a");
    fs::write(&inputs, serde_json::to_vec(&v).unwrap()).unwrap();
    let o = w.qa_record(&inputs, &out, body.clone(), &[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let err = text(&o.stderr);
    assert!(err.contains("inputs.declared_qa.command"), "{}", show(&o));
    assert!(
        !err.contains("FORGED-5c2a"),
        "the value is never repeated: {}",
        show(&o)
    );
    assert!(!out.exists(), "--out is never created");

    let mut v = read_json(&w.inputs_fixture());
    v["kind"] = json!("added-by-a-newer-script");
    v["declared_qa"]["scenarios"] = json!([]);
    fs::write(&inputs, serde_json::to_vec(&v).unwrap()).unwrap();
    let o = w.qa_record(&inputs, &out, body, &[]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "unknown inputs fields are ignored: {}",
        show(&o)
    );
}
