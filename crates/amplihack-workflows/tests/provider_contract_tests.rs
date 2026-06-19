//! TDD contract tests for provider-neutral workflow domain models.
//!
//! These tests intentionally describe the target `amplihack-workflows` API
//! before implementation exists.

use amplihack_workflows::workflow_contract::{
    ChangeRequest, ChangeRequestKind, ChangeRequestStatus, HelperEnvelope, HelperOperation,
    ManualAction, ProviderCapabilities, ProviderCapabilityState, ProviderContext,
    ProviderOperationStatus, RepositoryIdentity, RepositoryProvider, TerminalState,
    provider_capabilities, provider_default_next_action, provider_from_remote_url,
    validate_terminal_transition,
};
use serde_json::{Value, json};

#[test]
fn terminal_states_emit_canonical_screaming_snake_case() {
    let serialized = serde_json::to_string(&TerminalState::FollowupCreated).unwrap();
    assert_eq!(serialized, "\"FOLLOWUP_CREATED\"");

    let manual = serde_json::to_string(&TerminalState::BlockedManualProvider).unwrap();
    assert_eq!(manual, "\"BLOCKED_MANUAL_PROVIDER\"");
}

#[test]
fn terminal_states_accept_legacy_names_but_normalize_on_output() {
    let legacy: TerminalState = serde_json::from_str("\"ManualRequired\"").unwrap();
    assert_eq!(legacy, TerminalState::ManualRequired);
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        "\"MANUAL_REQUIRED\"",
        "legacy Rust-style terminal names may parse, but emitted JSON is canonical"
    );
}

#[test]
fn manual_and_blocked_provider_states_are_not_terminal_success() {
    assert!(!TerminalState::ManualRequired.is_success());
    assert!(!TerminalState::BlockedManualProvider.is_success());
    assert!(!TerminalState::HollowSuccess.is_success());
    assert!(TerminalState::FollowupCreated.is_success());
}

#[test]
fn helper_envelope_keeps_operation_data_nested_under_data() {
    let envelope = HelperEnvelope::succeeded(
        RepositoryProvider::GitHub,
        HelperOperation::DetectProvider,
        "No further provider setup is required.",
        json!({
            "repository": {
                "remote_url": "https://github.com/acme/service.git",
                "owner": "acme",
                "name": "service",
                "default_base": "main"
            },
            "capabilities": {
                "tracking_items": "Automated",
                "change_requests": "Automated",
                "stale_cleanup": "Automated"
            }
        }),
    );

    let value = serde_json::to_value(envelope).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["provider"], "GitHub");
    assert_eq!(value["operation"], "DetectProvider");
    assert_eq!(value["status"], "Succeeded");
    assert!(value["warnings"].as_array().unwrap().is_empty());
    assert_eq!(
        value["data"]["capabilities"]["change_requests"],
        "Automated"
    );
    assert!(
        value.get("tracking_item").is_none()
            && value.get("change_request").is_none()
            && value.get("manual_action").is_none(),
        "operation-specific fields must not appear at helper-envelope top level"
    );
}

#[test]
fn azure_repos_change_request_publication_is_manual_not_fake_success() {
    let manual = ManualAction {
        action: "CreateAzureReposPullRequest".into(),
        instructions: "Create an Azure Repos pull request from feat/auth-timeout to main.".into(),
        required_inputs: vec!["source_branch".into(), "base_branch".into()],
    };

    let envelope = HelperEnvelope::manual_required(
        RepositoryProvider::AzureDevOps,
        HelperOperation::PublishChangeRequest,
        "Create an Azure Repos pull request manually, then rerun status detection.",
        json!({
            "change_request": null,
            "manual_action": manual
        }),
    );

    let value = serde_json::to_value(envelope).unwrap();
    assert_eq!(value["provider"], "AzureDevOps");
    assert_eq!(value["status"], "ManualRequired");
    assert_eq!(value["data"]["change_request"], Value::Null);
    assert_eq!(
        value["data"]["manual_action"]["action"],
        "CreateAzureReposPullRequest"
    );
    assert!(
        value["next_action"].as_str().unwrap().contains("manually"),
        "manual provider states must include an actionable next_action"
    );
}

#[test]
fn provider_context_exposes_explicit_capability_states() {
    let context = ProviderContext {
        schema_version: 1,
        provider: RepositoryProvider::AzureDevOps,
        repository: RepositoryIdentity {
            remote_url: Some("https://dev.azure.com/acme/project/_git/service".into()),
            owner: "acme".into(),
            name: "service".into(),
            default_base: "main".into(),
        },
        capabilities: ProviderCapabilities {
            tracking_items: ProviderCapabilityState::Automated,
            change_requests: ProviderCapabilityState::ManualRequired,
            stale_cleanup: ProviderCapabilityState::ManualRequired,
        },
        status: ProviderOperationStatus::ManualRequired,
        next_action: "Create Azure Repos PRs manually for this provider.".into(),
    };

    let value = serde_json::to_value(context).unwrap();
    assert_eq!(value["capabilities"]["tracking_items"], "Automated");
    assert_eq!(value["capabilities"]["change_requests"], "ManualRequired");
    assert_eq!(value["status"], "ManualRequired");
}

#[test]
fn provider_capability_defaults_are_provider_neutral_and_explicit() {
    let github = provider_capabilities(RepositoryProvider::GitHub);
    assert_eq!(github.change_requests, ProviderCapabilityState::Automated);
    assert_eq!(github.stale_cleanup, ProviderCapabilityState::Automated);

    let azdo = provider_capabilities(RepositoryProvider::AzureDevOps);
    assert_eq!(azdo.tracking_items, ProviderCapabilityState::Automated);
    assert_eq!(
        azdo.change_requests,
        ProviderCapabilityState::ManualRequired
    );
    assert!(
        provider_default_next_action(RepositoryProvider::AzureDevOps).contains("manually"),
        "Azure DevOps change-request automation must not be implied where unavailable"
    );

    let manual = provider_capabilities(RepositoryProvider::Manual);
    assert_eq!(
        manual.tracking_items,
        ProviderCapabilityState::ManualRequired
    );
    assert_eq!(
        manual.change_requests,
        ProviderCapabilityState::ManualRequired
    );
}

#[test]
fn provider_detection_from_remote_urls_falls_back_to_manual_for_unknowns() {
    assert_eq!(
        provider_from_remote_url(Some("https://github.com/acme/service.git")),
        RepositoryProvider::GitHub
    );
    assert_eq!(
        provider_from_remote_url(Some("https://dev.azure.com/acme/project/_git/service")),
        RepositoryProvider::AzureDevOps
    );
    assert_eq!(provider_from_remote_url(None), RepositoryProvider::Manual);
    assert_eq!(
        provider_from_remote_url(Some("ssh://git.example.invalid/acme/service")),
        RepositoryProvider::Manual,
        "unknown remotes must require manual provider handling instead of pretending GitHub automation exists"
    );
}

#[test]
fn terminal_transition_fails_closed_when_manual_provider_path_claims_success() {
    let result = validate_terminal_transition(json!({
        "provider": "AzureDevOps",
        "terminal_state": "MANUAL_REQUIRED",
        "terminal_success": true,
        "required_next_action": "Create an Azure Repos pull request manually.",
        "evidence_used": [
            "provider=AzureDevOps",
            "change_requests=ManualRequired"
        ]
    }));

    assert_eq!(result.terminal_state, TerminalState::FailedInvalidEvidence);
    assert!(!result.terminal_success);
    assert!(
        result
            .terminal_reason
            .contains("MANUAL_REQUIRED cannot be terminal_success=true")
    );
}

#[test]
fn change_request_model_serializes_provider_neutral_pull_request_fields() {
    let change_request = ChangeRequest {
        kind: ChangeRequestKind::PullRequest,
        id: "812".into(),
        url: "https://github.com/acme/service/pull/812".into(),
        state: ChangeRequestStatus::Open,
        source_branch: "feat/provider-contract".into(),
        base_branch: "main".into(),
        head_sha: Some("1d2c3b4a".into()),
    };

    let value = serde_json::to_value(change_request).unwrap();
    assert_eq!(value["kind"], "PullRequest");
    assert_eq!(value["state"], "Open");
    assert_eq!(value["source_branch"], "feat/provider-contract");
    assert_eq!(value["base_branch"], "main");
    assert_eq!(value["head_sha"], "1d2c3b4a");
}
