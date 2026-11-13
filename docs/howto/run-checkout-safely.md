---
title: Run a checkout build safely
description: Exercise a development CLI without publishing it to the host installation.
doc_type: howto
review_schedule: as-needed
---

# Run a checkout build safely

Use this guide when running a compiled CLI from an amplihack-rs checkout.
Checkout binaries cannot repair or publish the host installation implicitly,
even when they report a higher version than the installed release.

## Disable automatic installation

From the checkout root, with the debug CLI already built:

```sh
export AMPLIHACK_SKIP_AUTO_INSTALL=1
./target/debug/amplihack --help
```

The command prints CLI help. Subsequent commands in this shell inherit the
opt-out. Any non-empty value suppresses implicit installation; an empty or
unset value leaves the origin and version safeguards active. The opt-out and
origin restriction each prevent implicit publication independently.

Git hooks using `scripts/git-hook-cargo.sh` set the opt-out unconditionally,
including the artifact-guard invocation. Keep that wrapper in place when
running the hooks.

## Handle missing assets

A checkout command can use available assets. If installation repair is needed,
it reports the refusal and continues when its required assets are usable.
A command needing missing assets may fail with recovery guidance. Refusal
does not create installation locks, fetch sources, switch releases or repair
profiles and agent settings.

A refused or opted-out repair also permits dispatch when installation
assessment fails, for example because the version stamp or staged `.layout`
marker cannot be read. Each command still validates the assets it uses and
may fail if they are unavailable. An eligible installed executable with
automatic installation enabled continues to report these assessment errors.

Use a supported release CLI's explicit `install` or `update` to repair the
host. The [release-executable contract](../reference/install-command.md#release-binary-staging-internal)
prevents explicit install from publishing checkout executables. For local framework assets, see
[Local framework installation](local-install.md).

## Isolate subprocess tests

Use the repository's isolated Rust fixtures for checkout CLI subprocesses.
Give each fixture a private HOME, installation, agent configuration, profile,
cache and temporary root, and a controlled PATH. Clear inherited overrides
that could redirect any of these to real user directories.

Ordinary fixtures retain `AMPLIHACK_SKIP_AUTO_INSTALL=1`. Origin-regression
fixtures remove it only inside fully isolated state. Setting `AMPLIHACK_HOME`
alone does not isolate installation or profiles: installation authority and
destinations derive from HOME. Tests must never access the developer's real
`~/.claude`, `~/.amplihack`, `~/.codex` or profile root.

See [Automatic repair reference](../features/self-heal-asset-restage.md) for
eligibility, refusal and configuration details.
