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
- [Startup Self-Update Prompt — Subprocess-Safe Skip](startup-update-prompt-subprocess-safe.md) — startup self-update prompt skips automatically in CI, delegated agents, non-TTY stdin, with `--subprocess-safe`, or when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set; emits a single skip-line to stderr (issue [#625](https://github.com/rysweet/amplihack-rs/issues/625)).

## Workflow Recovery

- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md) — recover existing pull requests through `default-workflow`, reuse the PR branch, verify hook and additive-copy readiness, and finalize only through workflow-owned steps.
- [Existing Branch Finalization Runbook](post-v0977-finalization.md) — inspect,
  preserve, validate, publish, and merge an already-implemented branch; includes
  the post-v0.9.77 issue-658 branch as the concrete example.
- [Recipe Subprocess and Hook Input Contracts](../howto/validate-recipe-subprocess-hook-contract.md) — recipe-runner child environment and hook input compatibility contract; see the [recipe environment reference](../reference/recipe-executor-environment.md#recipe-runner-subprocess-launch) and [hook input contract](../reference/hook-specifications.md#hook-input-json-contract).

## GitHub Distribution

- [GitHub Distribution](github-distribution.md) — publish agent bundles to GitHub repositories via the `gh` CLI, with idempotent uploads, visibility control, and tagged releases.

## Additional Features

- [amplihack-rs Parity Reference](../amplihack-rs-parity.md) - subprocess prompt delivery configuration, binary capability matrix, doctor diagnostics, and Rust API.
