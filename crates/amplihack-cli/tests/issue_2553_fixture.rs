//! Isolated boundary fixtures. Classifications are supplied by the test, not a model.
//! The Rust executable replaces external agents and GitHub, never the dispatcher.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

pub const REPOSITORY: &str = "example/operations";
pub const FIRST: &str = "8f9f57b5-9232-49df-b985-c2c92ee997c1";
pub const SECOND: &str = "64a1ff65-5a30-4c03-9c60-6500744fb328";

fn executable() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(path) = std::env::var_os("ISSUE_2553_FIXTURE_BIN") {
            let path = PathBuf::from(path)
                .canonicalize()
                .expect("prebuilt Rust fixture path");
            assert!(path.is_file(), "prebuilt Rust fixture must be a file");
            return path;
        }
        let out = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "build",
                "--locked",
                "--quiet",
                "-p",
                "amplihack-cli",
                "--example",
                "issue_2553_fixture",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("build Rust fixture executable");
        assert!(out.status.success(), "{}", describe(&out));
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/issue_2553_fixture")
    })
}

pub fn describe(out: &Output) -> String {
    format!(
        "exit={}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn leaf(key: &str, classification: &str, task: &str) -> Value {
    let outputs = if classification == "Development" || classification == "Consensus" {
        json!({"delivery": "delivery"})
    } else {
        json!({"answer": "answer"})
    };
    json!({"name": key, "repository": REPOSITORY, "classification": classification,
        "description": task, "required_outputs": outputs})
}

pub fn plan(leaves: Vec<Value>) -> Value {
    json!({"task_type": "Operations", "workstreams": leaves})
}

pub struct Fixture {
    _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub config: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let f = Self {
            repo: root.join("tmp/repo"),
            home: root.join("tmp/home"),
            tmp: root.join("tmp"),
            config: root.join("tmp/plan.json"),
            root,
            _temp: temp,
        };
        for dir in [
            f.repo.clone(),
            f.home.clone(),
            f.tmp.clone(),
            f.root.join("bin"),
            f.tmp.join("profiles"),
            f.tmp.join("runtime"),
            f.tmp.join("proc"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in [
                &f.home,
                &f.tmp,
                &f.tmp.join("proc"),
                &f.tmp.join("profiles"),
                &f.tmp.join("runtime"),
            ] {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
            }
            crate::isolated_amplihack::seed_proc_tree(&f.tmp.join("proc"));
        }
        for name in ["amplihack", "claude", "gh", "git"] {
            #[cfg(unix)]
            std::os::unix::fs::symlink(executable(), f.root.join("bin").join(name)).unwrap();
        }
        f.git(&["init", "-q", "--initial-branch=main"]);
        f.git(&["config", "user.name", "Routing fixture"]);
        f.git(&["config", "user.email", "routing@example.invalid"]);
        fs::write(f.repo.join("README.md"), "# Routing fixture\n").unwrap();
        f.git(&["add", "README.md"]);
        f.git(&["commit", "-qm", "fixture"]);
        f.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/example/operations.git",
        ]);
        f
    }

    pub fn command(&self, mode: &str) -> Command {
        let mut c = crate::isolated_amplihack::isolated(executable());
        c.arg(mode)
            .current_dir(&self.repo)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("XDG_RUNTIME_DIR", self.tmp.join("runtime"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.tmp.join("profiles"))
            .env("AMPLIHACK_TEST_HOME_GUARD", "1")
            .env("AMPLIHACK_TEST_PROC_ROOT", self.tmp.join("proc"))
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_REPO_PATH", &self.repo)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("ISSUE_2553_ROOT", &self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        c
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("/usr/bin/git")
            .args(args)
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", describe(&out));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    pub fn normalise_output(&self, input: &Value, workflow: &str) -> Output {
        let input_file = self.root.join("decomposition.json");
        fs::write(&input_file, serde_json::to_vec(input).unwrap()).unwrap();
        self.command("normalise")
            .arg(workflow)
            .stdin(fs::File::open(input_file).unwrap())
            .output()
            .unwrap()
    }

    pub fn normalise(&self, input: &Value) -> Value {
        let out = self.normalise_output(input, "");
        assert!(out.status.success(), "{}", describe(&out));
        let config: Value = serde_json::from_slice(&out.stdout).expect("normalised JSON array");
        fs::write(&self.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        config
    }

    pub fn execute(&self, mode: &str) -> Output {
        self.command("execute")
            .arg(&self.config)
            .arg(mode)
            .output()
            .unwrap()
    }

    pub fn events(&self) -> Vec<Value> {
        match fs::read_to_string(self.root.join("events.jsonl")) {
            Ok(s) => s
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => panic!("read fixture trace: {e}"),
        }
    }

    pub fn calls(&self, kind: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["kind"] == kind)
            .collect()
    }

    pub fn states(&self) -> Vec<Value> {
        let dir = self.tmp.join("amplihack-workstreams/state");
        if !dir.exists() {
            return vec![];
        }
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && !p.to_string_lossy().contains(".progress.")
            })
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
            .collect()
    }

    pub fn assert_direct(&self, count: usize) {
        let calls = self.calls("execute");
        assert_eq!(
            calls.len(),
            count,
            "one scoped invocation per leaf: {:?}",
            self.events()
        );
        for call in calls {
            assert_eq!(call["recipe"], "operations-direct", "{call:#}");
            assert_eq!(call["cwd"], self.repo.to_str().unwrap(), "{call:#}");
        }
        assert!(self.calls("development").is_empty(), "{:?}", self.events());
        assert!(
            self.calls("forbidden_git").is_empty(),
            "{:?}",
            self.events()
        );
        assert!(self.calls("pr").is_empty(), "{:?}", self.events());
    }
}
