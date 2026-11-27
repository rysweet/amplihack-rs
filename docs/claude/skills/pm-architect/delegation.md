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

1. Analyze requirements and examine relevant files listed below
2. Design solution following existing patterns
3. Implement working code (no stubs or placeholders)
4. Add comprehensive tests per test requirements
5. Follow architectural notes
6. Update documentation

Focus on ruthless simplicity. Start with simplest solution that works.

### Reviewer

1. Review code for philosophy compliance
2. Verify no stubs, placeholders, or dead code
3. Check test coverage against requirements
4. Validate architectural notes followed
5. Look for unnecessary complexity
6. Ensure documentation updated

Focus on ruthless simplicity and zero-BS implementation.

### Tester

1. Analyze behavior and contracts
2. Review test requirements below
3. Design tests for edge cases
4. Implement comprehensive coverage
5. Verify all tests pass
6. Document test scenarios

Focus on testing behavior, not implementation details.

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
