# Gherkin spec for session_tree Rust port (Issue #283)
# Port of amplifier-bundle/tools/session_tree.py

Feature: Session tree management for orchestration recursion prevention
  As an orchestration system
  I want to track session trees with depth and capacity limits
  So that infinite recursion and session storms are prevented

  Background:
    Given the state directory is at $TMPDIR/amplihack-session-trees
    And the state directory has permissions 0700

  Scenario: Check allows spawning when no tree exists
    Given AMPLIHACK_TREE_ID is not set
    When I run "amplihack session-tree check"
    Then stdout is "ALLOWED"

  Scenario: Check blocks at max depth
    Given AMPLIHACK_SESSION_DEPTH is "3"
    And AMPLIHACK_MAX_DEPTH is "3"
    When I run "amplihack session-tree check"
    Then stdout starts with "BLOCKED:"

  Scenario: Register creates new tree
    Given AMPLIHACK_TREE_ID is not set
    When I run "amplihack session-tree register test-sess"
    Then stdout matches "TREE_ID=\w+ DEPTH=0"
    And a state file is created for the tree

  Scenario: Register with parent links child
    Given a tree with session "parent" exists
    When I run "amplihack session-tree register child parent"
    Then the parent session's children list contains "child"

  Scenario: Register fails at max sessions
    Given a tree with max_sessions=2 and 2 active sessions
    When I run "amplihack session-tree register new-sess"
    Then stderr contains "max_sessions"
    And the exit code is 1

  Scenario: Complete marks session done
    Given a tree with active session "my-sess"
    When I run "amplihack session-tree complete my-sess"
    Then the session status is "completed"
    And completed_at is set

  Scenario: Status shows tree details as JSON
    Given a tree with active and completed sessions
    When I run "amplihack session-tree status <tree_id>"
    Then stdout is valid JSON with tree_id, active, completed, depths

  Scenario: Stale sessions are pruned on save
    Given a completed session older than 24 hours
    When the state is saved
    Then the stale session is removed

  Scenario: Leaked active sessions are pruned
    Given an active session older than 4 hours
    When the state is saved
    Then the leaked session is removed with a warning

  Scenario: Tree ID validation rejects path traversal
    Given a tree_id of "../etc/passwd"
    When I validate the tree_id
    Then it is rejected with "Invalid tree_id"

  Scenario: Concurrent registrations are serialized
    Given 5 threads registering sessions simultaneously
    When all threads complete
    Then all 5 sessions are present in the state file
