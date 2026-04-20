# Gherkin spec for orch_helper Rust port (Issue #283)
# Replaces inline python3 calls in smart-orchestrator.yaml

Feature: Orchestration helper functions
  As the smart-orchestrator recipe
  I want native Rust helpers for JSON extraction and type normalization
  So that python3 is no longer required at runtime

  Scenario: Extract JSON from tagged code block
    Given stdin contains "Here is output:\n```json\n{\"key\": \"value\"}\n```"
    When I run "amplihack orch-helper extract-json"
    Then stdout is valid JSON with key="value"

  Scenario: Extract JSON prefers tagged over untagged blocks
    Given stdin contains both tagged and untagged code blocks
    When I run "amplihack orch-helper extract-json"
    Then the tagged block's JSON is returned

  Scenario: Extract JSON falls back to raw JSON in prose
    Given stdin contains 'The result is {"key": "value"} end'
    When I run "amplihack orch-helper extract-json"
    Then stdout is valid JSON with key="value"

  Scenario: Extract JSON returns empty object for no JSON
    Given stdin contains "no json here"
    When I run "amplihack orch-helper extract-json"
    Then stdout is "{}"

  Scenario: Normalise type to Q&A
    Given stdin contains "question"
    When I run "amplihack orch-helper normalise-type"
    Then stdout is "Q&A"

  Scenario: Normalise type to Operations
    Given stdin contains "ops"
    When I run "amplihack orch-helper normalise-type"
    Then stdout is "Operations"

  Scenario: Normalise type to Investigation
    Given stdin contains "research"
    When I run "amplihack orch-helper normalise-type"
    Then stdout is "Investigation"

  Scenario: Normalise type defaults to Development
    Given stdin contains "build feature"
    When I run "amplihack orch-helper normalise-type"
    Then stdout is "Development"

  Scenario: Generate workstream config from decomposition
    Given stdin contains decomposition JSON with workstreams array
    When I run "amplihack orch-helper generate-workstream-config"
    Then stdout is the extracted JSON
