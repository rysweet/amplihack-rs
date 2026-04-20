# Gherkin spec for validate_frontmatter Rust port (Issue #283)
# Validates YAML frontmatter in skill/agent markdown files

Feature: Validate YAML frontmatter in markdown files
  As a skill/agent developer
  I want to validate frontmatter in my markdown files
  So that malformed files are caught before deployment

  Scenario: Valid frontmatter passes
    Given a markdown file with valid YAML frontmatter
    When I run "amplihack validate-frontmatter --file path/to/file.md"
    Then the exit code is 0
    And stdout indicates success

  Scenario: Missing frontmatter fails
    Given a markdown file without frontmatter
    When I run "amplihack validate-frontmatter --file path/to/file.md"
    Then the exit code is non-zero
    And stderr describes the error

  Scenario: Invalid YAML fails
    Given a markdown file with invalid YAML frontmatter
    When I run "amplihack validate-frontmatter --file path/to/file.md"
    Then the exit code is non-zero

  Scenario: Scan directory validates all markdown files
    Given a directory with multiple markdown files
    When I run "amplihack validate-frontmatter"
    Then all .md files in the directory are validated
