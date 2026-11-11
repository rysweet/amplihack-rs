//! Repository selection must survive inherited Git configuration injection.
use std::path::Path;
use std::process::Command;

fn succeeds(output: std::process::Output) -> Vec<u8> {
    assert!(output.status.success(), "{output:?}");
    output.stdout
}

fn clean_git(root: &Path, repo: &Path, args: &[&str]) -> Vec<u8> {
    succeeds(
        amplihack_git::command_in(repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap(),
    )
}

#[test]
fn inherited_config_parameters_cannot_redirect_a_repository_bound_command() {
    if let Some(root) = std::env::var_os("ISSUE_2096_POLICY_CHILD") {
        let root = Path::new(&root);
        let raw = Command::new("git")
            .current_dir(root.join("intended"))
            .args(["config", "--get", "core.worktree"])
            .output()
            .unwrap();
        assert!(
            raw.status.success(),
            "control must demonstrate inherited injection: {raw:?}"
        );
        let policy = amplihack_git::command_in(root.join("intended"))
            .args(["config", "--get", "core.worktree"])
            .output()
            .unwrap();
        assert_eq!(
            policy.status.code(),
            Some(1),
            "injected routing configuration survived the policy: {policy:?}"
        );
        let output = succeeds(
            amplihack_git::command_in(root.join("intended"))
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .unwrap(),
        );
        assert_eq!(
            Path::new(std::str::from_utf8(&output).unwrap().trim()),
            root.join("intended")
        );
        let inherited = succeeds(Command::new("git").arg("--exec-path").output().unwrap());
        assert_eq!(
            Path::new(std::str::from_utf8(&inherited).unwrap().trim()),
            root.join("untrusted-extensions")
        );
        let scrubbed = succeeds(
            amplihack_git::command_in(root.join("intended"))
                .arg("--exec-path")
                .output()
                .unwrap(),
        );
        assert_ne!(
            inherited, scrubbed,
            "inherited executable extensions survived"
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for name in ["home", "intended", "unrelated"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    for name in ["intended", "unrelated"] {
        clean_git(&root, &root.join(name), &["init", "-q"]);
    }
    // The child's environment, rather than an explicit override on the
    // builder under test, reproduces inherited configuration faithfully.
    for indexed in [true, false] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", root.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_EXEC_PATH", root.join("untrusted-extensions"))
            .env("GIT_TEMPLATE_DIR", root.join("untrusted-template"))
            .env("ISSUE_2096_POLICY_CHILD", &root)
            .args([
                "--exact",
                "inherited_config_parameters_cannot_redirect_a_repository_bound_command",
                "--test-threads=1",
                "--nocapture",
            ]);
        if indexed {
            child
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "core.worktree")
                .env("GIT_CONFIG_VALUE_0", root.join("unrelated"));
        } else {
            child.env(
                "GIT_CONFIG_PARAMETERS",
                format!("'core.worktree'='{}'", root.join("unrelated").display()),
            );
        }
        let output = succeeds(child.output().unwrap());
        assert!(
            String::from_utf8_lossy(&output).contains("1 passed"),
            "child must execute the contract, not filter it out: {}",
            String::from_utf8_lossy(&output)
        );
    }
}
