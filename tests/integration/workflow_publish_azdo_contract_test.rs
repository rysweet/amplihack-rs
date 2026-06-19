//! Provider-aware Azure DevOps PR publication contracts.

use serde_json::Value as JsonValue;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn helper_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join(name)
}

fn helper_text(name: &str) -> String {
    let path = helper_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn workflow_publish_text() -> String {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-publish.yaml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
    }
}

fn run_cmd(dir: &Path, program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run {program} {args:?} in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "command failed: {program} {args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn create_repo(tmp: &TempDir, with_diff: bool) -> PathBuf {
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    run_cmd(&repo, "git", &["init", "-b", "main"]);
    run_cmd(&repo, "git", &["config", "user.email", "test@example.com"]);
    run_cmd(&repo, "git", &["config", "user.name", "Workflow Test"]);
    write_file(&repo.join("README.md"), "base\n");
    run_cmd(&repo, "git", &["add", "README.md"]);
    run_cmd(&repo, "git", &["commit", "-m", "base"]);
    run_cmd(
        &repo,
        "git",
        &[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/org/project/_git/repo",
        ],
    );
    run_cmd(
        &repo,
        "git",
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    run_cmd(&repo, "git", &["switch", "-c", "feature/issue-765"]);
    if with_diff {
        write_file(&repo.join("feature.txt"), "feature\n");
        run_cmd(&repo, "git", &["add", "feature.txt"]);
        run_cmd(&repo, "git", &["commit", "-m", "feature"]);
    }
    repo
}

fn install_fake_tools(tmp: &TempDir, mode: &str) -> (PathBuf, PathBuf) {
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let az_log = tmp.path().join("az.log");
    let az = bin_dir.join("az");
    write_file(
        &az,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> {az_log:?}
mode={mode:?}
if [ "${{1:-}}" = "repos" ] && [ "${{2:-}}" = "pr" ] && [ "${{3:-}}" = "list" ]; then
  case "$mode" in
    existing)
      cat <<'JSON'
[
  {{"pullRequestId":456,"status":"active","sourceRefName":"refs/heads/feature/issue-765","targetRefName":"refs/heads/main","url":"https://dev.azure.com/org/project/_git/repo/pullrequest/456"}}
]
JSON
      exit 0
      ;;
    create|none)
      printf '[]\n'
      exit 0
      ;;
    authfail)
      echo "fatal: https://secret-token@dev.azure.com/org/project auth failed" >&2
      exit 42
      ;;
  esac
fi
if [ "${{1:-}}" = "repos" ] && [ "${{2:-}}" = "pr" ] && [ "${{3:-}}" = "create" ]; then
  if [ "$mode" = "create" ]; then
    cat <<'JSON'
{{"pullRequestId":789,"status":"active","url":"https://dev.azure.com/org/project/_git/repo/pullrequest/789"}}
JSON
    exit 0
  fi
  echo "create must not be reached for mode=$mode" >&2
  exit 77
fi
echo "unexpected az call: $*" >&2
exit 99
"#,
            az_log = az_log.display().to_string(),
            mode = mode
        ),
    );
    make_executable(&az);

    let amplihack = bin_dir.join("amplihack");
    write_file(
        &amplihack,
        "#!/usr/bin/env bash\nset -euo pipefail\nexit 0\n",
    );
    make_executable(&amplihack);

    (bin_dir, az_log)
}

fn run_publish(repo: &Path, bin_dir: &Path) -> Output {
    let old_path = std::env::var("PATH").unwrap_or_default();
    Command::new("bash")
        .arg(helper_path("workflow_publish_pr.sh"))
        .current_dir(repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("REMOTE_HOST_TYPE", "azdo")
        .env("ISSUE_NUMBER", "765")
        .env("SYSTEM_COLLECTIONURI", "https://dev.azure.com/org/")
        .env("SYSTEM_TEAMPROJECT", "project")
        .output()
        .expect("run workflow_publish_pr.sh")
}

fn parse_stdout_json(output: &Output) -> JsonValue {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("parse stdout JSON: {e}\n{stdout}"))
}

#[test]
fn publish_recipe_and_helper_declare_azdo_terminal_contract() {
    let recipe = workflow_publish_text();
    let helper = helper_text("workflow_publish_pr.sh");

    for required in [
        "REMOTE_HOST_TYPE",
        "azdo",
        "az repos pr list",
        "az repos pr create",
        "EXISTING_OPEN_PR",
        "BLOCKED_PROVIDER",
        "NO_DIFF_SUCCESS",
        "FOLLOWUP_CREATED",
    ] {
        assert!(
            recipe.contains(required) || helper.contains(required),
            "AzDO publish contract missing `{required}`"
        );
    }
    assert!(
        !helper.contains("non-GitHub host does not use gh pr create"),
        "AzDO must not be handled as a generic non-GitHub successful skip"
    );
}

#[test]
fn azdo_existing_active_pr_returns_existing_open_pr_without_create() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = create_repo(&tmp, true);
    let (bin_dir, az_log) = install_fake_tools(&tmp, "existing");

    let output = run_publish(&repo, &bin_dir);
    assert!(
        output.status.success(),
        "existing AzDO PR should be idempotent\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout_json(&output);
    assert_eq!(json["state"], "EXISTING_OPEN_PR");
    assert_eq!(json["terminal_status"], "success");
    assert_eq!(json["pr_number"], "456");

    let log = fs::read_to_string(&az_log).expect("read az log");
    assert!(
        log.contains("repos pr list"),
        "must query active PRs; log:\n{log}"
    );
    assert!(
        !log.contains("repos pr create"),
        "existing AzDO PR must suppress create; log:\n{log}"
    );
}

#[test]
fn azdo_no_diff_returns_no_diff_success_without_create() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = create_repo(&tmp, false);
    let (bin_dir, az_log) = install_fake_tools(&tmp, "none");

    let output = run_publish(&repo, &bin_dir);
    assert!(
        output.status.success(),
        "no-diff AzDO branch should be terminal success\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout_json(&output);
    assert_eq!(json["state"], "NO_DIFF_SUCCESS");
    assert_eq!(json["branch_diff_status"], "no-diff");

    let log = fs::read_to_string(&az_log).unwrap_or_default();
    assert!(
        !log.contains("repos pr create"),
        "no-diff AzDO branch must not create PR; log:\n{log}"
    );
}

#[test]
fn azdo_diff_without_existing_pr_creates_followup_pr_once() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = create_repo(&tmp, true);
    let (bin_dir, az_log) = install_fake_tools(&tmp, "create");

    let output = run_publish(&repo, &bin_dir);
    assert!(
        output.status.success(),
        "AzDO branch with diff should create PR\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout_json(&output);
    assert_eq!(json["state"], "FOLLOWUP_CREATED");
    assert_eq!(json["terminal_status"], "success");
    assert_eq!(json["pr_number"], "789");

    let log = fs::read_to_string(&az_log).expect("read az log");
    assert!(
        log.contains("repos pr list"),
        "must check duplicates first; log:\n{log}"
    );
    assert_eq!(
        log.matches("repos pr create").count(),
        1,
        "must create exactly one AzDO PR; log:\n{log}"
    );
}

#[test]
fn azdo_auth_or_cli_failure_is_blocked_provider_not_successful_skip() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = create_repo(&tmp, true);
    let (bin_dir, _az_log) = install_fake_tools(&tmp, "authfail");

    let output = run_publish(&repo, &bin_dir);
    assert!(
        !output.status.success(),
        "AzDO auth/provider failure must block publication\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout_json(&output);
    assert_eq!(json["state"], "BLOCKED_PROVIDER");
    assert_eq!(json["terminal_status"], "failure");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("secret-token"),
        "provider stderr must redact credential-bearing URLs"
    );
}

#[test]
fn final_status_blocks_azdo_missing_pr_instead_of_manual_success() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = create_repo(&tmp, true);

    let output = Command::new("bash")
        .arg(helper_path("workflow_final_status.sh"))
        .current_dir(&repo)
        .env("REMOTE_HOST_TYPE", "azdo")
        .env("ISSUE_NUMBER", "765")
        .env("TASK_DESCRIPTION", "verify AzDO publish terminal state")
        .env("IMPLEMENTATION_COMPLETED", "true")
        .env("VERIFICATION_COMPLETED", "true")
        .env("PUBLISH_STATE_REACHED", "false")
        .env("PR_PUBLISH_RESULT_STATE", "BLOCKED_PROVIDER")
        .output()
        .expect("run workflow_final_status.sh");

    assert!(
        !output.status.success(),
        "blocked AzDO publication must not complete manually\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("BLOCKED_PROVIDER") || stderr.contains("BLOCKED_PROVIDER"),
        "final status must preserve blocked-provider evidence"
    );
    assert!(
        !stdout.contains("manual creation required")
            && !stderr.contains("manual creation required"),
        "final status must not represent missing AzDO publication as manual success"
    );
}
