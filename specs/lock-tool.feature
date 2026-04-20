# Gherkin spec for lock_tool Rust port (Issue #283)
# Port of amplifier-bundle/tools/amplihack/lock_tool.py

Feature: Lock tool for continuous work mode
  As a developer using amplihack
  I want to lock/unlock continuous work mode
  So that Claude continues working without stopping

  Background:
    Given the CLAUDE_PROJECT_DIR environment variable is set
    And the lock directory is at $CLAUDE_PROJECT_DIR/.claude/runtime/locks

  Scenario: Lock creates lock file
    Given no lock is active
    When I run "amplihack lock lock"
    Then a .lock_active file is created in the lock directory
    And the file contains a "locked_at:" timestamp
    And stdout contains "Lock enabled"

  Scenario: Lock with custom message
    Given no lock is active
    When I run "amplihack lock lock --message 'finish all tests'"
    Then a .lock_active file is created
    And a .lock_message file is created with content "finish all tests"

  Scenario: Lock when already locked warns but succeeds
    Given a lock is already active
    When I run "amplihack lock lock"
    Then stdout contains "WARNING: Lock was already active"
    And the exit code is 0

  Scenario: Lock updates message when already locked
    Given a lock is already active with message "old message"
    When I run "amplihack lock lock --message 'new message'"
    Then the .lock_message file contains "new message"

  Scenario: Unlock removes lock and message files
    Given a lock is active with message "test"
    When I run "amplihack lock unlock"
    Then the .lock_active file does not exist
    And the .lock_message file does not exist
    And stdout contains "Lock disabled"

  Scenario: Unlock when not locked shows info
    Given no lock is active
    When I run "amplihack lock unlock"
    Then stdout contains "Lock was not enabled"
    And the exit code is 0

  Scenario: Check when locked shows status
    Given a lock is active
    When I run "amplihack lock check"
    Then stdout contains "Lock is ACTIVE"

  Scenario: Check when not locked shows status
    Given no lock is active
    When I run "amplihack lock check"
    Then stdout contains "Lock is NOT active"
