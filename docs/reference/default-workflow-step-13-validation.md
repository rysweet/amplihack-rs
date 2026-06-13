---
title: "Default Workflow Step 13 Validation Reference"
description: "Contract for toolchain-aware outside-in local validation in default-workflow Step 13."
last_updated: 2026-06-13
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Default Workflow Step 13 Validation Reference

> [Home](../index.md) > Reference > Default Workflow Step 13 Validation

Step 13 of `default-workflow` is the local outside-in validation gate. It
requires the acting agent to validate changes from the perspective of a user or
consumer before commit, push, and PR publication.

Step 13 is toolchain-aware. It does not prescribe one package manager, installer,
or command for every repository. The agent detects the languages, package
managers, manifests, lockfiles, changed files, and documented project commands,
then chooses the smallest validation strategy that still proves the requested
user-visible behavior works locally.

## Contract

Step 13 must produce evidence for three decisions:

| Decision | Required evidence |
| --- | --- |
| Detected toolchains | Languages, package managers, manifests, lockfiles, and entry points relevant to the change. |
| Chosen validation strategy | The outside-in commands or scenarios selected for those toolchains, with a short reason. |
| Results | Pass/fail outcome, meaningful output summary, and any follow-up fix loop performed before continuing. |

The strategy must be local, reproducible, and scoped to the change. CI-only
validation is not enough, and unit-test-only validation is not enough when a
consumer-facing path exists.

## Selection rules

1. Detect affected languages and toolchains before choosing commands.
2. Prefer documented project commands from `README`, package scripts,
   `Makefile`, CI configuration, or existing task runners.
3. Validate from a user or consumer boundary: CLI invocation, API request,
   library import, package execution, service startup, or integration scenario.
4. Include one simple scenario and, when the change has branching behavior, one
   complex or edge scenario.
5. Use the repository's local source or built artifact unless the project
   explicitly documents another local validation path.
6. Keep the validation scope narrow, but do not reduce it below the scenarios
   needed to prove the behavior a user or consumer will rely on.
7. Treat manifests, scripts, remotes, and package metadata as untrusted until
   inspected. Do not use remote code execution as a universal shortcut.
8. Record the selected commands and results for the PR description, not as a
   committed status report.

## Toolchain examples

These examples are illustrative. The acting agent chooses the exact commands
based on the repository it detects.

| Toolchain | Detection signals | Outside-in validation examples |
| --- | --- | --- |
| Rust / Cargo | `Cargo.toml`, `Cargo.lock`, `src/main.rs`, workspace members | `cargo test`, `cargo run -- <args>`, `cargo install --path . --locked` followed by invoking the installed CLI. |
| Node / npm | `package.json`, `package-lock.json`, `npm` scripts, `bin` entries | `npm test`, `npm run build`, `npm run <documented-script>`, direct `node ./bin/...` invocation for CLI behavior, or `npm link` only when local package-link behavior itself must be validated. |
| Python / uv | `pyproject.toml`, `uv.lock`, Python package entry points | `uv run pytest`, `uv run <module-or-script>`, or `uvx` only when validating a Python/uv-distributed CLI path is appropriate for the project. |
| Go | `go.mod`, `go.sum`, `cmd/` entry points | `go test ./...`, `go run ./cmd/<name> -- <args>`, `go install ./cmd/<name>` followed by invoking the built binary. |
| .NET | `.csproj`, `.sln`, `global.json`, `dotnet-tools.json` | `dotnet test`, `dotnet run --project <project> -- <args>`, `dotnet tool install --add-source <local-package-output>` for local tool-package validation. |

## Example evidence

```markdown
### Step 13 local outside-in validation

Detected toolchains:
- Rust/Cargo: `Cargo.toml`, `Cargo.lock`, CLI entry point in `src/main.rs`
- Node/npm: `package.json` script wrapping generated docs checks

Chosen strategy:
- Run Cargo tests for changed Rust behavior.
- Invoke the CLI from local source to validate the user-facing command.
- Run the documented npm docs check because the change updates generated docs.

Commands and results:
- `cargo test --workspace` -> passed
- `cargo run -- recipe show default-workflow` -> passed; Step 13 text is visible
- `npm run docs:check` -> passed
```

## Configuration

Step 13 does not require a feature-specific configuration flag. It uses the
normal `default-workflow` context:

| Context key | Required | How Step 13 uses it |
| --- | --- | --- |
| `repo_path` | Yes | Repository root for toolchain detection and local command execution. |
| `task_description` | Yes | User-facing behavior to validate outside-in. |
| Changed files from the active worktree | Yes | Scope for selecting relevant toolchains and scenarios. |

Workflow preferences may strengthen the evidence requirement, such as requiring
both a simple and complex scenario, but preferences must not force a single
toolchain command across all repositories.

## Non-goals

- Step 13 does not replace Step 12 unit tests, linters, type checks, or
  pre-commit hooks.
- Step 13 does not require every language in a polyglot repository to run when
  the change clearly affects only one isolated toolchain.
- Step 13 does not require Python tooling for non-Python projects.
- Step 13 does not require network-fetched package execution as the universal
  validation path.

## Regression expectations

Tests covering Step 13 documentation and recipe text should assert the semantic
contract rather than exact prose:

- The step preserves outside-in local validation intent.
- The step requires toolchain detection before command selection.
- The step includes generalized examples for Rust/Cargo, Node/npm, Python/uv,
  Go, and .NET.
- Python `uvx` usage is scoped to Python/uv validation only.
- The step does not present any one tool or network-fetched execution path as
  globally mandatory.
- Remote Git install phrases are absent from Step 13 guidance.

## See also

- [Default Workflow Skill](../claude/skills/default-workflow/SKILL.md)
- [Default Workflow Concept](../concepts/default-workflow.md)
- [Workflow Execution Guardrails](workflow-execution-guardrails.md)
