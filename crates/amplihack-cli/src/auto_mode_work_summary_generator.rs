//! Auto-mode work summary generation from shared state and git/GitHub probes.

use crate::auto_mode_state::AutoModeState;
use crate::auto_mode_work_summary::{GitHubState, GitState, TodoState, WorkSummary};
use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const GH_TIMEOUT: Duration = Duration::from_secs(10);

pub trait CommandRunner {
    fn run_output_with_timeout(&self, cmd: Command, timeout: Duration) -> Result<Output>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run_output_with_timeout(&self, cmd: Command, timeout: Duration) -> Result<Output> {
        crate::util::run_output_with_timeout(cmd, timeout)
    }
}

#[derive(Clone, Debug)]
pub struct WorkSummaryGenerator<R = SystemCommandRunner> {
    working_dir: PathBuf,
    runner: R,
}

impl WorkSummaryGenerator<SystemCommandRunner> {
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            runner: SystemCommandRunner,
        }
    }
}

impl<R: CommandRunner> WorkSummaryGenerator<R> {
    pub fn with_runner(working_dir: impl Into<PathBuf>, runner: R) -> Self {
        Self {
            working_dir: working_dir.into(),
            runner,
        }
    }

    pub fn generate(&self, state: &AutoModeState) -> WorkSummary {
        let todo_state = self.extract_todo_state(state);
        let git_state = self.extract_git_state();
        let github_state = git_state
            .current_branch
            .as_deref()
            .map(|branch| self.extract_github_state(branch))
            .unwrap_or_default();

        WorkSummary {
            todo_state,
            git_state,
            github_state,
        }
    }

    fn extract_todo_state(&self, state: &AutoModeState) -> TodoState {
        let todos = state.todos();
        let mut completed = 0;
        let mut in_progress = 0;
        let mut pending = 0;

        for todo in todos {
            match todo.get("status").map(String::as_str) {
                Some("completed") => completed += 1,
                Some("in_progress") => in_progress += 1,
                Some("pending") => pending += 1,
                _ => {}
            }
        }

        TodoState {
            total: completed + in_progress + pending,
            completed,
            in_progress,
            pending,
        }
    }

    fn extract_git_state(&self) -> GitState {
        let Some(current_branch) = self
            .run_text_command(
                git_command(&self.working_dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
                GIT_TIMEOUT,
            )
            .filter(|output| !output.is_empty())
        else {
            return GitState::default();
        };

        let has_uncommitted_changes = self
            .run_text_command(
                git_command(&self.working_dir, &["status", "--porcelain"]),
                GIT_TIMEOUT,
            )
            .map(|output| !output.trim().is_empty())
            .unwrap_or(false);

        let commits_ahead = self
            .run_text_command(
                git_command(&self.working_dir, &["rev-list", "--count", "@{u}..HEAD"]),
                GIT_TIMEOUT,
            )
            .and_then(|output| output.parse::<usize>().ok());

        GitState {
            current_branch: Some(current_branch),
            has_uncommitted_changes,
            commits_ahead,
        }
    }

    fn extract_github_state(&self, branch: &str) -> GitHubState {
        let mut command = Command::new("gh");
        command.current_dir(&self.working_dir).args([
            "pr",
            "list",
            "--head",
            branch,
            "--json",
            "number,state,statusCheckRollup,mergeable",
        ]);

        let Some(stdout) = self.run_text_command(command, GH_TIMEOUT) else {
            return GitHubState::default();
        };
        let Ok(prs) = serde_json::from_str::<Vec<PullRequestSummary>>(&stdout) else {
            return GitHubState::default();
        };
        let Some(pr) = prs.into_iter().next() else {
            return GitHubState::default();
        };

        let ci_status = pr.status_check_rollup.and_then(|checks| {
            checks
                .into_iter()
                .find_map(|check| match check.status.as_deref() {
                    Some("IN_PROGRESS") => Some("PENDING".to_string()),
                    Some("COMPLETED") => check.conclusion,
                    _ => None,
                })
        });
        let pr_mergeable = match pr.mergeable.as_deref() {
            Some("MERGEABLE") => Some(true),
            Some("CONFLICTING") => Some(false),
            _ => None,
        };

        GitHubState {
            pr_number: pr.number,
            pr_state: pr.state,
            ci_status,
            pr_mergeable,
        }
    }

    fn run_text_command(&self, cmd: Command, timeout: Duration) -> Option<String> {
        let output = self.runner.run_output_with_timeout(cmd, timeout).ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn git_command(working_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.current_dir(working_dir).args(args);
    command
}

#[derive(Debug, Deserialize)]
struct PullRequestSummary {
    number: Option<u64>,
    state: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Vec<StatusCheck>>,
    mergeable: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusCheck {
    status: Option<String>,
    conclusion: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_mode_state::{AutoModeState, AutoModeTodo};
    use std::collections::{BTreeMap, VecDeque};
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct FakeRunner {
        outputs: Mutex<VecDeque<(String, Output)>>,
    }

    impl FakeRunner {
        fn new(outputs: Vec<(String, Output)>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run_output_with_timeout(&self, cmd: Command, _timeout: Duration) -> Result<Output> {
            let mut outputs = self.outputs.lock().unwrap();
            let expected = outputs
                .pop_front()
                .expect("fake runner invoked more times than expected");
            let actual = format_command(&cmd);
            assert_eq!(actual, expected.0);
            Ok(expected.1)
        }
    }

    fn format_command(cmd: &Command) -> String {
        let program = cmd.get_program().to_string_lossy().into_owned();
        let args = cmd
            .get_args()
            .map(OsStr::to_string_lossy)
            .map(|value| value.into_owned())
            .collect::<Vec<_>>();
        std::iter::once(program)
            .chain(args)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn successful_output(stdout: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn todo(status: &str, title: &str) -> AutoModeTodo {
        BTreeMap::from([
            ("status".to_string(), status.to_string()),
            ("title".to_string(), title.to_string()),
        ])
    }

    #[test]
    fn generate_summarizes_state_and_git_github_outputs() {
        let state = AutoModeState::new("session-1", 10, "Ship parity");
        state.update_todos(vec![
            todo("completed", "Audit"),
            todo("in_progress", "Port auto mode"),
            todo("pending", "Validate"),
        ]);
        let runner = FakeRunner::new(vec![
            (
                "git rev-parse --abbrev-ref HEAD".to_string(),
                successful_output("feature/parity\n"),
            ),
            (
                "git status --porcelain".to_string(),
                successful_output(" M src/lib.rs\n"),
            ),
            (
                "git rev-list --count @{u}..HEAD".to_string(),
                successful_output("3\n"),
            ),
            (
                "gh pr list --head feature/parity --json number,state,statusCheckRollup,mergeable"
                    .to_string(),
                successful_output(
                    r#"[{"number":77,"state":"OPEN","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],"mergeable":"MERGEABLE"}]"#,
                ),
            ),
        ]);
        let generator = WorkSummaryGenerator::with_runner("/tmp", runner);

        let summary = generator.generate(&state);

        assert_eq!(summary.todo_state.completed, 1);
        assert_eq!(summary.todo_state.in_progress, 1);
        assert_eq!(summary.todo_state.pending, 1);
        assert_eq!(
            summary.git_state.current_branch.as_deref(),
            Some("feature/parity")
        );
        assert!(summary.git_state.has_uncommitted_changes);
        assert_eq!(summary.git_state.commits_ahead, Some(3));
        assert_eq!(summary.github_state.pr_number, Some(77));
        assert_eq!(summary.github_state.ci_status.as_deref(), Some("SUCCESS"));
        assert_eq!(summary.github_state.pr_mergeable, Some(true));
    }

    #[test]
    fn generate_gracefully_degrades_when_git_branch_lookup_fails() {
        let state = AutoModeState::new("session-1", 10, "Ship parity");
        let runner = FakeRunner::new(vec![(
            "git rev-parse --abbrev-ref HEAD".to_string(),
            Output {
                status: std::process::ExitStatus::from_raw(1),
                stdout: Vec::new(),
                stderr: b"fatal".to_vec(),
            },
        )]);
        let generator = WorkSummaryGenerator::with_runner("/tmp", runner);

        let summary = generator.generate(&state);

        assert_eq!(summary.git_state, GitState::default());
        assert_eq!(summary.github_state, GitHubState::default());
    }
}
