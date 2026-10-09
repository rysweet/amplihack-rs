use super::fixtures::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[path = "checkpoint_fixture.rs"]
mod checkpoint;

pub struct NativeRun {
    pub code: i32,
    pub stderr: String,
    pub child_stderr: String,
    stdout: Vec<u8>,
}
impl NativeRun {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.stdout)
            .unwrap_or_else(|e| panic!("native JSON: {e}: {}", self.stderr))
    }
}

pub struct NativeFixture {
    temp: tempfile::TempDir,
    runner: PathBuf,
    home: PathBuf,
}

fn copy_tools(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            copy_tools(&path, &target.join(entry.file_name()));
        } else if path.extension().is_some_and(|ext| ext == "sh") {
            let copied = target.join(entry.file_name());
            fs::copy(&path, &copied).unwrap();
            assert_eq!(fs::read(&path).unwrap(), fs::read(copied).unwrap());
        }
    }
}

impl NativeFixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let mut install = Command::new(env!("CARGO_BIN_EXE_amplihack"));
        install
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("public tool PATH"))
            .env("CARGO_HOME", home.join(".cargo"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_RUNTIME_DIR", home.join(".runtime"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("CLAUDE_PLUGIN_DATA", home.join("claude-plugin-data"))
            .env("COPILOT_CONFIG_DIR", home.join(".copilot"))
            .env("GH_CONFIG_DIR", home.join(".config/gh"))
            .env("RUSTC_WRAPPER", "")
            .env("CARGO_BUILD_JOBS", "8")
            .args(["install", "--local"])
            .arg(repo())
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("HOME", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("COPILOT_HOME", home.join(".copilot"))
            .env("TMPDIR", temp.path());
        // Public read-only selectors and a supported real runner override are
        // explicit; model/Git credentials and writable roots are never copied.
        for key in ["RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "RECIPE_RUNNER_RS_PATH"] {
            if let Some(value) = std::env::var_os(key) {
                install.env(key, value);
            }
        }
        let output = install
            .output()
            .expect("install current private CLI/assets");
        fs::write(
            temp.path().join("install-exit.json"),
            json!({"exit":output.status.code()}).to_string(),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "private install: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let runner = std::env::var_os("RECIPE_RUNNER_RS_PATH")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .and_then(|path| {
                let path = if let Ok(rest) = path.strip_prefix("~") {
                    home.join(rest)
                } else {
                    path
                };
                if path.components().count() > 1 {
                    path.is_file().then_some(path)
                } else {
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                        .map(|directory| directory.join(&path))
                        .find(|candidate| candidate.is_file())
                }
            })
            .or_else(|| {
                let managed = home.join(".cargo/bin/recipe-runner-rs");
                managed.is_file().then_some(managed)
            })
            .expect("real explicit runner or coherent private Cargo HOME installation required");
        Self { temp, runner, home }
    }

    pub fn run(&self, recipe: &str, id: &str, variant: &str, overrides: Value) -> NativeRun {
        let case = self.temp.path().join(format!("{recipe}-{variant}"));
        fs::create_dir(&case).unwrap();
        let target = match variant {
            "main" => repo(),
            "target-metachar" => case.join("target ' \" $(touch TARGET_INJECTION) ; [x]"),
            _ => case.join("ordinary target"),
        };
        if variant != "main" {
            fs::create_dir(&target).unwrap();
            assert!(!target.join("amplifier-bundle").exists());
        }
        let framework = case.join(if variant == "framework-metachar" {
            "framework ' \" $(touch FRAMEWORK_INJECTION) ; [x]"
        } else {
            "private framework"
        });
        let tools = framework.join("amplifier-bundle/tools");
        copy_tools(&repo().join("amplifier-bundle/tools"), &tools);
        if variant == "missing" {
            fs::remove_file(tools.join("workflow_context.sh")).unwrap();
        }
        let mut context = json!({"repo_path":target,"worktree_setup":{"worktree_path":target},
            "goal_already_met":"false","resume_checkpoint":"","allow_no_op":false,
            "verdict_json":{"verdict":"WORK_VERIFIED","evidence":["controlled native fixture"]},
            "doc_review_feedback":{"status":"OK"},"agentic_finalizer_narrative":"controlled report"});
        context
            .as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        // Preserve exact source command and all canonical metadata. No body()
        // rendering, scalar aliases or dispatcher replacements enter this test.
        let selected = step(recipe, id);
        let fixture = json!({"name":"canonical-helper-source-regression","version":"1.0.0",
            "context":context,"steps":[selected]});
        let recipe_file = case.join("recipe.yaml");
        fs::write(&recipe_file, serde_yaml::to_string(&fixture).unwrap()).unwrap();
        let observer = case.join("observer.sh");
        let child_stderr = case.join("child.stderr");
        fs::write(&observer, "printf '%s\\n' \"$BASH_EXECUTION_STRING\" > \"$SOURCE_RENDERED_BODY\"\nexec 2> >(tee \"$SOURCE_CHILD_STDERR\" >&2)\n").unwrap();
        let output = Command::new(&self.runner)
            .arg(&recipe_file)
            .arg("--no-auto-stage")
            .args(["--output-format", "json", "--audit-dir"])
            .arg(case.join("audit"))
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    Path::new(env!("CARGO_BIN_EXE_amplihack"))
                        .parent()
                        .unwrap()
                        .display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("CARGO_HOME", self.home.join(".cargo"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_RUNTIME_DIR", self.home.join(".runtime"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("CLAUDE_PLUGIN_DATA", self.home.join("claude-plugin-data"))
            .env("COPILOT_CONFIG_DIR", self.home.join(".copilot"))
            .env("GH_CONFIG_DIR", self.home.join(".config/gh"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("COPILOT_HOME", self.home.join(".copilot"))
            .env("TMPDIR", &case)
            .env("AMPLIHACK_HOME", &framework)
            .env("BASH_ENV", &observer)
            .env("SOURCE_RENDERED_BODY", case.join("rendered-command.sh"))
            .env("SOURCE_CHILD_STDERR", &child_stderr)
            .current_dir(&target)
            .output()
            .unwrap();
        // Real native exit is saved before postchecks and decoding.
        fs::write(
            case.join("exit-FIRST.json"),
            json!({"exit":output.status.code(),"runner":self.runner}).to_string(),
        )
        .unwrap();
        fs::write(case.join("stdout.json"), &output.stdout).unwrap();
        fs::write(case.join("stderr.log"), &output.stderr).unwrap();
        assert!(!target.join("TARGET_INJECTION").exists());
        assert!(!target.join("FRAMEWORK_INJECTION").exists());
        assert!(
            !fs::read(case.join("rendered-command.sh"))
                .unwrap()
                .is_empty()
        );
        NativeRun {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into(),
            child_stderr: fs::read_to_string(child_stderr).unwrap(),
            stdout: output.stdout,
        }
    }
}
