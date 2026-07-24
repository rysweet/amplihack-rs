# Features Documentation

> [Home](../index.md) > Features

This section documents amplihack-rs feature implementations.

## Power Steering

Intelligent guidance system that prevents common mistakes and ensures work completeness:

- [Overview](power-steering/README.md)
- [Architecture Refactor](power-steering/architecture-refactor.md)
- [Configuration](power-steering/configuration.md)
- [Customization Guide](power-steering/customization-guide.md)
- [Worktree Support](power-steering/worktree-support.md)
- [Troubleshooting](power-steering/troubleshooting.md)

## Self-Heal

- [Auto-Restage Framework Assets on Version Change](self-heal-asset-restage.md) — startup-time version-stamp check that re-runs `amplihack install` automatically when the binary version no longer matches `~/.amplihack/.installed-version`.

## Update Check

- [Post-Update Install — Re-exec New Binary](update-reexec-new-binary.md) — after `amplihack update` downloads a new binary, the post-update install step spawns the **new** binary as a subprocess instead of running the old binary's compiled-in install code (issue [#683](https://github.com/rysweet/amplihack-rs/issues/683)).
- [Install/Update PATH Conflict Handling](../reference/install-update-path-conflicts.md) — detects stale system binaries that shadow `~/.local/bin`, prefers safe user-local update targets, and reports manual sudo repair guidance without attempting privileged writes.
- [Framework Bundle Compatibility](../reference/framework-bundle-compatibility.md) — validates smart-orchestrator source and staged bundles so stale monolithic recipes cannot survive install/update repair (issue [#734](https://github.com/rysweet/amplihack-rs/issues/734)).
- [Startup Self-Update Prompt — Subprocess-Safe Skip](startup-update-prompt-subprocess-safe.md) — startup self-update prompt skips automatically in CI, delegated agents, non-TTY stdin, with `--subprocess-safe`, or when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set; emits a single skip-line to stderr (issue [#625](https://github.com/rysweet/amplihack-rs/issues/625)).

## Workflow Recovery

- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md) — recover existing pull requests through `default-workflow`, reuse the PR branch, verify hook and additive-copy readiness, and finalize only through workflow-owned steps.
- [Ancestry-Aware Step 15 Publish](workflow-publish-ancestry-aware-step15.md) — `workflow-publish` step 15 integrates with its upstream by branch ancestry: fast-forward when `behind == 0`, fail closed with structured `ahead=`/`behind=`/`merge_base=` evidence when histories diverge, and never blind-rebase already-integrated commits. Includes the PR #980 brick-rule cleanup that trims `workflow-publish.yaml` back under the 400-line limit (issue [#978](https://github.com/rysweet/amplihack-rs/issues/978)).
- [Existing Branch Finalization Runbook](post-v0977-finalization.md) — inspect,
  preserve, validate, publish, and merge an already-implemented branch; includes
  the post-v0.9.77 issue-658 branch as the concrete example.
- [Recipe Subprocess and Hook Input Contracts](../howto/validate-recipe-subprocess-hook-contract.md) — recipe-runner child environment and hook input compatibility contract; see the [recipe environment reference](../reference/recipe-executor-environment.md#recipe-runner-subprocess-launch) and [hook input contract](../reference/hook-specifications.md#hook-input-json-contract).
- [Workflow Provider Abstraction](workflow-provider-abstraction.md) — provider-neutral tracking, change-request publication, terminal state, and stale cleanup through typed helpers and provider adapters.
- [Provider-Aware Workflow Tracking](dual-provider-workflow.md) — compatibility entry point for the provider-neutral workflow contract.
- [Non-Fatal Documentation Review Checkpoint](doc-review-non-fatal-checkpoint.md) — a failed `step-06b-documentation-review` no longer reports a generic FAILURE after durable side effects (pushed commit, opened/merged PR, posted review thread) already landed; the workflow checkpoints partial success, surfaces the artifact references, and ends in a degraded-success state with a `WARNING` + `NEEDS_ATTENTION` follow-up (issue [#834](https://github.com/rysweet/amplihack-rs/issues/834)).

## Install Provisioning

- [Best-Effort Mermaid CLI Provisioning](mermaid-cli-best-effort-install.md) — `amplihack install` provisions the Mermaid CLI (`mmdc`, npm `@mermaid-js/mermaid-cli`) on a best-effort basis so the `pr-guide` skill can render mermaid diagrams to images locally for Azure DevOps instead of relying on the third-party `mermaid.ink` service. A failed or skipped install warns and continues (never blocks install); disable entirely with `AMPLIHACK_SKIP_MMDC=1` (issue [#828](https://github.com/rysweet/amplihack-rs/issues/828)).

## Workflow Guardrails

- [Skill-to-Agent Redirect](skill-to-agent-redirect.md) — the `PreToolUse` hook intercepts a `Skill` call naming an agent-only target (for example `prompt-writer`), blocks it with a non-fatal redirect to agent execution, and prevents the copilot runtime's `Skill not found` abort from silently skipping the requirements-clarification phase (issue [#838](https://github.com/rysweet/amplihack-rs/issues/838)). See the [API reference](../reference/skill-agent-redirect-api.md).

## Signal Channel

- [Signal Channel](../signal-channel.md) — feature-gated (`signal`, default OFF) per-session Signal messaging channel. Each session opens a private Signal group, posts throttled progress updates, and lets an allow-listed operator send **advisory** instructions back into the run. Inbound text is surfaced only as `hookSpecificOutput.additionalContext` and is never auto-executed; the gate is fail-closed (allowlist + `device == 1` + `groupId` match + bounded-TTL echo suppression). Wired through `amplihack-hooks` (SessionStart/PostToolUse/UserPromptSubmit/Stop) with a detached `signal-subscriber` process. Config is env-first with no silent defaults; see [`examples/signal-config.toml`](../../examples/signal-config.toml).
- Setup / onboarding: [Signal Onboarding](../SIGNAL_ONBOARDING.md) — step-by-step how-to for configuring one host with `amplihack signal setup` and distributing the config across a fleet with `amplihack signal distribute`.

## GitHub Distribution

- [GitHub Distribution](github-distribution.md) — publish agent bundles to GitHub repositories via the `gh` CLI, with idempotent uploads, visibility control, and tagged releases.

## Additional Features

- [amplihack-rs Parity Reference](../amplihack-rs-parity.md) - subprocess prompt delivery configuration, binary capability matrix, doctor diagnostics, and Rust API.
