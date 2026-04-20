Feature: QA team skill detects repo type for test command selection (#280)
  As a developer using the merge-ready / qa-team skill
  I want the skill to detect my repo type and use the correct test command
  So that Rust repos use `cargo test` instead of `gadugi-test`

  Scenario: Rust repository detected
    Given a repository with a `Cargo.toml` at the root
    When the qa-team skill selects a test command
    Then the test command is `cargo test --workspace`
    And `cargo clippy --workspace -- -D warnings` is also run
    And scenario validation uses `tests/parity/scenarios/*.yaml`

  Scenario: Node.js repository detected
    Given a repository with a `package.json` at the root
    When the qa-team skill selects a test command
    Then the test command is `gadugi-test run tests/agentic/*.yaml`

  Scenario: Python repository detected
    Given a repository with a `pyproject.toml` at the root
    When the qa-team skill selects a test command
    Then the test command is `python -m pytest`

  Scenario: Unknown repository falls back to gadugi-test
    Given a repository with no recognized marker files
    When the qa-team skill selects a test command
    Then the test command is `gadugi-test run tests/agentic/*.yaml`
