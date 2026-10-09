use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command};
use tempfile::TempDir;

pub const SENTINEL: &str = "No files modified — orchestration task";

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn step(recipe: &str, id: &str) -> Value {
    let path = repo().join(format!("amplifier-bundle/recipes/{recipe}.yaml"));
    let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let step = yaml["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing {id}"));
    serde_json::to_value(step).unwrap()
}

pub fn body(recipe: &str, id: &str) -> String {
    // Only the owning path templates are supported. Fail on any other Jinja.
    let quoted_path = repo().to_str().unwrap().replace('\'', "'\\''");
    let mut command = step(recipe, id)["command"].as_str().unwrap().to_owned();
    for key in ["repo_path", "worktree_setup.worktree_path"] {
        for template in [format!("{{{{{key}}}}}"), format!("{{{{ {key} }}}}")] {
            command = command.replace(&template, &quoted_path);
        }
    }
    assert!(
        !command.contains("{{"),
        "unsupported command template: {command}"
    );
    command
}

pub fn verdict(token: &str) -> String {
    json!({"verdict":token,"evidence":["src/actual.rs",{"sha":"abc","checks":[1,2]}],
        "rationale":"concrete artifacts retained"})
    .to_string()
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn exit(&self, expected: i32) {
        assert_eq!(
            self.code, expected,
            "stdout={} stderr={}",
            self.stdout, self.stderr
        );
        for startup_error in [
            "command not found",
            "No such file or directory",
            "Permission denied",
        ] {
            assert!(
                !self.stderr.contains(startup_error),
                "startup failure: {}",
                self.stderr
            );
        }
    }
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "consumer emitted invalid JSON: {e}; {} {}",
                self.stdout, self.stderr
            )
        })
    }
    pub fn helper_state(&self, state: &str) {
        self.exit(0);
        let j = self.json();
        let (completed, noop) = match state {
            "IMPLEMENTATION_COMPLETED" => ("true", "false"),
            "ALLOW_NO_OP" => ("false", "true"),
            "IMPLEMENTATION_UNPROVEN" => ("false", "false"),
            _ => panic!("unknown state"),
        };
        assert_eq!(
            j.as_object().unwrap().len(),
            4,
            "four String fields required"
        );
        assert_eq!(j["terminal_state"], state);
        assert_eq!(j["implementation_completed"], completed);
        assert_eq!(j["terminal_no_op"], noop);
        assert!(j["terminal_reason"].is_string());
    }
    pub fn gate(&self, diagnostic: &str) {
        self.exit(if diagnostic == "HOLLOW_SUCCESS" { 1 } else { 0 });
        assert!(
            self.stderr.contains(diagnostic),
            "expected {diagnostic}: {}",
            self.stderr
        );
        if diagnostic != "APPROVED" {
            assert!(
                !self.stderr.contains("work-verifier APPROVED"),
                "false approval"
            );
        }
    }
}

pub struct Fixture {
    pub temp: TempDir,
    env: Vec<(String, String)>,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        // Use the real freshly built CLI for extract-json/normalise-verdict.
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_amplihack"), bin.join("amplihack")).unwrap();
        #[cfg(not(unix))]
        fs::copy(env!("CARGO_BIN_EXE_amplihack"), bin.join("amplihack.exe")).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let isolated_repo = temp.path().display().to_string();
        Self {
            temp,
            env: vec![
                ("PATH".into(), path),
                ("REPO_PATH".into(), isolated_repo.clone()),
                ("WORKTREE_SETUP_WORKTREE_PATH".into(), isolated_repo),
                ("AMPLIHACK_HOME".into(), repo().display().to_string()),
            ],
        }
    }
    pub fn env(mut self, key: &str, value: impl Into<String>) -> Self {
        self.env.retain(|(k, _)| k != key);
        self.env.push((key.into(), value.into()));
        self
    }
    pub fn file_raw(self, raw: &str) -> Self {
        let p = self.temp.path().join("context.json");
        fs::write(&p, raw).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        self.env("AMPLIHACK_CONTEXT_FILE", p.display().to_string())
    }
    pub fn file(self, context: Value) -> Self {
        self.file_raw(&context.to_string())
    }
    pub fn run(&self, command: &str) -> Run {
        let script = self.temp.path().join("command.sh");
        fs::write(&script, command).unwrap();
        let out = Command::new("bash")
            .arg(&script)
            .env_clear()
            .envs(self.env.iter().cloned())
            .env("CODEX_HOME", self.temp.path().join(".codex"))
            .env("CARGO_HOME", self.temp.path().join(".cargo"))
            .env("XDG_CONFIG_HOME", self.temp.path().join(".config"))
            .env("XDG_CACHE_HOME", self.temp.path().join(".cache"))
            .env("XDG_DATA_HOME", self.temp.path().join(".local/share"))
            .env("XDG_STATE_HOME", self.temp.path().join(".local/state"))
            .env("CLAUDE_CONFIG_DIR", self.temp.path().join(".claude"))
            .env(
                "CLAUDE_PLUGIN_DATA",
                self.temp.path().join("claude-plugin-data"),
            )
            .env("COPILOT_HOME", self.temp.path().join(".copilot"))
            .env("COPILOT_CONFIG_DIR", self.temp.path().join(".copilot"))
            .env("HOME", self.temp.path())
            .env("TMPDIR", self.temp.path())
            .current_dir(self.temp.path())
            .output()
            .expect("spawn current consumer");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into(),
            stderr: String::from_utf8_lossy(&out.stderr).into(),
        }
    }
    pub fn helper(&self) -> Run {
        // Invoke by absolute owning path, from an unrelated cwd.
        self.run(&format!(
            "exec bash '{}'",
            repo()
                .join("amplifier-bundle/tools/workflow_implementation_evidence.sh")
                .display()
        ))
    }
    pub fn gate(&self) -> Run {
        self.run(&body("workflow-tdd", "step-08c-enforce-verdict"))
    }
    pub fn doc(&self) -> Run {
        self.run(&body("workflow-design", "step-06b-checkpoint-doc-review"))
    }
    pub fn finalizer(&self, mode: &str) -> Run {
        assert!(["collect", "validate", "complete", "invalid"].contains(&mode));
        self.run(&format!(
            "exec bash '{}' {mode}",
            repo()
                .join("amplifier-bundle/tools/workflow_agentic_finalization.sh")
                .display()
        ))
    }
}
