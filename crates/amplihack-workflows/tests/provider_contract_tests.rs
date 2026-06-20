//! TDD contract tests for provider-neutral workflow domain models.
//!
//! These tests intentionally describe the target `amplihack-workflows` API
//! before implementation exists.

use amplihack_workflows::workflow_contract::{
    ChangeRequest, ChangeRequestKind, ChangeRequestStatus, HelperEnvelope, HelperOperation,
    ManualAction, ProviderCapabilities, ProviderCapabilityState, ProviderContext,
    ProviderOperationStatus, RepositoryIdentity, RepositoryProvider, TerminalState,
    provider_capabilities, provider_default_next_action, provider_from_remote_url,
    redact_remote_url, repository_identity_from_remote_url, validate_terminal_transition,
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
fn manual_provider_change_request_publication_is_manual_not_fake_success() {
    let manual = ManualAction {
        action: "CreateProviderChangeRequest".into(),
        instructions: "Create a provider change request from feat/auth-timeout to main.".into(),
        required_inputs: vec!["source_branch".into(), "base_branch".into()],
    };

    let envelope = HelperEnvelope::manual_required(
        RepositoryProvider::Manual,
        HelperOperation::PublishChangeRequest,
        "Run provider-specific change request steps manually.",
        json!({
            "change_request": null,
            "manual_action": manual
        }),
    );

    let value = serde_json::to_value(envelope).unwrap();
    assert_eq!(value["provider"], "Manual");
    assert_eq!(value["status"], "ManualRequired");
    assert_eq!(value["data"]["change_request"], Value::Null);
    assert_eq!(
        value["data"]["manual_action"]["action"],
        "CreateProviderChangeRequest"
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
            change_requests: ProviderCapabilityState::Automated,
            stale_cleanup: ProviderCapabilityState::ManualRequired,
        },
        status: ProviderOperationStatus::Succeeded,
        next_action: "Use Azure Boards and Azure Repos automation where configured.".into(),
    };

    let value = serde_json::to_value(context).unwrap();
    assert_eq!(value["capabilities"]["tracking_items"], "Automated");
    assert_eq!(value["capabilities"]["change_requests"], "Automated");
    assert_eq!(value["status"], "Succeeded");
}

#[test]
fn provider_capability_defaults_are_provider_neutral_and_explicit() {
    let github = provider_capabilities(RepositoryProvider::GitHub);
    assert_eq!(github.change_requests, ProviderCapabilityState::Automated);
    assert_eq!(github.stale_cleanup, ProviderCapabilityState::Automated);

    let azdo = provider_capabilities(RepositoryProvider::AzureDevOps);
    assert_eq!(azdo.tracking_items, ProviderCapabilityState::Automated);
    assert_eq!(azdo.change_requests, ProviderCapabilityState::Automated);
    assert!(
        provider_default_next_action(RepositoryProvider::AzureDevOps).contains("Azure Repos"),
        "Azure DevOps change-request automation must remain an explicit provider capability"
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
        provider_from_remote_url(Some("git@github.com:acme/service.git")),
        RepositoryProvider::GitHub
    );
    assert_eq!(
        provider_from_remote_url(Some("https://dev.azure.com/acme/project/_git/service")),
        RepositoryProvider::AzureDevOps
    );
    assert_eq!(
        provider_from_remote_url(Some("ssh://git@ssh.dev.azure.com:v3/acme/project/service")),
        RepositoryProvider::AzureDevOps
    );
    assert_eq!(
        provider_from_remote_url(Some("ssh://git@SSH.DEV.AZURE.COM:v3/acme/project/service")),
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
fn provider_detection_rejects_provider_domain_substring_spoofing() {
    assert_eq!(
        provider_from_remote_url(Some("https://evil.example/github.com/acme/service.git")),
        RepositoryProvider::Manual,
        "provider detection must match the remote host, not path substrings"
    );
    assert_eq!(
        provider_from_remote_url(Some("https://github.com.evil.example/acme/service.git")),
        RepositoryProvider::Manual,
        "provider detection must reject lookalike host suffixes"
    );
    assert_eq!(
        provider_from_remote_url(Some(
            "https://dev.azure.com.evil.example/acme/project/_git/service"
        )),
        RepositoryProvider::Manual,
        "Azure DevOps detection must reject lookalike host suffixes"
    );
}

#[test]
fn remote_repository_identity_redacts_credentials_and_extracts_github_owner_name() {
    let (provider, repository) = repository_identity_from_remote_url(
        Some("https://user:ghp_secret_token@github.com/acme/service.git"),
        "fallback",
    );

    assert_eq!(provider, RepositoryProvider::GitHub);
    assert_eq!(
        repository.remote_url.as_deref(),
        Some("https://[redacted]@github.com/acme/service.git")
    );
    assert_eq!(repository.owner, "acme");
    assert_eq!(repository.name, "service");
}

#[test]
fn remote_repository_identity_extracts_azure_owner_name_from_https_and_ssh() {
    let (https_provider, https_repository) = repository_identity_from_remote_url(
        Some("https://dev.azure.com/acme/project/_git/service"),
        "fallback",
    );
    let (ssh_provider, ssh_repository) = repository_identity_from_remote_url(
        Some("ssh://git@ssh.dev.azure.com:v3/acme/project/service"),
        "fallback",
    );

    assert_eq!(https_provider, RepositoryProvider::AzureDevOps);
    assert_eq!(https_repository.owner, "acme/project");
    assert_eq!(https_repository.name, "service");
    assert_eq!(ssh_provider, RepositoryProvider::AzureDevOps);
    assert_eq!(ssh_repository.owner, "acme/project");
    assert_eq!(ssh_repository.name, "service");
}

#[test]
fn remote_repository_identity_does_not_extract_owner_from_spoofed_provider_path() {
    let (provider, repository) = repository_identity_from_remote_url(
        Some("https://evil.example/github.com/acme/service.git"),
        "fallback",
    );

    assert_eq!(provider, RepositoryProvider::Manual);
    assert_eq!(repository.owner, "unknown");
    assert_eq!(repository.name, "fallback");
}

#[test]
fn redact_remote_url_preserves_non_credential_remote_forms() {
    assert_eq!(
        redact_remote_url("git@github.com:acme/service.git"),
        "git@github.com:acme/service.git"
    );
    assert_eq!(
        redact_remote_url("https://github.com/acme/service.git"),
        "https://github.com/acme/service.git"
    );
}

#[test]
fn terminal_transition_fails_closed_when_manual_provider_path_claims_success() {
    let result = validate_terminal_transition(json!({
        "provider": "Manual",
        "terminal_state": "MANUAL_REQUIRED",
        "terminal_success": true,
        "required_next_action": "Create a provider change request manually.",
        "evidence_used": [
            "provider=Manual",
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
fn terminal_transition_rejects_manual_provider_followup_success_claims() {
    let result = validate_terminal_transition(json!({
        "provider": "Manual",
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "required_next_action": "Create a provider change request manually.",
        "evidence_used": ["provider=Manual"]
    }));

    assert_eq!(result.terminal_state, TerminalState::FailedInvalidEvidence);
    assert!(!result.terminal_success);
    assert!(
        result
            .terminal_reason
            .contains("Manual provider cannot be terminal_success=true")
    );
}

#[test]
fn terminal_transition_rejects_success_without_evidence() {
    let result = validate_terminal_transition(json!({
        "provider": "GitHub",
        "capabilities": {
            "change_requests": "Automated"
        },
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "required_next_action": "Monitor PR validation.",
        "evidence_used": []
    }));

    assert_eq!(result.terminal_state, TerminalState::FailedInvalidEvidence);
    assert!(!result.terminal_success);
    assert!(
        result
            .terminal_reason
            .contains("requires non-empty evidence_used")
    );
}

#[test]
fn terminal_transition_rejects_success_when_change_requests_are_manual() {
    let result = validate_terminal_transition(json!({
        "provider": "AzureDevOps",
        "capabilities": {
            "change_requests": "ManualRequired"
        },
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "required_next_action": "Create an Azure Repos PR manually.",
        "evidence_used": ["provider=AzureDevOps", "change_requests=ManualRequired"]
    }));

    assert_eq!(result.terminal_state, TerminalState::FailedInvalidEvidence);
    assert!(!result.terminal_success);
    assert!(
        result
            .terminal_reason
            .contains("change_requests=ManualRequired cannot be terminal_success=true")
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
