# amplihack-rs Documentation

amplihack-rs is the Rust implementation of the amplihack CLI. It replaces the Python-based installer with a native binary that bootstraps the complete amplihack environment in a single command.

## Contents

### Tutorials

- [Hive Mind — Getting Started](./tutorials/hive-mind-getting-started.md) — Fastest path from a clean checkout to a real distributed hive eval using `amplihack hive`
- [Hive Mind — Tutorial](./tutorials/hive-mind-tutorial.md) — Federated hive mind walkthrough: in-memory, federation, LearningAgent integration, distributed agents, Azure deployment, eval

### How-To Guides

- [Install amplihack for the First Time](./howto/first-install.md) — Bootstrap from scratch, including optional legacy-Python checks, binary deployment, and hook registration
- [Install from a Local Repository](./howto/local-install.md) — Install without network access using a local checkout
- [Uninstall amplihack](./howto/uninstall.md) — Cleanly remove all installed files, binaries, and hook registrations
- [Resolve LadybugDB Linker Errors](./howto/resolve-kuzu-linker-errors.md) — Diagnose and fix `undefined reference` errors caused by `cxx`/`cxx-build` version mismatch
- [Fix the cxx-build Pin CI Failure](./howto/fix-cxx-build-ci-failure.md) — Restore the `Cargo.lock` pin when the `Verify cxx-build pin` CI step fails
- [Enable Shell Completions](./howto/enable-shell-completions.md) — Install tab-completion for bash, zsh, fish, and PowerShell
- [Run amplihack in Non-interactive Mode](./howto/run-in-noninteractive-mode.md) — Use amplihack in CI pipelines, Docker containers, and piped scripts without interactive prompts
- [Manage Tool Update Notifications](./howto/manage-tool-update-checks.md) — Control or disable the pre-launch npm update check for `claude`, `copilot`, and `codex`
- [Run a Recipe End-to-End](./howto/run-a-recipe.md) — Find, inspect, dry-run, and execute YAML recipes through the Rust CLI
- [Index a Project with the Native SCIP Pipeline](./howto/index-a-project.md) — Build the LadybugDB code-graph from source using native SCIP indexers
- [Validate No-Python Compliance](./howto/validate-no-python.md) — Run the AC9 probe to confirm the binary operates without a Python interpreter
- [Use the Fleet Dashboard](./howto/use-fleet-dashboard.md) — Open the cockpit, start and adopt sessions, search sessions, run the reasoner from the TUI, and exit cleanly
- [Run Fleet Scout and Advance on Azure VMs](./howto/run-fleet-scout-and-advance.md) — Discover sessions across VMs, reason about them with the LLM backend, and execute recommended actions
- [Migrate Memory to the SQLite Backend](./howto/migrate-memory-backend.md) — Export hierarchical memory to portable JSON, switch to SQLite, and verify the migration
- [Agent Memory Quickstart](./howto/agent-memory-quickstart.md) — Inspect the memory graph, generate a memory-enabled agent with `amplihack new --enable-memory`, and use `memory tree`/`export`/`import`/`clean`
- [Watch CI and Auto-Merge a Pull Request](./howto/watch-and-merge-pr.md) — Poll CI checks on a PR and merge automatically when all checks pass
- [Troubleshoot Recipe Execution Failures](./howto/troubleshoot-recipe-execution.md) — Diagnose shell step hangs, agent context issues, missing prerequisites, and workflow misclassification
- [Skip Pre-Agent Validation on Large Codebases](./howto/skip-pre-agent-validation.md) — Prevent slow `npm test` / `npm run build` from blocking agent work in smart-orchestrator and default-workflow
- [Diagnose Problems with amplihack doctor](./howto/diagnose-with-doctor.md) — Run system health checks and fix failing prerequisites
- [Create a Custom Agent](./howto/create-custom-agent.md) — Build a domain agent with memory integration and evaluation
- [Run Agent Evaluations](./howto/run-agent-evaluations.md) — Evaluate agent performance across progressive difficulty levels
- [Deploy a Hive Swarm](./howto/deploy-hive-swarm.md) — Deploy multi-agent hive on Azure Container Apps
- [Generate an Agent from a Goal](./howto/generate-agent-from-goal.md) — Create specialized agents from natural-language descriptions

### Reference

- [amplihack install](./reference/install-command.md) — Full CLI reference for the `install` and `uninstall` commands
- [Install Manifest](./reference/install-manifest.md) — Schema and semantics of the uninstall manifest written at install time
- [Hook Specifications](./reference/hook-specifications.md) — Canonical table of all 7 Claude Code hooks registered by amplihack
- [Binary Resolution](./reference/binary-resolution.md) — How `amplihack` locates the `amplihack-hooks` binary at install time
- [amplihack completions](./reference/completions-command.md) — Full CLI reference for the `completions` subcommand
- [Environment Variables](./reference/environment-variables.md) — All environment variables read or injected by `amplihack` during a launch
- [Launch Flag Injection](./reference/launch-flag-injection.md) — How `amplihack` builds the subprocess command line: `--dangerously-skip-permissions`, `--model`, and extra args passthrough
- [Signal Handling and Exit Codes](./reference/signal-handling.md) — SIGINT, SIGTERM, SIGHUP behavior and exit code contract (Python parity)
- [amplihack recipe](./reference/recipe-command.md) — Full CLI reference for `recipe list`, `recipe show`, `recipe validate`, and `recipe run`
- [Recipe Executor Environment](./reference/recipe-executor-environment.md) — Environment variables, prerequisite checks, and context propagation for recipe steps
- [step-03-create-issue: Idempotency Guards](./reference/recipe-step-03-idempotency.md) — Reference and title-search guards that deduplicate issue creation in `default-workflow.yaml`
- [step-04-setup-worktree: Re-Prune After Orphan Cleanup](./reference/recipe-step-04-worktree-reattach-prune.md) — `git worktree prune` semantics for the three-state idempotency guard
- [worktree_setup Context Propagation](./reference/worktree-setup-propagation.md) — How `worktree_setup` flows through the composable default-workflow recipe chain to sub-recipes
- [Workflow Classifier](./reference/workflow-classifier.md) — Keyword tables, classification algorithm, and constructive-verb disambiguation
- [skip_pre_agent_validation](./reference/skip-pre-agent-validation.md) — Recipe context variable controlling pre-agent project validation in workflow-prep step-01
- [Issue Deduplication](./reference/issue-dedup.md) — Idempotency guards, dedup decision tree, and proposed fingerprint algorithm for avoiding duplicate GitHub issues
- [Launch Flag Matrix](./reference/flag-matrix.md) — Per-tool capability matrix for `--dangerously-skip-permissions`, `--model`, `--allow-all`, and proposed type-safe refactoring
- [Parity Test Scenarios](./reference/parity-test-scenarios.md) — Every parity tier file, its test cases, and expected Python↔Rust divergence
- [amplihack index-code and index-scip](./reference/memory-index-command.md) — Full CLI reference for code-graph ingestion commands
- [amplihack query-code](./reference/query-code-command.md) — Full CLI reference for querying the native LadybugDB code-graph
- [amplihack fleet](./reference/fleet-command.md) — Full CLI reference for the fleet dashboard: key bindings, refresh architecture, persistent state schema, and security properties
- [Memory Backend](./reference/memory-backend.md) — `BackendChoice` values, env vars, flat and hierarchical schema, transfer formats, and security properties
- [amplihack doctor](./reference/doctor-command.md) — Full CLI reference for the `doctor` subcommand
- [amplihack pr watch-and-merge](./reference/pr-watch-and-merge-command.md) — Full CLI reference for watching CI checks and auto-merging pull requests
- [Agent Configuration](./reference/agent-configuration.md) — Complete configuration reference for agents, memory, eval, and hive
- [amplihack-agent-core API](./reference/agent-core-api.md) — Agent lifecycle, session management, and OODA loop
- [amplihack-domain-agents API](./reference/domain-agents-api.md) — Teaching, code review, and meeting synthesizer agents
- [amplihack-agent-eval API](./reference/agent-eval-api.md) — Progressive evaluation framework and graders
- [amplihack-hive API](./reference/hive-api.md) — Multi-agent orchestration and workload management
- [amplihack-agent-generator API](./reference/agent-generator-api.md) — Goal-to-agent pipeline
- [amplihack-memory Extended API](./reference/memory-extended-api.md) — Memory facade, manager, LadybugDB store, and evaluation
- [LadybugDB Reference](./reference/ladybug-reference.md) — `lbug` crate API surface: schema, node/edge CRUD, querying, import/export, concurrency, and security

### Concepts

- [Bootstrap Parity](./concepts/bootstrap-parity.md) — Why the Rust CLI replicates the Python installer's first-install flow and what that means for users
- [Idempotent Installation](./concepts/idempotent-installation.md) — How repeated installs are safe and how existing hook registrations are updated in place
- [The cxx/cxx-build Version Contract](./concepts/cxx-version-contract.md) — Why `cxx` and `cxx-build` must share the same minor version and how a mismatch produces linker errors
- [Agent Binary Routing](./concepts/agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` lets the recipe runner and hooks call back into the correct AI tool
- [LadybugDB Code Graph](./concepts/kuzu-code-graph.md) — Architecture of the native code-graph store: schema, SCIP pipeline, blarify consumption, and security model
- [Memory Backend Architecture](./concepts/memory-backend-architecture.md) — Backend-neutral trait seams, auto-detection order, SQLite vs. graph-db storage layout, and the transfer layer
- [Fleet Dashboard Architecture](./concepts/fleet-dashboard-architecture.md) — Thread model, state design, persistence layer, terminal safety, and security rationale for `amplihack fleet`
- [Fleet Admiral Reasoning Engine](./concepts/fleet-admiral-reasoning.md) — How the LLM-backed reasoner works: what it sees, the five actions, confidence scoring, failure modes, and design rationale
- [Fleet State Machine](./concepts/fleet-state-machine.md) — Fleet lifecycle states and transitions managed by `amplihack fleet`
- [Signal Handling Lifecycle](./concepts/signal-handling-lifecycle.md) — OS signal handling across the CLI, fleet manager, and child processes
- [Recipe Execution Flow](./concepts/recipe-execution-flow.md) — How recipes are loaded, validated, and executed step-by-step
- [Memory Backend Migration](./concepts/memory-backend-migration.md) — Migrating agent memory between SQLite and LadybugDB backends
- [Agent Lifecycle](./concepts/agent-lifecycle.md) — Agent state machine, session management, OODA loop, and subprocess isolation
- [Domain Agents](./concepts/domain-agents.md) — Specialized goal-seeking agents: teaching, code review, meeting synthesis
- [Evaluation Framework](./concepts/eval-framework.md) — Progressive L1–L12 evaluation, graders, and self-improvement
- [Hive Orchestration](./concepts/hive-orchestration.md) — Multi-agent swarm deployment, events, and workload management
- [Hive Mind Design](./concepts/hive-mind-design.md) — Layered architecture (storage / transport / discovery / query) and module reference for the unified hive mind
- [Hive Mind Eval](./concepts/hive-mind-eval.md) — How the distributed hive evaluation harness fits together (local wrappers, Azure runs, question sets)
- [Goal Agent Generator](./concepts/agent-generator.md) — Four-stage pipeline: analyze → plan → synthesize → assemble
- [Recipe Runner Architecture](./concepts/recipe-runner-architecture.md) — Why the runner is an external binary, resolution logic, invocation contract, and Python coexistence
- [Agentic Step Patterns](./concepts/agentic-step-patterns.md) — Decision rules for bash vs agent vs recipe step types, prompt writing guide, and anti-patterns
- [Smart-Orchestrator Recovery](./concepts/smart-orchestrator-recovery.md) — Failure taxonomy, four-stage recovery pipeline, hollow-success detection, and goal status values
- [amplihack Retirement Direction](./concepts/amplihack-retirement-direction.md) — Python-to-Rust migration status, compatibility guarantees, and milestone tracking
- [Memory Tree](./concepts/memory-tree.md) — Mental model and CLI surface for the `amplihack memory tree` command
- [Five-Type Memory (Superseded)](./concepts/five-type-memory.md) — Redirect from the legacy 5-type memory guide to current memory documentation

## Quick Start

```sh
# Build from source
cargo build --release

# Install amplihack (first time)
~/.cargo/bin/amplihack install

# Install from a local clone (no network)
amplihack install --local /path/to/amplihack-clone

# Remove everything amplihack installed
amplihack uninstall
```

### Contributing

- [File Organization](./contributing/file-organization.md) — Where files go in the repository
- [Documentation Parity Audit](./contributing/documentation-parity-audit.md) — How upstream docs were audited and ported, and how to maintain parity

## Related

- [README](https://github.com/rysweet/amplihack-rs/blob/main/README.md) — Architecture overview and design principles
- [CONTRIBUTING_RUST.md](https://github.com/rysweet/amplihack-rs/blob/main/CONTRIBUTING_RUST.md) — Developer setup, build targets, test harness
