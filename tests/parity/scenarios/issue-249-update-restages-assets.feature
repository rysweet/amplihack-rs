Feature: Update re-stages framework assets after binary replacement (#249)
  As a user running `amplihack update`
  I want framework assets to be refreshed after the binary is replaced
  So that updated skills, recipes, and hooks are available immediately

  Scenario: Successful update re-stages assets
    Given a new version is available
    And `download_and_replace()` succeeds
    When `run_update()` completes
    Then `ensure_framework_installed()` is called after binary replacement
    And the update reports success

  Scenario: Asset re-staging failure is non-fatal
    Given a new version is available
    And `download_and_replace()` succeeds
    And `ensure_framework_installed()` fails with a transient error
    When `run_update()` completes
    Then the binary update still succeeds
    And a warning is printed suggesting `amplihack install`

  Scenario: Already at latest version skips everything
    Given the current version matches the latest release
    When `run_update()` is called
    Then no download is attempted
    And no asset re-staging is attempted
    And the message "Already at the latest version" is printed
