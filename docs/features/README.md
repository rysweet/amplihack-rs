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

## Workflow Recovery

- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md) — recover existing pull requests through `default-workflow`, reuse the PR branch, verify hook and additive-copy readiness, and finalize only through workflow-owned steps.

## GitHub Distribution

- [GitHub Distribution](github-distribution.md) — publish agent bundles to GitHub repositories via the `gh` CLI, with idempotent uploads, visibility control, and tagged releases.

## Additional Features

Additional feature documentation will be added as features are ported from upstream.
