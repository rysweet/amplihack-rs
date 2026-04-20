Feature: Update re-stages framework assets after binary swap (#249)
  After downloading and replacing the binary, run_update() must call
  ensure_framework_installed() to re-stage assets from amplifier-bundle.
  Without this, the new binary runs with stale assets.

  Scenario: Successful update refreshes framework assets
    Given the current version is older than the latest release
    And the download and binary replacement succeeds
    When run_update() completes
    Then ensure_framework_installed() is called
    And framework assets in ~/.amplihack/.claude/ are updated

  Scenario: Binary update succeeds but asset refresh fails
    Given the binary swap completes successfully
    And ensure_framework_installed() returns an error
    When run_update() completes
    Then a warning is printed mentioning "framework asset refresh failed"
    And the warning suggests running "amplihack install" manually
    And run_update() does NOT return an error (the binary swap is still valid)

  Scenario: Update when already at latest version
    Given the current version equals the latest release
    When run_update() is called
    Then it prints "Already at the latest version"
    And ensure_framework_installed() is NOT called
    And no binary swap occurs
