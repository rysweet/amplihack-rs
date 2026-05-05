---
name: e2e-outside-in-test-generator
version: 0.3.0
description: |
  Generates comprehensive end-to-end test scenarios using outside-in methodology.
  Supports Web (Playwright), CLI, TUI, API, and MCP/gadugi-style scenarios.
  Auto-detects app type or accepts explicit override.
activation_keywords:
  - "add e2e tests"
  - "add playwright tests"
  - "add browser tests"
  - "add outside-in tests"
  - "generate e2e suite"
  - "add cli tests"
  - "add tui tests"
  - "add api tests"
  - "add mcp tests"
  - "generate test scenarios"
category: testing
requires: []
invokes:
  - test-gap-analyzer
  - shadow-testing
  - qa-team
output_location: e2e/
---

# E2E Outside-In Test Generator

## Purpose

Generate runnable end-to-end tests from the user's point of view. The bundled helper creates a starter outside-in plan or seed scenario; the agent then adapts that output to the repository's existing test framework and validates it with normal test commands.

## Supported App Types

| App type | Preferred output |
| --- | --- |
| Web | Playwright specs |
| CLI | Shell, bats, cargo/npm/dotnet test harnesses already present |
| TUI | Terminal interaction tests using existing repo conventions |
| API | HTTP/client tests in the repo's existing test framework |
| MCP | YAML scenario tests when the repo already uses them |

## Workflow

1. Detect the app type from package manifests, routes, binaries, docs, and existing tests, or run the helper.
2. Identify 3-8 critical user journeys and expected outcomes.
3. Generate tests in the repository's existing e2e/test location, or `e2e/` if there is no convention.
4. Prefer real user entry points over mocked internals.
5. Run the relevant existing test command and fix failures caused by the new tests.
6. Report any product bugs found separately from test harness problems.

## Output Requirements

- Tests must be runnable by existing tooling.
- Generated assertions must validate observable behavior, not implementation details.
- Do not add a new test framework unless the repository has no suitable e2e path.
- Do not silently skip journeys; document unsupported journeys and why.

## Native Helper

```bash
# Inspect detected app type and journey plan
amplifier-bundle/skills/e2e-outside-in-test-generator/scripts/generate-e2e.sh plan

# Generate a starter scenario/spec
amplifier-bundle/skills/e2e-outside-in-test-generator/scripts/generate-e2e.sh generate --type web --output e2e
amplifier-bundle/skills/e2e-outside-in-test-generator/scripts/generate-e2e.sh generate --type cli --output e2e
```

The helper intentionally creates starter artifacts only. The agent must adapt the scenarios to real project commands, routes, and assertions before claiming completion.
