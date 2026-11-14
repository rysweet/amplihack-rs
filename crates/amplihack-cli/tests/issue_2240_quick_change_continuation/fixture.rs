use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use recipe_runner_rs::context::RecipeContext;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::exec_stub::write_executable;

pub const BRANCH: &str = "feat/review-2240";
pub const SUBJECT: &str = "fix: preserve review changes in README.md";
pub const TASK: &str = "Brief: /briefs/FACTORY-SF168-PR-DA.md\nFix the review findings. Refs #2240";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn recipe(name: &str) -> serde_yaml::Value {
    serde_yaml::from_str(
        &fs::read_to_string(root().join(format!("amplifier-bundle/recipes/{name}.yaml"))).unwrap(),
    )
    .unwrap()
}

pub fn step(name: &str, id: &str) -> serde_yaml::Value {
    recipe(name)["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing {id}"))
        .clone()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}
pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
pub fn success(o: &Output) -> String {
    assert!(
        o.status.success(),
        "exit {}\nstdout: {}\nstderr: {}",
        o.status,
        stdout(o),
        stderr(o)
    );
    stdout(o)
}

pub struct Fixture {
    pub tmp: TempDir,
    pub home: PathBuf,
    pub caller: PathBuf,
    pub origin: PathBuf,
    pub ctx: RecipeContext,
}

impl Fixture {
    pub fn new(branch: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let caller = tmp.path().join("caller");
        let origin = tmp.path().join("origin.git");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&caller).unwrap();
        let defaults: HashMap<String, Value> =
            serde_json::to_value(recipe("quick-change")["context"].clone())
                .unwrap()
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        let mut f = Self {
            tmp,
            home,
            caller,
            origin,
            ctx: RecipeContext::new(defaults),
        };
        f.ctx
            .set("repo_path", json!(f.caller.canonicalize().unwrap()));
        f.ctx.set("task_description", json!(TASK));
        f.ctx.set("issue_number", json!("2240"));
        f.ctx.set("existing_branch", json!(branch));
        // No inherited config or empty core.hooksPath: normal hooks must execute.
        fs::write(f.home.join(".gitconfig"), "[user]\n name = Test Author\n email = tests@example.com\n[commit]\n gpgsign = false\n[init]\n defaultBranch = main\n").unwrap();
        f.git(
            &f.caller,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                f.origin.to_str().unwrap(),
            ],
        );
        f.git(&f.caller, &["init", "-q", "-b", "main"]);
        f.git(&f.caller, &["config", "user.name", "Test Author"]);
        f.git(&f.caller, &["config", "user.email", "tests@example.com"]);
        f.write(&f.caller, "README.md", "base\n");
        f.write(&f.caller, "notes.md", "base notes\n");
        f.git(&f.caller, &["add", "-A"]);
        f.git(
            &f.caller,
            &[
                "commit",
                "-q",
                "-m",
                "historical subject unrelated to this run",
            ],
        );
        f.git(
            &f.caller,
            &["remote", "add", "origin", f.origin.to_str().unwrap()],
        );
        f.git(&f.caller, &["push", "-q", "-u", "origin", "main"]);
        f.git(&f.caller, &["remote", "set-head", "origin", "main"]);
        let bin = f.tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        write_executable(
            &bin.join("gh"),
            r#"#!/bin/bash
printf '%s\n' "$*" >> "$HOME/gh-calls"
[ "$1 $2" = 'pr create' ] || { echo 'unexpected gh operation' >&2; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --title) printf '%s' "$2" > "$HOME/pr-title"; shift ;;
    --body-file) cp "$2" "$HOME/pr-body"; shift ;;
    --base) printf '%s' "$2" > "$HOME/pr-base"; shift ;;
    --head) printf '%s' "$2" > "$HOME/pr-head"; shift ;;
  esac
  shift
done
echo https://github.com/example/demo/pull/1
"#,
            0o755,
        );
        // The actual binary runs hygiene and any other helper operation.
        std::os::unix::fs::symlink(
            crate::support::amplihack_bin::amplihack_bin(),
            bin.join("amplihack"),
        )
        .unwrap();
        for hook in ["pre-commit", "commit-msg", "pre-push"] {
            write_executable(
                &f.caller.join(".git/hooks").join(hook),
                format!("#!/bin/bash\nprintf '%s\\n' '{hook}' >> \"$HOME/hook-calls\"\n"),
                0o755,
            );
        }
        f
    }

    pub fn write(&self, dir: &Path, name: &str, text: &str) {
        let p = dir.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    pub fn branch(&self, remote: bool) {
        self.git(&self.caller, &["checkout", "-q", "-b", BRANCH]);
        self.write(
            &self.caller,
            "old-work.md",
            &"earlier branch work\n".repeat(160),
        );
        self.git(&self.caller, &["add", "-A"]);
        self.git(&self.caller, &["commit", "-q", "-m", "earlier branch work"]);
        if remote {
            self.git(&self.caller, &["push", "-q", "-u", "origin", BRANCH]);
        }
    }

    pub fn selected(&self) -> PathBuf {
        PathBuf::from(
            self.ctx
                .get("worktree_setup.worktree_path")
                .unwrap()
                .as_str()
                .unwrap(),
        )
    }

    pub fn run(&mut self, recipe_name: &str, id: &str) -> Output {
        let s = step(recipe_name, id);
        let script = s["command"].as_str().expect("bash command");
        // A red test prevents exercising the old wall-clock PR wrapper.
        assert!(
            !script.contains("timeout "),
            "{id} still adds a wall-clock window"
        );
        let mut c = self.command("bash");
        c.current_dir(&self.caller)
            .args(["-c", &self.ctx.render_shell(script)])
            .envs(self.ctx.shell_env_vars())
            .env("GIT_TRACE", self.home.join("git-trace"));
        let out = c.output().unwrap();
        if out.status.success()
            && let Some(key) = s["output"].as_str()
        {
            let value = if s["parse_json"].as_bool() == Some(true) {
                serde_json::from_slice(&out.stdout).expect("live setup JSON")
            } else {
                json!(stdout(&out))
            };
            self.ctx.set(key, value);
        }
        out
    }

    pub fn select(&mut self) -> Output {
        // The runner clones parent context, then overlays rendered child context.
        let parent = self.ctx.clone();
        let child = step("quick-change", "quick-change-worktree");
        for (key, val) in child["context"].as_mapping().unwrap() {
            let rendered = parent.render(val.as_str().unwrap());
            self.ctx.set(key.as_str().unwrap(), json!(rendered));
        }
        let out = self.run("workflow-worktree", "step-04-setup-worktree");
        let result = self.ctx.get("worktree_setup").cloned();
        self.ctx = parent;
        if out.status.success() {
            self.ctx.set("worktree_setup", result.unwrap());
        }
        out
    }

    pub fn select_direct(&mut self) -> Output {
        self.ctx.set("worktree_policy", json!("quick-change"));
        self.run("workflow-worktree", "step-04-setup-worktree")
    }

    pub fn qc(&mut self, suffix: &str) -> Output {
        self.run("quick-change", &format!("quick-change-{suffix}"))
    }

    pub fn common(&self, code: &str) -> Output {
        let mut c = self.command("bash");
        c.current_dir(&self.caller)
            .args(["-c", &format!(". \"$1\"\n{code}"), "bash"])
            .arg(root().join("amplifier-bundle/tools/quick_change_common.sh"))
            .envs(self.ctx.shell_env_vars());
        c.output().unwrap()
    }

    pub fn builder(&mut self, subject: &str) {
        self.write(&self.selected(), "README.md", "base\nreview fix\n");
        self.ctx.set(
            "quick_change_implementation",
            json!({"summary": "Preserve review changes in README.md", "commit_subject": subject}),
        );
    }

    pub fn read_home(&self, path: &str) -> String {
        fs::read_to_string(self.home.join(path)).unwrap_or_default()
    }
    pub fn state(&self) -> PathBuf {
        fs::read_dir(self.home.join(".amplihack/state/quick-change"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }
}
