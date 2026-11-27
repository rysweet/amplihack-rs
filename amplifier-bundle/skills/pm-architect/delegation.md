# Delegation

## Package

Gather the backlog item and create a JSON document with these fields.

| Field | Contents |
| --- | --- |
| `backlog_item` | `id`, `title`, `description`, `priority`, and `estimated_hours` |
| `agent_role` | Agent selected for the work |
| `category`, `complexity` | Classification from [REFERENCE.md](REFERENCE.md#complexity-estimation) |
| `project_context` | Project goals and quality bar from `.pm/config.yaml` and a summary from `.pm/roadmap.md` |
| `instructions` | Steps tailored to the selected agent |
| `relevant_files` | Paths relevant to the work |
| `similar_patterns` | Existing patterns and where to find them |
| `test_requirements` | Tests needed for this category |
| `architectural_notes` | Design guidance for this complexity |
| `success_criteria` | Conditions the result must meet |

## Agent Selection

| Category | Complexity | Agent |
| --- | --- | --- |
| Feature | Any | Builder |
| Bug | Simple | Builder with a test-first approach |
| Bug | Complex | Analyzer, then builder |
| Test | Any | Tester |
| Refactor | Any | Reviewer, then builder |
| Documentation | Any | Builder with a documentation focus |
| Complex feature | Complex | Architect, then builder |

Use the complex feature row when it applies.

## Instruction Templates

### Builder

1. Read the requirements and relevant files.
2. Design the simplest solution that follows existing patterns.
3. Implement working code without stubs or placeholders.
4. Add tests covering the stated requirements.
5. Follow the architectural notes.
6. Update the documentation.

### Reviewer

1. Check the code against the project philosophy and architectural notes.
2. Look for stubs, placeholders, or dead code.
3. Check test coverage against the requirements.
4. Flag unnecessary complexity.
5. Verify that the documentation reflects the change.

### Tester

1. Read the expected behavior, contracts, and test requirements.
2. Design tests for normal behavior and edge cases.
3. Implement coverage for the stated requirements and run the tests.
4. Document the test scenarios.

## Find Relevant Files

Extract significant words from the item's title and description. Match them against file and directory names in common locations such as `src/`, `tests/`, and `.claude/tools/`. Include up to ten of the most relevant paths.

## Test Requirements

| Category | Requirements |
| --- | --- |
| Feature | Unit tests for new functions or classes, integration tests for the workflow, and coverage of success, errors, and edge cases such as empty or invalid input. |
| Bug | A regression test that fails before the fix and passes afterward, with coverage of related edge cases. |
| Refactor | Existing tests pass, behavior stays the same, and coverage is maintained or improved. |
| Test | Cover the stated requirements with clear, maintainable tests that run in under one second each. |

## Architectural Notes

| Complexity | Guidance |
| --- | --- |
| Simple | Use a single file or function when feasible and follow existing patterns. |
| Medium | Break the work into testable components and follow existing patterns. |
| Complex | Build smaller, testable components with clear module contracts and deliver changes incrementally where possible. |
