//! Capability admission must fail before work, including on a fresh launch.
//! Only these boundary tests use a capability-report stub, never replay tests.
#![cfg(unix)]

use crate::issue_1741_fixture::{Fixture, describe};
use crate::support::amplihack_bin::amplihack_bin;

#[test]
fn delivery_validation_rejects_a_runner_without_durable_recovery_for_every_provider() {
    let f = Fixture::new();
    let runner = f.root.join("incomplete-runner");
    crate::support::exec_stub::write_executable(
        &runner,
        r#"#!/bin/sh
printf '%s\n' '{"schema_version":1,"version":"test","capabilities":["codex_exec"]}'
"#,
        0o700,
    );
    for provider in ["claude", "copilot", "codex"] {
        let output = crate::isolated_amplihack::isolated(amplihack_bin())
            .args([
                "internal",
                "validate-recipe-runner",
                "--provider",
                provider,
                "--binary",
            ])
            .arg(&runner)
            .env("HOME", &f.home)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "runner lacks durable_recovery_v1: {}",
            describe(&output)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("durable_recovery_v1"),
            "{}",
            describe(&output)
        );
    }
}

#[test]
fn missing_recovery_capability_prevents_execution_even_when_fresh_is_requested() {
    let f = Fixture::new();
    let runner = f.root.join("incomplete-runner");
    crate::support::exec_stub::write_executable(
        &runner,
        r#"#!/bin/sh
case "$1" in
  --version) printf '%s\n' 'recipe-runner 0.5.0'; exit 0 ;;
  --capabilities) printf '%s\n' '{"schema_version":1,"version":"test","capabilities":["codex_exec"]}'; exit 0 ;;
esac
printf '%s\n' '{"recipe_name":"smart-orchestrator","success":true,"step_results":[],"context":{}}'
"#,
        0o700,
    );
    for extra in [&[][..], &["--fresh"][..]] {
        let output = f
            .command(&f.recipe, &f.repo, "1741", extra)
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", describe(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("durable_recovery_v1"),
            "{}",
            describe(&output)
        );
        assert!(f.events().is_empty());
    }
}

#[test]
fn supported_runner_library_and_executable_share_the_same_immutable_revision() {
    let cli = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = cli.parent().unwrap().parent().unwrap();
    let pin = std::fs::read_to_string(root.join("claude-plugin/recipe-runner.rev")).unwrap();
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(cli.join("Cargo.toml")).unwrap()).unwrap();
    let runner = &manifest["dependencies"]["recipe-runner-rs"];
    assert_eq!(
        runner["git"].as_str(),
        Some("https://github.com/rysweet/amplihack-recipe-runner")
    );
    assert_eq!(runner["rev"].as_str(), Some(pin.trim()));
    assert_eq!(pin.trim().len(), 40);
}
