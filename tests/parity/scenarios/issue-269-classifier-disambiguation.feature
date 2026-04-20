Feature: Workflow classifier OPS keyword disambiguation (#269)
  OPS keywords must be multi-word phrases so that single generic words
  don't steal development tasks.

  Scenario Outline: Development tasks are not misclassified as Ops
    Given the default workflow classifier
    When the user request is "<request>"
    Then the workflow is "DEFAULT_WORKFLOW"

    Examples:
      | request                                                                   |
      | Add an agentic disk-cleanup loop. Extend src/cmd_cleanup.rs              |
      | Implement a new component to manage user sessions                         |
      | Create a utility to organize test fixtures by category                    |
      | Build a tool that can delete files safely                                 |
      | Refactor the repo management module for better performance                |
      | Fix the cleanup logic in the test harness                                 |
      | Update the manage_connections function to handle timeouts                 |

  Scenario Outline: Legitimate Ops tasks are correctly classified
    Given the default workflow classifier
    When the user request is "<request>"
    Then the workflow is "OPS_WORKFLOW"

    Examples:
      | request                                                |
      | disk cleanup of temporary build artifacts              |
      | manage repos across the organization                   |
      | run git operations to clean up stale branches          |
      | delete files from the shared drive                     |
      | organize files in the project archive                  |
      | clean up temp directories on the build server          |

  Scenario: Mixed Default + Ops keywords resolve to Default (priority)
    Given the default workflow classifier
    When the user request is "delete files and fix the build"
    Then the workflow is "DEFAULT_WORKFLOW"
    And the confidence is at least 0.7

  Scenario: No matching keywords defaults to Default with low confidence
    Given the default workflow classifier
    When the user request is "do something interesting"
    Then the workflow is "DEFAULT_WORKFLOW"
    And the confidence is less than 0.7
