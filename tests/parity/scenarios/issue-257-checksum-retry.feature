Feature: SHA-256 checksum verification uses retry logic (#257)
  The verify_sha256() function must use http_get_with_retry() instead of
  http_get() when fetching the checksum file. This prevents transient
  network errors from aborting an otherwise successful update.

  Scenario: Checksum download succeeds on first attempt
    Given a valid archive has been downloaded
    And the checksum URL is reachable
    When verify_sha256 is called
    Then the checksum file is fetched with retry support
    And the SHA-256 hash matches

  Scenario: Checksum download fails transiently then succeeds
    Given a valid archive has been downloaded
    And the checksum URL returns HTTP 503 on the first attempt
    And the checksum URL returns the correct checksum on the second attempt
    When verify_sha256 is called
    Then the function retries the request
    And the SHA-256 hash matches
    And the update proceeds

  Scenario: Checksum download fails permanently
    Given a valid archive has been downloaded
    And the checksum URL is unreachable after all retry attempts
    When verify_sha256 is called
    Then the function returns an error mentioning the checksum URL
    And the update is aborted

  Scenario: Checksum file contains invalid hex
    Given a valid archive has been downloaded
    And the checksum URL returns "not-a-hex-digest  filename.tar.gz"
    When verify_sha256 is called
    Then the function returns an error mentioning "valid SHA-256 hex digest"

  Scenario: Checksum mismatch detected
    Given a valid archive has been downloaded
    And the checksum URL returns a hex digest that does not match
    When verify_sha256 is called
    Then the function returns an error mentioning "checksum mismatch"
    And the error includes both expected and actual digests
