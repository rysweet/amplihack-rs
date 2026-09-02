use std::{fs, path::PathBuf, process::Command};

const CI_MARKERS: &[&str] = &[
    "CI",
    "GITHUB_ACTIONS",
    "TF_BUILD",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "JENKINS_URL",
];

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_amplihack"));
    for marker in CI_MARKERS {
        command.env_remove(marker);
    }
    command
}

#[test]
fn live_verification_refuses_ci_before_argument_or_environment_access() {
    let output = command()
        .env("CI", "true")
        .env("AMPLIHACK_LITELLM_API_KEY", "must-not-be-rendered")
        .args(["litellm", "verify-live"])
        .output()
        .expect("run amplihack");

    assert_eq!(output.status.code(), Some(78));
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostic");
    assert!(stderr.contains("AH-LIVE-CI-001 stage=host-eligibility"));
    assert!(stderr.contains("No credentials"));
    assert!(!stderr.contains("must-not-be-rendered"));
    assert!(output.stdout.is_empty());
}

#[test]
fn litellm_help_remains_available_in_ci() {
    let output = command()
        .env("GITHUB_ACTIONS", "true")
        .args(["litellm", "verify-live", "--help"])
        .output()
        .expect("run amplihack");

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .expect("UTF-8 help")
            .contains("Verify all real clients")
    );
}

#[test]
fn unknown_litellm_commands_use_clap_in_ci_instead_of_live_refusal() {
    let output = command()
        .env("CI", "true")
        .args(["litellm", "verify-live-extra"])
        .output()
        .expect("run amplihack");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        !String::from_utf8(output.stderr)
            .expect("UTF-8 diagnostic")
            .contains("AH-LIVE-CI-001")
    );
}

#[test]
fn ci_has_no_workflow_reachable_real_client_or_live_gateway_path() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .to_path_buf();
    let workflow =
        fs::read_to_string(root.join(".github/workflows/ci.yml")).expect("read CI workflow");
    for forbidden in [
        "@anthropic-ai/claude-code",
        "@github/copilot",
        "issue_1414_claude_subprocess_scrub.sh",
        "issue_1416_copilot_subprocess_scrub.sh",
        "external_litellm_user_journey.sh",
        "litellm verify-live",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "CI must not reach real-client/live verification path {forbidden}"
        );
    }

    let fixture = fs::read_to_string(root.join("tests/issue_1413_external_litellm_gateway.sh"))
        .expect("read deployment fixture");
    let refusal = fixture
        .find(r#"if [[ -n "${CI:-}" ]]; then"#)
        .expect("CI refusal in deployment fixture");
    let docker = fixture
        .find("docker info")
        .expect("host-only Docker fixture remains available");
    assert!(
        refusal < docker,
        "the CI refusal must precede every LiteLLM Docker operation"
    );
}
