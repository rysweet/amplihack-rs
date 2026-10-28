//! Small real projects and process-level fixtures for the evidence CLI contract.
//! No successful capture, digest, exit receipt or terminal completion is mocked.
//! Native proofs are opt-in where dependencies/toolchains are not guaranteed.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

pub const STARTED: &str = "2026-10-27T23:03:28Z";

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

pub fn tool(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

pub fn recipe_step(recipe: &str, id: &str) -> serde_yaml::Value {
    let text = fs::read_to_string(
        workspace_root()
            .join("amplifier-bundle/recipes")
            .join(format!("{recipe}.yaml")),
    )
    .unwrap();
    let value: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    value["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap()
        .clone()
}

pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

pub fn sha(bytes: impl AsRef<[u8]>) -> String {
    let mut child = Command::new("python3")
        .args([
            "-c",
            "import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bytes.as_ref())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().into()
}

pub struct Run(pub Output);
impl Run {
    pub fn describe(&self) -> String {
        format!(
            "exit={:?}\nstdout={}\nstderr={}",
            self.0.status.code(),
            String::from_utf8_lossy(&self.0.stdout),
            String::from_utf8_lossy(&self.0.stderr)
        )
    }
    pub fn exit(&self, expected: i32) {
        assert_eq!(self.0.status.code(), Some(expected), "{}", self.describe());
    }
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.0.stdout).unwrap_or_else(|e| {
            panic!("one complete JSON value required: {e}\n{}", self.describe())
        })
    }
    pub fn error(&self, code: &str) {
        self.exit(2);
        let value = self.json();
        assert_eq!(value["ok"], false, "{}", self.describe());
        assert_eq!(value["error"]["code"], code, "{}", self.describe());
        assert!(
            value["error"]["required_next_action"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
    }
}

pub fn verification(run: &Run, completed: bool) -> Value {
    run.exit(0); // JSON emission success is independent of verification success.
    let value = run.json();
    let object = value.as_object().expect("verification object");
    assert_eq!(
        object.len(),
        4,
        "transport stays exactly four Strings: {value}"
    );
    assert!(object.values().all(Value::is_string), "{value}");
    assert_eq!(
        value["verification_completed"],
        if completed { "true" } else { "false" },
        "verification decision: {value}"
    );
    assert_eq!(value["terminal_no_op"], "false");
    assert_eq!(
        value["terminal_state"],
        if completed {
            "VERIFICATION_COMPLETED"
        } else {
            "VERIFICATION_UNPROVEN"
        }
    );
    assert!(!value["terminal_reason"].as_str().unwrap().is_empty());
    value
}

pub struct Fixture {
    pub tmp: TempDir,
    pub repo: PathBuf,
    pub context: Value,
    pub plan: Value,
    pub env: BTreeMap<String, String>,
}

impl Fixture {
    pub fn javascript() -> Self {
        let mut tmp = if let Some(root) = std::env::var_os("WORKFLOW_TEST_RETAIN_ROOT") {
            fs::create_dir_all(&root).unwrap();
            tempfile::Builder::new()
                .prefix("native-")
                .tempdir_in(root)
                .unwrap()
        } else {
            TempDir::new().unwrap()
        };
        if std::env::var_os("WORKFLOW_TEST_RETAIN_ROOT").is_some() {
            tmp.disable_cleanup(true);
        }
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        for dir in ["home", "scratch", "bin"] {
            fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        let mut f = Self {
            tmp,
            repo,
            context: json!({}),
            plan: json!({}),
            env: BTreeMap::new(),
        };
        f.native("git", &["init", "-b", "feature"]);
        f.native(
            "git",
            &["config", "user.email", "workflow-test@example.invalid"],
        );
        f.native("git", &["config", "user.name", "Workflow evidence test"]);
        // A local repository needs neither GitHub identity plumbing nor a network.
        f.file(
            ".gitignore",
            "node_modules/\ntarget/\n.venv/\n__pycache__/\nbin/\nobj/\n",
        );
        f.file("package.json", r#"{"name":"evidence-consumer","private":true,"type":"module","scripts":{"test":"node --test contract.test.js","quality":"node --check contract.js"}}"#);
        f.file("contract.js", "export function decode(value) { if (!Array.isArray(value)) throw new TypeError('array required'); return value.map(x => x * 2); }\n");
        f.file("contract.test.js", "import {test} from 'node:test'; import assert from 'node:assert/strict'; import {decode} from './contract.js'; test('consumer value', () => assert.deepEqual(decode([2]), [4])); test('invalid input', () => assert.throws(() => decode(null), TypeError));\n");
        f.file("consumer.js", "import assert from 'node:assert/strict'; import {decode} from './contract.js'; const role=process.argv[2]; if (role==='basic') assert.deepEqual(decode([2]),[4]); else if (role==='edge') { assert.deepEqual(decode([]),[]); assert.throws(()=>decode(null),TypeError); } else process.exit(7); console.log(role+' consumer assertions passed');\n");
        f.file("README.md", "Maintained component: repository root. Test: npm test. Quality: npm run quality. Consumer: node consumer.js basic; node consumer.js edge. No pre-commit configuration or hooks are prescribed. Generated dependency/build directories are not source.\n");
        f.commit("native consumer fixture");
        f.context = json!({"repo_path":f.repo,"worktree_setup":{"worktree_path":f.repo},
            "branch_name":"feature","base_ref":"feature","remote_host_type":"other",
            "workflow_started_at":STARTED,"allow_no_op":false,
            "task_description":"verify the native public decoder", "unrelated":{"preserve":"original"},
            "finalizer_step_status":{"status":"ok","reporting_failure":"false"}});
        f.plan = f.node_plan("javascript");
        f
    }

    pub fn file(&self, name: &str, bytes: impl AsRef<[u8]>) {
        write(self.repo.join(name), bytes);
    }

    pub fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .current_dir(&self.repo)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.tmp.path().join("home"))
            .env("TMPDIR", self.tmp.path().join("scratch"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("AMPLIHACK_HOME", workspace_root())
            .env("REPO_PATH", &self.repo)
            .env("WORKFLOW_STARTED_AT", STARTED)
            .env("AMPLIHACK_CONTEXT_FILE", self.context_path())
            .env("CARGO_BUILD_JOBS", "2")
            .env("CARGO_NET_OFFLINE", "true")
            .env("GOTOOLCHAIN", "local")
            .env("GOPROXY", "off")
            .env("GOMAXPROCS", "2")
            .env("GOFLAGS", "-p=2")
            .env("UV_OFFLINE", "1")
            .env("DOTNET_PROCESSOR_COUNT", "2")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1");
        // Real installed toolchains, no copied/simulated executables.
        for key in ["RUSTUP_HOME", "CARGO_HOME", "DOTNET_ROOT"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        if std::env::var_os("RUSTUP_HOME").is_none()
            && let Some(home) = std::env::var_os("HOME")
        {
            cmd.env("RUSTUP_HOME", PathBuf::from(home).join(".rustup"));
        }
        cmd.envs(&self.env);
        cmd
    }

    pub fn native(&self, program: &str, argv: &[&str]) -> Run {
        let run = Run(self.command(program).args(argv).output().unwrap());
        run.exit(0);
        run
    }

    pub fn commit(&self, message: &str) {
        self.native("git", &["add", "."]);
        self.native("git", &["commit", "-m", message]);
    }

    pub fn head(&self) -> String {
        String::from_utf8(self.native("git", &["rev-parse", "HEAD"]).0.stdout)
            .unwrap()
            .trim()
            .into()
    }

    pub fn context_path(&self) -> PathBuf {
        self.tmp.path().join("context.json")
    }

    pub fn save_context(&self) {
        write(
            self.context_path(),
            serde_json::to_vec(&self.context).unwrap(),
        );
    }

    pub fn identity(&self) -> Value {
        json!({"schema_version":1,"workflow":{"name":"default-workflow","started_at":STARTED,"original_run_id":"fixture-original-run"},"repository_root":self.repo})
    }

    pub fn request(&self) -> Value {
        let mut v = self.identity();
        v["context_file"] = json!(self.context_path());
        v
    }

    pub fn run_raw(&self, args: &[&str], input: &[u8]) -> Run {
        self.save_context();
        let mut child = self
            .command("bash")
            .arg(tool("workflow_terminal_evidence.sh"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        Run(child.wait_with_output().unwrap())
    }

    pub fn call(&self, mode: &str, request: &Value) -> Run {
        self.run_raw(
            &[mode, "--request-file", "-"],
            &serde_json::to_vec(request).unwrap(),
        )
    }

    pub fn finalize(&self, action: &str) -> Run {
        self.run_raw(
            &["finalize", "--action", action, "--request-file", "-"],
            &serde_json::to_vec(&self.request()).unwrap(),
        )
    }

    pub fn step(&self, recipe: &str, id: &str) -> Run {
        self.save_context();
        let step = recipe_step(recipe, id);
        Run(self
            .command("bash")
            .arg("-c")
            .arg(step["command"].as_str().unwrap())
            .output()
            .unwrap())
    }

    pub fn reference(&self, name: &str) -> Value {
        json!({"path":name,"sha256":sha(fs::read(self.repo.join(name)).unwrap()),"purpose":"maintained command and native assertions"})
    }

    pub fn obligation(
        &self,
        id: &str,
        kind: &str,
        toolchain: &str,
        runner: &str,
        argv: &[&str],
    ) -> Value {
        let mut v = json!({"id":id,"component_ids":["root"],"kind":kind,"toolchain":toolchain,
            "runner":runner,"argv":argv,"cwd":self.repo,"scope":"COMPONENT",
            "selected_from":[self.reference("README.md")]});
        if kind == "consumer" {
            v["scenario"] = json!({"role":if id=="basic" {"basic"} else {"edge_or_integration"},
                "assertions":[if id=="basic" {"public decoder doubles [2] to [4]"} else {"empty input and invalid input retain their contract"}],
                "inspected":[self.reference("README.md")],
                "entry_points":[match toolchain { "rust" => "tests/consumer.rs", "python" => "consumer.py", "go" => "cmd/consumer/consumer_test.go", "dotnet" => "Program.cs", _ if runner == "vitest" => "consumer.test.ts", _ => "consumer.js" }],
                "cases":[if toolchain=="go" {if id=="basic" {"TestBasic"} else {"TestEdge"}} else if id=="basic" {"basic"} else {"edge"}],
                "assertion_refs":[self.reference(match toolchain { "rust" => "tests/consumer.rs", "python" => "consumer.py", "go" => "cmd/consumer/consumer_test.go", "dotnet" => "Program.cs", _ if runner == "vitest" => "consumer.test.ts", _ => "consumer.js" })]});
        }
        v
    }

    pub fn node_plan(&self, language: &str) -> Value {
        let mut p = self.identity();
        p["components"] = json!([{"id":"root","root":".","toolchain":language,"disposition":"required",
            "inspected":[self.reference("package.json"),self.reference("README.md")],"justification":"maintained root package"}]);
        p["obligations"] = json!([
            self.obligation("tests", "test", language, "node:test", &["npm", "test"]),
            self.obligation(
                "quality",
                "quality",
                language,
                "node",
                &["npm", "run", "quality"]
            ),
            self.obligation(
                "basic",
                "consumer",
                language,
                "node",
                &["node", "consumer.js", "basic"]
            ),
            self.obligation(
                "edge",
                "consumer",
                language,
                "node",
                &["node", "consumer.js", "edge"]
            )
        ]);
        p["exclusions"] = json!([{"id":"no-hooks","component_ids":["root"],"subject":"precommit",
            "reason":"no prescribed pre-commit configuration or hooks","policy_refs":[self.reference("README.md")]}]);
        p
    }

    pub fn register(&self) -> Value {
        let run = self.call("plan", &self.plan);
        run.exit(0);
        let v = run.json();
        assert_eq!(v["ok"], true, "{}", run.describe());
        assert_eq!(v["status"], "READY");
        assert!(v["plan_id"].as_str().is_some_and(|s| !s.is_empty()));
        v
    }

    pub fn capture_request(&self, plan: &Value, id: &str) -> Value {
        let o = self.plan["obligations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == id)
            .unwrap();
        let mut r = self.identity();
        r["plan_id"] = plan["plan_id"].clone();
        r["obligation_id"] = json!(id);
        r["argv"] = o["argv"].clone();
        r["cwd"] = o["cwd"].clone();
        r["producer_step"] = json!(if o["kind"] == "consumer" {
            "step-13-local-testing"
        } else {
            "step-12-run-precommit"
        });
        r
    }

    pub fn capture(&self, plan: &Value, id: &str) -> Value {
        let run = self.call("capture", &self.capture_request(plan, id));
        run.exit(0);
        let v = run.json();
        assert_eq!(v["ok"], true, "{}", run.describe());
        assert_eq!(v["process_exit_code"], 0);
        v
    }

    pub fn verify(&self, plan: &Value) -> Run {
        let mut r = self.request();
        r["plan_id"] = plan["plan_id"].clone();
        self.call("verify", &r)
    }

    pub fn execute_all(&mut self, plan: &Value) -> Vec<Value> {
        let obligations = self.plan["obligations"].as_array().unwrap().clone();
        let mut checks = vec![];
        let mut scenarios = vec![];
        let mut captures = vec![];
        for o in obligations {
            let id = o["id"].as_str().unwrap();
            let capture = self.capture(plan, id);
            if o["kind"] == "consumer" {
                scenarios.push(
                    json!({"obligation_id":id,"execution_id":capture["execution_id"],
                    "role":o["scenario"]["role"],"assertions":o["scenario"]["assertions"]}),
                );
            } else {
                checks.push(json!({"obligation_id":id,"execution_ids":[capture["execution_id"]]}));
            }
            captures.push(capture);
        }
        self.context["precommit_results"] = json!({"schema_version":1,"plan_id":plan["plan_id"],"status":"PASS","checks":checks,"exclusions":["no-hooks"]});
        self.context["local_testing_gate"] = json!({"schema_version":1,"plan_id":plan["plan_id"],"status":"PASS","scenarios":scenarios});
        captures
    }

    pub fn store(&self) -> PathBuf {
        let value = self.native("git", &["rev-parse", "--absolute-git-dir"]);
        PathBuf::from(String::from_utf8(value.0.stdout).unwrap().trim())
            .join("amplihack/workflow-evidence/v1")
    }

    pub fn artifact_path(&self, artifact: &Value) -> PathBuf {
        let relative = Path::new(artifact["path"].as_str().expect("artifact path"));
        assert!(!relative.is_absolute(), "store-relative artifact required");
        let candidates = files_under(&self.store())
            .into_iter()
            .filter(|p| p.ends_with(relative))
            .collect::<Vec<_>>();
        assert_eq!(
            candidates.len(),
            1,
            "one artifact for {}: {candidates:?}",
            relative.display()
        );
        candidates[0].clone()
    }

    pub fn artifact(&self, artifact: &Value) -> Vec<u8> {
        let bytes = fs::read(self.artifact_path(artifact)).unwrap();
        assert_eq!(artifact["bytes"], json!(bytes.len()));
        assert_eq!(artifact["sha256"], sha(&bytes));
        bytes
    }

    pub fn execution(&self, capture: &Value) -> Value {
        serde_json::from_slice(&self.artifact(&capture["record"])).unwrap()
    }

    pub fn install_original_implementation(&mut self) {
        // Execute real source inspection and verification surrounded by the
        // observer. The fixture uses the documented stock verifier output.
        // A linked worktree has a .git file. Keep fixture scripts inside its
        // root so the observer can bind them without admitting external inputs.
        let directory = if self.repo.join(".git").is_dir() {
            ".git"
        } else {
            "node_modules/.workflow-fixture"
        };
        let builder = format!("{directory}/fixture-builder.py");
        let verifier = format!("{directory}/fixture-verifier.py");
        self.file(&builder, "from pathlib import Path\ns=Path('contract.js').read_text()\nassert 'decode' in s\nprint('Files modified: contract.js\\nDiff summary: public decoder with invalid-input rejection.')\n");
        self.file(&verifier, "import json,subprocess\nfrom pathlib import Path\nassert 'decode' in Path('contract.js').read_text()\nsubprocess.run(['node','--test','contract.test.js'],check=True)\nprint('Inspected and executed concrete decoder assertions.')\nprint(json.dumps(dict(verdict='WORK_VERIFIED',evidence=['contract.js','contract.test.js'],rationale='Inspected and executed public decoder assertions')))\n");
        let mut request = self.request();
        request["producer_argv"] = json!(["python3", builder]);
        request["verifier_argv"] = json!(["python3", verifier]);
        request["cwd"] = json!(self.repo);
        let run = self.call("observe-implementation", &request);
        run.exit(0);
        let cohort: Value = serde_json::from_slice(&self.artifact(&run.json()["cohort"])).unwrap();
        for (key, value) in cohort["implementation"]["records"].as_object().unwrap() {
            self.context[key] = value.clone();
        }
    }

    pub fn checkpoint(&self) -> Value {
        let run = self.call("checkpoint-implementation", &self.request());
        run.exit(0);
        assert_eq!(run.json()["implementation_state"], "VALID");
        run.json()
    }

    pub fn verified_cohort(&mut self) -> Value {
        self.install_original_implementation();
        self.checkpoint();
        let p = self.register();
        self.execute_all(&p);
        self.context["verification_terminal_evidence"] = verification(&self.verify(&p), true);
        p
    }

    pub fn restore(&self) -> Run {
        self.call("restore", &self.request())
    }

    pub fn derived(&self, response: &Value) -> Value {
        serde_json::from_slice(&self.artifact(&response["derived_context"])).unwrap()
    }

    pub fn native_project(language: &str) -> Self {
        let mut f = Self::javascript();
        for name in ["package.json", "contract.test.js", "consumer.js"] {
            fs::remove_file(f.repo.join(name)).unwrap();
        }
        let (manifest, test, runner, basic, edge) = match language {
            "rust" => {
                // Native proofs resolve the installed rustup proxy directly.
                // A host's shell redirection wrapper remains unproven under the
                // production Rust rule; do not add a host-specific exception.
                let cargo_home = std::env::var_os("CARGO_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo")
                    });
                let proxy_dir = cargo_home.join("bin");
                assert!(
                    proxy_dir.join("cargo").is_file(),
                    "installed native Cargo proxy required"
                );
                f.env.insert(
                    "PATH".into(),
                    format!("{}:{}", proxy_dir.display(), std::env::var("PATH").unwrap()),
                );
                f.file("Cargo.toml","[package]\nname='evidence-native'\nversion='0.1.0'\nedition='2021'\n[workspace]\nmembers=['.']\n");
                f.file("src/lib.rs","pub fn decode(v: &[i32]) -> Vec<i32> { v.iter().map(|x|x*2).collect() }\n#[test] fn native_contract() { assert_eq!(decode(&[2]),vec![4]); assert_eq!(decode(&[]),vec![]); }\n");
                f.file("tests/consumer.rs", "#[test] fn basic() { assert_eq!(evidence_native::decode(&[2]),vec![4]); }\n#[test] fn edge() { assert_eq!(evidence_native::decode(&[]),vec![]); }\n");
                f.native("cargo", &["generate-lockfile", "--offline"]);
                (
                    "Cargo.toml",
                    vec!["cargo", "test", "--workspace", "--locked"],
                    "cargo",
                    vec![
                        "cargo",
                        "test",
                        "--locked",
                        "--test",
                        "consumer",
                        "basic",
                        "--",
                        "--exact",
                        "--nocapture",
                    ],
                    vec![
                        "cargo",
                        "test",
                        "--locked",
                        "--test",
                        "consumer",
                        "edge",
                        "--",
                        "--exact",
                        "--nocapture",
                    ],
                )
            }
            "python" => {
                f.file("pyproject.toml","[project]\nname='evidence-native'\nversion='0.1.0'\nrequires-python='>=3.10'\n");
                f.file("decoder.py","def decode(values):\n    if not isinstance(values, list): raise TypeError('array required')\n    return [x * 2 for x in values]\n");
                f.file("test_contract.py","import unittest\nfrom decoder import decode\nclass Contract(unittest.TestCase):\n    def test_basic(self): self.assertEqual(decode([2]), [4])\n    def test_edge(self):\n        self.assertEqual(decode([]), [])\n        with self.assertRaises(TypeError): decode(None)\n");
                f.file("consumer.py", "import unittest\nfrom decoder import decode\nclass Consumer(unittest.TestCase):\n    def basic(self): self.assertEqual(decode([2]),[4])\n    def edge(self): self.assertEqual(decode([]),[])\n");
                (
                    "pyproject.toml",
                    vec![
                        "uv",
                        "run",
                        "--offline",
                        "--no-sync",
                        "python",
                        "-m",
                        "unittest",
                        "test_contract",
                    ],
                    "unittest",
                    vec![
                        "uv",
                        "run",
                        "--offline",
                        "--no-sync",
                        "python",
                        "-m",
                        "unittest",
                        "-v",
                        "consumer.Consumer.basic",
                    ],
                    vec![
                        "uv",
                        "run",
                        "--offline",
                        "--no-sync",
                        "python",
                        "-m",
                        "unittest",
                        "-v",
                        "consumer.Consumer.edge",
                    ],
                )
            }
            "go" => {
                f.file("go.mod", "module example.invalid/evidence\n\ngo 1.22\n");
                f.file("decoder.go","package evidence\nfunc Decode(v []int) []int { r:=make([]int,len(v)); for i,n:=range v { r[i]=n*2 }; return r }\n");
                f.file("decoder_test.go","package evidence\nimport \"testing\"\nfunc TestContract(t *testing.T) { if Decode([]int{2})[0]!=4 || len(Decode(nil))!=0 { t.Fatal(\"decoder contract\") } }\n");
                f.file("cmd/consumer/consumer_test.go", "package consumer\nimport (\"testing\"; e \"example.invalid/evidence\")\nfunc TestBasic(t *testing.T) { if e.Decode([]int{2})[0]!=4 { t.Fatal(\"basic\") } }\nfunc TestEdge(t *testing.T) { if len(e.Decode(nil))!=0 { t.Fatal(\"edge\") } }\n");
                (
                    "go.mod",
                    vec!["go", "test", "./..."],
                    "go",
                    vec!["go", "test", "-v", "-run", "^TestBasic$", "./cmd/consumer"],
                    vec!["go", "test", "-v", "-run", "^TestEdge$", "./cmd/consumer"],
                )
            }
            "dotnet" => {
                f.file("Contract.csproj","<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup></Project>\n");
                f.file(
                    "NuGet.Config",
                    "<configuration><packageSources><clear /></packageSources></configuration>\n",
                );
                f.file("Program.cs","static int[] Decode(int[] v) => v.Select(x => x*2).ToArray();\nvoid Basic() { if (!Decode(new[]{2}).SequenceEqual(new[]{4})) throw new Exception(\"basic\"); }\nvoid Edge() { if (Decode(Array.Empty<int>()).Length!=0) throw new Exception(\"edge\"); }\nswitch(args[0]) { case \"check\": Basic(); Edge(); break; case \"basic\": Basic(); break; case \"edge\": Edge(); break; default: return 7; }\nConsole.WriteLine(args[0]+\" consumer assertions passed\"); return 0;\n");
                (
                    "Contract.csproj",
                    vec![
                        "dotnet",
                        "run",
                        "--project",
                        "Contract.csproj",
                        "--",
                        "check",
                    ],
                    "dotnet",
                    vec![
                        "dotnet",
                        "run",
                        "--project",
                        "Contract.csproj",
                        "--",
                        "basic",
                    ],
                    vec![
                        "dotnet",
                        "run",
                        "--project",
                        "Contract.csproj",
                        "--",
                        "edge",
                    ],
                )
            }
            _ => panic!("unsupported native fixture {language}"),
        };
        f.file("README.md",format!("Maintained {language} root component. Test: {}. Consumer basic: {}. Consumer edge: {}. No pre-commit configuration or hooks prescribed. No additional quality check prescribed.\n",test.join(" "),basic.join(" "),edge.join(" ")));
        f.commit("native language project");
        f.plan = f.identity();
        f.plan["components"] = json!([{"id":"root","root":".","toolchain":language,"disposition":"required",
            "inspected":[f.reference(manifest),f.reference("README.md")],"justification":"maintained native project"}]);
        let mut test_obligation = f.obligation("tests", "test", language, runner, &test);
        if language == "rust" {
            test_obligation["scope"] = json!("FULL_WORKSPACE");
            test_obligation["threading"] = json!("DEFAULT");
        }
        f.plan["obligations"] = json!([
            test_obligation,
            f.obligation("basic", "consumer", language, runner, &basic),
            f.obligation("edge", "consumer", language, runner, &edge)
        ]);
        f.plan["exclusions"] = json!([{"id":"no-hooks","component_ids":["root"],"subject":"precommit","reason":"no prescribed hooks","policy_refs":[f.reference("README.md")]}]);
        f
    }
}

pub fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if root.is_dir() {
        for entry in fs::read_dir(root).unwrap() {
            let p = entry.unwrap().path();
            // Do not follow malicious artifact symlinks during test inspection.
            if fs::symlink_metadata(&p).unwrap().is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}
