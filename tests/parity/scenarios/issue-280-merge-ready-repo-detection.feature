Feature: Merge-ready skill detects repo type for test command (#280)
  The merge-ready skill must detect the project type (Rust, Node, Python)
  and use the appropriate test command instead of hardcoding gadugi-test.

  Scenario: Rust project uses cargo test
    Given a project with Cargo.toml at the root
    When the merge-ready skill determines the test command
    Then the test command includes "cargo test"
    And the test command includes "cargo clippy"

  Scenario: Node project uses npm test
    Given a project with package.json at the root
    When the merge-ready skill determines the test command
    Then the test command includes "npm test" or "npx jest"

  Scenario: Python project uses pytest
    Given a project with pyproject.toml or setup.py at the root
    When the merge-ready skill determines the test command
    Then the test command includes "pytest" or "python -m pytest"

  Scenario: Unknown project type falls back to gadugi-test
    Given a project with no recognized build system files
    When the merge-ready skill determines the test command
    Then the test command includes "gadugi-test"
