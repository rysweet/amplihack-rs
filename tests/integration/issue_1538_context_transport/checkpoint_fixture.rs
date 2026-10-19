//! Real Git targets and unmodified canonical commands through the native runner.
use super::*;
use std::os::unix::fs::PermissionsExt;

pub struct CheckpointRun {
    pub code: i32,
    pub child_code: i32,
    pub output: Value,
    pub diagnostics: String,
    pub before: String,
    pub after: String,
    pub staged: String,
    pub hook: Option<String>,
    pub subject: String,
}

fn git(target: &Path, args: &[&str]) -> String {
    let output = amplihack_git::command_in(target)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {:?}", output);
    String::from_utf8(output.stdout).unwrap()
}

impl NativeFixture {
    pub fn checkpoint(&self, variant: &str) -> CheckpointRun {
        let root = std::env::var_os("AMPLIHACK_CHECKPOINT_NATIVE_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.temp.path().to_owned());
        let case = root.join(variant);
        fs::create_dir_all(&case).unwrap();
        let target = case.join("target ' \" $(touch TARGET_INJECTION) ; [x]");
        let framework = case.join("framework ' \" $(touch FRAMEWORK_INJECTION) ; [x]");
        let tools = framework.join("amplifier-bundle/tools");
        copy_tools(&repo().join("amplifier-bundle/tools"), &tools);
        if variant == "missing-helper" {
            fs::remove_file(tools.join("workflow_checkpoint_commit.sh")).unwrap();
        }
        // The checkpoint commits only at the top of a linked task worktree
        // (issue #1528), which is what workflow-worktree always produces.
        let main = case.join("main");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "--quiet"]);
        git(&main, &["config", "user.name", "Native Checkpoint Fixture"]);
        git(&main, &["config", "user.email", "checkpoint@example.com"]);
        fs::write(main.join("tracked.txt"), "original\n").unwrap();
        git(&main, &["add", "tracked.txt"]);
        git(&main, &["commit", "--quiet", "-m", "initial fixture"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "task",
                target.to_str().unwrap(),
            ],
        );
        let before = git(&target, &["rev-parse", "HEAD"]);
        fs::write(target.join("tracked.txt"), "genuine staged change\n").unwrap();
        if variant == "hook-config" {
            fs::write(target.join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
        }
        if variant == "hygiene-block" {
            fs::write(target.join("plan.md"), "fixture runtime artifact\n").unwrap();
        }
        git(&target, &["add", "-A"]);
        assert!(!git(&target, &["diff", "--cached", "--name-only"]).is_empty());
        let hook = main.join(".git/hooks/pre-commit");
        let hook_ran = target
            .join(git(&target, &["rev-parse", "--absolute-git-dir"]).trim())
            .join("hook-ran");
        let expected = if variant == "hook-config" {
            "unset"
        } else {
            "1"
        };
        fs::write(&hook, format!(
            "#!/bin/sh\nprintf '%s' \"${{PRE_COMMIT_ALLOW_NO_CONFIG-unset}}\" > \"$(git rev-parse --absolute-git-dir)/hook-ran\"\n[ \"${{PRE_COMMIT_ALLOW_NO_CONFIG-unset}}\" = '{expected}' ] || exit 41\n{}\n",
            if variant == "hook-failure" { "echo NATIVE_CHECKPOINT_HOOK_REJECTED >&2; exit 37" } else { "exit 0" }
        )).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        let selected = step("workflow-tdd", "checkpoint-after-implementation");
        let fixture = json!({"name":"ordinary-target-native-checkpoint", "version":"1.0.0",
            "context":{"repo_path":target,"worktree_setup":{"worktree_path":target},"resume_checkpoint":""},
            "steps":[selected]});
        let recipe_file = case.join("recipe.yaml");
        fs::write(&recipe_file, serde_yaml::to_string(&fixture).unwrap()).unwrap();
        let observer = case.join("observer.sh");
        // Observe only the step shell: a nested bash (e.g. a `git` wrapper script
        // on PATH) would otherwise re-source this and truncate the evidence files.
        fs::write(&observer, "unset BASH_ENV\nprintf '%s\\n' \"$BASH_EXECUTION_STRING\" > \"$SOURCE_RENDERED_BODY\"\ntrap 'code=$?; printf \"%s\\n\" \"$code\" > \"$SOURCE_CHILD_EXIT\"' EXIT\nexec > >(tee \"$SOURCE_CHILD_STDOUT\") 2> >(tee \"$SOURCE_CHILD_STDERR\" >&2)\n").unwrap();
        let mut command = Command::new(&self.runner);
        command
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
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CARGO_HOME", self.home.join(".cargo"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("CLAUDE_PLUGIN_DATA", self.home.join("claude-plugin-data"))
            .env("COPILOT_HOME", self.home.join(".copilot"))
            .env("COPILOT_CONFIG_DIR", self.home.join(".copilot"))
            .env("GH_CONFIG_DIR", self.home.join(".config/gh"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_HOME", &framework)
            .env("TMPDIR", &case)
            .env("BASH_ENV", &observer)
            .env("SOURCE_RENDERED_BODY", case.join("rendered-command.sh"))
            .env("SOURCE_CHILD_STDOUT", case.join("child.stdout"))
            .env("SOURCE_CHILD_EXIT", case.join("child.exit"))
            .env("SOURCE_CHILD_STDERR", case.join("child.stderr"));
        for (key, path) in [
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_CACHE_HOME", ".cache"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_STATE_HOME", ".local/state"),
            ("XDG_RUNTIME_DIR", ".runtime"),
        ] {
            command.env(key, self.home.join(path));
        }
        let output = command.current_dir(&target).output().unwrap();
        fs::write(
            case.join("exit-FIRST.json"),
            json!({"exit":output.status.code(),"runner":self.runner}).to_string(),
        )
        .unwrap();
        fs::write(case.join("stdout.json"), &output.stdout).unwrap();
        fs::write(case.join("stderr.log"), &output.stderr).unwrap();
        assert_eq!(
            fs::read(target.join("tracked.txt")).unwrap(),
            b"genuine staged change\n"
        );
        assert!(!target.join("amplifier-bundle").exists());
        for dir in [&target, &case, &framework] {
            assert!(!dir.join("TARGET_INJECTION").exists());
            assert!(!dir.join("FRAMEWORK_INJECTION").exists());
        }
        let result = CheckpointRun {
            code: output.status.code().unwrap_or(-1),
            child_code: fs::read_to_string(case.join("child.exit"))
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
            output: serde_json::from_slice(&output.stdout).unwrap(),
            diagnostics: format!(
                "{}{}",
                fs::read_to_string(case.join("child.stdout")).unwrap(),
                fs::read_to_string(case.join("child.stderr")).unwrap()
            ),
            before,
            after: git(&target, &["rev-parse", "HEAD"]),
            staged: git(&target, &["diff", "--cached", "--name-only"]),
            hook: fs::read_to_string(&hook_ran).ok(),
            subject: git(&target, &["log", "-1", "--format=%s"]),
        };
        fs::write(
            case.join("git-postchecks.json"),
            json!({"child_exit":result.child_code,"before":result.before,"after":result.after,
            "staged":result.staged,"hook":result.hook,"subject":result.subject})
            .to_string(),
        )
        .unwrap();
        result
    }
}
