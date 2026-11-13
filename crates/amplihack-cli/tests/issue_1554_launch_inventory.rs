//! Launch acceptance with a repository-owned inventory hook and no global config.
use crate::issue_1554_fixture::{Repo, describe, executable, sandbox, success, write};
use crate::support::quiet_process::child_set_var;
use std::fs;
use std::path::PathBuf;

#[test]
fn launch_without_home_or_gitignore_passes_the_original_inventory_hook() {
    sandbox(
        module_path!(),
        "launch_without_home_or_gitignore_passes_the_original_inventory_hook",
        || {
            let r = Repo::new();
            let manifest = r.work.join("toolchain/source-inventory");
            let marker = r.root.join("inventory-hook-ran");
            assert!(!r.work.join(".gitignore").exists());
            write(&manifest, "");
            executable(
                r.work.join("scripts/check-inventory"),
                r#"#!/bin/sh
set -eu
common=$(git rev-parse --path-format=absolute --git-common-dir)
env -u HOME -u XDG_CONFIG_HOME git ls-files --cached --others --exclude-standard -z > "$common/inventory.actual"
cmp toolchain/source-inventory "$common/inventory.actual"
printf 'inventory checked\n' >> "$ISSUE_1554_HOOK_MARKER"
"#,
            );
            r.git(
                &r.work,
                &[
                    "add",
                    "--",
                    "scripts/check-inventory",
                    "toolchain/source-inventory",
                ],
            );
            let inventory = r.git(
                &r.work,
                &[
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                ],
            );
            write(&manifest, &inventory);
            r.git(&r.work, &["add", "--", "toolchain/source-inventory"]);
            r.git(
                &r.work,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "establish repository inventory check",
                ],
            );
            r.hook("pre-commit", "#!/bin/sh\nexec scripts/check-inventory\n");
            executable(
                r.bin.join("claude"),
                r#"#!/bin/sh
case "$1" in
  --version) printf '2.1.319 (Claude Code)\n' ;;
  *) printf 'inventory fixture agent completed\n' ;;
esac
"#,
            );
            let before = r.inventory(&r.work);
            let out = r
                .command(r.bin.join("amplihack"))
                .env("AMPLIHACK_CLAUDE_BINARY_PATH", r.bin.join("claude"))
                .env_remove("HOME")
                .env_remove("XDG_CONFIG_HOME")
                .env("AMPLIHACK_RUNTIME_DIR", r.root.join("external-runtime"))
                .args([
                    "claude",
                    "--subprocess-safe",
                    "--no-reflection",
                    "--",
                    "-p",
                    "fixture source task",
                ])
                .output()
                .unwrap();
            success(&out);
            assert!(
                String::from_utf8_lossy(&out.stdout).contains("inventory fixture agent completed"),
                "launch did not reach the local agent: {}",
                describe(&out)
            );
            assert_eq!(
                r.inventory(&r.work),
                before,
                "launch changed the untracked inventory"
            );
            // Run the repository's actual normal hook; do not update its manifest.
            let commit = r
                .command("git")
                .env("ISSUE_1554_HOOK_MARKER", &marker)
                .args([
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "verify inventory contract",
                ])
                .output()
                .unwrap();
            success(&commit);
            assert_eq!(fs::read(&manifest).unwrap(), inventory);
            assert_eq!(fs::read_to_string(marker).unwrap(), "inventory checked\n");
            assert!(!r.work.join(".claude/runtime").exists());
        },
    );
}

#[test]
fn clone_shared_power_steering_and_checkout_specific_context_keep_their_contracts() {
    sandbox(
        module_path!(),
        "clone_shared_power_steering_and_checkout_specific_context_keep_their_contracts",
        || {
            let r = Repo::new();
            child_set_var("AMPLIHACK_RUNTIME_DIR", r.root.join("external-runtime"));
            let main_context = amplihack_git::launcher_state::resolve(&r.main).unwrap();
            let task_context = amplihack_git::launcher_state::resolve(&r.work).unwrap();
            assert_ne!(
                main_context.directory, task_context.directory,
                "launcher context must be checkout-specific"
            );
            let main_shared =
                PathBuf::from(amplihack_utils::worktree::get_shared_runtime_dir(&r.main).unwrap());
            let task_shared =
                PathBuf::from(amplihack_utils::worktree::get_shared_runtime_dir(&r.work).unwrap());
            write(
                main_shared.join("power-steering/state.json"),
                "shared steering state\n",
            );
            assert_eq!(
                main_shared, task_shared,
                "external migration broke clone-shared power-steering state"
            );
            assert_eq!(
                fs::read(task_shared.join("power-steering/state.json")).unwrap(),
                b"shared steering state\n"
            );
            for path in [
                &main_context.directory,
                &task_context.directory,
                &main_shared,
            ] {
                assert!(!path.starts_with(&r.main) && !path.starts_with(&r.work));
            }
        },
    );
}

#[test]
fn root_and_nested_launches_keep_cwd_and_share_checkout_runtime_identity() {
    sandbox(
        module_path!(),
        "root_and_nested_launches_keep_cwd_and_share_checkout_runtime_identity",
        || {
            let r = Repo::new();
            let cwd_record = r.root.join("agent-cwd");
            executable(
                r.bin.join("claude"),
                "#!/bin/sh\ncase \"$1\" in --version) echo '2.1.319 (Claude Code)';; *) pwd -P > \"$AGENT_CWD\"; echo 'fake agent reached';; esac\n",
            );
            for cwd in [&r.work, &r.work.join("src")] {
                let out = r
                    .command(r.bin.join("amplihack"))
                    .current_dir(cwd)
                    .env("AMPLIHACK_CLAUDE_BINARY_PATH", r.bin.join("claude"))
                    .env("AGENT_CWD", &cwd_record)
                    .args(["claude", "--subprocess-safe", "--", "-p", "fixture task"])
                    .output()
                    .unwrap();
                success(&out);
                assert!(String::from_utf8_lossy(&out.stdout).contains("fake agent reached"));
                assert_eq!(
                    fs::read_to_string(&cwd_record).unwrap().trim(),
                    cwd.to_str().unwrap()
                );
            }
            child_set_var("AMPLIHACK_RUNTIME_DIR", r.root.join("external-runtime"));
            let state = amplihack_git::launcher_state::resolve(&r.work).unwrap();
            let log = fs::read_to_string(state.directory.join("sessions.jsonl")).unwrap();
            assert_eq!(
                log.lines()
                    .filter(|line| line.contains("\"status\":\"active\""))
                    .count(),
                2
            );
            assert!(amplihack_utils::launcher_context::read_launcher_context(&r.work).is_some());
            let nested =
                amplihack_git::workspace_validation::checkout_root(&r.work.join("src")).unwrap();
            assert_eq!(
                amplihack_git::launcher_state::resolve(&nested)
                    .unwrap()
                    .directory,
                state.directory
            );
        },
    );
}
