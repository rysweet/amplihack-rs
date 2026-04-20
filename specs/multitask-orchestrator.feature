Feature: Multitask orchestrator
  Native Rust port of the Python multitask orchestrator that manages
  parallel workstream execution with subprocess isolation.

  Background:
    Given the amplihack binary is installed
    And a valid multitask config file exists

  Scenario: Run multitask with recipe-runner mode
    Given a config file with 2 workstreams
    When I run "amplihack multitask run config.json"
    Then each workstream runs in an isolated /tmp clone
    And the recipe-runner-rs binary is invoked for each workstream
    And a state.json file tracks progress
    And the exit code is 0 when all workstreams complete

  Scenario: Run multitask with classic mode
    Given a config file with 2 workstreams
    When I run "amplihack multitask run config.json --mode classic"
    Then each workstream runs in a single session
    And the delegate command is used for execution

  Scenario: Dry run shows plan without execution
    Given a config file with 3 workstreams
    When I run "amplihack multitask run config.json --dry-run"
    Then no workstreams are actually started
    And the plan is printed to stdout

  Scenario: Status shows workstream progress
    Given a running multitask session with state.json
    When I run "amplihack multitask status"
    Then it displays each workstream with its status icon
    And shows total/completed/running/failed counts

  Scenario: Cleanup removes worktrees
    Given a completed multitask session
    When I run "amplihack multitask cleanup config.json"
    Then worktree directories are removed
    And git worktree prune is executed

  Scenario: Invalid delegate is rejected
    Given AMPLIHACK_DELEGATE is set to "invalid-tool"
    When I run "amplihack multitask run config.json"
    Then it exits with error mentioning invalid delegate

  Scenario: Timeout policy enforcement
    Given a config with max_runtime of 60 seconds
    When a workstream exceeds the timeout
    Then the workstream is marked as failed_resumable
    And remaining workstreams continue
