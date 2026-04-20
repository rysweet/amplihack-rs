Feature: Checksum download uses retry with backoff (#257)
  As a user running `amplihack update`
  I want the SHA-256 checksum download to retry on transient failures
  So that a momentary 502 doesn't kill my update

  Scenario: Retry succeeds on second attempt
    Given the checksum URL returns HTTP 502 on the first request
    And the checksum URL returns a valid SHA-256 on the second request
    When `verify_sha256()` is called
    Then the verification succeeds
    And exactly 2 HTTP requests were made

  Scenario: Transient failure recovers within 3 attempts
    Given the checksum URL returns HTTP 503 on the first two requests
    And the checksum URL returns a valid SHA-256 on the third request
    When `verify_sha256()` is called
    Then the verification succeeds

  Scenario: Permanent failure after max retries
    Given the checksum URL returns HTTP 502 on all 3 attempts
    When `verify_sha256()` is called
    Then the verification fails with a network error
    And the error message mentions the checksum URL

  Scenario: Invalid hex digest in checksum file
    Given the checksum URL returns "not-a-hex-digest  archive.tar.gz"
    When `verify_sha256()` is called
    Then the verification fails
    And the error message mentions "valid SHA-256 hex digest"

  Scenario: Checksum mismatch detected
    Given the checksum URL returns a valid but wrong SHA-256 digest
    When `verify_sha256()` is called with different archive bytes
    Then the verification fails
    And the error message mentions "checksum mismatch"
