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
- [Startup Self-Update Prompt — Subprocess-Safe Skip](startup-update-prompt-subprocess-safe.md) — startup self-update prompt skips automatically in CI, delegated agents, non-TTY stdin, with `--subprocess-safe`, or when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set; emits a single skip-line to stderr (issue [#625](https://github.com/rysweet/amplihack-rs/issues/625)).

## Workflow Recovery

- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md) — recover existing pull requests through `default-workflow`, reuse the PR branch, verify hook and additive-copy readiness, and finalize only through workflow-owned steps.

## GitHub Distribution

- [GitHub Distribution](github-distribution.md) — publish agent bundles to GitHub repositories via the `gh` CLI, with idempotent uploads, visibility control, and tagged releases.

## Additional Features

Additional feature documentation will be added as features are ported from upstream.
