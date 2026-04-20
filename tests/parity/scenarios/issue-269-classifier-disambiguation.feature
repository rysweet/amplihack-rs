Feature: Workflow classifier disambiguates constructive tasks from ops (#269)
  As a developer using the smart-orchestrator
  I want constructive tasks to classify as DEFAULT even when they mention ops-like words
  So that multi-requirement "Add" tasks are not misclassified as Ops

  Scenario Outline: Constructive tasks with ops-like words classify as DEFAULT
    Given the default workflow classifier
    When I classify "<request>"
    Then the workflow type is "DEFAULT_WORKFLOW"
    And the confidence is at least 0.5

    Examples:
      | request                                              |
      | Add a feature to manage users in the admin panel     |
      | Create a management dashboard for monitoring         |
      | Implement a cleanup policy for expired sessions      |
      | Build a cleanup tool for database records            |
      | Design an organize files component for the UI        |

  Scenario Outline: Legitimate ops tasks classify as OPS
    Given the default workflow classifier
    When I classify "<request>"
    Then the workflow type is "OPS_WORKFLOW"

    Examples:
      | request                                             |
      | manage infrastructure for the production cluster    |
      | manage deployment of the staging environment        |
      | disk cleanup of /var/log files                      |
      | run command to restart the database service         |

  Scenario Outline: Mixed keywords with constructive verbs
    Given the default workflow classifier
    When I classify "<request>"
    Then the workflow type is "DEFAULT_WORKFLOW"

    Examples:
      | request                                              |
      | Add a cleanup feature to the admin panel             |
      | Create a disk cleanup scheduler module               |
      | Implement manage resources API endpoint              |

  Scenario: Low-confidence default for ambiguous requests
    Given the default workflow classifier
    When I classify "do something with the system"
    Then the workflow type is "DEFAULT_WORKFLOW"
    And the confidence is less than 0.7
