# Writing End-to-End Tests

When the repository already has an end-to-end suite, write new tests in that suite's framework and location. When no existing end-to-end suite exists, use the [Repo-Type Detection table](SKILL.md#repo-type-detection) to choose tooling.

| App type | Preferred output |
| --- | --- |
| Web | Playwright specs |
| CLI | Shell, Bats, or existing Cargo, npm, or .NET test harnesses |
| TUI | Terminal interaction tests using repository conventions |
| API | HTTP or client tests in the repository's framework |
| MCP | YAML scenarios where the repository already uses them |

## Workflow

1. Detect the application type from manifests, routes, binaries, documentation, and existing tests.
2. Identify three to eight critical user journeys and their expected outcomes.
3. Write the scenarios, using `e2e/` if the repository has no location convention.
4. Exercise real user entry points wherever possible.
5. Run the repository's test command and fix failures caused by the new tests.
6. Report product bugs separately from harness problems.

## Output Requirements

- Provide runnable tests and the command that runs them.
- Assert observable behavior at the user entry point.
- Add a framework only when no existing end-to-end suite exists and the selected tooling requires one.
- Document each unsupported journey and explain why it is unsupported.
