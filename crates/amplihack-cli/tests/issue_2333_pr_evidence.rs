//! TDD contracts for issue #2333's five planned Rust file commands.
//!
//! These tests call the production parser/CLI, not a copy of the schema.
//! Collection support, fact extraction and accepted bundles need a trusted
//! launch/registry fixture once that implementation exists; rejection here
//! is not a claim of coverage for those components.

use crate::support::auto_drive_light::{private_file, show};
use crate::support::pr_evidence::{CommandFixture, collect_request};
use amplihack_cli::Cli;
use serde_json::json;

macro_rules! command_contract {
    ($name:ident, $group:literal, $operation:literal) => {
        #[test]
        fn $name() {
            let argv = [
                "amplihack",
                "pr",
                $group,
                $operation,
                "--request",
                "/private/request with spaces.json",
                "--result",
                "/private/result with spaces.json",
            ];
            assert!(
                Cli::try_parse_from(argv).is_ok(),
                "the planned file command must parse: {argv:?}"
            );
            for missing in ["--request", "--result"] {
                let mut incomplete = argv.to_vec();
                let index = incomplete.iter().position(|arg| *arg == missing).unwrap();
                incomplete.drain(index..index + 2);
                assert!(
                    Cli::try_parse_from(incomplete).is_err(),
                    "{missing} is mandatory"
                );
            }
        }
    };
}

command_contract!(collect_has_a_file_interface, "evidence", "collect");
command_contract!(validate_has_a_file_interface, "evidence", "validate");
command_contract!(wait_has_a_file_interface, "evidence", "wait");
command_contract!(execute_has_a_file_interface, "merge", "execute");
command_contract!(inspect_has_a_file_interface, "merge", "inspect");

#[test]
fn unsupported_schema_is_an_input_error_without_github_access() {
    let fixture = CommandFixture::new();
    let mut input = collect_request();
    input["schema_version"] = json!(2);
    fixture.write(&input);
    let out = fixture.run("evidence", "collect");
    assert!(!out.status.success(), "{}", show(&out));
    let diagnostic = fixture.diagnostic(&out);
    assert_eq!(diagnostic["schema_version"], 1);
    assert_eq!(diagnostic["error"]["code"], "UNSUPPORTED_SCHEMA");
    fixture.assert_no_github();
}

#[test]
fn duplicate_nested_keys_are_rejected_before_serde_can_discard_one() {
    let fixture = CommandFixture::new();
    let raw = serde_json::to_string(&collect_request())
        .unwrap()
        .replace("\"api\":\"REST\"", "\"api\":\"GRAPHQL\",\"api\":\"REST\"");
    assert!(raw.contains("\"api\":\"GRAPHQL\",\"api\":\"REST\""));
    private_file(&fixture.input, &raw);
    let out = fixture.run("evidence", "collect");
    assert!(!out.status.success(), "{}", show(&out));
    assert_eq!(
        fixture.diagnostic(&out)["error"]["code"],
        "DUPLICATE_JSON_KEY"
    );
    fixture.assert_no_github();
}

#[test]
fn malformed_and_unknown_request_shapes_fail_with_stable_errors() {
    let mut unknown = collect_request();
    unknown["payload"]["operations"][0]["executable"] = json!("/tmp/agent-gh");
    let mut wrong_operation = collect_request();
    wrong_operation["operation"] = json!("execute");
    let mut empty = collect_request();
    empty["payload"]["operations"] = json!([]);
    let mut injected = collect_request();
    injected["payload"]["operations"][0]["resource"] = json!("https://attacker.invalid/collect");
    for input in [unknown, wrong_operation, empty, injected] {
        let fixture = CommandFixture::new();
        fixture.write(&input);
        let out = fixture.run("evidence", "collect");
        assert!(!out.status.success(), "{}", show(&out));
        assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
        fixture.assert_no_github();
    }
    let fixture = CommandFixture::new();
    private_file(&fixture.input, "{\"schema_version\":1,");
    let out = fixture.run("evidence", "collect");
    assert!(!out.status.success());
    assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
    fixture.assert_no_github();
}

#[test]
fn endpoint_and_query_text_cannot_be_smuggled_through_parameters() {
    for parameters in [
        json!({"url": "https://attacker.invalid/?token=fixture"}),
        json!({"sha": "--hostname=attacker.invalid"}),
        json!({"query": "mutation { mergePullRequest(input:{pullRequestId:\"7\"}) { clientMutationId } }"}),
    ] {
        let fixture = CommandFixture::new();
        let mut input = collect_request();
        input["payload"]["operations"][0]["parameters"] = parameters;
        fixture.write(&input);
        let out = fixture.run("evidence", "collect");
        assert!(!out.status.success(), "{}", show(&out));
        assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
        fixture.assert_no_github();
    }
}

#[test]
fn legacy_watch_and_merge_keeps_its_separate_cli_contract() {
    assert!(
        Cli::try_parse_from([
            "amplihack",
            "pr",
            "watch-and-merge",
            "7",
            "--strategy",
            "squash",
            "--interval",
            "5"
        ])
        .is_ok()
    );
}
